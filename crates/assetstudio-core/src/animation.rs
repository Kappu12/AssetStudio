//! Unity `.anim` output for explicit and packed legacy curves.
//!
//! Muscle streams need semantic expansion into editor curves. Until that
//! converter is implemented, they fail instead of producing an empty clip.
use std::io::Write;

use anyhow::{Context, Result, bail};
use unity_rs_core::animation_clip::{
    AnimationClip, AnimationClipReadLimits, AnimationCurve, CompressedAnimationCurve, Keyframe,
    Vector3, Vector4,
};

use crate::export::LimitedBytes;
use crate::{AssetKey, Session};
use unity_rs_core::serialized::ObjectReference;

const MAX_ANIMATION_BYTES: u64 = 64 * 1024 * 1024;

enum Node {
    Map(Vec<(&'static str, Node)>),
    Array(Vec<Node>),
    String(String),
    Int(i64),
    Float(f32),
}
impl Node {
    fn map<const N: usize>(values: [(&'static str, Self); N]) -> Self {
        Self::Map(Vec::from(values))
    }
    fn text(value: &str) -> Self {
        Self::String(value.to_owned())
    }
    fn flag(value: bool) -> Self {
        Self::Int(i64::from(value))
    }
    fn empty_map() -> Self {
        Self::Map(Vec::new())
    }
}

pub(crate) fn read_animation_yaml(session: &Session, key: AssetKey) -> Result<Vec<u8>> {
    let object = session.object(key)?;
    let clip = object.read_animation_clip(AnimationClipReadLimits {
        maximum_object_bytes: MAX_ANIMATION_BYTES,
        maximum_total_keyframes: 200_000,
        maximum_total_packed_bytes: 16 * 1024 * 1024,
        maximum_total_allocation_bytes: 128 * 1024 * 1024,
        ..AnimationClipReadLimits::default()
    })?;
    reject_unexpanded_streams(&clip)?;
    let version = session.studio.collection().serialized_files()[key.file_index]
        .file
        .unity_version
        .components();
    let node = clip_node(&clip, version)?;
    let mut output = LimitedBytes::new(MAX_ANIMATION_BYTES);
    output.write_all(
        b"%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!74 &7400000\nAnimationClip:\n",
    )?;
    write_node(&node, 2, &mut output)?;
    Ok(output.into_inner())
}

fn reject_unexpanded_streams(clip: &AnimationClip) -> Result<()> {
    if clip
        .streaming_info
        .as_ref()
        .is_some_and(|info| info.size != 0)
    {
        bail!("standalone .anim export does not yet expand streamed animation payloads");
    }
    if let Some(muscle) = &clip.muscle_clip {
        let data = &muscle.clip;
        if data.streamed.data.count != 0
            || data.streamed.curve_count != 0
            || data.dense.frame_count != 0
            || data.dense.curve_count != 0
            || data.dense.samples.count != 0
            || data.constant.values.count != 0
            || data
                .acl
                .as_ref()
                .is_some_and(|acl| acl.tracks.byte_length != 0)
        {
            bail!(
                "standalone .anim export does not yet expand compressed muscle/dense/streamed/ACL curves; use animated FBX for supported model tracks or raw/JSON export"
            );
        }
    }
    Ok(())
}

fn clip_node(clip: &AnimationClip, version: (u32, u32, u32)) -> Result<Node> {
    let rotation = nodes(&clip.rotation_curves, |item| {
        Ok(Node::map([
            ("curve", curve(&item.curve, version, vector4)?),
            ("path", Node::text(&item.path)),
        ]))
    })?;
    let positions = nodes(&clip.position_curves, |item| {
        Ok(Node::map([
            ("curve", curve(&item.curve, version, vector3)?),
            ("path", Node::text(&item.path)),
        ]))
    })?;
    let scales = nodes(&clip.scale_curves, |item| {
        Ok(Node::map([
            ("curve", curve(&item.curve, version, vector3)?),
            ("path", Node::text(&item.path)),
        ]))
    })?;
    let eulers = nodes(&clip.euler_curves, |item| {
        Ok(Node::map([
            ("curve", curve(&item.curve, version, vector3)?),
            ("path", Node::text(&item.path)),
        ]))
    })?;
    let floats = nodes(&clip.float_curves, |item| {
        Ok(Node::map([
            (
                "curve",
                curve(&item.curve, version, |value| Node::Float(*value))?,
            ),
            ("attribute", Node::text(&item.attribute)),
            ("path", Node::text(&item.path)),
            ("classID", Node::Int(i64::from(item.class_id))),
            ("script", pointer(item.script)?),
            ("flags", Node::Int(i64::from(item.flags.unwrap_or(0)))),
        ]))
    })?;
    let pointers = nodes(&clip.pptr_curves, |item| {
        Ok(Node::map([
            (
                "curve",
                nodes(&item.keyframes, |key| {
                    Ok(Node::map([
                        ("time", Node::Float(key.time)),
                        ("value", pointer(key.value)?),
                    ]))
                })?,
            ),
            ("attribute", Node::text(&item.attribute)),
            ("path", Node::text(&item.path)),
            ("classID", Node::Int(i64::from(item.class_id))),
            ("script", pointer(item.script)?),
            ("flags", Node::Int(i64::from(item.flags.unwrap_or(0)))),
        ]))
    })?;
    let bindings = nodes(&clip.binding_constant.generic_bindings, |item| {
        let mut values = vec![
            ("path", Node::Int(i64::from(item.path))),
            ("attribute", Node::Int(i64::from(item.attribute))),
            ("script", pointer(item.script)?),
            ("classID", Node::Int(i64::from(item.type_id))),
            ("customType", Node::Int(i64::from(item.custom_type))),
            ("isPPtrCurve", Node::Int(i64::from(item.is_pptr_curve))),
        ];
        if let Some(value) = item.is_int_curve {
            values.push(("isIntCurve", Node::Int(i64::from(value))));
        }
        if let Some(value) = item.is_serialize_reference_curve {
            values.push(("isSerializeReferenceCurve", Node::Int(i64::from(value))));
        }
        Ok(Node::Map(values))
    })?;
    let events = nodes(&clip.events, |item| {
        Ok(Node::map([
            ("time", Node::Float(item.time)),
            ("functionName", Node::text(&item.function_name)),
            ("data", Node::text(&item.data)),
            (
                "objectReferenceParameter",
                pointer(item.object_reference_parameter)?,
            ),
            ("floatParameter", Node::Float(item.float_parameter)),
            ("intParameter", Node::Int(i64::from(item.int_parameter))),
            ("messageOptions", Node::Int(i64::from(item.message_options))),
        ]))
    })?;
    let settings = clip
        .muscle_clip
        .as_ref()
        .map_or_else(Node::empty_map, |muscle| {
            Node::map([
                (
                    "serializedVersion",
                    Node::Int(if version >= (5, 6, 0) {
                        3
                    } else if version >= (4, 3, 0) {
                        2
                    } else {
                        1
                    }),
                ),
                ("m_StartTime", Node::Float(muscle.start_time)),
                ("m_StopTime", Node::Float(muscle.stop_time)),
                (
                    "m_OrientationOffsetY",
                    Node::Float(muscle.orientation_offset_y),
                ),
                ("m_Level", Node::Float(muscle.level)),
                ("m_CycleOffset", Node::Float(muscle.cycle_offset)),
                ("m_LoopTime", Node::flag(muscle.flags.loop_time())),
                ("m_LoopBlend", Node::flag(muscle.flags.loop_blend())),
                (
                    "m_LoopBlendOrientation",
                    Node::flag(muscle.flags.loop_blend_orientation()),
                ),
                (
                    "m_LoopBlendPositionY",
                    Node::flag(muscle.flags.loop_blend_position_y()),
                ),
                (
                    "m_LoopBlendPositionXZ",
                    Node::flag(muscle.flags.loop_blend_position_xz()),
                ),
                (
                    "m_KeepOriginalOrientation",
                    Node::flag(muscle.flags.keep_original_orientation()),
                ),
                (
                    "m_KeepOriginalPositionY",
                    Node::flag(muscle.flags.keep_original_position_y()),
                ),
                (
                    "m_KeepOriginalPositionXZ",
                    Node::flag(muscle.flags.keep_original_position_xz()),
                ),
                (
                    "m_HeightFromFeet",
                    Node::flag(muscle.flags.height_from_feet()),
                ),
                ("m_Mirror", Node::flag(muscle.flags.mirror())),
            ])
        });
    Ok(Node::map([
        ("m_Name", Node::text(&clip.name)),
        (
            "serializedVersion",
            Node::Int(if version.0 >= 5 {
                6
            } else if version >= (4, 3, 0) {
                4
            } else {
                3
            }),
        ),
        ("m_Legacy", Node::flag(clip.legacy)),
        ("m_Compressed", Node::flag(clip.compressed)),
        (
            "m_UseHighQualityCurve",
            Node::flag(clip.use_high_quality_curve),
        ),
        ("m_RotationCurves", rotation),
        (
            "m_CompressedRotationCurves",
            nodes(&clip.compressed_rotation_curves, packed_curve)?,
        ),
        ("m_EulerCurves", eulers),
        ("m_PositionCurves", positions),
        ("m_ScaleCurves", scales),
        ("m_FloatCurves", floats),
        ("m_PPtrCurves", pointers),
        ("m_SampleRate", Node::Float(clip.sample_rate)),
        ("m_WrapMode", Node::Int(i64::from(clip.wrap_mode))),
        (
            "m_Bounds",
            Node::map([
                ("m_Center", vector3(&clip.bounds.center)),
                ("m_Extent", vector3(&clip.bounds.extent)),
            ]),
        ),
        (
            "m_ClipBindingConstant",
            Node::map([
                ("genericBindings", bindings),
                (
                    "pptrCurveMapping",
                    nodes(&clip.binding_constant.pptr_curve_mapping, |value| {
                        pointer(*value)
                    })?,
                ),
            ]),
        ),
        ("m_AnimationClipSettings", settings),
        ("m_Events", events),
    ]))
}

fn nodes<T>(values: &[T], convert: impl Fn(&T) -> Result<Node>) -> Result<Node> {
    let mut output = Vec::new();
    output.try_reserve_exact(values.len())?;
    for value in values {
        output.push(convert(value)?);
    }
    Ok(Node::Array(output))
}

fn curve<T>(
    value: &AnimationCurve<T>,
    version: (u32, u32, u32),
    convert: impl Fn(&T) -> Node,
) -> Result<Node> {
    let keys = nodes(&value.keyframes, |key: &Keyframe<T>| {
        let mut fields = vec![
            (
                "serializedVersion",
                Node::Int(if version.0 >= 2018 {
                    3
                } else if version >= (5, 5, 0) {
                    2
                } else {
                    1
                }),
            ),
            ("time", Node::Float(key.time)),
            ("value", convert(&key.value)),
            ("inSlope", convert(&key.in_slope)),
            ("outSlope", convert(&key.out_slope)),
        ];
        if let Some(weighted) = key.weighted_mode {
            fields.push(("weightedMode", Node::Int(i64::from(weighted))));
            fields.push((
                "inWeight",
                convert(
                    key.in_weight
                        .as_ref()
                        .context("weighted curve has no incoming weight")?,
                ),
            ));
            fields.push((
                "outWeight",
                convert(
                    key.out_weight
                        .as_ref()
                        .context("weighted curve has no outgoing weight")?,
                ),
            ));
        }
        Ok(Node::Map(fields))
    })?;
    Ok(Node::map([
        ("serializedVersion", Node::Int(2)),
        ("m_Curve", keys),
        ("m_PreInfinity", Node::Int(i64::from(value.pre_infinity))),
        ("m_PostInfinity", Node::Int(i64::from(value.post_infinity))),
        (
            "m_RotationOrder",
            Node::Int(i64::from(value.rotation_order)),
        ),
    ]))
}
fn vector3(value: &Vector3) -> Node {
    Node::map([
        ("x", Node::Float(value.x)),
        ("y", Node::Float(value.y)),
        ("z", Node::Float(value.z)),
    ])
}
fn vector4(value: &Vector4) -> Node {
    Node::map([
        ("x", Node::Float(value.x)),
        ("y", Node::Float(value.y)),
        ("z", Node::Float(value.z)),
        ("w", Node::Float(value.w)),
    ])
}

fn pointer(value: ObjectReference) -> Result<Node> {
    if value.path_id != 0 {
        bail!(
            "standalone .anim export cannot remap non-null object/script references without exported Unity GUID metadata; use raw/JSON export"
        );
    }
    Ok(Node::map([("fileID", Node::Int(0))]))
}

fn packed_curve(value: &CompressedAnimationCurve) -> Result<Node> {
    if value.times.item_count != value.values.item_count {
        bail!("packed rotation time/value key counts differ");
    }
    let times = packed_bytes(
        &value.times.data,
        value.times.item_count,
        value.times.bit_size,
    )?;
    let rotations = packed_bytes(&value.values.data, value.values.item_count, 32)?;
    let slopes = packed_bytes(
        &value.slopes.data,
        value.slopes.item_count,
        value.slopes.bit_size,
    )?;
    Ok(Node::map([
        ("path", Node::text(&value.path)),
        (
            "m_Times",
            Node::map([
                ("m_NumItems", Node::Int(i64::from(value.times.item_count))),
                ("m_Data", times),
                ("m_BitSize", Node::Int(i64::from(value.times.bit_size))),
            ]),
        ),
        (
            "m_Values",
            Node::map([
                ("m_NumItems", Node::Int(i64::from(value.values.item_count))),
                ("m_Data", rotations),
            ]),
        ),
        (
            "m_Slopes",
            Node::map([
                ("m_NumItems", Node::Int(i64::from(value.slopes.item_count))),
                ("m_Range", Node::Float(value.slopes.range)),
                ("m_Start", Node::Float(value.slopes.start)),
                ("m_Data", slopes),
                ("m_BitSize", Node::Int(i64::from(value.slopes.bit_size))),
            ]),
        ),
        ("m_PreInfinity", Node::Int(i64::from(value.pre_infinity))),
        ("m_PostInfinity", Node::Int(i64::from(value.post_infinity))),
    ]))
}
fn packed_bytes(
    value: &unity_rs_core::animation_clip::ByteRegion,
    count: u32,
    bits: u8,
) -> Result<Node> {
    if bits > 32 {
        bail!("invalid packed animation bit width");
    }
    let required = u64::from(count)
        .checked_mul(u64::from(bits))
        .context("packed animation size overflow")?
        .div_ceil(8);
    if required > value.byte_length {
        bail!("truncated packed animation samples");
    }
    let bytes = value.read(16 * 1024 * 1024)?;
    let mut hex = String::new();
    hex.try_reserve_exact(
        bytes
            .len()
            .checked_mul(2)
            .context("packed hex length overflow")?,
    )?;
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    Ok(Node::String(hex))
}

fn write_node(node: &Node, indent: usize, output: &mut impl Write) -> Result<()> {
    match node {
        Node::Map(values) if !values.is_empty() => {
            for (name, value) in values {
                write!(output, "{:indent$}{name}:", "")?;
                write_child(value, indent, output)?;
            }
        }
        Node::Array(values) if !values.is_empty() => {
            for value in values {
                write!(output, "{:indent$}-", "")?;
                write_child(value, indent, output)?;
            }
        }
        _ => {
            write!(output, "{:indent$}", "")?;
            write_scalar(node, output)?;
            writeln!(output)?;
        }
    }
    Ok(())
}
fn write_child(value: &Node, indent: usize, output: &mut impl Write) -> Result<()> {
    match value {
        Node::Map(values) if !values.is_empty() => {
            writeln!(output)?;
            write_node(value, indent + 2, output)?;
        }
        Node::Array(values) if !values.is_empty() => {
            writeln!(output)?;
            write_node(value, indent + 2, output)?;
        }
        _ => {
            write!(output, " ")?;
            write_scalar(value, output)?;
            writeln!(output)?;
        }
    }
    Ok(())
}
fn write_scalar(node: &Node, output: &mut impl Write) -> Result<()> {
    match node {
        Node::Map(_) => write!(output, "{{}}")?,
        Node::Array(_) => write!(output, "[]")?,
        Node::String(value) => {
            let encoded = serde_json::to_string(value)?
                .replace('\u{0085}', "\\u0085")
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029");
            output.write_all(encoded.as_bytes())?;
        }
        Node::Int(value) => write!(output, "{value}")?,
        Node::Float(value) if value.is_nan() => write!(output, ".nan")?,
        Node::Float(value) if *value == f32::INFINITY => write!(output, ".inf")?,
        Node::Float(value) if *value == f32::NEG_INFINITY => write!(output, "-.inf")?,
        Node::Float(value) => write!(output, "{value}")?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_escaping_and_infinite_tangents_are_preserved() {
        let node = Node::map([
            ("name", Node::text("quoted:\n\"value\"")),
            ("slope", Node::Float(f32::INFINITY)),
        ]);
        let mut out = Vec::new();
        write_node(&node, 0, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("name: \"quoted:\\n\\\"value\\\"\""));
        assert!(text.contains("slope: .inf"));
        assert!(
            pointer(ObjectReference {
                file_id: 0,
                path_id: 10
            })
            .is_err()
        );
        let mut unicode = Vec::new();
        write_scalar(&Node::text("a\u{0085}b\u{2028}c\u{2029}d"), &mut unicode).unwrap();
        assert_eq!(
            String::from_utf8(unicode).unwrap(),
            "\"a\\u0085b\\u2028c\\u2029d\""
        );
    }
}
