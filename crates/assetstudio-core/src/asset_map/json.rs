//! Stream the root list and bound every temporary entry before allocating it.

use std::fmt;

use anyhow::{Result, ensure};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{AssetMap, AssetMapEntry, MAX_BYTES, MAX_ENTRIES, MAX_STRING_BYTES};

const MAX_ENTRY_NODES: usize = 64;
const MAX_ENTRY_DEPTH: usize = 4;

pub(super) fn read(bytes: &[u8]) -> Result<AssetMap> {
    // JSON escapes use at most six raw bytes for each decoded UTF-8 byte.
    // Reject oversized tokens before serde's escaped-string scratch allocation.
    let mut start = None;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if let Some(begin) = start {
            ensure!(
                index - begin <= MAX_STRING_BYTES * 6,
                "asset map JSON string token exceeds its limit"
            );
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                start = None;
            }
        } else if byte == b'"' {
            start = Some(index + 1);
        }
    }
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let map = parser.deserialize_any(Root)?;
    parser.end()?;
    Ok(map)
}

struct Root;
impl<'de> Visitor<'de> for Root {
    type Value = AssetMap;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an asset-entry array or an AssetMap object")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<AssetMap, A::Error> {
        AssetMap::new(entries(sequence)?, None).map_err(de::Error::custom)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<AssetMap, A::Error> {
        let mut items = None;
        let mut game = None;
        let mut game_seen = false;
        let mut count = 0usize;
        while let Some(key) = object.next_key_seed(Key)? {
            count += 1;
            if count > 16 {
                return Err(de::Error::custom("too many asset map root fields"));
            }
            match key.as_str() {
                "AssetEntries" | "asset_entries" | "entries" => {
                    if items.is_some() {
                        return Err(de::Error::custom("duplicate AssetEntries field"));
                    }
                    items = Some(object.next_value_seed(Entries)?);
                }
                "GameType" | "game_type" => {
                    if game_seen {
                        return Err(de::Error::custom("duplicate GameType field"));
                    }
                    game_seen = true;
                    let mut nodes = 0;
                    let value = object.next_value_seed(BoundedValue {
                        nodes: &mut nodes,
                        depth: 0,
                    })?;
                    game = Some(super::parse_game(&value).map_err(de::Error::custom)?);
                }
                _ => {
                    let mut nodes = 0;
                    object.next_value_seed(BoundedValue {
                        nodes: &mut nodes,
                        depth: 0,
                    })?;
                }
            }
        }
        let items =
            items.ok_or_else(|| de::Error::custom("asset map object has no AssetEntries array"))?;
        AssetMap::new(items, game).map_err(de::Error::custom)
    }
}

struct Entries;
impl<'de> DeserializeSeed<'de> for Entries {
    type Value = Vec<AssetMapEntry>;
    fn deserialize<D: Deserializer<'de>>(self, parser: D) -> Result<Self::Value, D::Error> {
        parser.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Entries {
    type Value = Vec<AssetMapEntry>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an asset-entry array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<Self::Value, A::Error> {
        entries(sequence)
    }
}
fn entries<'de, A: SeqAccess<'de>>(mut sequence: A) -> Result<Vec<AssetMapEntry>, A::Error> {
    let mut result = Vec::new();
    let mut bytes = 0usize;
    loop {
        let mut nodes = 0;
        let Some(value) = sequence.next_element_seed(BoundedValue {
            nodes: &mut nodes,
            depth: 0,
        })?
        else {
            return Ok(result);
        };
        if result.len() == MAX_ENTRIES {
            return Err(de::Error::custom("asset map exceeds 200,000 entries"));
        }
        let entry = super::entry_from_json(&value).map_err(de::Error::custom)?;
        entry.validate().map_err(de::Error::custom)?;
        bytes = bytes
            .checked_add(entry.metadata_size())
            .ok_or_else(|| de::Error::custom("asset map metadata size overflow"))?;
        if bytes > MAX_BYTES {
            return Err(de::Error::custom("asset map metadata exceeds 64 MiB"));
        }
        result.try_reserve(1).map_err(de::Error::custom)?;
        result.push(entry);
    }
}

struct Key;
impl<'de> DeserializeSeed<'de> for Key {
    type Value = String;
    fn deserialize<D: Deserializer<'de>>(self, parser: D) -> Result<String, D::Error> {
        parser.deserialize_str(self)
    }
}
impl Visitor<'_> for Key {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON key at most 64 bytes")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        if value.len() > 64 {
            return Err(E::custom("asset map JSON key exceeds 64 bytes"));
        }
        Ok(value.to_owned())
    }
}

struct BoundedValue<'a> {
    nodes: &'a mut usize,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for BoundedValue<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, parser: D) -> Result<Value, D::Error> {
        *self.nodes += 1;
        if *self.nodes > MAX_ENTRY_NODES || self.depth > MAX_ENTRY_DEPTH {
            return Err(de::Error::custom(
                "asset map JSON entry exceeds structural limits",
            ));
        }
        parser.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for BoundedValue<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded asset map JSON value")
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        if value.len() > MAX_STRING_BYTES {
            return Err(E::custom("asset map string exceeds 4096 bytes"));
        }
        Ok(Value::String(value.to_owned()))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(BoundedValue {
            nodes: self.nodes,
            depth: self.depth + 1,
        })? {
            values.try_reserve(1).map_err(de::Error::custom)?;
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key_seed(Key)? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate asset map JSON field"));
            }
            let value = object.next_value_seed(BoundedValue {
                nodes: self.nodes,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
