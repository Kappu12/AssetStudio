use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use assetstudio_core::class_structures::ClassStructure;
use assetstudio_core::{
    AssetKey, AssetRecord, ExportMode, ExportOptions, FbxUvType, GroupMode, ImageFormat,
    LoadOptions, MapFormat, ModelExportOptions, ModelFormat, OperationReport, Preview, SceneNode,
    Session,
};
use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense, TextEdit, Vec2};

use crate::audio::AudioPlayer;
use crate::preview_image::{self, Channels};
use crate::view::{
    self, Catalog, Filters, MeshMode, MeshPreview, MeshShading, RowIndex, SortColumn, TypeSort,
};

const ACCENT: Color32 = Color32::WHITE;
const MUTED: Color32 = Color32::from_gray(145);
const BACKGROUND: Color32 = Color32::from_rgb(9, 9, 9);
const PANEL: Color32 = Color32::from_gray(14);
const MAX_LOG_ENTRIES: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Assets,
    Hierarchy,
    ClassStructures,
    Log,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PreviewMode {
    #[default]
    Preview,
    Dump,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Load,
    Preview,
    Scene,
    ClassStructures,
    Export,
    Audio,
    Filter,
    KeyList,
    ImageChannels,
}

enum TaskPayload {
    Loaded {
        paths: Vec<PathBuf>,
        resolved_paths: Vec<PathBuf>,
        auxiliary_inputs: Vec<PathBuf>,
        session: Arc<Session>,
        options: LoadOptions,
        catalog: Catalog,
    },
    KeyNames {
        source: String,
        result: Result<Vec<String>, String>,
    },
    Filtered {
        generation: u64,
        revision: u64,
        result: Result<Vec<usize>, String>,
    },
    Preview {
        generation: u64,
        revision: u64,
        key: AssetKey,
        preview: PreparedPreview,
    },
    ImageChannels {
        generation: u64,
        key: AssetKey,
        revision: u64,
        result: Result<egui::ColorImage, String>,
    },
    Scene {
        generation: u64,
        nodes: Vec<SceneNode>,
        rows: Vec<(usize, usize)>,
    },
    ClassStructures {
        generation: u64,
        definitions: Vec<ClassStructure>,
    },
    Audio {
        generation: u64,
        revision: u64,
        key: AssetKey,
        bytes: Vec<u8>,
    },
    Report {
        operation: &'static str,
        report: OperationReport,
    },
}

struct Task {
    receiver: mpsc::Receiver<Result<TaskPayload, String>>,
    kind: TaskKind,
    label: &'static str,
    generation: u64,
    revision: u64,
}

enum DisplayPreview {
    Text(String),
    Image(ImagePreview),
    Mesh(MeshPreview),
    Error(String),
}

/// Expensive source validation, normal calculation and image conversion happen
/// on the worker. Only the texture upload and painting belong on the UI thread.
enum PreparedPreview {
    Text(String),
    Image {
        image: egui::ColorImage,
        original: Arc<Vec<u8>>,
        width: u32,
        height: u32,
    },
    Mesh(MeshPreview),
}

fn prepare_preview(preview: Preview) -> Result<PreparedPreview, String> {
    match preview {
        Preview::Text(text) => Ok(PreparedPreview::Text(text)),
        Preview::Image {
            width,
            height,
            rgba,
        } => Ok(PreparedPreview::Image {
            image: checked_color_image(width, height, &rgba)?,
            original: Arc::new(rgba),
            width,
            height,
        }),
        Preview::Mesh {
            vertices,
            indices,
            normals,
            colors,
        } => {
            let mesh = if normals.is_none() && colors.is_none() {
                MeshPreview::new(vertices, indices)
            } else {
                MeshPreview::with_attributes(vertices, indices, normals, colors)
            }?;
            Ok(PreparedPreview::Mesh(mesh))
        }
    }
}

struct ImagePreview {
    texture: egui::TextureHandle,
    original: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    channels: Channels,
    revision: u64,
}

struct LogEntry {
    error: bool,
    text: String,
}

#[derive(Default)]
struct LoadSettings {
    game: String,
    unity_version: String,
    unity_cn_key: String,
    unity_cn_keys_file: String,
    unity_cn_key_index: usize,
    xor_key: String,
    asset_index: String,
    schemas: String,
    dummy_dlls: String,
    cab_map: String,
    cab_base: String,
    continue_on_error: bool,
    maximum_assets: String,
}

impl LoadSettings {
    fn options(&self) -> Result<LoadOptions, String> {
        let xor_key = if self.xor_key.trim().is_empty() {
            0
        } else if let Some(hex) = self
            .xor_key
            .trim()
            .strip_prefix("0x")
            .or_else(|| self.xor_key.trim().strip_prefix("0X"))
        {
            u8::from_str_radix(hex, 16)
                .map_err(|_| "XOR key must be a byte from 0 to 255, or 0x00 to 0xFF.")?
        } else {
            self.xor_key
                .trim()
                .parse::<u8>()
                .map_err(|_| "XOR key must be a byte from 0 to 255, or 0x00 to 0xFF.")?
        };
        let options = LoadOptions {
            game: if self.game.trim().is_empty() {
                "Normal".to_owned()
            } else {
                self.game.trim().to_owned()
            },
            unity_version: nonempty(&self.unity_version),
            unity_cn_key: nonempty(&self.unity_cn_key),
            xor_key,
            continue_on_error: self.continue_on_error,
            maximum_assets: if self.maximum_assets.trim().is_empty() {
                10_000_000
            } else {
                self.maximum_assets
                    .trim()
                    .parse()
                    .map_err(|_| "Asset limit must be a whole number from 1 to 10000000.")?
            },
        };
        if options.game.eq_ignore_ascii_case("UnityCN") && options.unity_cn_key.is_none() {
            // Resource lookup belongs to the load worker; validate all other
            // fields now without opening a key file on the UI thread.
            let mut validation = options.clone();
            validation.game = "Normal".to_owned();
            validation.validate().map_err(|error| error.to_string())?;
        } else {
            options.validate().map_err(|error| error.to_string())?;
        }
        Ok(options)
    }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

pub struct StudioApp {
    session: Option<Arc<Session>>,
    source: Option<PathBuf>,
    sources: Vec<PathBuf>,
    resolved_sources: Vec<PathBuf>,
    source_auxiliary: Vec<PathBuf>,
    source_options: LoadOptions,
    generation: u64,
    assets: Arc<Vec<AssetRecord>>,
    by_key: Arc<HashMap<AssetKey, usize>>,
    classes: BTreeMap<String, usize>,
    row_index: Arc<RowIndex>,
    type_search: String,
    type_sort: TypeSort,
    visible: Vec<usize>,
    filters: Filters,
    filter_error: Option<String>,
    sort: SortColumn,
    descending: bool,
    filter_dirty: bool,
    filter_revision: u64,
    pending_filter: bool,
    selected: HashSet<AssetKey>,
    primary: Option<AssetKey>,
    anchor: Option<AssetKey>,
    preview: Option<DisplayPreview>,
    preview_mode: PreviewMode,
    preview_revision: u64,
    pending_preview: Option<AssetKey>,
    pending_channels: Option<(AssetKey, u64)>,
    image_zoom: f32,
    image_fit: bool,
    mesh_mode: MeshMode,
    mesh_shading: MeshShading,
    scene: Vec<SceneNode>,
    scene_loaded: bool,
    scene_error: Option<String>,
    scene_rows: Vec<(usize, usize)>,
    pending_scene: bool,
    class_structures: Vec<ClassStructure>,
    classes_loaded: bool,
    classes_error: Option<String>,
    pending_classes: bool,
    selected_class: Option<usize>,
    task: Option<Task>,
    task_progress: crate::progress::Progress,
    export_options: ExportOptions,
    model_options: ModelExportOptions,
    load_settings: LoadSettings,
    unity_cn_names: Vec<String>,
    pending_key_names: Option<String>,
    settings_open: bool,
    input_path: String,
    tab: Tab,
    log: VecDeque<LogEntry>,
    last_error: Option<String>,
    status: String,
    audio: AudioPlayer,
    audio_revision: u64,
}

impl StudioApp {
    pub fn new(context: &egui::Context, initial_path: Option<PathBuf>) -> Self {
        configure_style(context);
        let mut app = Self {
            session: None,
            source: None,
            sources: Vec::new(),
            resolved_sources: Vec::new(),
            source_auxiliary: Vec::new(),
            source_options: LoadOptions::default(),
            generation: 0,
            assets: Arc::new(Vec::new()),
            by_key: Arc::new(HashMap::new()),
            classes: BTreeMap::new(),
            row_index: Arc::new(RowIndex::default()),
            type_search: String::new(),
            type_sort: TypeSort::default(),
            visible: Vec::new(),
            filters: Filters::default(),
            filter_error: None,
            sort: SortColumn::Name,
            descending: false,
            filter_dirty: false,
            filter_revision: 0,
            pending_filter: false,
            selected: HashSet::new(),
            primary: None,
            anchor: None,
            preview: None,
            preview_mode: PreviewMode::Preview,
            preview_revision: 0,
            pending_preview: None,
            pending_channels: None,
            image_zoom: 1.0,
            image_fit: true,
            mesh_mode: MeshMode::Shaded,
            mesh_shading: MeshShading::Flat,
            scene: Vec::new(),
            scene_loaded: false,
            scene_error: None,
            scene_rows: Vec::new(),
            pending_scene: false,
            class_structures: Vec::new(),
            classes_loaded: false,
            classes_error: None,
            pending_classes: false,
            selected_class: None,
            task: None,
            task_progress: Default::default(),
            export_options: ExportOptions::default(),
            model_options: ModelExportOptions::default(),
            load_settings: LoadSettings {
                game: "Normal".to_owned(),
                maximum_assets: "10000000".to_owned(),
                ..Default::default()
            },
            unity_cn_names: Vec::new(),
            pending_key_names: None,
            settings_open: false,
            input_path: String::new(),
            tab: Tab::Assets,
            log: VecDeque::new(),
            last_error: None,
            status: "Open a Unity file, bundle, or game data folder to begin.".to_owned(),
            audio: AudioPlayer::default(),
            audio_revision: 0,
        };
        match crate::preferences::load()
            .and_then(|value| value.map(|value| app.apply_preferences(&value)).transpose())
        {
            Ok(_) => {}
            Err(error) => {
                app.status = format!("Saved settings could not be restored: {error}");
                app.log(false, app.status.clone());
            }
        }
        if let Some(path) = initial_path {
            app.open_path(path, context);
        }
        app
    }

    fn preferences_value(&self) -> serde_json::Value {
        let load = &self.load_settings;
        serde_json::json!({
            "load": {
                "game":load.game,"unity_version":load.unity_version,
                "unity_cn_keys_file":load.unity_cn_keys_file,"unity_cn_key_index":load.unity_cn_key_index,
                "xor_key":load.xor_key,"asset_index":load.asset_index,"schemas":load.schemas,
                "dummy_dlls":load.dummy_dlls,"cab_map":load.cab_map,"cab_base":load.cab_base,
                "continue_on_error":load.continue_on_error,
                "maximum_assets":load.maximum_assets,
            },
            "export": {
                "mode":match self.export_options.mode {ExportMode::Convert=>"Convert",ExportMode::Raw=>"Raw",ExportMode::Dump=>"Dump",ExportMode::Json=>"Json"},
                "group":match self.export_options.group {GroupMode::ByType=>"ByType",GroupMode::ByContainer=>"ByContainer",GroupMode::BySource=>"BySource",GroupMode::None=>"None"},
                "image_format":match self.export_options.image_format {ImageFormat::Png=>"Png",ImageFormat::Jpeg=>"Jpeg",ImageFormat::Bmp=>"Bmp",ImageFormat::Tga=>"Tga"},
                "overwrite":self.export_options.overwrite,
                "include_animations":self.model_options.include_animations,
                "include_textures":self.model_options.include_textures,
                "include_skins":self.model_options.include_skins,"include_materials":self.model_options.include_materials,
                "include_blend_shapes":self.model_options.include_blend_shapes,"export_all_nodes":self.model_options.export_all_nodes,
                "scale_factor":self.model_options.scale_factor,"euler_filter":self.model_options.euler_filter,
                "uv_enabled":self.model_options.uv_enabled,"fbx_version":self.model_options.fbx.version,
                "cast_to_bone":self.model_options.fbx.cast_to_bone,"bone_size":self.model_options.fbx.bone_size,
                "uv_types":self.model_options.fbx.uv_types.map(u8::from),
            },
            "preview": {
                "image_zoom":self.image_zoom,"image_fit":self.image_fit,
                "mesh_mode":match self.mesh_mode {MeshMode::Shaded=>"Shaded",MeshMode::Wireframe=>"Wireframe",MeshMode::Combined=>"Combined"},
                "mesh_shading":self.mesh_shading.preference(),
                "object_view":match self.preview_mode {PreviewMode::Preview=>"Preview",PreviewMode::Dump=>"Dump",PreviewMode::Json=>"Json"},
                "audio_volume":self.audio.volume(),"audio_loop":self.audio.is_looping(),
            },
        })
    }

    fn save_preferences_to(&self, path: &std::path::Path) -> Result<(), String> {
        // Controls can hold incomplete or out-of-range edits until the user
        // finishes typing. Preserve the previous file if those edits could not
        // be loaded on the next start.
        self.load_settings.options()?;
        self.model_options
            .validate()
            .map_err(|error| error.to_string())?;
        if let Some(session) = &self.session {
            session
                .check_output_target(path)
                .map_err(|error| error.to_string())?;
        }
        crate::preferences::save_to(path, &self.preferences_value())
    }

    fn apply_preferences(&mut self, value: &serde_json::Value) -> Result<(), String> {
        if !value.is_object() {
            return Err("Settings must contain an object.".to_owned());
        }
        let load = &value["load"];
        let export = &value["export"];
        let preview = &value["preview"];
        for section in [load, export, preview] {
            if !section.is_null() && !section.is_object() {
                return Err("A saved settings section is invalid.".to_owned());
            }
        }
        let mut settings = LoadSettings::default();
        for (name, target) in [
            ("game", &mut settings.game),
            ("unity_version", &mut settings.unity_version),
            ("maximum_assets", &mut settings.maximum_assets),
            ("unity_cn_keys_file", &mut settings.unity_cn_keys_file),
            ("xor_key", &mut settings.xor_key),
            ("asset_index", &mut settings.asset_index),
            ("schemas", &mut settings.schemas),
            ("dummy_dlls", &mut settings.dummy_dlls),
            ("cab_map", &mut settings.cab_map),
            ("cab_base", &mut settings.cab_base),
        ] {
            *target = preference_string(
                load,
                name,
                match name {
                    "game" => "Normal",
                    "maximum_assets" => "10000000",
                    _ => "",
                },
            )?;
        }
        settings.continue_on_error = preference_bool(load, "continue_on_error", false)?;
        settings.unity_cn_key_index = match load.get("unity_cn_key_index") {
            None => 0,
            Some(value) => value
                .as_u64()
                .filter(|index| *index < 1024)
                .ok_or("Saved UnityCN key index is invalid.")? as usize,
        };
        settings.options()?;
        let options = ExportOptions {
            mode: match preference_string(export, "mode", "Convert")?.as_str() {
                "Convert" => ExportMode::Convert,
                "Raw" => ExportMode::Raw,
                "Dump" => ExportMode::Dump,
                "Json" => ExportMode::Json,
                _ => return Err("Saved export mode is unknown.".to_owned()),
            },
            group: match preference_string(export, "group", "ByType")?.as_str() {
                "ByType" => GroupMode::ByType,
                "ByContainer" => GroupMode::ByContainer,
                "BySource" => GroupMode::BySource,
                "None" => GroupMode::None,
                _ => return Err("Saved export grouping is unknown.".to_owned()),
            },
            image_format: match preference_string(export, "image_format", "Png")?.as_str() {
                "Png" => ImageFormat::Png,
                "Jpeg" => ImageFormat::Jpeg,
                "Bmp" => ImageFormat::Bmp,
                "Tga" => ImageFormat::Tga,
                _ => return Err("Saved image format is unknown.".to_owned()),
            },
            overwrite: preference_bool(export, "overwrite", false)?,
        };
        let mut model_options = ModelExportOptions::default();
        for (name, target) in [
            ("include_animations", &mut model_options.include_animations),
            ("include_textures", &mut model_options.include_textures),
            ("include_skins", &mut model_options.include_skins),
            ("include_materials", &mut model_options.include_materials),
            (
                "include_blend_shapes",
                &mut model_options.include_blend_shapes,
            ),
            ("export_all_nodes", &mut model_options.export_all_nodes),
            ("euler_filter", &mut model_options.euler_filter),
        ] {
            *target = preference_bool(export, name, *target)?;
        }
        model_options.scale_factor = preference_number(export, "scale_factor", 1.0)? as f32;
        model_options.fbx.bone_size = preference_number(export, "bone_size", 10.0)?;
        model_options.fbx.cast_to_bone = preference_bool(export, "cast_to_bone", false)?;
        if let Some(version) = export.get("fbx_version") {
            model_options.fbx.version = version
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or("Saved FBX version is invalid.")?;
        }
        if let Some(channels) = export.get("uv_enabled") {
            let channels = channels
                .as_array()
                .filter(|channels| channels.len() == 8)
                .ok_or("Saved UV channel selection is invalid.")?;
            for (target, channel) in model_options.uv_enabled.iter_mut().zip(channels) {
                *target = channel
                    .as_bool()
                    .ok_or("Saved UV channel selection is invalid.")?;
            }
        }
        if let Some(roles) = export.get("uv_types") {
            let roles = roles
                .as_array()
                .filter(|roles| roles.len() == 8)
                .ok_or("Saved UV roles are invalid.")?;
            for (target, role) in model_options.fbx.uv_types.iter_mut().zip(roles) {
                let value = role
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or("Saved UV role is invalid.")?;
                *target = FbxUvType::try_from(value).map_err(|error| error.to_string())?;
            }
        }
        model_options
            .validate()
            .map_err(|error| format!("Saved model options are invalid: {error}"))?;
        let image_zoom = match preview.get("image_zoom") {
            None => 1.0,
            Some(value) => value
                .as_f64()
                .filter(|zoom| (0.1..=8.0).contains(zoom))
                .ok_or("Saved image zoom is invalid.")? as f32,
        };
        let image_fit = preference_bool(preview, "image_fit", true)?;
        let mesh_mode = match preference_string(preview, "mesh_mode", "Shaded")?.as_str() {
            "Shaded" => MeshMode::Shaded,
            "Wireframe" => MeshMode::Wireframe,
            "Combined" => MeshMode::Combined,
            _ => return Err("Saved mesh display mode is unknown.".to_owned()),
        };
        let mesh_shading =
            MeshShading::from_preference(&preference_string(preview, "mesh_shading", "Flat")?)?;
        let preview_mode = match preference_string(preview, "object_view", "Preview")?.as_str() {
            "Preview" => PreviewMode::Preview,
            "Dump" => PreviewMode::Dump,
            "Json" => PreviewMode::Json,
            _ => return Err("Saved object view is unknown.".to_owned()),
        };
        let audio_volume = match preview.get("audio_volume") {
            None => 0.8,
            Some(value) => value
                .as_f64()
                .filter(|volume| (0.0..=1.0).contains(volume))
                .ok_or("Saved audio volume is invalid.")? as f32,
        };
        let audio_loop = preference_bool(preview, "audio_loop", false)?;
        // Apply only after every field has passed validation.
        self.load_settings = settings;
        self.export_options = options;
        self.model_options = model_options;
        self.image_zoom = image_zoom;
        self.image_fit = image_fit;
        self.mesh_mode = mesh_mode;
        self.mesh_shading = mesh_shading;
        self.preview_mode = preview_mode;
        self.audio.set_volume(audio_volume);
        self.audio.set_looping(audio_loop);
        Ok(())
    }

    fn log(&mut self, error: bool, value: impl Into<String>) {
        if self.log.len() == MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
        let mut text = value.into();
        if text.len() > 16_384 {
            let mut end = 16_384;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("… [message truncated]");
        }
        self.log.push_back(LogEntry { error, text });
    }

    fn error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.log(true, error.clone());
        self.status = "The operation could not be completed. See the message below.".to_owned();
        self.last_error = Some(error);
    }

    fn start_task(
        &mut self,
        kind: TaskKind,
        label: &'static str,
        context: &egui::Context,
        operation: impl FnOnce() -> Result<TaskPayload, String> + Send + 'static,
    ) {
        self.start_task_with_progress(kind, label, context, move |_| operation());
    }

    fn start_task_with_progress(
        &mut self,
        kind: TaskKind,
        label: &'static str,
        context: &egui::Context,
        operation: impl FnOnce(crate::progress::Progress) -> Result<TaskPayload, String>
        + Send
        + 'static,
    ) {
        if self.task.is_some() {
            return;
        }
        self.task_progress = crate::progress::Progress::new(label);
        let progress = self.task_progress.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let context = context.clone();
        match std::thread::Builder::new().name("assetstudio-worker".to_owned()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(progress)))
                .unwrap_or_else(|_| Err("The operation stopped unexpectedly. Your source files have not been changed.".to_owned()));
            let _ = sender.send(result);
            context.request_repaint();
        }) {
            Ok(_) => self.task = Some(Task { receiver, kind, label, generation: self.generation, revision: match kind {TaskKind::Audio=>self.audio_revision,TaskKind::Preview=>self.preview_revision,_=>self.filter_revision} }),
            Err(error) => self.error(format!("Could not start a background operation: {error}")),
        }
    }

    fn poll_task(&mut self, context: &egui::Context) {
        let Some(task) = &self.task else {
            return;
        };
        let kind = task.kind;
        let generation = task.generation;
        let revision = task.revision;
        let result = match task.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The background worker closed before returning a result.".to_owned())
            }
        };
        self.task = None;
        match result {
            Ok(TaskPayload::Loaded {
                paths,
                resolved_paths,
                auxiliary_inputs,
                session,
                options,
                catalog,
            }) => {
                let Some(path) = paths.first().cloned() else {
                    self.error("The loader returned no input paths.");
                    return;
                };
                self.stop_audio();
                self.generation = self.generation.wrapping_add(1);
                self.visible.clear();
                self.assets = catalog.assets;
                self.classes = catalog.classes;
                self.by_key = catalog.by_key;
                self.row_index = catalog.rows;
                self.filters
                    .classes
                    .retain(|class| self.classes.contains_key(class));
                for diagnostic in session
                    .diagnostics()
                    .iter()
                    .rev()
                    .take(MAX_LOG_ENTRIES)
                    .rev()
                {
                    self.log(true, diagnostic.clone());
                }
                for warning in session.warnings().iter().rev().take(MAX_LOG_ENTRIES).rev() {
                    self.log(false, format!("Advisory: {warning}"));
                }
                self.status = format!(
                    "Loaded {} objects from {}. {} load issues, {} advisories.",
                    self.assets.len(),
                    path.display(),
                    session.diagnostics().len(),
                    session.warnings().len(),
                );
                self.log(false, self.status.clone());
                self.input_path = path.to_string_lossy().into_owned();
                self.source = Some(path);
                self.sources = paths;
                self.resolved_sources = resolved_paths;
                self.source_auxiliary = auxiliary_inputs;
                self.source_options = options;
                self.session = Some(session);
                self.selected.clear();
                self.primary = None;
                self.anchor = None;
                self.preview = None;
                self.pending_preview = None;
                self.pending_channels = None;
                self.scene.clear();
                self.scene_rows.clear();
                self.scene_loaded = false;
                self.scene_error = None;
                self.pending_scene = self.tab == Tab::Hierarchy;
                self.class_structures.clear();
                self.classes_loaded = false;
                self.classes_error = None;
                self.selected_class = None;
                self.pending_classes = self.tab == Tab::ClassStructures;
                self.filter_dirty = true;
                self.last_error = None;
            }
            Ok(TaskPayload::KeyNames { source, result }) => {
                if self.load_settings.unity_cn_keys_file.trim() == source {
                    match result {
                        Ok(names) => self.unity_cn_names = names,
                        Err(error) => {
                            self.unity_cn_names.clear();
                            self.error(error);
                        }
                    }
                }
            }
            Ok(TaskPayload::Filtered {
                generation,
                revision,
                result,
            }) => {
                if self.generation == generation && self.filter_revision == revision {
                    self.pending_filter = false;
                    match result {
                        Ok(rows) => {
                            self.visible = rows;
                            self.filter_error = None;
                        }
                        Err(error) => {
                            self.visible.clear();
                            self.filter_error = Some(error);
                        }
                    }
                }
            }
            Ok(TaskPayload::Preview {
                generation,
                revision,
                key,
                preview,
            }) => {
                if view::preview_is_current(self.generation, generation, self.primary, key)
                    && self.preview_revision == revision
                {
                    self.preview = Some(match preview {
                        PreparedPreview::Text(text) => DisplayPreview::Text(text),
                        PreparedPreview::Mesh(mut mesh) => {
                            mesh.mode = self.mesh_mode;
                            mesh.set_shading(self.mesh_shading);
                            DisplayPreview::Mesh(mesh)
                        }
                        PreparedPreview::Image {
                            image,
                            original,
                            width,
                            height,
                        } => DisplayPreview::Image(ImagePreview {
                            texture: context.load_texture(
                                "asset-preview",
                                image,
                                egui::TextureOptions::LINEAR,
                            ),
                            original,
                            width,
                            height,
                            channels: Channels::default(),
                            revision: 0,
                        }),
                    });
                }
            }
            Ok(TaskPayload::ImageChannels {
                generation,
                key,
                revision,
                result,
            }) => {
                if view::preview_is_current(self.generation, generation, self.primary, key)
                    && let Some(DisplayPreview::Image(image)) = &mut self.preview
                    && image.revision == revision
                {
                    match result {
                        Ok(pixels) => image.texture.set(pixels, egui::TextureOptions::LINEAR),
                        Err(error) => self.error(error),
                    }
                }
            }
            Ok(TaskPayload::Scene {
                generation,
                nodes,
                rows,
            }) => {
                if self.generation == generation {
                    self.pending_scene = false;
                    self.scene_rows = rows;
                    self.scene = nodes;
                    self.scene_loaded = true;
                    self.scene_error = None;
                }
            }
            Ok(TaskPayload::ClassStructures {
                generation,
                definitions,
            }) => {
                if self.generation == generation {
                    self.pending_classes = false;
                    self.classes_loaded = true;
                    self.classes_error = None;
                    self.selected_class = (!definitions.is_empty()).then_some(0);
                    self.class_structures = definitions;
                }
            }
            Ok(TaskPayload::Audio {
                generation,
                revision,
                key,
                bytes,
            }) => {
                if view::preview_is_current(self.generation, generation, self.primary, key)
                    && self.audio_revision == revision
                    && let Err(error) = self.audio.play(bytes)
                {
                    self.error(format!("Could not play this audio clip: {error}"));
                }
            }
            Ok(TaskPayload::Report { operation, report }) => {
                self.last_error = None;
                self.status = format!(
                    "{operation}: {} written, {} skipped, {} failed.",
                    report.written.len(),
                    report.skipped.len(),
                    report.failed.len()
                );
                self.log(!report.failed.is_empty(), self.status.clone());
                for skipped in &report.skipped {
                    self.log(false, format!("Skipped: {skipped}"));
                }
                for failure in &report.failed {
                    self.log(true, failure.clone());
                }
                if !report.failed.is_empty() {
                    self.last_error = Some(format!(
                        "{} item(s) failed. Open the activity log for details.\n\n{}",
                        report.failed.len(),
                        report
                            .failed
                            .iter()
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                }
            }
            Err(error) => match kind {
                TaskKind::Filter => {
                    if self.generation == generation && self.filter_revision == revision {
                        self.pending_filter = false;
                        self.visible.clear();
                        self.filter_error = Some(error.clone());
                    }
                    self.log(true, error);
                }
                TaskKind::Preview => {
                    // Selection can change while a preview is loading. Do not attach
                    // an old object's error to the newly selected object.
                    if self.generation == generation
                        && self.preview_revision == revision
                        && self.pending_preview.is_none()
                    {
                        self.preview = Some(DisplayPreview::Error(error.clone()));
                    }
                    self.log(true, error);
                }
                TaskKind::Scene => {
                    self.pending_scene = false;
                    self.scene_error = Some(error.clone());
                    self.scene_loaded = true;
                    self.log(true, error);
                }
                TaskKind::ClassStructures => {
                    if self.generation == generation {
                        self.pending_classes = false;
                        self.classes_loaded = true;
                        self.classes_error = Some(error.clone());
                    }
                    self.log(true, error);
                }
                TaskKind::Audio => {
                    if self.generation == generation && self.audio_revision == revision {
                        self.error(error);
                    }
                }
                _ => self.error(error),
            },
        }
    }

    fn queue_filter(&mut self) {
        self.filter_revision = self.filter_revision.wrapping_add(1);
        self.pending_filter = true;
        self.filter_error = None;
    }

    fn start_pending(&mut self, context: &egui::Context) {
        if self.task.is_some() {
            return;
        }
        if self.pending_filter {
            self.pending_filter = false;
            let assets = self.assets.clone();
            let rows = self.row_index.clone();
            let filters = self.filters.clone();
            let sort = self.sort;
            let descending = self.descending;
            let generation = self.generation;
            let revision = self.filter_revision;
            self.start_task(
                TaskKind::Filter,
                "Updating asset list…",
                context,
                move || {
                    Ok(TaskPayload::Filtered {
                        generation,
                        revision,
                        result: rows.visible_rows(&assets, &filters, sort, descending),
                    })
                },
            );
            return;
        }
        if let Some(source) = self.pending_key_names.take() {
            self.start_task(
                TaskKind::KeyList,
                "Reading UnityCN key names…",
                context,
                move || {
                    let result = key_resource_path(&source).and_then(|path| {
                        let keys = assetstudio_core::unity_cn_keys::UnityCnKeys::from_path(&path)
                            .map_err(|error| error.to_string())?;
                        Ok(keys.names().map(str::to_owned).collect())
                    });
                    Ok(TaskPayload::KeyNames { source, result })
                },
            );
            return;
        }
        if self.pending_scene {
            self.pending_scene = false;
            if let Some(session) = self.session.clone() {
                let generation = self.generation;
                self.start_task_with_progress(
                    TaskKind::Scene,
                    "Building hierarchy…",
                    context,
                    move |progress| {
                        session
                            .scene_outline_with_progress(Some(&progress.observer()))
                            .map(|nodes| {
                                progress.update("Sorting hierarchy", 0, None);
                                let rows = hierarchy_order(&nodes);
                                TaskPayload::Scene {
                                    generation,
                                    nodes,
                                    rows,
                                }
                            })
                            .map_err(|error| error.to_string())
                    },
                );
                return;
            }
        }
        if self.pending_classes {
            self.pending_classes = false;
            if let Some(session) = self.session.clone() {
                let generation = self.generation;
                self.start_task(
                    TaskKind::ClassStructures,
                    "Reading class definitions…",
                    context,
                    move || {
                        session
                            .class_structures()
                            .map(|definitions| TaskPayload::ClassStructures {
                                generation,
                                definitions,
                            })
                            .map_err(|error| error.to_string())
                    },
                );
                return;
            }
        }
        if let Some((key, revision)) = self.pending_channels.take()
            && self.primary == Some(key)
            && let Some(DisplayPreview::Image(image)) = &self.preview
            && image.revision == revision
        {
            let original = image.original.clone();
            let channels = image.channels;
            let (width, height) = (image.width, image.height);
            let generation = self.generation;
            self.start_task(
                TaskKind::ImageChannels,
                "Updating image channels…",
                context,
                move || {
                    let result = preview_image::apply_channels(&original, channels)
                        .and_then(|rgba| checked_color_image(width, height, &rgba));
                    Ok(TaskPayload::ImageChannels {
                        generation,
                        key,
                        revision,
                        result,
                    })
                },
            );
            return;
        }
        if let Some(key) = self.pending_preview.take()
            && self.primary == Some(key)
            && let Some(session) = self.session.clone()
        {
            let generation = self.generation;
            let revision = self.preview_revision;
            let mode = self.preview_mode;
            self.start_task(
                TaskKind::Preview,
                "Reading preview…",
                context,
                move || {
                    let result = match mode {
                        PreviewMode::Preview => session.preview(key),
                        PreviewMode::Dump => session.preview_dump(key).map(Preview::Text),
                        PreviewMode::Json => session.preview_json(key).map(Preview::Text),
                    };
                    result
                        .map_err(|error| error.to_string())
                        .and_then(prepare_preview)
                        .map(|preview| TaskPayload::Preview {
                            generation,
                            revision,
                            key,
                            preview,
                        })
                },
            );
        }
    }

    fn open_path(&mut self, path: PathBuf, context: &egui::Context) {
        self.open_paths(vec![path], context);
    }

    fn open_paths(&mut self, paths: Vec<PathBuf>, context: &egui::Context) {
        if paths.is_empty() {
            return;
        }
        if self.task.is_some() {
            return;
        }
        let mut options = match self.load_settings.options() {
            Ok(options) => options,
            Err(error) => {
                self.error(error);
                return;
            }
        };
        let asset_index = nonempty(&self.load_settings.asset_index).map(PathBuf::from);
        let schemas = nonempty(&self.load_settings.schemas).map(PathBuf::from);
        let dummy_dlls = nonempty(&self.load_settings.dummy_dlls).map(PathBuf::from);
        let cab_map = nonempty(&self.load_settings.cab_map).map(PathBuf::from);
        let cab_base = nonempty(&self.load_settings.cab_base).map(PathBuf::from);
        let key_specification = self.load_settings.unity_cn_keys_file.trim().to_owned();
        let key_index = self.load_settings.unity_cn_key_index;
        let cab_source = cab_map.clone();
        self.stop_audio();
        self.pending_preview = None;
        self.start_task_with_progress(TaskKind::Load, "Opening assets", context, move |progress| {
            let mut auxiliary_inputs = Vec::new();
            let key_source = resolve_resource_key(&mut options, &key_specification, key_index)?;
            let resolved_paths = if let Some(map_path) = cab_map {
                let base = cab_base_for_inputs(&paths, cab_base)?;
                let map =
                    assetstudio_core::cab_map::CabMap::from_path(&map_path).map_err(|error| {
                        format!("Could not load CAB map {}: {error}", map_path.display())
                    })?;
                map.resolve_paths(&paths, Some(&base))
                    .map_err(|error| format!("Could not resolve CAB dependencies: {error}"))?
            } else {
                paths.clone()
            };
            let mut session = Session::open_paths_with_progress(
                &resolved_paths,
                &options,
                Some(progress.observer()),
            )
            .map_err(|error| error.to_string())?;
            for auxiliary in key_source.iter().chain(cab_source.iter()) {
                session
                    .protect_input_path(auxiliary)
                    .map_err(|error| error.to_string())?;
                auxiliary_inputs.push(auxiliary.clone());
            }
            if let Some(index) = asset_index {
                session.recover_containers(&index).map_err(|error| {
                    format!(
                        "Could not recover container names from {}: {error}",
                        index.display()
                    )
                })?;
                auxiliary_inputs.push(index);
            }
            if let Some(schema) = schemas {
                session.load_schemas(&schema).map_err(|error| {
                    format!(
                        "Could not load behaviour schemas from {}: {error}",
                        schema.display()
                    )
                })?;
                auxiliary_inputs.push(schema);
            }
            if let Some(folder) = dummy_dlls {
                let generator = assetstudio_core::schema::sibling_cli_executable()
                    .map_err(|error| error.to_string())?;
                session
                    .load_dummy_dlls(&folder, &generator)
                    .map_err(|error| {
                        format!(
                            "Could not load behaviour layouts from {}: {error}",
                            folder.display()
                        )
                    })?;
                auxiliary_inputs.push(folder);
            }
            let catalog =
                Catalog::prepare_session_with_progress(&session, |stage, done, total| {
                    progress.update(stage, done, total);
                })?;
            Ok(TaskPayload::Loaded {
                paths,
                resolved_paths,
                auxiliary_inputs,
                session: Arc::new(session),
                options,
                catalog,
            })
        });
    }

    fn open_file(&mut self, context: &egui::Context) {
        if let Some(paths) = rfd::FileDialog::new()
            .set_title("Open Unity file or bundle")
            .pick_files()
        {
            self.open_paths(paths, context);
        }
    }

    fn open_folder(&mut self, context: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open game data folder")
            .pick_folder()
        {
            self.open_path(path, context);
        }
    }

    fn request_preview(&mut self, key: Option<AssetKey>) {
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.stop_audio();
        self.primary = key;
        self.preview = None;
        self.pending_preview = key;
        self.pending_channels = None;
    }

    fn ordered_keys(&self) -> Vec<AssetKey> {
        self.visible
            .iter()
            .map(|&index| self.assets[index].key)
            .collect()
    }

    fn selected_keys(&self) -> Vec<AssetKey> {
        self.assets
            .iter()
            .filter(|asset| self.selected.contains(&asset.key))
            .map(|asset| asset.key)
            .collect()
    }

    fn export_assets(&mut self, keys: Vec<AssetKey>, context: &egui::Context) {
        if keys.is_empty() || self.task.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(output) = rfd::FileDialog::new()
            .set_title("Choose export folder")
            .pick_folder()
        else {
            return;
        };
        let options = self.export_options;
        let model_options = self.model_options;
        self.pending_preview = None;
        self.start_task_with_progress(
            TaskKind::Export,
            "Exporting assets…",
            context,
            move |progress| {
                session
                    .export_with_progress(
                        &keys,
                        &output,
                        &options,
                        &model_options,
                        Some(&progress.observer()),
                    )
                    .map(|report| TaskPayload::Report {
                        operation: "Export",
                        report,
                    })
                    .map_err(|error| error.to_string())
            },
        );
    }

    fn export_map(&mut self, format: MapFormat, context: &egui::Context) {
        if self.task.is_some() || self.visible.is_empty() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let (extension, name) = if format == MapFormat::Json {
            ("json", "assets_map.json")
        } else {
            ("xml", "assets_map.xml")
        };
        let Some(output) = rfd::FileDialog::new()
            .set_title("Save filtered asset list")
            .set_file_name(name)
            .add_filter("Asset list", &[extension])
            .save_file()
        else {
            return;
        };
        let keys = self.ordered_keys();
        self.pending_preview = None;
        self.start_task(
            TaskKind::Export,
            "Writing asset list…",
            context,
            move || {
                session
                    .write_map(&keys, &output, format)
                    .map(|report| TaskPayload::Report {
                        operation: "Asset list",
                        report,
                    })
                    .map_err(|error| error.to_string())
            },
        );
    }

    fn export_scene(&mut self, format: ModelFormat, selected: bool, context: &egui::Context) {
        if self.task.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let roots: Vec<_> = if selected {
            self.assets
                .iter()
                .filter(|asset| self.selected.contains(&asset.key) && asset.class_id == 1)
                .map(|asset| asset.key)
                .collect()
        } else {
            Vec::new()
        };
        if selected && roots.is_empty() {
            self.error("Select at least one GameObject in the asset list or hierarchy first.");
            return;
        }
        let Some(output) = rfd::FileDialog::new()
            .set_title("Choose model export folder")
            .pick_folder()
        else {
            return;
        };
        self.pending_preview = None;
        let model_options = self.model_options;
        self.start_task(
            TaskKind::Export,
            "Exporting scene and textures…",
            context,
            move || {
                session
                    .export_scene_with_options(&output, format, &roots, &model_options)
                    .map(|report| TaskPayload::Report {
                        operation: "Scene export",
                        report,
                    })
                    .map_err(|error| error.to_string())
            },
        );
    }

    fn export_cab_map(&mut self, context: &egui::Context) {
        if self.task.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(base) = rfd::FileDialog::new()
            .set_title("Choose folder containing the loaded source files")
            .pick_folder()
        else {
            return;
        };
        let Some(output) = rfd::FileDialog::new()
            .set_title("Save CAB dependency map")
            .set_file_name("cab_map.bin")
            .add_filter("CAB dependency map", &["bin"])
            .save_file()
        else {
            return;
        };
        let overwrite = self.export_options.overwrite;
        self.pending_preview = None;
        self.start_task(
            TaskKind::Export,
            "Writing CAB dependency map…",
            context,
            move || {
                session
                    .write_cab_map(&base, &output, overwrite)
                    .map(|report| TaskPayload::Report {
                        operation: "CAB dependency map",
                        report,
                    })
                    .map_err(|error| error.to_string())
            },
        );
    }

    fn extract(&mut self, context: &egui::Context) {
        if !self.resolved_sources.is_empty() {
            self.extract_inputs(
                self.resolved_sources.clone(),
                self.source_options.clone(),
                self.source_auxiliary.clone(),
                context,
            );
        }
    }

    fn pick_extraction(&mut self, folder: bool, context: &egui::Context) {
        if self.task.is_some() {
            return;
        }
        let dialog = rfd::FileDialog::new().set_title("Choose input to extract");
        let input = if folder {
            dialog.pick_folder()
        } else {
            dialog.pick_file()
        };
        if let Some(input) = input {
            let options = match self.load_settings.options() {
                Ok(options) => options,
                Err(error) => {
                    self.error(error);
                    return;
                }
            };
            self.extract_input(input, options, context);
        }
    }

    fn extract_input(&mut self, input: PathBuf, options: LoadOptions, context: &egui::Context) {
        self.extract_inputs(vec![input], options, Vec::new(), context);
    }

    fn extract_inputs(
        &mut self,
        inputs: Vec<PathBuf>,
        mut options: LoadOptions,
        mut auxiliary: Vec<PathBuf>,
        context: &egui::Context,
    ) {
        if self.task.is_some() {
            return;
        }
        let Some(output) = rfd::FileDialog::new()
            .set_title("Choose extraction folder")
            .pick_folder()
        else {
            return;
        };
        let overwrite = self.export_options.overwrite;
        let key_specification = self.load_settings.unity_cn_keys_file.trim().to_owned();
        let key_index = self.load_settings.unity_cn_key_index;
        self.pending_preview = None;
        self.start_task(
            TaskKind::Export,
            "Extracting container files…",
            context,
            move || {
                if let Some(keyfile) =
                    resolve_resource_key(&mut options, &key_specification, key_index)?
                {
                    auxiliary.push(keyfile);
                }
                Session::extract_paths_with_auxiliary_inputs(
                    &inputs, &output, &options, overwrite, &auxiliary,
                )
                .map(|report| TaskPayload::Report {
                    operation: "Extraction",
                    report,
                })
                .map_err(|error| error.to_string())
            },
        );
    }

    fn play_audio(&mut self, context: &egui::Context) {
        if self.task.is_some() {
            return;
        }
        let (Some(key), Some(session)) = (self.primary, self.session.clone()) else {
            return;
        };
        self.stop_audio();
        let generation = self.generation;
        let revision = self.audio_revision;
        self.pending_preview = None;
        self.start_task(TaskKind::Audio, "Decoding audio…", context, move || {
            session
                .audio_preview(key)
                .map(|bytes| TaskPayload::Audio {
                    generation,
                    revision,
                    key,
                    bytes,
                })
                .map_err(|error| error.to_string())
        });
    }

    fn stop_audio(&mut self) {
        self.audio_revision = self.audio_revision.wrapping_add(1);
        self.audio.stop();
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        let idle = self.task.is_none();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("AssetStudio")
                    .size(18.0)
                    .strong()
                    .color(ACCENT),
            );
            ui.add_space(12.0);
            ui.add_enabled_ui(idle, |ui| {
                if ui.button("Open file").on_hover_text("Ctrl+O").clicked() {
                    self.open_file(&context);
                }
                if ui
                    .button("Open folder")
                    .on_hover_text("Ctrl+Shift+O")
                    .clicked()
                {
                    self.open_folder(&context);
                }
                if self.session.is_some() && ui.button("Reload").clicked() {
                    self.open_paths(self.sources.clone(), &context);
                }
            });
            if self.session.is_some() {
                ui.separator();
                ui.add_enabled_ui(idle, |ui| {
                    if ui
                        .add_enabled(
                            !self.selected.is_empty(),
                            egui::Button::new(RichText::new("Export selected").color(BACKGROUND))
                                .fill(ACCENT),
                        )
                        .clicked()
                    {
                        self.export_assets(self.selected_keys(), &context);
                    }
                    ui.menu_button("Export", |ui| {
                        if ui
                            .add_enabled(
                                !self.visible.is_empty(),
                                egui::Button::new("Filtered assets"),
                            )
                            .clicked()
                        {
                            ui.close();
                            self.export_assets(self.ordered_keys(), &context);
                        }
                        if ui
                            .add_enabled(
                                !self.selected.is_empty(),
                                egui::Button::new("Selected models · FBX"),
                            )
                            .clicked()
                        {
                            ui.close();
                            self.export_scene(ModelFormat::FbxBinary, true, &context);
                        }
                        ui.separator();
                        for (label, format) in [
                            ("Asset list · JSON", MapFormat::Json),
                            ("Asset list · XML", MapFormat::Xml),
                        ] {
                            if ui.button(label).clicked() {
                                ui.close();
                                self.export_map(format, &context);
                            }
                        }
                        if ui.button("CAB dependency map").clicked() {
                            ui.close();
                            self.export_cab_map(&context);
                        }
                        ui.separator();
                        for (label, format) in [
                            ("Entire scene · FBX", ModelFormat::FbxBinary),
                            ("Entire scene · FBX text", ModelFormat::FbxAscii),
                            ("Entire scene · OBJ", ModelFormat::Obj),
                        ] {
                            if ui.button(label).clicked() {
                                ui.close();
                                self.export_scene(format, false, &context);
                            }
                        }
                    });
                });
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
                ui.add_enabled_ui(idle, |ui| {
                    ui.menu_button("Extract", |ui| {
                        if ui.button("From a file…").clicked() {
                            ui.close();
                            self.pick_extraction(false, &context);
                        }
                        if ui.button("From a folder…").clicked() {
                            ui.close();
                            self.pick_extraction(true, &context);
                        }
                        if self.session.is_some() && ui.button("Current inputs").clicked() {
                            ui.close();
                            self.extract(&context);
                        }
                    });
                });
            });
        });
    }

    fn operation_progress(&self, ui: &mut egui::Ui, task: &Task) {
        let update = self.task_progress.snapshot();
        let stage = if update.stage == "Working" {
            task.label
        } else {
            update.stage
        };
        let fraction = update
            .total
            .filter(|&total| total != 0)
            .map(|total| (update.completed as f64 / total as f64).clamp(0.0, 1.0) as f32);
        ui.horizontal(|ui| {
            ui.label(RichText::new(stage).color(ACCENT));
            if let Some(total) = update.total.filter(|&total| total != 0) {
                ui.label(
                    RichText::new(format!(
                        "{} / {} · {:.0}%",
                        update.completed.min(total),
                        total,
                        fraction.unwrap_or(0.0) * 100.0
                    ))
                    .small()
                    .color(MUTED),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(self.task_progress.elapsed())
                        .small()
                        .color(MUTED),
                );
            });
        });
        if let Some(fraction) = fraction {
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_height(4.0)
                    .corner_radius(0)
                    .fill(ACCENT),
            );
        } else {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ProgressIndicator, true, stage)
            });
            ui.painter().rect_filled(rect, 0.0, Color32::from_gray(35));
            let segment = rect.width() * 0.18;
            let position = ui.input(|input| (input.time * 0.5).fract()) as f32;
            let x = rect.left() - segment + position * (rect.width() + segment);
            let fill = egui::Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(segment, rect.height()),
            )
            .intersect(rect);
            ui.painter().rect_filled(fill, 0.0, ACCENT);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("Files").small().strong().color(MUTED));
        ui.add_space(8.0);
        if let Some(source) = &self.source {
            ui.label(
                RichText::new(
                    source
                        .file_name()
                        .unwrap_or(source.as_os_str())
                        .to_string_lossy(),
                )
                .strong(),
            );
            ui.label(
                RichText::new(format!(
                    "{} objects · {} types",
                    self.assets.len(),
                    self.classes.len()
                ))
                .small()
                .color(MUTED),
            )
            .on_hover_text(source.display().to_string());
            ui.add_space(6.0);
        } else {
            ui.label(RichText::new("No files open").color(MUTED));
        }
        ui.add_space(15.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Types").small().strong().color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tooltip = match self.type_sort {
                    TypeSort::Alphabetical => "Alphabetical · click for most assets first",
                    TypeSort::Most => "Most assets first · click for fewest first",
                    TypeSort::Least => "Fewest assets first · click for alphabetical",
                };
                if ui
                    .small_button(self.type_sort.label())
                    .on_hover_text(tooltip)
                    .clicked()
                {
                    self.type_sort = self.type_sort.next();
                }
            });
        });
        ui.add(
            TextEdit::singleline(&mut self.type_search)
                .hint_text("Search types…")
                .desired_width(ui.available_width()),
        );
        ui.add_space(6.0);
        if ui
            .selectable_label(
                self.filters.classes.is_empty(),
                format!("All types ({})", self.assets.len()),
            )
            .clicked()
        {
            self.filters.classes.clear();
            self.filter_dirty = true;
        }
        egui::ScrollArea::vertical()
            .id_salt("type-filter")
            .max_height(260.0)
            .show(ui, |ui| {
                let types = self.type_sort.rows(&self.classes, &self.type_search);
                if types.is_empty() {
                    ui.label(RichText::new("No matching types").small().color(MUTED));
                }
                for (class, count) in types {
                    let mut selected = self.filters.classes.contains(class);
                    if ui
                        .checkbox(&mut selected, format!("{class}  {count}"))
                        .changed()
                    {
                        if selected {
                            self.filters.classes.insert(class.to_owned());
                        } else {
                            self.filters.classes.remove(class);
                        }
                        self.filter_dirty = true;
                    }
                }
            });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Export options").show(ui, |ui| {
            ui.label("Data format");
            egui::ComboBox::from_id_salt("export-mode")
                .width(ui.available_width())
                .selected_text(mode_label(self.export_options.mode))
                .show_ui(ui, |ui| {
                    for mode in [
                        ExportMode::Convert,
                        ExportMode::Raw,
                        ExportMode::Dump,
                        ExportMode::Json,
                    ] {
                        ui.selectable_value(&mut self.export_options.mode, mode, mode_label(mode));
                    }
                });
            ui.add_space(5.0);
            ui.label("Group files by");
            egui::ComboBox::from_id_salt("export-group")
                .width(ui.available_width())
                .selected_text(group_label(self.export_options.group))
                .show_ui(ui, |ui| {
                    for group in [
                        GroupMode::ByType,
                        GroupMode::ByContainer,
                        GroupMode::BySource,
                        GroupMode::None,
                    ] {
                        ui.selectable_value(
                            &mut self.export_options.group,
                            group,
                            group_label(group),
                        );
                    }
                });
            ui.add_space(5.0);
            ui.label("Image format");
            egui::ComboBox::from_id_salt("image-format")
                .width(ui.available_width())
                .selected_text(image_label(self.export_options.image_format))
                .show_ui(ui, |ui| {
                    for format in [
                        ImageFormat::Png,
                        ImageFormat::Jpeg,
                        ImageFormat::Bmp,
                        ImageFormat::Tga,
                    ] {
                        ui.selectable_value(
                            &mut self.export_options.image_format,
                            format,
                            image_label(format),
                        );
                    }
                });
            ui.add_space(8.0);
            ui.checkbox(
                &mut self.model_options.include_animations,
                "Include model animations",
            );
            ui.checkbox(
                &mut self.model_options.include_textures,
                "Include model textures",
            );
            egui::CollapsingHeader::new("Model and FBX options").show(ui, |ui| {
                ui.checkbox(
                    &mut self.model_options.include_materials,
                    "Include materials",
                );
                ui.checkbox(&mut self.model_options.include_skins, "Include skinning");
                ui.checkbox(
                    &mut self.model_options.include_blend_shapes,
                    "Include blend shapes",
                );
                ui.checkbox(
                    &mut self.model_options.export_all_nodes,
                    "Include nodes without geometry",
                );
                ui.checkbox(
                    &mut self.model_options.euler_filter,
                    "Unwrap Euler rotations",
                );
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    ui.add(
                        egui::DragValue::new(&mut self.model_options.scale_factor)
                            .range(0.000_001..=1_000_000.0)
                            .speed(0.1),
                    );
                });
                egui::ComboBox::from_id_salt("fbx-version")
                    .selected_text(format!(
                        "FBX {}.{}",
                        self.model_options.fbx.version / 1000,
                        (self.model_options.fbx.version / 100) % 10,
                    ))
                    .show_ui(ui, |ui| {
                        for (version, label) in [
                            (7100, "FBX 7.1"),
                            (7200, "FBX 7.2"),
                            (7300, "FBX 7.3"),
                            (7400, "FBX 7.4"),
                            (7500, "FBX 7.5"),
                        ] {
                            ui.selectable_value(
                                &mut self.model_options.fbx.version,
                                version,
                                label,
                            );
                        }
                    });
                ui.checkbox(
                    &mut self.model_options.fbx.cast_to_bone,
                    "Show transform nodes as bones",
                );
                ui.horizontal(|ui| {
                    ui.label("Bone display size");
                    ui.add(
                        egui::DragValue::new(&mut self.model_options.fbx.bone_size)
                            .range(f64::MIN_POSITIVE..=1_000_000.0)
                            .speed(1.0),
                    );
                });
                ui.label(
                    RichText::new("UV channels and FBX roles")
                        .small()
                        .color(MUTED),
                );
                for channel in 0..8 {
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut self.model_options.uv_enabled[channel],
                            format!("UV{channel}"),
                        );
                        ui.add_enabled_ui(self.model_options.uv_enabled[channel], |ui| {
                            egui::ComboBox::from_id_salt(("fbx-uv-role", channel))
                                .width(105.0)
                                .selected_text(uv_role_label(
                                    self.model_options.fbx.uv_types[channel],
                                ))
                                .show_ui(ui, |ui| {
                                    for role in [
                                        FbxUvType::Diffuse,
                                        FbxUvType::NormalMap,
                                        FbxUvType::Specular,
                                        FbxUvType::Bump,
                                        FbxUvType::Ambient,
                                        FbxUvType::Emissive,
                                        FbxUvType::Reflection,
                                        FbxUvType::Displacement,
                                    ] {
                                        ui.selectable_value(
                                            &mut self.model_options.fbx.uv_types[channel],
                                            role,
                                            uv_role_label(role),
                                        );
                                    }
                                });
                        });
                    });
                }
            });
            ui.add_space(5.0);
            ui.checkbox(&mut self.export_options.overwrite, "Replace existing files");
            if self.export_options.overwrite {
                ui.label(
                    RichText::new("Existing exports with matching names may be replaced.")
                        .small()
                        .color(Color32::from_rgb(235, 187, 112)),
                );
            }
        });
        ui.add_space(15.0);
        ui.label(
            RichText::new("Source files are opened read-only.")
                .small()
                .color(MUTED),
        );
    }

    fn asset_table(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let name = ui.add(
                TextEdit::singleline(&mut self.filters.name)
                    .hint_text("Search names…")
                    .desired_width((ui.available_width() * 0.5).max(100.0)),
            );
            let container = ui.add(
                TextEdit::singleline(&mut self.filters.container)
                    .hint_text("Filter container…")
                    .desired_width(170.0),
            );
            let regex = ui.checkbox(&mut self.filters.regex, "Regex");
            self.filter_dirty |= name.changed() || container.changed() || regex.changed();
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} shown / {} total",
                    self.visible.len(),
                    self.assets.len()
                ))
                .small()
                .color(MUTED),
            );
            if self.pending_filter
                || self
                    .task
                    .as_ref()
                    .is_some_and(|task| task.kind == TaskKind::Filter)
            {
                ui.spinner();
                ui.label(RichText::new("Updating…").small().color(MUTED));
            }
            if ui.small_button("Select filtered").clicked() {
                self.selected
                    .extend(self.visible.iter().map(|&index| self.assets[index].key));
                if self.primary.is_none() {
                    self.request_preview(self.visible.first().map(|&index| self.assets[index].key));
                }
            }
            if ui.small_button("Clear selection").clicked() {
                self.selected.clear();
                self.anchor = None;
                self.request_preview(None);
            }
            let previous_sort = self.sort;
            egui::ComboBox::from_id_salt("sort-column")
                .selected_text("Sort by…")
                .width(100.0)
                .show_ui(ui, |ui| {
                    for (column, label) in [
                        (SortColumn::Name, "Name"),
                        (SortColumn::Class, "Type"),
                        (SortColumn::Size, "Size"),
                        (SortColumn::Container, "Container"),
                        (SortColumn::Source, "Source file"),
                        (SortColumn::PathId, "Path ID"),
                    ] {
                        ui.selectable_value(&mut self.sort, column, label);
                    }
                });
            self.filter_dirty |= self.sort != previous_sort;
        });
        if let Some(error) = &self.filter_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        ui.separator();
        let width = ui.available_width();
        let widths = [
            (width * 0.38).max(110.0),
            108.0,
            82.0,
            (width - width * 0.38 - 108.0 - 82.0 - 42.0).max(90.0),
        ];
        ui.horizontal(|ui| {
            for (label, column, width) in [
                ("Name", SortColumn::Name, widths[0]),
                ("Type", SortColumn::Class, widths[1]),
                ("Size", SortColumn::Size, widths[2]),
                ("Container / source", SortColumn::Container, widths[3]),
            ] {
                let arrow = if self.sort == column {
                    if self.descending { " ↓" } else { " ↑" }
                } else {
                    ""
                };
                if ui
                    .add_sized(
                        [width, 25.0],
                        egui::Button::new(RichText::new(format!("{label}{arrow}")).small())
                            .frame(false),
                    )
                    .clicked()
                {
                    if self.sort == column {
                        self.descending = !self.descending;
                    } else {
                        self.sort = column;
                        self.descending = false;
                    }
                    self.filter_dirty = true;
                }
            }
        });
        let mut clicked = None;
        egui::ScrollArea::both()
            .id_salt("asset-table")
            .auto_shrink([false, false])
            .show_rows(ui, 29.0, self.visible.len(), |ui, range| {
                ui.set_min_width(width.max(520.0));
                for row in range {
                    let asset = &self.assets[self.visible[row]];
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 29.0), Sense::click());
                    let background = if self.selected.contains(&asset.key) {
                        Color32::from_gray(42)
                    } else if response.hovered() {
                        Color32::from_gray(27)
                    } else if row % 2 == 0 {
                        Color32::from_gray(13)
                    } else {
                        BACKGROUND
                    };
                    ui.painter().rect_filled(rect, 2.0, background);
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(egui::vec2(4.0, 2.0)))
                            .layout(Layout::left_to_right(Align::Center)),
                    );
                    // Cells belong to the row hit target. Selectable labels
                    // otherwise consume clicks and show a text-edit cursor.
                    row_ui.style_mut().interaction.selectable_labels = false;
                    row_ui.add_sized([widths[0], 25.0], egui::Label::new(&asset.name).truncate());
                    row_ui.add_sized(
                        [widths[1], 25.0],
                        egui::Label::new(RichText::new(&asset.class_name).color(MUTED)).truncate(),
                    );
                    row_ui.add_sized(
                        [widths[2], 25.0],
                        egui::Label::new(
                            RichText::new(view::byte_size(asset.size))
                                .monospace()
                                .small(),
                        )
                        .truncate(),
                    );
                    let location = if asset.container.is_empty() {
                        &asset.source
                    } else {
                        &asset.container
                    };
                    row_ui.add_sized(
                        [widths[3], 25.0],
                        egui::Label::new(RichText::new(location).small().color(MUTED)).truncate(),
                    );
                    if response.clicked() {
                        let modifiers = ui.input(|input| input.modifiers);
                        clicked = Some((asset.key, modifiers.command, modifiers.shift));
                    }
                    response.on_hover_text(format!(
                        "{}\n{}\nPath ID: {} · File: {}\n{}",
                        asset.name,
                        asset.class_name,
                        asset.key.path_id,
                        asset.key.file_index,
                        asset.source
                    ));
                }
            });
        if let Some((key, toggle, range)) = clicked {
            let keys = if range || (toggle && self.primary == Some(key)) {
                self.ordered_keys()
            } else {
                Vec::new()
            };
            let previous = self.primary;
            view::select_row(
                &mut self.selected,
                &mut self.primary,
                &mut self.anchor,
                &keys,
                key,
                toggle,
                range,
            );
            if self.primary != previous {
                self.request_preview(self.primary);
            }
        }
    }

    fn hierarchy(&mut self, ui: &mut egui::Ui) {
        if !self.scene_loaded {
            self.pending_scene = true;
            ui.spinner();
            ui.label("Building GameObject hierarchy…");
            return;
        }
        if let Some(error) = &self.scene_error {
            ui.colored_label(Color32::LIGHT_RED, error);
            if ui.button("Try again").clicked() {
                self.scene_loaded = false;
                self.pending_scene = true;
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} GameObjects", self.scene.len()))
                    .small()
                    .color(MUTED),
            );
            ui.label(
                RichText::new("Select a GameObject, then use Export → Selected models.")
                    .small()
                    .color(MUTED),
            );
        });
        if self.scene.is_empty() {
            ui.label("No GameObject hierarchy was found in these files.");
        }
        let mut clicked = None;
        egui::ScrollArea::both()
            .id_salt("hierarchy")
            .auto_shrink([false, false])
            .show_rows(ui, 27.0, self.scene_rows.len(), |ui, range| {
                for row in range {
                    let (index, depth) = self.scene_rows[row];
                    let node = &self.scene[index];
                    ui.horizontal(|ui| {
                        ui.add_space((depth.min(40) * 15) as f32);
                        ui.label(RichText::new("◇").color(ACCENT));
                        if ui
                            .selectable_label(self.selected.contains(&node.key), &node.name)
                            .clicked()
                        {
                            clicked = Some((node.key, ui.input(|input| input.modifiers.command)));
                        }
                    });
                }
            });
        if let Some((key, toggle)) = clicked {
            if !toggle {
                self.selected.clear();
            }
            if !self.selected.remove(&key) {
                self.selected.insert(key);
            }
            self.anchor = Some(key);
            self.request_preview(self.selected.contains(&key).then_some(key));
        }
    }

    fn export_class_structures(&mut self, context: &egui::Context) {
        if self.task.is_some() || self.class_structures.is_empty() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(output) = rfd::FileDialog::new()
            .set_title("Choose class definition folder")
            .pick_folder()
        else {
            return;
        };
        self.pending_preview = None;
        self.start_task(
            TaskKind::Export,
            "Exporting class definitions…",
            context,
            move || {
                session
                    .export_class_structures(&output)
                    .map(|report| TaskPayload::Report {
                        operation: "Class structures",
                        report,
                    })
                    .map_err(|error| error.to_string())
            },
        );
    }

    fn class_structures_table(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} embedded class definitions",
                self.class_structures.len()
            ));
            if ui
                .add_enabled(
                    self.task.is_none() && !self.class_structures.is_empty(),
                    egui::Button::new("Export class structures…"),
                )
                .clicked()
            {
                self.export_class_structures(ui.ctx());
            }
        });
        ui.label(RichText::new("Grouped by Unity version and class identity. Later loaded definitions take priority.").small().color(MUTED));
        ui.separator();
        if let Some(error) = &self.classes_error {
            ui.colored_label(Color32::LIGHT_RED, error);
            return;
        }
        if !self.classes_loaded {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading class definitions…");
            });
            return;
        }
        if self.class_structures.is_empty() {
            ui.label("No embedded type trees were loaded. Stripped files do not contain class definitions.");
            return;
        }
        egui::ScrollArea::both()
            .id_salt("class-structures")
            .auto_shrink([false, false])
            .show_rows(ui, 27.0, self.class_structures.len(), |ui, range| {
                for row in range {
                    let definition = &self.class_structures[row];
                    let text = format!(
                        "{}  ·  {}  ·  {}",
                        definition.unity_version, definition.class_id, definition.name
                    );
                    if ui
                        .add_sized(
                            [ui.available_width(), 25.0],
                            egui::Button::selectable(self.selected_class == Some(row), text)
                                .truncate(),
                        )
                        .on_hover_text(&definition.source)
                        .clicked()
                    {
                        self.selected_class = Some(row);
                    }
                }
            });
    }

    fn class_structure_preview(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("CLASS STRUCTURE")
                .small()
                .strong()
                .color(MUTED),
        );
        let Some(definition) = self
            .selected_class
            .and_then(|row| self.class_structures.get(row))
        else {
            ui.label("Select an embedded class definition.");
            return;
        };
        ui.label(RichText::new(&definition.name).strong());
        ui.label(format!(
            "Unity {} · type {} · class {}",
            definition.unity_version, definition.class_id, definition.serialized_class_id
        ));
        if ui.small_button("Copy class structure").clicked() {
            ui.ctx().copy_text(definition.text.clone());
        }
        ui.separator();
        let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let lines = definition.text.lines().count();
        // Virtual rows avoid laying out a multi-megabyte class text document
        // on every frame. Copy/export still retain the complete definition.
        egui::ScrollArea::both()
            .id_salt("class-structure-text")
            .auto_shrink([false, false])
            .show_rows(ui, line_height, lines, |ui, range| {
                for line in definition.text.lines().skip(range.start).take(range.len()) {
                    ui.add(
                        egui::Label::new(RichText::new(line).monospace())
                            .extend()
                            .selectable(true),
                    );
                }
            });
    }

    fn activity_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Most recent {} messages", self.log.len()))
                    .small()
                    .color(MUTED),
            );
            if ui.button("Copy log").clicked() {
                ui.ctx().copy_text(
                    self.log
                        .iter()
                        .map(|entry| entry.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            if ui.button("Clear").clicked() {
                self.log.clear();
            }
        });
        egui::ScrollArea::both()
            .id_salt("activity-log")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in &self.log {
                    ui.label(RichText::new(&entry.text).monospace().size(12.0).color(
                        if entry.error {
                            Color32::LIGHT_RED
                        } else {
                            MUTED
                        },
                    ));
                    ui.add_space(3.0);
                }
            });
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("PREVIEW").small().strong().color(MUTED));
        ui.add_space(10.0);
        let Some(key) = self.primary else {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Select an asset").size(21.0));
                ui.add_space(7.0);
                ui.label(
                    RichText::new("Images, text, audio, and meshes appear here.").color(MUTED),
                );
            });
            return;
        };
        let Some(asset) = self
            .by_key
            .get(&key)
            .and_then(|&index| self.assets.get(index))
        else {
            return;
        };
        ui.label(RichText::new(&asset.name).size(19.0).strong());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&asset.class_name).color(ACCENT));
            ui.label(
                RichText::new(view::byte_size(asset.size))
                    .small()
                    .color(MUTED),
            );
        });
        let is_audio = asset.class_id == 83;
        egui::CollapsingHeader::new("Object details")
            .id_salt("preview-details")
            .show(ui, |ui| {
                egui::Grid::new("metadata")
                    .num_columns(2)
                    .spacing([14.0, 5.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Path ID", key.path_id.to_string()),
                            ("File index", key.file_index.to_string()),
                            ("Class ID", asset.class_id.to_string()),
                            ("Container", asset.container.clone()),
                            ("Source", asset.source.clone()),
                        ] {
                            ui.label(RichText::new(label).small().color(MUTED));
                            ui.add(
                                egui::Label::new(RichText::new(value).small())
                                    .wrap()
                                    .selectable(true),
                            );
                            ui.end_row();
                        }
                    });
            });
        ui.separator();
        let previous_mode = self.preview_mode;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.preview_mode, PreviewMode::Preview, "Preview");
            ui.selectable_value(&mut self.preview_mode, PreviewMode::Dump, "Dump");
            ui.selectable_value(&mut self.preview_mode, PreviewMode::Json, "JSON");
        });
        if self.preview_mode != previous_mode {
            self.request_preview(Some(key));
        }
        ui.separator();
        if is_audio && self.preview_mode == PreviewMode::Preview {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.task.is_none(), egui::Button::new("Play"))
                    .clicked()
                {
                    self.play_audio(ui.ctx());
                }
                if self.audio.is_playing() && !self.audio.is_paused() {
                    if ui.button("Pause").clicked() {
                        self.audio.pause();
                    }
                } else if self.audio.is_paused() && ui.button("Resume").clicked() {
                    self.audio.resume();
                }
                if ui.button("Stop").clicked() {
                    self.stop_audio();
                }
            });
            ui.horizontal(|ui| {
                let mut volume = self.audio.volume();
                if ui
                    .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("Volume"))
                    .changed()
                {
                    self.audio.set_volume(volume);
                }
                let mut looping = self.audio.is_looping();
                if ui.checkbox(&mut looping, "Loop").changed() {
                    self.audio.set_looping(looping);
                }
            });
            if self.audio.is_playing() || self.audio.is_paused() {
                let mut position = self.audio.position().as_secs_f32();
                if let Some(duration) = self.audio.duration() {
                    let length = duration.as_secs_f32();
                    if length > 0.0
                        && ui
                            .add(egui::Slider::new(&mut position, 0.0..=length).text("Seconds"))
                            .changed()
                        && let Err(error) = self
                            .audio
                            .seek(Duration::from_secs_f32(position.clamp(0.0, length)))
                    {
                        self.error(format!("Could not seek this audio clip: {error}"));
                    }
                } else {
                    ui.label(format!("{position:.1} seconds"));
                }
            }
            ui.separator();
        }
        match &mut self.preview {
            None => {
                if self
                    .task
                    .as_ref()
                    .is_some_and(|task| task.kind == TaskKind::Preview)
                    || self.pending_preview.is_some()
                {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading preview…");
                    });
                } else {
                    ui.label(RichText::new("No preview available.").color(MUTED));
                }
            }
            Some(DisplayPreview::Error(error)) => {
                ui.colored_label(Color32::LIGHT_RED, error.as_str());
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Raw export may still be available for this object.")
                        .small()
                        .color(MUTED),
                );
            }
            Some(DisplayPreview::Text(text)) => {
                if ui.small_button("Copy text").clicked() {
                    ui.ctx().copy_text(text.clone());
                }
                egui::ScrollArea::both()
                    .id_salt("text-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut text.as_str())
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
            }
            Some(DisplayPreview::Image(image)) => {
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.label("Channels");
                    for (label, channel, shortcut) in [
                        ("R", &mut image.channels.red, egui::Key::R),
                        ("G", &mut image.channels.green, egui::Key::G),
                        ("B", &mut image.channels.blue, egui::Key::B),
                        ("A", &mut image.channels.alpha, egui::Key::A),
                    ] {
                        changed |= ui.checkbox(channel, label).changed();
                        if !ui.ctx().wants_keyboard_input()
                            && ui.input_mut(|input| {
                                input.consume_shortcut(&egui::KeyboardShortcut::new(
                                    egui::Modifiers::COMMAND,
                                    shortcut,
                                ))
                            })
                        {
                            *channel = !*channel;
                            changed = true;
                        }
                    }
                });
                if changed {
                    image.revision = image.revision.wrapping_add(1);
                    self.pending_channels = Some((key, image.revision));
                }
                let texture = &image.texture;
                ui.horizontal(|ui| {
                    ui.label(format!("{} × {}", texture.size()[0], texture.size()[1]));
                    ui.checkbox(&mut self.image_fit, "Fit");
                    if !self.image_fit {
                        ui.add(
                            egui::Slider::new(&mut self.image_zoom, 0.1..=8.0)
                                .logarithmic(true)
                                .text("Zoom"),
                        );
                    }
                });
                let size = texture.size_vec2();
                let available = ui.available_size();
                let factor = if self.image_fit {
                    (available.x / size.x)
                        .min(available.y / size.y)
                        .clamp(0.01, 1.0)
                } else {
                    self.image_zoom
                };
                egui::ScrollArea::both()
                    .id_salt("image-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(size * factor, Sense::hover());
                        checkerboard(ui.painter(), rect);
                        ui.painter().image(
                            texture.id(),
                            rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    });
            }
            Some(DisplayPreview::Mesh(mesh)) => {
                if !ui.ctx().wants_keyboard_input() {
                    let shortcuts = ui.input_mut(|input| {
                        [egui::Key::W, egui::Key::S, egui::Key::N].map(|key| {
                            input.consume_shortcut(&egui::KeyboardShortcut::new(
                                egui::Modifiers::COMMAND,
                                key,
                            ))
                        })
                    });
                    if shortcuts[0] {
                        mesh.cycle_wireframe();
                    }
                    if shortcuts[1] && mesh.toggle_vertex_colors() {
                        self.mesh_shading = mesh.shading;
                    }
                    if shortcuts[2] && mesh.toggle_normals() {
                        self.mesh_shading = mesh.shading;
                    }
                }
                ui.horizontal(|ui| {
                    for (mode, label) in [
                        (MeshMode::Shaded, "Shaded"),
                        (MeshMode::Wireframe, "Wireframe"),
                        (MeshMode::Combined, "Both"),
                    ] {
                        ui.selectable_value(&mut mesh.mode, mode, label);
                    }
                    if ui.small_button("Reset view").clicked() {
                        mesh.reset_camera();
                    }
                });
                self.mesh_mode = mesh.mode;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Surface");
                    for shading in [
                        MeshShading::Flat,
                        MeshShading::CalculatedNormals,
                        MeshShading::SourceNormals,
                        MeshShading::VertexColors,
                    ] {
                        if ui
                            .add_enabled(
                                mesh.supports_shading(shading),
                                egui::Button::new(shading.label())
                                    .selected(mesh.shading == shading),
                            )
                            .clicked()
                        {
                            mesh.set_shading(shading);
                            self.mesh_shading = shading;
                        }
                    }
                });
                if !mesh.supports_shading(self.mesh_shading) {
                    ui.label(
                        RichText::new(format!(
                            "{} are unavailable in this mesh; flat shading is shown.",
                            self.mesh_shading.label()
                        ))
                        .small()
                        .color(MUTED),
                    );
                }
                ui.label(
                    RichText::new(format!(
                        "{} vertices · {} triangles",
                        mesh.vertex_count(),
                        mesh.original_triangles
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.label(
                    RichText::new("Drag to rotate · right drag to pan · scroll to zoom · Ctrl/Cmd W/S/N: wire, color, normals")
                        .small()
                        .color(MUTED),
                );
                if mesh.is_simplified() {
                    ui.label(
                        RichText::new(
                            "Preview detail is reduced; exports retain the complete mesh.",
                        )
                        .small()
                        .color(MUTED),
                    );
                }
                let (rect, response) = ui.allocate_exact_size(
                    ui.available_size().max(egui::vec2(100.0, 100.0)),
                    Sense::drag(),
                );
                ui.painter().rect_filled(rect, 6.0, BACKGROUND);
                if response.dragged_by(egui::PointerButton::Primary) {
                    let delta = response.drag_delta();
                    mesh.yaw += delta.x * 0.01;
                    mesh.pitch = (mesh.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                }
                if response.dragged_by(egui::PointerButton::Secondary) {
                    let delta = response.drag_delta();
                    mesh.pan[0] = (mesh.pan[0] + delta.x).clamp(-100_000.0, 100_000.0);
                    mesh.pan[1] = (mesh.pan[1] + delta.y).clamp(-100_000.0, 100_000.0);
                }
                if response.hovered() {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    mesh.zoom = (mesh.zoom * (scroll * 0.002).exp()).clamp(0.2, 8.0);
                }
                let painter = ui.painter().with_clip_rect(rect);
                match mesh.faces(rect) {
                    Ok(faces) => {
                        for face in faces {
                            if mesh.mode != MeshMode::Wireframe {
                                painter.add(egui::Shape::mesh(
                                    crate::preview_geometry::shaded_mesh(&face, ACCENT),
                                ));
                            }
                            if mesh.mode != MeshMode::Shaded {
                                painter.add(egui::Shape::closed_line(
                                    face.points.to_vec(),
                                    egui::Stroke::new(0.8_f32, Color32::from_gray(190)),
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            error,
                            FontId::proportional(14.0),
                            Color32::LIGHT_RED,
                        );
                    }
                }
            }
        }
    }

    fn settings_window(&mut self, context: &egui::Context) {
        let mut open = self.settings_open;
        egui::Window::new("Load settings").open(&mut open).collapsible(false).resizable(false).default_width(470.0).vscroll(true).max_height((context.content_rect().height() - 60.0).max(240.0)).show(context, |ui| {
            ui.label("Changes apply the next time you open or reload assets.");
            ui.add_space(10.0);
            egui::Grid::new("load-settings").num_columns(2).spacing([20.0, 12.0]).show(ui, |ui| {
                ui.label("Game profile");
                egui::ComboBox::from_id_salt("game-profile").selected_text(&self.load_settings.game).show_ui(ui, |ui| {
                    for &profile in assetstudio_core::games::supported_profiles() {
                        ui.selectable_value(&mut self.load_settings.game, profile.to_owned(), profile);
                    }
                });
                ui.end_row();
                ui.label("Maximum catalog assets"); ui.add(TextEdit::singleline(&mut self.load_settings.maximum_assets).hint_text("10000000 (1 to 10000000)")); ui.end_row();
                ui.label("Unity version override"); ui.add(TextEdit::singleline(&mut self.load_settings.unity_version).hint_text("Use the version stored in the file")); ui.end_row();
                ui.label("UnityCN key file");
                ui.horizontal(|ui| {
                    let response = ui.add(TextEdit::singleline(&mut self.load_settings.unity_cn_keys_file).hint_text("Blank: Keys.json beside the applications").desired_width(180.0));
                    if response.changed() { self.unity_cn_names.clear(); }
                    if ui.small_button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("UnityCN key file", &["json"]).pick_file()
                    {
                        self.load_settings.unity_cn_keys_file = path.to_string_lossy().into_owned();
                        self.unity_cn_names.clear();
                        self.pending_key_names = Some(self.load_settings.unity_cn_keys_file.trim().to_owned());
                    }
                });
                ui.end_row();
                ui.label("UnityCN key selection");
                ui.horizontal(|ui| {
                    if self.unity_cn_names.is_empty() {
                        ui.add(egui::DragValue::new(&mut self.load_settings.unity_cn_key_index).range(0..=1023).prefix("Index "));
                    } else {
                        let selected = self.unity_cn_names.get(self.load_settings.unity_cn_key_index).map_or_else(|| format!("Index {} (unavailable)", self.load_settings.unity_cn_key_index), |name| format!("{}: {name}", self.load_settings.unity_cn_key_index));
                        egui::ComboBox::from_id_salt("unity-cn-key-index").selected_text(selected).width(180.0).show_ui(ui, |ui| {
                            for (index, name) in self.unity_cn_names.iter().enumerate() {
                                ui.selectable_value(&mut self.load_settings.unity_cn_key_index, index, format!("{index}: {name}"));
                            }
                        });
                    }
                    if ui.small_button("Load names").clicked() {
                        self.pending_key_names = Some(self.load_settings.unity_cn_keys_file.trim().to_owned());
                    }
                });
                ui.end_row();
                ui.label("Explicit UnityCN key"); ui.add(TextEdit::singleline(&mut self.load_settings.unity_cn_key).password(true).hint_text("Overrides the selected resource key")); ui.end_row();
                ui.label("Binary data XOR key"); ui.add(TextEdit::singleline(&mut self.load_settings.xor_key).hint_text("0 or 0x00")); ui.end_row();
                ui.label("Asset index JSON");
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.load_settings.asset_index).hint_text("Optional container name recovery").desired_width(180.0));
                    if ui.small_button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("Asset index", &["json"]).pick_file()
                    { self.load_settings.asset_index = path.to_string_lossy().into_owned(); }
                });
                ui.end_row();
                ui.label("Behaviour schemas");
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.load_settings.schemas).hint_text("Optional trusted schema JSON").desired_width(180.0));
                    if ui.small_button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("Behaviour schemas", &["json"]).pick_file()
                    { self.load_settings.schemas = path.to_string_lossy().into_owned(); }
                });
                ui.end_row();
                ui.label("Managed assemblies");
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.load_settings.dummy_dlls).hint_text("Folder containing Assembly-CSharp.dll").desired_width(180.0));
                    if ui.small_button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new().set_title("Choose managed assembly folder").pick_folder()
                    { self.load_settings.dummy_dlls = path.to_string_lossy().into_owned(); }
                });
                ui.end_row();
                ui.label("CAB dependency map");
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.load_settings.cab_map).hint_text("Optional CAB map .bin file").desired_width(180.0));
                    if ui.small_button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("CAB map", &["bin"]).pick_file()
                    { self.load_settings.cab_map = path.to_string_lossy().into_owned(); }
                });
                ui.end_row();
                ui.label("CAB base folder");
                ui.add_enabled_ui(!self.load_settings.cab_map.trim().is_empty(), |ui| {
                    ui.horizontal(|ui| {
                        ui.add(TextEdit::singleline(&mut self.load_settings.cab_base).hint_text("Blank: source folder / common parent").desired_width(180.0));
                        if ui.small_button("Browse…").clicked()
                            && let Some(path) = rfd::FileDialog::new().set_title("Choose CAB base folder").pick_folder()
                        { self.load_settings.cab_base = path.to_string_lossy().into_owned(); }
                    });
                });
                ui.end_row();
            });
            ui.add_space(10.0);
            ui.label(RichText::new("UnityCN uses the selected file/index when the explicit key is blank. An explicit key takes precedence.").small().color(MUTED));
            ui.checkbox(&mut self.load_settings.continue_on_error, "Continue loading other files after an error");
            ui.label(RichText::new("Any skipped or unrecognized files are listed in the activity log.").small().color(MUTED));
            if let Err(error) = self.load_settings.options() { ui.colored_label(Color32::LIGHT_RED, error); }
            ui.add_space(10.0);
            ui.label(RichText::new("Only game profiles implemented by this build can be loaded. An unknown profile reports an error.").small().color(MUTED));
        });
        self.settings_open = open;
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        ui.add_space((ui.available_height() * 0.18).max(20.0));
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Open assets").size(26.0).strong());
            ui.add_space(10.0);
            ui.label(
                RichText::new("Choose a file or folder to browse and export.")
                    .size(14.0)
                    .color(MUTED),
            );
            ui.add_space(25.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 272.0).max(0.0) * 0.5);
                if ui
                    .add_sized(
                        [130.0, 40.0],
                        egui::Button::new(RichText::new("Open file").color(BACKGROUND))
                            .fill(ACCENT),
                    )
                    .clicked()
                {
                    self.open_file(&context);
                }
                if ui
                    .add_sized([130.0, 40.0], egui::Button::new("Open folder"))
                    .clicked()
                {
                    self.open_folder(&context);
                }
            });
            ui.add_space(14.0);
            ui.label(
                RichText::new("You can also drop a file or folder anywhere in this window.")
                    .small()
                    .color(MUTED),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 500.0).max(0.0) * 0.5);
                let input = ui.add_sized(
                    [400.0, 30.0],
                    TextEdit::singleline(&mut self.input_path)
                        .hint_text("Paste a file or folder path…"),
                );
                if (ui.button("Open path").clicked()
                    || (input.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))))
                    && !self.input_path.trim().is_empty()
                {
                    self.open_path(PathBuf::from(self.input_path.trim()), &context);
                }
            });
        });
    }

    pub fn show(&mut self, context: &egui::Context) {
        self.poll_task(context);
        if self.filter_dirty {
            self.filter_dirty = false;
            self.queue_filter();
        }
        if self.task.is_none() {
            let dropped = context.input(|input| {
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect::<Vec<_>>()
            });
            if !dropped.is_empty() {
                self.open_paths(dropped, context);
            }
            if context.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::O,
                ))
            }) {
                self.open_folder(context);
            } else if context.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::O,
                ))
            }) {
                self.open_file(context);
            }
            if !self.selected.is_empty()
                && context.input_mut(|input| {
                    input.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::COMMAND,
                        egui::Key::E,
                    ))
                })
            {
                self.export_assets(self.selected_keys(), context);
            }
        }
        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(context, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(15, 8)),
            )
            .show(context, |ui| {
                if let Some(task) = &self.task {
                    self.operation_progress(ui, task);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.status).small().color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} selected", self.selected.len()))
                                .small()
                                .color(ACCENT),
                        );
                    });
                });
            });
        if self.session.is_some() {
            egui::SidePanel::left("sidebar")
                .resizable(true)
                .default_width(224.0)
                .width_range(190.0..=340.0)
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(16, 8)),
                )
                .show(context, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sidebar-scroll")
                        .show(ui, |ui| self.sidebar(ui));
                });
        }
        if self.session.is_some() {
            egui::SidePanel::right("preview")
                .resizable(true)
                .default_width(360.0)
                .width_range(280.0..=720.0)
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(16, 8)),
                )
                .show(context, |ui| {
                    if self.tab == Tab::ClassStructures {
                        self.class_structure_preview(ui);
                    } else {
                        self.preview_panel(ui);
                    }
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(12))
            .show(context, |ui| {
                if self.session.is_none() {
                    ui.add_enabled_ui(self.task.is_none(), |ui| self.welcome(ui));
                    if let Some(error) = &self.last_error {
                        ui.add_space(20.0);
                        ui.colored_label(Color32::LIGHT_RED, error);
                    }
                    return;
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Assets, "Assets");
                    if ui
                        .selectable_value(&mut self.tab, Tab::Hierarchy, "Scene hierarchy")
                        .clicked()
                        && !self.scene_loaded
                    {
                        self.pending_scene = true;
                    }
                    if ui
                        .selectable_value(&mut self.tab, Tab::ClassStructures, "Class structures")
                        .clicked()
                        && !self.classes_loaded
                    {
                        self.pending_classes = true;
                    }
                    let errors = self.log.iter().filter(|entry| entry.error).count();
                    ui.selectable_value(
                        &mut self.tab,
                        Tab::Log,
                        format!(
                            "Activity log{}",
                            if errors > 0 {
                                format!(" ({errors})")
                            } else {
                                String::new()
                            }
                        ),
                    );
                });
                ui.add_space(8.0);
                if let Some(error) = self.last_error.clone() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(60, 31, 38))
                        .inner_margin(10)
                        .corner_radius(5)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.colored_label(Color32::LIGHT_RED, &error);
                                });
                                if ui.small_button("Dismiss").clicked() {
                                    self.last_error = None;
                                }
                            });
                        });
                    ui.add_space(8.0);
                }
                let loading = self
                    .task
                    .as_ref()
                    .is_some_and(|task| task.kind == TaskKind::Load);
                ui.add_enabled_ui(!loading, |ui| match self.tab {
                    Tab::Assets => self.asset_table(ui),
                    Tab::Hierarchy => self.hierarchy(ui),
                    Tab::ClassStructures => self.class_structures_table(ui),
                    Tab::Log => self.activity_log(ui),
                });
            });
        if self.settings_open {
            self.settings_window(context);
        }
        if context.input(|input| !input.raw.hovered_files.is_empty()) {
            let rect = context.content_rect();
            let painter = context.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop-overlay"),
            ));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(190));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to open assets",
                FontId::proportional(30.0),
                ACCENT,
            );
        }
        if self.filter_dirty {
            self.filter_dirty = false;
            self.queue_filter();
        }
        self.start_pending(context);
        if self.task.is_some() || self.audio.is_playing() {
            context.request_repaint_after(Duration::from_millis(80));
        }
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.show(context);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let result = (|| {
            let path = crate::preferences::settings_path()?;
            self.save_preferences_to(&path)
        })();
        if let Err(error) = result {
            eprintln!("AssetStudio could not save settings: {error}");
        }
    }
}

fn preference_string(
    value: &serde_json::Value,
    name: &str,
    default: &str,
) -> Result<String, String> {
    match value.get(name) {
        None => Ok(default.to_owned()),
        Some(value) => value
            .as_str()
            .filter(|text| text.len() <= 4096 && !text.contains('\0'))
            .map(str::to_owned)
            .ok_or_else(|| format!("Saved setting {name:?} is invalid.")),
    }
}

fn preference_bool(value: &serde_json::Value, name: &str, default: bool) -> Result<bool, String> {
    match value.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("Saved setting {name:?} is invalid.")),
    }
}

fn preference_number(value: &serde_json::Value, name: &str, default: f64) -> Result<f64, String> {
    match value.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("Saved setting {name:?} is invalid.")),
    }
}

fn uv_role_label(role: FbxUvType) -> &'static str {
    match role {
        FbxUvType::Diffuse => "Diffuse",
        FbxUvType::NormalMap => "Normal map",
        FbxUvType::Specular => "Specular",
        FbxUvType::Bump => "Bump",
        FbxUvType::Ambient => "Ambient",
        FbxUvType::Emissive => "Emissive",
        FbxUvType::Reflection => "Reflection",
        FbxUvType::Displacement => "Displacement",
    }
}

fn key_resource_path(specification: &str) -> Result<PathBuf, String> {
    if !specification.trim().is_empty() {
        return Ok(PathBuf::from(specification.trim()));
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    assetstudio_core::unity_cn_keys::UnityCnKeys::beside_executable(&executable)
        .map_err(|error| error.to_string())
}

/// Called only inside the load worker. An explicit key must not cause any
/// unrelated resource file to be opened, and key bytes never become messages.
fn resolve_resource_key(
    options: &mut LoadOptions,
    specification: &str,
    index: usize,
) -> Result<Option<PathBuf>, String> {
    if !options.game.eq_ignore_ascii_case("UnityCN") || options.unity_cn_key.is_some() {
        return Ok(None);
    }
    let path = key_resource_path(specification)?;
    let keys = assetstudio_core::unity_cn_keys::UnityCnKeys::from_path(&path)
        .map_err(|error| error.to_string())?;
    options.unity_cn_key = Some(
        keys.key(index)
            .map_err(|error| error.to_string())?
            .to_owned(),
    );
    Ok(Some(path))
}

fn cab_base_for_inputs(inputs: &[PathBuf], explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(base) = explicit {
        return Ok(base);
    }
    let mut directories = inputs.iter().map(|input| {
        let path = input.canonicalize().map_err(|error| {
            format!("Could not inspect CAB source {}: {error}", input.display())
        })?;
        if path.is_dir() {
            Ok(path)
        } else {
            path.parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| "The selected source has no parent folder.".to_owned())
        }
    });
    let mut base = directories
        .next()
        .ok_or("Select at least one input before loading a CAB map.")??;
    for directory in directories {
        let directory = directory?;
        while !directory.starts_with(&base) {
            if !base.pop() || base.as_os_str().is_empty() {
                return Err(
                    "Choose a CAB base folder that contains every selected input.".to_owned(),
                );
            }
        }
    }
    Ok(base)
}

fn checked_color_image(width: u32, height: u32, rgba: &[u8]) -> Result<egui::ColorImage, String> {
    let width = usize::try_from(width).map_err(|_| "Image width exceeds this platform's limit.")?;
    let height =
        usize::try_from(height).map_err(|_| "Image height exceeds this platform's limit.")?;
    let length = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Image dimensions overflow.")?;
    if width == 0 || height == 0 || length != rgba.len() || length > 64 * 1024 * 1024 {
        return Err("The image's dimensions or pixel data are invalid for a preview.".to_owned());
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        rgba,
    ))
}

pub(crate) fn hierarchy_order(nodes: &[SceneNode]) -> Vec<(usize, usize)> {
    let by_key: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.key, index))
        .collect();
    let mut children = vec![Vec::new(); nodes.len()];
    let mut roots = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        if let Some(&parent) = node.parent.as_ref().and_then(|key| by_key.get(key)) {
            children[parent].push(index);
        } else {
            roots.push(index);
        }
    }
    let compare = |&left: &usize, &right: &usize| {
        nodes[left]
            .name
            .cmp(&nodes[right].name)
            .then_with(|| nodes[left].key.cmp(&nodes[right].key))
    };
    roots.sort_unstable_by(compare);
    for siblings in &mut children {
        siblings.sort_unstable_by(compare);
    }
    let mut stack: Vec<_> = roots
        .into_iter()
        .rev()
        .map(|index| (index, 0_usize))
        .collect();
    let mut visited = vec![false; nodes.len()];
    let mut output = Vec::with_capacity(nodes.len());
    while let Some((index, depth)) = stack.pop() {
        if std::mem::replace(&mut visited[index], true) {
            continue;
        }
        output.push((index, depth));
        stack.extend(
            children[index]
                .iter()
                .rev()
                .map(|&child| (child, depth.saturating_add(1))),
        );
    }
    // A defensive fallback keeps broken/orphaned cycles visible without recursing.
    for (index, seen) in visited.into_iter().enumerate() {
        if !seen {
            output.push((index, 0));
        }
    }
    output
}

fn checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    // The fit view is small; at high zoom, bound the checkerboard work to the
    // visible clip instead of painting tiles for the entire source image.
    let visible = rect.intersect(painter.clip_rect());
    let tile = 14.0;
    let start_x = ((visible.left() - rect.left()) / tile).floor() as i32;
    let start_y = ((visible.top() - rect.top()) / tile).floor() as i32;
    let columns = (visible.width() / tile).ceil() as i32 + 2;
    let rows = (visible.height() / tile).ceil() as i32 + 2;
    for y in start_y..start_y + rows {
        for x in start_x..start_x + columns {
            let tile_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(x as f32 * tile, y as f32 * tile),
                egui::vec2(tile, tile),
            )
            .intersect(rect);
            painter.rect_filled(
                tile_rect,
                0.0,
                if (x + y) % 2 == 0 {
                    Color32::from_gray(43)
                } else {
                    Color32::from_gray(58)
                },
            );
        }
    }
}

fn mode_label(mode: ExportMode) -> &'static str {
    match mode {
        ExportMode::Convert => "Converted assets",
        ExportMode::Raw => "Raw binary",
        ExportMode::Dump => "Type tree text",
        ExportMode::Json => "Type tree JSON",
    }
}

fn group_label(group: GroupMode) -> &'static str {
    match group {
        GroupMode::ByType => "Asset type",
        GroupMode::ByContainer => "Container",
        GroupMode::BySource => "Source file",
        GroupMode::None => "One folder",
    }
}

fn image_label(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tga => "TGA",
    }
}

fn configure_style(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(Color32::from_gray(225));
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.faint_bg_color = Color32::from_gray(17);
    style.visuals.window_stroke = egui::Stroke::new(1.0_f32, Color32::from_gray(38));
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.selection.bg_fill = Color32::from_gray(40);
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    style.visuals.widgets.active.bg_fill = Color32::from_gray(52);
    style.visuals.widgets.hovered.bg_fill = Color32::from_gray(34);
    style.visuals.widgets.inactive.bg_fill = Color32::from_gray(23);
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(11.0, 7.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(11.0));
    context.set_style(style);
}

pub fn smoke_test() -> Result<(), String> {
    let context = egui::Context::default();
    let mut app = StudioApp::new(&context, None);
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        },
        |context| app.show(context),
    );
    if output.shapes.is_empty() {
        return Err("The desktop interface did not produce any visible content.".to_owned());
    }
    println!("AssetStudio desktop interface smoke test passed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicking_text_in_every_asset_column_selects_the_whole_row() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 123,
        };
        app.assets = Arc::new(vec![AssetRecord {
            key,
            class_id: 43,
            class_name: "Mesh".into(),
            name: "Clickable mesh".into(),
            container: "assets/unique/model.prefab".into(),
            source: "CAB-click".into(),
            size: 4096,
        }]);
        app.visible = vec![0];
        let render = |app: &mut StudioApp, events| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| app.asset_table(ui));
                },
            )
        };
        let mut output = render(&mut app, vec![]);
        for label in [
            "Clickable mesh",
            "Mesh",
            "4.0 KiB",
            "assets/unique/model.prefab",
        ] {
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + egui::vec2(4.0, text.galley.size().y * 0.5))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing cell {label}"));
            app.selected.clear();
            app.primary = None;
            let _ = render(
                &mut app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    },
                ],
            );
            assert_eq!(
                app.primary,
                Some(key),
                "clicking the middle of {label} must select its row"
            );
            assert!(app.selected.contains(&key));
            output = render(&mut app, vec![]);
            assert_ne!(output.platform_output.cursor_icon, egui::CursorIcon::Text);
        }
    }

    #[test]
    fn type_widgets_toggle_multiple_checks_and_cycle_sort_without_losing_them() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.classes = BTreeMap::from([("Mesh".into(), 4), ("Texture2D".into(), 7)]);
        let render = |app: &mut StudioApp, events| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| app.sidebar(ui));
                },
            )
        };
        let mut output = render(&mut app, vec![]);
        let mut click = |app: &mut StudioApp, label: &str| {
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + egui::vec2(3.0, 5.0))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing type control {label}"));
            output = render(
                app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    },
                ],
            );
            // Refresh the changed label/order before choosing the next control.
            output = render(app, vec![]);
        };
        click(&mut app, "Mesh  4");
        click(&mut app, "Texture2D  7");
        assert_eq!(
            app.filters.classes,
            std::collections::BTreeSet::from(["Mesh".into(), "Texture2D".into()])
        );
        click(&mut app, "A");
        assert_eq!(app.type_sort.label(), "↓");
        click(&mut app, "↓");
        assert_eq!(app.type_sort.label(), "↑");
        click(&mut app, "Mesh  4");
        assert_eq!(
            app.filters.classes,
            std::collections::BTreeSet::from(["Texture2D".into()])
        );
        click(&mut app, "All types (0)");
        assert!(app.filters.classes.is_empty());
    }

    #[test]
    fn worker_progress_is_painted_in_the_footer_and_clears_after_failure() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let (ready, started) = mpsc::sync_channel(1);
        let (finish, wait) = mpsc::sync_channel(1);
        app.start_task_with_progress(
            TaskKind::Load,
            "Opening assets",
            &context,
            move |progress| {
                progress.update("Reading files", 4, Some(8));
                ready.send(()).unwrap();
                wait.recv().unwrap();
                Err("Fixture load failure".to_owned())
            },
        );
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                ..Default::default()
            },
            |context| app.show(context),
        );
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.fill == ACCENT && rect.rect.height() == 4.0
                && rect.rect.width() > 600.0 && rect.rect.width() < 800.0 && rect.rect.top() > 800.0
        )), "the worker's real 4/8 progress must paint a half-width white footer bar");
        finish.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while app.task.is_some() && std::time::Instant::now() < deadline {
            app.poll_task(&context);
            std::thread::yield_now();
        }
        assert!(app.task.is_none());
        assert_eq!(app.last_error.as_deref(), Some("Fixture load failure"));
    }

    #[test]
    fn preferences_round_trip_without_saving_explicit_decryption_key() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.load_settings.game = "UnityCN".to_owned();
        app.load_settings.unity_cn_key = "11".repeat(16);
        app.load_settings.unity_cn_keys_file = "public-keys.json".to_owned();
        app.load_settings.unity_cn_key_index = 3;
        app.export_options.mode = ExportMode::Raw;
        app.export_options.group = GroupMode::ByContainer;
        app.export_options.image_format = ImageFormat::Tga;
        app.model_options.include_animations = false;
        app.model_options.include_textures = false;
        app.model_options.include_skins = false;
        app.model_options.include_materials = false;
        app.model_options.include_blend_shapes = false;
        app.model_options.export_all_nodes = false;
        app.model_options.scale_factor = 2.5;
        app.model_options.euler_filter = false;
        app.model_options.uv_enabled = [true, false, true, false, false, true, false, true];
        app.model_options.fbx.version = 7500;
        app.model_options.fbx.cast_to_bone = true;
        app.model_options.fbx.bone_size = 25.5;
        app.model_options.fbx.uv_types = [
            FbxUvType::Diffuse,
            FbxUvType::NormalMap,
            FbxUvType::Specular,
            FbxUvType::Bump,
            FbxUvType::Ambient,
            FbxUvType::Emissive,
            FbxUvType::Reflection,
            FbxUvType::Displacement,
        ];
        app.image_fit = false;
        app.image_zoom = 2.0;
        app.mesh_mode = MeshMode::Combined;
        app.mesh_shading = MeshShading::SourceNormals;
        app.preview_mode = PreviewMode::Dump;
        let value = app.preferences_value();
        assert!(!value.to_string().contains(&app.load_settings.unity_cn_key));
        assert!(value["load"].get("unity_cn_key").is_none());
        let mut restored = StudioApp::new(&context, None);
        restored.apply_preferences(&value).unwrap();
        assert_eq!(restored.preferences_value(), value);
        assert!(restored.load_settings.unity_cn_key.is_empty());
        restored.request_preview(Some(AssetKey {
            file_index: 0,
            path_id: 1,
        }));
        assert_eq!(restored.image_zoom, 2.0);
        assert!(!restored.image_fit);
    }

    #[test]
    fn unfinished_settings_edits_preserve_the_last_loadable_preferences() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        app.model_options.scale_factor = 2.5;
        app.save_preferences_to(&path).unwrap();
        let saved = std::fs::read(&path).unwrap();
        app.model_options.scale_factor = 0.0;
        assert!(app.save_preferences_to(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        app.model_options.scale_factor = 2.5;
        app.load_settings.xor_key = "unfinished".to_owned();
        assert!(app.save_preferences_to(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        let document: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        let mut restored = StudioApp::new(&context, None);
        restored.apply_preferences(&document["data"]).unwrap();
        assert_eq!(restored.model_options.scale_factor, 2.5);
    }

    #[test]
    fn invalid_preferences_are_rejected_before_any_settings_are_changed() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let previous = app.preferences_value();
        for value in [
            serde_json::json!({"load":{"game":"Normal"},"export":{"mode":"Bogus"}}),
            serde_json::json!({"load":{"unity_cn_key_index":1024}}),
            serde_json::json!({"preview":{"image_zoom":0}}),
            serde_json::json!({"preview":{"mesh_shading":"Invented"}}),
            serde_json::json!({"preview":{"object_view":"Invented"}}),
            serde_json::json!({"export":{"scale_factor":0}}),
            serde_json::json!({"export":{"scale_factor":1e100}}),
            serde_json::json!({"export":{"bone_size":0}}),
            serde_json::json!({"export":{"bone_size":-1}}),
            serde_json::json!({"export":{"fbx_version":7600}}),
            serde_json::json!({"export":{"uv_enabled":[true,false]}}),
            serde_json::json!({"export":{"uv_types":[0,1,2,3,4,5,6,8]}}),
            serde_json::json!({"load":[]}),
        ] {
            assert!(
                app.apply_preferences(&value).is_err(),
                "invalid settings were accepted: {value}"
            );
            assert_eq!(app.preferences_value(), previous);
        }
        app.apply_preferences(
            &serde_json::json!({"load":{"unity_cn_key":"ignored"},"future":true}),
        )
        .unwrap();
        assert!(app.load_settings.unity_cn_key.is_empty());
    }

    #[test]
    fn image_channel_results_cannot_cross_channel_or_selection_changes() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 28,
        };
        app.primary = Some(key);
        app.preview = Some(DisplayPreview::Image(ImagePreview {
            texture: context.load_texture(
                "test-image",
                checked_color_image(1, 1, &[1, 2, 3, 255]).unwrap(),
                egui::TextureOptions::LINEAR,
            ),
            original: Arc::new(vec![1, 2, 3, 255]),
            width: 1,
            height: 1,
            channels: Channels::default(),
            revision: 2,
        }));
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::ImageChannels {
                generation: app.generation,
                key,
                revision: 1,
                result: Err("obsolete image mask".to_owned()),
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::ImageChannels,
            label: "Test mask",
            generation: app.generation,
            revision: 0,
        });
        app.poll_task(&context);
        assert!(app.last_error.is_none());
        assert!(
            matches!(&app.preview,Some(DisplayPreview::Image(image)) if image.revision==2 && image.original.as_slice()==[1,2,3,255])
        );
        app.request_preview(None);
        assert!(app.pending_channels.is_none());
        assert!(app.preview.is_none());
    }

    #[test]
    fn stopping_audio_invalidates_a_pending_decode_and_its_errors() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 83,
        };
        app.primary = Some(key);
        let obsolete = app.audio_revision;
        app.stop_audio();
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Audio {
                generation: app.generation,
                revision: obsolete,
                key,
                // Invalid WAV would report an error if stale playback were attempted.
                bytes: Vec::new(),
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Audio,
            label: "Test audio",
            generation: app.generation,
            revision: obsolete,
        });
        app.poll_task(&context);
        assert!(app.last_error.is_none());
        assert!(!app.audio.is_playing());

        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Err("obsolete decoder failure".to_owned()))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Audio,
            label: "Test audio",
            generation: app.generation,
            revision: obsolete,
        });
        app.poll_task(&context);
        assert!(app.last_error.is_none());
        assert!(app.log.is_empty());
    }

    #[test]
    fn returning_to_same_selection_cannot_revive_older_audio() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 83,
        };
        app.request_preview(Some(key));
        let revision = app.audio_revision;
        app.request_preview(None);
        app.request_preview(Some(key));
        assert_ne!(app.audio_revision, revision);
    }

    #[test]
    fn explicit_unity_cn_key_takes_precedence_without_opening_resource() {
        let explicit = "12".repeat(16);
        let mut options = LoadOptions {
            game: "UnityCN".to_owned(),
            unity_cn_key: Some(explicit.clone()),
            ..Default::default()
        };
        assert!(
            resolve_resource_key(
                &mut options,
                "this-key-file-does-not-exist.json",
                usize::MAX
            )
            .unwrap()
            .is_none()
        );
        assert!(options.unity_cn_key.as_deref() == Some(explicit.as_str()));
    }

    #[test]
    fn indexed_unity_cn_key_reads_selected_entry_and_never_falls_back() {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(br#"[{"Name":"First","Key":"11111111111111111111111111111111"},{"Name":"Second","Key":"22222222222222222222222222222222"}]"#).unwrap();
        file.flush().unwrap();
        let specification = file.path().to_str().unwrap();
        let mut options = LoadOptions {
            game: "UnityCN".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            resolve_resource_key(&mut options, specification, 1)
                .unwrap()
                .as_deref(),
            Some(file.path())
        );
        assert!(options.unity_cn_key.as_deref() == Some("22222222222222222222222222222222"));
        options.unity_cn_key = None;
        let error = resolve_resource_key(&mut options, specification, 2).unwrap_err();
        assert!(options.unity_cn_key.is_none());
        assert!(!error.contains("22222222222222222222222222222222"));
        assert!(!error.contains("11111111111111111111111111111111"));
    }

    #[test]
    fn stale_unity_cn_name_results_cannot_replace_new_resource_selection() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.load_settings.unity_cn_keys_file = "new.json".to_owned();
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::KeyNames {
                source: "old.json".to_owned(),
                result: Err("obsolete missing file".to_owned()),
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::KeyList,
            label: "Test",
            generation: 0,
            revision: 0,
        });
        app.poll_task(&context);
        assert!(app.last_error.is_none());
        assert!(app.unity_cn_names.is_empty());
    }

    #[test]
    fn indexed_unity_cn_settings_validate_without_reading_a_file_on_ui_thread() {
        let settings = LoadSettings {
            game: "UnityCN".to_owned(),
            unity_cn_keys_file: "missing-resource.json".to_owned(),
            ..Default::default()
        };
        let options = settings.options().unwrap();
        assert_eq!(options.game, "UnityCN");
        assert!(options.unity_cn_key.is_none());
        // Actual key file validation and selection happen in the worker.
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.settings_open = true;
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 640.0),
                )),
                ..Default::default()
            },
            |context| app.show(context),
        );
        assert!(!output.shapes.is_empty());
        assert!(app.task.is_none());
    }

    fn finish_filter(
        app: &mut StudioApp,
        context: &egui::Context,
        generation: u64,
        revision: u64,
        result: Result<Vec<usize>, String>,
    ) {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Filtered {
                generation,
                revision,
                result,
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Filter,
            label: "Test filter",
            generation,
            revision,
        });
        app.poll_task(context);
    }

    #[test]
    fn filter_requests_coalesce_and_old_results_cannot_replace_latest_search() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.generation = 10;
        app.visible = vec![3, 1];
        app.filters.name = "old".to_owned();
        app.queue_filter();
        let obsolete = app.filter_revision;
        app.filters.name = "new".to_owned();
        app.queue_filter();
        let latest = app.filter_revision;
        assert!(app.pending_filter);
        assert_eq!(app.filters.name, "new");
        assert_eq!(
            app.visible,
            vec![3, 1],
            "keep existing rows visible while a filter job runs"
        );
        finish_filter(
            &mut app,
            &context,
            10,
            obsolete,
            Err("old invalid regex".to_owned()),
        );
        assert!(app.pending_filter);
        assert!(app.filter_error.is_none());
        finish_filter(&mut app, &context, 10, latest, Ok(vec![]));
        assert!(!app.pending_filter);
        assert!(app.filter_error.is_none());
    }

    #[test]
    fn filter_errors_are_current_and_cannot_cross_a_source_change() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.generation = 4;
        app.queue_filter();
        let revision = app.filter_revision;
        finish_filter(&mut app, &context, 3, revision, Err("stale".to_owned()));
        assert!(app.filter_error.is_none());
        assert!(app.pending_filter);
        finish_filter(
            &mut app,
            &context,
            4,
            revision,
            Err("current invalid regex".to_owned()),
        );
        assert_eq!(app.filter_error.as_deref(), Some("current invalid regex"));
        assert!(!app.pending_filter);
    }

    #[test]
    fn successful_operation_clears_an_old_error_banner_but_keeps_history() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.error("previous export failed");
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Report {
                operation: "Export",
                report: OperationReport::default(),
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Export,
            label: "Test",
            generation: 0,
            revision: 0,
        });
        app.poll_task(&context);
        assert!(app.last_error.is_none());
        assert!(
            app.log
                .iter()
                .any(|entry| entry.text == "previous export failed")
        );
    }

    #[test]
    fn cab_default_base_uses_source_folder_or_common_parent() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let source = root.join("src");
        assert_eq!(
            cab_base_for_inputs(&[source.join("main.rs")], None).unwrap(),
            source
        );
        assert_eq!(
            cab_base_for_inputs(std::slice::from_ref(&root), None).unwrap(),
            root
        );
        assert_eq!(
            cab_base_for_inputs(&[root.join("src/main.rs"), root.join("Cargo.toml")], None)
                .unwrap(),
            root
        );
        let explicit = PathBuf::from("explicit-relocation");
        assert_eq!(
            cab_base_for_inputs(&[], Some(explicit.clone())).unwrap(),
            explicit
        );
        assert!(cab_base_for_inputs(&[], None).is_err());
    }

    #[test]
    fn loaded_asset_preview_hierarchy_and_log_widgets_render_headlessly() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 7,
        };
        let records = vec![AssetRecord {
            key,
            class_id: 49,
            class_name: "TextAsset".to_owned(),
            name: "example".to_owned(),
            container: "examples/readme.txt".to_owned(),
            source: "CAB-example".to_owned(),
            size: 16,
        }];
        let catalog = Catalog::prepare(&records).unwrap();
        app.assets = catalog.assets;
        app.by_key = catalog.by_key;
        app.classes = catalog.classes;
        app.visible = vec![0];
        app.primary = Some(key);
        app.preview = Some(DisplayPreview::Text("Example asset contents".to_owned()));
        app.scene_loaded = true;
        app.scene = vec![SceneNode {
            key,
            name: "Root object".to_owned(),
            parent: None,
        }];
        app.scene_rows = vec![(0, 0)];
        app.log(false, "A completed operation");
        for view in 0..4 {
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| match view {
                        0 => app.asset_table(ui),
                        1 => app.preview_panel(ui),
                        2 => app.hierarchy(ui),
                        _ => app.activity_log(ui),
                    });
                },
            );
            assert!(!output.shapes.is_empty());
        }
    }

    #[test]
    fn mesh_worker_channels_reach_the_real_colored_widget_without_a_gpu() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 43,
        };
        let catalog = Catalog::prepare(&[AssetRecord {
            key,
            class_id: 43,
            class_name: "Mesh".into(),
            name: "Color triangle".into(),
            container: String::new(),
            source: "CAB-preview".into(),
            size: 128,
        }])
        .unwrap();
        app.assets = catalog.assets;
        app.by_key = catalog.by_key;
        app.classes = catalog.classes;
        app.primary = Some(key);
        app.mesh_shading = MeshShading::VertexColors;
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Preview {
                generation: app.generation,
                revision: app.preview_revision,
                key,
                preview: prepare_preview(Preview::Mesh {
                    vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    indices: vec![0, 1, 2],
                    normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
                    colors: Some(vec![
                        [1.0, 0.0, 0.0, 1.0],
                        [0.0, 1.0, 0.0, 1.0],
                        [0.0, 0.0, 1.0, 1.0],
                    ]),
                })
                .unwrap(),
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Preview,
            label: "Mesh preview",
            generation: app.generation,
            revision: app.preview_revision,
        });
        app.poll_task(&context);
        let Some(DisplayPreview::Mesh(mesh)) = &app.preview else {
            panic!("Mesh result was not retained")
        };
        assert_eq!(mesh.shading, MeshShading::VertexColors);
        assert!(mesh.supports_shading(MeshShading::SourceNormals));
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 900.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.preview_panel(ui));
            },
        );
        assert!(
            output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) =>
                    mesh.vertices.len() == 3
                        && mesh.vertices.iter().map(|v| v.color).collect::<Vec<_>>()
                            == [Color32::RED, Color32::GREEN, Color32::BLUE],
                _ => false,
            }),
            "Source vertex colors were lost before painting"
        );
        assert!(!context.tessellate(output.shapes, 1.0).is_empty());
    }

    #[test]
    fn preview_view_switch_discards_old_results_and_errors_for_the_same_asset() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        let key = AssetKey {
            file_index: 0,
            path_id: 7,
        };
        app.request_preview(Some(key));
        let old_revision = app.preview_revision;
        app.preview_mode = PreviewMode::Dump;
        app.request_preview(Some(key));
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Preview {
                generation: app.generation,
                revision: old_revision,
                key,
                preview: PreparedPreview::Text("obsolete decoded preview".into()),
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Preview,
            label: "Old preview",
            generation: app.generation,
            revision: old_revision,
        });
        app.poll_task(&context);
        assert!(app.preview.is_none());
        assert_eq!(app.pending_preview, Some(key));

        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Preview {
                generation: app.generation,
                revision: app.preview_revision,
                key,
                preview: PreparedPreview::Text("current schema dump".into()),
            }))
            .unwrap();
        app.pending_preview = None;
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Preview,
            label: "Current dump",
            generation: app.generation,
            revision: app.preview_revision,
        });
        app.poll_task(&context);
        assert!(
            matches!(&app.preview,Some(DisplayPreview::Text(text)) if text=="current schema dump")
        );

        let current_revision = app.preview_revision;
        app.preview_mode = PreviewMode::Json;
        app.request_preview(Some(key));
        app.pending_preview = None; // A later command can supersede queued preview.
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err("obsolete dump failure".into())).unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Preview,
            label: "Old dump",
            generation: app.generation,
            revision: current_revision,
        });
        app.poll_task(&context);
        assert!(app.preview.is_none());
        assert!(
            app.log
                .iter()
                .any(|entry| entry.text == "obsolete dump failure")
        );
    }

    #[test]
    fn class_structure_workers_keep_source_identity_and_render_both_views_headlessly() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.generation = 3;
        app.pending_classes = true;
        let definition = ClassStructure {
            unity_version: "2022.3.62f1".into(),
            class_id: -3,
            serialized_class_id: 114,
            name: "ScriptData Base".into(),
            source: "CAB-classes".into(),
            text: "ScriptData Base -1 False\r\n\tint m_Value 4 False\r\n".into(),
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::ClassStructures {
                generation: 2,
                definitions: vec![definition.clone()],
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::ClassStructures,
            label: "Old definitions",
            generation: 2,
            revision: 0,
        });
        app.poll_task(&context);
        assert!(app.class_structures.is_empty());
        assert!(app.pending_classes);
        assert!(!app.classes_loaded);
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::ClassStructures {
                generation: 3,
                definitions: vec![definition],
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::ClassStructures,
            label: "Current definitions",
            generation: 3,
            revision: 0,
        });
        app.poll_task(&context);
        assert_eq!(app.selected_class, Some(0));
        assert!(!app.pending_classes);
        assert!(app.classes_loaded);
        assert_eq!(app.class_structures[0].class_id, -3);
        for preview in [false, true] {
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 800.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        if preview {
                            app.class_structure_preview(ui);
                        } else {
                            app.class_structures_table(ui);
                        }
                    });
                },
            );
            assert!(!output.shapes.is_empty());
        }
    }

    #[test]
    fn completed_hierarchy_clears_a_repeated_pending_request() {
        let context = egui::Context::default();
        let mut app = StudioApp::new(&context, None);
        app.generation = 3;
        app.pending_scene = true;
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(TaskPayload::Scene {
                generation: 3,
                nodes: vec![],
                rows: vec![],
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Scene,
            label: "Test",
            generation: app.generation,
            revision: app.filter_revision,
        });
        app.poll_task(&context);
        assert!(app.scene_loaded);
        assert!(!app.pending_scene);
        assert!(app.task.is_none());

        app.pending_scene = true;
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err("invalid scene".to_owned())).unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Scene,
            label: "Test",
            generation: app.generation,
            revision: app.filter_revision,
        });
        app.poll_task(&context);
        assert!(app.scene_loaded);
        assert!(!app.pending_scene);
        assert_eq!(app.scene_error.as_deref(), Some("invalid scene"));
    }

    #[test]
    fn image_validation_prevents_egui_panics_on_bad_payloads() {
        assert!(checked_color_image(0, 1, &[]).is_err());
        assert!(checked_color_image(2, 2, &[0; 15]).is_err());
        assert!(checked_color_image(u32::MAX, u32::MAX, &[]).is_err());
        assert!(checked_color_image(1, 1, &[255; 4]).is_ok());
    }

    #[test]
    fn load_options_accept_decimal_hex_and_reject_out_of_range_keys() {
        let mut settings = LoadSettings {
            xor_key: "0xFF".to_owned(),
            ..Default::default()
        };
        assert_eq!(settings.options().unwrap().xor_key, 255);
        settings.xor_key = "17".to_owned();
        assert_eq!(settings.options().unwrap().xor_key, 17);
        settings.xor_key = "256".to_owned();
        assert!(settings.options().is_err());
        settings.xor_key = "-1".to_owned();
        assert!(settings.options().is_err());
    }

    #[test]
    fn hierarchy_is_iterative_deterministic_and_displays_orphans() {
        let key = |path_id| AssetKey {
            file_index: 0,
            path_id,
        };
        let nodes = vec![
            SceneNode {
                key: key(2),
                name: "child".to_owned(),
                parent: Some(key(1)),
            },
            SceneNode {
                key: key(1),
                name: "parent".to_owned(),
                parent: None,
            },
            SceneNode {
                key: key(3),
                name: "orphan".to_owned(),
                parent: Some(key(99)),
            },
        ];
        assert_eq!(hierarchy_order(&nodes), vec![(2, 0), (1, 0), (0, 1)]);
    }

    #[test]
    fn empty_desktop_can_render_without_a_window_or_audio_device() {
        smoke_test().unwrap();
    }
}
