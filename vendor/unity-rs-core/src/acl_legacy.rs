//! ACL 1.1 variable-QVV compressed clips, as used by the baseline MHY DLL.
//!
//! Format contracts follow Nicholas Frechette and ACL contributors' MIT v1.1.0,
//! commit bb2a8fa89ecd7d197b670809c5ac983b38051401: core/compressed_clip.h,
//! compression/stream/write_*.h, and decompression/decompress_data.h.
//! Every retained positive fixture is independently checked against both that
//! upstream compressor/decoder and the original repository x64 acl.dll.
//! This is a distinct binary format from ACL 2.x compressed_tracks. Only the
//! verified variable rotation/vector formats are accepted; full/raw formats,
//! SR and database variants require different contracts.
use super::AclNativeDecodeLimits;
use crate::source::Region;
use crate::{Error, Result};

const BASE: usize = 16;
const HEADER_SIZE: usize = 32;
const LEGACY_TAG: u32 = 0xAC10_AC10;
const LEGACY_VERSION: u16 = 3;
const VARIABLE_BITS: [u8; 19] = [
    0, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 32,
];

/// Frame-major, then bone-major quaternion XYZW/translation XYZ/scale XYZ.
/// Default scales remain present in this output even if they were omitted from
/// storage, matching the original MHY decoder's ten values per bone.
#[derive(Debug, Clone, PartialEq)]
pub struct AclLegacyTracks {
    pub bone_count: u16,
    pub frame_count: u32,
    pub sample_rate: u32,
    pub has_scale: bool,
    pub values: Vec<f32>,
}

impl AclLegacyTracks {
    pub fn times(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        (0..self.frame_count).map(|frame| frame as f32 / self.sample_rate as f32)
    }
}

/// Decodes the stored frames of a verified legacy variable-QVV clip. Resource
/// limits cover compressed bytes, all output samples and per-bone working data.
/// Caller-specific Unity binding maps remain separate from this binary codec.
pub fn decode_legacy_compressed_clip(
    region: &Region,
    limits: AclNativeDecodeLimits,
) -> Result<AclLegacyTracks> {
    if region.len() < (BASE + HEADER_SIZE) as u64
        || region.len() > limits.compressed.maximum_compressed_bytes
    {
        return Err(invalid("compressed payload is outside its byte limits"));
    }
    let mut prefix = [0u8; BASE + HEADER_SIZE];
    region.read_exact_at(0, &mut prefix)?;
    if u64::from(u32_at(&prefix, 0)?) != region.len() {
        return Err(invalid("declared compressed size disagrees with region"));
    }
    if u32_at(&prefix, 8)? != LEGACY_TAG
        || u16_at(&prefix, 12)? != LEGACY_VERSION
        || prefix[14] != 0
    {
        return Err(Error::unsupported(
            "legacy ACL requires compressed_clip tag AC10AC10, serialized version 3 and uniformly sampled algorithm 0",
        ));
    }
    if prefix[BASE + 4..BASE + 7] != [4, 3, 3] {
        return Err(Error::unsupported(
            "legacy MHY ACL supports verified variable drop-W rotations and variable vectors only (formats 4/3/3)",
        ));
    }
    let bone_count = u16_at(&prefix, BASE)?;
    let segments = usize::from(u16_at(&prefix, BASE + 2)?);
    let frame_count = u32_at(&prefix, BASE + 12)?;
    let sample_rate = u32_at(&prefix, BASE + 16)?;
    let clip_flags = prefix[BASE + 7];
    let segment_flags = prefix[BASE + 8];
    let has_scale = prefix[BASE + 9] != 0;
    let default_scale = prefix[BASE + 10];
    if bone_count == 0
        || frame_count == 0
        || segments == 0
        || segments as u64 > u64::from(frame_count)
        || sample_rate == 0
    {
        return Err(invalid(
            "bone/frame/segment counts and sample rate must be positive",
        ));
    }
    if u32::from(bone_count) > limits.compressed.maximum_tracks
        || frame_count > limits.compressed.maximum_samples_per_track
    {
        return Err(invalid("bone or frame count exceeds its limit"));
    }
    if prefix[BASE + 9] > 1 || default_scale > 1 || clip_flags & !7 != 0 || segment_flags & !7 != 0
    {
        return Err(invalid("unknown range/default-transform flags"));
    }
    let values_count = u64::from(bone_count)
        .checked_mul(u64::from(frame_count))
        .and_then(|n| n.checked_mul(10))
        .ok_or_else(|| invalid("sample count overflowed"))?;
    if values_count > limits.compressed.maximum_decompressed_values {
        return Err(invalid("decoded scalar count exceeds its limit"));
    }
    let combined = values_count
        .checked_mul(4)
        .and_then(|n| n.checked_add(region.len()))
        .and_then(|n| n.checked_add(u64::from(bone_count) * 512))
        .ok_or_else(|| invalid("materialization budget overflowed"))?;
    if combined > limits.maximum_materialized_bytes || usize::try_from(combined).is_err() {
        return Err(invalid(
            "compressed input, decoded output and working data exceed shared budget",
        ));
    }
    let bytes = region.read_to_vec(limits.compressed.maximum_compressed_bytes)?;
    let stored_hash = u32_at(&bytes, 4)?;
    let actual_hash = bytes[8..].iter().fold(2_166_136_261u32, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    if stored_hash != actual_hash {
        return Err(invalid("compressed payload hash mismatch"));
    }
    let bones = usize::from(bone_count);
    let frames =
        usize::try_from(frame_count).map_err(|_| invalid("frame count does not fit platform"))?;
    let segment_headers = required(offset16(&bytes, BASE + 20)?, "missing segment headers")?;
    let defaults = required(offset16(&bytes, BASE + 22)?, "missing default bitset")?;
    let constants = required(offset16(&bytes, BASE + 24)?, "missing constant bitset")?;
    let constant_data = offset16(&bytes, BASE + 26)?;
    let clip_ranges = offset16(&bytes, BASE + 28)?;
    let headers_end = end(segment_headers, segments * 20)?;
    let kinds = if has_scale { 3 } else { 2 };
    let track_count = bones * kinds;
    let bitset_size = track_count.div_ceil(32) * 4;
    if segment_headers < BASE + HEADER_SIZE
        || defaults < headers_end
        || constants < end(defaults, bitset_size)?
    {
        return Err(invalid("segment headers and track bitsets overlap"));
    }
    slice(&bytes, segment_headers, segments * 20)?;
    slice(&bytes, defaults, bitset_size)?;
    slice(&bytes, constants, bitset_size)?;
    let structural_end = end(constants, bitset_size)?;
    for pointer in [constant_data, clip_ranges].into_iter().flatten() {
        if pointer < structural_end {
            return Err(invalid("track samples overlap headers or bitsets"));
        }
    }
    if !track_count.is_multiple_of(32) {
        let padding = (1u32 << (32 - track_count % 32)) - 1;
        for pointer in [defaults, constants] {
            if u32_at(&bytes, pointer + bitset_size - 4)? & padding != 0 {
                return Err(invalid("nonzero track bitset padding"));
            }
        }
    }
    let mut tracks = reserve(track_count)?;
    let mut template = reserve(bones * 10)?;
    template.resize(bones * 10, 0.0);
    for bone in 0..bones {
        template[bone * 10 + 3] = 1.0;
        template[bone * 10 + 7..bone * 10 + 10].fill(f32::from(default_scale));
    }
    let mut constant_cursor = constant_data.unwrap_or(structural_end);
    let mut clip_cursor = clip_ranges.unwrap_or(structural_end);
    for index in 0..track_count {
        let mask = 1u32 << (31 - index % 32);
        let is_default = u32_at(&bytes, defaults + index / 32 * 4)? & mask != 0;
        let is_constant = u32_at(&bytes, constants + index / 32 * 4)? & mask != 0;
        if is_default && !is_constant {
            return Err(invalid("default track is not marked constant"));
        }
        let kind = index % kinds;
        let destination = index / kinds * 10 + [0, 4, 7][kind];
        if is_default {
            continue;
        }
        if is_constant {
            required(constant_data, "constant track has no sample region")?;
            let xyz = floats3(&bytes, constant_cursor)?;
            constant_cursor = end(constant_cursor, 12)?;
            write(&mut template[destination..], kind, xyz)?;
        } else {
            if clip_flags != 7 {
                return Err(Error::unsupported(
                    "legacy MHY variable tracks require verified clip range normalization for all channels",
                ));
            }
            required(clip_ranges, "animated track has no clip range region")?;
            let min = floats3(&bytes, clip_cursor)?;
            let extent = floats3(&bytes, clip_cursor + 12)?;
            if extent.iter().any(|&value| value < 0.0) {
                return Err(invalid("negative clip range extent"));
            }
            clip_cursor = end(clip_cursor, 24)?;
            tracks.push(Track {
                destination,
                kind,
                min,
                extent,
            });
        }
    }
    if clip_ranges.is_some_and(|pointer| constant_cursor > pointer) && constant_data.is_some() {
        return Err(invalid("constant samples overlap clip ranges"));
    }
    let mut previous_end = structural_end.max(constant_cursor).max(clip_cursor);
    let mut values = reserve(
        usize::try_from(values_count).map_err(|_| invalid("decoded values do not fit platform"))?,
    )?;
    for _ in 0..frames {
        values.extend_from_slice(&template);
    }
    let row_size = bones * 10;
    let mut first_frame = 0usize;
    for segment in 0..segments {
        let header = segment_headers + segment * 20;
        let sample_count = u32_at(&bytes, header)? as usize;
        let last_frame = end(first_frame, sample_count)?;
        if sample_count == 0 || last_frame > frames {
            return Err(invalid("segment sample counts exceed clip samples"));
        }
        let pose_bits = u32_at(&bytes, header + 4)? as usize;
        let formats = offset32(&bytes, header + 8)?;
        let segment_ranges = offset32(&bytes, header + 12)?;
        let animated = offset32(&bytes, header + 16)?;
        let formats_size = tracks.len();
        let format_start = if formats_size != 0 {
            required(formats, "animated tracks have no bit rates")?
        } else {
            formats.unwrap_or(previous_end)
        };
        if format_start < previous_end {
            return Err(invalid("segment bit rates overlap preceding track data"));
        }
        let format_bytes = slice(&bytes, format_start, formats_size)?;
        let range_count = tracks
            .iter()
            .filter(|track| segment_flags & (1 << track.kind) != 0)
            .count();
        let range_size = range_count * 6;
        let format_end = end(format_start, formats_size)?;
        let range_start = if range_size != 0 {
            required(segment_ranges, "segment normalization has no range region")?
        } else {
            segment_ranges.unwrap_or(format_end)
        };
        if range_start < format_end {
            return Err(invalid("segment ranges overlap bit rates"));
        }
        let range_bytes = slice(&bytes, range_start, range_size)?;
        let range_end = end(range_start, range_size)?;
        let animated_size = pose_bits
            .checked_mul(sample_count)
            .ok_or_else(|| invalid("segment animated size overflowed"))?
            .div_ceil(8);
        let animated_start = if animated_size != 0 {
            required(animated, "animated segment has no sample region")?
        } else {
            animated.unwrap_or(range_end)
        };
        if animated_start < range_end {
            return Err(invalid("segment animated samples overlap metadata"));
        }
        let stream = slice(&bytes, animated_start, animated_size)?;
        previous_end = end(animated_start, animated_size)?;
        let mut encoded = reserve(tracks.len())?;
        let mut bit_offset = 0usize;
        let mut range_offset = 0usize;
        for (track, &rate) in tracks.iter().zip(format_bytes) {
            let bits = *VARIABLE_BITS
                .get(usize::from(rate))
                .ok_or_else(|| invalid("invalid variable bit-rate code"))?;
            let normalized = segment_flags & (1 << track.kind) != 0;
            if bits == 0 && !normalized {
                return Err(invalid("zero-bit track has no segment constant sample"));
            }
            let mut sample = Encoded {
                bits,
                bit_offset,
                min: [0.0; 3],
                extent: [1.0; 3],
            };
            if normalized {
                let range = &range_bytes[range_offset..range_offset + 6];
                for c in 0..3 {
                    if bits == 0 {
                        sample.min[c] = u16::from_le_bytes([range[c * 2], range[c * 2 + 1]]) as f32
                            * (1.0 / 65535.0);
                        sample.extent[c] = 0.0;
                    } else {
                        sample.min[c] = f32::from(range[c]) * (1.0 / 255.0);
                        sample.extent[c] = f32::from(range[c + 3]) * (1.0 / 255.0);
                    }
                }
                range_offset += 6;
            }
            bit_offset = end(bit_offset, usize::from(bits) * 3)?;
            encoded.push(sample);
        }
        if bit_offset != pose_bits {
            return Err(invalid("bit-rate sum disagrees with segment pose bits"));
        }
        for frame in first_frame..last_frame {
            let row = &mut values[frame * row_size..(frame + 1) * row_size];
            for (track, sample) in tracks.iter().zip(&encoded) {
                let mut xyz = [0.0; 3];
                for (c, value) in xyz.iter_mut().enumerate() {
                    let mut decoded = if sample.bits == 0 {
                        sample.min[c]
                    } else {
                        let offset = (frame - first_frame) * pose_bits
                            + sample.bit_offset
                            + c * usize::from(sample.bits);
                        let raw = bits_at(stream, offset, sample.bits)?;
                        if sample.bits == 32 {
                            f32::from_bits(raw)
                        } else {
                            raw as f32 * (1.0 / ((1u32 << sample.bits) - 1) as f32)
                        }
                    };
                    if sample.bits != 32 {
                        if sample.bits != 0 {
                            decoded = decoded * sample.extent[c] + sample.min[c];
                        }
                        decoded = decoded * track.extent[c] + track.min[c];
                    }
                    *value = decoded;
                }
                write(&mut row[track.destination..], track.kind, xyz)?;
            }
        }
        first_frame = last_frame;
    }
    if first_frame != frames || values.iter().any(|value| !value.is_finite()) {
        return Err(invalid("decoded clip has missing or non-finite samples"));
    }
    Ok(AclLegacyTracks {
        bone_count,
        frame_count,
        sample_rate,
        has_scale,
        values,
    })
}

struct Track {
    destination: usize,
    kind: usize,
    min: [f32; 3],
    extent: [f32; 3],
}
struct Encoded {
    bits: u8,
    bit_offset: usize,
    min: [f32; 3],
    extent: [f32; 3],
}
fn invalid(text: &str) -> Error {
    Error::invalid_data(format!("legacy ACL: {text}"))
}
fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| invalid("cannot allocate bounded decoder data"))?;
    Ok(values)
}
fn end(start: usize, length: usize) -> Result<usize> {
    start
        .checked_add(length)
        .ok_or_else(|| invalid("section offset overflowed"))
}
fn slice(bytes: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    bytes
        .get(start..end(start, length)?)
        .ok_or_else(|| invalid("section exceeds bounded compressed clip"))
}
fn u16_at(bytes: &[u8], start: usize) -> Result<u16> {
    let value = slice(bytes, start, 2)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}
fn u32_at(bytes: &[u8], start: usize) -> Result<u32> {
    let value = slice(bytes, start, 4)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}
fn required(value: Option<usize>, message: &str) -> Result<usize> {
    value.ok_or_else(|| invalid(message))
}
fn pointer(bytes: &[u8], value: u32, missing: u32) -> Result<Option<usize>> {
    if value == missing {
        return Ok(None);
    }
    let offset = end(BASE, value as usize)?;
    if offset < BASE + HEADER_SIZE {
        return Err(invalid("relative data offset overlaps header"));
    }
    slice(bytes, offset, 0)?;
    Ok(Some(offset))
}
fn offset16(bytes: &[u8], field: usize) -> Result<Option<usize>> {
    pointer(bytes, u32::from(u16_at(bytes, field)?), u32::from(u16::MAX))
}
fn offset32(bytes: &[u8], field: usize) -> Result<Option<usize>> {
    pointer(bytes, u32_at(bytes, field)?, u32::MAX)
}
fn floats3(bytes: &[u8], start: usize) -> Result<[f32; 3]> {
    let mut values = [0.0; 3];
    for (c, value) in values.iter_mut().enumerate() {
        *value = f32::from_bits(u32_at(bytes, start + c * 4)?);
        if !value.is_finite() {
            return Err(invalid("constant or range contains non-finite floats"));
        }
    }
    Ok(values)
}
fn bits_at(bytes: &[u8], bit: usize, width: u8) -> Result<u32> {
    let finish = end(bit, usize::from(width))?;
    if finish.div_ceil(8) > bytes.len() {
        return Err(invalid("animated sample exceeds bitstream"));
    }
    let mut value = 0;
    for offset in bit..finish {
        value = (value << 1) | u32::from((bytes[offset / 8] >> (7 - offset % 8)) & 1);
    }
    Ok(value)
}
fn write(output: &mut [f32], kind: usize, xyz: [f32; 3]) -> Result<()> {
    output[..3].copy_from_slice(&xyz);
    if kind == 0 {
        let [x, y, z] = xyz;
        let w = (((1.0 - x * x) - y * y) - z * z).abs().sqrt();
        let length = (x * x + y * y + z * z + w * w).sqrt();
        if !length.is_finite() || length <= 0.0 {
            return Err(invalid("invalid quaternion norm"));
        }
        for (to, from) in output[..4].iter_mut().zip([x, y, z, w]) {
            *to = from / length;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    macro_rules! fixture {
        ($name:literal) => {
            (
                $name,
                include_bytes!(concat!("../tests/fixtures/acl-legacy/", $name, ".acl")).as_slice(),
                include_str!(concat!("../tests/fixtures/acl-legacy/", $name, ".cpp.csv")),
                include_str!(concat!("../tests/fixtures/acl-legacy/", $name, ".dll.csv")),
            )
        };
    }
    #[test]
    fn legacy_samples_match_real_upstream_and_original_mhy_dll_oracles() {
        for (name, bytes, cpp, dll) in [
            fixture!("animated_variable"),
            fixture!("multisegment_variable"),
            fixture!("constant_variable"),
            fixture!("default_variable"),
            fixture!("mixed_variable"),
            fixture!("multibone_variable"),
            fixture!("bitset_boundary_variable"),
            fixture!("no_scale_variable"),
            fixture!("segment_constant_variable"),
            fixture!("raw_escape_variable"),
        ] {
            let tracks = decode_legacy_compressed_clip(
                &Region::from_bytes(bytes.to_vec()),
                AclNativeDecodeLimits::default(),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            let expected: Vec<f32> = cpp
                .lines()
                .flat_map(|line| {
                    line.split(',')
                        .skip(1)
                        .map(|value| value.parse::<f32>().unwrap())
                })
                .collect();
            assert_eq!(tracks.values.len(), expected.len());
            for (index, (actual, expected)) in tracks.values.iter().zip(expected).enumerate() {
                assert!(
                    (actual - expected).abs() <= 2.0e-6 * expected.abs().max(1.0),
                    "{name} sample {index}: {actual} != {expected}"
                );
            }
            let mut dll = dll.lines();
            let counts: Vec<usize> = dll
                .next()
                .unwrap()
                .split(',')
                .map(|s| s.parse().unwrap())
                .collect();
            assert_eq!(counts, [tracks.values.len(), tracks.frame_count as usize]);
            let times: Vec<f32> = dll
                .next()
                .unwrap()
                .split(',')
                .map(|s| s.parse().unwrap())
                .collect();
            assert_eq!(
                times,
                tracks.times().collect::<Vec<_>>(),
                "{name} sample times"
            );
            let dll_values: Vec<f32> = dll
                .next()
                .unwrap()
                .split(',')
                .map(|s| s.parse().unwrap())
                .collect();
            assert_eq!(dll_values.len(), tracks.values.len());
            for (index, (actual, expected)) in tracks.values.iter().zip(dll_values).enumerate() {
                assert!(
                    (actual - expected).abs() <= 2.0e-6 * expected.abs().max(1.0),
                    "{name} DLL sample {index}: {actual} != {expected}"
                );
            }
        }
    }

    fn rehash(bytes: &mut [u8]) {
        if bytes.len() < 8 {
            return;
        }
        let size = bytes.len() as u32;
        bytes[..4].copy_from_slice(&size.to_le_bytes());
        let hash = bytes[8..].iter().fold(2166136261u32, |hash, &byte| {
            (hash ^ u32::from(byte)).wrapping_mul(16777619)
        });
        bytes[4..8].copy_from_slice(&hash.to_le_bytes());
    }
    fn change(bytes: &[u8], offset: usize, value: u32) -> Vec<u8> {
        let mut output = bytes.to_vec();
        output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        rehash(&mut output);
        output
    }
    fn change_short(bytes: &[u8], offset: usize, value: u16) -> Vec<u8> {
        let mut output = bytes.to_vec();
        output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        rehash(&mut output);
        output
    }
    fn fails(bytes: Vec<u8>) {
        let result = std::panic::catch_unwind(|| {
            decode_legacy_compressed_clip(
                &Region::from_bytes(bytes),
                AclNativeDecodeLimits::default(),
            )
        });
        assert!(
            result
                .expect("malformed legacy ACL must not panic")
                .is_err()
        );
    }

    #[test]
    fn legacy_header_sections_rates_counts_ranges_and_budgets_are_checked() {
        let single = include_bytes!("../tests/fixtures/acl-legacy/animated_variable.acl");
        let multi = include_bytes!("../tests/fixtures/acl-legacy/multisegment_variable.acl");
        for field in [BASE, BASE + 2] {
            fails(change_short(single, field, 0));
        }
        for field in [BASE + 12, BASE + 16] {
            fails(change(single, field, 0));
        }
        fails(change(single, BASE + 12, u32::MAX));
        for field in [BASE + 20, BASE + 22, BASE + 24, BASE + 28] {
            for value in [0, u16::MAX] {
                fails(change_short(single, field, value));
            }
        }
        for field in [BASE + 7, BASE + 8, BASE + 9, BASE + 10] {
            let mut bytes = single.to_vec();
            bytes[field] = 255;
            rehash(&mut bytes);
            fails(bytes);
        }
        fails(change_short(single, 12, 4)); // ACL1.2 uses a different sample-rate kind.
        fails(change(single, 8, 0xAC11_AC11)); // ACL2 is a distinct container.
        let header = BASE + usize::from(u16_at(single, BASE + 20).unwrap());
        for (field, value) in [
            (header, 0),
            (header, 12),
            (header + 4, u32::MAX),
            (header + 8, 0),
            (header + 8, u32::MAX),
            (header + 16, 0),
            (header + 16, u32::MAX),
        ] {
            fails(change(single, field, value));
        }
        let header_multi = BASE + usize::from(u16_at(multi, BASE + 20).unwrap());
        fails(change(multi, header_multi + 12, u32::MAX));
        let rates = BASE + u32_at(single, header + 8).unwrap() as usize;
        for bad in [0, 19, 255] {
            let mut bytes = single.to_vec();
            bytes[rates] = bad;
            rehash(&mut bytes);
            fails(bytes);
        }
        let ranges = BASE + usize::from(u16_at(single, BASE + 28).unwrap());
        fails(change(single, ranges, f32::NAN.to_bits()));
        fails(change(single, ranges + 12, (-1.0f32).to_bits()));
        let defaults = BASE + usize::from(u16_at(single, BASE + 22).unwrap());
        fails(change(single, defaults, 0x80000000)); // A default bit must also be constant.
        fails(change(single, defaults, 1)); // Out-of-range bitset padding.
        let constant = include_bytes!("../tests/fixtures/acl-legacy/constant_variable.acl");
        fails(change_short(constant, BASE + 26, u16::MAX));
        let values = BASE + usize::from(u16_at(constant, BASE + 26).unwrap());
        fails(change(constant, values, f32::INFINITY.to_bits()));
        for raw in [
            include_bytes!("../tests/fixtures/acl-legacy/animated_raw.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl-legacy/animated_drop.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl-legacy/constant_raw.acl").as_slice(),
        ] {
            fails(raw.to_vec());
        }
        let region = Region::from_bytes(single.to_vec());
        for compressed in [
            super::super::AclCompressedTracksLimits {
                maximum_compressed_bytes: single.len() as u64 - 1,
                ..super::super::AclCompressedTracksLimits::default()
            },
            super::super::AclCompressedTracksLimits {
                maximum_tracks: 0,
                ..super::super::AclCompressedTracksLimits::default()
            },
            super::super::AclCompressedTracksLimits {
                maximum_samples_per_track: 10,
                ..super::super::AclCompressedTracksLimits::default()
            },
            super::super::AclCompressedTracksLimits {
                maximum_decompressed_values: 109,
                ..super::super::AclCompressedTracksLimits::default()
            },
        ] {
            assert!(
                decode_legacy_compressed_clip(
                    &region,
                    AclNativeDecodeLimits {
                        compressed,
                        ..AclNativeDecodeLimits::default()
                    }
                )
                .is_err()
            );
        }
        let exact = single.len() as u64 + 110 * 4 + 512;
        assert!(
            decode_legacy_compressed_clip(
                &region,
                AclNativeDecodeLimits {
                    maximum_materialized_bytes: exact,
                    ..AclNativeDecodeLimits::default()
                }
            )
            .is_ok()
        );
        assert!(
            decode_legacy_compressed_clip(
                &region,
                AclNativeDecodeLimits {
                    maximum_materialized_bytes: exact - 1,
                    ..AclNativeDecodeLimits::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn repaired_hash_legacy_truncations_and_payload_attacks_remain_bounded() {
        for bytes in [
            include_bytes!("../tests/fixtures/acl-legacy/animated_variable.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl-legacy/multisegment_variable.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl-legacy/multibone_variable.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl-legacy/default_variable.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl-legacy/segment_constant_variable.acl").as_slice(),
        ] {
            let reference = decode_legacy_compressed_clip(
                &Region::from_bytes(bytes.to_vec()),
                AclNativeDecodeLimits::default(),
            )
            .unwrap();
            for length in 0..bytes.len() {
                let mut prefix = bytes[..length].to_vec();
                rehash(&mut prefix);
                let result = std::panic::catch_unwind(|| {
                    decode_legacy_compressed_clip(
                        &Region::from_bytes(prefix),
                        AclNativeDecodeLimits::default(),
                    )
                })
                .expect("legacy prefix must not panic");
                if let Ok(decoded) = result {
                    assert_eq!(
                        decoded.values, reference.values,
                        "only unused padding may be trimmed"
                    );
                }
            }
            let mut seed = 0xC0F13A59u32;
            let compressed = super::super::AclCompressedTracksLimits {
                maximum_tracks: 64,
                maximum_samples_per_track: 1024,
                maximum_decompressed_values: 65536,
                ..super::super::AclCompressedTracksLimits::default()
            };
            for _ in 0..4096 {
                let mut corrupted = bytes.to_vec();
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let offset = 8 + seed as usize % (bytes.len() - 12);
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                corrupted[offset..offset + 4].copy_from_slice(&seed.to_le_bytes());
                rehash(&mut corrupted);
                let result = std::panic::catch_unwind(|| {
                    decode_legacy_compressed_clip(
                        &Region::from_bytes(corrupted),
                        AclNativeDecodeLimits {
                            compressed,
                            maximum_materialized_bytes: 1024 * 1024,
                        },
                    )
                })
                .expect("legacy payload mutations must not panic");
                if let Ok(decoded) = result {
                    assert_eq!(
                        decoded.values.len(),
                        usize::from(decoded.bone_count) * decoded.frame_count as usize * 10
                    );
                    assert!(decoded.values.iter().all(|value| value.is_finite()));
                }
            }
        }
    }
}
