use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::Path,
};

use crate::model_options::{ModelExportOptions, apply_animation_options, apply_model_options};
use crate::output::{SafeOutput, component};
use crate::session::{MAX_EXPORT_FILE_BYTES, MAX_EXPORT_TOTAL_BYTES};
use crate::{AssetKey, ModelFormat, OperationReport, Session};
use anyhow::{Context, Result, bail};
use unity_rs_core::{
    animation_graph::{AnimationGraphLimits, build_animation_graph},
    fbx_ascii::write_model_ir_fbx_ascii_with_settings,
    fbx_binary_scene::write_model_ir_fbx_binary_full_with_settings,
    image_export::ImageFormat as BackendImageFormat,
    model_animation::{ModelAnimationLimits, build_model_animations},
    model_ir::{ModelIr, ModelIrLimits, build_model_ir, build_model_ir_for_game_object},
    obj_scene::{write_model_ir_mtl, write_model_ir_obj},
    scene_hierarchy::{SceneObjectKey, build_scene_hierarchy_for_roots},
    scene_textures::{SceneTextureLimits, SceneTextureSet},
};

struct ModelBudget {
    used: u64,
    maximum: u64,
    options: ModelExportOptions,
}

impl Session {
    /// Exports whole-scene models or one independent package for each selected root.
    /// Packages contain the model and every successfully decoded material texture.
    pub fn export_scene(
        &self,
        output: &Path,
        format: ModelFormat,
        roots: &[AssetKey],
    ) -> Result<OperationReport> {
        self.export_scene_with_options(output, format, roots, &ModelExportOptions::default())
    }

    pub fn export_scene_with_options(
        &self,
        output: &Path,
        format: ModelFormat,
        roots: &[AssetKey],
        options: &ModelExportOptions,
    ) -> Result<OperationReport> {
        self.export_scene_with_limit_and_options(
            output,
            format,
            roots,
            MAX_EXPORT_TOTAL_BYTES,
            options,
        )
    }

    pub(crate) fn export_scene_with_limit_and_options(
        &self,
        output: &Path,
        format: ModelFormat,
        roots: &[AssetKey],
        maximum_total: u64,
        options: &ModelExportOptions,
    ) -> Result<OperationReport> {
        options.validate()?;
        if roots.len() > 1_000_000 {
            bail!("model root count exceeds catalog limit");
        }
        let destination = SafeOutput::new(output)?;
        let selected_roots = roots
            .iter()
            .filter_map(|&root| resolve_model_root(self, root).ok())
            .collect::<Vec<_>>();
        let mut hierarchy = if roots.is_empty() {
            self.studio.scene_hierarchy(self.scene_limits())?
        } else {
            build_scene_hierarchy_for_roots(
                self.studio.collection(),
                &selected_roots,
                self.scene_limits(),
            )?
        };
        // Disabled materials must not require their binary layouts or texture
        // dependencies to parse successfully during model construction.
        if !options.include_materials {
            for node in &mut hierarchy.nodes {
                if let Some(renderer) = &mut node.mesh_renderer {
                    renderer.materials.clear();
                }
                if let Some(renderer) = &mut node.skinned_mesh_renderer {
                    renderer.materials.clear();
                }
            }
        }
        let mut report = OperationReport::default();
        let mut budget = ModelBudget {
            used: 0,
            maximum: maximum_total,
            options: *options,
        };
        if roots.is_empty() {
            let model = build_model_ir(
                self.studio.collection(),
                &hierarchy,
                ModelIrLimits::default(),
            )?;
            self.write_model_package(
                &destination,
                "scene",
                model,
                format,
                &mut budget,
                &mut report,
            )?;
        } else {
            let mut seen = HashSet::new();
            for &root in roots {
                if !seen.insert(root) {
                    continue;
                }
                let selected = match resolve_model_root(self, root) {
                    Ok(selected) => selected,
                    Err(error) => {
                        report
                            .failed
                            .push(format!("selected root {root:?}: {error:#}"));
                        continue;
                    }
                };
                let Some(node) = hierarchy.node(selected) else {
                    report.failed.push(format!(
                        "selected root {root:?} is not a resolved GameObject"
                    ));
                    continue;
                };
                let name = model_package_name(&node.name, selected);
                let result = build_model_ir_for_game_object(
                    self.studio.collection(),
                    &hierarchy,
                    selected,
                    ModelIrLimits::default(),
                )
                .map_err(anyhow::Error::from)
                .and_then(|model| {
                    self.write_model_package(
                        &destination,
                        &name,
                        model,
                        format,
                        &mut budget,
                        &mut report,
                    )
                });
                if let Err(error) = result {
                    report.failed.push(format!("model {name:?}: {error:#}"));
                }
            }
        }
        Ok(report)
    }

    fn write_model_package(
        &self,
        destination: &SafeOutput,
        name: &str,
        mut model: ModelIr,
        format: ModelFormat,
        budget: &mut ModelBudget,
        report: &mut OperationReport,
    ) -> Result<()> {
        apply_model_options(&mut model, &budget.options)?;
        if !model.nodes.iter().any(|node| {
            node.export_content
                && node
                    .renderers
                    .iter()
                    .any(|renderer| renderer.mesh.is_some())
        }) {
            bail!("model contains no renderable mesh");
        }
        let target = destination.root().join(name);
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                bail!("model target is not a regular directory: {target:?}")
            }
            Ok(_) => {
                report
                    .skipped
                    .push(format!("existing model package: {}", target.display()));
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect model output"),
        }
        let remaining = budget
            .maximum
            .checked_sub(budget.used)
            .context("model output budget exhausted")?;
        let textures = if budget.options.include_textures {
            SceneTextureSet::from_model(
                self.studio.collection(),
                &model,
                BackendImageFormat::Png,
                SceneTextureLimits {
                    maximum_total_encoded_bytes: remaining.min(MAX_EXPORT_FILE_BYTES),
                    ..SceneTextureLimits::default()
                },
            )?
        } else {
            SceneTextureSet::default()
        };
        let texture_bytes = textures.textures.iter().try_fold(0u64, |sum, texture| {
            sum.checked_add(texture.encoded.len() as u64)
                .context("texture size overflow")
        })?;
        let model_budget = remaining
            .checked_sub(texture_bytes)
            .context("model output budget exhausted")?
            .min(MAX_EXPORT_FILE_BYTES);
        let stage = tempfile::Builder::new()
            .prefix(".assetstudio-model-")
            .tempdir_in(destination.root())?;
        let extension = if format == ModelFormat::Obj {
            "obj"
        } else {
            "fbx"
        };
        let model_name = format!("{name}.{extension}");
        let mut file = File::create(stage.path().join(&model_name))?;
        let mut extra_names = Vec::new();
        match format {
            ModelFormat::FbxAscii | ModelFormat::FbxBinary => {
                let animations = if budget.options.include_animations {
                    let graph = build_animation_graph(
                        self.studio.collection(),
                        &hierarchy_for_animation(self, &model)?,
                        AnimationGraphLimits::default(),
                    )?;
                    let mut animations = build_model_animations(
                        self.studio.collection(),
                        &model,
                        &graph,
                        ModelAnimationLimits {
                            maximum_keyframes: 2_000_000,
                            maximum_total_sample_values: 16 * 1024 * 1024,
                            ..ModelAnimationLimits::default()
                        },
                    )?;
                    crate::model_motion::repair_legacy_roots(
                        self,
                        &model,
                        &graph,
                        &mut animations,
                    )?;
                    apply_animation_options(&mut animations, &model, &budget.options)?;
                    animations
                } else {
                    unity_rs_core::model_animation::ModelAnimationSet::default()
                };
                if format == ModelFormat::FbxAscii {
                    write_model_ir_fbx_ascii_with_settings(
                        &model,
                        &animations,
                        &textures,
                        &budget.options.fbx,
                        &mut file,
                        model_budget,
                    )?;
                } else {
                    write_model_ir_fbx_binary_full_with_settings(
                        &model,
                        Some(&animations),
                        Some(&textures),
                        &budget.options.fbx,
                        &mut file,
                        model_budget,
                    )?;
                }
            }
            ModelFormat::Obj => {
                let material_name = format!("{name}.mtl");
                write_model_ir_obj(&model, Some(&material_name), &mut file, model_budget)?;
                let mut material = File::create(stage.path().join(&material_name))?;
                write_model_ir_mtl(&model, &textures, &mut material, model_budget)?;
                material.flush()?;
                material.sync_all()?;
                extra_names.push(material_name);
            }
        }
        file.flush()?;
        file.sync_all()?;
        drop(file);
        for texture in &textures.textures {
            // The backend already validates names; reject separators again at the application boundary.
            let path = Path::new(&texture.file_name);
            if path.components().count() != 1 || path.file_name().is_none() {
                bail!("invalid model texture filename");
            }
            let mut image = File::create(stage.path().join(path))?;
            image.write_all(&texture.encoded)?;
            image.sync_all()?;
            extra_names.push(texture.file_name.clone());
        }
        let mut package_bytes = 0u64;
        for entry in fs::read_dir(stage.path())? {
            package_bytes = package_bytes
                .checked_add(entry?.metadata()?.len())
                .context("model package size overflow")?;
        }
        if package_bytes > remaining {
            bail!("model package exceeds cumulative output limit");
        }
        // A complete directory is published together, so existing textures never mix with a new model.
        fs::rename(stage.path(), &target).context("publish model package")?;
        budget.used = budget
            .used
            .checked_add(package_bytes)
            .context("model output size overflow")?;
        report.written.push(target.join(model_name));
        report
            .written
            .extend(extra_names.into_iter().map(|file| target.join(file)));
        report
            .failed
            .extend(textures.skipped.into_iter().map(|entry| {
                format!(
                    "model {name:?} texture {:?}: {}",
                    entry.property, entry.reason
                )
            }));
        Ok(())
    }
}

fn hierarchy_for_animation(
    session: &Session,
    model: &ModelIr,
) -> Result<unity_rs_core::scene_hierarchy::SceneHierarchy> {
    let roots: Vec<_> = model
        .nodes
        .iter()
        .filter(|node| {
            node.export_content
                && node
                    .parent
                    .and_then(|parent| model.node(parent))
                    .is_none_or(|parent| !parent.export_content)
        })
        .map(|node| node.object)
        .collect();
    Ok(build_scene_hierarchy_for_roots(
        session.studio.collection(),
        &roots,
        session.scene_limits(),
    )?)
}

fn model_package_name(name: &str, key: SceneObjectKey) -> String {
    // Prefab roots can be named with a complete asset path. Repeating that
    // path as both directory and filename exceeds limits in FBX importers.
    let leaf = name
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(name);
    let stem = leaf
        .rsplit_once('.')
        .filter(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "prefab" | "fbx" | "obj"
            )
        })
        .map_or(leaf, |(stem, _)| stem);
    let mut name = component(stem);
    let mut end = name.len().min(48);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
    format!("{}_{}_{}", component(&name), key.file_index, key.path_id)
}

pub(crate) fn resolve_model_root(session: &Session, key: AssetKey) -> Result<SceneObjectKey> {
    use unity_rs_core::scene::{SceneReadLimits, read_animator, resolve_object_reference};
    use unity_rs_core::serialized::ObjectReference;
    let collection = session.studio.collection();
    let target = resolve_object_reference(
        collection,
        key.file_index,
        ObjectReference {
            file_id: 0,
            path_id: key.path_id,
        },
    )?
    .context("selected model root is not loaded")?;
    let target = if target.object.class_id == 95 {
        let animator = read_animator(target.file, target.object_index, SceneReadLimits::default())?;
        resolve_object_reference(
            collection,
            target.file_index,
            animator.behaviour.component.game_object,
        )?
        .context("selected Animator has no loaded GameObject")?
    } else {
        target
    };
    if target.object.class_id != 1 {
        bail!("selected model root is not a GameObject or Animator");
    }
    Ok(SceneObjectKey {
        file_index: target.file_index,
        path_id: target.object.path_id,
    })
}

#[cfg(test)]
mod package_name_tests {
    use super::*;

    #[test]
    fn names_use_the_prefab_leaf_stay_bounded_and_retain_unique_identity() {
        let key = SceneObjectKey {
            file_index: 117,
            path_id: 8520,
        };
        assert_eq!(
            model_package_name(
                "assets/prefabs/skins/rocket_launcher_dragon.viewmodel.prefab",
                key
            ),
            "rocket_launcher_dragon.viewmodel_117_8520"
        );
        assert_eq!(
            model_package_name(r"assets\models\Cube.FBX", key),
            "Cube_117_8520"
        );
        let long = model_package_name(&"模型".repeat(60), key);
        assert!(long.len() <= 48 + "_117_8520".len());
        assert!(long.ends_with("_117_8520"));
        assert_ne!(
            long,
            model_package_name(
                &"模型".repeat(60),
                SceneObjectKey {
                    file_index: 118,
                    ..key
                }
            )
        );
    }
}
