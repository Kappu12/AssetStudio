//! Application-side model controls shared by OBJ and both FBX encodings.

use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use unity_rs_core::{
    fbx_ascii::FbxExportSettings,
    model_animation::{ModelAnimationSet, ModelVectorKeyframe},
    model_ir::{ModelIr, ModelIrLimits, ModelRendererKind},
    scene_hierarchy::SceneObjectKey,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelExportOptions {
    pub include_animations: bool,
    pub include_textures: bool,
    pub include_skins: bool,
    pub include_materials: bool,
    pub include_blend_shapes: bool,
    pub export_all_nodes: bool,
    /// Positive geometry/translation scale; normals and angular values are unchanged.
    pub scale_factor: f32,
    /// Unwrap equivalent Euler angles across the +/-180 degree boundary.
    pub euler_filter: bool,
    pub uv_enabled: [bool; 8],
    pub fbx: FbxExportSettings,
}

impl Default for ModelExportOptions {
    fn default() -> Self {
        Self {
            include_animations: true,
            include_textures: true,
            include_skins: true,
            include_materials: true,
            include_blend_shapes: true,
            export_all_nodes: true,
            scale_factor: 1.0,
            euler_filter: true,
            uv_enabled: [true; 8],
            fbx: FbxExportSettings::default(),
        }
    }
}

impl ModelExportOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.scale_factor.is_finite() && (0.000_001..=1_000_000.0).contains(&self.scale_factor),
            "model scale factor must be finite and between 0.000001 and 1000000"
        );
        self.fbx.validate()?;
        Ok(())
    }
}

pub(crate) fn apply_model_options(model: &mut ModelIr, options: &ModelExportOptions) -> Result<()> {
    options.validate()?;
    for node in &mut model.nodes {
        if let Some(transform) = &mut node.transform {
            transform.local_position.x = scaled(transform.local_position.x, options.scale_factor)?;
            transform.local_position.y = scaled(transform.local_position.y, options.scale_factor)?;
            transform.local_position.z = scaled(transform.local_position.z, options.scale_factor)?;
        }
        for renderer in &mut node.renderers {
            if !options.include_materials {
                renderer.materials.clear();
            }
            if !options.include_skins
                && matches!(renderer.kind, ModelRendererKind::SkinnedMeshRenderer { .. })
            {
                // The writers require weights for a skinned binding. Opting
                // out exports the same geometry as a static renderer instead.
                renderer.kind = ModelRendererKind::MeshRenderer { mesh_filter: None };
            }
        }
    }
    if !options.include_materials {
        model.materials.clear();
    }
    for item in &mut model.meshes {
        let mesh = &mut item.mesh;
        for vertex in &mut mesh.vertices {
            scale_vector(vertex, options.scale_factor)?;
        }
        // Unity serializes these matrices row-major (e00,e01,...): translation
        // is e03/e13/e23. Conjugating by a unit conversion changes only those
        // elements; scaling the bottom row or diagonal would corrupt skinning.
        for bind_pose in &mut mesh.bind_poses {
            for index in [3, 7, 11] {
                bind_pose[index] = scaled(bind_pose[index], options.scale_factor)?;
            }
        }
        if !options.include_skins {
            mesh.skin = None;
            mesh.bind_poses.clear();
            mesh.bone_name_hashes.clear();
            mesh.root_bone_name_hash = 0;
        }
        if !options.include_blend_shapes {
            mesh.blend_shapes = None;
        } else if let Some(shapes) = &mut mesh.blend_shapes {
            for vertex in &mut shapes.vertices {
                scale_vector(&mut vertex.vertex, options.scale_factor)?;
            }
        }
        for (attribute, &enabled) in item.channels.uvs.iter_mut().zip(&options.uv_enabled) {
            if !enabled {
                *attribute = None;
            }
        }
        if !options.uv_enabled[0] {
            mesh.uv0 = None;
        }
    }
    if !options.export_all_nodes {
        prune_unused_nodes(model, options.include_skins)?;
    }
    model.rebuild_indexes(ModelIrLimits::default().maximum_index_bytes)?;
    Ok(())
}

pub(crate) fn apply_animation_options(
    animations: &mut ModelAnimationSet,
    model: &ModelIr,
    options: &ModelExportOptions,
) -> Result<()> {
    options.validate()?;
    if !options.include_animations {
        animations.clips.clear();
        return Ok(());
    }
    for clip in &mut animations.clips {
        clip.tracks.retain(|track| model.node(track.node).is_some());
        if !options.include_blend_shapes {
            clip.blend_shapes.clear();
        } else {
            clip.blend_shapes
                .retain(|track| model.node(track.node).is_some());
        }
        for track in &mut clip.tracks {
            for key in &mut track.translations {
                scale_vector(&mut key.value, options.scale_factor)?;
            }
            if options.euler_filter {
                unwrap_rotations(&mut track.rotations)?;
            }
        }
    }
    Ok(())
}

fn scaled(value: f32, factor: f32) -> Result<f32> {
    let result = value * factor;
    ensure!(
        value.is_finite() && result.is_finite(),
        "model scale produces a non-finite coordinate"
    );
    Ok(result)
}

fn scale_vector(vector: &mut [f32; 3], factor: f32) -> Result<()> {
    for value in vector {
        *value = scaled(*value, factor)?;
    }
    Ok(())
}

fn prune_unused_nodes(model: &mut ModelIr, include_skins: bool) -> Result<()> {
    let mut retained = HashSet::new();
    retained.try_reserve(model.nodes.len())?;
    // Renderer bone references identify Transform components; model indexes
    // identify their owning GameObjects. Never interpret the two key domains
    // interchangeably, even though both use SceneObjectKey.
    let mut transforms = std::collections::HashMap::new();
    transforms.try_reserve(model.nodes.len())?;
    for node in &model.nodes {
        if let Some(transform) = node.transform {
            ensure!(
                transforms
                    .insert(transform.component, node.object)
                    .is_none(),
                "model has ambiguous Transform ownership"
            );
        }
    }
    for node in &model.nodes {
        if node
            .renderers
            .iter()
            .any(|renderer| renderer.mesh.is_some())
        {
            retain_ancestors(model, node.object, &mut retained)?;
        }
        if include_skins {
            for renderer in &node.renderers {
                if let ModelRendererKind::SkinnedMeshRenderer { bones } = &renderer.kind {
                    if let Some(mesh) = renderer.mesh.and_then(|key| model.mesh(key))
                        && let Some(weights) = &mesh.mesh.skin
                    {
                        for vertex in weights {
                            for (&weight, &index) in vertex.weights.iter().zip(&vertex.bone_indices)
                            {
                                if weight > 0.0
                                    && bones
                                        .get(index as usize)
                                        .and_then(|bone| *bone)
                                        .is_none_or(|bone| !transforms.contains_key(&bone))
                                    && mesh.mesh.bone_name_hashes.get(index as usize).is_some()
                                {
                                    // The writer can resolve missing references through
                                    // Avatar/bone path hashes. Retain that name context
                                    // rather than pruning a candidate or changing how
                                    // an ambiguous hash is diagnosed by the writer.
                                    return Ok(());
                                }
                            }
                        }
                    }
                    for bone in bones.iter().flatten() {
                        if let Some(owner) = transforms.get(bone) {
                            retain_ancestors(model, *owner, &mut retained)?;
                        }
                    }
                }
            }
        }
    }
    model.nodes.retain(|node| retained.contains(&node.object));
    model.roots.retain(|root| retained.contains(root));
    for node in &mut model.nodes {
        node.children.retain(|child| retained.contains(child));
    }
    Ok(())
}

fn retain_ancestors(
    model: &ModelIr,
    mut key: SceneObjectKey,
    retained: &mut HashSet<SceneObjectKey>,
) -> Result<()> {
    let mut pending = Vec::new();
    loop {
        let node = model
            .node(key)
            .context("model references an absent required node")?;
        if retained.contains(&key) {
            break;
        }
        ensure!(
            pending.len() < model.nodes.len(),
            "model ancestor chain contains a cycle"
        );
        pending.try_reserve(1)?;
        pending.push(key);
        match node.parent {
            Some(parent) => key = parent,
            None => break,
        }
    }
    retained.extend(pending);
    Ok(())
}

/// Only unwrap source samples within the conventional Euler range. Explicit
/// multi-turn values such as 360/720 degrees retain their intended winding.
fn unwrap_rotations(keys: &mut [ModelVectorKeyframe]) -> Result<()> {
    let mut previous_input: Option<[f32; 3]> = None;
    let mut previous_output = [0f32; 3];
    for key in keys {
        ensure!(
            key.time.is_finite() && key.value.iter().all(|value| value.is_finite()),
            "rotation curve contains a non-finite sample"
        );
        let input = key.value;
        if let Some(previous) = previous_input {
            for component in 0..3 {
                if (-180.0..=180.0).contains(&input[component])
                    && (-180.0..=180.0).contains(&previous[component])
                {
                    let mut delta = input[component] - previous[component];
                    if delta > 180.0 {
                        delta -= 360.0;
                    } else if delta < -180.0 {
                        delta += 360.0;
                    }
                    key.value[component] = previous_output[component] + delta;
                    ensure!(key.value[component].is_finite(), "Euler unroll overflow");
                }
            }
        }
        previous_input = Some(input);
        previous_output = key.value;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skinned_model() -> (ModelIr, SceneObjectKey) {
        use unity_rs_core::{
            loader::AssetLoadOptions,
            mesh::MeshBoneWeight,
            model_ir::{ModelNode, build_model_ir},
            scene_hierarchy::SceneHierarchyLimits,
            source::Region,
        };
        let collection = unity_rs_core::loader::AssetCollection::load_regions_with_options(
            [(
                "fixture".into(),
                Region::from_bytes(
                    include_bytes!("../tests/fixtures/models/triangle.assets").to_vec(),
                ),
            )],
            AssetLoadOptions::default(),
        )
        .unwrap();
        let studio = unity_rs_core::studio::Studio::from_collection(collection);
        let hierarchy = studio
            .scene_hierarchy(SceneHierarchyLimits::default())
            .unwrap();
        let mut model =
            build_model_ir(studio.collection(), &hierarchy, ModelIrLimits::default()).unwrap();
        let root = model
            .nodes
            .iter()
            .position(|node| !node.renderers.is_empty())
            .unwrap();
        let bone_key = SceneObjectKey {
            file_index: 0,
            path_id: 12000,
        };
        let bone_transform = SceneObjectKey {
            file_index: 0,
            path_id: 12100,
        };
        let mut transform = model.nodes[root].transform.unwrap();
        transform.component = bone_transform;
        transform.local_position.x = 0.0;
        transform.local_position.y = 0.0;
        transform.local_position.z = 0.0;
        let parent = model.nodes[root].object;
        model.nodes[root].children.push(bone_key);
        model.nodes.push(ModelNode {
            object: bone_key,
            name: "Bone".into(),
            export_content: true,
            parent: Some(parent),
            children: Vec::new(),
            transform: Some(transform),
            renderers: Vec::new(),
            animator: None,
        });
        model.nodes[root].renderers[0].kind = ModelRendererKind::SkinnedMeshRenderer {
            bones: vec![Some(bone_transform)],
        };
        let mesh = &mut model.meshes[0].mesh;
        mesh.skin = Some(vec![
            MeshBoneWeight {
                weights: [1.0, 0.0, 0.0, 0.0],
                bone_indices: [0; 4]
            };
            mesh.vertices.len()
        ]);
        let mut bind = [0f32; 16];
        for index in [0, 5, 10, 15] {
            bind[index] = 1.0;
        }
        bind[3] = 1.0;
        bind[7] = 2.0;
        bind[11] = 3.0;
        mesh.bind_poses = vec![bind];
        model
            .rebuild_indexes(ModelIrLimits::default().maximum_index_bytes)
            .unwrap();
        (model, bone_key)
    }

    #[test]
    fn pruning_resolves_transform_bones_and_scaling_preserves_real_bind_translations() {
        let (mut model, bone) = skinned_model();
        let options = ModelExportOptions {
            export_all_nodes: false,
            scale_factor: 2.0,
            ..Default::default()
        };
        apply_model_options(&mut model, &options).unwrap();
        assert!(model.node(bone).is_some(), "required bone owner was pruned");
        let bind = model.meshes[0].mesh.bind_poses[0];
        assert_eq!([bind[3], bind[7], bind[11]], [2.0, 4.0, 6.0]);
        assert_eq!(&bind[12..16], &[0.0, 0.0, 0.0, 1.0]);
        let mut fbx = Vec::new();
        unity_rs_core::fbx_ascii::write_model_ir_fbx_ascii(&model, &mut fbx, 1024 * 1024).unwrap();
        let text = String::from_utf8(fbx).unwrap();
        assert!(text.contains("\"Cluster\""));
        assert!(text.contains("Bone"));
    }

    #[test]
    fn disabling_skins_removes_deformers_from_a_genuinely_skinned_model() {
        let (mut model, _) = skinned_model();
        let options = ModelExportOptions {
            include_skins: false,
            export_all_nodes: false,
            ..Default::default()
        };
        apply_model_options(&mut model, &options).unwrap();
        let mut fbx = Vec::new();
        unity_rs_core::fbx_ascii::write_model_ir_fbx_ascii(&model, &mut fbx, 1024 * 1024).unwrap();
        assert!(!String::from_utf8(fbx).unwrap().contains("\"Cluster\""));
        assert_eq!(model.nodes.len(), 1);
    }

    #[test]
    fn euler_unwrap_removes_wrapping_without_erasing_explicit_turns() {
        let mut keys = [170.0, -170.0, -150.0, 720.0, 1080.0].map(|angle| ModelVectorKeyframe {
            time: 0.0,
            value: [0.0, angle, 0.0],
        });
        unwrap_rotations(&mut keys).unwrap();
        assert_eq!(
            keys.map(|key| key.value[1]),
            [170.0, 190.0, 210.0, 720.0, 1080.0]
        );
        let mut half_turns = [0.0, 180.0, 0.0, -180.0, 0.0].map(|angle| ModelVectorKeyframe {
            time: 0.0,
            value: [0.0, angle, 0.0],
        });
        unwrap_rotations(&mut half_turns).unwrap();
        assert_eq!(
            half_turns.map(|key| key.value[1]),
            [0.0, 180.0, 0.0, -180.0, 0.0]
        );
        keys[0].value[0] = f32::NAN;
        assert!(unwrap_rotations(&mut keys).is_err());
    }

    #[test]
    fn scale_rejects_overflow_and_invalid_settings_before_export() {
        assert_eq!(scaled(-2.0, 5.0).unwrap(), -10.0);
        assert!(scaled(f32::MAX, 5.0).is_err());
        for scale_factor in [0.0, -1.0, f32::NAN, f32::INFINITY, 1_000_001.0] {
            assert!(
                ModelExportOptions {
                    scale_factor,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }
}
