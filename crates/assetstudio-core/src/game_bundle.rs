//! In-place normalization of fork-specific UnityFS compressed-block wrappers.
//!
//! The directory, block sizes and serialized payloads are unchanged. The
//! standard backend still validates and decompresses every resulting block.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use super::crypto;

const MAX_INFO_BYTES: usize = 64 * 1024 * 1024;
const MAX_BLOCKS: usize = 1_000_000;

pub(super) fn normalize(data: &[u8], profile: &str, path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let mut reader = Reader {
        bytes: data,
        position: 0,
    };
    ensure!(
        reader.string(32)? == b"UnityFS",
        "Expected a UnityFS bundle"
    );
    let version = reader.u32()?;
    ensure!(
        (6..=8).contains(&version),
        "Game bundle version {version} is not implemented"
    );
    reader.string(32768)?;
    reader.string(32768)?;
    let size = reader.u64()?;
    ensure!(
        size == data.len() as u64,
        "Game bundle declared size differs from its input"
    );
    let compressed_info = usize::try_from(reader.u32()?)?;
    let uncompressed_info = usize::try_from(reader.u32()?)?;
    let flags = reader.u32()?;
    ensure!(
        flags & !0x3ff == 0,
        "Game bundle uses unimplemented encryption flags {flags:#x}"
    );
    ensure!(
        compressed_info <= MAX_INFO_BYTES && uncompressed_info <= MAX_INFO_BYTES,
        super::limit("Game bundle block table exceeds 64 MiB")
    );
    ensure!(
        uncompressed_info as u64 <= maximum,
        super::limit("Game bundle table exceeds byte budget")
    );
    if version >= 7 {
        reader.align16()?;
    }
    let at_end = flags & 0x80 != 0;
    let info_start = if at_end {
        data.len()
            .checked_sub(compressed_info)
            .context("Game table size underflow")?
    } else {
        reader.position
    };
    let info_end = info_start
        .checked_add(compressed_info)
        .context("Game table size overflow")?;
    let info_source = data
        .get(info_start..info_end)
        .context("Game bundle block table is truncated")?;
    let mut output = super::copy_bytes(data)?;
    if profile == "PerpetualNovelty" && matches!(flags & 0x3f, 2 | 3) {
        ensure!(
            info_source.len() >= 2,
            "PerpetualNovelty encrypted block table is truncated"
        );
        let key = info_source[1];
        for byte in &mut output[info_start..info_start + compressed_info.min(50)] {
            *byte ^= key;
        }
    }
    let info = decompress_table(
        &output[info_start..info_end],
        uncompressed_info,
        flags & 0x3f,
    )?;
    let mut table = Reader {
        bytes: &info,
        position: 0,
    };
    table.take(16)?; // Content hash.
    let count = usize::try_from(table.u32()?)?;
    ensure!(
        count <= MAX_BLOCKS,
        super::limit("Game bundle block count exceeds its limit")
    );
    ensure!(
        count <= table.remaining() / 10,
        "Game bundle block count is invalid"
    );
    let mut data_position = if at_end { reader.position } else { info_end };
    if flags & 0x200 != 0 {
        data_position = align16(data_position)?;
    }
    let data_end = if at_end { info_start } else { data.len() };
    let mut total_expanded = 0_u64;
    for index in 0..count {
        let expanded = u64::from(table.u32()?);
        let compressed = usize::try_from(table.u32()?)?;
        let block_flags = table.u16()?;
        total_expanded = total_expanded
            .checked_add(expanded)
            .with_context(|| super::limit("Game block expansion overflow"))?;
        ensure!(
            total_expanded <= maximum,
            super::limit("Game bundle expansion exceeds byte budget")
        );
        let end = data_position
            .checked_add(compressed)
            .context("Game block offset overflow")?;
        ensure!(
            end <= data_end,
            "Game bundle compressed block overlaps table or end of input"
        );
        let block = output
            .get_mut(data_position..end)
            .context("Game compressed block is truncated")?;
        match (profile, block_flags & 0x3f) {
            ("OPFP", 2 | 3) => decode_opfp(block, path)?,
            ("NetEase", 1) if index == 0 => decode_netease_raw(block)?,
            ("NetEase", 2 | 3) if index == 0 => decode_netease_header(block)?,
            _ => {}
        }
        data_position = end;
    }
    // Validate the trailing directory's count/strings/ranges through the
    // standard backend, rather than duplicating its directory parser here.
    ensure!(
        table.remaining() >= 4,
        "Game bundle directory count is missing"
    );
    Ok(output)
}

pub(super) fn decompress_table(input: &[u8], expected: usize, compression: u32) -> Result<Vec<u8>> {
    ensure!(
        input.len() <= MAX_INFO_BYTES && expected <= MAX_INFO_BYTES,
        super::limit("Game bundle block table exceeds 64 MiB")
    );
    match compression {
        0 => {
            ensure!(
                input.len() == expected,
                "Uncompressed game table size mismatch"
            );
            super::copy_bytes(input)
        }
        1 => {
            let properties = *input
                .first()
                .context("LZMA game table has no properties byte")?;
            let dictionary = u32::from_le_bytes(
                input
                    .get(1..5)
                    .context("LZMA game table header is truncated")?
                    .try_into()?,
            );
            let memory = lzma_rust2::lzma_get_memory_usage_by_props(dictionary, properties)?;
            ensure!(
                memory <= 64 * 1024,
                super::limit("LZMA game table decoder memory exceeds 64 MiB")
            );
            let mut decoder = lzma_rust2::LzmaReader::new_with_props(
                std::io::Cursor::new(&input[5..]),
                expected as u64,
                properties,
                dictionary,
                None,
            )?;
            let mut output = Vec::new();
            output
                .try_reserve_exact(expected)
                .map_err(super::allocation_limit)?;
            output.resize(expected, 0);
            decoder.read_exact(&mut output)?;
            let mut tail = [0_u8; 1];
            ensure!(
                decoder.read(&mut tail)? == 0,
                "LZMA game table exceeds its declared size"
            );
            Ok(output)
        }
        2 | 3 => {
            let mut output = Vec::new();
            output
                .try_reserve_exact(expected)
                .map_err(super::allocation_limit)?;
            output.resize(expected, 0);
            let written = lz4_flex::block::decompress_into(input, &mut output)?;
            ensure!(
                written == expected,
                "LZ4 game table decompressed size mismatch"
            );
            Ok(output)
        }
        _ => bail!("Game wrapper block-table compression {compression} is not implemented"),
    }
}

fn decode_opfp(bytes: &mut [u8], path: &Path) -> Result<()> {
    let components: Vec<_> = path.components().collect();
    let Some(base) = components
        .iter()
        .position(|component| component.as_os_str() == "BundleResources")
    else {
        return Ok(());
    };
    let relative = components[base + 1..]
        .iter()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let lower = relative.to_ascii_lowercase();
    let version1 = [
        "dynamicatlas/",
        "atlas/skill",
        "atlas/playertitle",
        "uitexture/herocardep12",
        "uitexture/herocardep13",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));
    let version0 = ["ui/", "atlas/", "uitexture/"]
        .iter()
        .any(|prefix| lower.starts_with(prefix));
    if version1 {
        let name = relative.rsplit('/').next().unwrap_or("");
        let key = name.encode_utf16().count() as u8;
        for index in 1..bytes.len() {
            let target = (index + bytes.len() + usize::from(key).pow(2)) % (index + 1);
            bytes.swap(index, target);
            bytes[index] ^= key;
            bytes[target] ^= key;
        }
    } else if version0 && !bytes.is_empty() {
        let key = relative.encode_utf16().count() as u8;
        bytes[0] ^= key;
        for index in 1..bytes.len() {
            bytes[index] ^= bytes[index - 1];
        }
    }
    Ok(())
}

fn decode_netease_header(bytes: &mut [u8]) -> Result<()> {
    let position = bytes
        .windows(2)
        .take(64)
        .position(|chunk| chunk == [0xee, 0xdd])
        .context("NetEase encrypted block header was not found")?;
    let block_length = bytes.len();
    let info = bytes
        .get_mut(position..)
        .context("NetEase header position overflow")?;
    ensure!(
        info.len() >= 11,
        "NetEase encrypted block header is truncated"
    );
    let version = u16::from_le_bytes([info[2], info[3]]);
    ensure!(
        (0x2017..=0x2025).contains(&version),
        "NetEase encrypted block version {version:#x} is not implemented"
    );
    let digits = format!("{version:04X}");
    info[..4].copy_from_slice(digits.as_bytes());
    let (vectors, byte_count) = (info[4], info[6]);
    let mut encrypted_size = block_length.min(0x1000);
    if vectors != 0x2e && byte_count != 0x2e {
        encrypted_size = if vectors == 0xaa && byte_count == 0xbb {
            0x1000
        } else {
            usize::from(byte_count) + 16 * usize::from(vectors)
        };
        info[4] = 0x2e;
        info[6] = 0x2e;
    }
    let mut header_offset = 0_usize;
    for index in [8, 9] {
        if info[index + 1] == 0x31 && info[index] != 0x66 {
            header_offset = usize::from(info[index]);
            info[index] = 0x66;
        }
    }
    let header_size: usize = if header_offset == position || header_offset == 0 {
        if position >= 0x20 { 0x40 } else { 0x30 }
    } else if header_offset >= 0x20 {
        0x40
    } else {
        0x30
    };
    let mut encrypted_offset = header_size;
    if header_offset != position && header_offset != 0 && header_offset > position {
        encrypted_offset = encrypted_offset
            .checked_sub(header_offset - position)
            .context("NetEase encrypted block offset is negative")?;
    }
    encrypted_size = encrypted_size
        .checked_sub(header_size)
        .context("NetEase encrypted block size is smaller than its header")?;
    let end = encrypted_offset
        .checked_add(encrypted_size)
        .context("NetEase encrypted block range overflow")?;
    decode_netease(
        bytes
            .get_mut(encrypted_offset..end)
            .context("NetEase encrypted block range is truncated")?,
    )
}

fn decode_netease_raw(bytes: &mut [u8]) -> Result<()> {
    let length = bytes.len().min(0x1000);
    decode_netease(&mut bytes[..length])
}

fn decode_netease(bytes: &mut [u8]) -> Result<()> {
    ensure!(
        bytes.len() >= 0x20,
        "NetEase encrypted region is shorter than its seed header"
    );
    let word =
        |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]));
    let mut seed_bytes = [0_u8; 20];
    for (slot, value) in seed_bytes.as_chunks_mut::<4>().0.iter_mut().zip([
        word(12),
        word(4),
        word(16),
        bytes.len() as u32,
        word(8),
    ]) {
        *slot = value.to_le_bytes();
    }
    let seed = netease_crc(&seed_bytes);
    let key0 = seed ^ word(28).wrapping_add(0x1981);
    let key1 = seed ^ (bytes.len() as u32).wrapping_add(0x2013);
    let key2 = seed ^ word(20).wrapping_add(0x1985);
    let key3 = seed ^ word(24).wrapping_add(0x2018);
    for byte in &mut bytes[..0x20] {
        *byte ^= 0xa6;
    }
    let block = &mut bytes[0x20..];
    if block.len() < 0x80 {
        return netease_rc4(block, seed);
    }
    let mut key_block = [0_u8; 0x80];
    key_block.copy_from_slice(&block[..0x80]);
    netease_rc4(&mut block[..0x80], seed)?;
    netease_rc4(&mut key_block, key1)?;
    let key_words: Vec<u32> = key_block
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    let vector = [key2, key0, key1, key3];
    let keys = [0x571, key3, 0x892, 0x750, key2, key0, 0x746, key1, 0x568];
    let data = &mut block[0x80..];
    let (chunks, tail) = data.as_chunks_mut::<0x80>();
    for (index, chunk) in chunks.iter_mut().enumerate() {
        let kind = (keys[index % keys.len()] as u8) % 4;
        for (j, bytes) in chunk.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let extra = match kind {
                0 => keys[j % keys.len()] ^ (32 - j) as u32,
                1 => vector[usize::from(key_words[j] as u8) % 4],
                2 => vector[usize::from(key_words[j] as u8) % 4] ^ j as u32,
                _ => vector[usize::from(keys[j % keys.len()] as u8) % 4] ^ j as u32,
            };
            let value = u32::from_le_bytes(*bytes) ^ key_words[j] ^ extra;
            *bytes = value.to_le_bytes();
        }
    }
    for (index, byte) in tail.iter_mut().enumerate() {
        let key_index = (vector[index % 4] as usize) % keys.len();
        *byte ^= key_block[index] ^ (keys[key_index] % 255) as u8 ^ index as u8;
    }
    Ok(())
}

fn netease_crc(bytes: &[u8]) -> u32 {
    let mut table = [0_u32; 256];
    for (index, item) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            if value & 1 != 0 {
                value ^= 0x0982_3d6e;
            }
            value >>= 1;
        }
        *item = value;
    }
    let mut value = u32::MAX;
    for byte in bytes {
        value = (table[usize::from(value as u8 ^ *byte)] ^ (value >> 8)).wrapping_add(0x10);
    }
    (!value).wrapping_sub(0x7d29_c488)
}

fn netease_rc4(bytes: &mut [u8], key: u32) -> Result<()> {
    // Obtain the standard RC4 keystream with the tested compatibility helper,
    // then apply NetEase's rotate/add postprocessing to that keystream only.
    let mut stream = Vec::new();
    stream
        .try_reserve_exact(bytes.len())
        .map_err(super::allocation_limit)?;
    stream.resize(bytes.len(), 0);
    crypto::rc4(&mut stream, &key.to_le_bytes())?;
    for (byte, key) in bytes.iter_mut().zip(stream) {
        *byte ^= key.rotate_right(2).wrapping_add(0x3a);
    }
    Ok(())
}

fn align16(value: usize) -> Result<usize> {
    value
        .checked_add(15)
        .map(|value| value & !15)
        .context("Game bundle alignment overflow")
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
    fn take(&mut self, length: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(length)
            .context("Game bundle read range overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .context("Game bundle is truncated")?;
        self.position = end;
        Ok(value)
    }
    fn string(&mut self, maximum: usize) -> Result<&[u8]> {
        let length = self.bytes[self.position..]
            .iter()
            .take(maximum)
            .position(|byte| *byte == 0)
            .context("Game bundle string is unterminated or too long")?;
        let value = self.take(length + 1)?;
        Ok(&value[..length])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
    }
    fn align16(&mut self) -> Result<()> {
        let position = align16(self.position)?;
        self.take(position - self.position)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real UnityFS wire layout containing one compressed resource. The
    /// bytes are synthetic, but block-table alignment and flags are real.
    fn bundle_fixture(at_end: bool) -> (Vec<u8>, usize, usize) {
        let plain = b"a synthetic decompressed resource payload";
        let payload = lz4_flex::block::compress(plain);
        let mut info = vec![0; 16];
        info.extend_from_slice(&1_u32.to_be_bytes());
        info.extend_from_slice(&(plain.len() as u32).to_be_bytes());
        info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        info.extend_from_slice(&2_u16.to_be_bytes());
        info.extend_from_slice(&1_u32.to_be_bytes());
        info.extend_from_slice(&0_u64.to_be_bytes());
        info.extend_from_slice(&(plain.len() as u64).to_be_bytes());
        info.extend_from_slice(&0_u32.to_be_bytes());
        info.extend_from_slice(b"fixture.resS\0");
        let compressed_info = lz4_flex::block::compress(&info);
        assert_eq!(
            compressed_info[1], 0,
            "PerpetualNovelty header fixture requires the first LZ4 literal to be zero"
        );
        let mut output = b"UnityFS\0".to_vec();
        output.extend_from_slice(&7_u32.to_be_bytes());
        output.extend_from_slice(b"5.x.x\0");
        output.extend_from_slice(b"2022.3.0f1\0");
        let size_offset = output.len();
        output.extend_from_slice(&0_u64.to_be_bytes());
        output.extend_from_slice(&(compressed_info.len() as u32).to_be_bytes());
        output.extend_from_slice(&(info.len() as u32).to_be_bytes());
        output.extend_from_slice(&(if at_end { 0xc2_u32 } else { 0x42_u32 }).to_be_bytes());
        while !output.len().is_multiple_of(16) {
            output.push(0);
        }
        let info_offset;
        let payload_offset;
        if at_end {
            payload_offset = output.len();
            output.extend_from_slice(&payload);
            info_offset = output.len();
            output.extend_from_slice(&compressed_info);
        } else {
            info_offset = output.len();
            output.extend_from_slice(&compressed_info);
            payload_offset = output.len();
            output.extend_from_slice(&payload);
        }
        let size = output.len() as u64;
        output[size_offset..size_offset + 8].copy_from_slice(&size.to_be_bytes());
        (output, info_offset, payload_offset)
    }

    #[test]
    fn perpetual_normalizes_front_and_end_tables_without_changing_payload() {
        for at_end in [false, true] {
            let (expected, info_offset, payload_offset) = bundle_fixture(at_end);
            let mut encrypted = expected.clone();
            let info_length = if at_end {
                expected.len() - info_offset
            } else {
                payload_offset - info_offset
            };
            for byte in &mut encrypted[info_offset..info_offset + info_length.min(50)] {
                *byte ^= 0x5a;
            }
            let decoded = normalize(
                &encrypted,
                "PerpetualNovelty",
                Path::new("fixture.ab"),
                4096,
            )
            .unwrap();
            assert_eq!(decoded, expected);
            assert_resource_readable(decoded);
        }
    }

    #[test]
    fn opfp_normalizes_compressed_blocks_without_rewriting_metadata() {
        for at_end in [false, true] {
            let (expected, info_offset, payload_offset) = bundle_fixture(at_end);
            let end = if at_end { info_offset } else { expected.len() };
            let mut encrypted = expected.clone();
            let block = &mut encrypted[payload_offset..end];
            let key = b"UI/icon.ab".len() as u8;
            for index in (1..block.len()).rev() {
                block[index] ^= block[index - 1];
            }
            block[0] ^= key;
            let decoded = normalize(
                &encrypted,
                "OPFP",
                Path::new("/game/BundleResources/UI/icon.ab"),
                4096,
            )
            .unwrap();
            assert_eq!(decoded, expected);
            assert_resource_readable(decoded);
        }
    }

    fn assert_resource_readable(bytes: Vec<u8>) {
        let region = unity_rs_core::source::Region::from_bytes(bytes);
        let bundle = unity_rs_core::bundle::UnityFsBundle::open(&region).unwrap();
        assert_eq!(
            bundle.read_entry(0).unwrap(),
            b"a synthetic decompressed resource payload"
        );
    }

    #[test]
    fn mutated_bundle_headers_never_panic_or_allocate_from_unbounded_counts() {
        let (valid, _, _) = bundle_fixture(false);
        for index in 0..valid.len() {
            let mut bytes = valid.clone();
            bytes[index] ^= 0xff;
            let outcome = std::panic::catch_unwind(|| {
                normalize(
                    &bytes,
                    "OPFP",
                    Path::new("/game/BundleResources/UI/icon.ab"),
                    4096,
                )
            });
            assert!(
                outcome.is_ok(),
                "bundle header mutation at {index} panicked"
            );
        }
    }

    #[test]
    fn netease_matches_original_csharp_short_long_and_header_golden_vectors() {
        for (length, expected) in [
            (
                80,
                include_bytes!("../tests/fixtures/game-oracle/raw-80-expected.bin").as_slice(),
            ),
            (
                4100,
                include_bytes!("../tests/fixtures/game-oracle/raw-4100-expected.bin").as_slice(),
            ),
        ] {
            let mut bytes: Vec<u8> = (0..length)
                .map(|index| (index as u8).wrapping_mul(19).wrapping_add(71))
                .collect();
            decode_netease_raw(&mut bytes).unwrap();
            assert_eq!(bytes, expected);
        }
        let mut bytes: Vec<u8> = (0..800)
            .map(|index| (index as u8).wrapping_mul(19).wrapping_add(71))
            .collect();
        bytes[..4].copy_from_slice(&[0xee, 0xdd, 0x17, 0x20]);
        bytes[4] = 0x2e;
        bytes[6] = 0x2e;
        bytes[8] = 0x66;
        bytes[9] = 0x66;
        bytes[10] = 0x31;
        decode_netease_header(&mut bytes).unwrap();
        assert_eq!(
            bytes,
            include_bytes!("../tests/fixtures/game-oracle/header-800-expected.bin")
        );
    }

    #[test]
    fn opfp_v0_decodes_reference_chain() {
        let path = Path::new("/game/BundleResources/UI/icon.ab");
        let plain = b"uncompressed reference bytes";
        let key = b"UI/icon.ab".len() as u8;
        let mut encrypted = plain.to_vec();
        for index in (1..encrypted.len()).rev() {
            encrypted[index] ^= plain[index - 1];
        }
        encrypted[0] ^= key;
        decode_opfp(&mut encrypted, path).unwrap();
        assert_eq!(encrypted, plain);
    }

    #[test]
    fn opfp_v1_reverses_swaps_and_two_xors() {
        let path = Path::new("/game/BundleResources/DynamicAtlas/icon.ab");
        let expected = b"uncompressed reference bytes";
        let key = b"icon.ab".len() as u8;
        let mut encrypted = expected.to_vec();
        for index in (1..encrypted.len()).rev() {
            let target = (index + encrypted.len() + usize::from(key).pow(2)) % (index + 1);
            encrypted[index] ^= key;
            encrypted[target] ^= key;
            encrypted.swap(index, target);
        }
        decode_opfp(&mut encrypted, path).unwrap();
        assert_eq!(encrypted, expected);
    }

    #[test]
    fn netease_bad_headers_fail_without_panics() {
        for length in 0..512 {
            let original: Vec<u8> = (0..length)
                .map(|index| (index as u8).wrapping_mul(57))
                .collect();
            let outcome = std::panic::catch_unwind(|| {
                let mut bytes = original.clone();
                decode_netease_header(&mut bytes)
            });
            assert!(outcome.is_ok());
        }
    }
}
