//! MessagePack-CSharp's documented ext98/ext99 framing, verified against its
//! MessagePackSerializer.TryDecompress/ToLZ4BinaryCore primary implementation.
//! https://github.com/MessagePack-CSharp/MessagePack-CSharp

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Write;

use anyhow::{Context, Result, bail, ensure};

use super::{
    AssetMap, AssetMapEntry, BoundedBytes, MAX_BYTES, MAX_ENTRIES, MAX_STRING_BYTES,
    normalize_source,
};
use crate::class_name;

pub(super) fn read(bytes: &[u8]) -> Result<AssetMap> {
    let decoded = decompress(bytes)?;
    let mut reader = Reader::new(&decoded);
    ensure!(
        reader.array()? == 2,
        "MessagePack asset map must contain [GameType, AssetEntries]"
    );
    let game = i32::try_from(reader.integer()?)?;
    let count = reader.array()?;
    ensure!(
        count <= MAX_ENTRIES
            && count
                .checked_mul(6)
                .is_some_and(|minimum| minimum <= reader.remaining()),
        "MessagePack asset entry count exceeds limits or bytes present"
    );
    let mut entries = Vec::new();
    entries.try_reserve_exact(count)?;
    for _ in 0..count {
        ensure!(
            reader.array()? == 5,
            "unknown MessagePack AssetEntry layout: expected five fields"
        );
        let name = reader.string()?;
        let container = reader.string()?;
        let mut source = reader.string()?;
        let path_id = reader.integer()?;
        let class_id = i32::try_from(reader.integer()?)?;
        let mut serialized_source = None;
        normalize_source(&mut source, &mut serialized_source);
        entries.push(AssetMapEntry {
            name,
            container,
            source,
            path_id,
            class_id,
            class_name: class_name(class_id).into(),
            serialized_source,
            file_index: None,
            size: 0,
        });
    }
    ensure!(
        reader.remaining() == 0,
        "MessagePack asset map has trailing bytes"
    );
    AssetMap::new(entries, Some(game))
}

fn decompress(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    let mut reader = Reader::new(bytes);
    let tag = reader.peek()?;
    if extension_tag(tag) {
        let (kind, payload) = reader.extension()?;
        ensure!(
            kind == 99,
            "unsupported MessagePack compression extension {kind}"
        );
        ensure!(
            reader.remaining() == 0,
            "compressed MessagePack frame has trailing bytes"
        );
        let mut payload = Reader::new(payload);
        let length = usize::try_from(payload.integer()?).context("negative LZ4 output size")?;
        ensure!(length <= MAX_BYTES, "LZ4 asset map expands beyond 64 MiB");
        let compressed = payload.take(payload.remaining())?;
        let mut output = Vec::new();
        output.try_reserve_exact(length)?;
        output.resize(length, 0);
        ensure!(
            lz4_flex::block::decompress_into(compressed, &mut output)? == length,
            "LZ4 block output length differs from its declaration"
        );
        return Ok(Cow::Owned(output));
    }
    if array_tag(tag) {
        let count = reader.array()?;
        if count != 0 && reader.peek().is_ok_and(extension_tag) {
            let (kind, lengths) = reader.extension()?;
            ensure!(
                kind == 98,
                "unsupported MessagePack block-array extension {kind}"
            );
            let count = count - 1;
            ensure!(
                count > 0 && count <= 16_384,
                "LZ4 block-array count exceeds its limit"
            );
            let mut lengths = Reader::new(lengths);
            let mut sizes = Vec::new();
            sizes.try_reserve_exact(count)?;
            let mut total = 0usize;
            for _ in 0..count {
                let size = usize::try_from(lengths.integer()?)
                    .context("negative LZ4 block-array output size")?;
                total = total
                    .checked_add(size)
                    .context("LZ4 expansion size overflow")?;
                ensure!(total <= MAX_BYTES, "LZ4 block-array expands beyond 64 MiB");
                sizes.push(size);
            }
            ensure!(
                lengths.remaining() == 0,
                "LZ4 length extension contains extra fields"
            );
            let mut output = Vec::new();
            output.try_reserve_exact(total)?;
            output.resize(total, 0);
            let mut offset = 0usize;
            for size in sizes {
                let block = reader.binary()?;
                let end = offset
                    .checked_add(size)
                    .context("LZ4 output position overflow")?;
                ensure!(
                    lz4_flex::block::decompress_into(block, &mut output[offset..end])? == size,
                    "LZ4 block-array output differs from its declaration"
                );
                offset = end;
            }
            ensure!(
                reader.remaining() == 0,
                "LZ4 block-array has trailing bytes"
            );
            return Ok(Cow::Owned(output));
        }
    }
    Ok(Cow::Borrowed(bytes))
}

pub(super) fn write(map: &AssetMap, output: &mut impl Write) -> Result<()> {
    let game = map
        .game_type
        .context("MessagePack output requires an explicit game profile")?;
    // Original five-field records cannot preserve distinct inner-CAB identities.
    // Refuse the ambiguous legacy projection before publishing any output.
    let mut identities = HashSet::new();
    identities.try_reserve(map.entries.len())?;
    for entry in &map.entries {
        ensure!(
            identities.insert((&entry.source, entry.path_id, entry.class_id)),
            "original MessagePack format cannot represent duplicate PathID/class identities inside one source; use JSON/XML with serialized source metadata"
        );
    }
    let mut plain = BoundedBytes(Vec::new());
    array(&mut plain, 2)?;
    integer(&mut plain, i64::from(game))?;
    array(&mut plain, map.entries.len())?;
    for entry in &map.entries {
        array(&mut plain, 5)?;
        string(&mut plain, &entry.name)?;
        string(&mut plain, &entry.container)?;
        string(&mut plain, &entry.source)?;
        integer(&mut plain, entry.path_id)?;
        integer(&mut plain, i64::from(entry.class_id))?;
    }
    let blocks: Vec<_> = plain.0.chunks(64 * 1024).collect();
    let mut lengths = Vec::new();
    for block in &blocks {
        integer(&mut lengths, i64::try_from(block.len())?)?;
    }
    array(output, blocks.len() + 1)?;
    extension_header(output, 98, lengths.len())?;
    output.write_all(&lengths)?;
    for block in blocks {
        let compressed = lz4_flex::block::compress(block);
        output.write_all(&[0xc6])?;
        output.write_all(&u32::try_from(compressed.len())?.to_be_bytes())?;
        output.write_all(&compressed)?;
    }
    Ok(())
}

fn array(output: &mut impl Write, count: usize) -> Result<()> {
    if count < 16 {
        output.write_all(&[0x90 | u8::try_from(count)?])?;
    } else if let Ok(count) = u16::try_from(count) {
        output.write_all(&[0xdc])?;
        output.write_all(&count.to_be_bytes())?;
    } else {
        output.write_all(&[0xdd])?;
        output.write_all(&u32::try_from(count)?.to_be_bytes())?;
    }
    Ok(())
}
fn integer(output: &mut impl Write, value: i64) -> Result<()> {
    if (0..=127).contains(&value) {
        output.write_all(&[u8::try_from(value)?])?;
    } else if (-32..0).contains(&value) {
        output.write_all(&[i8::try_from(value)? as u8])?;
    } else if let Ok(value) = u8::try_from(value) {
        output.write_all(&[0xcc, value])?;
    } else if let Ok(value) = u16::try_from(value) {
        output.write_all(&[0xcd])?;
        output.write_all(&value.to_be_bytes())?;
    } else if let Ok(value) = u32::try_from(value) {
        output.write_all(&[0xce])?;
        output.write_all(&value.to_be_bytes())?;
    } else if let Ok(value) = i8::try_from(value) {
        output.write_all(&[0xd0, value as u8])?;
    } else if let Ok(value) = i16::try_from(value) {
        output.write_all(&[0xd1])?;
        output.write_all(&value.to_be_bytes())?;
    } else if let Ok(value) = i32::try_from(value) {
        output.write_all(&[0xd2])?;
        output.write_all(&value.to_be_bytes())?;
    } else {
        output.write_all(&[0xd3])?;
        output.write_all(&value.to_be_bytes())?;
    }
    Ok(())
}
fn string(output: &mut impl Write, value: &str) -> Result<()> {
    if value.len() < 32 {
        output.write_all(&[0xa0 | u8::try_from(value.len())?])?;
    } else if let Ok(length) = u8::try_from(value.len()) {
        output.write_all(&[0xd9, length])?;
    } else if let Ok(length) = u16::try_from(value.len()) {
        output.write_all(&[0xda])?;
        output.write_all(&length.to_be_bytes())?;
    } else {
        bail!("asset map string exceeds its length limit");
    }
    output.write_all(value.as_bytes())?;
    Ok(())
}
fn extension_header(output: &mut impl Write, kind: i8, length: usize) -> Result<()> {
    match length {
        1 => output.write_all(&[0xd4])?,
        2 => output.write_all(&[0xd5])?,
        4 => output.write_all(&[0xd6])?,
        8 => output.write_all(&[0xd7])?,
        16 => output.write_all(&[0xd8])?,
        _ => {
            if let Ok(length) = u8::try_from(length) {
                output.write_all(&[0xc7, length])?;
            } else if let Ok(length) = u16::try_from(length) {
                output.write_all(&[0xc8])?;
                output.write_all(&length.to_be_bytes())?;
            } else {
                output.write_all(&[0xc9])?;
                output.write_all(&u32::try_from(length)?.to_be_bytes())?;
            }
        }
    }
    output.write_all(&[kind as u8])?;
    Ok(())
}
fn array_tag(tag: u8) -> bool {
    matches!(tag, 0x90..=0x9f | 0xdc | 0xdd)
}
fn extension_tag(tag: u8) -> bool {
    matches!(tag,0xc7..=0xc9|0xd4..=0xd8)
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
    fn peek(&self) -> Result<u8> {
        self.bytes
            .get(self.position)
            .copied()
            .context("MessagePack asset map is truncated")
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .context("MessagePack position overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .context("MessagePack asset map is truncated")?;
        self.position = end;
        Ok(bytes)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .context("MessagePack fixed-size field mismatch")
    }
    fn array(&mut self) -> Result<usize> {
        let count = match self.byte()? {
            value @ 0x90..=0x9f => u32::from(value & 0x0f),
            0xdc => u32::from(u16::from_be_bytes(self.fixed()?)),
            0xdd => u32::from_be_bytes(self.fixed()?),
            _ => bail!("expected a MessagePack array"),
        };
        Ok(usize::try_from(count)?)
    }
    fn integer(&mut self) -> Result<i64> {
        Ok(match self.byte()? {
            value @ 0x00..=0x7f => i64::from(value),
            value @ 0xe0..=0xff => i64::from(value as i8),
            0xcc => i64::from(self.byte()?),
            0xcd => i64::from(u16::from_be_bytes(self.fixed()?)),
            0xce => i64::from(u32::from_be_bytes(self.fixed()?)),
            0xcf => i64::try_from(u64::from_be_bytes(self.fixed()?))?,
            0xd0 => i64::from(self.byte()? as i8),
            0xd1 => i64::from(i16::from_be_bytes(self.fixed()?)),
            0xd2 => i64::from(i32::from_be_bytes(self.fixed()?)),
            0xd3 => i64::from_be_bytes(self.fixed()?),
            _ => bail!("expected a MessagePack integer"),
        })
    }
    fn string(&mut self) -> Result<String> {
        let length = match self.byte()? {
            value @ 0xa0..=0xbf => usize::from(value & 0x1f),
            0xd9 => usize::from(self.byte()?),
            0xda => usize::from(u16::from_be_bytes(self.fixed()?)),
            0xdb => usize::try_from(u32::from_be_bytes(self.fixed()?))?,
            0xc0 => return Ok(String::new()),
            _ => bail!("expected a MessagePack string"),
        };
        ensure!(
            length <= MAX_STRING_BYTES,
            "MessagePack string exceeds 4096 bytes"
        );
        let value =
            std::str::from_utf8(self.take(length)?).context("MessagePack string is not UTF-8")?;
        Ok(value.to_owned())
    }
    fn extension(&mut self) -> Result<(i8, &'a [u8])> {
        let length = match self.byte()? {
            0xd4 => 1,
            0xd5 => 2,
            0xd6 => 4,
            0xd7 => 8,
            0xd8 => 16,
            0xc7 => usize::from(self.byte()?),
            0xc8 => usize::from(u16::from_be_bytes(self.fixed()?)),
            0xc9 => usize::try_from(u32::from_be_bytes(self.fixed()?))?,
            _ => bail!("expected a MessagePack extension"),
        };
        let kind = self.byte()? as i8;
        Ok((kind, self.take(length)?))
    }
    fn binary(&mut self) -> Result<&'a [u8]> {
        let length = match self.byte()? {
            0xc4 => usize::from(self.byte()?),
            0xc5 => usize::from(u16::from_be_bytes(self.fixed()?)),
            0xc6 => usize::try_from(u32::from_be_bytes(self.fixed()?))?,
            _ => bail!("expected a MessagePack binary LZ4 block"),
        };
        self.take(length)
    }
}
