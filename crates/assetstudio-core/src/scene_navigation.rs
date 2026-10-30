use std::collections::HashMap;
use std::mem::size_of;

use anyhow::{Context, Result, bail, ensure};
use unity_rs_core::{
    loader::AssetCollection,
    renderer::{RendererReadLimits, read_skinned_mesh_renderer},
    scene::{SceneReadLimits, read_game_object, read_mesh_filter, resolve_object_reference},
    serialized::ObjectReference,
};

use crate::{AssetKey, ProgressObserver, Session};

impl Session {
    /// Finds actual loaded GameObjects for the desktop's "Show in hierarchy".
    /// Mesh ownership is indexed lazily, keeping folder loading unchanged.
    pub fn scene_owners(
        &self,
        key: AssetKey,
        progress: Option<&ProgressObserver>,
    ) -> Result<Vec<AssetKey>> {
        let record = self.record(key)?;
        if matches!(record.class_id, 1 | 95) {
            let root = crate::model::resolve_model_root(self, key)?;
            return Ok(vec![AssetKey {
                file_index: root.file_index,
                path_id: root.path_id,
            }]);
        }
        if record.class_id != 43 {
            bail!("Choose a Mesh, GameObject or Animator to locate its model hierarchy.");
        }
        if self.mesh_scene_owners.get().is_none() {
            let collection = self.studio.collection();
            let total = self
                .assets()
                .iter()
                .filter(|asset| matches!(asset.class_id, 33 | 137))
                .count();
            let mut index = HashMap::<AssetKey, Vec<AssetKey>>::new();
            let mut bytes = 0usize;
            crate::session::report_progress(progress, "Finding mesh owners", 0, Some(total));
            for (done, object) in self
                .studio
                .objects()
                .filter(|object| matches!(object.class_id(), 33 | 137))
                .enumerate()
            {
                if done % 4096 == 0 {
                    crate::session::report_progress(
                        progress,
                        "Finding mesh owners",
                        done,
                        Some(total),
                    );
                }
                let loaded = &collection.serialized_files()[object.file_index()];
                let Ok((owner, mesh)) =
                    mesh_link(&loaded.file, object.object_index(), object.class_id())
                else {
                    continue;
                };
                let Some(mesh) = resolve_key(collection, object.file_index(), mesh, 43) else {
                    continue;
                };
                let Some(owner) = resolve_key(collection, object.file_index(), owner, 1) else {
                    continue;
                };
                bytes = bytes
                    .checked_add(size_of::<AssetKey>() * 3 + size_of::<Vec<AssetKey>>())
                    .context("mesh owner index size overflow")?;
                ensure!(
                    bytes <= self.scene_limits().maximum_index_bytes,
                    "mesh owner index exceeds byte limit"
                );
                index.try_reserve(1)?;
                let owners = index.entry(mesh).or_default();
                owners.try_reserve(1)?;
                owners.push(owner);
            }
            for owners in index.values_mut() {
                owners.sort_unstable();
                owners.dedup();
            }
            crate::session::report_progress(progress, "Finding mesh owners", total, Some(total));
            let _ = self.mesh_scene_owners.set(index);
        }
        let index = self
            .mesh_scene_owners
            .get()
            .context("mesh owner index was not prepared")?;
        let owners = index
            .get(&key)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&owner| self.owner_uses_mesh(owner, key))
            .collect();
        Ok(owners)
    }

    fn owner_uses_mesh(&self, owner: AssetKey, mesh: AssetKey) -> bool {
        let collection = self.studio.collection();
        let Ok(Some(target)) = resolve_object_reference(
            collection,
            owner.file_index,
            ObjectReference {
                file_id: 0,
                path_id: owner.path_id,
            },
        ) else {
            return false;
        };
        let Ok(object) =
            read_game_object(target.file, target.object_index, SceneReadLimits::default())
        else {
            return false;
        };
        let mut filter_mesh = None;
        let mut skinned_mesh = None;
        for component in object.components {
            let Ok(Some(target)) =
                resolve_object_reference(collection, owner.file_index, component.component)
            else {
                continue;
            };
            if !matches!(target.object.class_id, 33 | 137) {
                continue;
            }
            let Ok((_, reference)) =
                mesh_link(target.file, target.object_index, target.object.class_id)
            else {
                continue;
            };
            let resolved = resolve_key(collection, target.file_index, reference, 43);
            if target.object.class_id == 33 {
                filter_mesh = resolved;
            } else {
                skinned_mesh = resolved;
            }
        }
        filter_mesh == Some(mesh) || skinned_mesh == Some(mesh)
    }
}

fn mesh_link(
    file: &unity_rs_core::serialized::SerializedFile,
    index: usize,
    class: i32,
) -> Result<(ObjectReference, ObjectReference)> {
    if class == 33 {
        let filter = read_mesh_filter(file, index, SceneReadLimits::default())?;
        Ok((filter.component.game_object, filter.mesh))
    } else {
        let renderer = read_skinned_mesh_renderer(file, index, RendererReadLimits::default())?;
        Ok((renderer.renderer.component.game_object, renderer.mesh))
    }
}

fn resolve_key(
    collection: &AssetCollection,
    file: usize,
    reference: ObjectReference,
    class: i32,
) -> Option<AssetKey> {
    let target = resolve_object_reference(collection, file, reference)
        .ok()
        .flatten()?;
    (target.object.class_id == class).then_some(AssetKey {
        file_index: target.file_index,
        path_id: target.object.path_id,
    })
}
