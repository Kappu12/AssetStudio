use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use unity_rs_core::{
    image_export::{ImageRowOrder, write_rgba_image},
    material::MaterialReadLimits,
    mesh::MeshReadLimits,
    simple_assets::{SimpleAssetReadLimits, write_direct_wav},
    sprite::SpriteReadLimits,
    texture::TextureReadLimits,
};

use crate::output::{SafeOutput, atomic_write, component};
use crate::session::{MAX_EXPORT_FILE_BYTES, MAX_EXPORT_TOTAL_BYTES};
use crate::{
    AssetKey, AssetRecord, ExportMode, ExportOptions, GroupMode, ImageFormat, MapFormat,
    ModelFormat, OperationReport, Session,
};

pub(crate) struct LimitedBytes {
    bytes: Vec<u8>,
    maximum: u64,
}
impl LimitedBytes {
    pub(crate) fn new(maximum: u64) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
        }
    }
    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("output size overflow"))?;
        if length as u64 > self.maximum {
            return Err(io::Error::other("output exceeds its byte limit"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct LimitedWriter<'a> {
    inner: &'a mut std::fs::File,
    maximum: u64,
    written: u64,
}
impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("output size overflow"))?;
        if end > self.maximum {
            return Err(io::Error::other("export exceeds its byte budget"));
        }
        let written = self.inner.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

enum Payload {
    Raw,
    Bytes(Vec<u8>),
    Region(unity_rs_core::source::Region),
    Image(unity_rs_core::texture::RgbaImage, ImageRowOrder),
    Audio(unity_rs_core::simple_assets::AudioClipAsset),
}

impl Session {
    /// Exports exactly the supplied identities. An empty selection writes nothing.
    pub fn export(
        &self,
        keys: &[AssetKey],
        output: &Path,
        options: &ExportOptions,
    ) -> Result<OperationReport> {
        self.export_with_model_options(keys, output, options, &crate::ModelExportOptions::default())
    }

    pub fn export_with_model_options(
        &self,
        keys: &[AssetKey],
        output: &Path,
        options: &ExportOptions,
        model_options: &crate::ModelExportOptions,
    ) -> Result<OperationReport> {
        self.export_with_progress(keys, output, options, model_options, None)
    }

    pub fn export_with_progress(
        &self,
        keys: &[AssetKey],
        output: &Path,
        options: &ExportOptions,
        model_options: &crate::ModelExportOptions,
        progress: Option<&crate::ProgressObserver>,
    ) -> Result<OperationReport> {
        let records = self.selected(keys)?;
        let destination = SafeOutput::new(output)?;
        let mut report = OperationReport::default();
        let mut total = 0u64;
        let count = records.len();
        crate::session::report_progress(progress, "Exporting assets", 0, Some(count));
        for (index, record) in records.into_iter().enumerate() {
            crate::session::report_progress(progress, "Exporting assets", index, Some(count));
            if options.mode == ExportMode::Convert && matches!(record.class_id, 1 | 95) {
                let folder = destination.directory(&group_directory(record, options.group))?;
                match self.export_scene_with_limit_and_options(
                    &folder,
                    ModelFormat::FbxBinary,
                    &[record.key],
                    MAX_EXPORT_TOTAL_BYTES.saturating_sub(total),
                    model_options,
                ) {
                    Ok(scene) => {
                        for path in &scene.written {
                            total = total
                                .checked_add(std::fs::metadata(path)?.len())
                                .context("model export byte count overflow")?;
                        }
                        report.written.extend(scene.written);
                        report.failed.extend(scene.failed);
                        report.skipped.extend(scene.skipped);
                    }
                    Err(error) => report.failed.push(failure(record, &error)),
                }
                continue;
            }
            match self.export_one(
                record,
                &destination,
                options,
                MAX_EXPORT_TOTAL_BYTES.saturating_sub(total),
            ) {
                Ok((path, Some(bytes))) => {
                    total = total.checked_add(bytes).context("export total overflow")?;
                    report.written.push(path);
                }
                Ok((path, None)) => report
                    .skipped
                    .push(format!("existing output: {}", path.display())),
                Err(error) => report.failed.push(failure(record, &error)),
            }
        }
        crate::session::report_progress(progress, "Exporting assets", count, Some(count));
        Ok(report)
    }

    fn export_one(
        &self,
        record: &AssetRecord,
        destination: &SafeOutput,
        options: &ExportOptions,
        remaining: u64,
    ) -> Result<(PathBuf, Option<u64>)> {
        let object = self.object(record.key)?;
        let (extension, payload) = match options.mode {
            ExportMode::Raw => ("dat".to_owned(), Payload::Raw),
            ExportMode::Dump => (
                "txt".to_owned(),
                Payload::Bytes(self.object_dump(record.key, MAX_EXPORT_FILE_BYTES)?),
            ),
            ExportMode::Json => (
                "json".to_owned(),
                Payload::Bytes(self.object_json(record.key)?),
            ),
            ExportMode::Convert => self.convert_payload(record, options.image_format)?,
        };
        let folder = destination.directory(&group_directory(record, options.group))?;
        let stem = if record.name.is_empty() {
            record.class_name.as_str()
        } else {
            record.name.as_str()
        };
        let filename = format!(
            "{}_{}_{}.{}",
            component(stem),
            record.key.file_index,
            record.key.path_id,
            component(&extension)
        );
        let path = folder.join(filename);
        self.check_output_target(&path)?;
        let mut written = 0u64;
        let limit = remaining.min(MAX_EXPORT_FILE_BYTES);
        let published = atomic_write(&path, options.overwrite, |file| {
            let mut output = LimitedWriter {
                inner: file,
                maximum: limit,
                written: 0,
            };
            match payload {
                Payload::Raw => {
                    if object.byte_size() > limit {
                        bail!("raw object exceeds export output limit");
                    }
                    object.write_raw(&mut output)?;
                }
                Payload::Bytes(bytes) => output.write_all(&bytes)?,
                Payload::Region(region) => {
                    if region.len() > limit {
                        bail!("asset payload exceeds export output limit");
                    }
                    region.copy_range(0, region.len(), &mut output)?;
                }
                Payload::Image(image, row_order) => {
                    write_rgba_image(
                        &image,
                        backend_image_format(options.image_format),
                        row_order,
                        limit,
                        &mut output,
                    )?;
                }
                Payload::Audio(audio) => {
                    let kind = audio
                        .direct_wav
                        .context("audio WAV decoder not available")?;
                    write_direct_wav(&audio.payload, kind, limit, &mut output)?;
                }
            }
            output.flush()?;
            written = output.written;
            Ok(())
        })?;
        Ok((path, published.then_some(written)))
    }

    fn convert_payload(
        &self,
        record: &AssetRecord,
        image_format: ImageFormat,
    ) -> Result<(String, Payload)> {
        let object = self.object(record.key)?;
        Ok(match record.class_id {
            28 => (
                image_extension(image_format).into(),
                Payload::Image(
                    object.decode_texture_mip(0, TextureReadLimits::default())?,
                    ImageRowOrder::UnityDecoded,
                ),
            ),
            213 => (
                image_extension(image_format).into(),
                Payload::Image(
                    object
                        .decode_sprite(SpriteReadLimits::default(), TextureReadLimits::default())?,
                    ImageRowOrder::Display,
                ),
            ),
            43 => (
                "obj".into(),
                Payload::Bytes(object.read_mesh_obj(MeshReadLimits::default())?),
            ),
            48 => (
                "shader".into(),
                Payload::Bytes(object.read_shader_text(MAX_EXPORT_FILE_BYTES)?),
            ),
            49 => {
                let extension =
                    container_extension(&record.container).unwrap_or_else(|| "txt".into());
                (
                    extension,
                    Payload::Bytes(object.read_text_bytes(MAX_EXPORT_FILE_BYTES as usize)?),
                )
            }
            83 => {
                let audio = object.read_audio_clip(SimpleAssetReadLimits::default())?;
                if audio.direct_wav.is_some() {
                    ("wav".into(), Payload::Audio(audio))
                } else {
                    (
                        audio.raw_extension.trim_start_matches('.').to_owned(),
                        Payload::Region(audio.payload),
                    )
                }
            }
            128 => {
                let asset = object.read_font(SimpleAssetReadLimits::default())?;
                (
                    asset.suggested_extension.trim_start_matches('.').into(),
                    Payload::Region(asset.payload),
                )
            }
            152 => {
                let asset = object.read_movie_texture(SimpleAssetReadLimits::default())?;
                (
                    asset.suggested_extension.trim_start_matches('.').into(),
                    Payload::Region(asset.payload),
                )
            }
            329 => {
                let asset = object.read_video_clip(SimpleAssetReadLimits::default())?;
                (
                    asset.suggested_extension.trim_start_matches('.').into(),
                    Payload::Region(asset.payload),
                )
            }
            21 | 114 => ("json".into(), Payload::Bytes(self.object_json(record.key)?)),
            74 => (
                "anim".into(),
                Payload::Bytes(crate::animation::read_animation_yaml(self, record.key)?),
            ),
            1208 => {
                let bytes = read_bin_data(self, record.key, MAX_EXPORT_FILE_BYTES)?;
                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(value) if value.is_object() || value.is_array() => (
                        "json".into(),
                        Payload::Bytes(serde_json::to_vec_pretty(&value)?),
                    ),
                    _ => ("bin".into(), Payload::Bytes(bytes)),
                }
            }
            1210 => ("json".into(), Payload::Bytes(self.object_json(record.key)?)),
            // The original ExportConvertFile intentionally exported every other class raw.
            _ => ("dat".into(), Payload::Raw),
        })
    }

    pub(crate) fn object_dump(&self, key: AssetKey, maximum: u64) -> Result<Vec<u8>> {
        use unity_rs_core::mono_schema::{
            MonoBehaviourSchemaIdentity, MonoBehaviourSchemaProvider, MonoBehaviourSchemaSource,
            read_mono_behaviour_value_with_provider,
        };
        let object = self.object(key)?;
        if object.class_id() != 114 {
            return Ok(object.read_type_tree_dump(maximum)?);
        }
        let file = &self.studio.collection().serialized_files()[key.file_index].file;
        let resolved = read_mono_behaviour_value_with_provider(
            self.studio.collection(),
            key.file_index,
            object.object_index(),
            &self.mono_schemas,
            unity_rs_core::monobehaviour::MonoBehaviourReadLimits::default(),
        )?;
        let tree = match resolved.source {
            MonoBehaviourSchemaSource::Embedded => file.object_type_tree(object.object_index())?,
            MonoBehaviourSchemaSource::External => {
                let script = resolved
                    .script
                    .as_ref()
                    .context("external MonoBehaviour schema has no resolved script identity")?;
                self.mono_schemas
                    .schema(MonoBehaviourSchemaIdentity {
                        unity_version: &file.unity_version_string,
                        assembly_name: &script.assembly_name,
                        namespace: &script.namespace,
                        class_name: &script.class_name,
                    })?
                    .context("MonoBehaviour dump schema is no longer available")?
            }
        };
        let mut output = LimitedBytes::new(maximum);
        unity_rs_core::type_tree_dump::write_type_tree_dump(
            tree,
            &resolved.value,
            &mut output,
            maximum,
        )?;
        Ok(output.into_inner())
    }

    fn object_json(&self, key: AssetKey) -> Result<Vec<u8>> {
        self.object_json_with_limit(key, 64 * 1024 * 1024)
    }

    pub(crate) fn object_json_with_limit(
        &self,
        key: AssetKey,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>> {
        let object = self.object(key)?;
        if object.class_id() == 1210 {
            let mut bytes = LimitedBytes::new(maximum_bytes as u64);
            serde_json::to_writer_pretty(&mut bytes, &read_index_data(self, key)?)?;
            return Ok(bytes.into_inner());
        }
        if object.class_id() == 114 {
            let resolved = object.read_mono_behaviour_json(
                &self.mono_schemas,
                true,
                unity_rs_core::monobehaviour::MonoBehaviourReadLimits {
                    maximum_json_bytes: maximum_bytes,
                    ..unity_rs_core::monobehaviour::MonoBehaviourReadLimits::default()
                },
            )?;
            return Ok(resolved.json.into_bytes());
        }
        match object.read_type_tree_json(true,maximum_bytes) {
            Ok(bytes)=>Ok(bytes),
            Err(_) if object.class_id()==21=>{
                let bytes=material_json(object.read_material(MaterialReadLimits::default())?)?;
                if bytes.len()>maximum_bytes {bail!("material JSON exceeds output limit");}
                Ok(bytes)
            },
            Err(error)=>Err(error).context("structured JSON for this class requires an embedded Unity type tree; MonoBehaviour scripts can also use trusted JSON schemas or native DLL metadata"),
        }
    }

    /// Writes an asset map to an exact filename, without replacing an existing map.
    pub fn write_map(
        &self,
        keys: &[AssetKey],
        output: &Path,
        format: MapFormat,
    ) -> Result<OperationReport> {
        let selected = self.selected(keys)?;
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let destination = SafeOutput::new(parent)?;
        let filename = output.file_name().context("map filename is missing")?;
        let path = destination.root().join(filename);
        self.check_output_target(&path)?;
        let published = atomic_write(&path, false, |file| {
            let mut writer = LimitedWriter {
                inner: file,
                maximum: 64 * 1024 * 1024,
                written: 0,
            };
            match format {
                MapFormat::Json => serde_json::to_writer_pretty(&mut writer, &selected)?,
                MapFormat::Xml => {
                    writeln!(
                        writer,
                        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Assets>"
                    )?;
                    for record in selected {
                        writeln!(
                            writer,
                            "  <Asset file=\"{}\" pathId=\"{}\" classId=\"{}\" size=\"{}\">",
                            record.key.file_index, record.key.path_id, record.class_id, record.size
                        )?;
                        for (tag, value) in [
                            ("Name", record.name.as_str()),
                            ("Type", record.class_name.as_str()),
                            ("Container", record.container.as_str()),
                            ("Source", record.source.as_str()),
                        ] {
                            write!(writer, "    <{tag}>")?;
                            write_xml_text(&mut writer, value)?;
                            writeln!(writer, "</{tag}>")?;
                        }
                        writeln!(writer, "  </Asset>")?;
                    }
                    writeln!(writer, "</Assets>")?;
                }
            }
            Ok(())
        })?;
        if published {
            Ok(OperationReport {
                written: vec![path],
                ..OperationReport::default()
            })
        } else {
            Ok(OperationReport {
                skipped: vec![format!("existing output: {}", path.display())],
                ..OperationReport::default()
            })
        }
    }
}

fn failure(record: &AssetRecord, error: &anyhow::Error) -> String {
    format!(
        "{} {:?} (file {}, path {}): {error:#}",
        record.class_name, record.name, record.key.file_index, record.key.path_id
    )
}

fn group_directory(record: &AssetRecord, mode: GroupMode) -> PathBuf {
    match mode {
        GroupMode::ByType => PathBuf::from(component(&record.class_name)),
        GroupMode::BySource => PathBuf::from(format!(
            "{}_{}",
            component(
                record
                    .source
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&record.source)
            ),
            record.key.file_index
        )),
        GroupMode::ByContainer => {
            let mut result = PathBuf::new();
            let parts: Vec<_> = record.container.split(['/', '\\']).collect();
            for part in parts.iter().take(parts.len().saturating_sub(1)).take(32) {
                result.push(component(part));
            }
            if result.as_os_str().is_empty() {
                result.push("unassigned");
            }
            result
        }
        GroupMode::None => PathBuf::new(),
    }
}

fn container_extension(container: &str) -> Option<String> {
    let file = container.rsplit(['/', '\\']).next()?;
    let (_, extension) = file.rsplit_once('.')?;
    if extension.is_empty()
        || extension.len() > 16
        || !extension.bytes().all(|value| value.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(extension.to_owned())
}
fn image_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tga => "tga",
    }
}
pub(crate) fn backend_image_format(
    format: ImageFormat,
) -> unity_rs_core::image_export::ImageFormat {
    match format {
        ImageFormat::Png => unity_rs_core::image_export::ImageFormat::Png,
        ImageFormat::Jpeg => unity_rs_core::image_export::ImageFormat::Jpeg,
        ImageFormat::Bmp => unity_rs_core::image_export::ImageFormat::Bmp,
        ImageFormat::Tga => unity_rs_core::image_export::ImageFormat::Tga,
    }
}

pub(crate) fn read_bin_data(session: &Session, key: AssetKey, maximum: u64) -> Result<Vec<u8>> {
    let object = session.object(key)?;
    let file = &session.studio.collection().serialized_files()[key.file_index].file;
    let raw = object.read_raw(
        maximum
            .checked_add(8)
            .context("binary payload limit overflow")?,
    )?;
    let offset = if file.target_platform == -2 { 4 } else { 0 };
    let count: [u8; 4] = raw
        .get(offset..offset + 4)
        .context("truncated MiHoYoBinData length")?
        .try_into()?;
    let length = if file.header.endianness == 0 {
        i32::from_le_bytes(count)
    } else {
        i32::from_be_bytes(count)
    };
    if length < 0 || length as u64 > maximum {
        bail!("invalid MiHoYoBinData payload size {length}");
    }
    let start = offset + 4;
    let end = start
        .checked_add(length as usize)
        .context("binary payload length overflow")?;
    let bytes = raw
        .get(start..end)
        .context("truncated MiHoYoBinData payload")?;
    if end != raw.len() {
        bail!("unexpected trailing bytes in MiHoYoBinData");
    }
    Ok(bytes.iter().map(|value| value ^ session.xor_key).collect())
}

pub(crate) fn read_index_data(
    session: &Session,
    key: AssetKey,
) -> Result<crate::game_objects::IndexObjectData> {
    let object = session.object(key)?;
    let file = &session.studio.collection().serialized_files()[key.file_index].file;
    let info = &file.objects[object.object_index()];
    let options = crate::game_objects::IndexObjectReadOptions {
        little_endian: file.header.endianness == 0,
        no_target: file.target_platform == -2,
        path_id_64: file.header.version.0 >= 14,
        object_offset: info.byte_start,
    };
    crate::game_objects::decode_index_object(&object.read_raw(64 * 1024 * 1024)?, options)
}

fn material_json(material: unity_rs_core::material::Material) -> Result<Vec<u8>> {
    for property in &material.saved_properties.texture_environments {
        if property
            .value
            .scale
            .iter()
            .chain(&property.value.offset)
            .any(|value| !value.is_finite())
        {
            bail!(
                "material texture property {:?} has a non-finite transform that JSON cannot preserve",
                property.name
            );
        }
    }
    for property in &material.saved_properties.floats {
        if !property.value.is_finite() {
            bail!(
                "material float property {:?} has a non-finite value that JSON cannot preserve",
                property.name
            );
        }
    }
    for property in &material.saved_properties.colors {
        if property.value.iter().any(|value| !value.is_finite()) {
            bail!(
                "material color property {:?} has a non-finite value that JSON cannot preserve",
                property.name
            );
        }
    }
    let textures:Vec<_>=material.saved_properties.texture_environments.into_iter().map(|property|{let value=property.value;serde_json::json!({"name":property.name,"texture":{"file_id":value.texture.file_id,"path_id":value.texture.path_id},"scale":value.scale,"offset":value.offset})}).collect();
    let integers: Vec<_> = material
        .saved_properties
        .integers
        .into_iter()
        .map(|property| serde_json::json!({"name":property.name,"value":property.value}))
        .collect();
    let floats: Vec<_> = material
        .saved_properties
        .floats
        .into_iter()
        .map(|property| serde_json::json!({"name":property.name,"value":property.value}))
        .collect();
    let colors: Vec<_> = material
        .saved_properties
        .colors
        .into_iter()
        .map(|property| serde_json::json!({"name":property.name,"value":property.value}))
        .collect();
    let value = serde_json::json!({"name":material.name,"shader":{"file_id":material.shader.file_id,"path_id":material.shader.path_id},
        "legacy_shader_keywords":material.legacy_shader_keywords,"valid_keywords":material.valid_keywords,"invalid_keywords":material.invalid_keywords,
        "lightmap_flags":material.lightmap_flags,"enable_instancing_variants":material.enable_instancing_variants,"custom_render_queue":material.custom_render_queue,
        "string_tags":material.string_tags,"disabled_shader_passes":material.disabled_shader_passes,
        "saved_properties":{"textures":textures,"ints":integers,"floats":floats,"colors":colors}});
    Ok(serde_json::to_vec_pretty(&value)?)
}

fn write_xml_text(writer: &mut impl Write, value: &str) -> io::Result<()> {
    for ch in value.chars() {
        match ch {
            '&' => writer.write_all(b"&amp;")?,
            '<' => writer.write_all(b"&lt;")?,
            '>' => writer.write_all(b"&gt;")?,
            '"' => writer.write_all(b"&quot;")?,
            '\'' => writer.write_all(b"&apos;")?,
            '\u{9}'
            | '\u{a}'
            | '\u{d}'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => {
                let mut bytes = [0; 4];
                writer.write_all(ch.encode_utf8(&mut bytes).as_bytes())?;
            }
            _ => writer.write_all("�".as_bytes())?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_budget_prevents_partial_in_memory_success() {
        let mut writer = LimitedBytes::new(3);
        writer.write_all(b"abc").unwrap();
        assert!(writer.write_all(b"d").is_err());
        assert_eq!(writer.into_inner(), b"abc");
    }
    #[test]
    fn xml_strings_and_extension_are_safe() {
        let mut bytes = Vec::new();
        write_xml_text(&mut bytes, "<&\"'\u{1}").unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "&lt;&amp;&quot;&apos;�");
        assert_eq!(container_extension("a/b.json"), Some("json".into()));
        assert_eq!(container_extension("a\\b.unity3d"), Some("unity3d".into()));
        assert!(container_extension("a.x:y").is_none());
    }
    #[test]
    fn material_json_never_replaces_nonfinite_numbers_with_null() {
        use unity_rs_core::material::{
            Material, MaterialPropertySheet, MaterialTextureEnvironment, NamedMaterialProperty,
        };
        use unity_rs_core::serialized::ObjectReference;
        let reference = ObjectReference {
            file_id: 0,
            path_id: 0,
        };
        let material = Material {
            path_id: 1,
            name: "material".into(),
            shader: reference,
            legacy_shader_keywords: Vec::new(),
            valid_keywords: Vec::new(),
            invalid_keywords: Vec::new(),
            lightmap_flags: None,
            enable_instancing_variants: None,
            custom_render_queue: None,
            string_tags: Vec::new(),
            disabled_shader_passes: Vec::new(),
            saved_properties: MaterialPropertySheet {
                texture_environments: vec![NamedMaterialProperty {
                    name: "_MainTex".into(),
                    value: MaterialTextureEnvironment {
                        texture: reference,
                        scale: [1.0, 1.0],
                        offset: [0.0, 0.0],
                    },
                }],
                integers: Vec::new(),
                floats: vec![NamedMaterialProperty {
                    name: "_Glossiness".into(),
                    value: 0.5,
                }],
                colors: vec![NamedMaterialProperty {
                    name: "_Color".into(),
                    value: [1.0; 4],
                }],
            },
            trailing_bytes: 0,
        };
        let json: serde_json::Value =
            serde_json::from_slice(&material_json(material.clone()).unwrap()).unwrap();
        assert_eq!(json["saved_properties"]["floats"][0]["value"], 0.5);
        for property in 0..4 {
            let mut invalid = material.clone();
            match property {
                0 => {
                    invalid.saved_properties.texture_environments[0].value.scale[0] = f32::INFINITY
                }
                1 => {
                    invalid.saved_properties.texture_environments[0]
                        .value
                        .offset[1] = f32::NEG_INFINITY
                }
                2 => invalid.saved_properties.floats[0].value = f32::NAN,
                _ => invalid.saved_properties.colors[0].value[3] = f32::NAN,
            }
            let error = material_json(invalid).unwrap_err().to_string();
            assert!(error.contains("non-finite"), "{error}");
        }
    }
}
