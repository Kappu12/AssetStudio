//! Expansion of the standard streamed/dense/constant binding streams.
//!
//! Stream polynomial slope reconstruction and custom property contracts follow
//! the original MIT AssetStudio.Utility/YAML converter (Radu/Perfare/Razmoth).
//! Unlike the reference's `path_<hash>`/`typetree_<hash>` fallbacks, unresolved
//! identities return an error so a syntactically valid but inert curve is never
//! reported as a successful decoded export.
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::animation_clip::{
    AnimationClip, AnimationCurve, FloatCurve, GenericBinding, Keyframe, PPtrCurve, PPtrKeyframe,
    QuaternionCurve, Vector3, Vector3Curve, Vector4,
};
use unity_rs_core::animation_graph::{AnimationGraphLimits, build_animation_graph};
use unity_rs_core::avatar::AvatarReadLimits;
use unity_rs_core::scene_hierarchy::{SceneHierarchy, SceneHierarchyLimits, SceneObjectKey};

use crate::{AssetKey, Session};

#[path = "animation_properties.rs"]
mod properties;

const MAX_SCALAR_KEYS: usize = 200_000;
const MAX_STREAM_WORDS: usize = 2_000_000;
const MAX_BINDINGS: usize = 16_384;
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATHS: usize = 100_000;
const MAX_PATH_STORAGE: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct ScalarKey {
    time: f32,
    value: f32,
    incoming: f32,
    outgoing: f32,
}

#[derive(Clone, Copy, Debug)]
struct Span {
    start: usize,
    width: usize,
    binding: GenericBinding,
}

#[derive(Default)]
struct Samples {
    curves: Vec<Vec<ScalarKey>>,
    count: usize,
}

impl Samples {
    fn new(count: usize) -> Result<Self> {
        ensure!(
            count <= MAX_SCALAR_KEYS,
            "compressed muscle curve count exceeds its limit"
        );
        let mut curves = Vec::new();
        curves.try_reserve_exact(count)?;
        curves.resize_with(count, Vec::new);
        Ok(Self { curves, count: 0 })
    }
    fn push(&mut self, index: usize, key: ScalarKey) -> Result<()> {
        ensure!(
            key.time.is_finite() && key.time >= 0.0 && key.value.is_finite(),
            "compressed muscle curve has an invalid time/value"
        );
        ensure!(
            !key.incoming.is_nan() && !key.outgoing.is_nan(),
            "compressed muscle curve has a NaN tangent"
        );
        ensure!(
            self.count < MAX_SCALAR_KEYS,
            "expanded animation exceeds its shared key limit"
        );
        let keys = self
            .curves
            .get_mut(index)
            .context("compressed muscle sample has no matching binding")?;
        ensure!(
            keys.last().is_none_or(|last| last.time < key.time),
            "compressed muscle curve key times are duplicated or unordered"
        );
        keys.try_reserve(1)?;
        keys.push(key);
        self.count += 1;
        Ok(())
    }
}

pub(super) fn expand(session: &Session, key: AssetKey, clip: &mut AnimationClip) -> Result<()> {
    clip.validate_variant_decoder_support()?;
    ensure!(
        !clip
            .streaming_info
            .as_ref()
            .is_some_and(|info| info.size != 0),
        "standalone .anim cannot decode the clip's external ACL stream"
    );
    let Some(muscle) = &clip.muscle_clip else {
        return Ok(());
    };
    let data = &muscle.clip;
    ensure!(
        !data
            .acl
            .as_ref()
            .is_some_and(|acl| acl.tracks.byte_length != 0),
        "standalone .anim needs a verified decoder for this ACL payload"
    );
    if data.streamed.data.count == 0
        && data.streamed.curve_count == 0
        && data.dense.frame_count == 0
        && data.dense.curve_count == 0
        && data.dense.samples.count == 0
        && data.constant.values.count == 0
        && !data
            .mhy_acl
            .as_ref()
            .is_some_and(|acl| acl.tracks.byte_length != 0)
    {
        return Ok(());
    }
    let spans = spans(&clip.binding_constant.generic_bindings)?;
    let total = spans.last().map_or(0, |span| span.start + span.width);
    ensure!(
        total != 0,
        "compressed muscle curve data has no matching bindings"
    );
    let mut samples = Samples::new(total)?;
    let mut explicit_keys = 0usize;
    for count in clip
        .rotation_curves
        .iter()
        .map(|v| v.curve.keyframes.len())
        .chain(clip.position_curves.iter().map(|v| v.curve.keyframes.len()))
        .chain(clip.scale_curves.iter().map(|v| v.curve.keyframes.len()))
        .chain(clip.euler_curves.iter().map(|v| v.curve.keyframes.len()))
        .chain(clip.float_curves.iter().map(|v| v.curve.keyframes.len()))
        .chain(clip.pptr_curves.iter().map(|v| v.keyframes.len()))
        .chain(
            clip.compressed_rotation_curves
                .iter()
                .map(|v| v.times.item_count as usize),
        )
    {
        explicit_keys = explicit_keys
            .checked_add(count)
            .context("animation key count overflow")?;
    }
    ensure!(
        explicit_keys <= MAX_SCALAR_KEYS,
        "explicit animation exceeds its shared key limit"
    );
    samples.count = explicit_keys;
    let decoded_mhy = data
        .mhy_acl
        .as_ref()
        .filter(|acl| acl.tracks.byte_length != 0)
        .map(|acl| {
            acl.decode(unity_rs_core::acl::AclNativeDecodeLimits {
                compressed: unity_rs_core::acl::AclCompressedTracksLimits {
                    maximum_decompressed_values: MAX_SCALAR_KEYS as u64,
                    ..Default::default()
                },
                maximum_materialized_bytes: 64 * 1024 * 1024,
            })
        })
        .transpose()?;
    let acl_count = decoded_mhy
        .as_ref()
        .map_or(0, |acl| acl.following_curve_offset as usize);
    let continuous_count = usize::try_from(data.streamed.curve_count)?;
    let stream_count = continuous_count
        .checked_add(usize::from(data.streamed.discrete_curve_count.unwrap_or(0)))
        .context("streamed animation curve count overflow")?;
    let dense_count = usize::try_from(data.dense.curve_count)?;
    let dense_start = acl_count
        .checked_add(stream_count)
        .context("animation curve offset overflow")?;
    let constant_start = dense_start
        .checked_add(dense_count)
        .context("animation curve offset overflow")?;
    let end = constant_start
        .checked_add(data.constant.values.count)
        .context("animation curve count overflow")?;
    ensure!(
        end == total,
        "compressed muscle sample count disagrees with its bindings"
    );
    if let Some(acl) = decoded_mhy {
        for (frame, &time) in acl.times.iter().enumerate() {
            for (column, &value) in acl.values[frame * acl_count..(frame + 1) * acl_count]
                .iter()
                .enumerate()
            {
                samples.push(
                    column,
                    ScalarKey {
                        time,
                        value,
                        incoming: 0.0,
                        outgoing: 0.0,
                    },
                )?;
            }
        }
    }
    let words = data.streamed.data.read_values(MAX_STREAM_WORDS)?;
    streamed_at(
        &words,
        stream_count,
        continuous_count,
        acl_count,
        &mut samples,
    )?;
    let dense = &data.dense;
    let dense_size = dense
        .frame_count
        .checked_mul(dense_count)
        .context("dense animation sample count overflow")?;
    ensure!(
        dense_size == dense.samples.count,
        "dense animation sample count is inconsistent"
    );
    ensure!(
        dense_size <= MAX_SCALAR_KEYS,
        "dense animation exceeds its key limit"
    );
    if dense_size != 0 {
        let rate = dense.sample_rate();
        let begin = dense.begin_time();
        ensure!(
            rate.is_finite() && rate > 0.0 && begin.is_finite() && begin >= 0.0,
            "dense animation has an invalid sample rate/time"
        );
        let values = dense.samples.read_values(MAX_SCALAR_KEYS)?;
        for (frame, values) in values.chunks_exact(dense_count).enumerate() {
            let frame = u32::try_from(frame)?;
            let time = begin + frame as f32 / rate;
            for (column, &value) in values.iter().enumerate() {
                samples.push(
                    dense_start + column,
                    ScalarKey {
                        time,
                        value,
                        incoming: 0.0,
                        outgoing: 0.0,
                    },
                )?;
            }
        }
    }
    let constants = data.constant.values.read_values(MAX_SCALAR_KEYS)?;
    let end_time = muscle.stop_time;
    ensure!(
        end_time.is_finite() && end_time >= 0.0,
        "animation stop time is invalid"
    );
    for (column, &value) in constants.iter().enumerate() {
        samples.push(
            constant_start + column,
            ScalarKey {
                time: 0.0,
                value,
                incoming: 0.0,
                outgoing: 0.0,
            },
        )?;
        if end_time != 0.0 {
            samples.push(
                constant_start + column,
                ScalarKey {
                    time: end_time,
                    value,
                    incoming: 0.0,
                    outgoing: 0.0,
                },
            )?;
        }
    }
    let mut paths = Paths::build(session, key, &spans)?;
    materialize(clip, &spans, &samples, &mut paths)?;
    clip.compressed = false;
    Ok(())
}

fn spans(bindings: &[GenericBinding]) -> Result<Vec<Span>> {
    ensure!(
        bindings.len() <= MAX_BINDINGS,
        "animation has too many binding groups"
    );
    let mut output = Vec::new();
    output.try_reserve_exact(bindings.len())?;
    let mut start = 0usize;
    for &binding in bindings {
        ensure!(
            binding.is_pptr_curve <= 1
                && binding.is_int_curve.unwrap_or(0) <= 1
                && binding.is_serialize_reference_curve.unwrap_or(0) == 0,
            "animation binding has unsupported flags"
        );
        let width = if binding.type_id == 4 {
            match binding.attribute {
                1 | 3 | 4 => 3,
                2 => 4,
                _ => bail!(
                    "unknown Transform animation attribute {}",
                    binding.attribute
                ),
            }
        } else {
            1
        };
        ensure!(
            binding.type_id != 4 || binding.is_pptr_curve == 0,
            "Transform binding cannot be an object-reference curve"
        );
        output.push(Span {
            start,
            width,
            binding,
        });
        start = start
            .checked_add(width)
            .context("animation binding width overflow")?;
    }
    Ok(output)
}

#[derive(Clone, Copy)]
struct PolynomialKey {
    time: f32,
    value: f32,
    coeff: [f32; 3],
}

#[cfg(test)]
fn streamed(words: &[u32], curve_count: usize, output: &mut Samples) -> Result<()> {
    streamed_at(words, curve_count, curve_count, 0, output)
}

fn streamed_at(
    words: &[u32],
    curve_count: usize,
    continuous_count: usize,
    binding_offset: usize,
    output: &mut Samples,
) -> Result<()> {
    ensure!(
        words.len() <= MAX_STREAM_WORDS,
        "streamed animation exceeds its word limit"
    );
    if words.is_empty() {
        ensure!(
            curve_count == 0,
            "streamed animation declares curves without frames"
        );
        return Ok(());
    }
    // Unity also emits a terminal-only stream when all curves are dense or
    // constant. It carries no slope frame and declares no streamed curves.
    if curve_count == 0 && words == [f32::INFINITY.to_bits(), 0] {
        return Ok(());
    }
    let mut cursor = 0usize;
    let mut frame = 0usize;
    let mut previous_time = f32::NEG_INFINITY;
    let mut previous = HashMap::<usize, PolynomialKey>::new();
    previous.try_reserve(curve_count.min(MAX_SCALAR_KEYS))?;
    let mut seen = HashSet::new();
    seen.try_reserve(curve_count.min(MAX_SCALAR_KEYS))?;
    let mut final_frame = false;
    while cursor < words.len() {
        ensure!(
            words.len() - cursor >= 2,
            "streamed animation frame header is truncated"
        );
        let time = f32::from_bits(words[cursor]);
        let count = i32::from_ne_bytes(words[cursor + 1].to_ne_bytes());
        cursor += 2;
        let count = usize::try_from(count).context("streamed animation key count is negative")?;
        let length = count
            .checked_mul(5)
            .context("streamed frame length overflow")?;
        let end = cursor
            .checked_add(length)
            .context("streamed frame offset overflow")?;
        ensure!(end <= words.len(), "streamed animation frame is truncated");
        ensure!(
            count <= MAX_SCALAR_KEYS,
            "streamed animation frame exceeds its key limit"
        );
        if frame == 0 {
            ensure!(
                time == f32::NEG_INFINITY || time == f32::MIN,
                "streamed animation needs its initial slope sentinel"
            );
        } else {
            ensure!(
                !time.is_nan() && time > previous_time,
                "streamed animation frame times are invalid or unordered"
            );
        }
        let terminal = time == f32::INFINITY;
        ensure!(
            !terminal || end == words.len(),
            "streamed animation has frames after its terminal sentinel"
        );
        ensure!(
            terminal || frame == 0 || time >= 0.0 && time.is_finite(),
            "streamed animation sample time is invalid"
        );
        seen.clear();
        let mut last_index = None;
        while cursor < end {
            let index = usize::try_from(i32::from_ne_bytes(words[cursor].to_ne_bytes()))
                .context("streamed animation curve index is negative")?;
            ensure!(
                index < curve_count
                    && seen.insert(index)
                    && last_index.is_none_or(|last| last < index),
                "streamed animation indices are out of range, duplicated or unordered"
            );
            let coeff = [
                f32::from_bits(words[cursor + 1]),
                f32::from_bits(words[cursor + 2]),
                f32::from_bits(words[cursor + 3]),
            ];
            let value = f32::from_bits(words[cursor + 4]);
            ensure!(
                value.is_finite() && coeff.iter().all(|v| v.is_finite()),
                "streamed animation polynomial is non-finite"
            );
            let discrete = index >= continuous_count;
            ensure!(
                !discrete || coeff == [0.0; 3],
                "discrete streamed curve has interpolation coefficients"
            );
            if frame > 0 && !terminal {
                let before = previous
                    .get(&index)
                    .context("streamed animation curve has no preceding slope key")?;
                let incoming = if before.coeff == [0.0; 3] {
                    f32::INFINITY
                } else if before.time < 0.0 {
                    0.0
                } else {
                    let dx = (time - before.time).max(0.0001);
                    let dy = value - before.value;
                    {
                        let slope =
                            (3.0 * dy - 2.0 * before.coeff[2] * dx - before.coeff[1] * dx * dx)
                                / dx;
                        ensure!(slope.is_finite(), "streamed animation slope overflow");
                        slope
                    }
                };
                output.push(
                    binding_offset
                        .checked_add(index)
                        .context("streamed binding index overflow")?,
                    ScalarKey {
                        time,
                        value,
                        incoming,
                        outgoing: if discrete { f32::INFINITY } else { coeff[2] },
                    },
                )?;
            }
            if !terminal {
                previous.insert(index, PolynomialKey { time, value, coeff });
            }
            last_index = Some(index);
            cursor += 5;
        }
        previous_time = time;
        frame = frame
            .checked_add(1)
            .context("streamed frame count overflow")?;
        if terminal {
            final_frame = true;
        }
    }
    ensure!(
        final_frame && frame >= 2,
        "streamed animation is missing its terminal frame"
    );
    Ok(())
}

struct Paths<'a> {
    session: &'a Session,
    values: HashMap<u32, String>,
    hierarchy: Option<SceneHierarchy>,
    roots: Vec<SceneObjectKey>,
    property_work: usize,
    path_bytes: usize,
    property_budget: u64,
}
impl<'a> Paths<'a> {
    fn build(session: &'a Session, key: AssetKey, spans: &[Span]) -> Result<Self> {
        let mut this = Self {
            session,
            values: HashMap::new(),
            hierarchy: None,
            roots: Vec::new(),
            property_work: 0,
            path_bytes: 0,
            property_budget: 64 * 1024 * 1024,
        };
        this.values.try_reserve(MAX_BINDINGS)?;
        this.values.insert(0, String::new());
        if spans.iter().all(|span| {
            (span.binding.path == 0 || span.binding.custom_type == 8)
                && !matches!(span.binding.custom_type, 20 | 22)
        }) {
            return Ok(this);
        }
        let mut wanted = HashSet::new();
        wanted.try_reserve(spans.len())?;
        for span in spans {
            if span.binding.custom_type != 8 {
                wanted.insert(span.binding.path);
            }
        }
        let mut avatar_bytes = 0_u64;
        for asset in session.assets().iter().filter(|asset| asset.class_id == 90) {
            avatar_bytes = avatar_bytes
                .checked_add(asset.size)
                .context("animation Avatar work overflow")?;
            ensure!(
                avatar_bytes <= 64 * 1024 * 1024,
                "animation Avatar resolution exceeds its shared byte limit"
            );
            let avatar = session.object(asset.key)?.read_avatar(AvatarReadLimits {
                maximum_object_bytes: 64 * 1024 * 1024,
                maximum_string_bytes: MAX_PATH_BYTES,
                maximum_array_elements: MAX_PATHS,
                maximum_total_array_elements: MAX_PATHS * 4,
                maximum_nested_objects: MAX_PATHS,
                maximum_total_allocation_bytes: 64 * 1024 * 1024,
                maximum_total_string_bytes: MAX_PATH_STORAGE,
                ..AvatarReadLimits::default()
            })?;
            for entry in avatar.paths {
                if wanted.contains(&entry.hash) {
                    this.insert(entry.hash, &entry.path)?;
                }
            }
        }
        if wanted.iter().all(|hash| this.values.contains_key(hash))
            && !spans
                .iter()
                .any(|span| matches!(span.binding.custom_type, 20 | 22))
        {
            return Ok(this);
        }
        let hierarchy = session.studio.scene_hierarchy(SceneHierarchyLimits {
            maximum_game_objects: MAX_PATHS,
            maximum_hierarchy_edges: MAX_PATHS,
            maximum_index_bytes: 16 * 1024 * 1024,
            ..SceneHierarchyLimits::default()
        })?;
        let graph = build_animation_graph(
            session.studio.collection(),
            &hierarchy,
            AnimationGraphLimits {
                maximum_edges: MAX_PATHS,
                maximum_lookup_index_bytes: 16 * 1024 * 1024,
                ..AnimationGraphLimits::default()
            },
        )?;
        let clip = SceneObjectKey {
            file_index: key.file_index,
            path_id: key.path_id,
        };
        for animator in graph.animators {
            if animator.bound_clips.contains(&Some(clip)) {
                this.roots.push(animator.game_object);
            }
        }
        for animation in graph.legacy_animations {
            if (animation.default_clip == Some(clip) || animation.clips.contains(&Some(clip)))
                && let Some(root) = animation.game_object
                && !this.roots.contains(&root)
            {
                this.roots.push(root);
            }
        }
        let mut visited = HashSet::new();
        visited.try_reserve(hierarchy.nodes.len())?;
        let mut pending = Vec::new();
        pending.try_reserve_exact(hierarchy.nodes.len())?;
        for root in &this.roots {
            pending.push((*root, String::new()));
        }
        let mut storage = 0usize;
        let mut path_work = 0usize;
        while let Some((node, path)) = pending.pop() {
            ensure!(
                visited.insert(node),
                "animation hierarchy cycle or overlapping roots"
            );
            let current = hierarchy
                .node(node)
                .context("animation root/child is unresolved")?;
            if !path.is_empty() {
                this.insert(crc(path.as_bytes()), &path)?;
            }
            storage = storage
                .checked_add(path.len())
                .context("animation path storage overflow")?;
            ensure!(
                storage <= MAX_PATH_STORAGE && visited.len() <= MAX_PATHS,
                "animation path traversal exceeds its limit"
            );
            for &child in &current.children {
                let child = hierarchy
                    .node(child)
                    .context("animation child is unresolved")?;
                let length = path
                    .len()
                    .checked_add(child.name.len())
                    .and_then(|n| n.checked_add(usize::from(!path.is_empty())))
                    .context("animation path length overflow")?;
                ensure!(
                    length <= MAX_PATH_BYTES,
                    "animation hierarchy path exceeds 4096 bytes"
                );
                path_work = path_work
                    .checked_add(length)
                    .context("animation path work overflow")?;
                ensure!(
                    path_work <= MAX_PATH_STORAGE,
                    "animation queued paths exceed their shared byte limit"
                );
                let mut full = String::new();
                full.try_reserve_exact(length)?;
                full.push_str(&path);
                if !path.is_empty() {
                    full.push('/');
                }
                full.push_str(&child.name);
                ensure!(
                    pending.len() < MAX_PATHS,
                    "animation hierarchy pending work exceeds its limit"
                );
                pending.push((child.object, full));
            }
        }
        this.hierarchy = Some(hierarchy);
        Ok(this)
    }
    fn insert(&mut self, hash: u32, path: &str) -> Result<()> {
        ensure!(
            path.len() <= MAX_PATH_BYTES,
            "animation path exceeds4096 bytes"
        );
        if let Some(old) = self.values.get(&hash) {
            ensure!(old == path, "animation path hash {hash} is ambiguous");
            return Ok(());
        }
        ensure!(
            self.values.len() < MAX_PATHS,
            "animation has too many resolved paths"
        );
        self.path_bytes = self
            .path_bytes
            .checked_add(path.len())
            .context("animation path bytes overflow")?;
        ensure!(
            self.path_bytes <= MAX_PATH_STORAGE,
            "resolved animation paths exceed their shared byte limit"
        );
        let mut owned = String::new();
        owned.try_reserve_exact(path.len())?;
        owned.push_str(path);
        self.values.try_reserve(1)?;
        self.values.insert(hash, owned);
        Ok(())
    }
    fn path(&self, hash: u32) -> Result<&str> {
        self.values.get(&hash).map(String::as_str).with_context(|| {
            format!(
                "animation path hash {hash} cannot be resolved; load its bound hierarchy or Avatar"
            )
        })
    }
    fn attribute(&mut self, binding: GenericBinding, path: &str) -> Result<String> {
        self.property_work = self
            .property_work
            .checked_add(1)
            .context("animation property work overflow")?;
        ensure!(
            self.property_work <= MAX_BINDINGS,
            "animation property resolution exceeds its limit"
        );
        properties::resolve(
            self.session,
            self.hierarchy.as_ref(),
            &self.roots,
            binding,
            path,
            &mut self.property_budget,
        )
    }
}

fn materialize(
    clip: &mut AnimationClip,
    spans: &[Span],
    samples: &Samples,
    paths: &mut Paths<'_>,
) -> Result<()> {
    let mut generated = HashSet::new();
    generated.try_reserve(spans.len())?;
    for span in spans {
        let b = span.binding;
        let columns = &samples.curves[span.start..span.start + span.width];
        ensure!(
            columns.iter().all(|column| !column.is_empty()),
            "compressed muscle binding has no finite sample keys"
        );
        let count = columns[0].len();
        ensure!(
            columns.iter().all(|column| column.len() == count)
                && (0..count).all(|i| columns
                    .iter()
                    .all(|column| column[i].time == columns[0][i].time)),
            "Transform animation component key times do not match"
        );
        let path = if b.custom_type == 8 {
            String::new()
        } else {
            paths.path(b.path)?.to_owned()
        };
        let property = if b.type_id == 4 {
            String::new()
        } else {
            paths.attribute(b, &path)?
        };
        ensure!(
            generated.insert((path.clone(), b.type_id, b.attribute, b.custom_type)),
            "animation has duplicate property bindings"
        );
        if b.type_id == 4 {
            if b.attribute == 2 {
                ensure!(
                    !clip.rotation_curves.iter().any(|curve| curve.path == path),
                    "expanded rotation duplicates an explicit curve"
                );
                let curve = make_curve(columns, |values| Vector4 {
                    x: values[0],
                    y: values[1],
                    z: values[2],
                    w: values[3],
                })?;
                clip.rotation_curves.try_reserve(1)?;
                clip.rotation_curves.push(QuaternionCurve { curve, path });
            } else {
                let list = match b.attribute {
                    1 => &mut clip.position_curves,
                    3 => &mut clip.scale_curves,
                    4 => &mut clip.euler_curves,
                    _ => bail!("unknown Transform animation property"),
                };
                ensure!(
                    !list.iter().any(|curve| curve.path == path),
                    "expanded Transform duplicates an explicit curve"
                );
                let curve = make_curve(columns, |values| Vector3 {
                    x: values[0],
                    y: values[1],
                    z: values[2],
                })?;
                list.try_reserve(1)?;
                list.push(Vector3Curve { curve, path });
            }
        } else if b.is_pptr_curve != 0 {
            ensure!(
                !clip.pptr_curves.iter().any(|curve| curve.path == path
                    && curve.class_id == b.type_id
                    && curve.attribute == property),
                "expanded reference duplicates an explicit curve"
            );
            let mut keys = Vec::new();
            keys.try_reserve_exact(count)?;
            for sample in &columns[0] {
                ensure!(
                    sample.value.fract() == 0.0
                        && sample.value >= 0.0
                        && sample.value <= u32::MAX as f32,
                    "object-reference animation index is invalid"
                );
                let value = *clip
                    .binding_constant
                    .pptr_curve_mapping
                    .get(sample.value as usize)
                    .context("object-reference animation mapping index is out of range")?;
                keys.push(PPtrKeyframe {
                    time: sample.time,
                    value,
                });
            }
            clip.pptr_curves.try_reserve(1)?;
            clip.pptr_curves.push(PPtrCurve {
                keyframes: keys,
                attribute: property,
                path,
                class_id: b.type_id,
                script: b.script,
                flags: Some(0),
            });
        } else {
            ensure!(
                !clip.float_curves.iter().any(|curve| curve.path == path
                    && curve.class_id == b.type_id
                    && curve.attribute == property),
                "expanded scalar duplicates an explicit curve"
            );
            let curve = make_curve(columns, |values| values[0])?;
            clip.float_curves.try_reserve(1)?;
            clip.float_curves.push(FloatCurve {
                curve,
                attribute: property,
                path,
                class_id: b.type_id,
                script: b.script,
                flags: Some(0),
            });
        }
    }
    Ok(())
}

fn make_curve<T>(
    columns: &[Vec<ScalarKey>],
    make: impl Fn(&[f32; 4]) -> T,
) -> Result<AnimationCurve<T>> {
    let mut keys = Vec::new();
    keys.try_reserve_exact(columns[0].len())?;
    for index in 0..columns[0].len() {
        let mut values = [0.0; 4];
        let mut incoming = [0.0; 4];
        let mut outgoing = [0.0; 4];
        for (channel, column) in columns.iter().enumerate() {
            values[channel] = column[index].value;
            incoming[channel] = column[index].incoming;
            outgoing[channel] = column[index].outgoing;
        }
        keys.push(Keyframe {
            time: columns[0][index].time,
            value: make(&values),
            in_slope: make(&incoming),
            out_slope: make(&outgoing),
            weighted_mode: Some(0),
            in_weight: Some(make(&[1.0 / 3.0; 4])),
            out_weight: Some(make(&[1.0 / 3.0; 4])),
        });
    }
    Ok(AnimationCurve {
        keyframes: keys,
        pre_infinity: 2,
        post_infinity: 2,
        rotation_order: 4,
    })
}

pub(super) fn crc(bytes: &[u8]) -> u32 {
    let mut value = 0xffff_ffffu32;
    for &byte in bytes {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb8_8320u32 & 0u32.wrapping_sub(value & 1));
        }
    }
    !value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(words: &mut Vec<u32>, time: f32, keys: &[(i32, [f32; 4])]) {
        words.push(time.to_bits());
        words.push(keys.len() as u32);
        for &(index, coeff) in keys {
            words.push(index as u32);
            for value in coeff {
                words.push(value.to_bits());
            }
        }
    }
    #[test]
    fn streamed_polynomial_preserves_keys_and_reconstructs_exact_tangents() {
        let mut words = Vec::new();
        frame(&mut words, f32::NEG_INFINITY, &[(0, [0.0, 0.0, 0.0, 4.0])]);
        frame(&mut words, 0.0, &[(0, [1.0, 2.0, 3.0, 4.0])]);
        frame(&mut words, 1.0, &[(0, [0.0, 0.0, 10.0, 10.0])]);
        frame(&mut words, f32::INFINITY, &[]);
        let mut values = Samples::new(1).unwrap();
        streamed(&words, 1, &mut values).unwrap();
        assert_eq!(values.curves[0].len(), 2);
        assert_eq!(values.curves[0][0].value, 4.0);
        assert_eq!(values.curves[0][0].outgoing, 3.0);
        assert_eq!(values.curves[0][1].time, 1.0);
        assert_eq!(values.curves[0][1].value, 10.0);
        assert_eq!(values.curves[0][1].incoming, 10.0);
    }
    #[test]
    fn malformed_stream_frames_and_shared_key_budget_are_explicit_errors() {
        let mut valid = Vec::new();
        frame(&mut valid, f32::NEG_INFINITY, &[(0, [0.0; 4])]);
        frame(&mut valid, 0.0, &[(0, [0.0; 4])]);
        frame(&mut valid, f32::INFINITY, &[]);
        for length in 0..valid.len() {
            if length == 0 {
                continue;
            }
            assert!(
                streamed(&valid[..length], 1, &mut Samples::new(1).unwrap()).is_err(),
                "truncation {length}"
            );
        }
        let mut values = Samples::new(1).unwrap();
        values.count = MAX_SCALAR_KEYS;
        assert!(
            streamed(&valid, 1, &mut values)
                .unwrap_err()
                .to_string()
                .contains("key limit")
        );
        let mut duplicate = Vec::new();
        frame(
            &mut duplicate,
            f32::NEG_INFINITY,
            &[(0, [0.0; 4]), (0, [0.0; 4])],
        );
        frame(&mut duplicate, f32::INFINITY, &[]);
        assert!(streamed(&duplicate, 1, &mut Samples::new(1).unwrap()).is_err());
        let mut invalid = valid.clone();
        invalid[2] = (-1i32) as u32;
        assert!(streamed(&invalid, 1, &mut Samples::new(1).unwrap()).is_err());
        invalid = valid.clone();
        invalid[3] = f32::NAN.to_bits();
        assert!(streamed(&invalid, 1, &mut Samples::new(1).unwrap()).is_err());
    }
    #[test]
    fn binding_layout_preserves_transform_widths_and_refuses_unknown_shapes() {
        let base = GenericBinding {
            path: 0,
            attribute: 1,
            script: unity_rs_core::serialized::ObjectReference {
                file_id: 0,
                path_id: 0,
            },
            type_id: 4,
            custom_type: 4,
            is_pptr_curve: 0,
            is_int_curve: Some(0),
            is_serialize_reference_curve: Some(0),
        };
        let spans = spans(&[
            base,
            GenericBinding {
                attribute: 2,
                ..base
            },
            GenericBinding {
                attribute: 3,
                ..base
            },
            GenericBinding {
                type_id: 95,
                custom_type: 8,
                attribute: 42,
                ..base
            },
        ])
        .unwrap();
        assert_eq!(
            spans
                .iter()
                .map(|span| (span.start, span.width))
                .collect::<Vec<_>>(),
            [(0, 3), (3, 4), (7, 3), (10, 1)]
        );
        assert!(
            super::spans(&[GenericBinding {
                attribute: 9,
                ..base
            }])
            .is_err()
        );
        assert!(
            super::spans(&[GenericBinding {
                is_pptr_curve: 1,
                ..base
            }])
            .is_err()
        );
        assert!(
            super::spans(&[GenericBinding {
                is_serialize_reference_curve: Some(1),
                ..base
            }])
            .is_err()
        );
    }
}
