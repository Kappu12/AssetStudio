//! Compatibility with the public UnityCN key list shipped by the baseline.
//!
//! Key material is data supplied by a caller or the existing public resource.
//! Diagnostics deliberately report only indexes, lengths, and JSON positions.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 1024;

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "Name", alias = "name")]
    name: String,
    #[serde(rename = "Key", alias = "key")]
    key: String,
}

pub struct UnityCnKeys {
    entries: Vec<Entry>,
}

impl std::fmt::Debug for UnityCnKeys {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("UnityCnKeys")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl UnityCnKeys {
    /// Returns the same default location used by the managed application.
    pub fn beside_executable(executable: &Path) -> Result<PathBuf> {
        Ok(executable
            .parent()
            .context("executable has no directory")?
            .join("Keys.json"))
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let before =
            fs::metadata(path).with_context(|| format!("inspect UnityCN key file {path:?}"))?;
        ensure!(before.is_file(), "UnityCN key file must be a regular file");
        let file = File::open(path).context("open UnityCN key file")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_FILE_BYTES,
            "UnityCN key file exceeds its regular-file or 1 MiB limit"
        );
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(usize::try_from(metadata.len())?)?;
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "UnityCN key file exceeds 1 MiB"
        );
        let entries: Vec<Entry> = serde_json::from_slice(bytes).map_err(|error| {
            // serde_json's full message can include the offending field value.
            anyhow::anyhow!(
                "UnityCN key file is invalid JSON at line {}, column {}",
                error.line(),
                error.column()
            )
        })?;
        ensure!(
            !entries.is_empty() && entries.len() <= MAX_ENTRIES,
            "UnityCN key list must contain 1..1024 entries"
        );
        for (index, entry) in entries.iter().enumerate() {
            ensure!(
                !entry.name.trim().is_empty()
                    && entry.name.len() <= 512
                    && !entry.name.chars().any(char::is_control),
                "UnityCN key entry {index} has an invalid name"
            );
            ensure!(
                entry.key.len() == 32 && entry.key.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "UnityCN key entry {index} must contain exactly 32 hexadecimal digits"
            );
        }
        Ok(Self { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    /// Selects an existing key without ever including its value in an error.
    pub fn key(&self, index: usize) -> Result<&str> {
        self.entries
            .get(index)
            .map(|entry| entry.key.as_str())
            .with_context(|| {
                format!(
                    "UnityCN key index {index} is outside the {}-entry list",
                    self.entries.len()
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_resource_retains_every_original_entry_and_validates() {
        let list = UnityCnKeys::from_bytes(include_bytes!("../../../resources/Keys.json")).unwrap();
        assert_eq!(list.len(), 23);
        assert_eq!(list.names().next(), Some("PGR GLB/KR"));
        assert_eq!(list.key(0).unwrap().len(), 32);
        assert!(list.key(list.len()).is_err());
    }

    #[test]
    fn invalid_indexes_keys_and_json_never_echo_material() {
        let material = "REDACT_THIS_KEY_VALUE";
        let invalid = serde_json::json!([{ "Name":"test", "Key":material }]);
        let error = UnityCnKeys::from_bytes(&serde_json::to_vec(&invalid).unwrap()).unwrap_err();
        assert!(!error.to_string().contains(material));
        let wrong_type = serde_json::json!([{ "Name":material, "Key":42 }]);
        let error = UnityCnKeys::from_bytes(&serde_json::to_vec(&wrong_type).unwrap()).unwrap_err();
        assert!(!error.to_string().contains(material));
        assert!(UnityCnKeys::from_bytes(b"[]").is_err());
        assert!(UnityCnKeys::from_bytes(&vec![b' '; MAX_FILE_BYTES as usize + 1]).is_err());
        let list = UnityCnKeys::from_bytes(
            br#"[{"Name":"test","Key":"0123456789abcdef0123456789abcdef"}]"#,
        )
        .unwrap();
        assert!(!format!("{list:?}").contains(list.key(0).unwrap()));
        assert!(
            !list
                .key(1)
                .unwrap_err()
                .to_string()
                .contains(list.key(0).unwrap())
        );
    }
}
