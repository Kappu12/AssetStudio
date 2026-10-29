//! Bounded legacy/Rust asset-list caches, including MessagePack-CSharp LZ4 maps.
//!
//! Cache metadata never grants filesystem access by itself. Callers provide a
//! base folder; selected source files and their symlinks must remain inside it.
//! Selection resolves exact PathID/class identities, and ambiguous old entries
//! fail rather than selecting every object in a matching source file.

mod json;
mod messagepack;
mod xml;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::output::{SafeOutput, atomic_write};
use crate::{AssetKey, AssetRecord, OperationReport, Session, class_id, class_name};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_STRING_BYTES: usize = 4096;

type Identity = (PathBuf, i64, i32, Option<String>);
type LoadedSource = Option<(PathBuf, Option<String>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetMapFormat {
    Auto,
    Json,
    Xml,
    MessagePack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetMapEntry {
    pub name: String,
    pub container: String,
    /// Physical outer source file, as in the original AssetEntry.Source.
    pub source: String,
    pub path_id: i64,
    pub class_id: i32,
    pub class_name: String,
    /// Optional exact container identity. Old readers ignore this extra metadata.
    pub serialized_source: Option<String>,
    pub file_index: Option<usize>,
    pub size: u64,
}

impl AssetMapEntry {
    pub fn record(&self) -> AssetRecord {
        AssetRecord {
            key: AssetKey {
                file_index: self.file_index.unwrap_or(0),
                path_id: self.path_id,
            },
            class_id: self.class_id,
            class_name: self.class_name.clone(),
            name: self.name.clone(),
            container: self.container.clone(),
            source: self.source.clone(),
            size: self.size,
        }
    }
    fn validate(&self) -> Result<()> {
        for value in [&self.name, &self.container, &self.source, &self.class_name] {
            ensure!(
                value.len() <= MAX_STRING_BYTES,
                "asset map string exceeds 4096 bytes"
            );
        }
        ensure!(
            !self.source.is_empty() && !self.source.chars().any(char::is_control),
            "asset map Source is empty or contains control characters"
        );
        ensure!(!self.class_name.is_empty(), "asset map class name is empty");
        if !self.class_name.eq_ignore_ascii_case("Unknown") {
            ensure!(
                class_id(&self.class_name).is_none_or(|id| id == self.class_id),
                "asset map class name and ID disagree"
            );
        }
        if let Some(source) = &self.serialized_source {
            ensure!(
                source.len() <= MAX_STRING_BYTES && !source.chars().any(char::is_control),
                "asset map serialized source is invalid"
            );
            ensure!(
                source.split("::").next() == Some(self.source.as_str()),
                "serialized source disagrees with its outer Source"
            );
        }
        Ok(())
    }
    fn metadata_size(&self) -> usize {
        self.name.len()
            + self.container.len()
            + self.source.len()
            + self.class_name.len()
            + self.serialized_source.as_ref().map_or(0, String::len)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetMap {
    entries: Vec<AssetMapEntry>,
    game_type: Option<i32>,
}

impl AssetMap {
    pub fn new(entries: Vec<AssetMapEntry>, game_type: Option<i32>) -> Result<Self> {
        ensure!(
            entries.len() <= MAX_ENTRIES,
            "asset map exceeds 200,000 entries"
        );
        let mut strings = 0usize;
        for entry in &entries {
            entry.validate()?;
            strings = strings
                .checked_add(entry.metadata_size())
                .context("asset map metadata size overflow")?;
            ensure!(strings <= MAX_BYTES, "asset map metadata exceeds 64 MiB");
        }
        Ok(Self { entries, game_type })
    }
    pub fn entries(&self) -> &[AssetMapEntry] {
        &self.entries
    }
    pub fn game_type(&self) -> Option<i32> {
        self.game_type
    }

    pub fn from_path(path: &Path, format: AssetMapFormat) -> Result<Self> {
        let metadata = fs::metadata(path).with_context(|| format!("inspect asset map {path:?}"))?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_BYTES as u64,
            "asset map must be a regular file at most 64 MiB"
        );
        let file = File::open(path)?;
        ensure!(
            file.metadata()?.is_file(),
            "asset map must be a regular file"
        );
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(usize::try_from(metadata.len())?)?;
        file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes, format).with_context(|| format!("parse asset map {path:?}"))
    }
    pub fn from_bytes(bytes: &[u8], format: AssetMapFormat) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BYTES, "asset map exceeds 64 MiB");
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let format = if format == AssetMapFormat::Auto {
            match bytes
                .iter()
                .copied()
                .find(|value| !value.is_ascii_whitespace())
            {
                Some(b'[' | b'{') => AssetMapFormat::Json,
                Some(b'<') => AssetMapFormat::Xml,
                Some(_) => AssetMapFormat::MessagePack,
                None => bail!("asset map is empty"),
            }
        } else {
            format
        };
        match format {
            AssetMapFormat::Json => Self::from_json(bytes),
            AssetMapFormat::Xml => xml::read(bytes),
            AssetMapFormat::MessagePack => messagepack::read(bytes),
            AssetMapFormat::Auto => bail!("asset map format could not be detected"),
        }
    }
    fn from_json(bytes: &[u8]) -> Result<Self> {
        json::read(bytes)
    }

    pub fn from_session(session: &Session, keys: &[AssetKey], game: &str) -> Result<Self> {
        let mut entries = Vec::new();
        let selected = session.selected(keys)?;
        entries.try_reserve_exact(selected.len())?;
        for record in selected {
            let original = session.object(record.key)?.source_path().to_owned();
            let (outer, tail) = source_parts(&original);
            let (physical, _) = super::cab_map::source_file(outer)?;
            let source = physical
                .to_str()
                .context("asset maps require UTF-8 source paths")?
                .to_owned();
            let serialized_source = tail.map(|tail| format!("{source}::{tail}"));
            entries.push(AssetMapEntry {
                name: record.name.clone(),
                container: record.container.clone(),
                source,
                path_id: record.key.path_id,
                class_id: record.class_id,
                class_name: record.class_name.clone(),
                serialized_source,
                file_index: Some(record.key.file_index),
                size: record.size,
            });
        }
        Self::new(
            entries,
            Some(game_id(game).context("unknown asset map game profile")?),
        )
    }

    /// Selected indexes are explicit; an empty selection returns no input paths.
    pub fn resolve_sources(&self, indexes: &[usize], base_folder: &Path) -> Result<Vec<PathBuf>> {
        ensure!(
            indexes.len() <= MAX_ENTRIES,
            "asset map selection exceeds 200,000 indexes"
        );
        let base = canonical_base(base_folder)?;
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        seen.try_reserve(indexes.len())?;
        for &index in indexes {
            let entry = self
                .entries
                .get(index)
                .context("asset map selection index is invalid")?;
            let path = resolve_source(&entry.source, &base)?;
            if seen.insert(path.clone()) {
                paths.try_reserve(1)?;
                paths.push(path);
            }
        }
        Ok(paths)
    }

    /// Resolves exact cached identities against a loaded collection in linear
    /// object/entry work. Original maps without container identity must identify
    /// a unique object in their source; otherwise callers need a precise map.
    pub fn keys_in_session(
        &self,
        indexes: &[usize],
        session: &Session,
        base_folder: &Path,
    ) -> Result<Vec<AssetKey>> {
        ensure!(
            indexes.len() <= MAX_ENTRIES,
            "asset map selection exceeds 200,000 indexes"
        );
        let base = canonical_base(base_folder)?;
        let mut wanted: HashMap<Identity, Option<AssetKey>> = HashMap::new();
        wanted.try_reserve(indexes.len())?;
        let mut order = Vec::new();
        order.try_reserve(indexes.len())?;
        let mut source_cache: HashMap<String, PathBuf> = HashMap::new();
        for &index in indexes {
            let entry = self
                .entries
                .get(index)
                .context("asset map selection index is invalid")?;
            let path = if let Some(path) = source_cache.get(&entry.source) {
                path.clone()
            } else {
                let path = resolve_source(&entry.source, &base)?;
                source_cache.insert(entry.source.clone(), path.clone());
                path
            };
            let tail = entry
                .serialized_source
                .as_deref()
                .and_then(|value| source_parts(value).1)
                .map(normalized_tail);
            let identity = (path, entry.path_id, entry.class_id, tail);
            if !wanted.contains_key(&identity) {
                wanted.insert(identity.clone(), None);
                order.push(identity);
            }
        }
        let mut selected_sources = HashSet::new();
        selected_sources.try_reserve(wanted.len())?;
        selected_sources.extend(wanted.keys().map(|(path, _, _, _)| path.clone()));
        let mut loaded_sources: HashMap<String, LoadedSource> = HashMap::new();
        for record in session.assets() {
            let source = if let Some(source) = loaded_sources.get(&record.source) {
                source.clone()
            } else {
                let original = session.object(record.key)?.source_path().to_owned();
                let (outer, tail) = source_parts(&original);
                let resolved = super::cab_map::source_file(outer)
                    .ok()
                    .map(|(path, _)| path)
                    .filter(|path| selected_sources.contains(path))
                    .map(|path| (path, tail.map(normalized_tail)));
                loaded_sources.try_reserve(1)?;
                loaded_sources.insert(record.source.clone(), resolved.clone());
                resolved
            };
            let Some((path, tail)) = source else {
                continue;
            };
            let outer = (path.clone(), record.key.path_id, record.class_id, None);
            update_match(&mut wanted, &outer, record.key)?;
            if tail.is_some() {
                update_match(
                    &mut wanted,
                    &(path, record.key.path_id, record.class_id, tail),
                    record.key,
                )?;
            }
        }
        order
            .into_iter()
            .map(|identity| {
                wanted.remove(&identity).flatten().with_context(|| {
                    format!(
                        "cached asset no longer exists: source {:?}, PathID {}, class {}",
                        identity.0, identity.1, identity.2
                    )
                })
            })
            .collect()
    }

    pub fn to_bytes(&self, format: AssetMapFormat) -> Result<Vec<u8>> {
        let mut output = BoundedBytes(Vec::new());
        match format {
            AssetMapFormat::Json => {
                output.write_all(b"[")?;
                for (index, entry) in self.entries.iter().enumerate() {
                    if index != 0 {
                        output.write_all(b",")?;
                    }
                    let type_value = if entry.class_name == "Unknown" {
                        serde_json::json!(entry.class_id)
                    } else {
                        serde_json::json!(entry.class_name)
                    };
                    let value = serde_json::json!({"Name":entry.name,"Container":entry.container,"Source":entry.source,
                        "PathID":entry.path_id,"Type":type_value,"SerializedSource":entry.serialized_source});
                    serde_json::to_writer(&mut output, &value)?;
                }
                output.write_all(b"]")?;
            }
            AssetMapFormat::Xml => xml::write(self, &mut output)?,
            AssetMapFormat::MessagePack => messagepack::write(self, &mut output)?,
            AssetMapFormat::Auto => bail!("choose an explicit asset map output format"),
        }
        Ok(output.0)
    }
    pub fn write_path(&self, output: &Path, format: AssetMapFormat) -> Result<OperationReport> {
        let bytes = self.to_bytes(format)?;
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = SafeOutput::new(parent)?;
        let path = directory.root().join(
            output
                .file_name()
                .context("asset map filename is missing")?,
        );
        let published = atomic_write(&path, false, |file| {
            file.write_all(&bytes)?;
            Ok(())
        })?;
        Ok(if published {
            OperationReport {
                written: vec![path],
                ..OperationReport::default()
            }
        } else {
            OperationReport {
                skipped: vec![format!("existing asset map: {}", path.display())],
                ..OperationReport::default()
            }
        })
    }
}

impl Session {
    pub fn write_asset_cache(
        &self,
        keys: &[AssetKey],
        output: &Path,
        format: AssetMapFormat,
        game: &str,
    ) -> Result<OperationReport> {
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let root = SafeOutput::new(parent)?;
        let path = root.root().join(
            output
                .file_name()
                .context("asset map filename is missing")?,
        );
        self.check_output_target(&path)?;
        AssetMap::from_session(self, keys, game)?.write_path(&path, format)
    }
}

fn entry_from_json(item: &Value) -> Result<AssetMapEntry> {
    let object = item
        .as_object()
        .context("asset map entry must be an object")?;
    let name = string_field(object, &["Name", "name"], true)?;
    let container = string_field(object, &["Container", "container"], false)?;
    let mut source = string_field(object, &["Source", "source"], true)?;
    let key = field(object, &["key"]).and_then(Value::as_object);
    let path_id = field(object, &["PathID", "path_id", "pathId"])
        .or_else(|| key.and_then(|key| field(key, &["path_id", "PathID"])))
        .context("asset map entry has no PathID")
        .and_then(parse_i64)?;
    let type_value = field(object, &["class_id", "classId", "Type", "type"])
        .context("asset map entry has no Type/class_id")?;
    let (id, mut type_name) = parse_class(type_value)?;
    if let Some(name) = field(object, &["class_name"]).and_then(Value::as_str) {
        type_name = name.to_owned();
    }
    let mut serialized_source = field(object, &["SerializedSource", "serialized_source"])
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .context("SerializedSource must be a string")
                .map(str::to_owned)
        })
        .transpose()?;
    normalize_source(&mut source, &mut serialized_source);
    let file_index = key
        .and_then(|key| field(key, &["file_index"]))
        .or_else(|| field(object, &["FileIndex", "file_index"]))
        .map(|value| {
            value
                .as_u64()
                .context("file_index must be nonnegative")
                .and_then(|value| Ok(usize::try_from(value)?))
        })
        .transpose()?;
    let size = field(object, &["size", "Size"])
        .map(|value| value.as_u64().context("asset map size must be nonnegative"))
        .transpose()?
        .unwrap_or(0);
    Ok(AssetMapEntry {
        name,
        container,
        source,
        path_id,
        class_id: id,
        class_name: type_name,
        serialized_source,
        file_index,
        size,
    })
}

fn update_match(
    wanted: &mut HashMap<Identity, Option<AssetKey>>,
    identity: &Identity,
    key: AssetKey,
) -> Result<()> {
    if let Some(found) = wanted.get_mut(identity) {
        ensure!(
            found.is_none_or(|previous| previous == key),
            "legacy asset map identity is ambiguous inside source {:?}, PathID {}, class {}; regenerate a JSON/XML map with serialized source metadata",
            identity.0,
            identity.1,
            identity.2
        );
        *found = Some(key);
    }
    Ok(())
}
fn field<'a>(object: &'a serde_json::Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| object.get(*name))
}
fn string_field(
    object: &serde_json::Map<String, Value>,
    names: &[&str],
    required: bool,
) -> Result<String> {
    match field(object, names) {
        Some(Value::String(value)) => {
            ensure!(
                value.len() <= MAX_STRING_BYTES,
                "asset map string exceeds 4096 bytes"
            );
            Ok(value.clone())
        }
        Some(Value::Null) | None if !required => Ok(String::new()),
        _ => bail!("asset map entry has invalid {}", names[0]),
    }
}
fn parse_i64(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .context("asset map integer is invalid or out of range")
}
fn parse_class(value: &Value) -> Result<(i32, String)> {
    if let Some(name) = value.as_str() {
        let id = class_id(name).with_context(|| format!("unknown asset map class {name:?}"))?;
        return Ok((
            id,
            if name.parse::<i32>().is_ok() {
                class_name(id).into()
            } else {
                name.into()
            },
        ));
    }
    let id = i32::try_from(parse_i64(value)?)?;
    Ok((id, class_name(id).into()))
}
fn parse_game(value: &Value) -> Result<i32> {
    if let Some(name) = value.as_str() {
        game_id(name).context("unknown asset map game profile")
    } else {
        Ok(i32::try_from(parse_i64(value)?)?)
    }
}
fn normalize_source(source: &mut String, serialized: &mut Option<String>) {
    if let Some((outer, _)) = source.split_once("::") {
        let outer = outer.to_owned();
        if serialized.is_none() {
            *serialized = Some(source.clone());
        }
        *source = outer;
    }
}
fn normalized_tail(value: &str) -> String {
    value.replace('\\', "/").to_ascii_lowercase()
}
fn source_parts(source: &str) -> (&str, Option<&str>) {
    source
        .split_once("::")
        .map_or((source, None), |(outer, tail)| (outer, Some(tail)))
}
fn canonical_base(base: &Path) -> Result<PathBuf> {
    let base = fs::canonicalize(base).context("resolve asset map base")?;
    ensure!(
        base.is_dir() && base.parent().is_some(),
        "asset map base must be a directory below the filesystem root"
    );
    Ok(base)
}
fn resolve_source(source: &str, base: &Path) -> Result<PathBuf> {
    let path = Path::new(source);
    let mut candidates = Vec::new();
    if path.is_absolute() {
        candidates.push(path.to_owned());
    } else {
        ensure!(
            !source.contains(':') && !source.starts_with('\\'),
            "asset map contains a foreign absolute source path; relocate its Source fields explicitly"
        );
        candidates.push(base.join(path));
        candidates.push(std::env::current_dir()?.join(path));
    }
    let mut found = None;
    let mut outside = false;
    for candidate in candidates {
        if let Ok(path) = fs::canonicalize(candidate) {
            if !path.starts_with(base) {
                outside = true;
                continue;
            }
            if !fs::metadata(&path)?.is_file() {
                continue;
            }
            ensure!(
                found.as_ref().is_none_or(|previous| previous == &path),
                "relative asset map Source is ambiguous; use an absolute Source"
            );
            found = Some(path);
        }
    }
    if found.is_none() && outside {
        bail!("asset map Source escapes the selected base folder: {source:?}");
    }
    found.with_context(|| format!("asset map source file is missing or is not regular: {source:?}"))
}

struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = self
            .0
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("asset map size overflow"))?;
        if length > MAX_BYTES {
            return Err(std::io::Error::other("asset map exceeds 64 MiB"));
        }
        self.0
            .try_reserve(bytes.len())
            .map_err(std::io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn game_id(name: &str) -> Option<i32> {
    LEGACY_GAMES
        .iter()
        .position(|value| value.eq_ignore_ascii_case(name))
        .and_then(|value| i32::try_from(value).ok())
}
pub fn game_name(id: i32) -> Option<&'static str> {
    usize::try_from(id)
        .ok()
        .and_then(|index| LEGACY_GAMES.get(index))
        .copied()
}
const LEGACY_GAMES: &[&str] = &[
    "Normal",
    "UnityCN",
    "GI",
    "GI_Pack",
    "GI_CB1",
    "GI_CB2",
    "GI_CB3",
    "GI_CB3Pre",
    "BH3",
    "BH3Pre",
    "BH3PrePre",
    "ZZZ_CB1",
    "SR_CB2",
    "SR",
    "TOT",
    "Naraka",
    "EnsembleStars",
    "OPFP",
    "FakeHeader",
    "FantasyOfWind",
    "ShiningNikki",
    "HelixWaltz2",
    "NetEase",
    "AnchorPanic",
    "DreamscapeAlbireo",
    "ImaginaryFest",
    "AliceGearAegis",
    "ProjectSekai",
    "CodenameJump",
    "GirlsFrontline",
    "Reverse1999",
    "ArknightsEndfield",
    "JJKPhantomParade",
    "MuvLuvDimensions",
    "PartyAnimals",
    "LoveAndDeepspace",
    "SchoolGirlStrikers",
    "ExAstris",
    "PerpetualNovelty",
];
