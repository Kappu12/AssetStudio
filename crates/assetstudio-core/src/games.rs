//! Game wrapper compatibility transcribed from this fork's managed reader.
//!
//! A wrapper transform is not evidence that every asset layout in that game is
//! supported. See `docs/game-compatibility.md` for the profile-by-profile gaps.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::source::Region;

#[path = "game_bundle.rs"]
mod bundle;
#[path = "game_constants.rs"]
mod constants;
#[path = "game_crypto.rs"]
mod crypto;
#[path = "game_fairguard.rs"]
mod fairguard;
#[path = "game_mhy.rs"]
mod mhy;
#[path = "game_mr0k.rs"]
mod mr0k;

use constants::*;

const UNITY_FS: &[u8] = b"UnityFS\0";
const ORIGINAL_HEADER: &[u8; 16] = b"UnityFS\0\0\0\0\x075.x.";
const MAX_FILES: usize = 100_000;
const MAX_DIRECTORIES: usize = 16_384;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DIRECTORY_DEPTH: usize = 64;
const MAX_GAME_BUFFER_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SPLIT_PARTS: usize = 10_000;

pub type PreparedGameRegions = Option<Vec<(String, Region)>>;
pub type GamePreparation = (PreparedGameRegions, Vec<String>);

/// Shared across every selected path of one load/extraction operation. Failed
/// decode attempts consume read work too; folder/multi-file selection cannot
/// reset either the encoded-read or retained decoded-region allowance.
pub struct GamePreparationBudget {
    maximum: u64,
    retained_bytes: u64,
    materialized_input_bytes: u64,
    sources: HashSet<(&'static str, PathBuf)>,
    source_path_bytes: usize,
    discovered_files: usize,
    discovered_directories: usize,
    discovered_entries: usize,
    diagnostic_bytes: usize,
}

impl GamePreparationBudget {
    pub fn new(maximum: u64) -> Result<Self> {
        let maximum = maximum.min(MAX_GAME_BUFFER_BYTES);
        ensure!(
            maximum > 0,
            limit("Game input byte budget must be positive")
        );
        Ok(Self {
            maximum,
            retained_bytes: 0,
            materialized_input_bytes: 0,
            sources: HashSet::new(),
            source_path_bytes: 0,
            discovered_files: 0,
            discovered_directories: 0,
            discovered_entries: 0,
            diagnostic_bytes: 0,
        })
    }

    fn visit_file(&mut self) -> Result<()> {
        charge_discovery(
            &mut self.discovered_files,
            MAX_FILES,
            "Game input contains too many files across the selected paths",
        )
    }

    fn visit_directory(&mut self) -> Result<()> {
        charge_discovery(
            &mut self.discovered_directories,
            MAX_DIRECTORIES,
            "Game input contains too many directories across the selected paths",
        )
    }

    fn visit_entry(&mut self) -> Result<()> {
        charge_discovery(
            &mut self.discovered_entries,
            MAX_FILES + MAX_DIRECTORIES,
            "Game input contains too many directory entries across the selected paths",
        )
    }
}

fn charge_discovery(counter: &mut usize, maximum: usize, description: &'static str) -> Result<()> {
    *counter = counter.checked_add(1).with_context(|| limit(description))?;
    ensure!(*counter <= maximum, limit(description));
    Ok(())
}

const SUPPORTED: &[&str] = &[
    "Normal",
    "UnityCN",
    "Naraka",
    "TOT",
    "BH3PrePre",
    "BH3Pre",
    "ExAstris",
    "LoveAndDeepspace",
    "ArknightsEndfield",
    "OPFP",
    "NetEase",
    "PerpetualNovelty",
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
    Ok(prepare_regions_with_policy(input, profile, max_bytes, false)?.0)
}

/// The game equivalent of the backend's skip-input policy. Per-file I/O and
/// format failures are recoverable; shared discovery/materialization limits
/// remain hard errors even when `continue_on_error` is enabled.
pub fn prepare_regions_with_policy(
    input: &Path,
    profile: &str,
    max_bytes: u64,
    continue_on_error: bool,
) -> Result<GamePreparation> {
    prepare_regions_with_budget(
        input,
        profile,
        &mut GamePreparationBudget::new(max_bytes)?,
        continue_on_error,
    )
}

pub fn prepare_regions_with_budget(
    input: &Path,
    profile: &str,
    budget: &mut GamePreparationBudget,
    continue_on_error: bool,
) -> Result<GamePreparation> {
    let profile = canonical_profile(profile)?;
    if matches!(profile, "Normal" | "UnityCN") {
        return Ok((None, Vec::new()));
    }
    let maximum = budget.maximum;
    let files = collect_sources(input, budget)?;
    let mut regions = Vec::new();
    regions.try_reserve(files.len()).map_err(allocation_limit)?;
    let mut diagnostics = Vec::new();
    for file in files {
        let identity = (profile, file.logical_path.clone());
        if budget.sources.contains(&identity) {
            continue;
        }
        ensure!(
            budget.sources.len() < MAX_FILES,
            limit("Shared game source count exceeds file limit")
        );
        let path_bytes = budget
            .source_path_bytes
            .checked_add(file.logical_path.as_os_str().as_encoded_bytes().len())
            .with_context(|| limit("Shared game path size overflow"))?;
        ensure!(
            path_bytes <= 256 * 1024 * 1024,
            limit("Shared game source paths exceed byte limit")
        );
        let path = file.logical_path.to_string_lossy().into_owned();
        ensure!(
            path.len() <= MAX_PATH_BYTES,
            limit("Game input path is too long")
        );
        let remaining = maximum
            .checked_sub(budget.retained_bytes)
            .with_context(|| limit("Game input materialization accounting overflow"))?;
        let remaining_input = maximum
            .checked_sub(budget.materialized_input_bytes)
            .with_context(|| limit("Game input read accounting overflow"))?;
        let available = remaining.min(remaining_input);
        let prepared = (|| -> Result<(Region, u64)> {
            let source = file.open()?;
            if is_companion_resource(&file.logical_path) {
                return Ok((source, 0));
            }
            let mut prefix = [0_u8; 4096];
            let prefix_length = usize::try_from(source.len().min(prefix.len() as u64))?;
            source.read_exact_at(0, &mut prefix[..prefix_length])?;
            if looks_like_standard_input(&prefix[..prefix_length], source.len())
                && !transform_standard_container(profile, &prefix[..prefix_length])
            {
                return Ok((source, 0));
            }
            if source.len() > available {
                return Err(anyhow::Error::new(limit(
                    "Game wrapper input exceeds the cumulative materialization byte budget",
                )));
            }
            // Charge attempted reads before I/O or decoding. Otherwise a
            // continue-mode folder of malformed sparse files could repeat
            // the full byte budget indefinitely without retaining output.
            budget.materialized_input_bytes = budget
                .materialized_input_bytes
                .checked_add(source.len())
                .with_context(|| limit("Game input read accounting overflow"))?;
            let length = usize::try_from(source.len())
                .with_context(|| limit("Game input exceeds platform address space"))?;
            let mut data = Vec::new();
            data.try_reserve_exact(length).map_err(allocation_limit)?;
            data.resize(length, 0);
            source.read_exact_at(0, &mut data)?;
            // The per-file decoder receives the absolute file budget. Its
            // result is committed to the shared budget outside this recovery
            // scope, so expansion cannot turn a shared-limit failure into a
            // silently skipped input.
            let decoded = decode_container_at(&data, profile, &file.logical_path, maximum)
                .with_context(|| {
                    format!("Could not decode game wrapper {:?}", file.logical_path)
                })?;
            let length = decoded.len() as u64;
            Ok((Region::from_bytes(decoded), length))
        })();
        match prepared {
            Ok((region, length)) => {
                budget.retained_bytes = budget
                    .retained_bytes
                    .checked_add(length)
                    .with_context(|| limit("Game wrapper materialization accounting overflow"))?;
                ensure!(
                    budget.retained_bytes <= maximum,
                    limit(format!(
                        "Decoded game wrappers exceed the {maximum} byte cumulative materialization budget"
                    ))
                );
                budget.sources.try_reserve(1).map_err(allocation_limit)?;
                budget.sources.insert(identity);
                budget.source_path_bytes = path_bytes;
                regions.push((path, region));
            }
            Err(error) if continue_on_error && !is_preparation_limit(&error) => {
                let mut message = format!("Skipped game input {:?}: {error:#}", file.logical_path);
                if message.len() > 8192 {
                    let mut boundary = 8192;
                    while !message.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    message.truncate(boundary);
                }
                budget.diagnostic_bytes = budget
                    .diagnostic_bytes
                    .checked_add(message.len())
                    .with_context(|| limit("Game diagnostics accounting overflow"))?;
                ensure!(
                    budget.diagnostic_bytes <= 16 * 1024 * 1024,
                    limit("Game diagnostics exceed 16 MiB")
                );
                diagnostics.try_reserve(1).map_err(allocation_limit)?;
                diagnostics.push(message);
            }
            Err(error) => return Err(error),
        }
    }
    Ok((Some(regions), diagnostics))
}

#[derive(Debug)]
struct GameBudgetExceeded(String);

impl std::fmt::Display for GameBudgetExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GameBudgetExceeded {}

fn limit(message: impl Into<String>) -> GameBudgetExceeded {
    GameBudgetExceeded(message.into())
}

fn allocation_limit(error: std::collections::TryReserveError) -> GameBudgetExceeded {
    limit(format!(
        "Game preparation allocation limit exceeded: {error}"
    ))
}

/// Resource limits remain fatal through both the preparation and Session
/// continue-on-error policies. `anyhow` preserves this typed cause across
/// context, so callers must classify the cause instead of matching text.
pub(crate) fn is_preparation_limit(error: &anyhow::Error) -> bool {
    error.is::<GameBudgetExceeded>()
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
        limit(format!("Game wrapper path exceeds {MAX_PATH_BYTES} bytes"))
    );
    let maximum = max_bytes.min(MAX_GAME_BUFFER_BYTES);
    ensure!(
        data.len() as u64 <= maximum,
        limit(format!("Game wrapper exceeds {maximum} byte budget"))
    );
    if matches!(profile, "Normal" | "UnityCN") {
        return copy_bytes(data);
    }
    if looks_like_standard_input(data, data.len() as u64)
        && !transform_standard_container(profile, data)
    {
        return copy_bytes(data);
    }
    let output = match profile {
        "FakeHeader" | "ShiningNikki" => strip_fake_header(data)?,
        "OPFP" => bundle::normalize(&strip_fake_header(data)?, profile, path, maximum)?,
        "NetEase" | "PerpetualNovelty" => bundle::normalize(data, profile, path, maximum)?,
        "Naraka" | "TOT" | "BH3PrePre" | "BH3Pre" | "ExAstris" | "ArknightsEndfield" => {
            mhy::normalize(data, profile, maximum)?
        }
        "LoveAndDeepspace" => mhy::normalize(&decode_love_and_deepspace(data)?, profile, maximum)?,
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
        limit(format!(
            "Decoded game wrapper exceeds {maximum} byte budget"
        ))
    );
    ensure!(
        looks_like_standard_input(&output, output.len() as u64),
        "Decoded {profile} input does not contain a recognized Unity/container header"
    );
    Ok(output)
}

fn force_standard_bundle_transform(profile: &str) -> bool {
    matches!(
        profile,
        "FakeHeader"
            | "ShiningNikki"
            | "AnchorPanic"
            | "PartyAnimals"
            | "OPFP"
            | "NetEase"
            | "PerpetualNovelty"
            | "Naraka"
            | "TOT"
            | "BH3PrePre"
            | "BH3Pre"
            | "ExAstris"
            | "ArknightsEndfield"
            | "LoveAndDeepspace"
    )
}

fn transform_standard_container(profile: &str, data: &[u8]) -> bool {
    if !force_standard_bundle_transform(profile) {
        return false;
    }
    if data.starts_with(UNITY_FS) {
        return true;
    }
    if matches!(
        profile,
        "Naraka" | "TOT" | "BH3PrePre" | "BH3Pre" | "ExAstris" | "ArknightsEndfield"
    ) {
        for signature in [b"UnityWeb\0".as_slice(), b"UnityRaw\0".as_slice()] {
            if data.starts_with(signature)
                && data
                    .get(signature.len()..signature.len() + 4)
                    .is_some_and(|bytes| bytes == 6u32.to_be_bytes())
            {
                return true;
            }
        }
    }
    false
}

fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(data.len())
        .map_err(allocation_limit)?;
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
        limit("DreamscapeAlbireo decoded size exceeds byte budget")
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
        .with_context(|| limit("DreamscapeAlbireo output size overflow"))?;
    ensure!(
        output_size as u64 <= maximum,
        limit("DreamscapeAlbireo output exceeds byte budget")
    );
    ensure!(
        size == output_size as u64,
        "DreamscapeAlbireo declared bundle size does not match reconstructed bytes"
    );
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_size)
        .map_err(allocation_limit)?;
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

fn decode_love_and_deepspace(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        data.len() >= 16,
        "LoveAndDeepspace encrypted header is truncated"
    );
    const VECTOR: [u8; 4] = [0x35, 0x6b, 0x05, 0];
    let mut key = [0_u8; 64];
    for (index, &byte) in data[..16].iter().enumerate() {
        let seed = byte ^ ORIGINAL_HEADER[index] ^ VECTOR[0];
        for (group, vector) in VECTOR.iter().enumerate() {
            key[group * 16 + index] = seed ^ vector;
        }
    }
    xor_copy(data, &key)
}

/// Decodes the old GI_CB1 `mark` envelope. This standalone primitive is tested,
/// but GI_CB1 is deliberately not advertised as a working profile: its custom
/// serialized and renderer layouts have not yet been ported.
pub fn decode_mark(data: &[u8], maximum: u64) -> Result<Vec<u8>> {
    ensure!(
        data.len() as u64 <= maximum.min(MAX_GAME_BUFFER_BYTES),
        limit("Mark input exceeds byte budget")
    );
    ensure!(data.starts_with(b"mark"), "Invalid mark wrapper signature");
    const BLOCK: usize = 0xa00;
    const CHUNK: usize = 0x264;
    const PADDING: usize = 4;
    const PADDED_BLOCK: usize = BLOCK + (BLOCK / CHUNK + 1) * PADDING;
    let mut output = Vec::new();
    output
        .try_reserve_exact(data.len() - 4)
        .map_err(allocation_limit)?;
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

struct GameSource {
    logical_path: PathBuf,
    parts: Vec<(Option<usize>, PathBuf)>,
}

impl GameSource {
    fn open(&self) -> Result<Region> {
        let split = self.parts.first().is_some_and(|part| part.0.is_some());
        if split {
            ensure!(
                self.parts.len() <= MAX_SPLIT_PARTS,
                limit("Game split input has too many segments")
            );
        } else {
            ensure!(
                self.parts.len() == 1,
                "Both a complete input and split segments exist for {:?}",
                self.logical_path
            );
        }
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(self.parts.len())
            .map_err(allocation_limit)?;
        for (position, (index, path)) in self.parts.iter().enumerate() {
            if split {
                ensure!(
                    *index == Some(position),
                    "Game split input has duplicate or missing segment {position}: {:?}",
                    self.logical_path
                );
            }
            let metadata = fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Game split input is not a regular file: {path:?}"
            );
            regions.push(Region::from_file(path)?);
        }
        Region::concatenate(regions, u64::MAX).map_err(|error| {
            limit(format!("Game split source region limit exceeded: {error}")).into()
        })
    }
}

fn split_base(path: &Path) -> Option<(PathBuf, usize)> {
    let suffix = path.extension()?.to_str()?.strip_prefix("split")?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Keep even a too-large index in the group, then report it through the
    // ordinary per-input failure policy rather than interpreting it as a file.
    let index = suffix.parse().unwrap_or(usize::MAX);
    let stem = path.file_stem()?;
    Some((path.with_file_name(stem), index))
}

fn collect_sources(input: &Path, budget: &mut GamePreparationBudget) -> Result<Vec<GameSource>> {
    let mut files = collect_files(input, budget)?;
    if input.is_file() && split_base(input).is_some() {
        // Absolute lexical paths keep selected filename/path-derived keys,
        // while ensuring read_dir's entries and the explicitly selected part
        // have the same prefix even for a relative filename in the cwd.
        let selected = if input.is_absolute() {
            input.to_owned()
        } else {
            std::env::current_dir()?.join(input)
        };
        files[0] = selected.clone();
        let (base, _) = split_base(&selected).context("Game split source name changed")?;
        let parent = selected
            .parent()
            .context("Game split source has no parent")?;
        budget.visit_directory()?;
        for entry in fs::read_dir(parent)? {
            budget.visit_entry()?;
            let path = entry?.path();
            if (path == base || split_base(&path).is_some_and(|(candidate, _)| candidate == base))
                && path != selected
            {
                budget.visit_file()?;
                files.try_reserve(1).map_err(allocation_limit)?;
                files.push(path);
            }
        }
    }
    let mut grouped: BTreeMap<PathBuf, Vec<(Option<usize>, PathBuf)>> = BTreeMap::new();
    for file in files {
        ensure!(
            file.as_os_str().as_encoded_bytes().len() <= MAX_PATH_BYTES,
            limit("Game input path is too long")
        );
        let (logical, index) = match split_base(&file) {
            Some((path, index)) => (path, Some(index)),
            None => (file.clone(), None),
        };
        let parts = grouped.entry(logical).or_default();
        parts.try_reserve(1).map_err(allocation_limit)?;
        parts.push((index, file));
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(grouped.len())
        .map_err(allocation_limit)?;
    for (logical_path, mut parts) in grouped {
        parts.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        result.push(GameSource {
            logical_path,
            parts,
        });
    }
    Ok(result)
}

fn collect_files(input: &Path, budget: &mut GamePreparationBudget) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![(input.to_owned(), 0_usize)];
    while let Some((path, depth)) = pending.pop() {
        ensure!(
            path.as_os_str().as_encoded_bytes().len() <= MAX_PATH_BYTES,
            limit("Game input path is too long")
        );
        ensure!(
            depth <= MAX_DIRECTORY_DEPTH,
            limit(format!(
                "Game input directory depth exceeds {MAX_DIRECTORY_DEPTH}"
            ))
        );
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("Cannot inspect input {path:?}"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Game input symlinks are not followed: {path:?}"
        );
        if metadata.is_file() {
            budget.visit_file()?;
            files.try_reserve(1).map_err(allocation_limit)?;
            files.push(path);
        } else if metadata.is_dir() {
            budget.visit_directory()?;
            for entry in fs::read_dir(&path)? {
                budget.visit_entry()?;
                pending.try_reserve(1).map_err(allocation_limit)?;
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
    fn dreamscape_reconstructs_64_bit_size_and_block_table() {
        // Encode the inverse bit permutation independently, including the
        // cross-word size-byte arrangement from the managed wire format.
        let inverse = |value: u32| {
            ((value >> 3) & 0x3ff)
                | ((value >> 14) & 0x3fc00)
                | ((value << 5) & 0x1ffc0000)
                | ((value << 29) & 0xe0000000)
        };
        let flags = 0x40_u32;
        let info_size = 24_u32;
        let u4 = inverse(flags ^ 0x70020017);
        let u1 = inverse(info_size ^ u4);
        let residue = (u1 & 255) ^ (u4 >> 24);
        let mut size = residue;
        while size < 49 + info_size {
            size += 256;
        }
        let payload_length = (size - 49) as usize;
        let u2 = ((u4 & 255) << 24) | ((size ^ u1) >> 8);
        let u5 = u4 & 0xffff_ff00;
        let u6 = inverse(info_size ^ u1);
        let mut encrypted = b"MJJ\0".to_vec();
        for value in [u1, u2, 0, u4, u5, u6] {
            encrypted.extend_from_slice(&value.to_be_bytes());
        }
        let key = [0x1e, 0x1e, 1, 1, 0xfc];
        for index in 0..24 {
            encrypted.push(key[index % key.len()]);
        }
        encrypted.resize(28 + payload_length, 0);
        let decoded = decode_container(&encrypted, "DreamscapeAlbireo", 1024).unwrap();
        assert_eq!(decoded.len(), size as usize);
        assert_eq!(
            u64::from_be_bytes(decoded[29..37].try_into().unwrap()),
            u64::from(size)
        );
        assert_eq!(&decoded[49..73], &[0; 24]);
        let region = Region::from_bytes(decoded);
        assert!(unity_rs_core::bundle::UnityFsBundle::open(&region).is_ok());
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

    #[test]
    fn continue_policy_skips_bad_game_input_but_never_shared_budget_limits() {
        let directory = tempfile::tempdir().unwrap();
        let mut wrapped = 0x20_u32.to_le_bytes().to_vec();
        wrapped.extend_from_slice(&plain());
        fs::write(directory.path().join("a-good.bundle"), &wrapped).unwrap();
        fs::write(directory.path().join("b-bad.bundle"), b"malformed input").unwrap();
        assert!(prepare_regions(directory.path(), "ProjectSekai", 1024).is_err());
        let (regions, diagnostics) =
            prepare_regions_with_policy(directory.path(), "ProjectSekai", 1024, true).unwrap();
        assert_eq!(regions.unwrap().len(), 1);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("b-bad.bundle"));
        fs::remove_file(directory.path().join("b-bad.bundle")).unwrap();
        fs::write(directory.path().join("b-second.bundle"), wrapped).unwrap();
        let error =
            prepare_regions_with_policy(directory.path(), "ProjectSekai", 300, true).unwrap_err();
        assert!(error.to_string().contains("cumulative"));
    }

    #[test]
    fn malformed_inputs_still_consume_shared_read_work_budget() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..4 {
            fs::write(directory.path().join(format!("{index}.bundle")), [0x61; 80]).unwrap();
        }
        let error =
            prepare_regions_with_policy(directory.path(), "ProjectSekai", 200, true).unwrap_err();
        assert!(error.to_string().contains("cumulative"));
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_discovery_rejects_symlinks_even_in_continue_mode() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("actual.resS"), b"resource").unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("actual.resS"),
            directory.path().join("alias.resS"),
        )
        .unwrap();
        assert!(prepare_regions_with_policy(directory.path(), "ProjectSekai", 1024, true).is_err());
    }

    #[test]
    fn shared_preparation_budget_cannot_reset_across_separate_selected_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut encoded = 0x20_u32.to_le_bytes().to_vec();
        encoded.extend_from_slice(&plain());
        let a = root.path().join("a.bundle");
        let b = root.path().join("b.bundle");
        fs::write(&a, &encoded).unwrap();
        fs::write(&b, &encoded).unwrap();
        let mut budget = GamePreparationBudget::new(300).unwrap();
        let first = prepare_regions_with_budget(&a, "ProjectSekai", &mut budget, false)
            .unwrap()
            .0
            .unwrap();
        assert_eq!(first[0].1.len(), 256);
        let error = prepare_regions_with_budget(&b, "ProjectSekai", &mut budget, true).unwrap_err();
        assert!(error.is::<GameBudgetExceeded>());
        assert_eq!(budget.retained_bytes, 256);
        assert_eq!(budget.materialized_input_bytes, 260);
        assert_eq!(first[0].1.read_to_vec(300).unwrap(), plain());
    }

    #[test]
    fn expansion_limits_remain_fatal_through_continue_policy_and_context() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("oversized-table.bundle");
        let mut bundle = b"UnityFS\0".to_vec();
        bundle.extend_from_slice(&6u32.to_be_bytes());
        bundle.extend_from_slice(b"5.x.x\0");
        bundle.extend_from_slice(b"2017.4.18f1\0");
        let size_offset = bundle.len();
        bundle.extend_from_slice(&0u64.to_be_bytes());
        bundle.extend_from_slice(&0u32.to_be_bytes());
        bundle.extend_from_slice(&8192u32.to_be_bytes());
        bundle.extend_from_slice(&0x40u32.to_be_bytes());
        let length = bundle.len() as u64;
        bundle[size_offset..size_offset + 8].copy_from_slice(&length.to_be_bytes());
        fs::write(&path, bundle).unwrap();

        for profile in ["ExAstris", "NetEase"] {
            let error = prepare_regions_with_policy(&path, profile, 1024, true)
                .unwrap_err()
                .context("Selected-input operation failed");
            assert!(is_preparation_limit(&error), "{profile}: {error:#}");
        }
        // A hostile LZMA dictionary is also a resource limit, even though the
        // table's declared output and compressed input are both tiny.
        let error = bundle::decompress_table(&[0x5d, 0, 0, 0, 0x10], 1, 1)
            .unwrap_err()
            .context("Could not decode selected game input");
        assert!(is_preparation_limit(&error), "{error:#}");
    }

    #[test]
    fn shared_path_metadata_exhaustion_remains_fatal_across_calls() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("a.resS");
        let second = root.path().join("b.resS");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();
        let mut budget = GamePreparationBudget::new(10).unwrap();
        prepare_regions_with_budget(&first, "Naraka", &mut budget, true).unwrap();
        assert_eq!(
            budget.source_path_bytes,
            first.as_os_str().as_encoded_bytes().len()
        );
        // Represent earlier selected roots without allocating 256 MiB of
        // actual path strings in a regression test.
        budget.source_path_bytes = 256 * 1024 * 1024;
        let error = prepare_regions_with_budget(&second, "Naraka", &mut budget, true).unwrap_err();
        assert!(is_preparation_limit(&error));
        assert_eq!(budget.sources.len(), 1);
        assert_eq!(budget.retained_bytes, 0);
    }

    #[test]
    fn discovery_depth_limit_is_fatal_in_continue_mode() {
        let root = tempfile::tempdir().unwrap();
        let mut nested = root.path().to_path_buf();
        for _ in 0..=MAX_DIRECTORY_DEPTH {
            nested.push("d");
            fs::create_dir(&nested).unwrap();
        }
        let error = prepare_regions_with_policy(root.path(), "Naraka", 10, true).unwrap_err();
        assert!(is_preparation_limit(&error), "{error:#}");
    }

    #[test]
    fn oversized_selected_paths_are_limits_before_filesystem_lookup() {
        let path = PathBuf::from("a".repeat(MAX_PATH_BYTES + 1));
        let error = prepare_regions_with_policy(&path, "Naraka", 10, true).unwrap_err();
        assert!(is_preparation_limit(&error), "{error:#}");
    }

    #[test]
    fn discovery_and_diagnostic_allowances_do_not_reset_between_selected_paths() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.resS");
        let b = root.path().join("b.resS");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();

        let mut files = GamePreparationBudget::new(100).unwrap();
        files.discovered_files = MAX_FILES - 1;
        prepare_regions_with_budget(&a, "Naraka", &mut files, true).unwrap();
        assert!(is_preparation_limit(
            &prepare_regions_with_budget(&b, "Naraka", &mut files, true).unwrap_err()
        ));

        let mut directories = GamePreparationBudget::new(100).unwrap();
        directories.discovered_directories = MAX_DIRECTORIES - 1;
        prepare_regions_with_budget(root.path(), "Naraka", &mut directories, true).unwrap();
        assert!(is_preparation_limit(
            &prepare_regions_with_budget(root.path(), "Naraka", &mut directories, true)
                .unwrap_err()
        ));

        let mut entries = GamePreparationBudget::new(100).unwrap();
        entries.discovered_entries = MAX_FILES + MAX_DIRECTORIES - 2;
        prepare_regions_with_budget(root.path(), "Naraka", &mut entries, true).unwrap();
        assert!(is_preparation_limit(
            &prepare_regions_with_budget(root.path(), "Naraka", &mut entries, true).unwrap_err()
        ));

        let bad_a = root.path().join("a.bundle");
        let bad_b = root.path().join("b.bundle");
        fs::write(&bad_a, b"malformed").unwrap();
        fs::write(&bad_b, b"malformed").unwrap();
        let mut diagnostics = GamePreparationBudget::new(100).unwrap();
        let (_, messages) =
            prepare_regions_with_budget(&bad_a, "Naraka", &mut diagnostics, true).unwrap();
        assert_eq!(diagnostics.diagnostic_bytes, messages[0].len());
        diagnostics.diagnostic_bytes = 16 * 1024 * 1024;
        let error =
            prepare_regions_with_budget(&bad_b, "Naraka", &mut diagnostics, true).unwrap_err();
        assert!(is_preparation_limit(&error), "{error:#}");
        assert_eq!(diagnostics.materialized_input_bytes, 18);
    }

    #[test]
    fn failed_decode_work_remains_charged_across_preparation_calls() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.bundle");
        let b = root.path().join("b.bundle");
        fs::write(&a, [0x61; 80]).unwrap();
        fs::write(&b, [0x62; 80]).unwrap();
        let mut budget = GamePreparationBudget::new(100).unwrap();
        let (first, diagnostics) =
            prepare_regions_with_budget(&a, "ProjectSekai", &mut budget, true).unwrap();
        assert!(first.unwrap().is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(budget.materialized_input_bytes, 80);
        assert_eq!(budget.retained_bytes, 0);
        assert!(
            prepare_regions_with_budget(&b, "ProjectSekai", &mut budget, true)
                .unwrap_err()
                .is::<GameBudgetExceeded>()
        );
    }

    #[test]
    fn encrypted_split_parts_join_in_numeric_order_and_keep_unsuffixed_filename_keys() {
        let root = tempfile::tempdir().unwrap();
        let expected = plain();
        let encoded = decode_reverse(&expected, Path::new("asset.bundle")).unwrap();
        let mut parts = Vec::new();
        for (index, bytes) in encoded.chunks(22).enumerate() {
            let path = root.path().join(format!("asset.bundle.split{index}"));
            fs::write(&path, bytes).unwrap();
            parts.push((path, bytes.to_vec()));
        }
        assert!(parts.len() > 10);
        let regions = prepare_regions(&parts[10].0, "Reverse1999", 1024)
            .unwrap()
            .unwrap();
        assert_eq!(regions.len(), 1);
        assert!(regions[0].0.ends_with("asset.bundle"));
        assert_eq!(regions[0].1.read_to_vec(1024).unwrap(), expected);
        for (path, bytes) in parts {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        fs::write(root.path().join("asset.resS.split0"), b"resource-").unwrap();
        fs::write(root.path().join("asset.resS.split1"), b"stream").unwrap();
        let regions = prepare_regions(root.path(), "Reverse1999", 1024)
            .unwrap()
            .unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(
            regions
                .iter()
                .find(|(name, _)| name.ends_with("asset.resS"))
                .unwrap()
                .1
                .read_to_vec(1024)
                .unwrap(),
            b"resource-stream"
        );
    }

    #[test]
    fn incomplete_ambiguous_and_oversized_split_groups_are_explicit_failures() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("bundle.split0");
        let last = root.path().join("bundle.split2");
        fs::write(&first, [0x61; 80]).unwrap();
        fs::write(&last, [0x62; 80]).unwrap();
        assert!(prepare_regions(&first, "ProjectSekai", 1024).is_err());
        let (regions, diagnostics) =
            prepare_regions_with_policy(&first, "ProjectSekai", 1024, true).unwrap();
        assert!(regions.unwrap().is_empty());
        assert!(diagnostics[0].contains("missing segment 1"));
        fs::rename(last, root.path().join("bundle.split00")).unwrap();
        assert!(
            prepare_regions(&first, "ProjectSekai", 1024)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        fs::rename(
            root.path().join("bundle.split00"),
            root.path().join("bundle.split1"),
        )
        .unwrap();
        assert!(
            prepare_regions_with_policy(&first, "ProjectSekai", 100, true)
                .unwrap_err()
                .is::<GameBudgetExceeded>()
        );
        fs::write(
            root.path().join("bundle"),
            b"unambiguous selection required",
        )
        .unwrap();
        assert!(
            prepare_regions(root.path(), "ProjectSekai", 1024)
                .unwrap_err()
                .to_string()
                .contains("Both a complete input")
        );
        assert!(
            prepare_regions(&first, "ProjectSekai", 1024)
                .unwrap_err()
                .to_string()
                .contains("Both a complete input")
        );
    }

    #[cfg(unix)]
    #[test]
    fn selected_split_part_never_follows_a_symlink_companion() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("wrapped.split0");
        fs::write(&first, b"first bytes").unwrap();
        let target = root.path().join("outside");
        fs::write(&target, b"original").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("wrapped.split1")).unwrap();
        assert!(prepare_regions(&first, "ProjectSekai", 1024).is_err());
        assert_eq!(fs::read(target).unwrap(), b"original");
    }

    #[test]
    fn love_and_deepspace_prefix_key_matches_original_csharp_oracle() {
        let encoded = include_bytes!("../tests/fixtures/mhy-oracle/LnD-reference.encrypted");
        let expected = include_bytes!("../tests/fixtures/mhy-oracle/LnD-reference.bin");
        assert_eq!(decode_love_and_deepspace(encoded).unwrap(), expected);
        for length in 0..16 {
            assert!(decode_love_and_deepspace(&encoded[..length]).is_err());
        }
    }

    #[test]
    fn choosing_multiple_parts_or_overlapping_paths_only_decodes_each_source_once() {
        let root = tempfile::tempdir().unwrap();
        let mut encoded = 0x20_u32.to_le_bytes().to_vec();
        encoded.extend_from_slice(&plain());
        let a = root.path().join("asset.bundle.split0");
        let b = root.path().join("asset.bundle.split1");
        fs::write(&a, &encoded[..100]).unwrap();
        fs::write(&b, &encoded[100..]).unwrap();
        let mut budget = GamePreparationBudget::new(300).unwrap();
        let first = prepare_regions_with_budget(&a, "ProjectSekai", &mut budget, false)
            .unwrap()
            .0
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(
            prepare_regions_with_budget(&b, "ProjectSekai", &mut budget, false)
                .unwrap()
                .0
                .unwrap()
                .is_empty()
        );
        assert!(
            prepare_regions_with_budget(root.path(), "ProjectSekai", &mut budget, false)
                .unwrap()
                .0
                .unwrap()
                .is_empty()
        );
        assert_eq!(budget.materialized_input_bytes, 260);
        assert_eq!(budget.retained_bytes, 256);
        assert_eq!(budget.sources.len(), 1);
    }
}
