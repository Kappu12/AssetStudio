use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use assetstudio_core::{
    AssetKey, AssetRecord, ExportMode, ExportOptions, GroupMode, ImageFormat, LoadOptions,
    MapFormat, ModelFormat, OperationReport, Preview, SceneNode, Session,
};
use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense, TextEdit, Vec2};

use crate::audio::AudioPlayer;
use crate::view::{self, Filters, MeshPreview, SortColumn};

const ACCENT: Color32 = Color32::from_rgb(111, 224, 191);
const MUTED: Color32 = Color32::from_rgb(144, 159, 180);
const BACKGROUND: Color32 = Color32::from_rgb(15, 20, 29);
const PANEL: Color32 = Color32::from_rgb(22, 29, 40);
const MAX_LOG_ENTRIES: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Assets,
    Hierarchy,
    Log,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Load,
    Preview,
    Scene,
    Export,
    Audio,
}

enum TaskPayload {
    Loaded {
        paths: Vec<PathBuf>,
        session: Arc<Session>,
        options: LoadOptions,
    },
    Preview {
        generation: u64,
        key: AssetKey,
        preview: Preview,
    },
    Scene {
        generation: u64,
        nodes: Vec<SceneNode>,
    },
    Audio {
        generation: u64,
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
}

enum DisplayPreview {
    Text(String),
    Image(egui::TextureHandle),
    Mesh(MeshPreview),
    Error(String),
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
    xor_key: String,
    asset_index: String,
    schemas: String,
    continue_on_error: bool,
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
        };
        options.validate().map_err(|error| error.to_string())?;
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
    source_options: LoadOptions,
    generation: u64,
    assets: Vec<AssetRecord>,
    classes: BTreeMap<String, usize>,
    visible: Vec<usize>,
    filters: Filters,
    filter_error: Option<String>,
    sort: SortColumn,
    descending: bool,
    filter_dirty: bool,
    selected: HashSet<AssetKey>,
    primary: Option<AssetKey>,
    anchor: Option<AssetKey>,
    preview: Option<DisplayPreview>,
    pending_preview: Option<AssetKey>,
    image_zoom: f32,
    image_fit: bool,
    scene: Vec<SceneNode>,
    scene_loaded: bool,
    scene_error: Option<String>,
    scene_rows: Vec<(usize, usize)>,
    pending_scene: bool,
    task: Option<Task>,
    export_options: ExportOptions,
    load_settings: LoadSettings,
    settings_open: bool,
    input_path: String,
    tab: Tab,
    log: VecDeque<LogEntry>,
    last_error: Option<String>,
    status: String,
    audio: AudioPlayer,
}

impl StudioApp {
    pub fn new(context: &egui::Context, initial_path: Option<PathBuf>) -> Self {
        configure_style(context);
        let mut app = Self {
            session: None,
            source: None,
            sources: Vec::new(),
            source_options: LoadOptions::default(),
            generation: 0,
            assets: Vec::new(),
            classes: BTreeMap::new(),
            visible: Vec::new(),
            filters: Filters::default(),
            filter_error: None,
            sort: SortColumn::Name,
            descending: false,
            filter_dirty: false,
            selected: HashSet::new(),
            primary: None,
            anchor: None,
            preview: None,
            pending_preview: None,
            image_zoom: 1.0,
            image_fit: true,
            scene: Vec::new(),
            scene_loaded: false,
            scene_error: None,
            scene_rows: Vec::new(),
            pending_scene: false,
            task: None,
            export_options: ExportOptions::default(),
            load_settings: LoadSettings {
                game: "Normal".to_owned(),
                ..Default::default()
            },
            settings_open: false,
            input_path: String::new(),
            tab: Tab::Assets,
            log: VecDeque::new(),
            last_error: None,
            status: "Open a Unity file, bundle, or game data folder to begin.".to_owned(),
            audio: AudioPlayer::default(),
        };
        if let Some(path) = initial_path {
            app.open_path(path, context);
        }
        app
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
        if self.task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let context = context.clone();
        match std::thread::Builder::new().name("assetstudio-worker".to_owned()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                .unwrap_or_else(|_| Err("The operation stopped unexpectedly. Your source files have not been changed.".to_owned()));
            let _ = sender.send(result);
            context.request_repaint();
        }) {
            Ok(_) => self.task = Some(Task { receiver, kind, label }),
            Err(error) => self.error(format!("Could not start a background operation: {error}")),
        }
    }

    fn poll_task(&mut self, context: &egui::Context) {
        let Some(task) = &self.task else {
            return;
        };
        let kind = task.kind;
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
                session,
                options,
            }) => {
                let Some(path) = paths.first().cloned() else {
                    self.error("The loader returned no input paths.");
                    return;
                };
                self.audio.stop();
                self.generation = self.generation.wrapping_add(1);
                self.assets = session.assets().to_vec();
                self.classes.clear();
                for asset in &self.assets {
                    *self.classes.entry(asset.class_name.clone()).or_default() += 1;
                }
                for diagnostic in session.diagnostics() {
                    self.log(true, diagnostic.clone());
                }
                self.status = format!(
                    "Loaded {} objects from {}.",
                    self.assets.len(),
                    path.display()
                );
                self.log(false, self.status.clone());
                self.input_path = path.to_string_lossy().into_owned();
                self.source = Some(path);
                self.sources = paths;
                self.source_options = options;
                self.session = Some(session);
                self.selected.clear();
                self.primary = None;
                self.anchor = None;
                self.preview = None;
                self.pending_preview = None;
                self.scene.clear();
                self.scene_rows.clear();
                self.scene_loaded = false;
                self.scene_error = None;
                self.pending_scene = self.tab == Tab::Hierarchy;
                self.filter_dirty = true;
                self.last_error = None;
            }
            Ok(TaskPayload::Preview {
                generation,
                key,
                preview,
            }) => {
                if view::preview_is_current(self.generation, generation, self.primary, key) {
                    self.preview = Some(match preview {
                        Preview::Text(text) => DisplayPreview::Text(text),
                        Preview::Mesh { vertices, indices } => {
                            match MeshPreview::new(vertices, indices) {
                                Ok(mesh) => DisplayPreview::Mesh(mesh),
                                Err(error) => DisplayPreview::Error(error),
                            }
                        }
                        Preview::Image {
                            width,
                            height,
                            rgba,
                        } => match checked_color_image(width, height, rgba) {
                            Ok(image) => DisplayPreview::Image(context.load_texture(
                                "asset-preview",
                                image,
                                egui::TextureOptions::LINEAR,
                            )),
                            Err(error) => DisplayPreview::Error(error),
                        },
                    });
                }
            }
            Ok(TaskPayload::Scene { generation, nodes }) => {
                if self.generation == generation {
                    self.pending_scene = false;
                    self.scene_rows = hierarchy_order(&nodes);
                    self.scene = nodes;
                    self.scene_loaded = true;
                    self.scene_error = None;
                }
            }
            Ok(TaskPayload::Audio {
                generation,
                key,
                bytes,
            }) => {
                if view::preview_is_current(self.generation, generation, self.primary, key)
                    && let Err(error) = self.audio.play(bytes)
                {
                    self.error(format!("Could not play this audio clip: {error}"));
                }
            }
            Ok(TaskPayload::Report { operation, report }) => {
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
                TaskKind::Preview => {
                    // Selection can change while a preview is loading. Do not attach
                    // an old object's error to the newly selected object.
                    if self.pending_preview.is_none() {
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
                _ => self.error(error),
            },
        }
    }

    fn start_pending(&mut self, context: &egui::Context) {
        if self.task.is_some() {
            return;
        }
        if self.pending_scene {
            self.pending_scene = false;
            if let Some(session) = self.session.clone() {
                let generation = self.generation;
                self.start_task(
                    TaskKind::Scene,
                    "Building hierarchy…",
                    context,
                    move || {
                        session
                            .scene()
                            .map(|nodes| TaskPayload::Scene { generation, nodes })
                            .map_err(|error| error.to_string())
                    },
                );
                return;
            }
        }
        if let Some(key) = self.pending_preview.take()
            && self.primary == Some(key)
            && let Some(session) = self.session.clone()
        {
            let generation = self.generation;
            self.start_task(
                TaskKind::Preview,
                "Reading preview…",
                context,
                move || {
                    session
                        .preview(key)
                        .map(|preview| TaskPayload::Preview {
                            generation,
                            key,
                            preview,
                        })
                        .map_err(|error| error.to_string())
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
        let options = match self.load_settings.options() {
            Ok(options) => options,
            Err(error) => {
                self.error(error);
                return;
            }
        };
        let asset_index = nonempty(&self.load_settings.asset_index).map(PathBuf::from);
        let schemas = nonempty(&self.load_settings.schemas).map(PathBuf::from);
        self.audio.stop();
        self.pending_preview = None;
        self.start_task(TaskKind::Load, "Opening assets…", context, move || {
            let mut session =
                Session::open_paths(&paths, &options).map_err(|error| error.to_string())?;
            if let Some(index) = asset_index {
                session.recover_containers(&index).map_err(|error| {
                    format!(
                        "Could not recover container names from {}: {error}",
                        index.display()
                    )
                })?;
            }
            if let Some(schema) = schemas {
                session.load_schemas(&schema).map_err(|error| {
                    format!(
                        "Could not load behaviour schemas from {}: {error}",
                        schema.display()
                    )
                })?;
            }
            Ok(TaskPayload::Loaded {
                paths,
                session: Arc::new(session),
                options,
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
        self.audio.stop();
        self.primary = key;
        self.preview = None;
        self.pending_preview = key;
        self.image_zoom = 1.0;
        self.image_fit = true;
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
        self.pending_preview = None;
        self.start_task(
            TaskKind::Export,
            "Exporting assets…",
            context,
            move || {
                session
                    .export(&keys, &output, &options)
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
        self.start_task(
            TaskKind::Export,
            "Exporting scene and textures…",
            context,
            move || {
                session
                    .export_scene(&output, format, &roots)
                    .map(|report| TaskPayload::Report {
                        operation: "Scene export",
                        report,
                    })
                    .map_err(|error| error.to_string())
            },
        );
    }

    fn extract(&mut self, context: &egui::Context) {
        if !self.sources.is_empty() {
            self.extract_inputs(self.sources.clone(), self.source_options.clone(), context);
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
        self.extract_inputs(vec![input], options, context);
    }

    fn extract_inputs(
        &mut self,
        inputs: Vec<PathBuf>,
        options: LoadOptions,
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
        self.pending_preview = None;
        self.start_task(
            TaskKind::Export,
            "Extracting container files…",
            context,
            move || {
                let mut report = OperationReport::default();
                for (index, input) in inputs.iter().enumerate() {
                    let root = if inputs.len() == 1 {
                        output.clone()
                    } else {
                        output.join(format!("input-{:04}", index + 1))
                    };
                    match Session::extract(input, &root, &options, overwrite) {
                        Ok(mut part) => {
                            report.written.append(&mut part.written);
                            report.skipped.append(&mut part.skipped);
                            report.failed.append(&mut part.failed);
                        }
                        Err(error) => report.failed.push(format!("{}: {error}", input.display())),
                    }
                }
                Ok(TaskPayload::Report {
                    operation: "Extraction",
                    report,
                })
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
        let generation = self.generation;
        self.pending_preview = None;
        self.start_task(TaskKind::Audio, "Decoding audio…", context, move || {
            session
                .audio_preview(key)
                .map(|bytes| TaskPayload::Audio {
                    generation,
                    key,
                    bytes,
                })
                .map_err(|error| error.to_string())
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        let idle = self.task.is_none();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("A").size(29.0).strong().color(ACCENT));
            ui.add_space(5.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("AssetStudio").size(22.0).strong());
                ui.label(
                    RichText::new("Explore · inspect · export")
                        .size(11.0)
                        .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
                ui.label(
                    RichText::new(env!("CARGO_PKG_VERSION"))
                        .small()
                        .color(MUTED),
                );
            });
        });
        ui.add_space(10.0);
        ui.separator();
        ui.horizontal(|ui| {
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
                ui.menu_button("Extract…", |ui| {
                    if ui.button("Extract a file…").clicked() {
                        ui.close();
                        self.pick_extraction(false, &context);
                    }
                    if ui.button("Extract a folder…").clicked() {
                        ui.close();
                        self.pick_extraction(true, &context);
                    }
                });
                if ui
                    .add_enabled(self.source.is_some(), egui::Button::new("Reload"))
                    .clicked()
                {
                    self.open_paths(self.sources.clone(), &context);
                }
            });
            ui.separator();
            ui.add_enabled_ui(idle && self.session.is_some(), |ui| {
                if ui
                    .add_enabled(
                        !self.selected.is_empty(),
                        egui::Button::new(format!("Export selected ({})", self.selected.len()))
                            .fill(Color32::from_rgb(35, 91, 78)),
                    )
                    .clicked()
                {
                    self.export_assets(self.selected_keys(), &context);
                }
                if ui
                    .add_enabled(
                        !self.visible.is_empty(),
                        egui::Button::new("Export filtered"),
                    )
                    .clicked()
                {
                    self.export_assets(self.ordered_keys(), &context);
                }
                ui.menu_button("More exports", |ui| {
                    if ui.button("Filtered asset list · JSON").clicked() {
                        ui.close();
                        self.export_map(MapFormat::Json, &context);
                    }
                    if ui.button("Filtered asset list · XML").clicked() {
                        ui.close();
                        self.export_map(MapFormat::Xml, &context);
                    }
                    ui.separator();
                    if ui.button("Entire scene · FBX binary").clicked() {
                        ui.close();
                        self.export_scene(ModelFormat::FbxBinary, false, &context);
                    }
                    if ui.button("Entire scene · FBX text").clicked() {
                        ui.close();
                        self.export_scene(ModelFormat::FbxAscii, false, &context);
                    }
                    if ui.button("Entire scene · OBJ + materials").clicked() {
                        ui.close();
                        self.export_scene(ModelFormat::Obj, false, &context);
                    }
                    if ui
                        .add_enabled(
                            !self.selected.is_empty(),
                            egui::Button::new("Selected GameObjects · FBX"),
                        )
                        .clicked()
                    {
                        ui.close();
                        self.export_scene(ModelFormat::FbxBinary, true, &context);
                    }
                    ui.separator();
                    if ui.button("Extract container files").clicked() {
                        ui.close();
                        self.extract(&context);
                    }
                });
            });
            if let Some(task) = &self.task {
                ui.separator();
                ui.spinner();
                ui.label(RichText::new(task.label).color(ACCENT));
            }
        });
        ui.add_space(5.0);
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("WORKSPACE").small().strong().color(MUTED));
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
                RichText::new(source.display().to_string())
                    .small()
                    .color(MUTED),
            );
            ui.add_space(6.0);
            ui.label(format!("{} input paths", self.sources.len()));
            ui.label(format!("{} objects", self.assets.len()));
            ui.label(format!("{} asset types", self.classes.len()));
        } else {
            ui.label(RichText::new("No files open").color(MUTED));
        }
        ui.add_space(15.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(RichText::new("ASSET TYPES").small().strong().color(MUTED));
        ui.add_space(6.0);
        if ui
            .selectable_label(
                self.filters.class.is_none(),
                format!("All types ({})", self.assets.len()),
            )
            .clicked()
        {
            self.filters.class = None;
            self.filter_dirty = true;
        }
        egui::ScrollArea::vertical()
            .id_salt("type-filter")
            .max_height(260.0)
            .show(ui, |ui| {
                for (class, count) in &self.classes {
                    if ui
                        .selectable_label(
                            self.filters.class.as_ref() == Some(class),
                            format!("{class}  {count}"),
                        )
                        .clicked()
                    {
                        self.filters.class = Some(class.clone());
                        self.filter_dirty = true;
                    }
                }
            });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(
            RichText::new("EXPORT SETTINGS")
                .small()
                .strong()
                .color(MUTED),
        );
        ui.add_space(7.0);
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
                    ui.selectable_value(&mut self.export_options.group, group, group_label(group));
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
        ui.checkbox(&mut self.export_options.overwrite, "Replace existing files");
        if self.export_options.overwrite {
            ui.label(
                RichText::new("Existing exports with matching names may be replaced.")
                    .small()
                    .color(Color32::from_rgb(235, 187, 112)),
            );
        }
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
                        Color32::from_rgb(32, 69, 65)
                    } else if response.hovered() {
                        Color32::from_rgb(35, 43, 57)
                    } else if row % 2 == 0 {
                        Color32::from_rgb(21, 27, 37)
                    } else {
                        BACKGROUND
                    };
                    ui.painter().rect_filled(rect, 2.0, background);
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(egui::vec2(4.0, 2.0)))
                            .layout(Layout::left_to_right(Align::Center)),
                    );
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
            let keys = self.ordered_keys();
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
                RichText::new("Select a GameObject, then use More exports.")
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
        let Some(asset) = self.assets.iter().find(|asset| asset.key == key) else {
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
        if is_audio {
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
                    self.audio.stop();
                }
            });
            if self.audio.is_playing() || self.audio.is_paused() {
                let position = self.audio.position().as_secs_f32();
                if let Some(duration) = self.audio.duration() {
                    ui.add(
                        egui::ProgressBar::new(
                            (position / duration.as_secs_f32().max(0.001)).clamp(0.0, 1.0),
                        )
                        .text(format!(
                            "{position:.1} / {:.1} seconds",
                            duration.as_secs_f32()
                        )),
                    );
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
            Some(DisplayPreview::Image(texture)) => {
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
                    RichText::new("Drag to rotate · scroll to zoom")
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
                if response.dragged() {
                    let delta = response.drag_delta();
                    mesh.yaw += delta.x * 0.01;
                    mesh.pitch = (mesh.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                }
                if response.hovered() {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    mesh.zoom = (mesh.zoom * (scroll * 0.002).exp()).clamp(0.2, 8.0);
                }
                let painter = ui.painter().with_clip_rect(rect);
                for line in mesh.lines(rect) {
                    painter.line_segment(
                        line,
                        egui::Stroke::new(0.8_f32, ACCENT.gamma_multiply(0.65)),
                    );
                }
            }
        }
    }

    fn settings_window(&mut self, context: &egui::Context) {
        let mut open = self.settings_open;
        egui::Window::new("Load settings").open(&mut open).collapsible(false).resizable(false).default_width(430.0).show(context, |ui| {
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
                ui.label("Unity version override"); ui.add(TextEdit::singleline(&mut self.load_settings.unity_version).hint_text("Use the version stored in the file")); ui.end_row();
                ui.label("UnityCN key"); ui.add(TextEdit::singleline(&mut self.load_settings.unity_cn_key).password(true).hint_text("32 hexadecimal characters")); ui.end_row();
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
            });
            ui.add_space(10.0);
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
            ui.label(
                RichText::new("Explore your Unity assets")
                    .size(32.0)
                    .strong(),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new("Open a bundle, an asset file, or a complete game data folder.")
                    .size(16.0)
                    .color(MUTED),
            );
            ui.add_space(25.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 272.0).max(0.0) * 0.5);
                if ui
                    .add_sized(
                        [130.0, 40.0],
                        egui::Button::new("Open file").fill(Color32::from_rgb(35, 91, 78)),
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
            match view::visible_rows(&self.assets, &self.filters, self.sort, self.descending) {
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
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(context, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(15, 8)),
            )
            .show(context, |ui| {
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
        if self.session.is_some() {
            egui::SidePanel::right("preview")
                .resizable(true)
                .default_width(415.0)
                .width_range(280.0..=720.0)
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(16, 8)),
                )
                .show(context, |ui| self.preview_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(16))
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
}

fn checked_color_image(width: u32, height: u32, rgba: Vec<u8>) -> Result<egui::ColorImage, String> {
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
        &rgba,
    ))
}

fn hierarchy_order(nodes: &[SceneNode]) -> Vec<(usize, usize)> {
    let mut children = BTreeMap::<Option<AssetKey>, Vec<usize>>::new();
    let keys: HashSet<_> = nodes.iter().map(|node| node.key).collect();
    for (index, node) in nodes.iter().enumerate() {
        let parent = node.parent.filter(|key| keys.contains(key));
        children.entry(parent).or_default().push(index);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|&left, &right| {
            nodes[left]
                .name
                .cmp(&nodes[right].name)
                .then_with(|| nodes[left].key.cmp(&nodes[right].key))
        });
    }
    let mut stack: Vec<_> = children
        .get(&None)
        .into_iter()
        .flatten()
        .rev()
        .map(|&index| (index, 0_usize))
        .collect();
    let mut visited = HashSet::new();
    let mut output = Vec::with_capacity(nodes.len());
    while let Some((index, depth)) = stack.pop() {
        if !visited.insert(nodes[index].key) {
            continue;
        }
        output.push((index, depth));
        if let Some(descendants) = children.get(&Some(nodes[index].key)) {
            stack.extend(
                descendants
                    .iter()
                    .rev()
                    .map(|&child| (child, depth.saturating_add(1))),
            );
        }
    }
    // A defensive fallback keeps broken/orphaned cycles visible without recursing.
    for (index, node) in nodes.iter().enumerate() {
        if visited.insert(node.key) {
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
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.selection.bg_fill = Color32::from_rgb(39, 92, 80);
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(43, 104, 90);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(49, 63, 80);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(33, 43, 57);
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
            }))
            .unwrap();
        app.task = Some(Task {
            receiver,
            kind: TaskKind::Scene,
            label: "Test",
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
        });
        app.poll_task(&context);
        assert!(app.scene_loaded);
        assert!(!app.pending_scene);
        assert_eq!(app.scene_error.as_deref(), Some("invalid scene"));
    }

    #[test]
    fn image_validation_prevents_egui_panics_on_bad_payloads() {
        assert!(checked_color_image(0, 1, vec![]).is_err());
        assert!(checked_color_image(2, 2, vec![0; 15]).is_err());
        assert!(checked_color_image(u32::MAX, u32::MAX, vec![]).is_err());
        assert!(checked_color_image(1, 1, vec![255; 4]).is_ok());
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
