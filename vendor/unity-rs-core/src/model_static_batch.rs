//! Restore static-batch submeshes in their original renderer's local space.
//!
//! Unity's StaticBatchingUtility stores combined vertices in the batch root's
//! coordinate space (world space for a null root). Keep node transforms intact
//! and convert each compact subset by inverse(ownerWorld) * batchRootWorld.
//! See Unity's StaticBatchingUtility.Combine and static-batching manuals.

use std::collections::HashMap;

use super::{ModelLocalTransform, ModelMesh};
use crate::mesh::{Mesh, MeshSubMesh, MeshVertexAttribute, MeshVertexChannels};
use crate::scene_hierarchy::{
    SceneHierarchy, SceneHierarchyNode, SceneObjectKey, StaticBatchBinding,
};
use crate::{Error, Result};

#[derive(Default)]
pub(super) struct Transforms {
    world: HashMap<SceneObjectKey, Matrix>,
    bytes: usize,
}

impl Transforms {
    pub(super) fn prepare<'a>(
        hierarchy: &SceneHierarchy,
        nodes: impl Iterator<Item = &'a SceneHierarchyNode>,
        maximum_bytes: usize,
    ) -> Result<Self> {
        let mut result = Self::default();
        for node in nodes {
            let Some(batch) = node
                .mesh_renderer
                .as_ref()
                .and_then(|renderer| renderer.static_batch)
            else {
                continue;
            };
            if node
                .mesh_filter
                .as_ref()
                .and_then(|filter| filter.mesh)
                .is_none()
            {
                continue;
            }
            result.resolve(hierarchy, node.object, maximum_bytes)?;
            if let Some(root) = batch.root {
                result.resolve(hierarchy, root, maximum_bytes)?;
            }
        }
        Ok(result)
    }

    pub(super) fn index_bytes(&self) -> usize {
        self.bytes
    }

    fn resolve(
        &mut self,
        hierarchy: &SceneHierarchy,
        start: SceneObjectKey,
        maximum: usize,
    ) -> Result<()> {
        let mut chain = Vec::new();
        let mut current = Some(start);
        while let Some(key) = current {
            if self.world.contains_key(&key) {
                break;
            }
            if chain.len() >= hierarchy.nodes.len() {
                return Err(Error::invalid_data(
                    "static batch transform hierarchy contains a cycle",
                ));
            }
            let work_bytes = chain
                .len()
                .checked_add(1)
                .and_then(|count| count.checked_mul(size_of::<SceneObjectKey>()))
                .and_then(|count| count.checked_add(self.bytes))
                .ok_or_else(|| {
                    Error::invalid_data("static batch transform work size overflowed")
                })?;
            if work_bytes > maximum {
                return Err(Error::invalid_data(
                    "static batch transform work exceeds model index budget",
                ));
            }
            let node = hierarchy.node(key).ok_or_else(|| {
                Error::invalid_data("static batch root or ancestor is absent from the hierarchy")
            })?;
            chain.try_reserve(1).map_err(|error| {
                Error::invalid_data(format!(
                    "cannot reserve static batch transform traversal: {error}"
                ))
            })?;
            chain.push(key);
            current = node.parent;
        }
        let traversal_bytes = chain
            .capacity()
            .checked_mul(size_of::<SceneObjectKey>())
            .ok_or_else(|| Error::invalid_data("static batch traversal size overflowed"))?;
        for key in chain.into_iter().rev() {
            let node = hierarchy
                .node(key)
                .ok_or_else(|| Error::invalid_data("static batch transform node is absent"))?;
            let local =
                Matrix::local(node.transform.as_ref().map(|binding| ModelLocalTransform {
                    component: binding.component,
                    local_rotation: binding.local_rotation,
                    local_position: binding.local_position,
                    local_scale: binding.local_scale,
                }))?;
            let parent = match node.parent {
                Some(parent) => *self.world.get(&parent).ok_or_else(|| {
                    Error::invalid_data("static batch parent transform was not resolved")
                })?,
                None => Matrix::IDENTITY,
            };
            let world = parent.multiply(local)?;
            let next = self
                .bytes
                .checked_add(size_of::<(SceneObjectKey, Matrix)>())
                .ok_or_else(|| {
                    Error::invalid_data("static batch transform index size overflowed")
                })?;
            if next
                .checked_add(traversal_bytes)
                .is_none_or(|bytes| bytes > maximum)
            {
                return Err(Error::invalid_data(
                    "static batch transforms exceed model index budget",
                ));
            }
            self.world.try_reserve(1).map_err(|error| {
                Error::invalid_data(format!(
                    "cannot reserve static batch transform index: {error}"
                ))
            })?;
            self.world.insert(key, world);
            self.bytes = next;
        }
        Ok(())
    }

    pub(super) fn to_owner(
        &self,
        owner: SceneObjectKey,
        root: Option<SceneObjectKey>,
    ) -> Result<Matrix> {
        let owner = *self
            .world
            .get(&owner)
            .ok_or_else(|| Error::invalid_data("static batch owner transform is absent"))?;
        let root = match root {
            Some(root) => *self
                .world
                .get(&root)
                .ok_or_else(|| Error::invalid_data("static batch root transform is absent"))?,
            None => Matrix::IDENTITY,
        };
        owner.inverse()?.multiply(root)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Matrix([[f64; 4]; 3]);
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
        let norm = q.iter().map(|value| value * value).sum::<f64>().sqrt();
        if !norm.is_finite() || norm == 0.0 {
            return Err(Error::invalid_data(
                "static batch transform has an invalid quaternion",
            ));
        }
        q.iter_mut().for_each(|value| *value /= norm);
        let [x, y, z, w] = q;
        let p = transform.local_position;
        let s = [
            transform.local_scale.x,
            transform.local_scale.y,
            transform.local_scale.z,
        ]
        .map(f64::from);
        let mut result = Self([
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
        for row in &mut result.0 {
            for (value, scale) in row[..3].iter_mut().zip(s) {
                *value *= scale;
            }
        }
        result.validate()?;
        Ok(result)
    }

    fn validate(self) -> Result<()> {
        if self.0.iter().flatten().any(|value| !value.is_finite()) {
            return Err(Error::invalid_data(
                "static batch transform contains non-finite values",
            ));
        }
        Ok(())
    }

    fn multiply(self, other: Self) -> Result<Self> {
        let mut result = Self([[0.0; 4]; 3]);
        for (r, row) in result.0.iter_mut().enumerate() {
            for (c, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|k| self.0[r][k] * other.0[k][c]).sum();
                if c == 3 {
                    *value += self.0[r][3];
                }
            }
        }
        result.validate()?;
        Ok(result)
    }

    fn determinant(self) -> f64 {
        let a = self.0;
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    }

    fn inverse(self) -> Result<Self> {
        let a = self.0;
        let determinant = self.determinant();
        if !determinant.is_finite() || determinant == 0.0 {
            return Err(Error::invalid_data(
                "static batch owner has a singular transform",
            ));
        }
        let d = determinant.recip();
        let mut result = Self([
            [
                (a[1][1] * a[2][2] - a[1][2] * a[2][1]) * d,
                (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * d,
                (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * d,
                0.0,
            ],
            [
                (a[1][2] * a[2][0] - a[1][0] * a[2][2]) * d,
                (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * d,
                (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * d,
                0.0,
            ],
            [
                (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * d,
                (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * d,
                (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * d,
                0.0,
            ],
        ]);
        for row in &mut result.0 {
            row[3] = -(0..3).map(|k| row[k] * a[k][3]).sum::<f64>();
        }
        result.validate()?;
        Ok(result)
    }

    fn point(self, point: [f32; 3]) -> Result<[f32; 3]> {
        let mut result = [0.0; 3];
        for (row, value) in self.0.iter().zip(&mut result) {
            let sum = row[3] + (0..3).map(|k| row[k] * f64::from(point[k])).sum::<f64>();
            *value = finite_f32(sum)?;
        }
        Ok(result)
    }

    fn direction(self, vector: [f32; 3]) -> Result<[f32; 3]> {
        let mut result = [0.0; 3];
        for (row, value) in self.0.iter().zip(&mut result) {
            *value = finite_f32((0..3).map(|k| row[k] * f64::from(vector[k])).sum())?;
        }
        normalize(result)
    }

    fn normal(self, vector: [f32; 3]) -> Result<[f32; 3]> {
        let inverse = self.inverse()?;
        let mut result = [0.0; 3];
        for (column, value) in result.iter_mut().enumerate() {
            *value = finite_f32(
                (0..3)
                    .map(|row| inverse.0[row][column] * f64::from(vector[row]))
                    .sum(),
            )?;
        }
        normalize(result)
    }
}

fn finite_f32(value: f64) -> Result<f32> {
    let result = value as f32;
    if !result.is_finite() {
        return Err(Error::invalid_data(
            "static batch geometry has non-finite transformed values",
        ));
    }
    Ok(result)
}
fn normalize(vector: [f32; 3]) -> Result<[f32; 3]> {
    let length = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if !length.is_finite() {
        return Err(Error::invalid_data("static batch direction is non-finite"));
    }
    if length == 0.0 {
        return Ok([0.0; 3]);
    }
    Ok(vector.map(|value| (f64::from(value) / length) as f32))
}

pub(super) fn mesh_data_bytes(mesh: &Mesh, channels: &MeshVertexChannels) -> Result<usize> {
    let mut total = size_of::<ModelMesh>()
        .checked_add(mesh.name.len())
        .ok_or_else(|| Error::invalid_data("model mesh data size overflowed"))?;
    let mut charge = |count: usize, size: usize| -> Result<()> {
        total = count
            .checked_mul(size)
            .and_then(|bytes| total.checked_add(bytes))
            .ok_or_else(|| Error::invalid_data("model mesh data size overflowed"))?;
        Ok(())
    };
    charge(mesh.vertices.len(), size_of::<[f32; 3]>())?;
    if let Some(values) = &mesh.normals {
        charge(values.len(), size_of::<[f32; 3]>())?;
    }
    if let Some(values) = &mesh.uv0 {
        charge(values.len(), size_of::<[f32; 2]>())?;
    }
    charge(mesh.bind_poses.len(), size_of::<[f32; 16]>())?;
    charge(mesh.bone_name_hashes.len(), size_of::<u32>())?;
    if let Some(values) = &mesh.skin {
        charge(values.len(), size_of::<crate::mesh::MeshBoneWeight>())?;
    }
    charge(mesh.sub_meshes.len(), size_of::<MeshSubMesh>())?;
    for submesh in &mesh.sub_meshes {
        charge(submesh.indices.len(), size_of::<u32>())?;
    }
    for channel in channels
        .uvs
        .iter()
        .chain([&channels.colors, &channels.tangents])
        .flatten()
    {
        charge(channel.values.len(), size_of::<f32>())?;
    }
    if let Some(shapes) = &mesh.blend_shapes {
        charge(
            shapes.vertices.len(),
            size_of::<crate::mesh::MeshBlendShapeVertex>(),
        )?;
        charge(
            shapes.frames.len(),
            size_of::<crate::mesh::MeshBlendShapeFrame>(),
        )?;
        charge(
            shapes.channels.len(),
            size_of::<crate::mesh::MeshBlendShapeChannel>(),
        )?;
        charge(shapes.full_weights.len(), size_of::<f32>())?;
        for channel in &shapes.channels {
            charge(channel.name.len(), 1)?;
        }
    }
    Ok(total)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compact_mesh(
    source: &ModelMesh,
    key: SceneObjectKey,
    batch: StaticBatchBinding,
    transform: Matrix,
    maximum_vertices: usize,
    maximum_indices: usize,
    maximum_submeshes: usize,
    maximum_bytes: u64,
) -> Result<ModelMesh> {
    let mesh = &source.mesh;
    let begin = usize::from(batch.first_sub_mesh);
    let count = usize::from(batch.sub_mesh_count);
    let end = begin
        .checked_add(count)
        .ok_or_else(|| Error::invalid_data("static batch submesh range overflowed"))?;
    let submeshes = mesh
        .sub_meshes
        .get(begin..end)
        .ok_or_else(|| Error::invalid_data("static batch submesh range exceeds the shared mesh"))?;
    if count == 0 || count > maximum_submeshes {
        return Err(Error::invalid_data(
            "static batch submesh count exceeds model limit",
        ));
    }
    if mesh.skin.is_some() || !mesh.bind_poses.is_empty() || mesh.blend_shapes.is_some() {
        return Err(Error::invalid_data(
            "static-batched MeshRenderer must not contain skinned or blend-shape geometry",
        ));
    }
    let index_count = submeshes
        .iter()
        .try_fold(0usize, |sum, submesh| {
            sum.checked_add(submesh.indices.len())
        })
        .ok_or_else(|| Error::invalid_data("static batch index count overflowed"))?;
    if index_count > maximum_indices {
        return Err(Error::invalid_data(
            "static batch indices exceed model limit",
        ));
    }
    let maximum_unique = mesh.vertices.len().min(index_count);
    // Account the dense source→compact map and an upper bound for every
    // retained output numeric channel before allocating any of them.
    let mut scalars =
        3usize + usize::from(mesh.normals.is_some()) * 3 + usize::from(mesh.uv0.is_some()) * 2;
    for channel in source
        .channels
        .uvs
        .iter()
        .chain([&source.channels.colors, &source.channels.tangents])
        .flatten()
    {
        if !(1..=4).contains(&channel.dimension)
            || channel.values.len()
                != mesh
                    .vertices
                    .len()
                    .checked_mul(channel.dimension)
                    .ok_or_else(|| Error::invalid_data("static batch channel size overflowed"))?
        {
            return Err(Error::invalid_data(
                "static batch vertex channel shape is invalid",
            ));
        }
        scalars = scalars
            .checked_add(channel.dimension)
            .ok_or_else(|| Error::invalid_data("static batch channel count overflowed"))?;
    }
    let bytes = mesh
        .vertices
        .len()
        .checked_mul(size_of::<u32>())
        .and_then(|map| {
            maximum_unique
                .checked_mul(scalars)
                .and_then(|values| values.checked_mul(4))
                .and_then(|values| map.checked_add(values))
        })
        .and_then(|values| {
            maximum_unique
                .checked_mul(size_of::<usize>())
                .and_then(|indices| values.checked_add(indices))
        })
        .and_then(|values| {
            index_count
                .checked_mul(4)
                .and_then(|indices| values.checked_add(indices))
        })
        .ok_or_else(|| Error::invalid_data("static batch allocation size overflowed"))?;
    if bytes as u64 > maximum_bytes {
        return Err(Error::invalid_data(
            "static batch geometry exceeds decoded byte limit",
        ));
    }
    let mut remap = reserve(mesh.vertices.len(), "static batch vertex map")?;
    remap.resize(mesh.vertices.len(), u32::MAX);
    let mut originals = reserve(
        maximum_unique.min(maximum_vertices),
        "static batch selected vertices",
    )?;
    let mut selected = reserve(submeshes.len(), "static batch submeshes")?;
    for submesh in submeshes {
        if !submesh.indices.len().is_multiple_of(3) {
            return Err(Error::invalid_data(
                "static batch contains an incomplete triangle",
            ));
        }
        let mut indices = reserve(submesh.indices.len(), "static batch indices")?;
        for &source_index in &submesh.indices {
            let slot = remap.get_mut(source_index as usize).ok_or_else(|| {
                Error::invalid_data("static batch references an absent shared vertex")
            })?;
            if *slot == u32::MAX {
                if originals.len() >= maximum_vertices {
                    return Err(Error::invalid_data(
                        "static batch vertices exceed model limit",
                    ));
                }
                *slot = u32::try_from(originals.len())
                    .map_err(|_| Error::invalid_data("static batch vertex index exceeds u32"))?;
                originals.push(source_index as usize);
            }
            indices.push(*slot);
        }
        selected.push(MeshSubMesh {
            first_byte: 0,
            index_count: u32::try_from(indices.len())
                .map_err(|_| Error::invalid_data("static batch index count exceeds u32"))?,
            first_vertex: 0,
            vertex_count: 0,
            indices,
        });
    }
    let mut vertices = reserve(originals.len(), "static batch positions")?;
    for &index in &originals {
        vertices.push(transform.point(mesh.vertices[index])?);
    }
    let normals = mesh
        .normals
        .as_ref()
        .map(|values| remap_vectors(values, &originals, |value| transform.normal(value)))
        .transpose()?;
    let uv0 = mesh
        .uv0
        .as_ref()
        .map(|values| remap_vectors(values, &originals, Ok))
        .transpose()?;
    let mut channels = MeshVertexChannels {
        colors: remap_attribute(source.channels.colors.as_ref(), &originals, None)?,
        tangents: remap_attribute(
            source.channels.tangents.as_ref(),
            &originals,
            Some(transform),
        )?,
        ..MeshVertexChannels::default()
    };
    for (output, source) in channels.uvs.iter_mut().zip(&source.channels.uvs) {
        *output = remap_attribute(source.as_ref(), &originals, None)?;
    }
    let vertex_count = u32::try_from(vertices.len())
        .map_err(|_| Error::invalid_data("static batch vertex count exceeds u32"))?;
    for submesh in &mut selected {
        submesh.vertex_count = vertex_count;
    }
    Ok(ModelMesh {
        object: key,
        mesh: Mesh {
            path_id: key.path_id,
            name: super::clone_string(&mesh.name, "static batch mesh name")?,
            vertices,
            normals,
            uv0,
            bind_poses: Vec::new(),
            bone_name_hashes: Vec::new(),
            root_bone_name_hash: 0,
            skin: None,
            blend_shapes: None,
            sub_meshes: selected,
        },
        channels,
    })
}

fn reserve<T>(count: usize, label: &str) -> Result<Vec<T>> {
    super::reserve_vec(count, label)
}
fn remap_vectors<T: Copy, const N: usize>(
    source: &[[T; N]],
    indices: &[usize],
    mut convert: impl FnMut([T; N]) -> Result<[T; N]>,
) -> Result<Vec<[T; N]>> {
    let mut result = reserve(indices.len(), "static batch channel")?;
    for &index in indices {
        result.push(convert(*source.get(index).ok_or_else(|| {
            Error::invalid_data("static batch vertex channel is truncated")
        })?)?);
    }
    Ok(result)
}
fn remap_attribute(
    source: Option<&MeshVertexAttribute>,
    indices: &[usize],
    direction: Option<Matrix>,
) -> Result<Option<MeshVertexAttribute>> {
    let Some(source) = source else {
        return Ok(None);
    };
    let count = indices
        .len()
        .checked_mul(source.dimension)
        .ok_or_else(|| Error::invalid_data("static batch channel size overflowed"))?;
    let mut values = reserve(count, "static batch vertex attribute")?;
    for &index in indices {
        let begin = index
            .checked_mul(source.dimension)
            .ok_or_else(|| Error::invalid_data("static batch channel offset overflowed"))?;
        let data = source
            .values
            .get(
                begin
                    ..begin.checked_add(source.dimension).ok_or_else(|| {
                        Error::invalid_data("static batch channel end overflowed")
                    })?,
            )
            .ok_or_else(|| Error::invalid_data("static batch attribute is truncated"))?;
        if data.iter().any(|value| !value.is_finite()) {
            return Err(Error::invalid_data("static batch attribute is non-finite"));
        }
        if let Some(transform) = direction
            && source.dimension >= 3
        {
            values.extend_from_slice(&transform.direction([data[0], data[1], data[2]])?);
            if source.dimension == 4 {
                values.push(
                    data[3]
                        * if transform.determinant() < 0.0 {
                            -1.0
                        } else {
                            1.0
                        },
                );
            }
        } else {
            values.extend_from_slice(data);
        }
    }
    Ok(Some(MeshVertexAttribute {
        dimension: source.dimension,
        values,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Quaternion, Vector3};
    use crate::scene_hierarchy::{MeshFilterBinding, RendererBinding, TransformBinding};

    fn key(path_id: i64) -> SceneObjectKey {
        SceneObjectKey {
            file_index: 0,
            path_id,
        }
    }
    fn local(position: [f32; 3], scale: [f32; 3]) -> ModelLocalTransform {
        ModelLocalTransform {
            component: key(11),
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
                x: scale[0],
                y: scale[1],
                z: scale[2],
            },
        }
    }
    fn source() -> ModelMesh {
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [4.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [4.0, 2.0, 0.0],
        ];
        let mut channels = MeshVertexChannels {
            colors: Some(MeshVertexAttribute {
                dimension: 4,
                values: (0..6)
                    .flat_map(|index| [index as f32 / 6.0, 0.5, 1.0, 1.0])
                    .collect(),
            }),
            tangents: Some(MeshVertexAttribute {
                dimension: 4,
                values: [1.0, 0.0, 0.0, 1.0].repeat(6),
            }),
            ..Default::default()
        };
        channels.uvs[1] = Some(MeshVertexAttribute {
            dimension: 3,
            values: (0..6).flat_map(|index| [index as f32, 0.5, 0.25]).collect(),
        });
        ModelMesh {
            object: key(51),
            mesh: Mesh {
                path_id: 51,
                name: "combined".into(),
                vertices,
                normals: Some(vec![[0.0, 0.0, 1.0]; 6]),
                uv0: Some(vec![[0.0, 0.0]; 6]),
                bind_poses: Vec::new(),
                bone_name_hashes: Vec::new(),
                root_bone_name_hash: 0,
                skin: None,
                blend_shapes: None,
                sub_meshes: vec![
                    MeshSubMesh {
                        first_byte: 0,
                        index_count: 3,
                        first_vertex: 0,
                        vertex_count: 3,
                        indices: vec![0, 1, 2],
                    },
                    MeshSubMesh {
                        first_byte: 6,
                        index_count: 3,
                        first_vertex: 3,
                        vertex_count: 3,
                        indices: vec![3, 4, 5],
                    },
                ],
            },
            channels,
        }
    }
    fn batch(first: u16, count: u16) -> StaticBatchBinding {
        StaticBatchBinding {
            first_sub_mesh: first,
            sub_mesh_count: count,
            root: None,
        }
    }
    fn compact(
        source: &ModelMesh,
        batch: StaticBatchBinding,
        transform: Matrix,
    ) -> Result<ModelMesh> {
        compact_mesh(source, key(31), batch, transform, 100, 100, 10, 1024 * 1024)
    }
    fn near(actual: [f32; 3], expected: [f32; 3]) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!((a - e).abs() < 0.0001, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn compact_subsets_preserve_only_owned_triangles_and_every_vertex_channel() {
        let original = source();
        let owner = Matrix::local(Some(local([2.0, 3.0, 4.0], [2.0, 1.0, 0.5]))).unwrap();
        let variant = compact(&original, batch(1, 1), owner.inverse().unwrap()).unwrap();
        assert_eq!(variant.object, key(31));
        assert_eq!(variant.mesh.vertices.len(), 3);
        assert_eq!(variant.mesh.sub_meshes.len(), 1);
        assert_eq!(variant.mesh.sub_meshes[0].indices, [0, 1, 2]);
        for (vertex, index) in variant.mesh.vertices.iter().zip(3..6) {
            near(owner.point(*vertex).unwrap(), original.mesh.vertices[index]);
        }
        assert_eq!(
            variant.mesh.normals.as_ref().unwrap(),
            &vec![[0.0, 0.0, 1.0]; 3]
        );
        assert_eq!(
            variant.channels.colors.as_ref().unwrap().values,
            &original.channels.colors.as_ref().unwrap().values[12..]
        );
        assert_eq!(
            variant.channels.uvs[1].as_ref().unwrap().values,
            &original.channels.uvs[1].as_ref().unwrap().values[9..]
        );
        assert_eq!(
            variant.channels.tangents.as_ref().unwrap().values,
            [1.0, 0.0, 0.0, 1.0].repeat(3)
        );
    }

    #[test]
    fn root_to_owner_conversion_preserves_world_points_and_inverse_transpose_normals() {
        let owner = Matrix::local(Some(local([10.0, -3.0, 2.0], [-2.0, 3.0, 0.5]))).unwrap();
        let mut root_transform = local([-5.0, 8.0, 1.0], [1.0, 2.0, 4.0]);
        root_transform.local_rotation.z = std::f32::consts::FRAC_1_SQRT_2;
        root_transform.local_rotation.w = std::f32::consts::FRAC_1_SQRT_2;
        let root = Matrix::local(Some(root_transform)).unwrap();
        let transform = owner.inverse().unwrap().multiply(root).unwrap();
        for point in [[1.0, 2.0, 3.0], [-2.0, 1.0, 4.0]] {
            near(
                owner.point(transform.point(point).unwrap()).unwrap(),
                root.point(point).unwrap(),
            );
        }
        let normal = [0.57735026; 3];
        near(
            owner.normal(transform.normal(normal).unwrap()).unwrap(),
            root.normal(normal).unwrap(),
        );
        assert!(transform.determinant() < 0.0);
        let variant = compact(&source(), batch(1, 1), transform).unwrap();
        assert_eq!(variant.channels.tangents.unwrap().values[3], -1.0);
    }

    #[test]
    fn invalid_ranges_attributes_indices_and_allocation_budgets_fail_explicitly() {
        let source = source();
        assert!(
            compact(&source, batch(1, 2), Matrix::IDENTITY)
                .unwrap_err()
                .to_string()
                .contains("range")
        );
        assert!(compact(&source, batch(0, 0), Matrix::IDENTITY).is_err());
        assert!(
            compact_mesh(
                &source,
                key(31),
                batch(0, 1),
                Matrix::IDENTITY,
                2,
                100,
                10,
                1024 * 1024
            )
            .unwrap_err()
            .to_string()
            .contains("vertices")
        );
        assert!(
            compact_mesh(
                &source,
                key(31),
                batch(0, 1),
                Matrix::IDENTITY,
                100,
                2,
                10,
                1024 * 1024
            )
            .unwrap_err()
            .to_string()
            .contains("indices")
        );
        assert!(
            compact_mesh(
                &source,
                key(31),
                batch(0, 1),
                Matrix::IDENTITY,
                100,
                100,
                10,
                1
            )
            .unwrap_err()
            .to_string()
            .contains("byte")
        );
        let mut broken = source.clone();
        broken.mesh.sub_meshes[0].indices[0] = 9;
        assert!(
            compact(&broken, batch(0, 1), Matrix::IDENTITY)
                .unwrap_err()
                .to_string()
                .contains("absent")
        );
        let mut broken = source.clone();
        broken.channels.colors.as_mut().unwrap().values.clear();
        assert!(
            compact(&broken, batch(0, 1), Matrix::IDENTITY)
                .unwrap_err()
                .to_string()
                .contains("shape")
        );
        let mut broken = source.clone();
        broken.mesh.sub_meshes[0].indices.pop();
        assert!(
            compact(&broken, batch(0, 1), Matrix::IDENTITY)
                .unwrap_err()
                .to_string()
                .contains("triangle")
        );
        assert!(
            Matrix::local(Some(local([0.0; 3], [0.0, 1.0, 1.0])))
                .unwrap()
                .inverse()
                .is_err()
        );
    }

    fn node(
        id: i64,
        parent: Option<i64>,
        position: [f32; 3],
        batch_root: Option<Option<i64>>,
    ) -> SceneHierarchyNode {
        let transform = local(position, [1.0; 3]);
        SceneHierarchyNode {
            object: key(id),
            name: format!("node{id}"),
            transform: Some(TransformBinding {
                component: key(id + 100),
                local_rotation: transform.local_rotation,
                local_position: transform.local_position,
                local_scale: transform.local_scale,
                parent_transform: parent.map(|parent| key(parent + 100)),
                declared_children: Vec::new(),
            }),
            mesh_filter: batch_root.map(|_| MeshFilterBinding {
                component: key(id + 200),
                mesh: Some(key(51)),
            }),
            mesh_renderer: batch_root.map(|root| RendererBinding {
                component: key(id + 300),
                materials: Vec::new(),
                static_batch: Some(StaticBatchBinding {
                    first_sub_mesh: 0,
                    sub_mesh_count: 1,
                    root: root.map(key),
                }),
            }),
            skinned_mesh_renderer: None,
            animator: None,
            parent: parent.map(key),
            children: Vec::new(),
        }
    }

    #[test]
    fn transform_resolution_retains_external_roots_and_is_iterative_and_bounded() {
        let hierarchy = SceneHierarchy::from_test_parts(
            vec![
                node(1, Some(2), [2.0, 0.0, 0.0], Some(Some(3))),
                node(2, None, [10.0, 0.0, 0.0], None),
                node(3, None, [100.0, 0.0, 0.0], None),
            ],
            vec![key(2), key(3)],
        );
        let transforms =
            Transforms::prepare(&hierarchy, std::iter::once(&hierarchy.nodes[0]), 4096).unwrap();
        assert_eq!(
            transforms
                .to_owner(key(1), Some(key(3)))
                .unwrap()
                .point([1.0, 0.0, 0.0])
                .unwrap(),
            [89.0, 0.0, 0.0]
        );
        assert!(Transforms::prepare(&hierarchy, hierarchy.nodes.iter(), 1).is_err());
        let count = 2048;
        let nodes = (0..count)
            .map(|id| {
                node(
                    id,
                    (id + 1 < count).then_some(id + 1),
                    [1.0, 0.0, 0.0],
                    (id == 0).then_some(None),
                )
            })
            .collect();
        let deep = SceneHierarchy::from_test_parts(nodes, vec![key(count - 1)]);
        let transforms = Transforms::prepare(&deep, deep.nodes.iter(), 1024 * 1024).unwrap();
        assert_eq!(
            transforms
                .to_owner(key(0), None)
                .unwrap()
                .point([0.0; 3])
                .unwrap(),
            [-(count as f32), 0.0, 0.0]
        );
        let cyclic =
            SceneHierarchy::from_test_parts(vec![node(1, Some(1), [0.0; 3], Some(None))], vec![]);
        assert!(Transforms::prepare(&cyclic, cyclic.nodes.iter(), 4096).is_err());
    }
}
