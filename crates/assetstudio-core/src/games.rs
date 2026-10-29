//! Game wrapper compatibility transcribed from this fork's managed reader.
//!
//! A wrapper transform is not evidence that every asset layout in that game is
//! supported. See `docs/game-compatibility.md` for the profile-by-profile gaps.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::source::Region;

#[path = "game_constants.rs"]
mod constants;
#[path = "game_crypto.rs"]
mod crypto;

use constants::*;

const UNITY_FS: &[u8] = b"UnityFS\0";
const ORIGINAL_HEADER: &[u8; 16] = b"UnityFS\0\0\0\0\x075.x.";
const MAX_FILES: usize = 100_000;
const MAX_DIRECTORIES: usize = 16_384;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DIRECTORY_DEPTH: usize = 64;
const MAX_GAME_BUFFER_BYTES: u64 = 512 * 1024 * 1024;

const SUPPORTED: &[&str] = &[
    "Normal",
    "UnityCN",
    "EnsembleStars",
    "FakeHeader",
    "ShiningNikki",
    "FantasyOfWind",
    "HelixWaltz2",
    "AnchorPanic",
    "DreamscapeAlbireo",
    "ImaginaryFest",
    "AliceGearAegis",
    "ProjectSekai",
    "CodenameJump",
    "GirlsFrontline",
    "Reverse1999",
    "JJKPhantomParade",
    "MuvLuvDimensions",
    "PartyAnimals",
    "SchoolGirlStrikers",
];

const BASELINE: &[&str] = &[
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

/// Profiles with implemented loader or wrapper paths. Real game compatibility
/// still requires a representative sample, as documented in the matrix.
pub fn supported_profiles() -> &'static [&'static str] {
    SUPPORTED
}

/// The full legacy profile inventory, including explicitly unported formats.
pub fn all_profiles() -> &'static [&'static str] {
    BASELINE
}

pub fn validate_profile(profile: &str) -> Result<()> {
    canonical_profile(profile).map(|_| ())
}

fn canonical_profile(profile: &str) -> Result<&'static str> {
    if let Some(canonical) = SUPPORTED
        .iter()
        .find(|name| name.eq_ignore_ascii_case(profile))
    {
        return Ok(canonical);
    }
    if BASELINE
        .iter()
        .any(|name| name.eq_ignore_ascii_case(profile))
    {
        bail!(
            "Game profile {profile:?} is not implemented in the Rust rewrite: it needs fork-specific block or object layouts; see docs/game-compatibility.md. It will not be read as Normal."
        );
    }
    bail!(
        "Unknown game profile {profile:?}. Implemented profiles: {}",
        SUPPORTED.join(", ")
    );
}

/// Non-default wrappers have synthetic transform tests, not a supplied corpus
/// proving that a current release of the named game has the same layout.
pub fn profile_warning(profile: &str) -> Option<&'static str> {
    let profile = canonical_profile(profile).ok()?;
    if matches!(profile, "Normal" | "UnityCN") {
        None
    } else {
        Some(
            "Game wrapper decoding is implemented; real-game compatibility has not been verified. Check the game compatibility matrix before relying on decoded assets.",
        )
    }
}

/// Returns `None` for the standard loader, or bounded decoded regions for game
/// wrappers. Companion resources stay file-backed and retain their real names.
/// This operation never modifies the original input files.
pub fn prepare_regions(
    input: &Path,
    profile: &str,
    max_bytes: u64,
) -> Result<Option<Vec<(String, Region)>>> {
    let profile = canonical_profile(profile)?;
    if matches!(profile, "Normal" | "UnityCN") {
        return Ok(None);
    }
    let maximum = max_bytes.min(MAX_GAME_BUFFER_BYTES);
    ensure!(maximum > 0, "Game input byte budget must be positive");
    let files = collect_files(input)?;
    let mut regions = Vec::new();
    regions.try_reserve(files.len())?;
    let mut retained_bytes = 0_u64;
    for file in files {
        let path = file.to_string_lossy().into_owned();
        ensure!(path.len() <= MAX_PATH_BYTES, "Game input path is too long");
        let source = Region::from_file(&file)?;
        if is_companion_resource(&file) {
            regions.push((path, source));
            continue;
        }
        let mut prefix = [0_u8; 4096];
        let prefix_length = usize::try_from(source.len().min(prefix.len() as u64))?;
        source.read_exact_at(0, &mut prefix[..prefix_length])?;
        if !matches!(
            profile,
            "FakeHeader" | "ShiningNikki" | "AnchorPanic" | "PartyAnimals"
        ) && looks_like_standard_input(&prefix[..prefix_length], source.len())
        {
            regions.push((path, source));
            continue;
        }
        let remaining = maximum
            .checked_sub(retained_bytes)
            .context("Game input materialization accounting overflow")?;
        ensure!(
            source.len() <= remaining,
            "Game wrapper input exceeds the {maximum} byte cumulative materialization budget"
        );
        let data = source.read_to_vec(remaining)?;
        let decoded = decode_container_at(&data, profile, &file, remaining)
            .with_context(|| format!("Could not decode game wrapper {file:?}"))?;
        retained_bytes = retained_bytes
            .checked_add(decoded.len() as u64)
            .context("Game wrapper materialization accounting overflow")?;
        ensure!(
            retained_bytes <= maximum,
            "Decoded game wrappers exceed byte budget"
        );
        regions.push((path, Region::from_bytes(decoded)));
    }
    Ok(Some(regions))
}

/// Decodes a wrapper whose key does not depend on its filename. For
/// filename-keyed formats use [`decode_container_at`].
pub fn decode_container(data: &[u8], profile: &str, max_bytes: u64) -> Result<Vec<u8>> {
    decode_container_at(data, profile, Path::new(""), max_bytes)
}

/// Decodes one game wrapper under an explicit input/output byte budget.
/// Unrecognized encrypted input produces an error rather than plausible bytes.
pub fn decode_container_at(
    data: &[u8],
    profile: &str,
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let profile = canonical_profile(profile)?;
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= MAX_PATH_BYTES,
        "Game wrapper path exceeds {MAX_PATH_BYTES} bytes"
    );
    let maximum = max_bytes.min(MAX_GAME_BUFFER_BYTES);
    ensure!(
        data.len() as u64 <= maximum,
        "Game wrapper exceeds {maximum} byte budget"
    );
    if matches!(profile, "Normal" | "UnityCN") {
        return copy_bytes(data);
    }
    if !matches!(
        profile,
        "FakeHeader" | "ShiningNikki" | "AnchorPanic" | "PartyAnimals"
    ) && looks_like_standard_input(data, data.len() as u64)
    {
        return copy_bytes(data);
    }
    let output = match profile {
        "FakeHeader" | "ShiningNikki" => strip_fake_header(data)?,
        "EnsembleStars" => decode_ensemble(data)?,
        "FantasyOfWind" => decode_fantasy(data)?,
        "HelixWaltz2" => decode_helix(data)?,
        "AnchorPanic" => decode_anchor(data, path)?,
        "DreamscapeAlbireo" => decode_dreamscape(data, maximum)?,
        "ImaginaryFest" => decode_imaginary(data, path)?,
        "AliceGearAegis" => decode_alice(data)?,
        "ProjectSekai" => decode_sekai(data)?,
        "CodenameJump" => xor_copy(data, &CODENAME_KEY)?,
        "GirlsFrontline" => decode_girls(data)?,
        "Reverse1999" => decode_reverse(data, path)?,
        "JJKPhantomParade" => decode_jjk(data)?,
        "MuvLuvDimensions" => xor_copy(
            data,
            &[0xfd, 0x13, 0x7b, 0xee, 0xc5, 0xfe, 0x50, 0x12, 0x4d, 0x38],
        )?,
        "PartyAnimals" => decode_party(data, path)?,
        "SchoolGirlStrikers" => decode_school(data)?,
        _ => bail!("No wrapper decoder exists for profile {profile}"),
    };
    ensure!(
        output.len() as u64 <= maximum,
        "Decoded game wrapper exceeds {maximum} byte budget"
    );
    ensure!(
        looks_like_standard_input(&output, output.len() as u64),
        "Decoded {profile} input does not contain a recognized Unity/container header"
    );
    Ok(output)
}

fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    output.try_reserve_exact(data.len())?;
    output.extend_from_slice(data);
    Ok(output)
}

fn xor_copy(data: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    ensure!(!key.is_empty(), "Game XOR key is empty");
    let mut output = copy_bytes(data)?;
    for (index, byte) in output.iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
    Ok(output)
}

fn looks_like_standard_input(data: &[u8], full_length: u64) -> bool {
    if [
        UNITY_FS,
        b"UnityWeb\0",
        b"UnityRaw\0",
        b"UnityWebData1.0\0",
        b"TuanjieWebData1.0\0",
        b"\x1f\x8b",
        b"PK\x03\x04",
        b"PK\x07\x08",
    ]
    .iter()
    .any(|signature| data.starts_with(signature))
    {
        return true;
    }
    if data.len() >= 38 && &data[32..38] == b"brotli" {
        return true;
    }
    if data.len() < 20 {
        return false;
    }
    let version = u32::from_be_bytes(data[8..12].try_into().unwrap_or([0; 4]));
    if !(5..=22).contains(&version) {
        return false;
    }
    let (size, offset) = if version == 22 {
        if data.len() < 48 {
            return false;
        }
        (
            u64::from_be_bytes(data[24..32].try_into().unwrap_or([0; 8])),
            u64::from_be_bytes(data[32..40].try_into().unwrap_or([0; 8])),
        )
    } else {
        (
            u64::from(u32::from_be_bytes(data[4..8].try_into().unwrap_or([0; 4]))),
            u64::from(u32::from_be_bytes(
                data[12..16].try_into().unwrap_or([0; 4]),
            )),
        )
    };
    size == full_length && offset <= size && data[16] <= 1
}

fn strip_fake_header(data: &[u8]) -> Result<Vec<u8>> {
    let scan = &data[..data.len().min(4096)];
    let mut positions = scan
        .windows(UNITY_FS.len())
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == UNITY_FS).then_some(index));
    let first = positions
        .next()
        .context("No UnityFS header found in first 4096 bytes")?;
    // Match this fork: the second signature supersedes the deliberately false
    // first signature. The standard parser then validates its actual fields.
    let offset = positions.next().unwrap_or(first);
    copy_bytes(&data[offset..])
}

fn decode_ensemble(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = copy_bytes(data)?;
    let stride = data.len() % 3 + 1;
    let remaining = (data.len() % 7) as i32;
    let size = remaining + !((data.len() % 3) as i32) + ENSEMBLE_STAR_KEY2.len() as i32;
    for (index, byte) in output.iter_mut().enumerate().step_by(stride) {
        let offset = index / stride;
        let value = (size
            ^ i32::from(ENSEMBLE_STAR_KEY3[offset % ENSEMBLE_STAR_KEY3.len()])
            ^ i32::from(*byte)
            ^ i32::from(ENSEMBLE_STAR_KEY2[offset % ENSEMBLE_STAR_KEY2.len()]))
            + remaining;
        *byte = ENSEMBLE_STAR_KEY1[offset % ENSEMBLE_STAR_KEY1.len()] ^ value as u8;
    }
    Ok(output)
}

fn decode_fantasy(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() > 200, "FantasyOfWind wrapper is truncated");
    ensure!(
        data[0] == b'K' && data[1..5].iter().all(u8::is_ascii_digit),
        "Invalid FantasyOfWind version signature"
    );
    let key_length = usize::from(data[data.len() - 21]);
    let encrypted_length = usize::from(data[data.len() - 22]) % 50 + 50;
    let key_position = data.len() - 20 - (key_length % 8 + 8);
    ensure!(
        5 + encrypted_length <= data.len(),
        "FantasyOfWind encrypted prefix is truncated"
    );
    let mut key = [0_u8; 13];
    key[..5].copy_from_slice(b"28856");
    key[5..].copy_from_slice(&data[key_position..key_position + 8]);
    for (index, byte) in key[5..].iter_mut().enumerate() {
        if *byte == 0 {
            *byte = (key_position + index) as u8;
        }
    }
    let mut output = copy_bytes(data)?;
    output[..5].copy_from_slice(b"Unity");
    for (index, byte) in output[5..5 + encrypted_length].iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
    Ok(output)
}

fn decode_helix(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        data.len() >= 16 && data.starts_with(b"SzxFS\0"),
        "Invalid or truncated HelixWaltz2 wrapper"
    );
    let seed = i32::from_be_bytes(data[8..12].try_into()?);
    let mut random = crypto::DotNetRandom::new(seed);
    let mut box_values = [0_u8; 256];
    for (index, byte) in box_values.iter_mut().enumerate() {
        *byte = index as u8;
    }
    let mut key = [0_u8; 256];
    for index in 0..256 {
        let target = random.range(index, 256);
        box_values.swap(index, target);
        key[usize::from(box_values[index])] = index as u8;
    }
    let mut output = copy_bytes(data)?;
    output[..16].copy_from_slice(ORIGINAL_HEADER);
    for byte in &mut output[16..] {
        *byte = key[usize::from(*byte)];
    }
    Ok(output)
}

fn filename_stem(path: &Path) -> Result<&str> {
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("This game wrapper requires its original UTF-8 filename")?;
    ensure!(!name.is_empty(), "Game wrapper filename cannot be empty");
    Ok(name)
}

fn decode_anchor(data: &[u8], path: &Path) -> Result<Vec<u8>> {
    if data[..data.len().min(4096)]
        .windows(UNITY_FS.len())
        .any(|bytes| bytes == UNITY_FS)
    {
        return strip_fake_header(data);
    }
    let name_hash = crypto::md5_hex(filename_stem(path)?);
    let salt_hash = crypto::md5_hex("KxZKZolAT3QXvsUU");
    let key = format!(
        "{}leiyan{}",
        &salt_hash[..5],
        &name_hash[name_hash.len() - 5..]
    );
    let mut output = copy_bytes(data)?;
    for (index, block) in output.chunks_mut(0x800).enumerate() {
        let encrypted = index == 0
            || data.len() / 0x800 == index
            || (index <= 4 && data.len() % 2 == index % 2);
        if encrypted {
            crypto::rc4(block, key.as_bytes())?;
        }
    }
    Ok(output)
}

fn decode_dreamscape(data: &[u8], maximum: u64) -> Result<Vec<u8>> {
    ensure!(
        data.len() >= 28 && data.starts_with(b"MJJ\0"),
        "Invalid or truncated DreamscapeAlbireo wrapper"
    );
    let mut values = [0_u32; 6];
    for (slot, bytes) in values.iter_mut().zip(data[4..28].as_chunks::<4>().0) {
        *slot = u32::from_be_bytes(*bytes);
    }
    let [u1, u2, _u3, u4, u5, u6] = values;
    let scramble = |value: u32| {
        (value >> 5) & 0x00ff_e000
            | (value >> 29)
            | (value << 14) & 0xff00_0000
            | (value << 3) & 0x1ff8
    };
    let flags = scramble(u4) ^ 0x7002_0017;
    let compressed_size = scramble(u1) ^ u4;
    let uncompressed_size = scramble(u6) ^ u1;
    let size_high = (u5 & 0xffff_ff00 | u2 >> 24) ^ u4;
    let size_low = (u5 >> 24 | u2 << 8) ^ u1;
    // Promote before shifting: the C# expression shifted a uint by 32 and
    // accidentally wrapped the shift count to zero.
    let size = (u64::from(size_high) << 32) | u64::from(size_low);
    ensure!(
        size <= maximum,
        "DreamscapeAlbireo decoded size exceeds byte budget"
    );
    let compressed_size_usize = usize::try_from(compressed_size)?;
    ensure!(
        compressed_size_usize <= data.len() - 28,
        "DreamscapeAlbireo block table is truncated"
    );
    let output_size = data
        .len()
        .checked_sub(28)
        .and_then(|n| n.checked_add(49))
        .context("DreamscapeAlbireo output size overflow")?;
    ensure!(
        output_size as u64 <= maximum,
        "DreamscapeAlbireo output exceeds byte budget"
    );
    ensure!(
        size == output_size as u64,
        "DreamscapeAlbireo declared bundle size does not match reconstructed bytes"
    );
    let mut output = Vec::new();
    output.try_reserve_exact(output_size)?;
    output.extend_from_slice(UNITY_FS);
    output.extend_from_slice(&6_u32.to_be_bytes());
    output.extend_from_slice(b"5.x.x\0");
    output.extend_from_slice(b"2018.4.2f1\0");
    output.extend_from_slice(&size.to_be_bytes());
    output.extend_from_slice(&compressed_size.to_be_bytes());
    output.extend_from_slice(&uncompressed_size.to_be_bytes());
    output.extend_from_slice(&flags.to_be_bytes());
    let key = [0x1e, 0x1e, 0x01, 0x01, 0xfc];
    for (index, byte) in data[28..28 + compressed_size_usize].iter().enumerate() {
        output.push(*byte ^ key[index % key.len()]);
    }
    output.extend_from_slice(&data[28 + compressed_size_usize..]);
    Ok(output)
}

fn decode_imaginary(data: &[u8], path: &Path) -> Result<Vec<u8>> {
    ensure!(data.len() >= 8, "ImaginaryFest wrapper is truncated");
    if data[7] != 0 {
        let key = data[7];
        let mut header = [0_u8; 8];
        for (target, byte) in header.iter_mut().zip(&data[..8]) {
            *target = *byte ^ key;
        }
        if header == UNITY_FS {
            return xor_copy(data, &[key]);
        }
    }
    let components: Vec<_> = path.components().collect();
    let start = components.iter().position(|value| value.as_os_str() == "data")
        .context("ImaginaryFest path-keyed encryption requires its original path beneath a data directory")?;
    ensure!(
        start + 1 < components.len(),
        "ImaginaryFest relative path is missing"
    );
    let relative = components[start + 1..]
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let key_value = imaginary_hash(&relative)?;
    let mut offset = 1_i32;
    let iterations = (key_value & 3) + 1;
    for index in 0..iterations {
        let value = key_value >> (4 + 4 * index);
        offset = offset.wrapping_add((value % 0x80) | 0x80);
    }
    if let Ok(offset) = usize::try_from(offset)
        && data
            .get(offset..)
            .is_some_and(|tail| tail.starts_with(UNITY_FS))
    {
        return copy_bytes(&data[offset..]);
    }
    let mut key_value = key_value;
    let step = (key_value >> 8) % 3 + 1;
    let mut output = copy_bytes(data)?;
    for byte in &mut output {
        *byte ^= IMAGINARY_KEY[usize::from(key_value as u8)];
        key_value = key_value.wrapping_add(step);
    }
    Ok(output)
}

fn imaginary_hash(value: &str) -> Result<i32> {
    let chars: Vec<u16> = value.encode_utf16().collect();
    ensure!(!chars.is_empty(), "ImaginaryFest relative path is empty");
    let mut table = [0_i32; 4];
    for (index, slot) in table.iter_mut().enumerate() {
        let position = chars.len().saturating_sub(index + 1);
        let key_offset = usize::from(chars[position])
            .checked_add(index * 4)
            .context("ImaginaryFest key offset overflow")?;
        let bytes = IMAGINARY_KEY
            .get(key_offset..key_offset.saturating_add(4))
            .context("ImaginaryFest filename cannot be represented by this legacy key table")?;
        *slot = i32::from_le_bytes(bytes.try_into()?);
    }
    let mut shift = 0_i32;
    for index in (0..chars.len()).rev() {
        let character = i32::from(chars[index]);
        shift = shift.wrapping_add(index as i32) ^ character;
        table[index % 4] = table[index % 4].wrapping_add(character.wrapping_shl(shift as u32));
    }
    Ok(table[0] ^ table[1] ^ table[2] ^ table[3])
}

fn decode_alice(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 8, "AliceGearAegis wrapper is truncated");
    let key = [
        0x1b, 0x59, 0x62, 0x33, 0x78, 0x76, 0x45, 0xb3, 0x5b, 0x48, 0x39, 0xd7, 0x9c, 0x21, 0x89,
        0x94,
    ];
    let mut output = copy_bytes(data)?;
    output[..8].copy_from_slice(UNITY_FS);
    let seed = (data.len() - 8) % key.len();
    let end = data.len().min(8 + 0x80);
    for (index, byte) in output[8..end].iter_mut().enumerate() {
        *byte ^= key[(seed + index) % key.len()];
    }
    Ok(output)
}

fn decode_sekai(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 4, "ProjectSekai wrapper is truncated");
    let version = u32::from_le_bytes(data[..4].try_into()?);
    ensure!(
        version == 0x10 || version == 0x20,
        "Unknown ProjectSekai wrapper version {version:#x}"
    );
    let mut output = copy_bytes(&data[4..])?;
    if version == 0x10 {
        ensure!(
            output.len() >= 0x80,
            "ProjectSekai v0x10 encrypted header is truncated"
        );
        let key = [0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0];
        for (index, byte) in output[..0x80].iter_mut().enumerate() {
            *byte ^= key[index % 8];
        }
    }
    Ok(output)
}

fn decode_girls(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 16, "GirlsFrontline wrapper is truncated");
    let mut key = [0_u8; 16];
    for index in 0..16 {
        let value = data[index] ^ ORIGINAL_HEADER[index];
        key[index] = if value != ORIGINAL_HEADER[index] {
            value
        } else {
            ORIGINAL_HEADER[index]
        };
    }
    let mut output = copy_bytes(data)?;
    let end = data.len().min(0x8000);
    for (index, byte) in output[..end].iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
    Ok(output)
}

fn decode_reverse(data: &[u8], path: &Path) -> Result<Vec<u8>> {
    let mut key = filename_stem(path)?
        .encode_utf16()
        .fold(0_u8, |total, character| total.wrapping_add(character as u8));
    key = key.wrapping_add(2 * ((key & 1) + 1));
    xor_copy(data, &[key])
}

fn decode_jjk(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 16, "JJKPhantomParade wrapper is truncated");
    let key = [data[0], data[1]];
    let mut signature = [0_u8; 13];
    for (index, byte) in signature.iter_mut().enumerate() {
        *byte = data[2 + index] ^ key[index % 2];
    }
    ensure!(
        &signature == b"_GhostAssets_",
        "Invalid JJKPhantomParade signature"
    );
    ensure!(
        data[15] ^ key[0] ^ key[1] == 1,
        "Unknown JJKPhantomParade generation"
    );
    let mut output = copy_bytes(&data[16..])?;
    crypto::jjk_counter_xor(&mut output)?;
    Ok(output)
}

fn decode_party(data: &[u8], path: &Path) -> Result<Vec<u8>> {
    let name = filename_stem(path)?.as_bytes();
    let key = name.iter().fold(0x7c_u8, |value, byte| value ^ *byte);
    let sum = name
        .iter()
        .fold(0_u8, |value, byte| value.wrapping_add(*byte));
    let start = usize::from(PARTY_TABLE[usize::from(sum) % PARTY_TABLE.len()]);
    let mut output = copy_bytes(data)?;
    for (index, byte) in output.iter_mut().enumerate().skip(start) {
        *byte ^= key ^ ((index / 8 + 1) as u8);
    }
    Ok(output)
}

fn decode_school(data: &[u8]) -> Result<Vec<u8>> {
    let stride = data.len() % 7 + 3;
    let mut output = copy_bytes(data)?;
    let mut key = 0xff;
    for (index, byte) in output.iter_mut().enumerate().skip(1) {
        if index % stride != 0 {
            *byte ^= key;
        } else {
            key = !key;
        }
    }
    Ok(output)
}

/// Decodes the old GI_CB1 `mark` envelope. This standalone primitive is tested,
/// but GI_CB1 is deliberately not advertised as a working profile: its custom
/// serialized and renderer layouts have not yet been ported.
pub fn decode_mark(data: &[u8], maximum: u64) -> Result<Vec<u8>> {
    ensure!(
        data.len() as u64 <= maximum.min(MAX_GAME_BUFFER_BYTES),
        "Mark input exceeds byte budget"
    );
    ensure!(data.starts_with(b"mark"), "Invalid mark wrapper signature");
    const BLOCK: usize = 0xa00;
    const CHUNK: usize = 0x264;
    const PADDING: usize = 4;
    const PADDED_BLOCK: usize = BLOCK + (BLOCK / CHUNK + 1) * PADDING;
    let mut output = Vec::new();
    output.try_reserve_exact(data.len() - 4)?;
    let mut key_index = 0_usize;
    for block in data[4..].chunks(PADDED_BLOCK) {
        for chunk in block.chunks(CHUNK + PADDING) {
            let length = if block.len() == PADDED_BLOCK || chunk.len() == CHUNK + PADDING {
                chunk
                    .len()
                    .checked_sub(PADDING)
                    .context("Mark chunk padding is truncated")?
            } else {
                chunk.len()
            };
            for byte in &chunk[..length] {
                output.push(*byte ^ MARK_KEY[key_index % MARK_KEY.len()]);
                key_index += 1;
            }
        }
    }
    Ok(output)
}

fn is_companion_resource(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            [
                "ress", "resource", "json", "xml", "txt", "png", "jpg", "jpeg", "bmp", "tga",
                "ico", "wav", "ogg", "mp3", "dll", "so", "pdb", "exe", "ini", "config", "manifest",
                "hash",
            ]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn collect_files(input: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![(input.to_owned(), 0_usize)];
    let mut directories = 0_usize;
    let mut entries = 0_usize;
    while let Some((path, depth)) = pending.pop() {
        ensure!(
            depth <= MAX_DIRECTORY_DEPTH,
            "Game input directory depth exceeds {MAX_DIRECTORY_DEPTH}"
        );
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("Cannot inspect input {path:?}"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Game input symlinks are not followed: {path:?}"
        );
        if metadata.is_file() {
            ensure!(
                files.len() < MAX_FILES,
                "Game input contains too many files"
            );
            files.try_reserve(1)?;
            files.push(path);
        } else if metadata.is_dir() {
            directories += 1;
            ensure!(
                directories <= MAX_DIRECTORIES,
                "Game input contains too many directories"
            );
            for entry in fs::read_dir(&path)? {
                entries += 1;
                ensure!(
                    entries <= MAX_FILES + MAX_DIRECTORIES,
                    "Game input contains too many directory entries"
                );
                pending.try_reserve(1)?;
                pending.push((entry?.path(), depth + 1));
            }
        } else {
            bail!("Game input must be a regular file or directory: {path:?}");
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Vec<u8> {
        let mut bytes = ORIGINAL_HEADER.to_vec();
        bytes.resize(256, 0x73);
        bytes
    }

    #[test]
    fn profiles_never_silently_enable_unported_layouts() {
        assert!(validate_profile("normal").is_ok());
        assert!(
            validate_profile("GI")
                .unwrap_err()
                .to_string()
                .contains("not implemented")
        );
        assert!(
            validate_profile("unknown")
                .unwrap_err()
                .to_string()
                .contains("Unknown")
        );
        assert_eq!(all_profiles().len(), 39);
    }

    #[test]
    fn project_sekai_versions_have_exact_header_and_payload() {
        let plain = plain();
        let mut encrypted = 0x10_u32.to_le_bytes().to_vec();
        let mut payload = plain.clone();
        for (index, byte) in payload[..128].iter_mut().enumerate() {
            *byte ^= [0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0][index % 8];
        }
        encrypted.extend_from_slice(&payload);
        assert_eq!(
            decode_container(&encrypted, "ProjectSekai", 1024).unwrap(),
            plain
        );
        let mut version20 = 0x20_u32.to_le_bytes().to_vec();
        version20.extend_from_slice(&plain);
        assert_eq!(
            decode_container(&version20, "ProjectSekai", 1024).unwrap(),
            plain
        );
    }

    #[test]
    fn codename_and_muv_wrappers_preserve_exact_payloads() {
        let plain = plain();
        let codename = xor_copy(&plain, &CODENAME_KEY).unwrap();
        assert_eq!(
            decode_container(&codename, "CodenameJump", 1024).unwrap(),
            plain
        );
        let muv = xor_copy(
            &plain,
            &[0xfd, 0x13, 0x7b, 0xee, 0xc5, 0xfe, 0x50, 0x12, 0x4d, 0x38],
        )
        .unwrap();
        assert_eq!(
            decode_container(&muv, "MuvLuvDimensions", 1024).unwrap(),
            plain
        );
    }

    #[test]
    fn school_and_imaginary_simple_xor_wrappers() {
        let plain = plain();
        let encrypted = decode_school(&plain).unwrap();
        assert_eq!(
            decode_container(&encrypted, "SchoolGirlStrikers", 1024).unwrap(),
            plain
        );
        let encrypted = xor_copy(&plain, &[0x41]).unwrap();
        assert_eq!(
            decode_container(&encrypted, "ImaginaryFest", 1024).unwrap(),
            plain
        );
    }

    #[test]
    fn filename_keyed_reverse_uses_original_stem() {
        let plain = plain();
        let encrypted = decode_reverse(&plain, Path::new("abcd.bundle")).unwrap();
        assert_eq!(
            decode_container_at(&encrypted, "Reverse1999", Path::new("abcd.bundle"), 1024).unwrap(),
            plain
        );
        assert!(decode_container(&encrypted, "Reverse1999", 1024).is_err());
    }

    #[test]
    fn fake_header_removes_second_signature_prefix() {
        let plain = plain();
        let mut data = b"prefix-UnityFS\0-invalid-header-and-padding".to_vec();
        data.extend_from_slice(&plain);
        assert_eq!(decode_container(&data, "FakeHeader", 1024).unwrap(), plain);
        assert!(decode_container(b"no signature", "FakeHeader", 1024).is_err());
        let mut data = b"UnityFS\0-invalid-first-header".to_vec();
        data.extend_from_slice(&plain);
        assert_eq!(decode_container(&data, "FakeHeader", 1024).unwrap(), plain);
    }

    #[test]
    fn ensemble_wrapper_reverses_stride_transform() {
        let expected = plain();
        let mut encrypted = expected.clone();
        let stride = encrypted.len() % 3 + 1;
        let remaining = (encrypted.len() % 7) as u8;
        let size = (i32::from(remaining) + !((encrypted.len() % 3) as i32) + 32) as u8;
        for (index, byte) in encrypted.iter_mut().enumerate().step_by(stride) {
            let offset = index / stride;
            *byte = (*byte ^ ENSEMBLE_STAR_KEY1[offset % 29]).wrapping_sub(remaining)
                ^ size
                ^ ENSEMBLE_STAR_KEY3[offset % 16]
                ^ ENSEMBLE_STAR_KEY2[offset % 32];
        }
        assert_eq!(
            decode_container(&encrypted, "EnsembleStars", 1024).unwrap(),
            expected
        );
    }

    #[test]
    fn fantasy_wrapper_uses_footer_key_and_repairs_signature() {
        let mut expected = plain();
        let end = expected.len();
        expected[end - 21] = 3;
        expected[end - 22] = 7;
        let key_position = end - 20 - 11;
        expected[key_position..key_position + 8].copy_from_slice(b"abcdefgh");
        let key = b"28856abcdefgh";
        let mut encrypted = expected.clone();
        encrypted[..5].copy_from_slice(b"K0001");
        for (index, byte) in encrypted[5..62].iter_mut().enumerate() {
            *byte ^= key[index % key.len()];
        }
        assert_eq!(
            decode_container(&encrypted, "FantasyOfWind", 1024).unwrap(),
            expected
        );
    }

    #[test]
    fn helix_wrapper_uses_big_endian_seed_and_dotnet_shuffle() {
        let expected = plain();
        let mut random = crypto::DotNetRandom::new(12345);
        let mut permutation = [0_u8; 256];
        for (index, byte) in permutation.iter_mut().enumerate() {
            *byte = index as u8;
        }
        for index in 0..256 {
            let target = random.range(index, 256);
            permutation.swap(index, target);
        }
        let mut encrypted = expected.clone();
        encrypted[..16].fill(0);
        encrypted[..6].copy_from_slice(b"SzxFS\0");
        encrypted[8..12].copy_from_slice(&12345_i32.to_be_bytes());
        for byte in &mut encrypted[16..] {
            *byte = permutation[usize::from(*byte)];
        }
        assert_eq!(
            decode_container(&encrypted, "HelixWaltz2", 1024).unwrap(),
            expected
        );
    }

    #[test]
    fn anchor_wrapper_uses_per_chunk_reset_with_filename_key() {
        let mut expected = plain();
        expected.resize(7000, 0x27);
        let salt = crypto::md5_hex("KxZKZolAT3QXvsUU");
        let name = crypto::md5_hex("fixture");
        let key = format!("{}leiyan{}", &salt[..5], &name[27..]);
        let mut encrypted = expected.clone();
        for (index, block) in encrypted.chunks_mut(0x800).enumerate() {
            if index == 0
                || expected.len() / 0x800 == index
                || (index <= 4 && expected.len() % 2 == index % 2)
            {
                crypto::rc4(block, key.as_bytes()).unwrap();
            }
        }
        assert_eq!(
            decode_container_at(&encrypted, "AnchorPanic", Path::new("fixture.bundle"), 8192)
                .unwrap(),
            expected
        );
    }

    #[test]
    fn alice_and_girls_wrappers_restore_prefix_bytes() {
        let expected = plain();
        let key = [
            0x1b, 0x59, 0x62, 0x33, 0x78, 0x76, 0x45, 0xb3, 0x5b, 0x48, 0x39, 0xd7, 0x9c, 0x21,
            0x89, 0x94,
        ];
        let mut encrypted = expected.clone();
        encrypted[..8].fill(0x12);
        let seed = (encrypted.len() - 8) % 16;
        for (index, byte) in encrypted[8..136].iter_mut().enumerate() {
            *byte ^= key[(seed + index) % 16];
        }
        assert_eq!(
            decode_container(&encrypted, "AliceGearAegis", 1024).unwrap(),
            expected
        );
        let encrypted = xor_copy(&expected, &[0xa5]).unwrap();
        assert_eq!(
            decode_container(&encrypted, "GirlsFrontline", 1024).unwrap(),
            expected
        );
    }

    #[test]
    fn party_wrapper_decodes_tail_even_with_clear_unity_header() {
        let mut expected = plain();
        expected.resize(7000, 0x28);
        let encrypted = decode_party(&expected, Path::new("fixture.bundle")).unwrap();
        assert!(encrypted.starts_with(UNITY_FS));
        assert_ne!(encrypted, expected);
        assert_eq!(
            decode_container_at(
                &encrypted,
                "PartyAnimals",
                Path::new("fixture.bundle"),
                8192
            )
            .unwrap(),
            expected
        );
    }

    #[test]
    fn mark_removes_per_chunk_padding_and_keeps_partial_tail() {
        let original = vec![0x52; 0x264 * 2 + 10];
        let mut encrypted = b"mark".to_vec();
        let mut key_index = 0;
        for chunk in original.chunks(0x264) {
            for byte in chunk {
                encrypted.push(*byte ^ MARK_KEY[key_index % MARK_KEY.len()]);
                key_index += 1;
            }
            if chunk.len() == 0x264 {
                encrypted.extend_from_slice(&[0x11; 4]);
            }
        }
        assert_eq!(decode_mark(&encrypted, 4096).unwrap(), original);
    }

    #[test]
    fn jjk_header_and_counter_stream_decode() {
        let plain = plain();
        let key = [0x32, 0x73];
        let mut encrypted = key.to_vec();
        encrypted.extend(
            b"_GhostAssets_"
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ key[index % 2]),
        );
        encrypted.push(1 ^ key[0] ^ key[1]);
        let mut payload = plain.clone();
        crypto::jjk_counter_xor(&mut payload).unwrap();
        encrypted.extend_from_slice(&payload);
        assert_eq!(
            decode_container(&encrypted, "JJKPhantomParade", 1024).unwrap(),
            plain
        );
        encrypted[15] ^= 1;
        assert!(decode_container(&encrypted, "JJKPhantomParade", 1024).is_err());
    }

    #[test]
    fn oversized_and_malformed_inputs_are_bounded_without_panics() {
        assert!(decode_container(&plain(), "Normal", 8).is_err());
        for profile in supported_profiles() {
            for length in 0..192 {
                let bytes: Vec<u8> = (0..length)
                    .map(|index| (index as u8).wrapping_mul(43).wrapping_add(length as u8))
                    .collect();
                let outcome = std::panic::catch_unwind(|| {
                    decode_container_at(
                        &bytes,
                        profile,
                        Path::new("data/folder/fixture.bundle"),
                        1024,
                    )
                });
                assert!(outcome.is_ok(), "{profile} panicked for {length} bytes");
            }
        }
    }

    #[test]
    fn prepares_resources_without_transforming_or_modifying_source() {
        let directory = tempfile::tempdir().unwrap();
        let mut wrapped = 0x20_u32.to_le_bytes().to_vec();
        wrapped.extend_from_slice(&plain());
        fs::write(directory.path().join("fixture.bundle"), &wrapped).unwrap();
        fs::write(directory.path().join("fixture.resS"), b"resource bytes").unwrap();
        let regions = prepare_regions(directory.path(), "ProjectSekai", 1024)
            .unwrap()
            .unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].1.read_to_vec(1024).unwrap(), plain());
        assert_eq!(regions[1].1.read_to_vec(1024).unwrap(), b"resource bytes");
        assert_eq!(
            fs::read(directory.path().join("fixture.bundle")).unwrap(),
            wrapped
        );
    }
}
