//! Canonical UnityFS reconstruction for the baseline Naraka/TOT/BH3PrePre paths.
//! Decryption can remove prefixes from compressed blocks, so block/table sizes
//! are rewritten together rather than hiding an invalid block behind flags.

use anyhow::{Context, Result, ensure};

const MAX_INFO_BYTES: usize = 64 * 1024 * 1024;
const MAX_BLOCKS: usize = 1_000_000;
const NARAKA_MAGIC: &[u8] = &[0x15, 0x1e, 0x1c, 0x0d, 0x0d, 0x23, 0x21];

struct Header<'a> {
    version: u32,
    engine: &'a [u8],
    revision: &'a [u8],
    compressed_info: usize,
    expanded_info: usize,
    flags: u32,
    payload_start: usize,
    has_hash: bool,
}

pub(super) fn normalize(input: &[u8], profile: &str, maximum: u64) -> Result<Vec<u8>> {
    if profile != "TOT" {
        return normalize_single(input, profile, maximum);
    }
    ensure!(
        input.len() as u64 <= maximum,
        super::limit("Game bundle exceeds byte budget")
    );
    let first = standard_bundle_length(input)?;
    if first == input.len() {
        return normalize_single(input, profile, maximum);
    }
    let mut cursor = 0_usize;
    let mut parts = Vec::new();
    let mut retained = 0_u64;
    while cursor < input.len() {
        ensure!(
            parts.len() < 16_384,
            super::limit("Concatenated game bundle count exceeds its limit")
        );
        let size = standard_bundle_length(&input[cursor..])?;
        let end = cursor
            .checked_add(size)
            .context("Concatenated game bundle range overflow")?;
        let encoded = input
            .get(cursor..end)
            .context("Concatenated game bundle is truncated")?;
        let decoded = normalize_single(
            encoded,
            profile,
            maximum
                .checked_sub(retained)
                .with_context(|| super::limit("Concatenated game bundle budget exhausted"))?,
        )?;
        retained = retained
            .checked_add(decoded.len() as u64)
            .with_context(|| super::limit("Concatenated game bundle size overflow"))?;
        parts.try_reserve(1).map_err(super::allocation_limit)?;
        parts.push((format!("bundle_{cursor:016x}.bundle"), decoded));
        cursor = end;
    }
    canonical_web(parts, maximum)
}

fn standard_bundle_length(input: &[u8]) -> Result<usize> {
    let mut reader = Reader::new(input);
    let signature = reader.string(32)?;
    ensure!(
        matches!(signature, b"UnityFS" | b"UnityWeb" | b"UnityRaw" | b"ENCR"),
        "Concatenated TOT unit has no UnityFS header"
    );
    if signature != b"ENCR" {
        ensure!(
            (6..=8).contains(&reader.u32()?),
            "Concatenated TOT unit uses an unknown version"
        );
        reader.string(32768)?;
        reader.string(32768)?;
    }
    let size = usize::try_from(reader.u64()?)?;
    ensure!(
        size > reader.position && size <= input.len(),
        "Concatenated game bundle size is invalid"
    );
    Ok(size)
}

fn canonical_web(parts: Vec<(String, Vec<u8>)>, maximum: u64) -> Result<Vec<u8>> {
    const SIGNATURE: &[u8] = b"UnityWebData1.0\0";
    let mut header = SIGNATURE.len() + 4;
    let mut payload = 0_usize;
    for (name, data) in &parts {
        header = header
            .checked_add(12)
            .and_then(|n| n.checked_add(name.len()))
            .with_context(|| super::limit("Game bundle directory size overflow"))?;
        payload = payload
            .checked_add(data.len())
            .with_context(|| super::limit("Game bundle payload size overflow"))?;
    }
    ensure!(
        header <= MAX_INFO_BYTES,
        super::limit("Concatenated game bundle directory exceeds 64 MiB")
    );
    let total = header
        .checked_add(payload)
        .with_context(|| super::limit("Game bundle output size overflow"))?;
    ensure!(
        total as u64 <= maximum,
        super::limit("Concatenated game bundles exceed byte budget")
    );
    let mut output = Vec::new();
    output
        .try_reserve_exact(total)
        .map_err(super::allocation_limit)?;
    output.extend_from_slice(SIGNATURE);
    output.extend_from_slice(&i32::try_from(header)?.to_le_bytes());
    let mut offset = header;
    for (name, data) in &parts {
        output.extend_from_slice(&i32::try_from(offset)?.to_le_bytes());
        output.extend_from_slice(&i32::try_from(data.len())?.to_le_bytes());
        output.extend_from_slice(&i32::try_from(name.len())?.to_le_bytes());
        output.extend_from_slice(name.as_bytes());
        offset = offset
            .checked_add(data.len())
            .with_context(|| super::limit("Game bundle output offset overflow"))?;
    }
    for (_, data) in parts {
        output.extend_from_slice(&data);
    }
    Ok(output)
}

fn normalize_single(input: &[u8], profile: &str, maximum: u64) -> Result<Vec<u8>> {
    ensure!(
        input.len() as u64 <= maximum,
        super::limit("Game bundle exceeds byte budget")
    );
    let header = header(input, profile)?;
    ensure!(header.flags & !0x3ff == 0, "Game bundle uses unknown flags");
    ensure!(
        header.compressed_info <= MAX_INFO_BYTES && header.expanded_info <= MAX_INFO_BYTES,
        super::limit("Game bundle block table exceeds 64 MiB")
    );
    ensure!(
        header.expanded_info as u64 <= maximum,
        super::limit("Game table exceeds byte budget")
    );
    let at_end = header.flags & 0x80 != 0;
    let info_start = if at_end {
        input
            .len()
            .checked_sub(header.compressed_info)
            .context("Game table range underflow")?
    } else {
        header.payload_start
    };
    ensure!(
        info_start >= header.payload_start,
        "Game table overlaps its header"
    );
    let info_end = info_start
        .checked_add(header.compressed_info)
        .context("Game table range overflow")?;
    let info_encoded = input
        .get(info_start..info_end)
        .context("Game block table is truncated")?;
    let compression = header.flags & 0x3f;
    let decoded_info;
    let info_encoded = if compression == 5 {
        decoded_info = if info_encoded.starts_with(b"mr0k") {
            super::mr0k::decrypt(info_encoded, profile)?
        } else {
            super::copy_bytes(info_encoded)?
        };
        decoded_info.as_slice()
    } else {
        info_encoded
    };
    let mut info = super::bundle::decompress_table(
        info_encoded,
        header.expanded_info,
        if compression == 5 { 3 } else { compression },
    )?;
    if !header.has_hash {
        ensure!(
            info.len() <= MAX_INFO_BYTES - 16,
            super::limit("ENCR table exceeds normalized size limit")
        );
        info.try_reserve(16).map_err(super::allocation_limit)?;
        info.splice(0..0, [0u8; 16]);
    }
    let mut table = Reader::new(&info);
    table.take(16)?;
    let count = usize::try_from(table.u32()?)?;
    ensure!(
        count <= MAX_BLOCKS,
        super::limit("Game bundle block count exceeds its limit")
    );
    ensure!(
        count <= table.remaining() / 10,
        "Game block count is invalid"
    );
    let mut descriptors = Vec::new();
    descriptors
        .try_reserve_exact(count)
        .map_err(super::allocation_limit)?;
    let mut expanded = 0u64;
    for _ in 0..count {
        let expanded_size = table.u32()?;
        let compressed_size = usize::try_from(table.u32()?)?;
        let flags = table.u16()?;
        ensure!(flags & !0x7f == 0, "Game block uses unknown flags");
        let compression = flags & 0x3f;
        ensure!(
            matches!(compression, 0..=3 | 5) || profile == "ExAstris" && compression == 4,
            "Game block uses an unknown compression"
        );
        expanded = expanded
            .checked_add(u64::from(expanded_size))
            .with_context(|| super::limit("Game expansion overflow"))?;
        ensure!(
            expanded <= maximum,
            super::limit("Game block expansion exceeds byte budget")
        );
        descriptors.push((expanded_size, compressed_size, flags));
    }
    // The backend validates directory names, ranges and all decompressed data.
    ensure!(table.remaining() >= 4, "Game directory count is missing");
    let mut cursor = if at_end {
        header.payload_start
    } else {
        info_end
    };
    if header.flags & 0x200 != 0 {
        cursor = align16(cursor)?;
    }
    let data_end = if at_end { info_start } else { input.len() };
    ensure!(cursor <= data_end, "Game payload range is invalid");
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(data_end - cursor)
        .map_err(super::allocation_limit)?;
    for (index, (expanded, size, flags)) in descriptors.into_iter().enumerate() {
        let end = cursor
            .checked_add(size)
            .context("Game block range overflow")?;
        ensure!(end <= data_end, "Game block overlaps table or end of input");
        let block = input.get(cursor..end).context("Game block is truncated")?;
        let decoded;
        let custom_lz4 = profile == "ExAstris" && matches!(flags & 0x3f, 4 | 5);
        let mr0k_lz4 = matches!(profile, "TOT" | "BH3PrePre") && flags & 0x3f == 5;
        let block = if custom_lz4 {
            decoded = normalize_lz4_lit(block, usize::try_from(expanded)?)?;
            decoded.as_slice()
        } else if mr0k_lz4 && block.starts_with(b"mr0k") {
            decoded = super::mr0k::decrypt(block, profile)?;
            decoded.as_slice()
        } else {
            block
        };
        let new_size = u32::try_from(block.len())?;
        let new_flags = if custom_lz4 || mr0k_lz4 {
            flags & !0x3f | 3
        } else {
            flags
        };
        let descriptor = 20usize
            .checked_add(index.checked_mul(10).context("Game block index overflow")?)
            .context("Game block index overflow")?;
        info[descriptor + 4..descriptor + 8].copy_from_slice(&new_size.to_be_bytes());
        info[descriptor + 8..descriptor + 10].copy_from_slice(&new_flags.to_be_bytes());
        payload.extend_from_slice(block);
        cursor = end;
    }
    ensure!(
        cursor == data_end,
        "Game bundle has unaccounted trailing payload bytes"
    );
    let output_length = 8usize
        .checked_add(4)
        .and_then(|n| n.checked_add(header.engine.len() + 1))
        .and_then(|n| n.checked_add(header.revision.len() + 1))
        .and_then(|n| n.checked_add(20))
        .with_context(|| super::limit("Game normalized header size overflow"))?;
    let output_length = if header.version >= 7 {
        align16(output_length)?
    } else {
        output_length
    };
    let total = output_length
        .checked_add(info.len())
        .and_then(|n| n.checked_add(payload.len()))
        .with_context(|| super::limit("Game normalized size overflow"))?;
    ensure!(
        total as u64 <= maximum,
        super::limit("Normalized game bundle exceeds byte budget")
    );
    let mut output = Vec::new();
    output
        .try_reserve_exact(total)
        .map_err(super::allocation_limit)?;
    output.extend_from_slice(b"UnityFS\0");
    output.extend_from_slice(&header.version.to_be_bytes());
    output.extend_from_slice(header.engine);
    output.push(0);
    output.extend_from_slice(header.revision);
    output.push(0);
    output.extend_from_slice(&(total as u64).to_be_bytes());
    output.extend_from_slice(&u32::try_from(info.len())?.to_be_bytes());
    output.extend_from_slice(&u32::try_from(info.len())?.to_be_bytes());
    output.extend_from_slice(&0x40u32.to_be_bytes());
    output.resize(output_length, 0);
    output.extend_from_slice(&info);
    output.extend_from_slice(&payload);
    Ok(output)
}

/// Ex Astris reverses the literal/match nibbles of each LZ4 token only. Walk
/// the bounded compressed syntax to change those bytes, retaining offsets and
/// literal data verbatim; the normal backend still performs decompression.
fn normalize_lz4_lit(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    ensure!(!input.is_empty(), "LZ4Lit stream is empty");
    let mut output = super::copy_bytes(input)?;
    let mut cursor = 0_usize;
    let mut produced = 0_usize;
    while cursor < input.len() {
        let token = input[cursor];
        output[cursor] = token.rotate_left(4);
        cursor += 1;
        let literal = extended_lz4_length(input, &mut cursor, usize::from(token & 15))?;
        cursor = cursor
            .checked_add(literal)
            .context("LZ4Lit literal offset overflow")?;
        ensure!(cursor <= input.len(), "LZ4Lit literal is truncated");
        produced = produced
            .checked_add(literal)
            .context("LZ4Lit output size overflow")?;
        ensure!(
            produced <= expected,
            "LZ4Lit literal exceeds declared output"
        );
        if cursor == input.len() {
            break;
        }
        let end = cursor
            .checked_add(2)
            .context("LZ4Lit offset range overflow")?;
        let offset = u16::from_le_bytes(
            input
                .get(cursor..end)
                .context("LZ4Lit back-reference is truncated")?
                .try_into()?,
        );
        cursor = end;
        ensure!(
            offset != 0 && usize::from(offset) <= produced,
            "LZ4Lit back-reference is invalid"
        );
        let length = extended_lz4_length(input, &mut cursor, usize::from(token >> 4))?
            .checked_add(4)
            .context("LZ4Lit match length overflow")?;
        produced = produced
            .checked_add(length)
            .context("LZ4Lit output size overflow")?;
        ensure!(produced <= expected, "LZ4Lit match exceeds declared output");
    }
    ensure!(
        produced == expected,
        "LZ4Lit decoded size differs from its declaration"
    );
    Ok(output)
}

fn extended_lz4_length(input: &[u8], cursor: &mut usize, mut length: usize) -> Result<usize> {
    if length == 15 {
        loop {
            let next = *input
                .get(*cursor)
                .context("LZ4Lit extended length is truncated")?;
            *cursor = cursor
                .checked_add(1)
                .context("LZ4Lit input offset overflow")?;
            length = length
                .checked_add(usize::from(next))
                .context("LZ4Lit length overflow")?;
            if next != 255 {
                break;
            }
        }
    }
    Ok(length)
}

fn header<'a>(input: &'a [u8], profile: &str) -> Result<Header<'a>> {
    let mut reader = Reader::new(input);
    let signature = reader.string(32)?;
    ensure!(
        matches!(signature, b"UnityFS" | b"UnityWeb" | b"UnityRaw" | b"ENCR")
            || profile == "Naraka" && signature == NARAKA_MAGIC,
        "Invalid game bundle signature"
    );
    let (version, engine, revision, compressed_info, expanded_info, flags, size) =
        if profile == "BH3PrePre" && signature == b"UnityFS" {
            let mut random = HeaderRandom::new(u32::try_from(input.len())?);
            let expanded_info = reader.u32()? ^ random.word();
            let compressed_info = reader.u32()? ^ random.word();
            let flags = reader.u32()? ^ random.word();
            let size = reader.u64()? ^ random.long();
            reader.u32()?; // Historical header's version field is ignored by the reference.
            reader.string(32768)?;
            reader.string(32768)?;
            (
                6,
                b"5.x.x".as_slice(),
                b"2017.4.18f1".as_slice(),
                compressed_info,
                expanded_info,
                flags,
                size,
            )
        } else {
            let version = if signature == b"ENCR" {
                6
            } else {
                reader.u32()?
            };
            ensure!(
                (6..=8).contains(&version),
                "Game bundle version is outside 6-8"
            );
            ensure!(
                !matches!(signature, b"UnityWeb" | b"UnityRaw") || version == 6,
                "Game legacy signature requires its documented version-6 header"
            );
            let engine = if signature == b"ENCR" {
                b"5.x.x".as_slice()
            } else {
                reader.string(32768)?
            };
            let revision = if signature == b"ENCR" {
                b"2019.4.32f1".as_slice()
            } else {
                reader.string(32768)?
            };
            let size = reader.u64()?;
            let mut compressed = reader.u32()?;
            let mut expanded = reader.u32()?;
            let flags = reader.u32()?;
            if matches!(signature, b"UnityWeb" | b"UnityRaw" | b"ENCR") {
                reader.take(1)?;
            }
            if profile == "Naraka" {
                compressed = compressed
                    .checked_sub(0xca)
                    .context("Naraka compressed table adjustment underflow")?;
                expanded = expanded
                    .checked_sub(0xca)
                    .context("Naraka expanded table adjustment underflow")?;
            }
            (version, engine, revision, compressed, expanded, flags, size)
        };
    ensure!(
        size == input.len() as u64,
        "Game bundle declared size differs from its input"
    );
    if version >= 7 {
        let end = align16(reader.position)?;
        reader.take(end - reader.position)?;
    }
    Ok(Header {
        version,
        engine,
        revision,
        compressed_info: usize::try_from(compressed_info)?,
        expanded_info: usize::try_from(expanded_info)?,
        flags,
        payload_start: reader.position,
        has_hash: signature != b"ENCR",
    })
}

struct HeaderRandom {
    seed: u32,
    x: u32,
    y: u32,
    z: u32,
    w: u32,
}
impl HeaderRandom {
    fn new(seed: u32) -> Self {
        let y = 0x6c078965u32.wrapping_mul(seed).wrapping_add(1);
        let z = 0x6c078965u32.wrapping_mul(y).wrapping_add(1);
        let w = 0x6c078965u32.wrapping_mul(z).wrapping_add(1);
        Self {
            seed,
            x: seed,
            y,
            z,
            w,
        }
    }
    fn next(&mut self) -> u32 {
        let t = self.x ^ (self.x << 11);
        self.x = self.y;
        self.y = self.z;
        self.z = self.w;
        self.w = self.w ^ (self.w >> 19) ^ t ^ (t >> 8);
        self.w
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        // C# promotes uint*int to long, then performs unchecked signed 64-bit arithmetic.
        let key = (N as u64)
            .wrapping_mul(u64::from(self.seed))
            .wrapping_sub(0x61c8864e7a143579);
        let source = key.to_le_bytes();
        let mut output = [0; N];
        for byte in &mut output {
            *byte = source[(self.next() % 8) as usize];
        }
        output
    }
    fn word(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }
    fn long(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes())
    }
}
fn align16(value: usize) -> Result<usize> {
    value
        .checked_add(15)
        .map(|n| n & !15)
        .context("Game bundle alignment overflow")
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
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(size)
            .context("Game bundle read overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .context("Game bundle is truncated")?;
        self.position = end;
        Ok(value)
    }
    fn string(&mut self, maximum: usize) -> Result<&'a [u8]> {
        let n = self.bytes[self.position..]
            .iter()
            .take(maximum)
            .position(|&b| b == 0)
            .context("Game bundle string is unterminated or oversized")?;
        Ok(&self.take(n + 1)?[..n])
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_rs_core::{bundle::UnityFsBundle, source::Region};

    fn standard_fixture(
        payload: &[u8],
        expanded: usize,
        compression: u16,
        at_end: bool,
    ) -> (Vec<u8>, usize) {
        let mut info = vec![0; 16];
        info.extend_from_slice(&1_u32.to_be_bytes());
        info.extend_from_slice(&(expanded as u32).to_be_bytes());
        info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        info.extend_from_slice(&compression.to_be_bytes());
        info.extend_from_slice(&1_u32.to_be_bytes());
        info.extend_from_slice(&0_u64.to_be_bytes());
        info.extend_from_slice(&(expanded as u64).to_be_bytes());
        info.extend_from_slice(&0_u32.to_be_bytes());
        info.extend_from_slice(b"fixture.resS\0");
        let encoded = lz4_flex::block::compress(&info);
        let mut result = b"UnityFS\0".to_vec();
        result.extend_from_slice(&7u32.to_be_bytes());
        result.extend_from_slice(b"5.x.x\0");
        result.extend_from_slice(b"2022.3.0f1\0");
        let size = result.len();
        result.extend_from_slice(&0u64.to_be_bytes());
        result.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        result.extend_from_slice(&(info.len() as u32).to_be_bytes());
        result.extend_from_slice(&(if at_end { 0xc5u32 } else { 0x45u32 }).to_be_bytes());
        result.resize(result.len().next_multiple_of(16), 0);
        if at_end {
            result.extend_from_slice(payload);
            result.extend_from_slice(&encoded);
        } else {
            result.extend_from_slice(&encoded);
            result.extend_from_slice(payload);
        }
        let len = result.len() as u64;
        result[size..size + 8].copy_from_slice(&len.to_be_bytes());
        (result, size)
    }
    fn assert_payload(bundle: Vec<u8>, expected: &[u8]) {
        let parsed = UnityFsBundle::open(&Region::from_bytes(bundle)).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.read_entry(0).unwrap(), expected);
    }

    #[test]
    fn header_random_matches_four_independent_original_csharp_sequences() {
        for (seed, expected) in [
            (0, [0x37873737, 0xebeb87ca, 0xb1b18587, 0xb1857987eb8787b1]),
            (1, [0xebebebeb, 0xeb37ebeb, 0x9e79cab1, 0x8579b19e9eca9e9e]),
            (
                4096,
                [0x0aec379e, 0x37870a79, 0xb19e3787, 0xec85b1794aec8737],
            ),
            (
                u32::MAX,
                [0x83858383, 0x79837937, 0x9e9e3779, 0xb99eeb9e9e9e9eeb],
            ),
        ] {
            let mut rng = HeaderRandom::new(seed);
            assert_eq!(
                [
                    u64::from(rng.word()),
                    u64::from(rng.word()),
                    u64::from(rng.word()),
                    rng.long()
                ],
                expected
            );
        }
    }

    #[test]
    fn naraka_signature_and_biased_sizes_become_valid_front_and_end_tables() {
        let expected = b"Naraka's complete standard payload";
        let compressed = lz4_flex::block::compress(expected);
        for at_end in [false, true] {
            let (mut encoded, size) = standard_fixture(&compressed, expected.len(), 2, at_end);
            encoded[..7].copy_from_slice(NARAKA_MAGIC);
            for position in [size + 8, size + 12] {
                let n =
                    u32::from_be_bytes(encoded[position..position + 4].try_into().unwrap()) + 0xca;
                encoded[position..position + 4].copy_from_slice(&n.to_be_bytes());
            }
            let flags = if at_end { 0xc2u32 } else { 0x42u32 };
            encoded[size + 16..size + 20].copy_from_slice(&flags.to_be_bytes());
            assert_payload(normalize(&encoded, "Naraka", 65536).unwrap(), expected);
            assert_eq!(&encoded[..7], NARAKA_MAGIC);
            encoded[size + 8..size + 12].copy_from_slice(&1u32.to_be_bytes());
            assert!(
                normalize(&encoded, "Naraka", 65536)
                    .unwrap_err()
                    .to_string()
                    .contains("underflow")
            );
        }
    }

    #[test]
    fn mr0k_blocks_from_original_csharp_oracle_decode_through_real_bundle_reader() {
        for (profile, encrypted, expected) in [
            (
                "TOT",
                include_bytes!("../tests/fixtures/mhy-oracle/TOT-literal.mr0k").as_slice(),
                include_bytes!("../tests/fixtures/mhy-oracle/TOT-literal-payload.bin").as_slice(),
            ),
            (
                "BH3PrePre",
                include_bytes!("../tests/fixtures/mhy-oracle/BH3PrePre-literal.mr0k").as_slice(),
                include_bytes!("../tests/fixtures/mhy-oracle/BH3PrePre-literal-payload.bin")
                    .as_slice(),
            ),
        ] {
            for at_end in [false, true] {
                let (bundle, size) = standard_fixture(encrypted, expected.len(), 5, at_end);
                let bundle = if profile == "BH3PrePre" {
                    bh3_header(&bundle, size)
                } else {
                    bundle
                };
                assert_payload(normalize(&bundle, profile, 65536).unwrap(), expected);
            }
        }
    }

    fn bh3_header(standard: &[u8], size: usize) -> Vec<u8> {
        let mut header = b"UnityFS\0".to_vec();
        header.resize(8 + 24, 0);
        header.extend_from_slice(b"encoded-version\0encoded-revision\0");
        let payload = standard[(size + 20).next_multiple_of(16)..].to_vec();
        let length = header.len() + payload.len();
        let mut rng = HeaderRandom::new(length as u32);
        let expanded =
            u32::from_be_bytes(standard[size + 12..size + 16].try_into().unwrap()) ^ rng.word();
        let compressed =
            u32::from_be_bytes(standard[size + 8..size + 12].try_into().unwrap()) ^ rng.word();
        let flags =
            u32::from_be_bytes(standard[size + 16..size + 20].try_into().unwrap()) ^ rng.word();
        let declared = (length as u64) ^ rng.long();
        header[8..12].copy_from_slice(&expanded.to_be_bytes());
        header[12..16].copy_from_slice(&compressed.to_be_bytes());
        header[16..20].copy_from_slice(&flags.to_be_bytes());
        header[20..28].copy_from_slice(&declared.to_be_bytes());
        header[28..32].copy_from_slice(&1234u32.to_be_bytes());
        header.extend_from_slice(&payload);
        header
    }

    #[test]
    fn concatenated_tot_bundles_keep_both_roots_with_original_byte_offsets() {
        let payload = lz4_flex::block::compress(b"first and second complete resources");
        let (one, _) = standard_fixture(&payload, 35, 3, false);
        let mut many = one.clone();
        many.extend_from_slice(&one);
        let normalized = normalize(&many, "TOT", 65536).unwrap();
        let web = unity_rs_core::web_file::WebFile::open(Region::from_bytes(normalized)).unwrap();
        assert_eq!(web.entries.len(), 2);
        assert_eq!(web.entries[0].path, "bundle_0000000000000000.bundle");
        assert_eq!(
            web.entries[1].path,
            format!("bundle_{:016x}.bundle", one.len())
        );
        for index in 0..2 {
            assert_payload(
                web.read_entry(index).unwrap(),
                b"first and second complete resources",
            );
        }
        many.push(0);
        assert!(normalize(&many, "TOT", 65536).is_err());
        assert!(normalize(&one, "TOT", one.len() as u64 - 1).is_err());
    }

    #[test]
    fn truncations_and_malformed_header_bytes_return_errors_without_panics() {
        let payload = lz4_flex::block::compress(b"ordinary resource");
        let (good, _) = standard_fixture(&payload, 17, 3, false);
        for length in 0..good.len() {
            for profile in ["TOT", "BH3PrePre", "Naraka"] {
                let outcome =
                    std::panic::catch_unwind(|| normalize(&good[..length], profile, 65536));
                assert!(outcome.is_ok(), "{profile} truncation{length}");
                assert!(outcome.unwrap().is_err());
            }
        }
        for position in 0..good.len() {
            let mut bad = good.clone();
            bad[position] ^= 255;
            assert!(std::panic::catch_unwind(|| normalize(&bad, "TOT", 65536)).is_ok());
        }
    }

    #[test]
    fn exastris_lz4lit_matches_original_csharp_with_overlap_and_extended_lengths() {
        let source = include_bytes!("../tests/fixtures/mhy-oracle/LZ4Lit-reference.lit");
        let expected = include_bytes!("../tests/fixtures/mhy-oracle/LZ4Lit-reference.bin");
        let normal = normalize_lz4_lit(source, expected.len()).unwrap();
        assert_eq!(
            lz4_flex::block::decompress(&normal, expected.len()).unwrap(),
            expected
        );
        for block_code in [4, 5] {
            for at_end in [false, true] {
                let (source, _) = standard_fixture(source, expected.len(), block_code, at_end);
                assert_payload(normalize(&source, "ExAstris", 65536).unwrap(), expected);
            }
        }
        let expected: Vec<u8> = (0..300).map(|n| n as u8).collect();
        let mut source = vec![0x0f, 255, 30];
        source.extend_from_slice(&expected);
        let normal = normalize_lz4_lit(&source, 300).unwrap();
        assert_eq!(lz4_flex::block::decompress(&normal, 300).unwrap(), expected);
        for bytes in [
            b"".as_slice(),
            &[0x0f],
            &[0x0f, 255],
            &[0x10, 0, 0],
            &[0x10, 1, 0],
            &[1],
        ] {
            assert!(normalize_lz4_lit(bytes, 100).is_err());
        }
        assert!(normalize_lz4_lit(source.as_slice(), 299).is_err());
    }

    #[test]
    fn naraka_zstd_blocks_keep_their_original_compression_semantics() {
        let plain = b"standard Zstandard frame inside Naraka";
        // A Zstandard single-segment frame containing one final raw block.
        // No encoder or custom decompressor is shared with the backend reader.
        let mut zstd = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, plain.len() as u8];
        let header = ((plain.len() as u32) << 3) | 1;
        zstd.extend_from_slice(&header.to_le_bytes()[..3]);
        zstd.extend_from_slice(plain);
        let (mut source, size) = standard_fixture(&zstd, plain.len(), 5, false);
        source[..7].copy_from_slice(NARAKA_MAGIC);
        for offset in [size + 8, size + 12] {
            let n = u32::from_be_bytes(source[offset..offset + 4].try_into().unwrap()) + 0xca;
            source[offset..offset + 4].copy_from_slice(&n.to_be_bytes());
        }
        source[size + 16..size + 20].copy_from_slice(&0x42u32.to_be_bytes());
        assert_payload(normalize(&source, "Naraka", 65536).unwrap(), plain);
    }

    fn alternate_header(signature: &[u8], payload: &[u8], at_end: bool, naraka: bool) -> Vec<u8> {
        let mut info = if signature == b"ENCR" {
            Vec::new()
        } else {
            vec![0; 16]
        };
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        info.extend_from_slice(&0u16.to_be_bytes());
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&0u64.to_be_bytes());
        info.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        info.extend_from_slice(&0u32.to_be_bytes());
        info.extend_from_slice(b"fixture.resS\0");
        let mut data = signature.to_vec();
        data.push(0);
        if signature != b"ENCR" {
            data.extend_from_slice(&6u32.to_be_bytes());
            data.extend_from_slice(b"5.x.x\0");
            data.extend_from_slice(b"2019.4.32f1\0");
        }
        let position = data.len();
        data.extend_from_slice(&0u64.to_be_bytes());
        let size = info.len() as u32 + if naraka { 0xca } else { 0 };
        data.extend_from_slice(&size.to_be_bytes());
        data.extend_from_slice(&size.to_be_bytes());
        data.extend_from_slice(&(if at_end { 0xc0u32 } else { 0x40u32 }).to_be_bytes());
        data.push(123); // Required old-signature compatibility byte, not padding.
        if at_end {
            data.extend_from_slice(payload);
            data.extend_from_slice(&info);
        } else {
            data.extend_from_slice(&info);
            data.extend_from_slice(payload);
        }
        let length = data.len() as u64;
        data[position..position + 8].copy_from_slice(&length.to_be_bytes());
        data
    }

    #[test]
    fn unityweb_raw_v6_and_encr_without_hash_are_normalized_for_every_new_profile() {
        for profile in ["TOT", "BH3PrePre", "Naraka", "ExAstris"] {
            for signature in [
                b"UnityWeb".as_slice(),
                b"UnityRaw".as_slice(),
                b"ENCR".as_slice(),
            ] {
                for at_end in [false, true] {
                    let encoded = alternate_header(
                        signature,
                        b"legacy complete payload",
                        at_end,
                        profile == "Naraka",
                    );
                    assert_payload(
                        normalize(&encoded, profile, 65536).unwrap(),
                        b"legacy complete payload",
                    );
                }
            }
        }
        let one = alternate_header(b"ENCR", b"first", false, false);
        let two = alternate_header(b"UnityRaw", b"second", true, false);
        let mut source = one;
        source.extend_from_slice(&two);
        let web = unity_rs_core::web_file::WebFile::open(Region::from_bytes(
            normalize(&source, "TOT", 65536).unwrap(),
        ))
        .unwrap();
        assert_eq!(web.entries.len(), 2);
        assert_payload(web.read_entry(0).unwrap(), b"first");
        assert_payload(web.read_entry(1).unwrap(), b"second");
    }
}
