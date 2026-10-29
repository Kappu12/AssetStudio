//! Repairs the pinned backend's empty legacy-root path join.
//!
//! At backend revision b3e509ed, joining Some("root") and "" produces
//! "root/", so root curves are omitted. Replay only those missing tracks;
//! the backend remains responsible for every non-root and modern curve.
//! Packed quaternion and Euler formulas follow that MIT-licensed backend
//! (Copyright 2016 Radu; 2016-2022 Perfare; 2021-2025 aelurum).
use std::collections::{HashMap, HashSet};

use crate::{AssetKey, Session};
use anyhow::{Context, Result, bail};
use unity_rs_core::{
    animation_clip::{
        AnimationClip, AnimationClipReadLimits, CompressedAnimationCurve, Keyframe, Vector3,
    },
    animation_graph::AnimationGraph,
    model_animation::{
        ModelAnimationClip, ModelAnimationSet, ModelAnimationTrack, ModelBlendShapeTrack,
        ModelScalarKeyframe, ModelVectorKeyframe,
    },
    model_ir::ModelIr,
    scene_hierarchy::SceneObjectKey,
};

const MAX_KEYS: usize = 2_000_000;

pub(crate) fn repair_legacy_roots(
    session: &Session,
    model: &ModelIr,
    graph: &AnimationGraph,
    animations: &mut ModelAnimationSet,
) -> Result<()> {
    let mut index = HashMap::new();
    index.try_reserve(animations.clips.len())?;
    let mut key_count = 0usize;
    for (position, clip) in animations.clips.iter().enumerate() {
        index.insert(clip.object, position);
        for track in &clip.tracks {
            key_count = key_count
                .checked_add(
                    track.translations.len() + track.rotations.len() + track.scalings.len(),
                )
                .context("animation key count overflow")?;
        }
        for track in &clip.blend_shapes {
            key_count = key_count
                .checked_add(track.keys.len())
                .context("animation key count overflow")?;
        }
    }
    let animator = graph.animators.iter().flat_map(|binding| {
        binding
            .bound_clips
            .iter()
            .flatten()
            .map(move |&clip| (binding.game_object, clip))
    });
    let legacy = graph.legacy_animations.iter().flat_map(|binding| {
        binding.game_object.into_iter().flat_map(move |owner| {
            binding
                .clips
                .iter()
                .flatten()
                .map(move |&clip| (owner, clip))
        })
    });
    let mut seen = HashSet::new();
    seen.try_reserve(animations.clips.len())?;
    for (owner, key) in animator.chain(legacy) {
        if model.node(owner).is_none_or(|node| !node.export_content) || !seen.insert((owner, key)) {
            continue;
        }
        let Some(&position) = index.get(&key) else {
            continue;
        };
        let clip = session
            .object(AssetKey {
                file_index: key.file_index,
                path_id: key.path_id,
            })?
            .read_animation_clip(AnimationClipReadLimits::default())?;
        if clip.legacy {
            repair_clip(
                &clip,
                owner,
                model,
                &mut animations.clips[position],
                &mut key_count,
            )?;
        }
    }
    Ok(())
}

fn repair_clip(
    source: &AnimationClip,
    owner: SceneObjectKey,
    model: &ModelIr,
    target: &mut ModelAnimationClip,
    total: &mut usize,
) -> Result<()> {
    let positions: Vec<_> = source
        .position_curves
        .iter()
        .filter(|curve| curve.path.is_empty() && !curve.curve.keyframes.is_empty())
        .collect();
    let scales: Vec<_> = source
        .scale_curves
        .iter()
        .filter(|curve| curve.path.is_empty() && !curve.curve.keyframes.is_empty())
        .collect();
    let eulers: Vec<_> = source
        .euler_curves
        .iter()
        .filter(|curve| curve.path.is_empty() && !curve.curve.keyframes.is_empty())
        .collect();
    let rotations: Vec<_> = source
        .rotation_curves
        .iter()
        .filter(|curve| curve.path.is_empty() && !curve.curve.keyframes.is_empty())
        .collect();
    let packed: Vec<_> = source
        .compressed_rotation_curves
        .iter()
        .filter(|curve| curve.path.is_empty() && curve.times.item_count != 0)
        .collect();
    if positions.len() > 1 || scales.len() > 1 || eulers.len() + rotations.len() + packed.len() > 1
    {
        bail!("multiple legacy root curves target the same property");
    }
    if !positions.is_empty()
        || !scales.is_empty()
        || !eulers.is_empty()
        || !rotations.is_empty()
        || !packed.is_empty()
    {
        let position = match target.tracks.iter().position(|track| track.node == owner) {
            Some(index) => index,
            None => {
                target.tracks.try_reserve(1)?;
                target.tracks.push(ModelAnimationTrack {
                    node: owner,
                    translations: Vec::new(),
                    rotations: Vec::new(),
                    scalings: Vec::new(),
                });
                target.tracks.len() - 1
            }
        };
        let track = &mut target.tracks[position];
        if track.translations.is_empty()
            && let Some(curve) = positions.first()
        {
            vector_keys(
                &curve.curve.keyframes,
                &mut track.translations,
                total,
                |value| [-value.x, value.y, value.z],
            )?;
        }
        if track.scalings.is_empty()
            && let Some(curve) = scales.first()
        {
            vector_keys(
                &curve.curve.keyframes,
                &mut track.scalings,
                total,
                |value| [value.x, value.y, value.z],
            )?;
        }
        if track.rotations.is_empty() {
            if let Some(curve) = eulers.first() {
                vector_keys(
                    &curve.curve.keyframes,
                    &mut track.rotations,
                    total,
                    |value| [value.x, -value.y, -value.z],
                )?;
            }
            if let Some(curve) = rotations.first() {
                charge(curve.curve.keyframes.len(), total)?;
                track.rotations.try_reserve(curve.curve.keyframes.len())?;
                for key in &curve.curve.keyframes {
                    let value =
                        quaternion_euler([key.value.x, -key.value.y, -key.value.z, key.value.w])?;
                    push_key(&mut track.rotations, key.time, value)?;
                }
            }
            if let Some(curve) = packed.first() {
                packed_keys(curve, &mut track.rotations, total)?;
            }
        }
    }
    for curve in source
        .float_curves
        .iter()
        .filter(|curve| curve.path.is_empty() && curve.class_id == 137)
    {
        let Some(channel) = curve.attribute.strip_prefix("blendShape.") else {
            continue;
        };
        let exists = model.node(owner).is_some_and(|node| {
            node.renderers
                .iter()
                .filter_map(|renderer| renderer.mesh)
                .filter_map(|key| model.mesh(key))
                .filter_map(|mesh| mesh.mesh.blend_shapes.as_ref())
                .any(|shapes| shapes.channels.iter().any(|item| item.name == channel))
        });
        if !exists {
            continue;
        }
        if target
            .blend_shapes
            .iter()
            .any(|track| track.node == owner && track.channel == channel)
        {
            continue;
        }
        charge(curve.curve.keyframes.len(), total)?;
        let mut keys = Vec::new();
        keys.try_reserve(curve.curve.keyframes.len())?;
        for key in &curve.curve.keyframes {
            if !key.time.is_finite() || !key.value.is_finite() {
                bail!("legacy root blend-shape has a non-finite sample");
            }
            keys.push(ModelScalarKeyframe {
                time: key.time,
                value: key.value,
            });
        }
        target.blend_shapes.try_reserve(1)?;
        target.blend_shapes.push(ModelBlendShapeTrack {
            node: owner,
            channel: channel.to_owned(),
            keys,
        });
    }
    Ok(())
}

fn charge(additional: usize, total: &mut usize) -> Result<()> {
    *total = total
        .checked_add(additional)
        .context("animation key count overflow")?;
    if *total > MAX_KEYS {
        bail!("animation exceeds the shared {MAX_KEYS} key limit");
    }
    Ok(())
}
fn vector_keys(
    source: &[Keyframe<Vector3>],
    target: &mut Vec<ModelVectorKeyframe>,
    total: &mut usize,
    convert: impl Fn(Vector3) -> [f32; 3],
) -> Result<()> {
    charge(source.len(), total)?;
    target.try_reserve(source.len())?;
    for key in source {
        push_key(target, key.time, convert(key.value))?;
    }
    Ok(())
}
fn push_key(target: &mut Vec<ModelVectorKeyframe>, time: f32, value: [f32; 3]) -> Result<()> {
    if !time.is_finite() || value.iter().any(|value| !value.is_finite()) {
        bail!("legacy root animation contains a non-finite sample");
    }
    if target.last().is_some_and(|last| last.time > time) {
        bail!("legacy root animation key times are not ordered");
    }
    target.push(ModelVectorKeyframe { time, value });
    Ok(())
}

fn packed_keys(
    curve: &CompressedAnimationCurve,
    target: &mut Vec<ModelVectorKeyframe>,
    total: &mut usize,
) -> Result<()> {
    if curve.times.item_count != curve.values.item_count || curve.times.bit_size > 32 {
        bail!("invalid packed legacy root curve");
    }
    let count = usize::try_from(curve.times.item_count)?;
    charge(count, total)?;
    let times_length = count
        .checked_mul(usize::from(curve.times.bit_size))
        .context("packed time size overflow")?
        .div_ceil(8);
    let values_length = count
        .checked_mul(4)
        .context("packed quaternion size overflow")?;
    let times = curve
        .times
        .data
        .region
        .subregion(0, times_length as u64)?
        .read_to_vec(64 * 1024 * 1024)?;
    let values = curve
        .values
        .data
        .region
        .subregion(0, values_length as u64)?
        .read_to_vec(64 * 1024 * 1024)?;
    let mut times = BitReader {
        bytes: &times,
        position: 0,
    };
    let mut values = BitReader {
        bytes: &values,
        position: 0,
    };
    target.try_reserve(count)?;
    let mut centiseconds = 0u32;
    for _ in 0..count {
        centiseconds = centiseconds
            .checked_add(times.read(curve.times.bit_size)?)
            .context("packed key time overflow")?;
        let flags = values.read(3)?;
        let omitted = (flags & 3) as usize;
        let reduced = (omitted + 1) % 4;
        let mut quaternion = [0f32; 4];
        let mut length = 0f32;
        for (index, slot) in quaternion.iter_mut().enumerate() {
            if index == omitted {
                continue;
            }
            let bits = if index == reduced { 9 } else { 10 };
            let value = values.read(bits)? as f32 / (0.5 * ((1u32 << bits) - 1) as f32) - 1.0;
            *slot = value;
            length += value * value;
        }
        if !length.is_finite() || length > 1.0 {
            bail!("packed root quaternion has invalid length");
        }
        quaternion[omitted] = (1.0 - length).sqrt();
        if flags & 4 != 0 {
            quaternion[omitted] = -quaternion[omitted];
        }
        let value =
            quaternion_euler([quaternion[0], -quaternion[1], -quaternion[2], quaternion[3]])?;
        push_key(target, centiseconds as f32 * 0.01, value)?;
    }
    Ok(())
}
struct BitReader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl BitReader<'_> {
    fn read(&mut self, count: u8) -> Result<u32> {
        let end = self
            .position
            .checked_add(usize::from(count))
            .context("packed bit offset overflow")?;
        if count > 32
            || end
                > self
                    .bytes
                    .len()
                    .checked_mul(8)
                    .context("packed bit size overflow")?
        {
            bail!("truncated packed root animation sample");
        }
        let mut value = 0u32;
        for bit in 0..usize::from(count) {
            let source = self.position + bit;
            value |= u32::from((self.bytes[source / 8] >> (source % 8)) & 1) << bit;
        }
        self.position = end;
        Ok(value)
    }
}

fn quaternion_euler(mut quaternion: [f32; 4]) -> Result<[f32; 3]> {
    let norm = quaternion.iter().map(|value| value * value).sum::<f32>();
    if !norm.is_finite() || norm < f32::MIN_POSITIVE {
        bail!("legacy root quaternion has zero/non-finite length");
    }
    for value in &mut quaternion {
        *value *= norm.sqrt().recip();
    }
    let [x, y, z, w] = quaternion;
    let m00 = 1.0 - 2.0 * (y * y + z * z);
    let m01 = 2.0 * (x * y - z * w);
    let m10 = 2.0 * (x * y + z * w);
    let m11 = 1.0 - 2.0 * (x * x + z * z);
    let m20 = 2.0 * (x * z - y * w);
    let m21 = 2.0 * (y * z + x * w);
    let m22 = 1.0 - 2.0 * (x * x + y * y);
    let cosine = (m00 * m00 + m10 * m10).sqrt();
    let ry = (-m20).atan2(cosine).to_degrees();
    let (rx, rz) = if cosine <= 16.0 * f32::EPSILON {
        (
            (if (-m20).is_sign_positive() {
                m01.atan2(m11)
            } else {
                (-m01).atan2(m11)
            })
            .to_degrees(),
            0.0,
        )
    } else {
        (m21.atan2(m22).to_degrees(), m10.atan2(m00).to_degrees())
    };
    Ok([rx, ry, rz])
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_rs_core::{
        animation_clip::{ByteRegion, PackedFloatVector, PackedIntVector, PackedQuaternionVector},
        source::Region,
    };
    fn bytes(data: &[u8]) -> ByteRegion {
        ByteRegion {
            region: Region::from_bytes(data.to_vec()),
            byte_length: data.len() as u64,
        }
    }
    #[test]
    fn quaternion_identity_and_quarter_turn_have_expected_euler_angles() {
        assert_eq!(
            quaternion_euler([0.0, 0.0, 0.0, 1.0]).unwrap(),
            [0.0, 0.0, 0.0]
        );
        let value = quaternion_euler([
            0.0,
            0.0,
            -std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ])
        .unwrap();
        assert!((value[2] + 90.0).abs() < 0.01);
        assert!(value[0].abs() < 0.01 && value[1].abs() < 0.01);
        assert!(quaternion_euler([0.0; 4]).is_err());
        assert!(quaternion_euler([f32::NAN, 0.0, 0.0, 1.0]).is_err());
    }
    #[test]
    fn packed_root_rotations_preserve_time_and_orientation_and_reject_truncation() {
        // Two 32-bit packed quaternions: near identity, then a Z quarter turn.
        let mut curve = CompressedAnimationCurve {
            path: String::new(),
            times: PackedIntVector {
                item_count: 2,
                data: bytes(&[0, 50]),
                bit_size: 8,
            },
            values: PackedQuaternionVector {
                item_count: 2,
                data: bytes(&[3, 8, 32, 128, 3, 8, 96, 218]),
            },
            slopes: PackedFloatVector {
                item_count: 0,
                range: 0.0,
                start: 0.0,
                data: bytes(&[]),
                bit_size: 0,
            },
            pre_infinity: 0,
            post_infinity: 0,
        };
        let mut keys = Vec::new();
        let mut total = 0;
        packed_keys(&curve, &mut keys, &mut total).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].time, 0.0);
        assert_eq!(keys[1].time, 0.5);
        assert!(keys[0].value.iter().all(|value| value.abs() < 0.5));
        assert!((keys[1].value[2] + 90.0).abs() < 0.5);
        curve.values.data = bytes(&[0; 7]);
        assert!(packed_keys(&curve, &mut Vec::new(), &mut 0).is_err());
        curve.values.item_count = 1;
        assert!(packed_keys(&curve, &mut Vec::new(), &mut 0).is_err());
    }
}
