//! Safe sample decoding of the public ACL binary format.
//!
//! Layout/bitstream contracts are derived from Nicholas Frechette and ACL
//! contributors' MIT ACL 2.1.0, commit 414689d5cff4286a7898487a46dc5e48005d38da:
//! core/impl/compressed_headers.h, compression/impl/write_* and decompression/impl.
//! The development-only upstream compressor/decoder oracle is in tests/fixtures/acl.
//! This module does not infer any Unity-fork decoder-map or bind-pose contract.
use super::{
    AclCompressedTracks, AclCompressedTracksLimits, AclTrackType, inspect_compressed_tracks,
};
use crate::source::Region;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AclNativeDecodeLimits {
    pub compressed: AclCompressedTracksLimits,
    /// Combined compressed input, decoded f32 samples, and decoder working data.
    pub maximum_materialized_bytes: u64,
}

impl Default for AclNativeDecodeLimits {
    fn default() -> Self {
        Self {
            compressed: AclCompressedTracksLimits::default(),
            maximum_materialized_bytes: 512 * 1024 * 1024,
        }
    }
}

/// Stored ACL samples, ordered frame, track, then component.
/// QVV components are quaternion XYZW, translation XYZ, scale XYZ.
/// A wrap-optimized clip omits its repeated first sample at the loop endpoint;
/// this API returns stored frames and retains that fact in `header`.
#[derive(Debug, Clone, PartialEq)]
pub struct AclNativeTracks {
    pub header: AclCompressedTracks,
    pub values: Vec<f32>,
}

/// Decodes actual stored samples under a shared allocation budget. Unsupported
/// format versions and external-database/default-pose contracts return errors.
pub fn decode_compressed_tracks(
    region: &Region,
    limits: AclNativeDecodeLimits,
) -> Result<AclNativeTracks> {
    let header = inspect_compressed_tracks(region, limits.compressed)?;
    if !matches!(header.version, 7 | 10) {
        return Err(Error::unsupported(
            "ACL native decoder does not support development format versions 8/9",
        ));
    }
    let values_len = usize::try_from(header.decompressed_value_count)
        .map_err(|_| invalid("sample count does not fit this platform"))?;
    let output_bytes = header
        .decompressed_value_count
        .checked_mul(4)
        .ok_or_else(|| invalid("decoded byte size overflowed"))?;
    let work_bytes = u64::from(header.num_tracks)
        .checked_mul(512)
        .ok_or_else(|| invalid("working byte size overflowed"))?;
    let combined = u64::from(header.declared_size)
        .checked_add(output_bytes)
        .and_then(|n| n.checked_add(work_bytes))
        .ok_or_else(|| invalid("allocation budget overflowed"))?;
    if combined > limits.maximum_materialized_bytes {
        return Err(invalid(
            "compressed input, decoded samples, and working data exceed materialization budget",
        ));
    }
    usize::try_from(combined)
        .map_err(|_| invalid("materialized decoder buffers do not fit this platform"))?;
    let bytes = region.read_to_vec(limits.compressed.maximum_compressed_bytes)?;
    let mut values = reserve(values_len)?;
    if header.track_type == AclTrackType::Qvv {
        transforms(&bytes, &header, &mut values)?;
    } else {
        scalars(&bytes, &header, &mut values)?;
    }
    if values.len() != values_len || values.iter().any(|value| !value.is_finite()) {
        return Err(invalid(
            "decoded samples have an invalid count or non-finite value",
        ));
    }
    Ok(AclNativeTracks { header, values })
}

fn invalid(message: &str) -> Error {
    Error::invalid_data(format!("ACL native decoder: {message}"))
}
fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| invalid("cannot allocate bounded decoder buffer"))?;
    Ok(values)
}
fn section(bytes: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| invalid("section offset overflowed"))?;
    bytes
        .get(start..end)
        .ok_or_else(|| invalid("section exceeds bounded compressed payload"))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let data = section(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}
fn float_at(bytes: &[u8], offset: usize) -> Result<f32> {
    let value = f32::from_bits(u32_at(bytes, offset)?);
    if !value.is_finite() {
        return Err(invalid("constant/range data contains non-finite values"));
    }
    Ok(value)
}
fn offset_at(bytes: &[u8], base: usize, field: usize, minimum: usize) -> Result<usize> {
    let relative = usize::try_from(u32_at(bytes, base + field)?)
        .map_err(|_| invalid("offset does not fit platform"))?;
    if relative < minimum {
        return Err(invalid("section overlaps its header"));
    }
    let absolute = base
        .checked_add(relative)
        .ok_or_else(|| invalid("relative section offset overflowed"))?;
    section(bytes, absolute, 0)?;
    Ok(absolute)
}
fn bit_value(bytes: &[u8], bit: usize, width: u8) -> Result<u32> {
    if width > 32 {
        return Err(invalid("invalid component bit width"));
    }
    let end = bit
        .checked_add(usize::from(width))
        .ok_or_else(|| invalid("bit offset overflowed"))?;
    if end.div_ceil(8) > bytes.len() {
        return Err(invalid("animated sample exceeds bounded bitstream"));
    }
    let mut value = 0;
    for pos in bit..end {
        value = (value << 1) | u32::from((bytes[pos / 8] >> (7 - pos % 8)) & 1);
    }
    Ok(value)
}
fn bits_to_float(value: u32, width: u8) -> f32 {
    if width == 32 {
        f32::from_bits(value)
    } else {
        value as f32 * (1.0 / ((1u32 << width) - 1) as f32)
    }
}
fn scalar_width(kind: AclTrackType) -> usize {
    match kind {
        AclTrackType::Float1 => 1,
        AclTrackType::Float2 => 2,
        AclTrackType::Float3 => 3,
        _ => 4,
    }
}

struct Scalar {
    bits: u8,
    offset: usize,
    constant: [f32; 4],
    min: [f32; 4],
    extent: [f32; 4],
}

fn scalars(bytes: &[u8], header: &AclCompressedTracks, out: &mut Vec<f32>) -> Result<()> {
    const BASE: usize = 32;
    section(bytes, BASE, 20)?;
    if header.misc_packed & 0x3FFF_FFFF != 0 {
        return Err(invalid("scalar header has reserved flags"));
    }
    let frame_bits =
        usize::try_from(u32_at(bytes, BASE)?).map_err(|_| invalid("frame bit size overflowed"))?;
    let metadata = offset_at(bytes, BASE, 4, 20)?;
    let constants = offset_at(bytes, BASE, 8, 20)?;
    let ranges = offset_at(bytes, BASE, 12, 20)?;
    let animated = offset_at(bytes, BASE, 16, 20)?;
    if !(metadata <= constants && constants <= ranges && ranges <= animated) {
        return Err(invalid("scalar data sections overlap or are out of order"));
    }
    let count =
        usize::try_from(header.num_tracks).map_err(|_| invalid("track count overflowed"))?;
    let frames = usize::try_from(header.num_samples_per_track)
        .map_err(|_| invalid("frame count overflowed"))?;
    if metadata.checked_add(count).is_none_or(|n| n > constants) {
        return Err(invalid("scalar metadata overlaps constants"));
    }
    let rates = section(bytes, metadata, count)?;
    let mut tracks = reserve(count)?;
    let width = scalar_width(header.track_type);
    let mut cp = constants;
    let mut rp = ranges;
    let mut sum = 0usize;
    for &rate in rates {
        let bits = if header.version == 7 {
            *[
                0, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 32,
            ]
            .get(usize::from(rate))
            .ok_or_else(|| invalid("invalid ACL 2.0 scalar bit rate"))?
        } else {
            match rate {
                0..=23 => rate,
                24 => 32,
                _ => return Err(invalid("invalid ACL 2.1 scalar bit rate")),
            }
        };
        let mut track = Scalar {
            bits,
            offset: sum,
            constant: [0.0; 4],
            min: [0.0; 4],
            extent: [0.0; 4],
        };
        if bits == 0 {
            for value in &mut track.constant[..width] {
                *value = float_at(bytes, cp)?;
                cp += 4;
            }
        } else {
            sum = sum
                .checked_add(usize::from(bits) * width)
                .ok_or_else(|| invalid("scalar frame size overflowed"))?;
            if bits != 32 {
                for value in &mut track.min[..width] {
                    *value = float_at(bytes, rp)?;
                    rp += 4;
                }
                for value in &mut track.extent[..width] {
                    *value = float_at(bytes, rp)?;
                    rp += 4;
                    if *value < 0.0 {
                        return Err(invalid("negative scalar range extent"));
                    }
                }
            }
        }
        tracks.push(track);
    }
    if cp > ranges || rp > animated || sum != frame_bits {
        return Err(invalid("scalar byte counts disagree with headers"));
    }
    let total_bits = frame_bits
        .checked_mul(frames)
        .ok_or_else(|| invalid("scalar animated size overflowed"))?;
    let stream = section(bytes, animated, total_bits.div_ceil(8))?;
    for frame in 0..frames {
        for track in &tracks {
            for c in 0..width {
                let value = if track.bits == 0 {
                    track.constant[c]
                } else {
                    let bits = bit_value(
                        stream,
                        frame * frame_bits + track.offset + c * usize::from(track.bits),
                        track.bits,
                    )?;
                    let sample = bits_to_float(bits, track.bits);
                    if track.bits == 32 {
                        sample
                    } else {
                        sample.mul_add(track.extent[c], track.min[c])
                    }
                };
                out.push(value);
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct Transform {
    bone: usize,
    kind: usize,
    state: u8,
    min: [f32; 3],
    extent: [f32; 3],
}
struct SegmentSample {
    track: usize,
    bits: u8,
    bit_offset: usize,
    min: [f32; 3],
    extent: [f32; 3],
}

fn quaternion(xyz: [f32; 3]) -> Result<[f32; 4]> {
    let [x, y, z] = xyz;
    let w = (((1.0 - x * x) - y * y) - z * z).abs().sqrt();
    normalize([x, y, z, w])
}
fn normalize(mut q: [f32; 4]) -> Result<[f32; 4]> {
    let length = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !length.is_finite() || length <= 0.0 {
        return Err(invalid("invalid quaternion norm"));
    }
    for value in &mut q {
        *value /= length;
    }
    Ok(q)
}
fn destination(bone: usize, kind: usize) -> usize {
    bone * 10 + [0, 4, 7][kind]
}
fn align(offset: usize, alignment: usize) -> Result<usize> {
    offset
        .checked_add(alignment - 1)
        .map(|n| n & !(alignment - 1))
        .ok_or_else(|| invalid("alignment overflowed"))
}
fn write_transform(row: &mut [f32], track: &Transform, value: [f32; 4]) {
    let start = destination(track.bone, track.kind);
    let count = if track.kind == 0 { 4 } else { 3 };
    row[start..start + count].copy_from_slice(&value[..count]);
}

fn transforms(bytes: &[u8], header: &AclCompressedTracks, out: &mut Vec<f32>) -> Result<()> {
    const BASE: usize = 32;
    section(bytes, BASE, 52)?;
    if header.version != 10 {
        return Err(Error::unsupported(
            "ACL native QVV decoding currently requires release format version 10 (ACL 2.1)",
        ));
    }
    if header.has_database()
        || header.has_stripped_keyframes()
        || header.misc_packed & (1 << 9) == 0
    {
        return Err(Error::unsupported(
            "ACL native QVV decoding requires resident unstripped tracks with trivial default transforms",
        ));
    }
    if header.misc_packed & 0x3FFF_F800 != 0 {
        return Err(invalid("transform header has reserved flags"));
    }
    let rotation = (header.misc_packed >> 4) & 15;
    if !matches!(rotation, 0 | 2 | 3) {
        return Err(invalid("unknown rotation representation"));
    }
    let variable = [
        rotation == 3,
        header.misc_packed & 8 != 0,
        header.misc_packed & 4 != 0,
    ];
    let has_scale = header.misc_packed & 1 != 0;
    let kinds = if has_scale { 3 } else { 2 };
    let bones = header.num_tracks as usize;
    let frames = header.num_samples_per_track as usize;
    let segments = u32_at(bytes, BASE)? as usize;
    if segments == 0 || segments > frames.max(1) {
        return Err(invalid("invalid transform segment count"));
    }
    if bones == 0 && frames != 0 {
        return Err(invalid("empty transform track list declares samples"));
    }
    let variable_count = u32_at(bytes, BASE + 4)? as usize;
    let animated_counts = [
        u32_at(bytes, BASE + 8)? as usize,
        u32_at(bytes, BASE + 12)? as usize,
        u32_at(bytes, BASE + 16)? as usize,
    ];
    let constant_counts = [
        u32_at(bytes, BASE + 20)? as usize,
        u32_at(bytes, BASE + 24)? as usize,
        u32_at(bytes, BASE + 28)? as usize,
    ];
    if u32_at(bytes, BASE + 32)? != u32::MAX {
        return Err(invalid(
            "resident transform tracks have a database header offset",
        ));
    }
    let headers = offset_at(bytes, BASE, 36, 52)?;
    let types = offset_at(bytes, BASE, 40, 52)?;
    let constants = offset_at(bytes, BASE, 44, 52)?;
    let ranges = offset_at(bytes, BASE, 48, 52)?;
    let index_bytes = if segments > 1 {
        (segments + 1)
            .checked_mul(4)
            .ok_or_else(|| invalid("segment start index size overflowed"))?
    } else {
        0
    };
    let headers_end = headers
        .checked_add(
            segments
                .checked_mul(16)
                .ok_or_else(|| invalid("segment headers size overflowed"))?,
        )
        .ok_or_else(|| invalid("segment header offset overflowed"))?;
    let words_per_kind = bones.div_ceil(16);
    let type_bytes = words_per_kind
        .checked_mul(4 * kinds)
        .ok_or_else(|| invalid("sub-track types size overflowed"))?;
    if headers < BASE + 52 + index_bytes
        || types < headers_end
        || constants < types + type_bytes
        || ranges < constants
    {
        return Err(invalid(
            "transform structural sections overlap or are out of order",
        ));
    }
    section(bytes, headers, segments * 16)?;
    section(bytes, types, type_bytes)?;
    let mut tracks = reserve(bones * kinds)?;
    let mut template = reserve(bones * 10)?;
    template.resize(bones * 10, 0.0);
    let scale = if header.misc_packed & 2 != 0 {
        1.0
    } else {
        0.0
    };
    for bone in 0..bones {
        template[bone * 10 + 3] = 1.0;
        template[bone * 10 + 7..bone * 10 + 10].fill(scale);
    }
    let mut real_animated = [0usize; 3];
    let mut real_constant = [0usize; 3];
    for kind in 0..kinds {
        for bone in 0..bones {
            let packed = u32_at(bytes, types + (kind * words_per_kind + bone / 16) * 4)?;
            let state = ((packed >> ((15 - bone % 16) * 2)) & 3) as u8;
            match state {
                0 => {}
                1 => real_constant[kind] += 1,
                2 => real_animated[kind] += 1,
                _ => return Err(invalid("reserved sub-track type")),
            };
            tracks.push(Transform {
                bone,
                kind,
                state,
                min: [0.0; 3],
                extent: [0.0; 3],
            });
        }
        if !bones.is_multiple_of(16) && words_per_kind != 0 {
            let last = u32_at(bytes, types + ((kind + 1) * words_per_kind - 1) * 4)?;
            let padding_bits = (16 - bones % 16) * 2;
            if last & ((1u32 << padding_bits) - 1) != 0 {
                return Err(invalid("nonzero packed sub-track padding"));
            }
        }
    }
    if animated_counts != real_animated || constant_counts != real_constant {
        return Err(invalid(
            "sub-track type counts disagree with transform header",
        ));
    }
    let expected_variable = if variable[0] {
        align(real_animated[0], 4)?
    } else {
        0
    } + if variable[1] { real_animated[1] } else { 0 }
        + if variable[2] { real_animated[2] } else { 0 };
    if variable_count != expected_variable {
        return Err(invalid(
            "padded variable sub-track count disagrees with formats",
        ));
    }

    let mut selected = reserve(bones)?;
    let mut cp = constants;
    for kind in 0..kinds {
        selected.clear();
        selected.extend(
            tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.kind == kind && t.state == 1)
                .map(|(i, _)| i),
        );
        for group in selected.chunks(4) {
            if kind == 0 && rotation != 0 {
                for (lane, &index) in group.iter().enumerate() {
                    let xyz = [
                        float_at(bytes, cp + lane * 4)?,
                        float_at(bytes, cp + (group.len() + lane) * 4)?,
                        float_at(bytes, cp + (2 * group.len() + lane) * 4)?,
                    ];
                    write_transform(&mut template, &tracks[index], quaternion(xyz)?);
                }
                cp += group.len() * 12;
            } else {
                for &index in group {
                    let components = if kind == 0 { 4 } else { 3 };
                    let mut value = [0.0; 4];
                    for component in &mut value[..components] {
                        *component = float_at(bytes, cp)?;
                        cp += 4;
                    }
                    write_transform(&mut template, &tracks[index], value);
                }
            }
        }
    }
    if cp > ranges {
        return Err(invalid("constant transform samples overlap clip ranges"));
    }
    let mut rp = ranges;
    for (kind, &is_variable) in variable.iter().enumerate().take(kinds) {
        if !is_variable {
            continue;
        }
        selected.clear();
        selected.extend(
            tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.kind == kind && t.state == 2)
                .map(|(i, _)| i),
        );
        for group in selected.chunks(4) {
            for (lane, &index) in group.iter().enumerate() {
                for c in 0..3 {
                    let (min_offset, extent_offset) = if kind == 0 {
                        (
                            rp + (c * group.len() + lane) * 4,
                            rp + ((c + 3) * group.len() + lane) * 4,
                        )
                    } else {
                        (rp + (lane * 6 + c) * 4, rp + (lane * 6 + c + 3) * 4)
                    };
                    tracks[index].min[c] = float_at(bytes, min_offset)?;
                    tracks[index].extent[c] = float_at(bytes, extent_offset)?;
                    if tracks[index].extent[c] < 0.0 {
                        return Err(invalid("negative transform clip range extent"));
                    }
                }
            }
            rp += group.len() * 24;
        }
    }
    for _ in 0..frames {
        out.extend_from_slice(&template);
    }
    let row_size = bones * 10;
    let mut previous_data_end = rp;
    for segment in 0..segments {
        let start = if segments == 1 {
            0
        } else {
            u32_at(bytes, BASE + 52 + segment * 4)? as usize
        };
        let end = if segment + 1 == segments {
            frames
        } else {
            u32_at(bytes, BASE + 52 + (segment + 1) * 4)? as usize
        };
        if (segment == 0 && start != 0) || start >= end || end > frames {
            return Err(invalid("invalid transform segment sample interval"));
        }
        if segments > 1
            && segment + 1 == segments
            && u32_at(bytes, BASE + 52 + segments * 4)? != u32::MAX
        {
            return Err(invalid("missing segment sample sentinel"));
        }
        let sh = headers + segment * 16;
        let pose_bits = u32_at(bytes, sh)? as usize;
        let rotation_bits = u32_at(bytes, sh + 4)? as usize;
        let translation_bits = u32_at(bytes, sh + 8)? as usize;
        let data = offset_at(bytes, BASE, sh + 12 - BASE, 52)?;
        if data < previous_data_end {
            return Err(invalid("transform segment overlaps preceding data"));
        }
        let formats = section(bytes, data, variable_count)?;
        let segment_ranges = align(data + variable_count, 2)?;
        let range_size = if segments > 1 {
            variable_count
                .checked_mul(6)
                .ok_or_else(|| invalid("segment range size overflowed"))?
        } else {
            0
        };
        let sr = section(bytes, segment_ranges, range_size)?;
        let animated = align(segment_ranges + range_size, 4)?;
        let bits_size = pose_bits
            .checked_mul(end - start)
            .ok_or_else(|| invalid("transform animated size overflowed"))?;
        let stream = section(bytes, animated, bits_size.div_ceil(8))?;
        previous_data_end = animated + stream.len();
        let mut samples = reserve(real_animated.iter().sum())?;
        let mut format_pos = 0usize;
        let mut range_pos = 0usize;
        let mut offsets = [0usize; 3];
        let mut bit_offset = 0usize;
        for kind in 0..kinds {
            selected.clear();
            selected.extend(
                tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.kind == kind && t.state == 2)
                    .map(|(i, _)| i),
            );
            for group in selected.chunks(4) {
                for (lane, &index) in group.iter().enumerate() {
                    let bits = if variable[kind] {
                        let value = formats[format_pos + lane];
                        match value {
                            0..=23 => value,
                            31 => 32,
                            _ => return Err(invalid("invalid variable transform bit width")),
                        }
                    } else {
                        32
                    };
                    if bits == 0 && segments == 1 {
                        return Err(invalid(
                            "constant-bit transform lacks its segment range sample",
                        ));
                    }
                    let mut sample = SegmentSample {
                        track: index,
                        bits,
                        bit_offset,
                        min: [0.0; 3],
                        extent: [1.0; 3],
                    };
                    if variable[kind] && segments > 1 && bits != 32 {
                        for c in 0..3 {
                            if bits == 0 {
                                let (hi, lo) = if kind == 0 {
                                    (
                                        sr[range_pos + c * 8 + lane],
                                        sr[range_pos + c * 8 + 4 + lane],
                                    )
                                } else {
                                    (
                                        sr[range_pos + lane * 6 + c * 2 + 1],
                                        sr[range_pos + lane * 6 + c * 2],
                                    )
                                };
                                sample.min[c] =
                                    u16::from_be_bytes([hi, lo]) as f32 * (1.0 / 65535.0);
                                sample.extent[c] = 0.0;
                            } else {
                                let (min, extent) = if kind == 0 {
                                    (
                                        sr[range_pos + c * 4 + lane],
                                        sr[range_pos + (c + 3) * 4 + lane],
                                    )
                                } else {
                                    (
                                        sr[range_pos + lane * 6 + c],
                                        sr[range_pos + lane * 6 + c + 3],
                                    )
                                };
                                sample.min[c] = f32::from(min) * (1.0 / 255.0);
                                sample.extent[c] = f32::from(extent) * (1.0 / 255.0);
                            }
                        }
                    }
                    let components = if kind == 0 && rotation == 0 { 4 } else { 3 };
                    let size = usize::from(bits) * components;
                    bit_offset = bit_offset
                        .checked_add(size)
                        .ok_or_else(|| invalid("transform pose size overflowed"))?;
                    offsets[kind] += size;
                    samples.push(sample);
                }
                if variable[kind] {
                    let padded = if kind == 0 { 4 } else { group.len() };
                    if kind == 0
                        && group.len() != 4
                        && formats[format_pos + group.len()..format_pos + 4]
                            .iter()
                            .any(|&v| v != 0)
                    {
                        return Err(invalid("nonzero variable rotation metadata padding"));
                    }
                    format_pos += padded;
                    if segments > 1 {
                        range_pos += padded * 6;
                    }
                }
            }
        }
        if format_pos != variable_count
            || bit_offset != pose_bits
            || offsets[0] != rotation_bits
            || offsets[1] != translation_bits
        {
            return Err(invalid(
                "transform sample bit counts disagree with segment header",
            ));
        }
        for frame in start..end {
            let row = &mut out[frame * row_size..(frame + 1) * row_size];
            for sample in &samples {
                let track = &tracks[sample.track];
                let components = if track.kind == 0 && rotation == 0 {
                    4
                } else {
                    3
                };
                let mut value = [0.0; 4];
                for (c, component) in value.iter_mut().enumerate().take(components) {
                    let mut v = if sample.bits == 0 {
                        sample.min[c]
                    } else {
                        let bit = (frame - start) * pose_bits
                            + sample.bit_offset
                            + c * usize::from(sample.bits);
                        bits_to_float(bit_value(stream, bit, sample.bits)?, sample.bits)
                    };
                    if variable[track.kind] && sample.bits != 32 {
                        if sample.bits != 0 && segments > 1 {
                            v = v * sample.extent[c] + sample.min[c];
                        }
                        v = v * track.extent[c] + track.min[c];
                    }
                    *component = v;
                }
                if track.kind == 0 {
                    value = if rotation == 0 {
                        normalize(value)?
                    } else {
                        quaternion([value[0], value[1], value[2]])?
                    };
                }
                write_transform(row, track, value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qvv_samples_match_independent_upstream_compressor_and_decoder() {
        for (name, bytes, csv) in [
            (
                "qvv_raw",
                include_bytes!("../tests/fixtures/acl/qvv_raw.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_raw.csv"),
            ),
            (
                "qvv_variable",
                include_bytes!("../tests/fixtures/acl/qvv_variable.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_variable.csv"),
            ),
            (
                "qvv_single_segment",
                include_bytes!("../tests/fixtures/acl/qvv_single_segment.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_single_segment.csv"),
            ),
            (
                "qvv_segment_constants",
                include_bytes!("../tests/fixtures/acl/qvv_segment_constants.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_segment_constants.csv"),
            ),
            (
                "qvv_variable_raw",
                include_bytes!("../tests/fixtures/acl/qvv_variable_raw.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_variable_raw.csv"),
            ),
            (
                "qvv_mixed_constants",
                include_bytes!("../tests/fixtures/acl/qvv_mixed_constants.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_mixed_constants.csv"),
            ),
            (
                "qvv_wrap",
                include_bytes!("../tests/fixtures/acl/qvv_wrap.acl").as_slice(),
                include_str!("../tests/fixtures/acl/qvv_wrap.csv"),
            ),
        ] {
            check(name, bytes, csv);
        }
    }
    #[test]
    fn scalar_samples_match_independent_upstream_compressor_and_decoder() {
        for (name, bytes, csv) in [
            (
                "scalar1",
                include_bytes!("../tests/fixtures/acl/scalar1.acl").as_slice(),
                include_str!("../tests/fixtures/acl/scalar1.csv"),
            ),
            (
                "scalar2",
                include_bytes!("../tests/fixtures/acl/scalar2.acl").as_slice(),
                include_str!("../tests/fixtures/acl/scalar2.csv"),
            ),
            (
                "scalar3",
                include_bytes!("../tests/fixtures/acl/scalar3.acl").as_slice(),
                include_str!("../tests/fixtures/acl/scalar3.csv"),
            ),
            (
                "scalar4",
                include_bytes!("../tests/fixtures/acl/scalar4.acl").as_slice(),
                include_str!("../tests/fixtures/acl/scalar4.csv"),
            ),
            (
                "vector4",
                include_bytes!("../tests/fixtures/acl/vector4.acl").as_slice(),
                include_str!("../tests/fixtures/acl/vector4.csv"),
            ),
        ] {
            check(name, bytes, csv);
        }
    }

    fn rehash(bytes: &mut [u8]) {
        if bytes.len() < 8 {
            return;
        }
        let length = bytes.len() as u32;
        bytes[..4].copy_from_slice(&length.to_le_bytes());
        let hash = bytes[8..].iter().fold(2166136261u32, |hash, &byte| {
            (hash ^ u32::from(byte)).wrapping_mul(16777619)
        });
        bytes[4..8].copy_from_slice(&hash.to_le_bytes());
    }
    fn change(bytes: &[u8], offset: usize, value: u32) -> Vec<u8> {
        let mut bytes = bytes.to_vec();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        rehash(&mut bytes);
        bytes
    }
    fn fails(bytes: Vec<u8>) {
        let result = std::panic::catch_unwind(|| {
            decode_compressed_tracks(&Region::from_bytes(bytes), AclNativeDecodeLimits::default())
        });
        assert!(result.expect("untrusted ACL bytes must not panic").is_err());
    }

    #[test]
    fn native_decoder_checks_sections_counts_flags_finite_values_and_shared_budget() {
        let scalar = include_bytes!("../tests/fixtures/acl/scalar4.acl");
        for offset in [32, 36, 40, 44, 48] {
            fails(change(scalar, offset, u32::MAX));
        }
        for offset in [36, 40, 44, 48] {
            fails(change(scalar, offset, 0));
        }
        let scalar_metadata = 32 + u32_at(scalar, 36).unwrap() as usize;
        let mut bytes = scalar.to_vec();
        bytes[scalar_metadata] = 255;
        rehash(&mut bytes);
        fails(bytes);
        let scalar_constants = 32 + u32_at(scalar, 40).unwrap() as usize;
        fails(change(scalar, scalar_constants, f32::NAN.to_bits()));
        let scalar_ranges = 32 + u32_at(scalar, 44).unwrap() as usize;
        if scalar_ranges < 32 + u32_at(scalar, 48).unwrap() as usize {
            fails(change(scalar, scalar_ranges, f32::INFINITY.to_bits()));
        }
        let qvv = include_bytes!("../tests/fixtures/acl/qvv_variable.acl");
        for offset in [32, 36, 40, 44, 48, 52, 56, 60, 68, 72, 76, 80] {
            fails(change(qvv, offset, u32::MAX));
        }
        fails(change(qvv, 32, 0));
        fails(change(qvv, 64, 0));
        let flags = u32_at(qvv, 28).unwrap();
        for invalid_flags in [
            flags | 256,
            flags | 1024,
            flags & !512,
            flags | 2048,
            (flags & !240) | 16,
        ] {
            fails(change(qvv, 28, invalid_flags));
        }
        let sh = 32 + u32_at(qvv, 68).unwrap() as usize;
        for offset in [sh, sh + 4, sh + 8, sh + 12] {
            fails(change(qvv, offset, u32::MAX));
        }
        let types = 32 + u32_at(qvv, 72).unwrap() as usize;
        fails(change(qvv, types, u32::MAX));
        let clip_ranges = 32 + u32_at(qvv, 80).unwrap() as usize;
        fails(change(qvv, clip_ranges, f32::NAN.to_bits()));
        let formats = 32 + u32_at(qvv, sh + 12).unwrap() as usize;
        let mut bytes = qvv.to_vec();
        bytes[formats] = 255;
        rehash(&mut bytes);
        fails(bytes);
        fails(change(qvv, 84, 1)); // First segment must begin at frame zero.
        for source in [scalar.as_slice(), qvv.as_slice()] {
            let region = Region::from_bytes(source.to_vec());
            let header =
                inspect_compressed_tracks(&region, AclCompressedTracksLimits::default()).unwrap();
            let bytes = u64::from(header.declared_size)
                + header.decompressed_value_count * 4
                + u64::from(header.num_tracks) * 512;
            let limits = AclNativeDecodeLimits {
                maximum_materialized_bytes: bytes,
                ..AclNativeDecodeLimits::default()
            };
            assert!(decode_compressed_tracks(&region, limits).is_ok());
            assert!(
                decode_compressed_tracks(
                    &region,
                    AclNativeDecodeLimits {
                        maximum_materialized_bytes: bytes - 1,
                        ..limits
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn repaired_hash_truncations_and_payload_mutations_never_panic_or_invent_missing_samples() {
        for bytes in [
            include_bytes!("../tests/fixtures/acl/scalar4.acl").as_slice(),
            include_bytes!("../tests/fixtures/acl/qvv_variable.acl").as_slice(),
        ] {
            let reference = decode_compressed_tracks(
                &Region::from_bytes(bytes.to_vec()),
                AclNativeDecodeLimits::default(),
            )
            .unwrap();
            for end in 0..bytes.len() {
                let mut truncated = bytes[..end].to_vec();
                rehash(&mut truncated);
                let result = std::panic::catch_unwind(|| {
                    decode_compressed_tracks(
                        &Region::from_bytes(truncated),
                        AclNativeDecodeLimits::default(),
                    )
                })
                .expect("every truncated ACL prefix must remain bounded");
                if let Ok(result) = result {
                    assert_eq!(
                        result.values, reference.values,
                        "only unused end padding may be truncated ({end})"
                    );
                }
            }
            let mut seed = 0x73A1B4C5u32;
            for _ in 0..1024 {
                let mut mutated = bytes.to_vec();
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let offset = 12 + seed as usize % (mutated.len() - 16);
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                mutated[offset..offset + 4].copy_from_slice(&seed.to_le_bytes());
                rehash(&mut mutated);
                let limits = AclNativeDecodeLimits {
                    compressed: AclCompressedTracksLimits {
                        maximum_tracks: 1024,
                        maximum_samples_per_track: 1024,
                        maximum_decompressed_values: 65536,
                        ..AclCompressedTracksLimits::default()
                    },
                    maximum_materialized_bytes: 1024 * 1024,
                };
                let _result = std::panic::catch_unwind(|| {
                    decode_compressed_tracks(&Region::from_bytes(mutated), limits)
                })
                .expect("repaired-hash payload mutations must remain bounded");
            }
        }
    }

    fn check(name: &str, bytes: &[u8], csv: &str) {
        let decoded = decode_compressed_tracks(
            &Region::from_bytes(bytes.to_vec()),
            AclNativeDecodeLimits::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        let mut lines = csv.lines();
        let dims: Vec<f32> = lines
            .next()
            .unwrap()
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(
            (
                decoded.header.num_tracks,
                decoded.header.num_samples_per_track
            ),
            (dims[0] as u32, dims[1] as u32)
        );
        let expected: Vec<f32> = lines
            .flat_map(|line| line.split(',').map(|s| s.parse::<f32>().unwrap()))
            .collect();
        assert_eq!(decoded.values.len(), expected.len());
        for (index, (actual, expected)) in decoded.values.iter().zip(expected).enumerate() {
            assert!(
                (actual - expected).abs() <= 2.0e-6 * expected.abs().max(1.0),
                "{name} value {index}: {actual} != {expected}"
            );
        }
        assert!(
            decoded.values.windows(5).any(|pair| pair[1] != pair[4]),
            "fixture must contain animated samples"
        );
    }
}
