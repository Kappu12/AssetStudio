//! Static model previews retain Unity coordinates and never evaluate animation.

use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::{
    loader::AssetCollection,
    mesh::MeshReadLimits,
    model_ir::{
        ModelIr, ModelIrLimits, ModelLocalTransform, ModelNode, ModelRendererKind,
        build_model_ir_for_game_object,
    },
    renderer::{
        RendererReadLimits, StaticBatchInfo, read_mesh_renderer, read_skinned_mesh_renderer,
    },
    scene::{ObjectReference, resolve_object_reference},
    scene_hierarchy::{SceneHierarchy, SceneHierarchyLimits, SceneObjectKey},
};

use crate::{AssetKey, Preview, Session};

const MAX_VERTICES: usize = 100_000;
const MAX_INDICES: usize = 300_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: usize = 16_384;

pub(crate) fn preview_model(session: &Session, key: AssetKey) -> Result<Preview> {
    let selected = SceneObjectKey {
        file_index: key.file_index,
        path_id: key.path_id,
    };
    let class_id = session.object(key)?.class_id();
    ensure!(
        matches!(class_id, 1 | 95),
        "Selected asset is not a model root"
    );
    let mut hierarchy_limits = SceneHierarchyLimits {
        maximum_game_objects: MAX_NODES,
        maximum_total_components: MAX_NODES * 8,
        maximum_total_transform_child_references: MAX_NODES * 2,
        maximum_total_material_references: MAX_NODES * 8,
        maximum_total_bone_references: MAX_NODES * 8,
        maximum_hierarchy_edges: MAX_NODES,
        maximum_index_bytes: MAX_BYTES / 8,
        ..SceneHierarchyLimits::default()
    };
    hierarchy_limits.scene.maximum_string_bytes = 1024;
    hierarchy_limits.scene.maximum_game_object_components = 1024;
    hierarchy_limits.scene.maximum_transform_children = MAX_NODES;
    hierarchy_limits.scene.maximum_reference_bytes = (MAX_BYTES / 4) as u64;
    hierarchy_limits.renderer.maximum_object_bytes = MAX_BYTES as u64;
    hierarchy_limits.renderer.maximum_materials = 4096;
    hierarchy_limits.renderer.maximum_bones = 4096;
    hierarchy_limits.renderer.maximum_blend_shape_weights = MAX_VERTICES;
    hierarchy_limits.renderer.maximum_reference_bytes = (MAX_BYTES / 4) as u64;
    let mut hierarchy = session.studio.scene_hierarchy(hierarchy_limits)?;
    let selected = if class_id == 95 {
        hierarchy
            .nodes
            .iter()
            .find(|node| {
                node.animator
                    .as_ref()
                    .is_some_and(|animator| animator.component == selected)
            })
            .map(|node| node.object)
            .context("Selected Animator has no resolved GameObject")?
    } else {
        selected
    };
    let mesh_count = selected_mesh_count(&hierarchy, selected)?;
    ensure!(
        mesh_count != 0,
        "Selected model contains no renderable mesh"
    );

    // Flat previews need neither material textures, avatar definitions nor
    // controller data. Removing those references also prevents an unsupported
    // animation layout from hiding otherwise readable static geometry.
    for node in &mut hierarchy.nodes {
        node.animator = None;
        if let Some(renderer) = &mut node.mesh_renderer {
            renderer.materials.clear();
        }
        if let Some(renderer) = &mut node.skinned_mesh_renderer {
            renderer.materials.clear();
            renderer.bones.clear();
        }
    }
    // Share the decoded-mesh allowance across unique meshes before any mesh is
    // read. Instancing is charged again against the final vertex/index limits.
    let mesh_bytes = (MAX_BYTES / 2) / mesh_count;
    let mesh_limits = MeshReadLimits {
        maximum_object_bytes: MAX_BYTES as u64,
        maximum_vertices: MAX_VERTICES,
        maximum_indices: MAX_INDICES,
        maximum_sub_meshes: 4096,
        maximum_string_bytes: 4096,
        maximum_total_string_bytes: mesh_bytes,
        maximum_bones: 4096,
        maximum_skin_weights: MAX_VERTICES,
        maximum_blend_shape_vertices: MAX_VERTICES,
        maximum_blend_shape_frames: 4096,
        maximum_blend_shape_channels: 4096,
        maximum_array_elements: MAX_INDICES,
        maximum_vertex_data_bytes: mesh_bytes,
        maximum_compressed_data_bytes: mesh_bytes as u64,
        maximum_auxiliary_bytes: mesh_bytes as u64,
        maximum_decoded_bytes: mesh_bytes as u64,
        maximum_output_bytes: MAX_BYTES as u64,
        ..MeshReadLimits::default()
    };
    let model = build_model_ir_for_game_object(
        session.studio.collection(),
        &hierarchy,
        selected,
        ModelIrLimits {
            maximum_nodes: MAX_NODES,
            maximum_roots: 1,
            maximum_hierarchy_edges: MAX_NODES,
            maximum_renderers: MAX_NODES * 2,
            maximum_material_references: 0,
            maximum_bone_references: 0,
            maximum_meshes: mesh_count,
            maximum_materials: 0,
            maximum_avatars: 0,
            maximum_index_bytes: MAX_BYTES / 8,
            maximum_mesh_vertices: MAX_VERTICES,
            maximum_mesh_indices: MAX_INDICES,
            maximum_mesh_sub_meshes: MAX_INDICES / 3,
            maximum_material_entries: 0,
            maximum_avatar_elements: 0,
            maximum_total_string_bytes: MAX_BYTES / 8,
            mesh: mesh_limits,
            ..ModelIrLimits::default()
        },
    )?;
    drop(hierarchy);
    ensure_no_static_batches(session.studio.collection(), &model)?;
    combine_model(&model)
}

/// The backend's static model graph currently omits the renderer's batch
/// subset and batch-root placement. Until those are retained, copying the
/// shared mesh for each owner would duplicate unrelated triangles and place
/// them incorrectly. Apply the same explicit boundary to preview and export.
pub(crate) fn ensure_no_static_batches(
    collection: &AssetCollection,
    model: &ModelIr,
) -> Result<()> {
    for node in model.nodes.iter().filter(|node| node.export_content) {
        for renderer in node
            .renderers
            .iter()
            .filter(|renderer| renderer.mesh.is_some())
        {
            let key = renderer.component;
            let source = resolve_object_reference(
                collection,
                key.file_index,
                ObjectReference {
                    file_id: 0,
                    path_id: key.path_id,
                },
            )?
            .context("Model renderer source component is missing")?;
            let limits = RendererReadLimits::default();
            let batch = match renderer.kind {
                ModelRendererKind::MeshRenderer { .. } => {
                    read_mesh_renderer(source.file, source.object_index, limits)?
                        .renderer
                        .static_batch_info
                }
                ModelRendererKind::SkinnedMeshRenderer { .. } => {
                    read_skinned_mesh_renderer(source.file, source.object_index, limits)?
                        .renderer
                        .static_batch_info
                }
            };
            require_unbatched(key, batch)?;
        }
    }
    Ok(())
}

fn require_unbatched(key: SceneObjectKey, batch: StaticBatchInfo) -> Result<()> {
    ensure!(
        batch.sub_mesh_count == 0,
        "Static batch model conversion is not implemented: renderer {}:{} selects {} shared submeshes starting at {}; its subset and batch-root placement must be preserved",
        key.file_index,
        key.path_id,
        batch.sub_mesh_count,
        batch.first_sub_mesh
    );
    Ok(())
}

fn selected_mesh_count(hierarchy: &SceneHierarchy, root: SceneObjectKey) -> Result<usize> {
    let mut visited = HashSet::new();
    let mut meshes = HashSet::new();
    let mut pending = Vec::new();
    pending.try_reserve_exact(hierarchy.nodes.len())?;
    visited.try_reserve(hierarchy.nodes.len())?;
    meshes.try_reserve(
        hierarchy
            .nodes
            .len()
            .checked_mul(2)
            .context("Model preview mesh count overflow")?,
    )?;
    pending.push(root);
    while let Some(key) = pending.pop() {
        ensure!(
            visited.insert(key),
            "Model preview hierarchy contains a cycle"
        );
        let node = hierarchy
            .node(key)
            .context("Model preview root or child is missing")?;
        if let Some(key) = node.mesh_filter.as_ref().and_then(|filter| filter.mesh)
            && node.mesh_renderer.is_some()
        {
            meshes.insert(key);
        }
        if let Some(key) = node
            .skinned_mesh_renderer
            .as_ref()
            .and_then(|renderer| renderer.mesh)
        {
            meshes.insert(key);
        }
        ensure!(
            pending
                .len()
                .checked_add(node.children.len())
                .is_some_and(|n| n <= MAX_NODES),
            "Model preview hierarchy exceeds its traversal limit"
        );
        pending.extend_from_slice(&node.children);
    }
    Ok(meshes.len())
}

fn combine_model(model: &ModelIr) -> Result<Preview> {
    let mut parents = Vec::new();
    parents.try_reserve_exact(model.nodes.len())?;
    for node in &model.nodes {
        parents.push(
            node.parent
                .map(|key| {
                    model
                        .node_index(key)
                        .context("Model preview parent is missing")
                })
                .transpose()?,
        );
    }
    let matrices = global_matrices(&model.nodes, &parents)?;
    let mut geometry = PreviewGeometry::default();
    for (node, matrix) in model.nodes.iter().zip(matrices) {
        if !node.export_content {
            continue;
        }
        for renderer in &node.renderers {
            let Some(key) = renderer.mesh else { continue };
            let mesh = &model
                .mesh(key)
                .context("Model preview mesh is missing")?
                .mesh;
            geometry.append_instance(
                matrix,
                &mesh.vertices,
                mesh.sub_meshes
                    .iter()
                    .map(|submesh| submesh.indices.as_slice()),
            )?;
        }
    }
    ensure!(
        !geometry.vertices.is_empty() && !geometry.indices.is_empty(),
        "Selected model contains no triangles"
    );
    Ok(Preview::Mesh {
        vertices: geometry.vertices,
        indices: geometry.indices,
    })
}

#[derive(Default)]
struct PreviewGeometry {
    vertices: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl PreviewGeometry {
    fn append_instance<'a>(
        &mut self,
        matrix: Matrix,
        vertices: &[[f32; 3]],
        submeshes: impl Iterator<Item = &'a [u32]> + Clone,
    ) -> Result<()> {
        let count = submeshes.clone().try_fold(0_usize, |sum, indices| {
            ensure!(
                indices.len().is_multiple_of(3),
                "Model preview has an incomplete triangle"
            );
            ensure!(
                indices
                    .iter()
                    .all(|&index| (index as usize) < vertices.len()),
                "Model preview triangle references a missing vertex"
            );
            sum.checked_add(indices.len())
                .context("Model preview index count overflow")
        })?;
        let total_vertices = self
            .vertices
            .len()
            .checked_add(vertices.len())
            .context("Model preview vertex count overflow")?;
        let total_indices = self
            .indices
            .len()
            .checked_add(count)
            .context("Model preview index count overflow")?;
        ensure!(
            total_vertices <= MAX_VERTICES,
            "Model preview exceeds 100,000 vertices"
        );
        ensure!(
            total_indices <= MAX_INDICES,
            "Model preview exceeds 300,000 indices"
        );
        let bytes = total_vertices
            .checked_mul(size_of::<[f32; 3]>())
            .and_then(|n| {
                total_indices
                    .checked_mul(size_of::<u32>())
                    .and_then(|m| n.checked_add(m))
            })
            .context("Model preview byte count overflow")?;
        ensure!(bytes <= MAX_BYTES, "Model preview exceeds 64 MiB");
        let base = u32::try_from(self.vertices.len())?;
        self.vertices.try_reserve(vertices.len())?;
        self.indices.try_reserve(count)?;
        for &point in vertices {
            self.vertices.push(matrix.point(point)?);
        }
        for indices in submeshes {
            for &index in indices {
                self.indices.push(
                    base.checked_add(index)
                        .context("Model preview vertex index overflow")?,
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Matrix([[f64; 4]; 3]);

impl Matrix {
    const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);

    fn local(transform: Option<ModelLocalTransform>) -> Result<Self> {
        let Some(transform) = transform else {
            return Ok(Self::IDENTITY);
        };
        let mut q = [
            transform.local_rotation.x,
            transform.local_rotation.y,
            transform.local_rotation.z,
            transform.local_rotation.w,
        ]
        .map(f64::from);
        let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
        ensure!(
            norm.is_finite() && norm > 0.0,
            "Model preview has an invalid quaternion"
        );
        q.iter_mut().for_each(|value| *value /= norm);
        let [x, y, z, w] = q;
        let p = transform.local_position;
        let s = transform.local_scale;
        let scale = [s.x, s.y, s.z].map(f64::from);
        let mut out = Self([
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
                f64::from(p.x),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
                f64::from(p.y),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
                f64::from(p.z),
            ],
        ]);
        for row in &mut out.0 {
            for (value, scale) in row[..3].iter_mut().zip(scale) {
                *value *= scale;
            }
        }
        out.validate()?;
        Ok(out)
    }

    fn multiply(self, local: Self) -> Result<Self> {
        let mut out = Self([[0.0; 4]; 3]);
        for (r, row) in out.0.iter_mut().enumerate() {
            for (c, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|k| self.0[r][k] * local.0[k][c]).sum();
                if c == 3 {
                    *value += self.0[r][3];
                }
            }
        }
        out.validate()?;
        Ok(out)
    }

    fn point(self, point: [f32; 3]) -> Result<[f32; 3]> {
        ensure!(
            point.iter().all(|v| v.is_finite()),
            "Model preview has a non-finite vertex"
        );
        let mut out = [0.0; 3];
        for (value, row) in out.iter_mut().zip(self.0) {
            let position = row[3]
                + row[..3]
                    .iter()
                    .zip(point)
                    .map(|(m, p)| m * f64::from(p))
                    .sum::<f64>();
            ensure!(
                position.is_finite() && position.abs() <= f64::from(f32::MAX),
                "Model preview transformed vertex is outside the finite range"
            );
            *value = position as f32;
        }
        Ok(out)
    }

    fn validate(self) -> Result<()> {
        ensure!(
            self.0.iter().flatten().all(|v| v.is_finite()),
            "Model preview transform contains a non-finite value"
        );
        Ok(())
    }
}

fn global_matrices(nodes: &[ModelNode], parents: &[Option<usize>]) -> Result<Vec<Matrix>> {
    ensure!(
        nodes.len() == parents.len() && nodes.len() <= MAX_NODES,
        "Invalid model preview graph size"
    );
    let mut matrices = Vec::new();
    let mut states = Vec::new();
    let mut pending = Vec::new();
    matrices.try_reserve_exact(nodes.len())?;
    states.try_reserve_exact(nodes.len())?;
    pending.try_reserve_exact(nodes.len())?;
    matrices.resize(nodes.len(), Matrix::IDENTITY);
    states.resize(nodes.len(), 0_u8);
    for start in 0..nodes.len() {
        let mut current = Some(start);
        while let Some(index) = current {
            match states.get(index) {
                Some(2) => break,
                Some(1) => bail!("Model preview transform graph contains a cycle"),
                Some(0) => {}
                _ => bail!("Model preview parent index is outside the graph"),
            }
            states[index] = 1;
            pending.push(index);
            current = parents[index];
        }
        while let Some(index) = pending.pop() {
            let local = Matrix::local(nodes[index].transform)?;
            matrices[index] =
                parents[index].map_or(Ok(local), |parent| matrices[parent].multiply(local))?;
            states[index] = 2;
        }
    }
    Ok(matrices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_rs_core::scene::{Quaternion, Vector3};

    fn node(id: i64, position: [f32; 3]) -> ModelNode {
        let object = SceneObjectKey {
            file_index: 0,
            path_id: id,
        };
        ModelNode {
            object,
            name: String::new(),
            export_content: true,
            parent: None,
            children: Vec::new(),
            renderers: Vec::new(),
            animator: None,
            transform: Some(ModelLocalTransform {
                component: object,
                local_rotation: Quaternion {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                local_position: Vector3 {
                    x: position[0],
                    y: position[1],
                    z: position[2],
                },
                local_scale: Vector3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            }),
        }
    }

    #[test]
    fn hierarchy_composes_trs_in_unmirrored_unity_coordinates() {
        let mut parent = node(1, [10.0, 20.0, 30.0]);
        let transform = parent.transform.as_mut().unwrap();
        transform.local_rotation.z = std::f32::consts::FRAC_1_SQRT_2;
        transform.local_rotation.w = std::f32::consts::FRAC_1_SQRT_2;
        transform.local_scale.x = 2.0;
        let nodes = [node(2, [1.0, 0.0, 0.0]), parent];
        let matrices = global_matrices(&nodes, &[Some(1), None]).unwrap();
        assert_eq!(
            matrices[0].point([1.0, 0.0, 0.0]).unwrap(),
            [10.0, 24.0, 30.0]
        );
    }

    #[test]
    fn graph_is_iterative_and_rejects_cycles_or_invalid_parents() {
        let nodes: Vec<_> = (0..MAX_NODES).map(|id| node(id as i64, [0.0; 3])).collect();
        let parents: Vec<_> = (0..MAX_NODES)
            .map(|id| (id + 1 < MAX_NODES).then_some(id + 1))
            .collect();
        assert_eq!(global_matrices(&nodes, &parents).unwrap().len(), MAX_NODES);
        assert!(global_matrices(&nodes[..1], &[Some(0)]).is_err());
        assert!(global_matrices(&nodes[..1], &[Some(1)]).is_err());
    }

    #[test]
    fn instances_rebase_indices_and_validate_geometry_and_transforms() {
        let vertices = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let indices = [0, 1, 2];
        let mut geometry = PreviewGeometry::default();
        for _ in 0..2 {
            geometry
                .append_instance(
                    Matrix::IDENTITY,
                    &vertices,
                    std::iter::once(indices.as_slice()),
                )
                .unwrap();
        }
        assert_eq!(geometry.indices, [0, 1, 2, 3, 4, 5]);
        assert!(
            geometry
                .append_instance(
                    Matrix::IDENTITY,
                    &vertices,
                    std::iter::once([0, 1, 3].as_slice())
                )
                .is_err()
        );
        assert!(
            geometry
                .append_instance(
                    Matrix::IDENTITY,
                    &vertices,
                    std::iter::once([0, 1].as_slice())
                )
                .is_err()
        );
        assert!(Matrix::IDENTITY.point([f32::NAN, 0.0, 0.0]).is_err());
        let mut invalid = node(1, [0.0; 3]);
        invalid.transform.as_mut().unwrap().local_rotation.w = 0.0;
        assert!(Matrix::local(invalid.transform).is_err());
        invalid.transform.as_mut().unwrap().local_rotation.w = 1.0;
        invalid.transform.as_mut().unwrap().local_scale.x = f32::INFINITY;
        assert!(Matrix::local(invalid.transform).is_err());
    }

    #[test]
    fn expanded_instances_respect_shared_limits_and_finite_output() {
        let mut geometry = PreviewGeometry {
            vertices: vec![[0.0; 3]; MAX_VERTICES],
            indices: Vec::new(),
        };
        assert!(
            geometry
                .append_instance(
                    Matrix::IDENTITY,
                    &[[0.0; 3]],
                    std::iter::once([0, 0, 0].as_slice())
                )
                .is_err()
        );
        assert_eq!(geometry.vertices.len(), MAX_VERTICES);
        geometry.vertices.clear();
        geometry.indices.resize(MAX_INDICES, 0);
        assert!(
            geometry
                .append_instance(
                    Matrix::IDENTITY,
                    &[[0.0; 3]],
                    std::iter::once([0, 0, 0].as_slice())
                )
                .is_err()
        );
        let mut scaled = Matrix::IDENTITY;
        scaled.0[0][0] = 2.0;
        assert!(scaled.point([f32::MAX, 0.0, 0.0]).is_err());
    }

    #[test]
    fn static_batches_are_explicitly_rejected_instead_of_duplicating_shared_geometry() {
        let key = SceneObjectKey {
            file_index: 2,
            path_id: 17,
        };
        require_unbatched(
            key,
            StaticBatchInfo {
                first_sub_mesh: 0,
                sub_mesh_count: 0,
            },
        )
        .unwrap();
        let error = require_unbatched(
            key,
            StaticBatchInfo {
                first_sub_mesh: 3,
                sub_mesh_count: 4,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Static batch"));
        assert!(error.contains("2:17"));
        assert!(error.contains("4 shared submeshes starting at 3"));
    }
}
