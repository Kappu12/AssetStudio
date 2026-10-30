//! Native attribute names from the original MIT AnimationClip YAML resolver.
use super::crc;
use crate::{AssetKey, Session};
use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::animation_clip::GenericBinding;
use unity_rs_core::material::MaterialReadLimits;
use unity_rs_core::mesh::MeshReadLimits;
use unity_rs_core::scene_hierarchy::{SceneHierarchy, SceneObjectKey};

const MUSCLES: &[&str] = &[
    "Spine Front-Back",
    "Spine Left-Right",
    "Spine Twist Left-Right",
    "Chest Front-Back",
    "Chest Left-Right",
    "Chest Twist Left-Right",
    "UpperChest Front-Back",
    "UpperChest Left-Right",
    "UpperChest Twist Left-Right",
    "Neck Nod Down-Up",
    "Neck Tilt Left-Right",
    "Neck Turn Left-Right",
    "Head Nod Down-Up",
    "Head Tilt Left-Right",
    "Head Turn Left-Right",
    "Left Eye Down-Up",
    "Left Eye In-Out",
    "Right Eye Down-Up",
    "Right Eye In-Out",
    "Jaw Close",
    "Jaw Left-Right",
    "Left Upper Leg Front-Back",
    "Left Upper Leg In-Out",
    "Left Upper Leg Twist In-Out",
    "Left Lower Leg Stretch",
    "Left Lower Leg Twist In-Out",
    "Left Foot Up-Down",
    "Left Foot Twist In-Out",
    "Left Toes Up-Down",
    "Right Upper Leg Front-Back",
    "Right Upper Leg In-Out",
    "Right Upper Leg Twist In-Out",
    "Right Lower Leg Stretch",
    "Right Lower Leg Twist In-Out",
    "Right Foot Up-Down",
    "Right Foot Twist In-Out",
    "Right Toes Up-Down",
    "Left Shoulder Down-Up",
    "Left Shoulder Front-Back",
    "Left Arm Down-Up",
    "Left Arm Front-Back",
    "Left Arm Twist In-Out",
    "Left Forearm Stretch",
    "Left Forearm Twist In-Out",
    "Left Hand Down-Up",
    "Left Hand In-Out",
    "Right Shoulder Down-Up",
    "Right Shoulder Front-Back",
    "Right Arm Down-Up",
    "Right Arm Front-Back",
    "Right Arm Twist In-Out",
    "Right Forearm Stretch",
    "Right Forearm Twist In-Out",
    "Right Hand Down-Up",
    "Right Hand In-Out",
];
const ENGINE_PROPERTIES: &[&str] = &[
    "field of view",
    "m_Active",
    "m_AffectRotationX",
    "m_AffectRotationY",
    "m_AffectRotationZ",
    "m_AffectScalingX",
    "m_AffectScalingY",
    "m_AffectScalingZ",
    "m_AffectTranslationX",
    "m_AffectTranslationY",
    "m_AffectTranslationZ",
    "m_AnchorMax.x",
    "m_AnchorMax.y",
    "m_AnchorMin.x",
    "m_AnchorMin.y",
    "m_AnchoredPosition.x",
    "m_AnchoredPosition.y",
    "m_Color.a",
    "m_Color.b",
    "m_Color.g",
    "m_Color.r",
    "m_ColorTemperature",
    "m_CookieSize",
    "m_DrawHalo",
    "m_Enabled",
    "m_FocalLength",
    "m_Intensity",
    "m_IsActive",
    "m_LocalPosition.x",
    "m_LocalPosition.y",
    "m_LocalPosition.z",
    "m_LocalScale.x",
    "m_LocalScale.y",
    "m_LocalScale.z",
    "m_Materials.",
    "m_Parameters",
    "m_Parameters.widthMultiplier",
    "m_Pivot.x",
    "m_Pivot.y",
    "m_Range",
    "m_ReceiveShadows",
    "m_RestRotation.x",
    "m_RestRotation.y",
    "m_RestRotation.z",
    "m_RestTranslation.x",
    "m_RestTranslation.y",
    "m_RestTranslation.z",
    "m_Roll",
    "m_RotationOffset.x",
    "m_RotationOffset.y",
    "m_RotationOffset.z",
    "m_ScaleAtRest.x",
    "m_ScaleAtRest.y",
    "m_ScaleAtRest.z",
    "m_ScalingOffset.x",
    "m_ScalingOffset.y",
    "m_ScalingOffset.z",
    "m_Shadows.m_Bias",
    "m_Shadows.m_NearPlane",
    "m_Shadows.m_NormalBias",
    "m_Shadows.m_Strength",
    "m_SizeDelta.x",
    "m_SizeDelta.y",
    "m_SortingOrder",
    "m_SpotAngle",
    "m_Sprite",
    "m_TranslationOffset.x",
    "m_TranslationOffset.y",
    "m_TranslationOffset.z",
    "m_Weight",
    "m_WorldUpObject",
];

pub(super) fn resolve(
    session: &Session,
    hierarchy: Option<&SceneHierarchy>,
    roots: &[SceneObjectKey],
    binding: GenericBinding,
    path: &str,
    budget: &mut u64,
) -> Result<String> {
    let attr = binding.attribute;
    match binding.custom_type {
        8 => {
            ensure!(
                binding.type_id == 95,
                "humanoid curve is not bound to Animator"
            );
            humanoid(attr)
        }
        20 => blend_shape(session, hierarchy, roots, path, attr, budget),
        21 => Ok(format!("m_Materials.Array.data[{attr}]")),
        22 => material_property(session, hierarchy, roots, path, attr, budget),
        23 if attr == 0 => Ok("m_Sprite".into()),
        31..=36 => constraint(binding.custom_type, attr),
        0 | 24 | 25 | 26 | 28 | 29 | 30 | 37 => {
            if let Some(&name) = ENGINE_PROPERTIES.iter().find(|name| {
                standard_property(binding.custom_type, binding.type_id, name)
                    && crc(name.as_bytes()) == attr
            }) {
                return Ok(name.into());
            }
            type_tree_property(session, binding, budget)
        }
        _ => bail!(
            "animation custom property type {} attribute {attr} has no verified name",
            binding.custom_type
        ),
    }
}

fn standard_property(custom: u8, class: i32, name: &str) -> bool {
    match custom {
        0 => class == 1 && name == "m_IsActive",
        24 => name == "m_Enabled",
        25 => {
            name.starts_with("m_Color.")
                || name.starts_with("m_Shadows.")
                || matches!(
                    name,
                    "m_CookieSize"
                        | "m_DrawHalo"
                        | "m_Intensity"
                        | "m_Range"
                        | "m_SpotAngle"
                        | "m_ColorTemperature"
                )
        }
        26 => matches!(name, "m_ReceiveShadows" | "m_SortingOrder"),
        28 => {
            name == "m_LocalPosition.z"
                || [
                    "m_AnchoredPosition.",
                    "m_AnchorMin.",
                    "m_AnchorMax.",
                    "m_SizeDelta.",
                    "m_Pivot.",
                ]
                .iter()
                .any(|prefix| {
                    name.strip_prefix(prefix)
                        .is_some_and(|v| matches!(v, "x" | "y"))
                })
        }
        29 | 30 => name == "m_Parameters.widthMultiplier",
        37 => matches!(name, "field of view" | "m_FocalLength"),
        _ => false,
    }
}

fn humanoid(index: u32) -> Result<String> {
    const TRANSFORMS: [&str; 7] = ["T.x", "T.y", "T.z", "Q.x", "Q.y", "Q.z", "Q.w"];
    if index < 14 {
        return Ok(format!(
            "{}{}",
            if index < 7 { "Motion" } else { "Root" },
            TRANSFORMS[(index % 7) as usize]
        ));
    }
    if index < 42 {
        let value = index - 14;
        return Ok(format!(
            "{}{}",
            ["LeftFoot", "RightFoot", "LeftHand", "RightHand"][(value / 7) as usize],
            TRANSFORMS[(value % 7) as usize]
        ));
    }
    if index < 97 {
        return Ok(MUSCLES[(index - 42) as usize].into());
    }
    if index < 137 {
        let value = index - 97;
        return Ok(format!(
            "{}.{}.{}",
            ["LeftHand", "RightHand"][(value / 20) as usize],
            ["Thumb", "Index", "Middle", "Ring", "Little"][((value % 20) / 4) as usize],
            ["1 Stretched", "Spread", "2 Stretched", "3 Stretched"][(value % 4) as usize]
        ));
    }
    const BONES: [&str; 21] = [
        "Spine",
        "Chest",
        "UpperChest",
        "Neck",
        "Head",
        "LeftUpperLeg",
        "LeftLowerLeg",
        "LeftFoot",
        "LeftToes",
        "RightUpperLeg",
        "RightLowerLeg",
        "RightFoot",
        "RightToes",
        "LeftShoulder",
        "LeftUpperArm",
        "LeftLowerArm",
        "LeftHand",
        "RightShoulder",
        "RightUpperArm",
        "RightLowerArm",
        "RightHand",
    ];
    if index < 200 {
        let value = index - 137;
        return Ok(format!(
            "{}TDOF.{}",
            BONES[(value / 3) as usize],
            ["x", "y", "z"][(value % 3) as usize]
        ));
    }
    bail!("unknown humanoid muscle binding {index}")
}

fn constraint(kind: u8, attr: u32) -> Result<String> {
    let property = (attr & 15) as usize;
    let item = attr >> 8;
    let simple: &[&str] = match kind {
        31 => &[
            "m_RestTranslation.x",
            "m_RestTranslation.y",
            "m_RestTranslation.z",
            "m_Weight",
            "m_TranslationOffset.x",
            "m_TranslationOffset.y",
            "m_TranslationOffset.z",
            "m_AffectTranslationX",
            "m_AffectTranslationY",
            "m_AffectTranslationZ",
            "m_Active",
        ],
        32 => &[
            "m_RestRotation.x",
            "m_RestRotation.y",
            "m_RestRotation.z",
            "m_Weight",
            "m_RotationOffset.x",
            "m_RotationOffset.y",
            "m_RotationOffset.z",
            "m_AffectRotationX",
            "m_AffectRotationY",
            "m_AffectRotationZ",
            "m_Active",
        ],
        33 => &[
            "m_ScaleAtRest.x",
            "m_ScaleAtRest.y",
            "m_ScaleAtRest.z",
            "m_Weight",
            "m_ScalingOffset.x",
            "m_ScalingOffset.y",
            "m_ScalingOffset.z",
            "m_AffectScalingX",
            "m_AffectScalingY",
            "m_AffectScalingZ",
            "m_Active",
        ],
        34 => &[
            "m_Weight",
            "m_AffectRotationX",
            "m_AffectRotationY",
            "m_AffectRotationZ",
            "m_Active",
            "m_WorldUpObject",
        ],
        35 => &[
            "m_Weight",
            "m_AffectTranslationX",
            "m_AffectTranslationY",
            "m_AffectTranslationZ",
            "m_AffectRotationX",
            "m_AffectRotationY",
            "m_AffectRotationZ",
            "m_Active",
        ],
        36 => &["m_Weight", "m_Active", "m_WorldUpObject"],
        _ => bail!("invalid constraint binding type"),
    };
    if let Some(name) = simple.get(property) {
        return Ok((*name).into());
    }
    let source_start = match kind {
        31..=33 => 11,
        34 => 6,
        35 => 14,
        36 => 3,
        _ => bail!("invalid constraint binding type"),
    };
    if property == source_start {
        return Ok(format!("m_Sources.Array.data[{item}].sourceTransform"));
    }
    if property == source_start + 1 {
        return Ok(format!("m_Sources.Array.data[{item}].weight"));
    }
    if kind == 35 && (8..=13).contains(&property) {
        let array = if property < 11 {
            "m_TranslationOffsets"
        } else {
            "m_RotationOffsets"
        };
        return Ok(format!(
            "{array}.Array.data[{item}].{}",
            ["x", "y", "z"][(property - 8) % 3]
        ));
    }
    if kind == 36 && property == 5 {
        return Ok("m_Roll".into());
    }
    bail!("unknown constraint binding type {kind} attribute {attr}")
}

fn targets(
    hierarchy: &SceneHierarchy,
    roots: &[SceneObjectKey],
    path: &str,
    budget: &mut u64,
) -> Result<Vec<SceneObjectKey>> {
    let mut found = Vec::new();
    found.try_reserve_exact(roots.len())?;
    'root: for &root in roots {
        charge(budget, 1)?;
        let mut node = hierarchy
            .node(root)
            .context("animation hierarchy root is missing")?;
        if !path.is_empty() {
            for name in path.split('/') {
                charge(budget, node.children.len() as u64 + name.len() as u64)?;
                let mut matches = node
                    .children
                    .iter()
                    .filter_map(|key| hierarchy.node(*key))
                    .filter(|child| child.name == name);
                node = match matches.next() {
                    Some(node) => node,
                    None => continue 'root,
                };
                ensure!(
                    matches.next().is_none(),
                    "animation property hierarchy path is ambiguous"
                );
            }
        }
        found.push(node.object);
    }
    Ok(found)
}

fn blend_shape(
    session: &Session,
    hierarchy: Option<&SceneHierarchy>,
    roots: &[SceneObjectKey],
    path: &str,
    attribute: u32,
    budget: &mut u64,
) -> Result<String> {
    let hierarchy = hierarchy.context("blend-shape animation needs its bound hierarchy")?;
    let mut result = None;
    for key in targets(hierarchy, roots, path, budget)? {
        let node = hierarchy
            .node(key)
            .context("animation property target is missing")?;
        let Some(mesh) = node
            .skinned_mesh_renderer
            .as_ref()
            .and_then(|renderer| renderer.mesh)
        else {
            continue;
        };
        let object = session.object(AssetKey {
            file_index: mesh.file_index,
            path_id: mesh.path_id,
        })?;
        charge(budget, object.byte_size())?;
        let geometry = object.read_mesh(MeshReadLimits {
            maximum_object_bytes: 64 * 1024 * 1024,
            maximum_vertices: 100_000,
            maximum_indices: 300_000,
            maximum_blend_shape_channels: 16_384,
            maximum_decoded_bytes: 64 * 1024 * 1024,
            ..MeshReadLimits::default()
        })?;
        if let Some(shapes) = geometry.mesh.blend_shapes {
            for channel in shapes.channels {
                if channel.name_hash == attribute
                    || crc(channel.name.as_bytes()) == attribute
                    || crc(format!("blendShape.{}", channel.name).as_bytes()) == attribute
                {
                    let name = format!("blendShape.{}", channel.name);
                    ensure!(
                        result.as_ref().is_none_or(|old| old == &name),
                        "blend-shape animation property hash is ambiguous"
                    );
                    result = Some(name);
                }
            }
        }
    }
    result.context("blend-shape animation property name cannot be resolved")
}

fn material_property(
    session: &Session,
    hierarchy: Option<&SceneHierarchy>,
    roots: &[SceneObjectKey],
    path: &str,
    attribute: u32,
    budget: &mut u64,
) -> Result<String> {
    let hierarchy = hierarchy.context("material animation needs its bound hierarchy")?;
    let mut result = None;
    for key in targets(hierarchy, roots, path, budget)? {
        let node = hierarchy
            .node(key)
            .context("material animation target is missing")?;
        let materials = node
            .mesh_renderer
            .as_ref()
            .map(|r| r.materials.as_slice())
            .or_else(|| {
                node.skinned_mesh_renderer
                    .as_ref()
                    .map(|r| r.materials.as_slice())
            })
            .unwrap_or(&[]);
        for material in materials.iter().flatten() {
            let object = session.object(AssetKey {
                file_index: material.file_index,
                path_id: material.path_id,
            })?;
            charge(budget, object.byte_size())?;
            let value = object.read_material(MaterialReadLimits {
                maximum_object_bytes: 16 * 1024 * 1024,
                maximum_string_bytes: 4096,
                maximum_total_string_bytes: 1024 * 1024,
                maximum_array_elements: 16_384,
            })?;
            let mut names = Vec::new();
            let props = &value.saved_properties;
            let count = props
                .texture_environments
                .len()
                .checked_mul(3)
                .and_then(|n| n.checked_add(props.floats.len()))
                .and_then(|n| n.checked_add(props.colors.len()))
                .and_then(|n| n.checked_add(props.integers.len()))
                .context("material animation property count overflow")?;
            names.try_reserve_exact(count)?;
            for texture in &props.texture_environments {
                for suffix in ["_HDR", "_ST", "_TexelSize"] {
                    names.push(format!("{}{suffix}", texture.name));
                }
            }
            for p in &props.floats {
                names.push(p.name.clone());
            }
            for p in &props.colors {
                names.push(p.name.clone());
            }
            for p in &props.integers {
                names.push(p.name.clone());
            }
            for name in names {
                if crc(name.as_bytes()) & 0x0fff_ffff == attribute & 0x0fff_ffff {
                    let resolved = if attribute & 0x8000_0000 != 0 {
                        format!("material.{name}")
                    } else {
                        format!(
                            "material.{name}.{}",
                            if attribute & 0x4000_0000 != 0 {
                                ['r', 'g', 'b', 'a']
                            } else {
                                ['x', 'y', 'z', 'w']
                            }[((attribute >> 28) & 3) as usize]
                        )
                    };
                    ensure!(
                        result.as_ref().is_none_or(|old| old == &resolved),
                        "material animation property hash is ambiguous"
                    );
                    result = Some(resolved);
                }
            }
        }
    }
    result.context("material animation property name cannot be resolved")
}

fn type_tree_property(
    session: &Session,
    binding: GenericBinding,
    budget: &mut u64,
) -> Result<String> {
    let mut result = None;
    let mut work = 0usize;
    for loaded in session.studio.collection().serialized_files() {
        for (index, object) in loaded
            .file
            .objects
            .iter()
            .enumerate()
            .filter(|(_, object)| object.class_id == binding.type_id)
        {
            let _ = object;
            let Ok(tree) = loaded.file.object_type_tree(index) else {
                continue;
            };
            let mut parts = Vec::<(u32, String)>::new();
            parts.try_reserve(32)?;
            for node in tree.nodes.iter().skip(1) {
                charge(budget, 64 + node.field_name.len() as u64)?;
                work += 1;
                ensure!(
                    work <= 100_000,
                    "animation property tree scan exceeds its work limit"
                );
                while parts.last().is_some_and(|(level, _)| *level >= node.level) {
                    parts.pop();
                }
                ensure!(
                    node.field_name.len() <= 4096 && parts.len() < 128,
                    "animation property tree exceeds its path limit"
                );
                if node.field_name == "Array"
                    || node.field_name == "data"
                    || node.field_name == "size"
                {
                    continue;
                }
                let mut path = String::new();
                for (_, part) in &parts {
                    path.push_str(part);
                    path.push('.');
                }
                path.push_str(&node.field_name);
                ensure!(
                    path.len() <= 4096,
                    "animation property path exceeds4096bytes"
                );
                if crc(path.as_bytes()) == binding.attribute {
                    ensure!(
                        result.as_ref().is_none_or(|old| old == &path),
                        "animation property hash is ambiguous"
                    );
                    result = Some(path);
                }
                parts.push((node.level, node.field_name.clone()));
            }
        }
    }
    result.context("animation property hash has no verified name; load its embedded type tree or a supported custom binding")
}

fn charge(budget: &mut u64, work: u64) -> Result<()> {
    *budget = budget
        .checked_sub(work)
        .context("animation property resolution exceeds its shared work budget")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_humanoid_names_cover_muscles_fingers_and_tdof() {
        assert_eq!(humanoid(0).unwrap(), "MotionT.x");
        assert_eq!(humanoid(13).unwrap(), "RootQ.w");
        assert_eq!(humanoid(14).unwrap(), "LeftFootT.x");
        assert_eq!(humanoid(42).unwrap(), "Spine Front-Back");
        assert_eq!(humanoid(96).unwrap(), "Right Hand In-Out");
        assert_eq!(humanoid(97).unwrap(), "LeftHand.Thumb.1 Stretched");
        assert_eq!(humanoid(136).unwrap(), "RightHand.Little.3 Stretched");
        assert_eq!(humanoid(137).unwrap(), "SpineTDOF.x");
        assert_eq!(humanoid(199).unwrap(), "RightHandTDOF.z");
        assert!(humanoid(200).is_err());
    }
    #[test]
    fn constraints_preserve_array_slots_and_channels() {
        assert_eq!(
            constraint(31, (12 << 8) | 11).unwrap(),
            "m_Sources.Array.data[12].sourceTransform"
        );
        assert_eq!(
            constraint(35, (3 << 8) | 12).unwrap(),
            "m_RotationOffsets.Array.data[3].y"
        );
        assert_eq!(constraint(36, 5).unwrap(), "m_Roll");
        assert!(constraint(31, 15).is_err());
    }
}
