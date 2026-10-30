//! Facepunch material metadata and conservative catalog-wide Texture2D recovery.
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use unity_rs_core::{
    image_export::{ImageFormat, ImageRowOrder, write_rgba_image},
    model_ir::ModelIr,
    scene_hierarchy::SceneObjectKey,
    scene_textures::{SceneTexture, SceneTextureSet},
    texture::TextureReadLimits,
};

use crate::{AssetRecord, Session};

const CHANNELS: [&str; 5] = ["albedo", "specular", "emission", "normal", "occlusion"];

fn channel(name: &str) -> Option<(&'static str, bool)> {
    let name = name.to_ascii_lowercase();
    Some(match name.trim_start_matches('_') {
        "maintex" | "basemap" | "basecolormap" | "albedo" | "diff" | "dif" | "color"
        | "diffuse" | "basecolor" | "red" | "white" | "blue" => ("albedo", false),
        "specglossmap" | "specular" | "spec" | "specgloss" => ("specular", false),
        "metallicglossmap" | "metal" | "metalness" | "metallic" | "metalrough"
        | "metallicroughness" | "metg" => ("specular", true),
        "emissionmap" | "emission" | "emissive" | "em" => ("emission", false),
        "bumpmap" | "normalmap" | "normal" | "normals" | "nrm" | "norm" => ("normal", false),
        "occlusionmap" | "occlusion" | "ao" => ("occlusion", false),
        _ => return None,
    })
}

fn stem(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let name = name.strip_suffix(" (Instance)").unwrap_or(name);
    let name = name
        .rsplit_once('.')
        .filter(|(_, ext)| matches!(*ext, "png" | "tga" | "mat" | "jpg"))
        .map_or(name, |(stem, _)| stem);
    name.to_ascii_lowercase()
}

fn texture_identity(name: &str) -> Option<(String, &'static str, bool)> {
    let name = stem(name);
    // Workshop skins use _MainTex_<skin id>, _SpecGlossMap_<skin id>, etc.
    for property in [
        "_maintex",
        "_specglossmap",
        "_metallicglossmap",
        "_emissionmap",
        "_bumpmap",
        "_occlusionmap",
    ] {
        if let Some(tail) = name.strip_prefix(property).filter(|tail| !tail.is_empty()) {
            let (slot, metallic) = channel(property)?;
            return Some((format!("skin:{tail}"), slot, metallic));
        }
    }
    let (base, suffix) = name.rsplit_once(['_', '-', ' '])?;
    let (slot, metallic) = channel(suffix)?;
    Some((base.to_owned(), slot, metallic))
}

pub(crate) struct TextureCatalog<'a> {
    by_base: HashMap<String, Vec<(&'a AssetRecord, &'static str, bool)>>,
    by_key: HashMap<SceneObjectKey, &'a AssetRecord>,
}

impl<'a> TextureCatalog<'a> {
    pub(crate) fn new(session: &'a Session) -> Self {
        let mut by_base: HashMap<String, Vec<_>> = HashMap::new();
        let mut by_key = HashMap::new();
        for record in session
            .assets()
            .iter()
            .filter(|record| record.class_id == 28)
        {
            by_key.insert(
                SceneObjectKey {
                    file_index: record.key.file_index,
                    path_id: record.key.path_id,
                },
                record,
            );
            if let Some((base, slot, metallic)) = texture_identity(&record.name) {
                by_base
                    .entry(base)
                    .or_default()
                    .push((record, slot, metallic));
            }
        }
        Self { by_base, by_key }
    }

    pub(crate) fn manifest(
        &self,
        session: &Session,
        model: &mut ModelIr,
        textures: &mut SceneTextureSet,
        maximum_bytes: u64,
    ) -> Result<Value> {
        let mut materials = Vec::new();
        let mut encoded_bytes: u64 = textures
            .textures
            .iter()
            .map(|t| t.encoded.len() as u64)
            .sum();
        for material in &mut model.materials {
            let original_name = material.material.name.clone();
            let mut channels = serde_json::Map::new();
            let mut bases = vec![stem(&original_name)];
            let mut metallic = None;
            let mut warnings = Vec::new();
            for binding in textures.bindings_for(material.object) {
                let texture = &textures.textures[binding.texture];
                if let Some((slot, is_metallic)) = channel(&binding.property)
                    && !channels.contains_key(slot)
                {
                    if let Some(record) = self.by_key.get(&texture.object)
                        && let Some((base, _, _)) = texture_identity(&record.name)
                    {
                        if slot == "albedo" {
                            bases.retain(|candidate| candidate != &base);
                            bases.insert(0, base);
                        } else if !bases.contains(&base) {
                            bases.push(base);
                        }
                    }
                    if slot == "specular" {
                        metallic = Some(is_metallic);
                    }
                    channels.insert(
                        slot.to_owned(),
                        json!({
                            "file": texture.file_name, "property": binding.property,
                            "scale": binding.scale, "offset": binding.offset, "source": "reference"
                        }),
                    );
                }
            }
            for slot in CHANNELS {
                if channels.contains_key(slot) {
                    continue;
                }
                for base in &bases {
                    let candidates: Vec<_> = self
                        .by_base
                        .get(base)
                        .into_iter()
                        .flatten()
                        .filter(|(_, candidate_slot, _)| *candidate_slot == slot)
                        .collect();
                    if candidates.is_empty() {
                        continue;
                    }
                    let local: Vec<_> = candidates
                        .iter()
                        .copied()
                        .filter(|(record, _, _)| {
                            record.key.file_index == material.object.file_index
                        })
                        .collect();
                    let candidates = if local.is_empty() { candidates } else { local };
                    if candidates.len() != 1 {
                        warnings.push(format!(
                            "{slot}: {} catalog matches for {base:?}; left unresolved",
                            candidates.len()
                        ));
                        break;
                    }
                    let (record, _, is_metallic) = candidates[0];
                    let object = SceneObjectKey {
                        file_index: record.key.file_index,
                        path_id: record.key.path_id,
                    };
                    let existing = textures.textures.iter().position(|t| t.object == object);
                    let index = if let Some(index) = existing {
                        index
                    } else {
                        if textures.textures.len() >= 4096 {
                            warnings.push("Texture count limit reached".into());
                            break;
                        }
                        let result = (|| -> Result<_> {
                            let image = session
                                .object(record.key)?
                                .decode_texture_mip(0, TextureReadLimits::default())?;
                            let mut encoded = Vec::new();
                            write_rgba_image(
                                &image,
                                ImageFormat::Png,
                                ImageRowOrder::UnityDecoded,
                                maximum_bytes
                                    .saturating_sub(encoded_bytes)
                                    .min(crate::session::MAX_EXPORT_FILE_BYTES),
                                &mut encoded,
                            )?;
                            encoded_bytes = encoded_bytes
                                .checked_add(encoded.len() as u64)
                                .context("texture size overflow")?;
                            let name = format!(
                                "recovered_{}_{}.png",
                                record.key.file_index, record.key.path_id
                            );
                            Ok(textures.push_texture(SceneTexture {
                                file_name: name,
                                object,
                                encoded,
                            })?)
                        })();
                        match result {
                            Ok(index) => index,
                            Err(error) => {
                                warnings.push(format!("{}: {error:#}", record.name));
                                break;
                            }
                        }
                    };
                    let transform = material
                        .material
                        .saved_properties
                        .texture_environments
                        .iter()
                        .find(|p| channel(&p.name).is_some_and(|(s, _)| s == slot));
                    let scale = transform.map_or([1.0, 1.0], |p| p.value.scale);
                    let offset = transform.map_or([0.0, 0.0], |p| p.value.offset);
                    if slot == "specular" {
                        metallic = Some(*is_metallic);
                    }
                    channels.insert(slot.into(), json!({"file":textures.textures[index].file_name,"source":"catalog","asset_name":record.name,"scale":scale,"offset":offset}));
                    break;
                }
            }
            let unresolved: Vec<_> = CHANNELS
                .into_iter()
                .filter(|slot| !channels.contains_key(*slot))
                .collect();
            // A recovered channel supersedes its failed original pointer.
            textures.skipped.retain(|entry| {
                entry.material != material.object
                    || channel(&entry.property).is_none_or(|(slot, _)| !channels.contains_key(slot))
            });
            let fbx_name = format!(
                "ASMaterial_{}_{}",
                material.object.file_index, material.object.path_id
            );
            material.material.name.clone_from(&fbx_name);
            let colors: HashMap<_, _> = material
                .material
                .saved_properties
                .colors
                .iter()
                .map(|p| (p.name.clone(), p.value))
                .collect();
            let floats: HashMap<_, _> = material
                .material
                .saved_properties
                .floats
                .iter()
                .map(|p| (p.name.clone(), p.value))
                .collect();
            let metallic = metallic.unwrap_or_else(|| {
                !material
                    .material
                    .saved_properties
                    .texture_environments
                    .iter()
                    .any(|p| p.name == "_SpecGlossMap")
            });
            materials.push(json!({"name":original_name,"fbx_name":fbx_name,"workflow":if metallic {"metallic"} else {"specular"},
                "unity_normal":true,"channels":channels,"colors":colors,"floats":floats,"unresolved":unresolved,"warnings":warnings}));
        }
        // Keep diagnostics deterministic even if several references pointed at one texture.
        let mut seen = HashSet::new();
        textures
            .skipped
            .retain(|s| seen.insert((s.material, s.property.clone(), s.reason.clone())));
        Ok(json!({"version":1,"game":"facepunch-rust","materials":materials}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_require_a_known_channel_and_keep_complete_base() {
        assert_eq!(
            texture_identity("assets/wood_wall_metalrough.png"),
            Some(("wood_wall".into(), "specular", true))
        );
        assert_eq!(
            texture_identity("DragonLauncher_body_Normal"),
            Some(("dragonlauncher_body".into(), "normal", false))
        );
        assert_eq!(texture_identity("some_wood_detail"), None);
        assert_eq!(channel("_SpecGlossMap"), Some(("specular", false)));
        assert_eq!(
            texture_identity("_MainTex_12345"),
            Some(("skin:_12345".into(), "albedo", false))
        );
        assert_eq!(
            texture_identity("_SpecGlossMap_12345"),
            Some(("skin:_12345".into(), "specular", false))
        );
    }
}
