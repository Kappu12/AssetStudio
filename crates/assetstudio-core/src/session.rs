use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use unity_rs_core::{
    extraction::{ExtractionOptions, extract_path, extract_region},
    image_export::flip_rgba_rows,
    loader::{AssetCollection, AssetCollectionParts, AssetLoadOptions, LoadFailurePolicy},
    mesh::MeshReadLimits,
    scene_hierarchy::SceneHierarchyLimits,
    simple_assets::{SimpleAssetReadLimits, write_direct_wav},
    sprite::SpriteReadLimits,
    studio::{Studio, StudioObject},
    texture::TextureReadLimits,
    unity_cn::UnityCnKey,
};

use crate::{AssetKey, AssetRecord, LoadOptions, OperationReport, Preview, SceneNode, class_name};

const MAX_CATALOG_OBJECTS: usize = 1_000_000;
const MAX_CATALOG_TEXT_BYTES: usize = 256 * 1024 * 1024;
const MAX_PREVIEW_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PREVIEW_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_AUDIO_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_EXPORT_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_EXPORT_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug)]
pub struct Session {
    pub(crate) studio: Studio,
    assets: Vec<AssetRecord>,
    by_key: HashMap<AssetKey, usize>,
    diagnostics: Vec<String>,
    pub(crate) xor_key: u8,
    pub(crate) mono_schemas: unity_rs_core::mono_schema::MonoBehaviourSchemaRegistry,
    input_paths: Vec<PathBuf>,
}

impl Session {
    /// Opens the explicit file/folder set as one reference-resolving collection.
    /// Companion resources and split segments use the backend's file discovery;
    /// unrelated assets beside a selected file are never loaded implicitly.
    pub fn open_paths(inputs: &[PathBuf], options: &LoadOptions) -> Result<Self> {
        if inputs.is_empty() {
            bail!("no input paths were selected");
        }
        if inputs.len() == 1 {
            return Self::open(&inputs[0], options);
        }
        options.validate()?;
        let load = backend_options(options)?;
        if inputs.len() > load.limits.maximum_input_files {
            bail!("input file count exceeds the loader limit");
        }
        let mut parts = AssetCollectionParts::default();
        let mut input_paths = HashSet::new();
        let mut canonical_inputs = Vec::new();
        let mut files = HashSet::new();
        let mut resources = HashSet::new();
        let mut expanded = 0u64;
        let mut objects = 0usize;
        for input in inputs {
            let canonical = std::fs::canonicalize(input)
                .with_context(|| format!("resolve selected input {input:?}"))?;
            if !input_paths.insert(canonical.clone()) {
                continue;
            }
            canonical_inputs.push(canonical.clone());
            let result = match crate::games::prepare_regions(
                &canonical,
                &options.game,
                MAX_EXPORT_FILE_BYTES,
            )? {
                Some(regions) => AssetCollection::load_regions_with_options(regions, load.clone()),
                None => AssetCollection::load_path_with_options(&canonical, load.clone()),
            };
            let collection = match result {
                Ok(value) => value,
                Err(error) if options.continue_on_error => {
                    parts
                        .diagnostics
                        .push(unity_rs_core::loader::LoadDiagnostic {
                            path: canonical.to_string_lossy().into_owned(),
                            message: error.to_string(),
                        });
                    continue;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("open selected input {canonical:?}"));
                }
            };
            let next = collection.into_parts();
            for file in next.serialized_files {
                if !files.insert(file.path.clone()) {
                    continue;
                }
                expanded = expanded
                    .checked_add(file.file.header.file_size)
                    .context("multi-file input size overflow")?;
                objects = objects
                    .checked_add(file.file.objects.len())
                    .context("multi-file object count overflow")?;
                if objects > MAX_CATALOG_OBJECTS || expanded > load.limits.maximum_expanded_bytes {
                    bail!("multi-file input exceeds the shared object/expansion limits");
                }
                parts.serialized_files.try_reserve(1)?;
                parts.serialized_files.push(file);
            }
            for resource in next.resources {
                if !resources.insert(resource.path.clone()) {
                    continue;
                }
                expanded = expanded
                    .checked_add(resource.region.len())
                    .context("multi-file resource size overflow")?;
                if expanded > load.limits.maximum_expanded_bytes {
                    bail!("multi-file input exceeds the shared expansion limit");
                }
                parts.resources.try_reserve(1)?;
                parts.resources.push(resource);
            }
            parts.diagnostics.extend(next.diagnostics);
            if parts
                .serialized_files
                .len()
                .checked_add(parts.resources.len())
                .context("multi-file file count overflow")?
                > load.limits.maximum_discovered_files
            {
                bail!("multi-file input exceeds the discovered file limit");
            }
        }
        let mut collection = AssetCollection::from_parts(parts);
        collection.resolve_object_metadata(load.limits)?;
        let mut session = Self::from_studio(Studio::from_collection(collection), options.xor_key)?;
        session.input_paths = canonical_inputs;
        if let Some(warning) = crate::games::profile_warning(&options.game) {
            session.diagnostics.push(warning.to_owned());
        }
        Ok(session)
    }

    pub fn open(input: &Path, options: &LoadOptions) -> Result<Self> {
        crate::games::validate_profile(&options.game)?;
        let load = backend_options(options)?;
        let studio =
            match crate::games::prepare_regions(input, &options.game, MAX_EXPORT_FILE_BYTES)? {
                Some(regions) => Studio::open_regions_with_options(regions, load),
                None => Studio::open_with_options(input, load),
            }
            .with_context(|| format!("open Unity input {input:?}"))?;
        let mut session = Self::from_studio(studio, options.xor_key)?;
        session.input_paths = vec![std::fs::canonicalize(input)?];
        if let Some(warning) = crate::games::profile_warning(&options.game) {
            session.diagnostics.push(warning.to_owned());
        }
        Ok(session)
    }

    /// Opens caller-owned, bounded in-memory inputs, useful to embed the reader or test real layouts.
    pub fn from_regions(
        inputs: impl IntoIterator<Item = (String, unity_rs_core::source::Region)>,
        options: &LoadOptions,
    ) -> Result<Self> {
        if !options.game.eq_ignore_ascii_case("Normal")
            && !options.game.eq_ignore_ascii_case("UnityCN")
        {
            bail!(
                "from_regions accepts decoded Normal/UnityCN inputs; preprocess game containers first"
            );
        }
        Self::from_studio(
            Studio::open_regions_with_options(inputs, backend_options(options)?)?,
            options.xor_key,
        )
    }

    fn from_studio(studio: Studio, xor_key: u8) -> Result<Self> {
        if studio.object_count() == 0 {
            bail!("input contains no readable Unity serialized objects");
        }
        if studio.object_count() > MAX_CATALOG_OBJECTS {
            bail!("input exceeds the {MAX_CATALOG_OBJECTS} object catalog limit");
        }
        let mut assets = Vec::new();
        assets.try_reserve_exact(studio.object_count())?;
        let mut by_key = HashMap::new();
        by_key.try_reserve(studio.object_count())?;
        let mut text_bytes = 0usize;
        for object in studio.objects() {
            let key = AssetKey {
                file_index: object.file_index(),
                path_id: object.path_id(),
            };
            let record = AssetRecord {
                key,
                class_id: object.class_id(),
                class_name: class_name(object.class_id()).to_owned(),
                name: bounded_name(object.name().unwrap_or("")),
                container: bounded_name(object.container().unwrap_or("")),
                source: bounded_name(object.source_path()),
                size: object.byte_size(),
            };
            for value in [
                &record.class_name,
                &record.name,
                &record.container,
                &record.source,
            ] {
                text_bytes = text_bytes
                    .checked_add(value.len())
                    .context("catalog metadata length overflow")?;
            }
            if text_bytes > MAX_CATALOG_TEXT_BYTES {
                bail!("catalog metadata exceeds 256 MiB");
            }
            if by_key.insert(key, assets.len()).is_some() {
                bail!("duplicate object identity {key:?}");
            }
            assets.push(record);
        }
        let diagnostics: Vec<String> = studio
            .load_diagnostics()
            .iter()
            .map(|entry| format!("{}: {}", entry.path, entry.message))
            .collect();
        let mut session = Self {
            studio,
            assets,
            by_key,
            diagnostics,
            xor_key,
            mono_schemas: unity_rs_core::mono_schema::MonoBehaviourSchemaRegistry::default(),
            input_paths: Vec::new(),
        };
        session.resolve_index_metadata()?;
        Ok(session)
    }

    pub fn assets(&self) -> &[AssetRecord] {
        &self.assets
    }
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    pub fn input_paths(&self) -> &[PathBuf] {
        &self.input_paths
    }

    /// Loads a trusted, non-executable JSON schema document for stripped MonoBehaviours.
    pub fn load_schemas(&mut self, path: &Path) -> Result<usize> {
        crate::schema::extend_registry(&mut self.mono_schemas, path)
    }

    pub fn recover_containers(&mut self, index_path: &Path) -> Result<usize> {
        crate::recovery::AssetIndex::from_path(index_path)?.apply(&mut self.assets)
    }

    fn resolve_index_metadata(&mut self) -> Result<()> {
        let keys: Vec<_> = self
            .assets
            .iter()
            .filter(|asset| asset.class_id == 1210)
            .map(|asset| asset.key)
            .collect();
        let mut retained = self.assets.iter().try_fold(0usize, |size, asset| {
            size.checked_add(
                asset.name.len()
                    + asset.container.len()
                    + asset.source.len()
                    + asset.class_name.len(),
            )
            .context("metadata byte overflow")
        })?;
        let mut added_diagnostics = 0usize;
        for key in keys {
            let decoded = match crate::export::read_index_data(self, key) {
                Ok(decoded) => decoded,
                Err(error) => {
                    if added_diagnostics < 10_000 {
                        self.diagnostics
                            .push(format!("IndexObject {key:?}: {error:#}"));
                        added_diagnostics += 1;
                    }
                    continue;
                }
            };
            if let Some(&index) = self.by_key.get(&key)
                && self.assets[index].name.is_empty()
            {
                let name = bounded_name(&decoded.name);
                retained = retained
                    .checked_add(name.len())
                    .context("IndexObject name size overflow")?;
                if retained > MAX_CATALOG_TEXT_BYTES {
                    bail!("IndexObject metadata exceeds catalog byte limit");
                }
                self.assets[index].name = name;
            }
            for entry in decoded.entries {
                let reference = unity_rs_core::serialized::ObjectReference {
                    file_id: entry.file_id,
                    path_id: entry.path_id,
                };
                let resolved = match unity_rs_core::scene::resolve_object_reference(
                    self.studio.collection(),
                    key.file_index,
                    reference,
                ) {
                    Ok(Some(resolved)) => AssetKey {
                        file_index: resolved.file_index,
                        path_id: resolved.object.path_id,
                    },
                    Ok(None) => continue,
                    Err(error) => {
                        if added_diagnostics < 10_000 {
                            self.diagnostics
                                .push(format!("IndexObject {key:?} reference: {error}"));
                            added_diagnostics += 1;
                        }
                        continue;
                    }
                };
                let Some(&index) = self.by_key.get(&resolved) else {
                    continue;
                };
                let container = bounded_name(&entry.name);
                let name = if self.assets[index].name.is_empty() {
                    container
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or("")
                        .to_owned()
                } else {
                    self.assets[index].name.clone()
                };
                retained = retained
                    .checked_sub(self.assets[index].container.len() + self.assets[index].name.len())
                    .context("metadata accounting underflow")?
                    .checked_add(container.len() + name.len())
                    .context("metadata accounting overflow")?;
                if retained > MAX_CATALOG_TEXT_BYTES {
                    bail!("IndexObject metadata exceeds catalog byte limit");
                }
                self.assets[index].container = container;
                self.assets[index].name = name;
            }
        }
        Ok(())
    }

    pub(crate) fn object(&self, key: AssetKey) -> Result<StudioObject<'_>> {
        self.studio
            .object(key.file_index, key.path_id)
            .with_context(|| format!("unknown asset {key:?}"))
    }
    pub(crate) fn record(&self, key: AssetKey) -> Result<&AssetRecord> {
        self.by_key
            .get(&key)
            .and_then(|&index| self.assets.get(index))
            .with_context(|| format!("unknown asset {key:?}"))
    }
    pub(crate) fn selected(&self, keys: &[AssetKey]) -> Result<Vec<&AssetRecord>> {
        if keys.len() > MAX_CATALOG_OBJECTS {
            bail!("selection exceeds the object limit");
        }
        let mut seen = HashSet::new();
        seen.try_reserve(keys.len())?;
        let mut result = Vec::new();
        result.try_reserve(keys.len())?;
        for &key in keys {
            if seen.insert(key) {
                result.push(self.record(key)?);
            }
        }
        Ok(result)
    }

    pub fn preview(&self, key: AssetKey) -> Result<Preview> {
        let object = self.object(key)?;
        match object.class_id() {
            28 => {
                let mut image = object.decode_texture_mip(0, preview_texture_limits())?;
                flip_rgba_rows(&mut image)?;
                Ok(Preview::Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.pixels,
                })
            }
            213 => {
                let image =
                    object.decode_sprite(SpriteReadLimits::default(), preview_texture_limits())?;
                Ok(Preview::Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.pixels,
                })
            }
            43 => {
                let limits = MeshReadLimits {
                    maximum_vertices: 100_000,
                    maximum_indices: 300_000,
                    maximum_decoded_bytes: 64 * 1024 * 1024,
                    maximum_vertex_data_bytes: 64 * 1024 * 1024,
                    maximum_output_bytes: 64 * 1024 * 1024,
                    ..MeshReadLimits::default()
                };
                let mesh = object.read_mesh(limits)?.mesh;
                let mut indices = Vec::new();
                for submesh in &mesh.sub_meshes {
                    if indices
                        .len()
                        .checked_add(submesh.indices.len())
                        .context("preview index count overflow")?
                        > 300_000
                    {
                        bail!("mesh preview exceeds 300,000 indices");
                    }
                    indices.try_reserve(submesh.indices.len())?;
                    indices.extend_from_slice(&submesh.indices);
                }
                Ok(Preview::Mesh {
                    vertices: mesh.vertices,
                    indices,
                })
            }
            49 => Ok(Preview::Text(
                String::from_utf8_lossy(&object.read_text_bytes(MAX_PREVIEW_TEXT_BYTES)?)
                    .into_owned(),
            )),
            48 => Ok(Preview::Text(
                String::from_utf8_lossy(&object.read_shader_text(MAX_PREVIEW_TEXT_BYTES as u64)?)
                    .into_owned(),
            )),
            114 | 1210 => Ok(Preview::Text(String::from_utf8(
                self.object_json_with_limit(key, MAX_PREVIEW_TEXT_BYTES)?,
            )?)),
            1208 => Ok(Preview::Text(
                String::from_utf8_lossy(&crate::export::read_bin_data(
                    self,
                    key,
                    MAX_PREVIEW_TEXT_BYTES as u64,
                )?)
                .into_owned(),
            )),
            _ => match object.read_type_tree_json(true, MAX_PREVIEW_TEXT_BYTES) {
                Ok(bytes) => Ok(Preview::Text(String::from_utf8_lossy(&bytes).into_owned())),
                Err(error) => {
                    let metadata = serde_json::to_string_pretty(self.record(key)?)?;
                    Ok(Preview::Text(format!(
                        "{metadata}\n\nStructured preview unavailable: {error}"
                    )))
                }
            },
        }
    }

    /// Decodes supported audio to a bounded self-contained WAV for desktop playback.
    pub fn audio_preview(&self, key: AssetKey) -> Result<Vec<u8>> {
        let object = self.object(key)?;
        let clip = object.read_audio_clip(SimpleAssetReadLimits {
            maximum_payload_bytes: MAX_PREVIEW_AUDIO_BYTES,
            ..SimpleAssetReadLimits::default()
        })?;
        let kind = clip
            .direct_wav
            .context("this audio codec has no bundled Rust WAV decoder")?;
        let mut output = crate::export::LimitedBytes::new(MAX_PREVIEW_AUDIO_BYTES);
        write_direct_wav(&clip.payload, kind, MAX_PREVIEW_AUDIO_BYTES, &mut output)?;
        Ok(output.into_inner())
    }

    pub fn scene(&self) -> Result<Vec<SceneNode>> {
        let hierarchy = self
            .studio
            .scene_hierarchy(SceneHierarchyLimits::default())?;
        Ok(hierarchy
            .nodes
            .into_iter()
            .map(|node| SceneNode {
                key: AssetKey {
                    file_index: node.object.file_index,
                    path_id: node.object.path_id,
                },
                name: node.name,
                parent: node.parent.map(|parent| AssetKey {
                    file_index: parent.file_index,
                    path_id: parent.path_id,
                }),
            })
            .collect())
    }

    pub fn extract(
        input: &Path,
        output: &Path,
        options: &LoadOptions,
        overwrite: bool,
    ) -> Result<OperationReport> {
        crate::games::validate_profile(&options.game)?;
        let load = backend_options(options)?;
        let mut extraction = ExtractionOptions {
            overwrite_existing: overwrite,
            unity_cn_key: load.unity_cn_key,
            ..ExtractionOptions::default()
        };
        let mut result = OperationReport::default();
        match crate::games::prepare_regions(input, &options.game, MAX_EXPORT_FILE_BYTES)? {
            Some(regions) => {
                let mut written = 0u64;
                for (label, region) in regions {
                    extraction.limits.maximum_output_bytes = MAX_EXPORT_TOTAL_BYTES
                        .checked_sub(written)
                        .context("extraction output budget exceeded")?;
                    let report = extract_region(&label, region, output, extraction.clone())?;
                    written = written
                        .checked_add(report.output_bytes)
                        .context("extraction size overflow")?;
                    append_extraction(&mut result, report);
                }
            }
            None => append_extraction(&mut result, extract_path(input, output, extraction)?),
        }
        Ok(result)
    }
}

fn append_extraction(
    result: &mut OperationReport,
    report: unity_rs_core::extraction::ExtractionReport,
) {
    result
        .written
        .extend(report.extracted.into_iter().map(|entry| entry.output_path));
    result.skipped.extend(
        report
            .skipped_existing
            .into_iter()
            .map(|entry| format!("existing output: {}", entry.output_path.display())),
    );
    result.failed.extend(
        report
            .failures
            .into_iter()
            .map(|entry| format!("{}: {}", entry.source, entry.error)),
    );
}

fn bounded_name(value: &str) -> String {
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|&index| index <= 4096)
        .last()
        .unwrap_or(0);
    if value.len() <= 4096 {
        value.to_owned()
    } else {
        value[..end].to_owned()
    }
}

pub(crate) fn backend_options(options: &LoadOptions) -> Result<AssetLoadOptions> {
    let unity_version_override = options
        .unity_version
        .as_deref()
        .map(str::parse)
        .transpose()
        .context("invalid Unity version override")?;
    let unity_cn_key = options
        .unity_cn_key
        .as_deref()
        .map(|key| {
            let key = key.strip_prefix("0x").unwrap_or(key);
            if key.len() != 32 || !key.is_ascii() {
                bail!("UnityCN key must be exactly 32 hexadecimal characters");
            }
            let mut bytes = [0u8; 16];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&key[index * 2..index * 2 + 2], 16)
                    .context("UnityCN key contains a non-hexadecimal character")?;
            }
            Ok(UnityCnKey::new(bytes))
        })
        .transpose()?;
    if options.game.eq_ignore_ascii_case("UnityCN") && unity_cn_key.is_none() {
        bail!("UnityCN requires a 16-byte --unity-cn-key");
    }
    Ok(AssetLoadOptions {
        unity_version_override,
        unity_cn_key,
        failure_policy: if options.continue_on_error {
            LoadFailurePolicy::SkipInput
        } else {
            LoadFailurePolicy::Abort
        },
        ..AssetLoadOptions::default()
    })
}

fn preview_texture_limits() -> TextureReadLimits {
    TextureReadLimits {
        maximum_dimension: 4096,
        maximum_output_bytes: MAX_PREVIEW_IMAGE_BYTES,
        maximum_decoder_working_bytes: MAX_PREVIEW_IMAGE_BYTES,
        ..TextureReadLimits::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_can_be_shared_with_ui_worker() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Session>();
    }
    #[test]
    fn key_and_version_validation_happens_before_io() {
        let mut options = LoadOptions {
            unity_cn_key: Some("bad".into()),
            ..LoadOptions::default()
        };
        assert!(backend_options(&options).is_err());
        options.unity_cn_key = Some("11".repeat(16));
        assert!(backend_options(&options).is_ok());
        options.unity_version = Some("not-a-version".into());
        assert!(backend_options(&options).is_err());
    }
    #[test]
    fn bounded_names_do_not_cut_utf8() {
        let name = "😀".repeat(2000);
        assert!(bounded_name(&name).len() <= 4096);
    }
}
