//! Bounded loading of trusted, non-executing MonoBehaviour type-tree documents.
//!
//! A document supplies a complete serialized object layout, including Unity's
//! MonoBehaviour header. It does not load the assembly named by an identity.
//! Embedded trees remain authoritative; among external entries an exact Unity
//! version outranks a version-independent fallback and the first equal entry
//! keeps priority. JSON documents are not a replacement for DLL reconstruction.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use unity_rs_core::mono_schema::{
    MonoBehaviourSchemaDocumentLimits, MonoBehaviourSchemaIdentity, MonoBehaviourSchemaProvider,
    MonoBehaviourSchemaRegistry,
};

pub const MAX_GENERATOR_REQUEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_GENERATOR_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_GENERATOR_TARGETS: usize = 10_000;
const MAX_GENERATOR_STDERR_BYTES: usize = 64 * 1024;
const GENERATOR_TIMEOUT: Duration = Duration::from_secs(60);

/// A script layout required by a stripped object, never executable code.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSchemaTarget {
    pub unity_version: String,
    pub assembly: String,
    pub namespace: String,
    pub class_name: String,
}

/// Versioned protocol between the application and its isolated Rust helper.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSchemaRequest {
    pub version: u32,
    pub dll_directory: PathBuf,
    pub targets: Vec<ManagedSchemaTarget>,
}

impl ManagedSchemaRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unknown managed-schema request version");
        ensure!(
            self.targets.len() <= MAX_GENERATOR_TARGETS,
            "Too many managed schema targets"
        );
        ensure!(
            self.dll_directory.as_os_str().len() <= 16 * 1024,
            "Managed assembly folder path is too long"
        );
        ensure!(
            self.dll_directory.is_dir(),
            "Managed assembly folder does not exist"
        );
        for target in &self.targets {
            ensure!(
                !target.assembly.is_empty() && !target.class_name.is_empty(),
                "Managed script identity is incomplete"
            );
            for string in [
                &target.unity_version,
                &target.assembly,
                &target.namespace,
                &target.class_name,
            ] {
                ensure!(
                    string.len() <= 4096,
                    "Managed script identity exceeds its string limit"
                );
            }
            let version: unity_rs_core::unity_version::UnityVersion =
                target.unity_version.parse()?;
            ensure!(
                version.major >= 5 && !version.is_tuanjie(),
                "Native DLL schema generation requires standard Unity 5 or newer"
            );
            ensure!(
                !target.assembly.contains(['/', '\\', ':', '\0'])
                    && target.assembly != "."
                    && target.assembly != "..",
                "Managed assembly identity must be one file name"
            );
        }
        Ok(())
    }
}

impl crate::Session {
    /// Adds schemas under shared retained budgets; prior matching entries win.
    pub fn add_schema_registry(&mut self, registry: MonoBehaviourSchemaRegistry) -> Result<usize> {
        merge_registry(&mut self.mono_schemas, registry)
    }

    /// Reconstructs required script layouts in a short-lived native Rust child.
    pub fn load_dummy_dlls(&mut self, folder: &Path, cli_executable: &Path) -> Result<usize> {
        self.protect_input_path(folder)?;
        let registry = generate_registry_via_cli(self, folder, cli_executable)?;
        self.add_schema_registry(registry)
    }
}

pub fn merge_registry(
    existing: &mut MonoBehaviourSchemaRegistry,
    incoming: MonoBehaviourSchemaRegistry,
) -> Result<usize> {
    let limits = SchemaLimits::default().document;
    SchemaUsage::of(existing, limits)?
        .add(SchemaUsage::of(&incoming, limits)?)?
        .remaining(limits)?;
    let count = incoming.entries().len();
    existing.extend(incoming)?;
    Ok(count)
}

/// The packaged CLI is the DLL-schema helper used by the native desktop app.
pub fn sibling_cli_executable() -> Result<PathBuf> {
    let path = std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "AssetStudioCLI.exe"
    } else {
        "AssetStudioCLI"
    });
    ensure!(
        path.is_file(),
        "AssetStudioCLI must be installed beside the desktop executable for DLL schema recovery"
    );
    Ok(path)
}

/// Resolves actual m_Script references instead of guessing every assembly type.
pub fn managed_targets(session: &crate::Session) -> Result<Vec<ManagedSchemaTarget>> {
    use unity_rs_core::{
        monobehaviour::{MonoBehaviourReadLimits, read_mono_behaviour, read_mono_script},
        scene::resolve_object_reference,
        serialized::ObjectReference,
    };
    let collection = session.studio.collection();
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for (file_index, loaded) in collection.serialized_files().iter().enumerate() {
        for (object_index, object) in loaded.file.objects.iter().enumerate() {
            if object.class_id != 114 {
                continue;
            }
            let embedded = object
                .serialized_type_index
                .and_then(|index| loaded.file.types.get(index))
                .and_then(|kind| kind.type_tree.as_ref())
                .is_some_and(|tree| !tree.nodes.is_empty());
            if embedded {
                continue;
            }
            let limits = MonoBehaviourReadLimits {
                maximum_string_bytes: 4096,
                maximum_total_string_bytes: 16 * 1024,
                ..MonoBehaviourReadLimits::default()
            };
            let behaviour = read_mono_behaviour(&loaded.file, object_index, limits)?;
            let script = resolve_object_reference(
                collection,
                file_index,
                ObjectReference {
                    file_id: behaviour.script.file_id,
                    path_id: behaviour.script.path_id,
                },
            )?
            .context("A stripped MonoBehaviour has no resolvable MonoScript")?;
            ensure!(
                script.object.class_id == 115,
                "MonoBehaviour m_Script does not reference a MonoScript"
            );
            let script = read_mono_script(script.file, script.object_index, limits)?;
            let identity = MonoBehaviourSchemaIdentity {
                unity_version: &loaded.file.unity_version_string,
                assembly_name: &script.assembly_name,
                namespace: &script.namespace,
                class_name: &script.class_name,
            };
            if session.mono_schemas.schema(identity)?.is_some() {
                continue;
            }
            ensure!(
                loaded.file.target_platform != -2 && loaded.file.header.version.0 >= 14,
                "DLL schema generation for editor-only or pre-64-bit-PPtr assets requires a matching external JSON schema"
            );
            let target = ManagedSchemaTarget {
                unity_version: loaded.file.unity_version_string.clone(),
                assembly: script
                    .assembly_name
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                namespace: script.namespace,
                class_name: script.class_name,
            };
            if seen.contains(&target) {
                continue;
            }
            ensure!(
                targets.len() < MAX_GENERATOR_TARGETS,
                "Too many distinct stripped managed script layouts"
            );
            seen.try_reserve(1)?;
            targets.try_reserve(1)?;
            seen.insert(target.clone());
            targets.push(target);
        }
    }
    Ok(targets)
}

pub fn generate_registry_via_cli(
    session: &crate::Session,
    folder: &Path,
    executable: &Path,
) -> Result<MonoBehaviourSchemaRegistry> {
    let request = ManagedSchemaRequest {
        version: 1,
        dll_directory: fs::canonicalize(folder)
            .context("Cannot resolve managed assembly folder")?,
        targets: managed_targets(session)?,
    };
    request.validate()?;
    if request.targets.is_empty() {
        return Ok(MonoBehaviourSchemaRegistry::new());
    }
    let mut file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(
        BoundedWriter::new(file.as_file_mut(), MAX_GENERATOR_REQUEST_BYTES),
        &request,
    )?;
    file.flush()?;
    let path = file.into_temp_path();
    let bytes = run_generator(executable, &path)?;
    let registry = MonoBehaviourSchemaRegistry::from_json_with_limits(
        &bytes,
        SchemaLimits::default().document,
    )
    .context("Native DLL schema generator returned an invalid schema document")?;
    for target in &request.targets {
        let identity = MonoBehaviourSchemaIdentity {
            unity_version: &target.unity_version,
            assembly_name: &target.assembly,
            namespace: &target.namespace,
            class_name: &target.class_name,
        };
        ensure!(
            registry.schema(identity)?.is_some(),
            "Native DLL schema generator omitted requested type {:?}::{:?}",
            target.namespace,
            target.class_name
        );
    }
    Ok(registry)
}

/// Streaming byte ceiling shared by the native metadata process protocol.
pub struct BoundedWriter<W> {
    inner: W,
    remaining: usize,
}
impl<W> BoundedWriter<W> {
    pub const fn new(inner: W, maximum: usize) -> Self {
        Self {
            inner,
            remaining: maximum,
        }
    }
}
impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other(
                "Managed schema output exceeds its byte limit",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn run_generator(executable: &Path, request: &Path) -> Result<Vec<u8>> {
    run_generator_with_timeout(executable, request, GENERATOR_TIMEOUT)
}

fn run_generator_with_timeout(
    executable: &Path,
    request: &Path,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let mut command = Command::new(executable);
    command
        .arg("--internal-schema-generator")
        .arg(request)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for the private helper.
    }
    let mut child = ChildGuard(
        command
            .spawn()
            .with_context(|| format!("Cannot start native DLL schema generator {executable:?}"))?,
    );
    let stdout = child
        .0
        .stdout
        .take()
        .context("Schema generator stdout was not captured")?;
    let stderr = child
        .0
        .stderr
        .take()
        .context("Schema generator stderr was not captured")?;
    let failed = Arc::new(AtomicBool::new(false));
    let output_failed = Arc::clone(&failed);
    let output_thread = thread::Builder::new()
        .name("schema-output".into())
        .spawn(move || {
            let result = read_process_pipe(stdout, MAX_GENERATOR_OUTPUT_BYTES);
            if result.is_err() {
                output_failed.store(true, Ordering::Release);
            }
            result
        })?;
    let error_failed = Arc::clone(&failed);
    let error_thread = thread::Builder::new()
        .name("schema-errors".into())
        .spawn(move || {
            let result = read_process_pipe(stderr, MAX_GENERATOR_STDERR_BYTES);
            if result.is_err() {
                error_failed.store(true, Ordering::Release);
            }
            result
        })?;
    let start = Instant::now();
    let status = (|| -> Result<_> {
        loop {
            if let Some(status) = child.0.try_wait()? {
                return Ok(status);
            }
            ensure!(
                !failed.load(Ordering::Acquire),
                "Native DLL schema generator exceeded its output limit"
            );
            ensure!(
                start.elapsed() < timeout,
                "Native DLL schema generator exceeded its time limit"
            );
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if status.is_err() {
        let _ = child.0.kill();
        let _ = child.0.wait();
    }
    let output = output_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Schema generator output reader panicked"))?;
    let errors = error_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Schema generator error reader panicked"))?;
    let status = status?;
    let output = output?;
    let errors = errors?;
    if !status.success() {
        bail!(
            "Native DLL schema generation failed ({status}): {}",
            String::from_utf8_lossy(&errors).trim()
        );
    }
    ensure!(
        !output.is_empty(),
        "Native DLL schema generator returned no schema data"
    );
    Ok(output)
}

fn read_process_pipe(mut input: impl Read, maximum: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let length = input.read(&mut buffer)?;
        if length == 0 {
            return Ok(bytes);
        }
        let next = bytes
            .len()
            .checked_add(length)
            .context("Schema process output size overflow")?;
        ensure!(
            next <= maximum,
            "Schema process output exceeds its {maximum} byte limit"
        );
        bytes.try_reserve(length)?;
        bytes.extend_from_slice(&buffer[..length]);
    }
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    #[test]
    fn process_input_and_output_bounds_refuse_excess_bytes() {
        assert_eq!(read_process_pipe(&b"1234"[..], 4).unwrap(), b"1234");
        assert!(read_process_pipe(&b"12345"[..], 4).is_err());
        let mut output = Vec::new();
        let mut writer = BoundedWriter::new(&mut output, 4);
        writer.write_all(b"1234").unwrap();
        assert!(writer.write_all(b"5").is_err());
        assert_eq!(output, b"1234");
    }

    #[cfg(unix)]
    #[test]
    fn hung_metadata_child_is_terminated_and_pipes_are_joined() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("helper");
        fs::write(&helper, b"#!/bin/sh\nexec sleep 10\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let start = Instant::now();
        let error = run_generator_with_timeout(
            &helper,
            Path::new("unused-request"),
            Duration::from_millis(30),
        )
        .unwrap_err();
        assert!(error.to_string().contains("time limit"));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout must not wait for the hanging child"
        );
    }
}

/// Independent limits for encoded files and retained schema structures.
#[derive(Clone, Copy, Debug)]
pub struct SchemaLimits {
    pub maximum_files: usize,
    pub maximum_total_document_bytes: usize,
    pub document: MonoBehaviourSchemaDocumentLimits,
}

impl Default for SchemaLimits {
    fn default() -> Self {
        Self {
            maximum_files: 64,
            maximum_total_document_bytes: 64 * 1024 * 1024,
            document: MonoBehaviourSchemaDocumentLimits {
                maximum_document_bytes: 16 * 1024 * 1024,
                maximum_entries: 50_000,
                maximum_nodes_per_entry: 100_000,
                maximum_total_nodes: 500_000,
                maximum_string_bytes: 64 * 1024,
                maximum_total_string_bytes: 64 * 1024 * 1024,
            },
        }
    }
}

/// Loads one JSON schema file under the application's bounded defaults.
pub fn load_registry(path: &Path) -> Result<MonoBehaviourSchemaRegistry> {
    load_registries(&[path.to_path_buf()])
}

/// Loads a set in caller order, preserving the first document's tie priority.
pub fn load_registries(paths: &[PathBuf]) -> Result<MonoBehaviourSchemaRegistry> {
    load_registries_with_limits(paths, SchemaLimits::default())
}

pub fn load_registries_with_limits(
    paths: &[PathBuf],
    limits: SchemaLimits,
) -> Result<MonoBehaviourSchemaRegistry> {
    ensure!(
        paths.len() <= limits.maximum_files,
        "MonoBehaviour schema file count exceeds {}",
        limits.maximum_files
    );
    let mut registry = MonoBehaviourSchemaRegistry::new();
    let mut usage = SchemaUsage::default();
    let mut encoded_bytes = 0_usize;
    for path in paths {
        let mut remaining = usage.remaining(limits.document)?;
        remaining.maximum_document_bytes = remaining.maximum_document_bytes.min(
            limits
                .maximum_total_document_bytes
                .checked_sub(encoded_bytes)
                .context("MonoBehaviour schema document byte budget exhausted")?,
        );
        let (loaded, document_bytes) = read_registry(path, remaining)?;
        let incoming = SchemaUsage::of(&loaded, limits.document)?;
        encoded_bytes = encoded_bytes
            .checked_add(document_bytes)
            .context("MonoBehaviour schema encoded byte count overflow")?;
        usage = usage.add(incoming)?;
        registry.extend(loaded)?;
    }
    Ok(registry)
}

/// Extends an existing session registry without weakening the shared retained
/// entry, node, or string budgets. A failed load leaves its entries unchanged.
///
/// Incremental calls each enforce the per-file encoded-byte ceiling; callers
/// loading a batch should use `load_registries` for an additional cumulative
/// encoded-byte/file-count ceiling.
pub fn extend_registry(registry: &mut MonoBehaviourSchemaRegistry, path: &Path) -> Result<usize> {
    extend_registry_with_limits(registry, path, SchemaLimits::default())
}

pub fn extend_registry_with_limits(
    registry: &mut MonoBehaviourSchemaRegistry,
    path: &Path,
    limits: SchemaLimits,
) -> Result<usize> {
    ensure!(
        limits.maximum_files != 0,
        "MonoBehaviour schema file budget exhausted"
    );
    let usage = SchemaUsage::of(registry, limits.document)?;
    let mut remaining = usage.remaining(limits.document)?;
    remaining.maximum_document_bytes = remaining
        .maximum_document_bytes
        .min(limits.maximum_total_document_bytes);
    let (loaded, _) = read_registry(path, remaining)?;
    let count = loaded.entries().len();
    registry.extend(loaded)?;
    Ok(count)
}

fn read_registry(
    path: &Path,
    limits: MonoBehaviourSchemaDocumentLimits,
) -> Result<(MonoBehaviourSchemaRegistry, usize)> {
    let read = || -> Result<_> {
        // Avoid opening devices or named pipes as trusted schema documents.
        // Explicit file selection still does not authorize executable content.
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_file(),
            "Schema input must be a regular JSON file"
        );
        ensure!(
            metadata.len() <= limits.maximum_document_bytes as u64,
            "Schema file exceeds the {} byte limit",
            limits.maximum_document_bytes
        );
        let mut file = File::open(path)?;
        ensure!(
            file.metadata()?.is_file(),
            "Schema input must be a regular JSON file"
        );
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let length = file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            let next = bytes
                .len()
                .checked_add(length)
                .context("Schema byte count overflow")?;
            ensure!(
                next <= limits.maximum_document_bytes,
                "Schema file exceeds the {} byte limit",
                limits.maximum_document_bytes
            );
            bytes
                .try_reserve(length)
                .context("Cannot allocate schema file buffer")?;
            bytes.extend_from_slice(&buffer[..length]);
        }
        let registry = MonoBehaviourSchemaRegistry::from_json_with_limits(&bytes, limits)?;
        Ok((registry, bytes.len()))
    };
    read().with_context(|| format!("Cannot load MonoBehaviour schema {path:?}"))
}

#[derive(Default, Clone, Copy)]
struct SchemaUsage {
    entries: usize,
    nodes: usize,
    strings: usize,
}

impl SchemaUsage {
    fn of(
        registry: &MonoBehaviourSchemaRegistry,
        limits: MonoBehaviourSchemaDocumentLimits,
    ) -> Result<Self> {
        let mut usage = Self {
            entries: registry.entries().len(),
            ..Self::default()
        };
        for entry in registry.entries() {
            ensure!(
                entry.tree.nodes.len() <= limits.maximum_nodes_per_entry,
                "Schema node-per-entry limit exceeded"
            );
            usage.nodes = usage
                .nodes
                .checked_add(entry.tree.nodes.len())
                .context("Schema node count overflow")?;
            for string in [
                entry.assembly_name.as_str(),
                entry.namespace.as_str(),
                entry.class_name.as_str(),
                entry.unity_version.as_deref().unwrap_or_default(),
            ] {
                usage.string(string, limits)?;
            }
            for node in &entry.tree.nodes {
                usage.string(&node.type_name, limits)?;
                usage.string(&node.field_name, limits)?;
            }
            usage.strings = usage
                .strings
                .checked_add(entry.tree.string_buffer.len())
                .context("Schema string-buffer byte count overflow")?;
        }
        usage.remaining(limits)?;
        Ok(usage)
    }

    fn string(&mut self, string: &str, limits: MonoBehaviourSchemaDocumentLimits) -> Result<()> {
        ensure!(
            string.len() <= limits.maximum_string_bytes,
            "Schema string byte limit exceeded"
        );
        self.strings = self
            .strings
            .checked_add(string.len())
            .context("Schema string byte count overflow")?;
        Ok(())
    }

    fn add(self, other: Self) -> Result<Self> {
        Ok(Self {
            entries: self
                .entries
                .checked_add(other.entries)
                .context("Schema entry count overflow")?,
            nodes: self
                .nodes
                .checked_add(other.nodes)
                .context("Schema node count overflow")?,
            strings: self
                .strings
                .checked_add(other.strings)
                .context("Schema string byte count overflow")?,
        })
    }

    fn remaining(
        self,
        limits: MonoBehaviourSchemaDocumentLimits,
    ) -> Result<MonoBehaviourSchemaDocumentLimits> {
        Ok(MonoBehaviourSchemaDocumentLimits {
            maximum_entries: limits
                .maximum_entries
                .checked_sub(self.entries)
                .context("Schema cumulative entry limit exceeded")?,
            maximum_total_nodes: limits
                .maximum_total_nodes
                .checked_sub(self.nodes)
                .context("Schema cumulative node limit exceeded")?,
            maximum_total_string_bytes: limits
                .maximum_total_string_bytes
                .checked_sub(self.strings)
                .context("Schema cumulative string byte limit exceeded")?,
            ..limits
        })
    }
}
