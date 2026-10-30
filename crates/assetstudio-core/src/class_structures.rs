//! Bounded embedded class definitions and the legacy text export layout.
//!
//! Like the original class list, definitions are grouped by Unity version and
//! class identity. A script type index has identity `-1 - script_type_index`.
//! When several files define that same pair, the last loaded definition wins.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result, ensure};

use crate::output::{SafeOutput, atomic_write, component};
use crate::{OperationReport, Session};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassStructure {
    pub unity_version: String,
    /// The legacy list identity: script entries use a negative script index.
    pub class_id: i32,
    pub serialized_class_id: i32,
    pub name: String,
    pub source: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ClassStructureLimits {
    pub maximum_entries: usize,
    pub maximum_types_scanned: usize,
    pub maximum_total_nodes: usize,
    pub maximum_node_level: u32,
    pub maximum_string_bytes: usize,
    pub maximum_entry_text_bytes: usize,
    pub maximum_total_text_bytes: usize,
}

impl Default for ClassStructureLimits {
    fn default() -> Self {
        Self {
            maximum_entries: 50_000,
            maximum_types_scanned: 1_000_000,
            maximum_total_nodes: 500_000,
            maximum_node_level: 255,
            maximum_string_bytes: 4096,
            maximum_entry_text_bytes: 4 * 1024 * 1024,
            maximum_total_text_bytes: 64 * 1024 * 1024,
        }
    }
}

impl Session {
    pub fn class_structures(&self) -> Result<Vec<ClassStructure>> {
        self.class_structures_with_limits(ClassStructureLimits::default())
    }

    /// Budgets cover every scanned definition, including definitions replaced
    /// by a later file, so repeated identities cannot bypass the work ceiling.
    pub fn class_structures_with_limits(
        &self,
        limits: ClassStructureLimits,
    ) -> Result<Vec<ClassStructure>> {
        let files = self.studio.collection().serialized_files();
        let mut versions = Vec::new();
        versions.try_reserve_exact(files.len())?;
        let mut total_bytes = 0usize;
        for loaded in files {
            let version = loaded.file.unity_version.to_string();
            ensure!(
                version.len() <= limits.maximum_string_bytes,
                "class structure version exceeds string limit"
            );
            charge(
                &mut total_bytes,
                version.len(),
                limits.maximum_total_text_bytes,
                "class structure metadata",
            )?;
            versions.push(version);
        }

        let mut selected = HashMap::new();
        selected.try_reserve(files.len().min(limits.maximum_entries))?;
        let mut scanned = 0usize;
        let mut node_count = 0usize;
        for (file_index, loaded) in files.iter().enumerate() {
            ensure!(
                loaded.path.len() <= limits.maximum_string_bytes,
                "class structure source exceeds string limit"
            );
            for (type_index, kind) in loaded.file.types.iter().enumerate() {
                charge(
                    &mut scanned,
                    1,
                    limits.maximum_types_scanned,
                    "class structure scan",
                )?;
                let Some(tree) = &kind.type_tree else {
                    continue;
                };
                if tree.nodes.is_empty() {
                    continue;
                }
                charge(
                    &mut node_count,
                    tree.nodes.len(),
                    limits.maximum_total_nodes,
                    "class structure nodes",
                )?;
                let text_bytes = structure_text_size(tree, limits)?;
                charge(
                    &mut total_bytes,
                    text_bytes,
                    limits.maximum_total_text_bytes,
                    "class structure text",
                )?;
                let root = &tree.nodes[0];
                let metadata_bytes = [
                    loaded.path.len(),
                    versions[file_index].len(),
                    root.type_name.len(),
                    root.field_name.len(),
                    1,
                ]
                .into_iter()
                .try_fold(0usize, |total, length| {
                    total
                        .checked_add(length)
                        .context("class structure metadata size overflow")
                })?;
                charge(
                    &mut total_bytes,
                    metadata_bytes,
                    limits.maximum_total_text_bytes,
                    "class structure metadata",
                )?;
                let identity = if kind.script_type_index >= 0 {
                    -1 - i32::from(kind.script_type_index)
                } else {
                    kind.class_id
                };
                let key = (versions[file_index].as_str(), identity);
                if !selected.contains_key(&key) {
                    ensure!(
                        selected.len() < limits.maximum_entries,
                        "class structure entry limit exceeded"
                    );
                    selected.try_reserve(1)?;
                }
                selected.insert(key, (file_index, type_index, text_bytes));
            }
        }
        let mut definitions = Vec::new();
        definitions.try_reserve_exact(selected.len())?;
        let mut ordered = Vec::new();
        ordered.try_reserve_exact(selected.len())?;
        ordered.extend(selected);
        ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        for ((version, identity), (file_index, type_index, text_bytes)) in ordered {
            let loaded = &files[file_index];
            let kind = &loaded.file.types[type_index];
            let tree = kind
                .type_tree
                .as_ref()
                .context("class definition disappeared")?;
            let root = tree.nodes.first().context("class definition has no root")?;
            let mut name = String::new();
            let name_bytes = root
                .type_name
                .len()
                .checked_add(root.field_name.len())
                .and_then(|length| length.checked_add(1))
                .context("class structure name size overflow")?;
            name.try_reserve_exact(name_bytes)?;
            write!(name, "{} {}", root.type_name, root.field_name)?;
            let mut text = String::new();
            text.try_reserve_exact(text_bytes)?;
            for node in &tree.nodes {
                for _ in 0..node.level {
                    text.push('\t');
                }
                write!(
                    text,
                    "{} {} {} {}\r\n",
                    node.type_name,
                    node.field_name,
                    node.byte_size,
                    if node.meta_flags & 0x4000 != 0 {
                        "True"
                    } else {
                        "False"
                    }
                )?;
            }
            ensure!(
                text.len() == text_bytes,
                "class structure size accounting mismatch"
            );
            definitions.push(ClassStructure {
                unity_version: copy_string(version)?,
                class_id: identity,
                serialized_class_id: kind.class_id,
                name,
                source: copy_string(&loaded.path)?,
                text,
            });
        }
        Ok(definitions)
    }

    /// Exports `<version>/<identity> <root type and field>.txt`, preserving
    /// existing files and all original/auxiliary input identities.
    pub fn export_class_structures(&self, output: &Path) -> Result<OperationReport> {
        let definitions = self.class_structures()?;
        if definitions.is_empty() {
            return Ok(OperationReport::default());
        }
        let destination = SafeOutput::new(output)?;
        let mut report = OperationReport::default();
        for definition in definitions {
            let result = (|| -> Result<_> {
                let folder =
                    destination.directory(Path::new(&component(&definition.unity_version)))?;
                let path = folder.join(format!(
                    "{} {}.txt",
                    definition.class_id,
                    component(&definition.name)
                ));
                self.check_output_target(&path)?;
                let written = atomic_write(&path, false, |file| {
                    file.write_all(definition.text.as_bytes())?;
                    Ok(())
                })?;
                Ok((path, written))
            })();
            match result {
                Ok((path, true)) => {
                    report.written.try_reserve(1)?;
                    report.written.push(path);
                }
                Ok((path, false)) => {
                    report.skipped.try_reserve(1)?;
                    report
                        .skipped
                        .push(format!("existing class structure: {}", path.display()));
                }
                Err(error) => {
                    report.failed.try_reserve(1)?;
                    report.failed.push(format!(
                        "class structure {}/{}: {error:#}",
                        definition.unity_version, definition.class_id
                    ));
                }
            }
        }
        Ok(report)
    }
}

fn copy_string(value: &str) -> Result<String> {
    let mut copy = String::new();
    copy.try_reserve_exact(value.len())?;
    copy.push_str(value);
    Ok(copy)
}

fn charge(current: &mut usize, additional: usize, maximum: usize, label: &str) -> Result<()> {
    *current = current
        .checked_add(additional)
        .with_context(|| format!("{label} size overflow"))?;
    ensure!(*current <= maximum, "{label} limit exceeded");
    Ok(())
}

fn structure_text_size(
    tree: &unity_rs_core::serialized::TypeTree,
    limits: ClassStructureLimits,
) -> Result<usize> {
    let mut bytes = 0usize;
    let mut previous = 0u32;
    for (index, node) in tree.nodes.iter().enumerate() {
        ensure!(
            node.level <= limits.maximum_node_level,
            "class structure depth limit exceeded"
        );
        ensure!(
            (index == 0 && node.level == 0)
                || (index != 0 && node.level != 0 && node.level <= previous.saturating_add(1)),
            "class structure has invalid node nesting"
        );
        previous = node.level;
        ensure!(
            node.type_name.len() <= limits.maximum_string_bytes
                && node.field_name.len() <= limits.maximum_string_bytes,
            "class structure node string limit exceeded"
        );
        let mut line = usize::try_from(node.level)?;
        for length in [
            node.type_name.len(),
            node.field_name.len(),
            node.byte_size.to_string().len(),
            if node.meta_flags & 0x4000 != 0 { 4 } else { 5 },
            5,
        ] {
            line = line
                .checked_add(length)
                .context("class structure line size overflow")?;
        }
        charge(
            &mut bytes,
            line,
            limits.maximum_entry_text_bytes,
            "class structure definition text",
        )?;
    }
    Ok(bytes)
}
