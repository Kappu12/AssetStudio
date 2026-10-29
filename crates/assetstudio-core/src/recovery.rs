//! Offline container-name recovery from the original asset_index JSON format.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::AssetRecord;

const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_NAMES: usize = 1_000_000;
const MAX_NAME_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
struct InputIndex {
    #[serde(rename = "Assets", alias = "assets", default)]
    assets: Option<BTreeMap<i32, BlockInfo>>,
    #[serde(rename = "SubAssets", alias = "subAssets", default)]
    sub_assets: Option<BTreeMap<i32, Vec<SubAssetInfo>>>,
}

#[derive(Debug, Deserialize)]
struct BlockInfo {
    #[serde(rename = "Id", alias = "id")]
    id: u32,
}

#[derive(Debug, Deserialize)]
struct SubAssetInfo {
    #[serde(rename = "Name", alias = "name")]
    name: String,
    #[serde(rename = "PathHashLast", alias = "pathHashLast")]
    path_hash_last: u32,
}

/// A bounded, offline lookup from bundle ID and path hash to container name.
#[derive(Debug, Default)]
pub struct AssetIndex {
    names: BTreeMap<(u32, u32), String>,
}

impl AssetIndex {
    pub fn from_path(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("Open asset index {}", path.display()))?;
        ensure!(
            file.metadata()?.is_file(),
            "Asset index must be a regular file"
        );
        ensure!(
            file.metadata()?.len() <= MAX_INDEX_BYTES,
            "Asset index exceeds 64 MiB"
        );
        let mut bytes = Vec::new();
        file.take(MAX_INDEX_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_INDEX_BYTES,
            "Asset index exceeds 64 MiB"
        );
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_INDEX_BYTES,
            "Asset index exceeds 64 MiB"
        );
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let input: InputIndex = serde_json::from_slice(bytes).context("Parse asset_index JSON")?;
        let assets = input.assets.unwrap_or_default();
        let sub_assets = input.sub_assets.unwrap_or_default();
        let mut output = Self::default();
        let mut count = 0_usize;
        for (key, block) in assets {
            if let Some(entries) = sub_assets.get(&key) {
                for entry in entries {
                    count = count
                        .checked_add(1)
                        .context("Asset index entry count overflow")?;
                    ensure!(count <= MAX_NAMES, "Asset index exceeds one million names");
                    ensure!(!entry.name.is_empty(), "Asset index contains an empty name");
                    ensure!(
                        entry.name.len() <= MAX_NAME_BYTES,
                        "Asset index name exceeds 4096 bytes"
                    );
                    if let Some(previous) = output
                        .names
                        .insert((block.id, entry.path_hash_last), entry.name.clone())
                        && previous != entry.name
                    {
                        bail!(
                            "Conflicting asset index names for bundle {} hash {}",
                            block.id,
                            entry.path_hash_last
                        );
                    }
                }
            }
        }
        Ok(output)
    }

    pub fn resolve(&self, bundle_id: u32, path_hash: u32) -> Option<&str> {
        self.names.get(&(bundle_id, path_hash)).map(String::as_str)
    }

    /// Replaces only matching numeric containers; existing named paths survive.
    pub fn apply(&self, records: &mut [AssetRecord]) -> Result<usize> {
        self.apply_with_limit(records, 256 * 1024 * 1024)
    }

    fn apply_with_limit(&self, records: &mut [AssetRecord], maximum_bytes: usize) -> Result<usize> {
        ensure!(
            records.len() <= MAX_NAMES,
            "Recovery exceeds one million records"
        );
        let mut total_bytes = records.iter().try_fold(0usize, |total, record| {
            total
                .checked_add(record.name.len())
                .and_then(|n| n.checked_add(record.container.len()))
                .and_then(|n| n.checked_add(record.source.len()))
                .and_then(|n| n.checked_add(record.class_name.len()))
                .context("Recovery metadata size overflow")
        })?;
        ensure!(
            total_bytes <= maximum_bytes,
            "Recovered metadata exceeds its byte budget"
        );
        let mut changes = Vec::new();
        changes.try_reserve(records.len())?;
        for (index, record) in records.iter().enumerate() {
            let hash = match record.container.parse::<i32>() {
                Ok(value) => value as u32,
                Err(_) => match record.container.parse::<u32>() {
                    Ok(value) => value,
                    Err(_) => continue,
                },
            };
            // The backend records nested inputs as outer::inner. Check the
            // actual bundle filename first, never a numeric ancestor directory.
            let outer = record.source.split("::").next().unwrap_or(&record.source);
            let filename = outer.rsplit(['/', '\\']).next().unwrap_or(outer);
            let stem = filename.split('.').next().unwrap_or(filename);
            let Ok(bundle_id) = stem.parse::<u32>() else {
                continue;
            };
            if let Some(name) = self.resolve(bundle_id, hash) {
                let new_name = if record.class_name == "MiHoYoBinData" {
                    let filename = name.rsplit(['/', '\\']).next().unwrap_or(name);
                    Some(filename.rsplit_once('.').map_or(filename, |(stem, _)| stem))
                } else {
                    None
                };
                total_bytes = total_bytes
                    .checked_sub(record.container.len())
                    .and_then(|n| n.checked_add(name.len()))
                    .context("Recovery size overflow")?;
                if let Some(new_name) = new_name {
                    total_bytes = total_bytes
                        .checked_sub(record.name.len())
                        .and_then(|n| n.checked_add(new_name.len()))
                        .context("Recovery size overflow")?;
                }
                ensure!(
                    total_bytes <= maximum_bytes,
                    "Recovered metadata exceeds its byte budget"
                );
                changes.push((index, name, new_name));
            }
        }
        // Preflight every replacement before changing the caller's records.
        let count = changes.len();
        for (index, container, name) in changes {
            records[index].container = container.to_owned();
            if let Some(name) = name {
                records[index].name = name.to_owned();
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssetKey;

    fn record(source: &str, container: &str) -> AssetRecord {
        AssetRecord {
            key: AssetKey {
                file_index: 0,
                path_id: 7,
            },
            class_id: 49,
            class_name: "TextAsset".into(),
            name: "original".into(),
            container: container.into(),
            source: source.into(),
            size: 12,
        }
    }

    #[test]
    fn matches_original_bundle_map_and_signed_hash_semantics() {
        let index = AssetIndex::from_json(br#"{"Assets":{"5":{"Id":123,"Offset":0}},"SubAssets":{"5":[{"Name":"Assets/Test.json","PathHashPre":0,"PathHashLast":4294967295}]}}"#).unwrap();
        assert_eq!(index.resolve(123, u32::MAX), Some("Assets/Test.json"));
        let mut records = [
            record("C:\\game\\123.blk::CAB-abcdef", "-1"),
            record("/game/123.blk::CAB-abcdef", "already/named.txt"),
            record("/game/999.blk::CAB-abcdef", "-1"),
        ];
        assert_eq!(index.apply(&mut records).unwrap(), 1);
        assert_eq!(records[0].container, "Assets/Test.json");
        assert_eq!(records[1].container, "already/named.txt");
        assert_eq!(records[2].container, "-1");
    }

    #[test]
    fn missing_null_and_bom_maps_are_supported() {
        assert!(
            AssetIndex::from_json(b"{}")
                .unwrap()
                .resolve(0, 0)
                .is_none()
        );
        assert!(AssetIndex::from_json(b"\xef\xbb\xbf{\"Assets\":null,\"SubAssets\":null}").is_ok());
        assert!(AssetIndex::from_json(b"not json").is_err());
    }

    #[test]
    fn conflicting_hashes_are_reported() {
        let index = AssetIndex::from_json(br#"{"Assets":{"1":{"Id":2},"2":{"Id":2}},"SubAssets":{"1":[{"Name":"first","PathHashLast":3}],"2":[{"Name":"second","PathHashLast":3}]}}"#);
        assert!(index.unwrap_err().to_string().contains("Conflicting"));
    }

    #[test]
    fn recovery_budget_is_checked_before_mutating_records() {
        let index = AssetIndex::from_json(br#"{"Assets":{"1":{"Id":123}},"SubAssets":{"1":[{"Name":"a-much-longer-container-name","PathHashLast":1}]}}"#).unwrap();
        let mut records = [record("123.bundle", "1")];
        let original = records.clone();
        let initial =
            records[0].name.len() + records[0].class_name.len() + records[0].source.len() + 1;
        assert!(index.apply_with_limit(&mut records, initial + 1).is_err());
        assert_eq!(records, original);
    }

    #[test]
    fn numeric_parent_folder_is_not_mistaken_for_a_bundle_id() {
        let index = AssetIndex::from_json(br#"{"Assets":{"1":{"Id":123}},"SubAssets":{"1":[{"Name":"recovered","PathHashLast":1}]}}"#).unwrap();
        let mut records = [record("/game/123/not-a-number.bundle::CAB-foo", "1")];
        assert_eq!(index.apply(&mut records).unwrap(), 0);
    }
}
