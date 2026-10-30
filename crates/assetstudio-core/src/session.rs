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
    warnings: Vec<String>,
    pub(crate) xor_key: u8,
    pub(crate) mono_schemas: unity_rs_core::mono_schema::MonoBehaviourSchemaRegistry,
    input_paths: Vec<PathBuf>,
    protected_sources: crate::source_protection::SourceProtection,
    maximum_assets: usize,
    maximum_catalog_text_bytes: usize,
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
        let mut game_budget = crate::games::GamePreparationBudget::new(MAX_EXPORT_FILE_BYTES)?;
        for input in inputs {
            let canonical = match std::fs::canonicalize(input) {
                Ok(path) => path,
                Err(error) if options.continue_on_error => {
                    parts
                        .diagnostics
                        .push(unity_rs_core::loader::LoadDiagnostic {
                            path: input.to_string_lossy().into_owned(),
                            message: format!("resolve selected input: {error}"),
                        });
                    continue;
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("resolve selected input {input:?}"));
                }
            };
            if !input_paths.insert(canonical.clone()) {
                continue;
            }
            canonical_inputs.push(canonical.clone());
            let mut preprocess_diagnostics = Vec::new();
            let result: Result<AssetCollection> = (|| {
                let (prepared, diagnostics) = crate::games::prepare_regions_with_budget(
                    &canonical,
                    &options.game,
                    &mut game_budget,
                    options.continue_on_error,
                )?;
                preprocess_diagnostics = diagnostics;
                Ok(match prepared {
                    Some(regions) => {
                        AssetCollection::load_regions_with_options(regions, load.clone())?
                    }
                    None => AssetCollection::load_path_with_options(&canonical, load.clone())?,
                })
            })();
            let collection = match result {
                Ok(value) => value,
                Err(error)
                    if options.continue_on_error && !crate::games::is_preparation_limit(&error) =>
                {
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
            for message in preprocess_diagnostics {
                parts
                    .diagnostics
                    .push(unity_rs_core::loader::LoadDiagnostic {
                        path: canonical.to_string_lossy().into_owned(),
                        message,
                    });
            }
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
                if objects > options.maximum_assets || expanded > load.limits.maximum_expanded_bytes
                {
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
        let mut session = Self::from_studio(Studio::from_collection(collection), options)?;
        for path in &canonical_inputs {
            session.protected_sources.include_input(path)?;
        }
        session.input_paths = canonical_inputs;
        if let Some(warning) = crate::games::profile_warning(&options.game) {
            session.warnings.push(warning.to_owned());
        }
        Ok(session)
    }

    pub fn open(input: &Path, options: &LoadOptions) -> Result<Self> {
        crate::games::validate_profile(&options.game)?;
        let load = backend_options(options)?;
        let (prepared, preprocess_diagnostics) = crate::games::prepare_regions_with_policy(
            input,
            &options.game,
            MAX_EXPORT_FILE_BYTES,
            options.continue_on_error,
        )?;
        let studio = match prepared {
            Some(regions) => Studio::open_regions_with_options(regions, load),
            None => Studio::open_with_options(input, load),
        }
        .with_context(|| format!("open Unity input {input:?}"))?;
        let mut session = Self::from_studio(studio, options)?;
        session.protected_sources.include_input(input)?;
        session.input_paths = vec![std::fs::canonicalize(input)?];
        session.diagnostics.extend(preprocess_diagnostics);
        if let Some(warning) = crate::games::profile_warning(&options.game) {
            session.warnings.push(warning.to_owned());
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
            options,
        )
    }

    fn from_studio(studio: Studio, options: &LoadOptions) -> Result<Self> {
        if studio.object_count() == 0 {
            bail!("input contains no readable Unity serialized objects");
        }
        if studio.object_count() > options.maximum_assets {
            bail!(
                "input exceeds the {} object catalog limit; select fewer inputs or raise the asset limit",
                options.maximum_assets
            );
        }
        let maximum_catalog_text_bytes = catalog_text_budget(options.maximum_assets);
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
            if text_bytes > maximum_catalog_text_bytes {
                bail!("catalog metadata exceeds its {maximum_catalog_text_bytes} byte limit");
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
            warnings: Vec::new(),
            xor_key: options.xor_key,
            mono_schemas: unity_rs_core::mono_schema::MonoBehaviourSchemaRegistry::default(),
            input_paths: Vec::new(),
            protected_sources: crate::source_protection::SourceProtection::default(),
            maximum_assets: options.maximum_assets,
            maximum_catalog_text_bytes,
        };
        for file in session.studio.collection().serialized_files() {
            session.protected_sources.include_label(&file.path)?;
        }
        for resource in session.studio.collection().resources() {
            session.protected_sources.include_label(&resource.path)?;
        }
        session.resolve_index_metadata()?;
        Ok(session)
    }

    pub fn assets(&self) -> &[AssetRecord] {
        &self.assets
    }
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Advisories do not indicate a failed input or make an operation partial.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Refuses a destination that identifies any original input or companion resource.
    pub fn check_output_target(&self, target: &Path) -> Result<()> {
        self.protected_sources.check(target)
    }

    /// Registers an auxiliary input (schema, key file, index or DLL folder) as read-only provenance.
    pub fn protect_input_path(&mut self, path: &Path) -> Result<()> {
        self.protected_sources.include_input(path)
    }

    pub fn input_paths(&self) -> &[PathBuf] {
        &self.input_paths
    }

    /// Loads a trusted, non-executable JSON schema document for stripped MonoBehaviours.
    pub fn load_schemas(&mut self, path: &Path) -> Result<usize> {
        self.protect_input_path(path)?;
        crate::schema::extend_registry(&mut self.mono_schemas, path)
    }

    pub fn recover_containers(&mut self, index_path: &Path) -> Result<usize> {
        self.protect_input_path(index_path)?;
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
                if retained > self.maximum_catalog_text_bytes {
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
                if retained > self.maximum_catalog_text_bytes {
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
        if keys.len() > self.maximum_assets {
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
            1 | 95 => crate::model_preview::preview_model(self, key),
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
                let geometry = object.read_mesh(limits)?;
                let mesh = geometry.mesh;
                let colors = match geometry.channels.colors {
                    Some(attribute) => {
                        let dimension = attribute.dimension;
                        if !matches!(dimension, 3 | 4)
                            || attribute.values.len()
                                != mesh
                                    .vertices
                                    .len()
                                    .checked_mul(dimension)
                                    .context("preview color count overflow")?
                        {
                            bail!(
                                "mesh preview source colors must have three or four components per vertex"
                            );
                        }
                        let mut colors = Vec::new();
                        colors.try_reserve_exact(mesh.vertices.len())?;
                        for color in attribute.values.chunks_exact(dimension) {
                            if color.iter().any(|value| !value.is_finite()) {
                                bail!("mesh preview source colors contain non-finite values");
                            }
                            colors.push([
                                color[0],
                                color[1],
                                color[2],
                                color.get(3).copied().unwrap_or(1.0),
                            ]);
                        }
                        Some(colors)
                    }
                    None => None,
                };
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
                    normals: mesh.normals,
                    colors,
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
            128 => {
                let maximum = crate::font_preview::MAX_FONT_BYTES as u64;
                let font = object.read_font(SimpleAssetReadLimits {
                    maximum_payload_bytes: maximum,
                    ..SimpleAssetReadLimits::default()
                })?;
                crate::font_preview::render(&font.payload.read_to_vec(maximum)?)
            }
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

    /// Bounded serialized-object dump, including recovered MonoBehaviour schemas.
    pub fn preview_dump(&self, key: AssetKey) -> Result<String> {
        String::from_utf8(self.object_dump(key, MAX_PREVIEW_TEXT_BYTES as u64)?)
            .context("serialized dump is not UTF-8")
    }

    /// Bounded structured JSON for the selected object's complete decoded value.
    pub fn preview_json(&self, key: AssetKey) -> Result<String> {
        String::from_utf8(self.object_json_with_limit(key, MAX_PREVIEW_TEXT_BYTES)?)
            .context("serialized JSON is not UTF-8")
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

    pub fn extract_paths(
        inputs: &[PathBuf],
        output: &Path,
        options: &LoadOptions,
        overwrite: bool,
    ) -> Result<OperationReport> {
        Self::extract_paths_with_auxiliary_inputs(inputs, output, options, overwrite, &[])
    }

    pub fn extract_paths_with_auxiliary_inputs(
        inputs: &[PathBuf],
        output: &Path,
        options: &LoadOptions,
        overwrite: bool,
        auxiliary: &[PathBuf],
    ) -> Result<OperationReport> {
        if auxiliary.len() > 100_000 {
            bail!("auxiliary input count exceeds limit");
        }
        if inputs.is_empty() {
            bail!("no extraction inputs selected");
        }
        if inputs.len() > 100_000 {
            bail!("extraction input count exceeds limit");
        }
        let mut protection = crate::source_protection::SourceProtection::default();
        for path in auxiliary {
            protection.include_input(path)?;
        }
        let mut readable = Vec::new();
        let mut result = OperationReport::default();
        for (index, input) in inputs.iter().enumerate() {
            match protection.include_input(input) {
                Ok(()) => readable.push((index, input)),
                Err(error) if options.continue_on_error => {
                    result.failed.push(format!("input {input:?}: {error:#}"))
                }
                Err(error) => return Err(error),
            }
        }
        let mut remaining_output = MAX_EXPORT_TOTAL_BYTES;
        let mut game_budget = crate::games::GamePreparationBudget::new(MAX_EXPORT_FILE_BYTES)?;
        for (index, input) in readable {
            let target = if inputs.len() == 1 {
                output.to_owned()
            } else {
                output.join(format!("input-{:04}", index + 1))
            };
            match Self::extract_with_guard(
                input,
                &target,
                options,
                overwrite,
                Some(&protection),
                &mut remaining_output,
                &mut game_budget,
            ) {
                Ok(report) => {
                    result.written.extend(report.written);
                    result.skipped.extend(report.skipped);
                    result.failed.extend(report.failed);
                }
                Err(error)
                    if options.continue_on_error && !crate::games::is_preparation_limit(&error) =>
                {
                    result.failed.push(format!("input {input:?}: {error:#}"))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }

    pub fn extract(
        input: &Path,
        output: &Path,
        options: &LoadOptions,
        overwrite: bool,
    ) -> Result<OperationReport> {
        let mut remaining_output = MAX_EXPORT_TOTAL_BYTES;
        let mut game_budget = crate::games::GamePreparationBudget::new(MAX_EXPORT_FILE_BYTES)?;
        Self::extract_with_guard(
            input,
            output,
            options,
            overwrite,
            None,
            &mut remaining_output,
            &mut game_budget,
        )
    }

    fn extract_with_guard(
        input: &Path,
        output: &Path,
        options: &LoadOptions,
        overwrite: bool,
        additional: Option<&crate::source_protection::SourceProtection>,
        remaining_output: &mut u64,
        game_budget: &mut crate::games::GamePreparationBudget,
    ) -> Result<OperationReport> {
        crate::games::validate_profile(&options.game)?;
        let load = backend_options(options)?;
        let mut protection = crate::source_protection::SourceProtection::default();
        protection.include_input(input)?;
        let destination = crate::output::SafeOutput::new(output)?;
        let canonical_input = std::fs::canonicalize(input)?;
        if canonical_input.is_dir() && destination.root().starts_with(&canonical_input) {
            bail!("extraction output must be outside the input directory");
        }
        // Native extraction writes only into our private staging directory.
        // Every eventual user destination is checked before atomic publication.
        let staging = tempfile::Builder::new()
            .prefix(".assetstudio-extract-")
            .tempdir_in(destination.root())?;
        let mut extraction = ExtractionOptions {
            overwrite_existing: false,
            unity_cn_key: load.unity_cn_key,
            ..ExtractionOptions::default()
        };
        extraction.limits.maximum_output_bytes = *remaining_output;
        let mut result = OperationReport::default();
        let (prepared, preprocess_diagnostics) = crate::games::prepare_regions_with_budget(
            input,
            &options.game,
            game_budget,
            options.continue_on_error,
        )?;
        result.failed.extend(preprocess_diagnostics);
        match prepared {
            Some(regions) => {
                let mut written = 0u64;
                for (label, region) in regions {
                    extraction.limits.maximum_output_bytes = (*remaining_output)
                        .checked_sub(written)
                        .context("extraction output budget exceeded")?;
                    let report =
                        extract_region(&label, region, staging.path(), extraction.clone())?;
                    written = written
                        .checked_add(report.output_bytes)
                        .context("extraction size overflow")?;
                    append_extraction(&mut result, report);
                }
            }
            None => append_extraction(
                &mut result,
                extract_path(input, staging.path(), extraction)?,
            ),
        }
        let staged = std::mem::take(&mut result.written);
        let mut published = std::collections::HashSet::new();
        for source in staged {
            let relative = source
                .strip_prefix(staging.path())
                .context("extracted output escaped its staging root")?;
            let parent = relative.parent().unwrap_or(Path::new(""));
            let folder = destination.directory(parent)?;
            let target = folder.join(
                relative
                    .file_name()
                    .context("extracted file has no filename")?,
            );
            if !published.insert(target.clone()) {
                continue;
            }
            let operation = (|| -> Result<bool> {
                protection.check(&target)?;
                if let Some(guard) = additional {
                    guard.check(&target)?;
                }
                let limit = *remaining_output;
                let mut length = 0u64;
                let written = crate::output::atomic_write(&target, overwrite, |file| {
                    use std::io::Read as _;
                    let mut input = std::fs::File::open(&source)?.take(limit.saturating_add(1));
                    length = std::io::copy(&mut input, file)?;
                    if length > limit {
                        bail!("extracted output exceeds cumulative byte limit");
                    }
                    Ok(())
                })?;
                if written {
                    *remaining_output = remaining_output
                        .checked_sub(length)
                        .context("extraction publication size exceeds cumulative limit")?;
                }
                Ok(written)
            })();
            match operation {
                Ok(true) => result.written.push(target),
                Ok(false) => result
                    .skipped
                    .push(format!("existing output: {}", target.display())),
                Err(error) => result
                    .failed
                    .push(format!("extracted output {}: {error:#}", target.display())),
            }
        }
        let temporary = staging.path().to_string_lossy();
        let final_root = destination.root().to_string_lossy();
        for message in &mut result.skipped {
            *message = message.replace(temporary.as_ref(), final_root.as_ref());
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
    let mut limits = unity_rs_core::loader::AssetLoadLimits::default();
    limits.maximum_object_name_assignments = options.maximum_assets;
    limits.maximum_total_object_name_bytes = catalog_text_budget(options.maximum_assets);
    limits.maximum_reference_index_entries = options
        .maximum_assets
        .saturating_add(limits.maximum_input_files);
    limits.maximum_object_metadata_entries = options.maximum_assets;
    limits.maximum_object_metadata_index_bytes =
        catalog_text_budget(options.maximum_assets).saturating_mul(4);
    limits.maximum_container_assignments = limits
        .maximum_container_assignments
        .max(options.maximum_assets.saturating_mul(4));
    limits.container_metadata.maximum_preload_references = limits
        .container_metadata.maximum_preload_references
        .max(options.maximum_assets.saturating_mul(4));
    limits.container_metadata.maximum_container_entries = limits
        .container_metadata.maximum_container_entries
        .max(options.maximum_assets);
    limits.container_metadata.maximum_total_string_bytes =
        catalog_text_budget(options.maximum_assets);
    Ok(AssetLoadOptions {
        limits,
        unity_version_override,
        unity_cn_key,
        game_variant: match options.game.to_ascii_lowercase().as_str() {
            "naraka" => unity_rs_core::game_variant::GameVariant::Naraka,
            "tot" => unity_rs_core::game_variant::GameVariant::TearsOfThemis,
            "bh3prepre" => unity_rs_core::game_variant::GameVariant::HonkaiImpact3PrePre,
            "bh3pre" => unity_rs_core::game_variant::GameVariant::HonkaiImpact3Pre,
            "exastris" => unity_rs_core::game_variant::GameVariant::ExAstris,
            "arknightsendfield" => unity_rs_core::game_variant::GameVariant::ArknightsEndfield,
            "loveanddeepspace" => unity_rs_core::game_variant::GameVariant::LoveAndDeepspace,
            _ => unity_rs_core::game_variant::GameVariant::Standard,
        },
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

fn catalog_text_budget(maximum_assets: usize) -> usize {
    maximum_assets.saturating_mul(256).max(256 * 1024 * 1024)
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
    #[test]
    fn extraction_byte_budget_is_shared_across_inputs() {
        let source = tempfile::tempdir().unwrap();
        let first = source.path().join("first.resource");
        let second = source.path().join("second.resource");
        std::fs::write(&first, b"first payload").unwrap();
        std::fs::write(&second, b"second payload").unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut remaining = 20;
        let mut game_budget =
            crate::games::GamePreparationBudget::new(MAX_EXPORT_FILE_BYTES).unwrap();
        let first_report = Session::extract_with_guard(
            &first,
            &output.path().join("first"),
            &LoadOptions::default(),
            false,
            None,
            &mut remaining,
            &mut game_budget,
        )
        .unwrap();
        assert_eq!(first_report.written.len(), 1);
        assert_eq!(remaining, 7);
        let second_report = Session::extract_with_guard(
            &second,
            &output.path().join("second"),
            &LoadOptions::default(),
            false,
            None,
            &mut remaining,
            &mut game_budget,
        )
        .unwrap();
        assert!(second_report.written.is_empty());
        assert!(!second_report.failed.is_empty());
        assert_eq!(remaining, 7);
        assert!(!output.path().join("second/second.resource").exists());
    }
}
