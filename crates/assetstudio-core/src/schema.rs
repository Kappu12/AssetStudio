//! Bounded loading of trusted, non-executing MonoBehaviour type-tree documents.
//!
//! A document supplies a complete serialized object layout, including Unity's
//! MonoBehaviour header. It does not load the assembly named by an identity.
//! Embedded trees remain authoritative; among external entries an exact Unity
//! version outranks a version-independent fallback and the first equal entry
//! keeps priority. JSON documents are not a replacement for DLL reconstruction.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use unity_rs_core::mono_schema::{MonoBehaviourSchemaDocumentLimits, MonoBehaviourSchemaRegistry};

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
