//! The fork's class-1210 `IndexObject` wire format.
//!
//! Copyright (c) 2022 Razmoth. Transcribed from the MIT-licensed managed
//! `Object`, `EditorExtension`, `NamedObject`, `PPtr` and `IndexObject` readers.

use anyhow::{Context, Result, ensure};
use serde::Serialize;

const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_NAME_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct IndexObjectReadOptions {
    pub little_endian: bool,
    pub no_target: bool,
    pub path_id_64: bool,
    /// Absolute object offset in the serialized file, for string alignment.
    pub object_offset: u64,
}

impl Default for IndexObjectReadOptions {
    fn default() -> Self {
        Self {
            little_endian: true,
            no_target: false,
            path_id_64: true,
            object_offset: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IndexEntry {
    pub name: String,
    pub file_id: i32,
    pub path_id: i64,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IndexObjectData {
    pub name: String,
    pub entries: Vec<IndexEntry>,
}

pub fn decode_index_object(raw: &[u8], options: IndexObjectReadOptions) -> Result<IndexObjectData> {
    ensure!(raw.len() <= MAX_OBJECT_BYTES, "IndexObject exceeds 64 MiB");
    let mut reader = Reader {
        bytes: raw,
        position: 0,
        options,
        name_bytes: 0,
    };
    if options.no_target {
        reader.u32()?; // Object hide flags.
        reader.pptr()?; // EditorExtension prefab parent.
        reader.pptr()?; // EditorExtension prefab internal.
    }
    let name = reader.aligned_string()?;
    let count = usize::try_from(reader.i32()?).context("IndexObject entry count is negative")?;
    ensure!(
        count <= MAX_ENTRIES,
        "IndexObject contains more than {MAX_ENTRIES} entries"
    );
    // Every entry has an empty-string count, PPtr and UInt64 size at minimum.
    // Check the wire minimum before allocating the entry table.
    let minimum_entry = if options.path_id_64 { 24 } else { 20 };
    ensure!(
        count <= reader.remaining() / minimum_entry,
        "IndexObject entry table is truncated"
    );
    let mut entries = Vec::new();
    entries.try_reserve_exact(count)?;
    for _ in 0..count {
        let name = reader.aligned_string()?;
        let (file_id, path_id) = reader.pptr()?;
        let size = reader.u64()?;
        entries.push(IndexEntry {
            name,
            file_id,
            path_id,
            size,
        });
    }
    ensure!(
        reader.remaining() == 0,
        "IndexObject contains unexpected trailing data"
    );
    Ok(IndexObjectData { name, entries })
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    options: IndexObjectReadOptions,
    name_bytes: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, length: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(length)
            .context("IndexObject byte range overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .context("IndexObject is truncated")?;
        self.position = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        let little = self.options.little_endian;
        let bytes = self.take(4)?.try_into()?;
        Ok(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64> {
        let little = self.options.little_endian;
        let bytes = self.take(8)?.try_into()?;
        Ok(if little {
            u64::from_le_bytes(bytes)
        } else {
            u64::from_be_bytes(bytes)
        })
    }

    fn pptr(&mut self) -> Result<(i32, i64)> {
        let file_id = self.i32()?;
        let path_id = if self.options.path_id_64 {
            self.u64()? as i64
        } else {
            i64::from(self.i32()?)
        };
        Ok((file_id, path_id))
    }

    fn aligned_string(&mut self) -> Result<String> {
        let length =
            usize::try_from(self.i32()?).context("IndexObject string length is negative")?;
        ensure!(length <= MAX_NAME_BYTES, "IndexObject string exceeds 1 MiB");
        self.name_bytes = self
            .name_bytes
            .checked_add(length)
            .context("IndexObject name budget overflow")?;
        ensure!(
            self.name_bytes <= MAX_TOTAL_NAME_BYTES,
            "IndexObject names exceed 32 MiB"
        );
        let source = self.take(length)?;
        let text = std::str::from_utf8(source).context("IndexObject name is not UTF-8")?;
        let mut value = String::new();
        value.try_reserve_exact(text.len())?;
        value.push_str(text);
        let absolute = self
            .options
            .object_offset
            .checked_add(self.position as u64)
            .context("IndexObject alignment offset overflow")?;
        let padding = ((4 - absolute % 4) % 4) as usize;
        self.take(padding)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(little: bool, path64: bool, no_target: bool, offset: u64) -> Vec<u8> {
        fn int(bytes: &mut Vec<u8>, value: u32, little: bool) {
            bytes.extend_from_slice(&if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }
        fn long(bytes: &mut Vec<u8>, value: u64, little: bool) {
            bytes.extend_from_slice(&if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }
        fn string(bytes: &mut Vec<u8>, value: &str, little: bool, offset: u64) {
            int(bytes, value.len() as u32, little);
            bytes.extend_from_slice(value.as_bytes());
            while !(offset + bytes.len() as u64).is_multiple_of(4) {
                bytes.push(0);
            }
        }
        let mut bytes = Vec::new();
        if no_target {
            int(&mut bytes, 0, little);
            for _ in 0..2 {
                int(&mut bytes, 0, little);
                if path64 {
                    long(&mut bytes, 0, little);
                } else {
                    int(&mut bytes, 0, little);
                }
            }
        }
        string(&mut bytes, "Index", little, offset);
        int(&mut bytes, 2, little);
        string(&mut bytes, "assets/first.bin", little, offset);
        int(&mut bytes, 1, little);
        if path64 {
            long(&mut bytes, i64::MIN as u64 + 17, little);
        } else {
            int(&mut bytes, (-91_i32) as u32, little);
        }
        long(&mut bytes, u64::from(u32::MAX) + 12, little);
        string(&mut bytes, "assets/second.json", little, offset);
        int(&mut bytes, 0, little);
        if path64 {
            long(&mut bytes, 99, little);
        } else {
            int(&mut bytes, 99, little);
        }
        long(&mut bytes, 1024, little);
        bytes
    }

    #[test]
    fn index_object_preserves_reference_width_endian_and_absolute_alignment() {
        for little in [false, true] {
            for path64 in [false, true] {
                for no_target in [false, true] {
                    for offset in 0..4 {
                        let bytes = fixture(little, path64, no_target, offset);
                        let parsed = decode_index_object(
                            &bytes,
                            IndexObjectReadOptions {
                                little_endian: little,
                                path_id_64: path64,
                                no_target,
                                object_offset: offset,
                            },
                        )
                        .unwrap();
                        assert_eq!(parsed.name, "Index");
                        assert_eq!(parsed.entries.len(), 2);
                        assert_eq!(parsed.entries[0].name, "assets/first.bin");
                        assert_eq!(parsed.entries[0].file_id, 1);
                        assert_eq!(
                            parsed.entries[0].path_id,
                            if path64 { i64::MIN + 17 } else { -91 }
                        );
                        assert_eq!(parsed.entries[0].size, u64::from(u32::MAX) + 12);
                        assert_eq!(parsed.entries[1].path_id, 99);
                    }
                }
            }
        }
    }

    #[test]
    fn index_object_rejects_negative_huge_and_truncated_lengths() {
        assert!(
            decode_index_object(&(-1_i32).to_le_bytes(), IndexObjectReadOptions::default())
                .is_err()
        );
        let mut bytes = 0_u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_index_object(&bytes, IndexObjectReadOptions::default()).is_err());
        let complete = fixture(true, true, false, 0);
        for length in 0..complete.len() {
            assert!(
                decode_index_object(&complete[..length], IndexObjectReadOptions::default())
                    .is_err()
            );
        }
    }
}
