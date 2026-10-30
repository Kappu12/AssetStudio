//! Backward-compatible CAB caches and bounded dependency resolution.
//!
//! The binary layout matches `AssetsHelper.DumpCABMap`: BinaryWriter UTF-8
//! strings prefixed with a 7-bit encoded byte length, followed by little-endian
//! Int32 counts and Int64 source offsets. New maps describe complete standard
//! source files at offset zero. A legacy nonzero offset is retained while
//! reading the map, but resolution rejects it explicitly: it must never be
//! mistaken for an offset into a decompressed or decrypted stream.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::file_type::{FileType, HEADER_SCAN_LENGTH, detect_file_type};

use crate::output::{SafeOutput, atomic_write};
use crate::{OperationReport, Session};

pub const MAX_MAP_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_DEPENDENCIES: usize = 2_000_000;
pub const MAX_STRING_BYTES: usize = 4096;
pub const MAX_TOTAL_STRING_BYTES: usize = 32 * 1024 * 1024;

/// One serialized-file name and the complete source file containing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CabEntry {
    pub cab: String,
    pub path: String,
    pub offset: i64,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CabMap {
    base_folder: String,
    entries: Vec<CabEntry>,
}

impl CabMap {
    /// Creates and validates a map, preserving the supplied entry order.
    pub fn new(base_folder: String, entries: Vec<CabEntry>) -> Result<Self> {
        let map = Self {
            base_folder,
            entries,
        };
        map.validate()?;
        Ok(map)
    }

    pub fn base_folder(&self) -> &str {
        &self.base_folder
    }
    pub fn entries(&self) -> &[CabEntry] {
        &self.entries
    }

    /// Reads one regular map file under a fixed byte budget.
    pub fn from_path(path: &Path) -> Result<Self> {
        let before =
            fs::symlink_metadata(path).with_context(|| format!("inspect CAB map {path:?}"))?;
        ensure!(
            before.is_file() && !before.file_type().is_symlink(),
            "CAB map must be a regular file: {path:?}"
        );
        let file = File::open(path).with_context(|| format!("open CAB map {path:?}"))?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "CAB map is not a regular file");
        ensure!(
            metadata.len() <= MAX_MAP_BYTES,
            "CAB map exceeds the 64 MiB limit"
        );
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(usize::try_from(metadata.len())?)?;
        file.take(MAX_MAP_BYTES + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes).with_context(|| format!("parse CAB map {path:?}"))
    }

    /// Parses the original .NET map format without touching its source paths.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_MAP_BYTES,
            "CAB map exceeds the 64 MiB limit"
        );
        let mut reader = MapReader {
            bytes,
            position: 0,
            string_bytes: 0,
        };
        let base_folder = reader.string("base folder")?;
        let count = reader.count(MAX_ENTRIES, 14, "CAB entry")?;
        let mut entries = Vec::new();
        entries.try_reserve_exact(count)?;
        let mut dependencies_total = 0usize;
        for _ in 0..count {
            let cab = reader.string("CAB name")?;
            let path = reader.string("source path")?;
            let offset = i64::from_le_bytes(reader.array()?);
            ensure!(offset >= 0, "CAB offset cannot be negative");
            let remaining_dependencies = MAX_DEPENDENCIES
                .checked_sub(dependencies_total)
                .context("CAB dependency count overflow")?;
            let dependency_count = reader.count(remaining_dependencies, 1, "CAB dependency")?;
            dependencies_total = dependencies_total
                .checked_add(dependency_count)
                .context("CAB dependency count overflow")?;
            let mut dependencies = Vec::new();
            dependencies.try_reserve_exact(dependency_count)?;
            for _ in 0..dependency_count {
                dependencies.push(reader.string("dependency name")?);
            }
            entries.push(CabEntry {
                cab,
                path,
                offset,
                dependencies,
            });
        }
        ensure!(
            reader.position == bytes.len(),
            "CAB map contains trailing bytes"
        );
        Self::new(base_folder, entries)
    }

    /// Emits bytes that the original BinaryReader.ParseCABMap can read.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut writer = MapWriter { bytes: Vec::new() };
        writer.string(&self.base_folder)?;
        writer.write(&i32::try_from(self.entries.len())?.to_le_bytes())?;
        for entry in &self.entries {
            writer.string(&entry.cab)?;
            writer.string(&entry.path)?;
            writer.write(&entry.offset.to_le_bytes())?;
            writer.write(&i32::try_from(entry.dependencies.len())?.to_le_bytes())?;
            for dependency in &entry.dependencies {
                writer.string(dependency)?;
            }
        }
        Ok(writer.bytes)
    }

    /// Atomically writes a cache. Existing files are preserved unless requested.
    pub fn write_path(&self, output: &Path, overwrite: bool) -> Result<OperationReport> {
        let bytes = self.to_bytes()?;
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = SafeOutput::new(parent)?;
        let output = directory.root().join(
            output
                .file_name()
                .context("CAB output filename is missing")?,
        );
        self.check_indexed_output(&output)?;
        let published = atomic_write(&output, overwrite, |file| {
            file.write_all(&bytes)?;
            Ok(())
        })?;
        Ok(if published {
            OperationReport {
                written: vec![output],
                ..OperationReport::default()
            }
        } else {
            OperationReport {
                skipped: vec![format!("existing CAB map: {}", output.display())],
                ..OperationReport::default()
            }
        })
    }

    fn check_indexed_output(&self, output: &Path) -> Result<()> {
        // An offline/foreign-platform cache can still be copied. Where the base
        // is available, its indexed source paths must never become map outputs.
        let requested = Path::new(&self.base_folder);
        if !requested.is_absolute() {
            return Ok(());
        }
        let Ok(base) = fs::canonicalize(requested) else {
            return Ok(());
        };
        let output = portable_path_key(output)?;
        let output_split = split_base(&output);
        let mut checked = HashSet::new();
        checked.try_reserve(self.entries.len())?;
        for entry in &self.entries {
            let source = base.join(portable_relative_path(&entry.path)?);
            if !checked.insert(source.clone()) {
                continue;
            }
            let canonical = fs::canonicalize(&source).unwrap_or(source);
            let source_key = portable_path_key(&canonical)?;
            let same_split = output_split
                .as_ref()
                .is_some_and(|output_base| split_base(&source_key).as_ref() == Some(output_base));
            ensure!(
                source_key != output && !same_split,
                "refusing to overwrite indexed CAB source {canonical:?}"
            );
        }
        Ok(())
    }

    /// Resolves all transitive CAB dependencies from explicit selected inputs.
    ///
    /// Paths remain below the canonical base folder, including after resolving
    /// symbolic links. The optional base override relocates a portable cache;
    /// it does not permit entries to escape that directory. Missing CABs and
    /// reachable nonzero-offset entries are errors, not silently missing files.
    pub fn resolve_paths(
        &self,
        seeds: &[PathBuf],
        base_override: Option<&Path>,
    ) -> Result<Vec<PathBuf>> {
        self.validate()?;
        ensure!(
            !seeds.is_empty(),
            "CAB resolution requires at least one selected input"
        );
        ensure!(seeds.len() <= MAX_ENTRIES, "too many CAB seed paths");
        let base = self.resolved_base(base_override)?;
        let mut output = Vec::new();
        let mut selected_files = HashSet::new();
        let mut selected_directories = HashSet::new();
        let mut selected_split_bases = HashSet::new();
        selected_files.try_reserve(seeds.len())?;
        selected_directories.try_reserve(seeds.len())?;
        selected_split_bases.try_reserve(seeds.len())?;
        let mut seen_paths = HashSet::new();
        seen_paths.try_reserve(seeds.len())?;
        for seed in seeds {
            let path =
                fs::canonicalize(seed).with_context(|| format!("resolve CAB seed {seed:?}"))?;
            ensure!(
                path.starts_with(&base),
                "CAB seed is outside the base folder: {seed:?}"
            );
            let metadata = fs::metadata(&path)?;
            ensure!(
                metadata.is_file() || metadata.is_dir(),
                "CAB seed must be a regular file or directory"
            );
            if seen_paths.insert(path.clone()) {
                output.try_reserve(1)?;
                output.push(path.clone());
                let key = portable_path_key(&path)?;
                if metadata.is_dir() {
                    selected_directories.insert(key);
                } else {
                    if let Some(base) = split_base(&key) {
                        selected_split_bases.insert(base);
                    }
                    selected_files.insert(key);
                }
            }
        }
        let mut by_name = HashMap::new();
        by_name.try_reserve(self.entries.len())?;
        let mut pending = VecDeque::new();
        pending.try_reserve(self.entries.len())?;
        let mut scheduled = HashSet::new();
        scheduled.try_reserve(self.entries.len())?;
        for (index, entry) in self.entries.iter().enumerate() {
            by_name.insert(entry.cab.to_ascii_lowercase(), index);
            let relative = portable_relative_path(&entry.path)?;
            let path = base.join(&relative);
            let key = portable_path_key(&path)?;
            let from_directory = key
                .ancestors()
                .any(|parent| selected_directories.contains(parent));
            let from_split =
                split_base(&key).is_some_and(|base| selected_split_bases.contains(&base));
            if (selected_files.contains(&key) || from_directory || from_split)
                && scheduled.insert(index)
            {
                pending.push_back(index);
            }
        }
        ensure!(
            !pending.is_empty(),
            "the selected input is not indexed by this CAB map"
        );
        let mut visited = HashSet::new();
        visited.try_reserve(self.entries.len())?;
        while let Some(index) = pending.pop_front() {
            if !visited.insert(index) {
                continue;
            }
            let entry = &self.entries[index];
            ensure!(
                entry.offset == 0,
                "CAB {:?} requires a specialized source offset {}. Precise block-offset CAB loading is not implemented; refusing to load the full file as that block",
                entry.cab,
                entry.offset
            );
            let candidate = base.join(portable_relative_path(&entry.path)?);
            let path = fs::canonicalize(&candidate)
                .with_context(|| format!("resolve CAB source {candidate:?}"))?;
            ensure!(
                path.starts_with(&base),
                "CAB source escapes the base folder through a symbolic link: {candidate:?}"
            );
            ensure!(
                fs::metadata(&path)?.is_file(),
                "CAB source must be a regular file: {path:?}"
            );
            let key = portable_path_key(&path)?;
            let covered_by_directory = key
                .ancestors()
                .any(|parent| selected_directories.contains(parent));
            let covered_by_split =
                split_base(&key).is_some_and(|base| selected_split_bases.contains(&base));
            if seen_paths.insert(path.clone()) && !covered_by_directory && !covered_by_split {
                output.try_reserve(1)?;
                output.push(path);
            }
            for dependency in &entry.dependencies {
                let next = by_name
                    .get(&dependency.to_ascii_lowercase())
                    .with_context(|| {
                        format!("CAB {:?} depends on missing CAB {dependency:?}", entry.cab)
                    })?;
                if scheduled.insert(*next) {
                    pending.push_back(*next);
                }
            }
        }
        Ok(output)
    }

    /// Resolves the stored or explicitly relocated base, rejecting filesystem roots.
    pub fn resolved_base(&self, base_override: Option<&Path>) -> Result<PathBuf> {
        let requested = base_override.unwrap_or_else(|| Path::new(&self.base_folder));
        ensure!(
            requested.is_absolute(),
            "CAB base folder is not an absolute path on this platform; supply --cab-base to relocate it"
        );
        let base = fs::canonicalize(requested).with_context(|| {
            format!("resolve CAB base {requested:?}; supply --cab-base if the cache moved")
        })?;
        ensure!(base.is_dir(), "CAB base folder must be a directory");
        ensure!(
            base.parent().is_some(),
            "a filesystem root is too broad for a CAB base folder"
        );
        Ok(base)
    }

    fn validate(&self) -> Result<()> {
        ensure!(!self.base_folder.is_empty(), "CAB base folder is empty");
        ensure!(
            !self.base_folder.chars().any(char::is_control),
            "CAB base folder contains control characters"
        );
        ensure!(
            self.entries.len() <= MAX_ENTRIES,
            "CAB map exceeds the entry limit"
        );
        let mut names = HashSet::new();
        names.try_reserve(self.entries.len())?;
        let mut string_bytes = 0usize;
        charge_string(&self.base_folder, &mut string_bytes)?;
        let mut dependencies = 0usize;
        for entry in &self.entries {
            valid_cab_name(&entry.cab)?;
            portable_relative_path(&entry.path)?;
            ensure!(entry.offset >= 0, "CAB offset cannot be negative");
            ensure!(
                names.insert(entry.cab.to_ascii_lowercase()),
                "duplicate case-insensitive CAB name {:?}",
                entry.cab
            );
            charge_string(&entry.cab, &mut string_bytes)?;
            charge_string(&entry.path, &mut string_bytes)?;
            dependencies = dependencies
                .checked_add(entry.dependencies.len())
                .context("CAB dependency count overflow")?;
            ensure!(
                dependencies <= MAX_DEPENDENCIES,
                "CAB map exceeds the dependency limit"
            );
            for dependency in &entry.dependencies {
                valid_cab_name(dependency)?;
                charge_string(dependency, &mut string_bytes)?;
            }
        }
        Ok(())
    }
}

impl Session {
    /// Generates and writes a cache while protecting all loaded inputs and resources.
    pub fn write_cab_map(
        &self,
        base_folder: &Path,
        output: &Path,
        overwrite: bool,
    ) -> Result<OperationReport> {
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = SafeOutput::new(parent)?;
        let target = directory.root().join(
            output
                .file_name()
                .context("CAB output filename is missing")?,
        );
        self.check_output_target(&target)?;
        self.build_cab_map(base_folder)?
            .write_path(&target, overwrite)
    }

    /// Builds a cache from the loaded files' actual external-reference tables.
    ///
    /// This is full-file mapping for standard sources. It does not pretend that
    /// the serialized payload's position inside a compressed bundle is a file
    /// offset suitable for one of the fork's decrypted block readers.
    pub fn build_cab_map(&self, base_folder: &Path) -> Result<CabMap> {
        ensure!(
            !self.input_paths().is_empty(),
            "CAB map generation requires filesystem input provenance, not anonymous in-memory regions"
        );
        for input in self.input_paths() {
            let text = input
                .to_str()
                .context("CAB maps require UTF-8 filesystem paths")?;
            ensure!(
                !text.contains("::"),
                "CAB maps cannot represent an input path containing the container separator '::'"
            );
        }
        let base = fs::canonicalize(base_folder).context("resolve CAB build base")?;
        ensure!(
            base.is_dir() && base.parent().is_some(),
            "CAB build base must be a directory below a filesystem root"
        );
        let base_text = base
            .to_str()
            .context("CAB maps require a UTF-8 base path")?
            .to_owned();
        let files = self.studio.collection().serialized_files();
        ensure!(
            files.len() <= MAX_ENTRIES,
            "CAB map exceeds the entry limit"
        );
        let provenance = Provenance::new(self.input_paths())?;
        let mut entries = Vec::new();
        entries.try_reserve(files.len())?;
        let mut dependencies_total = 0usize;
        let mut string_bytes = 0usize;
        charge_string(&base_text, &mut string_bytes)?;
        let mut source_cache: HashMap<String, String> = HashMap::new();
        source_cache.try_reserve(files.len())?;
        for loaded in files {
            let outer = loaded
                .path
                .split("::")
                .next()
                .context("CAB source path is empty")?;
            let path = if let Some(path) = source_cache.get(outer) {
                path.clone()
            } else {
                let (physical, split) = source_file(outer)?;
                ensure!(
                    provenance.contains(&physical, split),
                    "loaded source is outside the selected input provenance: {physical:?}"
                );
                ensure!(
                    physical.starts_with(&base),
                    "loaded CAB source is outside the base folder: {physical:?}"
                );
                verify_standard_source(&physical, split.then_some(loaded.file.header.file_size))?;
                let relative = physical
                    .strip_prefix(&base)
                    .context("strip CAB base folder")?;
                let relative = portable_path_string(relative)?;
                source_cache.insert(outer.to_owned(), relative.clone());
                relative
            };
            let cab =
                portable_name(loaded.path.rsplit("::").next().unwrap_or(&loaded.path)).to_owned();
            valid_cab_name(&cab)?;
            charge_string(&cab, &mut string_bytes)?;
            charge_string(&path, &mut string_bytes)?;
            dependencies_total = dependencies_total
                .checked_add(loaded.file.externals.len())
                .context("CAB dependency count overflow")?;
            ensure!(
                dependencies_total <= MAX_DEPENDENCIES,
                "CAB map exceeds the dependency limit"
            );
            let mut dependencies = Vec::new();
            dependencies.try_reserve(loaded.file.externals.len())?;
            for external in &loaded.file.externals {
                let dependency = portable_name(&external.path).to_owned();
                valid_cab_name(&dependency)?;
                charge_string(&dependency, &mut string_bytes)?;
                dependencies.push(dependency);
            }
            entries.push(CabEntry {
                cab,
                path,
                offset: 0,
                dependencies,
            });
        }
        entries.sort_by(|left, right| left.cab.cmp(&right.cab));
        CabMap::new(base_text, entries)
    }
}

pub(crate) fn source_file(label: &str) -> Result<(PathBuf, bool)> {
    let source = Path::new(label);
    if source.is_file() {
        return Ok((fs::canonicalize(source)?, false));
    }
    let mut split = source.as_os_str().to_owned();
    split.push(".split0");
    let split = PathBuf::from(split);
    if split.is_file() {
        return Ok((fs::canonicalize(split)?, true));
    }
    bail!(
        "cannot recover a regular filesystem source from loaded path {label:?}; anonymous or ambiguous container labels are not CAB-map provenance"
    )
}

struct Provenance {
    files: HashSet<PathBuf>,
    directories: HashSet<PathBuf>,
    split_bases: HashSet<PathBuf>,
}

impl Provenance {
    fn new(inputs: &[PathBuf]) -> Result<Self> {
        ensure!(inputs.len() <= MAX_ENTRIES, "too many CAB input paths");
        let mut files = HashSet::new();
        let mut directories = HashSet::new();
        let mut split_bases = HashSet::new();
        files.try_reserve(inputs.len())?;
        directories.try_reserve(inputs.len())?;
        split_bases.try_reserve(inputs.len())?;
        for input in inputs {
            if input.is_dir() {
                directories.insert(input.clone());
            } else {
                files.insert(input.clone());
                if let Some(base) = split_base(input) {
                    split_bases.insert(base);
                }
            }
        }
        Ok(Self {
            files,
            directories,
            split_bases,
        })
    }
    fn contains(&self, source: &Path, split: bool) -> bool {
        self.files.contains(source)
            || source
                .ancestors()
                .any(|parent| self.directories.contains(parent))
            || (split && split_base(source).is_some_and(|base| self.split_bases.contains(&base)))
    }
}

fn split_base(path: &Path) -> Option<PathBuf> {
    let extension = path.extension()?.to_str()?;
    let number = extension.strip_prefix("split")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(path.with_extension(""))
}

fn verify_standard_source(path: &Path, split_size: Option<u64>) -> Result<()> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "CAB source is not a regular file");
    let mut header = [0; HEADER_SCAN_LENGTH];
    let length = usize::try_from(metadata.len().min(HEADER_SCAN_LENGTH as u64))?;
    file.read_exact(&mut header[..length])?;
    let detection = detect_file_type(&header[..length], split_size.unwrap_or(metadata.len()));
    ensure!(
        detection.file_type != FileType::ResourceFile && detection.data_offset == 0,
        "CAB map generation for game-wrapped or embedded-offset sources is not implemented: {path:?}; standard complete source files are supported"
    );
    Ok(())
}

fn portable_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn portable_path_string(path: &Path) -> Result<String> {
    let mut output = String::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            bail!("CAB source must be a relative normal path");
        };
        if !output.is_empty() {
            output.push('/');
        }
        output.push_str(value.to_str().context("CAB source path is not UTF-8")?);
    }
    portable_relative_path(&output)?;
    Ok(output)
}

fn portable_path_key(path: &Path) -> Result<PathBuf> {
    if cfg!(windows) {
        Ok(PathBuf::from(
            path.to_str()
                .context("CAB paths must be UTF-8")?
                .to_ascii_lowercase(),
        ))
    } else {
        Ok(path.to_owned())
    }
}

fn portable_relative_path(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && value.len() <= MAX_STRING_BYTES,
        "CAB source path is empty or too long"
    );
    ensure!(
        !value.starts_with(['/', '\\']),
        "CAB source path must be relative"
    );
    let mut path = PathBuf::new();
    for part in value.split(['/', '\\']) {
        ensure!(
            !part.is_empty() && part != "." && part != "..",
            "CAB source path contains an unsafe component"
        );
        ensure!(
            part.len() <= 255
                && !part.ends_with([' ', '.'])
                && !part
                    .chars()
                    .any(|ch| ch.is_control() || ":*?\"<>|".contains(ch)),
            "CAB source path is not portable"
        );
        let stem = part
            .split('.')
            .next()
            .unwrap_or(part)
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        let serial = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"));
        let reserved = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
        ) || serial.is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
        ensure!(!reserved, "CAB source path contains a reserved device name");
        path.push(part);
    }
    Ok(path)
}

fn valid_cab_name(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= MAX_STRING_BYTES,
        "CAB name is empty or too long"
    );
    ensure!(
        !value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | ':')),
        "CAB names must be plain serialized-file names"
    );
    Ok(())
}

fn charge_string(value: &str, total: &mut usize) -> Result<()> {
    ensure!(
        value.len() <= MAX_STRING_BYTES,
        "CAB map string exceeds 4096 bytes"
    );
    *total = total
        .checked_add(value.len())
        .context("CAB string accounting overflow")?;
    ensure!(
        *total <= MAX_TOTAL_STRING_BYTES,
        "CAB map string metadata exceeds 32 MiB"
    );
    Ok(())
}

struct MapReader<'a> {
    bytes: &'a [u8],
    position: usize,
    string_bytes: usize,
}

impl MapReader<'_> {
    fn take(&mut self, length: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(length)
            .context("CAB map position overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .context("CAB map is truncated")?;
        self.position = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .context("CAB fixed field size mismatch")
    }

    fn count(&mut self, maximum: usize, minimum_bytes: usize, label: &str) -> Result<usize> {
        let count = i32::from_le_bytes(self.array()?);
        let count =
            usize::try_from(count).with_context(|| format!("{label} count cannot be negative"))?;
        ensure!(count <= maximum, "{label} count exceeds its limit");
        let minimum = count
            .checked_mul(minimum_bytes)
            .context("CAB count byte-size overflow")?;
        ensure!(
            minimum <= self.bytes.len() - self.position,
            "CAB map is truncated for its {label} count"
        );
        Ok(count)
    }

    fn string(&mut self, label: &str) -> Result<String> {
        let mut length = 0u32;
        let mut finished = false;
        for shift in [0, 7, 14, 21, 28] {
            let byte = self.take(1)?[0];
            ensure!(
                shift != 28 || byte <= 0x07,
                "CAB string length is not a positive Int32 7-bit encoding"
            );
            length |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                finished = true;
                break;
            }
        }
        ensure!(finished, "CAB string length exceeds five bytes");
        let length = usize::try_from(length)?;
        ensure!(length <= MAX_STRING_BYTES, "CAB {label} exceeds 4096 bytes");
        self.string_bytes = self
            .string_bytes
            .checked_add(length)
            .context("CAB string accounting overflow")?;
        ensure!(
            self.string_bytes <= MAX_TOTAL_STRING_BYTES,
            "CAB string metadata exceeds 32 MiB"
        );
        let bytes = self.take(length)?;
        let value =
            std::str::from_utf8(bytes).with_context(|| format!("CAB {label} is not UTF-8"))?;
        let mut output = String::new();
        output.try_reserve_exact(length)?;
        output.push_str(value);
        Ok(output)
    }
}

struct MapWriter {
    bytes: Vec<u8>,
}

impl MapWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .context("CAB output length overflow")?;
        ensure!(
            length as u64 <= MAX_MAP_BYTES,
            "CAB map exceeds the 64 MiB limit"
        );
        self.bytes.try_reserve(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<()> {
        let mut length = u32::try_from(value.len())?;
        while length >= 0x80 {
            self.write(&[(length as u8) | 0x80])?;
            length >>= 7;
        }
        self.write(&[length as u8])?;
        self.write(value.as_bytes())
    }
}
