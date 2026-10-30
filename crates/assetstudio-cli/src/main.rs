mod schema_generator;

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use assetstudio_core::asset_map::{AssetMap, AssetMapFormat};
use assetstudio_core::cab_map::CabMap;
use assetstudio_core::unity_cn_keys::UnityCnKeys;
use assetstudio_core::{
    AssetFilter, AssetKey, AssetRecord, ExportMode, ExportOptions, FbxExportSettings, FbxUvType,
    GroupMode, ImageFormat, LoadOptions, ModelExportOptions, ModelFormat, OperationReport, Session,
};
use clap::{Parser, ValueEnum};
use serde_json::json;

const MAX_FILTER_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FILTER_PATTERNS: usize = 16_384;
const MAX_PATH_IDS: usize = 200_000;

/// Inspect and export Unity assets without a managed runtime.
#[derive(Debug, Parser)]
#[command(
    name = "AssetStudioCLI",
    version,
    about,
    after_help = "Examples:\n  AssetStudioCLI game_Data exported --game Normal\n  AssetStudioCLI bundle --operation list --json\n  AssetStudioCLI bundle exported --types Texture2D Sprite --names '^Icon'\n  AssetStudioCLI bundle exported --operation model --model-format fbx-binary\n  AssetStudioCLI bundle unpacked --operation extract\n\nExit codes: 0 success, 1 runtime error, 2 invalid arguments, 3 partial failure.\nExisting files are preserved unless --overwrite is specified."
)]
struct Args {
    /// Input file or directory.
    input_path: PathBuf,
    /// Output directory for export, extract, model, or map operations.
    output_path: Option<PathBuf>,

    /// Operation to perform. The default retains the legacy positional export invocation.
    #[arg(long, value_enum, ignore_case = true, default_value = "export")]
    operation: Operation,
    /// Game-specific format profile. Normal reads standard Unity files.
    #[arg(long, default_value = "Normal")]
    game: String,
    /// Maximum catalog objects. Larger values also raise bounded metadata budgets.
    #[arg(long = "max-assets", default_value_t = 1_000_000, value_parser = clap::value_parser!(u32).range(1..=10_000_000))]
    maximum_assets: u32,
    /// Unity version override, for example 2022.3.62f1.
    #[arg(long = "unity_version", alias = "unity-version")]
    unity_version: Option<String>,
    /// Explicit UnityCN key, as 32 hexadecimal digits.
    #[arg(long = "unity-cn-key", alias = "unity_cn_key")]
    unity_cn_key: Option<String>,
    /// MiHoYoBinData XOR byte, decimal or prefixed with 0x.
    #[arg(long, alias = "xor_key", alias = "xor-key", value_parser = parse_byte)]
    key: Option<u8>,

    /// Class names or IDs; supports legacy :Parse, :Export, and :Both suffixes.
    #[arg(long, alias = "type", num_args = 1.., allow_negative_numbers = true, action = clap::ArgAction::Append)]
    types: Vec<String>,
    /// Case-insensitive name regexes, or one UTF-8 file with a regex per line.
    #[arg(long, alias = "filter", num_args = 1.., action = clap::ArgAction::Append)]
    names: Vec<String>,
    /// Case-insensitive container regexes, or one UTF-8 filter file.
    #[arg(long, num_args = 1.., action = clap::ArgAction::Append)]
    containers: Vec<String>,
    /// Restrict selected objects to these exact signed PathIDs.
    #[arg(long = "path-ids", alias = "path_ids", num_args = 1.., allow_negative_numbers = true)]
    path_ids: Vec<i64>,
    /// Exclude AssetBundle objects from the selected assets.
    #[arg(long = "no_asset_bundle", alias = "no-asset-bundle")]
    no_asset_bundle: bool,
    /// Exclude IndexObject and MiHoYoBinData objects.
    #[arg(long = "no_index_object", alias = "no-index-object")]
    no_index_object: bool,

    /// Export mode: Convert, Raw, Dump, or JSON.
    #[arg(
        long = "export_type",
        alias = "export-type",
        value_enum,
        ignore_case = true,
        default_value = "convert"
    )]
    export_type: Mode,
    /// Output grouping: ByType, ByContainer, BySource, or None.
    #[arg(long = "group_assets", aliases = ["group_assets_type", "group-assets"], value_enum, ignore_case = true, default_value = "ByType")]
    group_assets: Group,
    /// Format used for converted textures and sprites.
    #[arg(
        long = "image-format",
        value_enum,
        ignore_case = true,
        default_value = "png"
    )]
    image_format: Image,
    /// Permit replacement of existing exports. The default preserves them.
    #[arg(long)]
    overwrite: bool,

    /// Map operation: None, AssetMap, CABMap, Both, or All.
    #[arg(long = "map_op", alias = "map-op", default_value = "None")]
    map_op: String,
    /// Also write an asset map during an export operation.
    #[arg(long = "write-map", alias = "write_map")]
    write_map: bool,
    /// Asset map format.
    #[arg(
        long = "map_type",
        alias = "map-type",
        value_enum,
        ignore_case = true,
        default_value = "xml"
    )]
    map_type: Map,
    /// Plain file name for the asset map, without a directory.
    #[arg(long = "map_name", alias = "map-name", default_value = "assets_map")]
    map_name: String,
    /// Load an existing JSON, XML, or legacy MessagePack asset list before export.
    #[arg(long = "asset-map", alias = "asset_map")]
    asset_map: Option<PathBuf>,
    /// Constrain cached asset Sources to this directory; defaults to the input directory.
    #[arg(long = "asset-base", alias = "asset_base")]
    asset_base: Option<PathBuf>,
    /// Read a CAB cache and resolve all transitive input dependencies.
    #[arg(long = "cab-map", alias = "cab_map")]
    cab_map: Option<PathBuf>,
    /// Base folder for CAB generation, or explicit relocation of a loaded cache.
    #[arg(long = "cab-base", alias = "cab_base")]
    cab_base: Option<PathBuf>,
    /// Trusted JSON MonoBehaviour schema files; earlier files take priority.
    #[arg(long = "mono-schema", action = clap::ArgAction::Append)]
    mono_schemas: Vec<PathBuf>,
    /// Model format, including native Rust FBX 7.4 output.
    #[arg(
        long = "model-format",
        value_enum,
        ignore_case = true,
        default_value = "fbx-binary"
    )]
    model_format: Model,

    /// Export models without decoding controller or clip animations.
    #[arg(long = "no-animations", aliases = ["ignore-controller-anim", "ignore_controller_anim"])]
    no_animations: bool,
    /// Omit model texture decoding and model-side image outputs.
    #[arg(long = "no-textures")]
    no_textures: bool,
    /// Omit skin weights and bone bindings from converted models.
    #[arg(long = "no-skins")]
    no_skins: bool,
    /// Omit model materials and their texture dependencies.
    #[arg(long = "no-materials")]
    no_materials: bool,
    /// Omit model blend-shape geometry and its animation tracks.
    #[arg(long = "no-blend-shapes")]
    no_blend_shapes: bool,
    /// Retain mesh/bone nodes and required ancestors, pruning unused scene nodes.
    #[arg(long = "prune-nodes")]
    prune_nodes: bool,
    /// Positive geometry, bind-pose and translation scale.
    #[arg(long = "scale-factor", default_value_t = 1.0)]
    scale_factor: f32,
    /// Preserve wrapped Euler values instead of unwrapping +/-180 degree jumps.
    #[arg(long = "no-euler-filter")]
    no_euler_filter: bool,
    /// Enabled UV channel indexes, separated by commas.
    #[arg(long = "uv-channels", value_delimiter = ',', num_args = 1..=8, value_parser = clap::value_parser!(u8).range(0..=7), default_value = "0,1,2,3,4,5,6,7")]
    uv_channels: Vec<u8>,
    /// UV semantic override, e.g. 1:NormalMap; repeat for several channels.
    #[arg(long = "uv-slot", action = clap::ArgAction::Append)]
    uv_slots: Vec<String>,
    /// FBX schema version (7100, 7200, 7300, 7400, or 7500).
    #[arg(long = "fbx-version", default_value_t = 7400)]
    fbx_version: u32,
    /// Mark every exported node as a joint in FBX.
    #[arg(long = "cast-to-bone")]
    cast_to_bone: bool,
    /// FBX joint display size.
    #[arg(long = "bone-size", default_value_t = 10.0)]
    bone_size: f64,
    /// Write list, inspect, and scene output as JSON.
    #[arg(long)]
    json: bool,
    /// Continue after an input fails to load. A partial load still exits with code 3.
    #[arg(long = "continue-on-error")]
    continue_on_error: bool,
    /// Hide informational messages. Errors and partial failures remain visible.
    #[arg(long)]
    silent: bool,
    /// Legacy logging categories; recognized categories control informational output.
    #[arg(long = "logger_flags", alias = "logger-flags", num_args = 1..)]
    logger_flags: Vec<String>,

    /// UnityCN key-list index; defaults to zero when UnityCN has no explicit key.
    #[arg(
        long = "key_index",
        alias = "key-index",
        conflicts_with = "unity_cn_key"
    )]
    key_index: Option<u32>,
    /// UnityCN key JSON file; defaults to Keys.json beside this executable.
    #[arg(long = "keys-file", alias = "keys_file")]
    keys_file: Option<PathBuf>,
    /// Game asset-index JSON used to recover original container names.
    #[arg(long = "ai_file", alias = "ai-file")]
    ai_file: Option<PathBuf>,
    /// Read managed DLL metadata in an isolated Rust helper to recover schemas.
    #[arg(long = "dummy_dlls", alias = "dummy-dlls")]
    dummy_dlls: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Operation {
    Export,
    List,
    Inspect,
    Extract,
    Scene,
    Model,
    Map,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Mode {
    Convert,
    Raw,
    Dump,
    #[value(name = "JSON", alias = "json")]
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Group {
    #[value(name = "ByType", alias = "by-type")]
    ByType,
    #[value(name = "ByContainer", alias = "by-container")]
    ByContainer,
    #[value(name = "BySource", alias = "by-source")]
    BySource,
    #[value(name = "None", alias = "none")]
    None,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Image {
    Png,
    #[value(alias = "jpg")]
    Jpeg,
    Bmp,
    Tga,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Map {
    Json,
    Xml,
    #[value(name = "MessagePack", alias = "messagepack", alias = "map")]
    MessagePack,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Model {
    FbxAscii,
    FbxBinary,
    Obj,
}

struct Selection {
    filter: AssetFilter,
    type_flags: Vec<(String, bool)>,
    types_supplied: bool,
    path_ids: HashSet<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapPlan {
    Asset,
    Cab,
    Both,
}

struct Validated {
    operation: Operation,
    load: LoadOptions,
    key_source: Option<PathBuf>,
    map_plan: MapPlan,
    cab_input: Option<PathBuf>,
    asset_input: Option<PathBuf>,
    asset_format: AssetMapFormat,
    filter_sources: Vec<PathBuf>,
    maps_after_export: bool,
    selection: Selection,
    info_logs: bool,
    map_filename: String,
}

enum Failure {
    Usage(String),
    Runtime(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(value: anyhow::Error) -> Self {
        Self::Runtime(value)
    }
}

fn main() -> ExitCode {
    let raw_arguments: Vec<_> = std::env::args_os().collect();
    if raw_arguments
        .get(1)
        .is_some_and(|value| value == "--internal-schema-generator")
    {
        if raw_arguments.len() != 3 {
            eprintln!("AssetStudioCLI: internal schema generation requires one request file");
            return ExitCode::from(2);
        }
        return match schema_generator::run_child(Path::new(&raw_arguments[2])) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("AssetStudioCLI: {error:#}");
                ExitCode::from(1)
            }
        };
    }
    let args = Args::parse_from(raw_arguments.into_iter().map(|argument| {
        if argument == "-?" {
            "--help".into()
        } else {
            argument
        }
    }));
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    let result = execute(&args, &mut output);
    let result = match result {
        Ok(code) => output
            .flush()
            .map(|()| code)
            .map_err(|error| Failure::Runtime(error.into())),
        Err(error) => Err(error),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(Failure::Usage(message)) => {
            eprintln!("AssetStudioCLI: {message}\nRun with --help for usage.");
            ExitCode::from(2)
        }
        Err(Failure::Runtime(error)) => {
            if is_broken_pipe(&error) {
                return ExitCode::SUCCESS;
            }
            eprintln!("AssetStudioCLI: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
            || cause
                .downcast_ref::<serde_json::Error>()
                .is_some_and(|error| error.io_error_kind() == Some(io::ErrorKind::BrokenPipe))
    })
}

fn execute(args: &Args, output: &mut impl Write) -> std::result::Result<u8, Failure> {
    // Validate every option before opening or creating anything.
    let validated = validate(args).map_err(|error| Failure::Usage(format!("{error:#}")))?;
    let load = validated.load.clone();
    let model_options =
        model_options(args).map_err(|error| Failure::Usage(format!("{error:#}")))?;
    let export = ExportOptions {
        mode: match args.export_type {
            Mode::Convert => ExportMode::Convert,
            Mode::Raw => ExportMode::Raw,
            Mode::Dump => ExportMode::Dump,
            Mode::Json => ExportMode::Json,
        },
        group: match args.group_assets {
            Group::ByType => GroupMode::ByType,
            Group::ByContainer => GroupMode::ByContainer,
            Group::BySource => GroupMode::BySource,
            Group::None => GroupMode::None,
        },
        image_format: match args.image_format {
            Image::Png => ImageFormat::Png,
            Image::Jpeg => ImageFormat::Jpeg,
            Image::Bmp => ImageFormat::Bmp,
            Image::Tga => ImageFormat::Tga,
        },
        overwrite: args.overwrite,
    };
    if validated.operation == Operation::Extract {
        let auxiliary: Vec<PathBuf> = validated.key_source.iter().cloned().collect();
        let report = Session::extract_paths_with_auxiliary_inputs(
            std::slice::from_ref(&args.input_path),
            output_directory(args)?,
            &load,
            args.overwrite,
            &auxiliary,
        )?;
        return Ok(print_report(&report, validated.info_logs));
    }

    let asset_cache = validated
        .asset_input
        .as_deref()
        .map(|path| AssetMap::from_path(path, validated.asset_format))
        .transpose()?;
    let mut asset_base = None;
    let mut asset_indexes = Vec::new();
    let mut input_paths = vec![args.input_path.clone()];
    if let Some(map) = &asset_cache {
        if let Some(game) = map.game_type() {
            let expected = assetstudio_core::asset_map::game_name(game)
                .context("asset map references an unknown game profile")?;
            if !args.game.eq_ignore_ascii_case(expected) {
                return Err(Failure::Runtime(anyhow::anyhow!(
                    "asset map requires --game {expected}; the selected game differs"
                )));
            }
        }
        asset_indexes = map
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                selected(&entry.record(), args, &validated.selection).then_some(index)
            })
            .collect();
        if asset_indexes.is_empty() {
            return empty_cached_selection(args, &validated, output);
        }
        let base = if let Some(base) = &args.asset_base {
            base.clone()
        } else {
            let input =
                std::fs::canonicalize(&args.input_path).context("resolve asset map input base")?;
            if input.is_dir() {
                input
            } else {
                input
                    .parent()
                    .context("asset map input has no parent directory")?
                    .to_owned()
            }
        };
        input_paths = map.resolve_sources(&asset_indexes, &base)?;
        asset_base = Some(base);
    }
    let cache = validated
        .cab_input
        .as_deref()
        .map(CabMap::from_path)
        .transpose()?;
    if let Some(map) = &cache {
        input_paths = map.resolve_paths(&input_paths, args.cab_base.as_deref())?;
        if validated.info_logs {
            eprintln!(
                "Resolved {} input path(s) through the CAB cache",
                input_paths.len()
            );
        }
    }
    let mut session = Session::open_paths(&input_paths, &load)?;
    if let Some(path) = &validated.asset_input {
        session.protect_input_path(path)?;
    }
    if let Some(path) = &validated.key_source {
        session.protect_input_path(path)?;
    }
    if let Some(path) = &validated.cab_input {
        session.protect_input_path(path)?;
    }
    for path in &validated.filter_sources {
        session.protect_input_path(path)?;
    }
    for schema in &args.mono_schemas {
        let loaded = session.load_schemas(schema)?;
        if validated.info_logs {
            eprintln!(
                "Loaded {loaded} MonoBehaviour schema(s) from {}",
                schema.display()
            );
        }
    }
    if let Some(folder) = &args.dummy_dlls {
        let loaded = session.load_dummy_dlls(
            folder,
            &std::env::current_exe().context("locate schema helper executable")?,
        )?;
        if validated.info_logs {
            eprintln!("Recovered {loaded} MonoBehaviour schema(s) from DLL metadata");
        }
    }
    if let Some(index_path) = &args.ai_file {
        let recovered = session.recover_containers(index_path)?;
        if validated.info_logs {
            eprintln!("Recovered container names for {recovered} asset(s)");
        }
    }
    for warning in session.warnings() {
        eprintln!("warning: {warning}");
    }
    let diagnostics = session.diagnostics();
    for diagnostic in diagnostics {
        eprintln!("warning: {diagnostic}");
    }
    let partial_load = !diagnostics.is_empty();
    let cached_keys = if let Some(map) = &asset_cache {
        Some(
            map.keys_in_session(
                &asset_indexes,
                &session,
                asset_base.as_deref().context("asset map base is missing")?,
            )?
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        )
    } else {
        None
    };
    let assets: Vec<&AssetRecord> = session
        .assets()
        .iter()
        .filter(|asset| {
            if let Some(keys) = &cached_keys {
                keys.contains(&asset.key)
            } else {
                selected(asset, args, &validated.selection)
            }
        })
        .collect();
    let keys: Vec<AssetKey> = assets.iter().map(|asset| asset.key).collect();

    let mut code = 0;
    match validated.operation {
        Operation::List => write_listing(&assets, args.json, output)?,
        Operation::Inspect => {
            if args.json {
                let classes = class_counts(&assets);
                write_json(
                    output,
                    &json!({
                        "input": args.input_path.to_string_lossy(),
                        "assets": assets.len(),
                        "classes": classes,
                        "diagnostics": diagnostics,
                    }),
                )?;
            } else {
                writeln!(output, "Input: {}", args.input_path.display())
                    .context("cannot write input summary")?;
                writeln!(output, "Selected assets: {}", assets.len())
                    .context("cannot write asset count")?;
                for (name, count) in class_counts(&assets) {
                    writeln!(output, "{name}\t{count}").context("cannot write class summary")?;
                }
            }
        }
        Operation::Scene => {
            let nodes = session.scene()?;
            if args.json {
                let values = nodes.iter().map(|node| {
                    json!({
                        "file_index": node.key.file_index,
                        "path_id": node.key.path_id,
                        "name": node.name,
                        "parent": node.parent.map(|key| json!({
                            "file_index": key.file_index, "path_id": key.path_id,
                        })),
                    })
                });
                write_json_sequence(output, values)?;
            } else {
                for node in nodes {
                    writeln!(
                        output,
                        "{}:{}\t{}\tparent={}",
                        node.key.file_index,
                        node.key.path_id,
                        visible_text(&node.name),
                        node.parent.map_or_else(
                            || "<root>".to_owned(),
                            |key| format!("{}:{}", key.file_index, key.path_id),
                        ),
                    )
                    .context("cannot write scene")?;
                }
            }
        }
        Operation::Export => {
            if validated.info_logs {
                eprintln!("Selected {} asset(s)", keys.len());
            }
            let report = session.export_with_model_options(
                &keys,
                output_directory(args)?,
                &export,
                &model_options,
            )?;
            code = print_report(&report, validated.info_logs);
            if args.write_map || validated.maps_after_export {
                code = code.max(write_requested_maps(
                    &session,
                    &keys,
                    args,
                    &validated,
                    cache.as_ref(),
                )?);
            }
        }
        Operation::Map => {
            code = write_requested_maps(&session, &keys, args, &validated, cache.as_ref())?;
        }
        Operation::Model => {
            let filtered = !args.types.is_empty()
                || !args.names.is_empty()
                || !args.containers.is_empty()
                || !validated.selection.path_ids.is_empty()
                || args.no_asset_bundle
                || args.no_index_object
                || validated.asset_input.is_some();
            // Empty roots mean the whole scene in Core. Never turn an empty
            // filtered selection into an accidental whole-scene export.
            let roots: Vec<_> = assets
                .iter()
                .filter(|asset| matches!(asset.class_id, 1 | 95))
                .map(|asset| asset.key)
                .collect();
            if filtered && roots.is_empty() {
                return Err(Failure::Runtime(anyhow::anyhow!(
                    "no GameObject or Animator roots matched the model filters"
                )));
            }
            let format = match args.model_format {
                Model::FbxAscii => ModelFormat::FbxAscii,
                Model::FbxBinary => ModelFormat::FbxBinary,
                Model::Obj => ModelFormat::Obj,
            };
            let report = session.export_scene_with_options(
                output_directory(args)?,
                format,
                if filtered { &roots } else { &[] },
                &model_options,
            )?;
            code = print_report(&report, validated.info_logs);
        }
        Operation::Extract => {
            return Err(Failure::Runtime(anyhow::anyhow!("invalid operation state")));
        }
    }
    Ok(if partial_load { code.max(3) } else { code })
}

fn model_options(args: &Args) -> Result<ModelExportOptions> {
    let mut uv_enabled = [false; 8];
    for &channel in &args.uv_channels {
        *uv_enabled
            .get_mut(usize::from(channel))
            .context("UV channel must be between 0 and 7")? = true;
    }
    anyhow::ensure!(
        args.uv_slots.len() <= 8,
        "at most eight --uv-slot mappings may be supplied"
    );
    let mut fbx = FbxExportSettings {
        version: args.fbx_version,
        cast_to_bone: args.cast_to_bone,
        bone_size: args.bone_size,
        ..FbxExportSettings::default()
    };
    let mut mapped = [false; 8];
    for mapping in &args.uv_slots {
        anyhow::ensure!(mapping.len() <= 64, "UV slot mapping exceeds 64 bytes");
        let (channel, semantic) = mapping
            .split_once(':')
            .context("UV slot mapping must be channel:semantic")?;
        let channel = channel
            .parse::<usize>()
            .context("UV slot channel must be an integer from 0 to 7")?;
        let slot = fbx
            .uv_types
            .get_mut(channel)
            .context("UV slot channel must be between 0 and 7")?;
        anyhow::ensure!(
            !mapped[channel],
            "a UV slot channel may only be mapped once"
        );
        mapped[channel] = true;
        *slot = match semantic.to_ascii_lowercase().as_str() {
            "diffuse" | "0" => FbxUvType::Diffuse,
            "normalmap" | "normal-map" | "1" => FbxUvType::NormalMap,
            "specular" | "2" => FbxUvType::Specular,
            "bump" | "3" => FbxUvType::Bump,
            "ambient" | "4" => FbxUvType::Ambient,
            "emissive" | "5" => FbxUvType::Emissive,
            "reflection" | "6" => FbxUvType::Reflection,
            "displacement" | "7" => FbxUvType::Displacement,
            _ => anyhow::bail!(
                "UV semantic must be Diffuse, NormalMap, Specular, Bump, Ambient, Emissive, Reflection or Displacement"
            ),
        };
    }
    let options = ModelExportOptions {
        include_animations: !args.no_animations,
        include_textures: !args.no_textures,
        include_skins: !args.no_skins,
        include_materials: !args.no_materials,
        include_blend_shapes: !args.no_blend_shapes,
        export_all_nodes: !args.prune_nodes,
        scale_factor: args.scale_factor,
        euler_filter: !args.no_euler_filter,
        uv_enabled,
        fbx,
    };
    options.validate()?;
    Ok(options)
}

fn validate(args: &Args) -> Result<Validated> {
    let model = model_options(args)?;
    let compact_map_op = args
        .map_op
        .split([',', '|', '+'])
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let mut cab_input = args.cab_map.clone();
    let mut asset_input = args.asset_map.clone();
    let mut asset_format = AssetMapFormat::Auto;
    let mut map_name_is_input = false;
    let (operation, map_plan, maps_after_export) = match compact_map_op.as_slice() {
        [name] if name == "none" => (args.operation, MapPlan::Asset, false),
        [name] if name == "assetmap" => (Operation::Map, MapPlan::Asset, false),
        [name] if name == "cabmap" => (Operation::Map, MapPlan::Cab, false),
        [name] if name == "both" => (Operation::Map, MapPlan::Both, false),
        [name] if name == "all" => (Operation::Export, MapPlan::Both, true),
        values
            if values.len() == 2
                && values.iter().any(|value| value == "cabmap")
                && values.iter().any(|value| value == "load") =>
        {
            if cab_input.is_none() {
                cab_input = Some(Path::new("Maps").join(cab_map_filename(&args.map_name)?));
            }
            (args.operation, MapPlan::Asset, false)
        }
        values
            if values.len() == 2
                && values.iter().any(|value| value == "assetmap")
                && values.iter().any(|value| value == "load") =>
        {
            if asset_input.is_none() {
                asset_input = Some(PathBuf::from(&args.map_name));
                map_name_is_input = true;
            }
            asset_format = map_format(args.map_type);
            (args.operation, MapPlan::Asset, false)
        }
        [name] if name == "load" && cab_input.is_some() => (args.operation, MapPlan::Asset, false),
        _ => anyhow::bail!(
            "invalid --map_op {:?}; use None, AssetMap, CABMap, Both, All, or CABMap,Load. AssetMap,Load selects cached asset identities; --cab-map resolves dependency caches",
            args.map_op
        ),
    };
    if operation != args.operation && args.operation != Operation::Export {
        anyhow::bail!("--map_op conflicts with the selected --operation");
    }
    if model != ModelExportOptions::default()
        && !matches!(operation, Operation::Export | Operation::Model)
    {
        anyhow::bail!(
            "model controls (--no-animations, --no-textures, scale, UV and FBX settings) apply to asset or model export"
        );
    }
    if args.mono_schemas.len() > 64 {
        anyhow::bail!("at most 64 --mono-schema files may be supplied");
    }
    if matches!(
        operation,
        Operation::Export | Operation::Extract | Operation::Model | Operation::Map
    ) && args.output_path.is_none()
    {
        anyhow::bail!("an output_path is required for this operation");
    }
    if args.write_map && operation != Operation::Export {
        anyhow::bail!("--write-map requires --operation export");
    }
    if args.json
        && !matches!(
            operation,
            Operation::List | Operation::Inspect | Operation::Scene
        )
    {
        anyhow::bail!(
            "--json is for list, inspect, and scene; use --export_type JSON or --map_type JSON for file output"
        );
    }
    if operation == Operation::Extract
        && (!args.types.is_empty()
            || !args.names.is_empty()
            || !args.containers.is_empty()
            || args.ai_file.is_some()
            || cab_input.is_some()
            || args.cab_base.is_some()
            || !args.mono_schemas.is_empty()
            || args.dummy_dlls.is_some()
            || !args.path_ids.is_empty()
            || asset_input.is_some()
            || args.asset_base.is_some()
            || args.no_asset_bundle
            || args.no_index_object)
    {
        anyhow::bail!("asset filters cannot be applied to container extraction");
    }
    if operation == Operation::Scene
        && (!args.types.is_empty()
            || !args.names.is_empty()
            || !args.containers.is_empty()
            || !args.path_ids.is_empty()
            || asset_input.is_some()
            || args.no_asset_bundle
            || args.no_index_object)
    {
        anyhow::bail!(
            "scene listing does not apply asset filters; use --operation list for filtered assets"
        );
    }
    if args.game.trim().is_empty() {
        anyhow::bail!("--game cannot be empty");
    }
    let (load, key_source) = load_options(args)?;
    load.validate()?;
    if args.overwrite
        && (operation == Operation::Model
            || (operation == Operation::Map && map_plan != MapPlan::Cab))
    {
        anyhow::bail!(
            "--overwrite is implemented for asset export, extraction, and CAB caches; model and asset-map exports preserve existing files"
        );
    }
    if args.cab_base.is_some()
        && cab_input.is_none()
        && !matches!(map_plan, MapPlan::Cab | MapPlan::Both)
    {
        anyhow::bail!("--cab-base requires CAB cache loading or generation");
    }
    if args.asset_base.is_some() && asset_input.is_none() {
        anyhow::bail!("--asset-base requires an asset map input");
    }
    if map_name_is_input && (args.write_map || operation == Operation::Map || maps_after_export) {
        anyhow::bail!(
            "use --asset-map to load a cache when --map_name is needed for output naming"
        );
    }
    let mut info_logs = !args.silent;
    if !args.logger_flags.is_empty() {
        for flag in &args.logger_flags {
            if !["Verbose", "Debug", "Info", "Warning", "Error", "None"]
                .iter()
                .any(|candidate| flag.eq_ignore_ascii_case(candidate))
            {
                anyhow::bail!("unknown --logger_flags category {flag:?}");
            }
        }
        info_logs &= args.logger_flags.iter().any(|flag| {
            ["Info", "Debug", "Verbose"]
                .iter()
                .any(|name| flag.eq_ignore_ascii_case(name))
        });
    }
    let (names, name_source) = read_patterns(&args.names)?;
    let (containers, container_source) = read_patterns(&args.containers)?;
    let filter_sources = name_source.into_iter().chain(container_source).collect();
    if args.path_ids.len() > MAX_PATH_IDS {
        anyhow::bail!("at most {MAX_PATH_IDS} PathIDs may be supplied");
    }
    let path_ids = args.path_ids.iter().copied().collect();
    let mut type_flags = Vec::new();
    for value in &args.types {
        let (name, setting) = value.split_once(':').unwrap_or((value, "Both"));
        if name.trim().is_empty() {
            anyhow::bail!("an empty class name is invalid");
        }
        if !["Parse", "Export", "Both", "None"]
            .iter()
            .any(|value| setting.eq_ignore_ascii_case(value))
        {
            anyhow::bail!("invalid class flag {setting:?}; expected Parse, Export, Both, or None");
        }
        type_flags.push((
            name.to_owned(),
            setting.eq_ignore_ascii_case("Export") || setting.eq_ignore_ascii_case("Both"),
        ));
    }
    let all_classes: Vec<_> = type_flags.iter().map(|(name, _)| name.clone()).collect();
    let filter = AssetFilter::new(&all_classes, &names, &containers)?;
    let map_filename = map_filename(
        if map_name_is_input {
            "assets_map"
        } else {
            &args.map_name
        },
        args.map_type,
    )?;
    Ok(Validated {
        operation,
        load,
        key_source,
        map_plan,
        cab_input,
        asset_input,
        asset_format,
        filter_sources,
        maps_after_export,
        selection: Selection {
            filter,
            type_flags,
            types_supplied: !args.types.is_empty(),
            path_ids,
        },
        info_logs,
        map_filename,
    })
}

fn selected(asset: &AssetRecord, args: &Args, selection: &Selection) -> bool {
    if !selection.path_ids.is_empty() && !selection.path_ids.contains(&asset.key.path_id) {
        return false;
    }
    if !selection.filter.matches(asset) {
        return false;
    }
    if selection.types_supplied
        && !selection
            .type_flags
            .iter()
            .rev()
            .find(|(class, _)| {
                class.eq_ignore_ascii_case(&asset.class_name)
                    || assetstudio_core::class_id(class) == Some(asset.class_id)
            })
            .is_some_and(|(_, export)| *export)
    {
        return false;
    }
    if args.no_asset_bundle && asset.class_id == 142 {
        return false;
    }
    if args.no_index_object
        && (asset.class_name.eq_ignore_ascii_case("IndexObject")
            || asset.class_name.eq_ignore_ascii_case("MiHoYoBinData"))
    {
        return false;
    }
    true
}

fn load_options(args: &Args) -> Result<(LoadOptions, Option<PathBuf>)> {
    let mut unity_cn_key = args.unity_cn_key.clone();
    let mut key_source = None;
    let selected_index = args.key_index.is_some() || args.keys_file.is_some();
    if selected_index && !args.game.eq_ignore_ascii_case("UnityCN") {
        anyhow::bail!("--key_index and --keys-file require --game UnityCN");
    }
    if args.unity_cn_key.is_some() && (args.keys_file.is_some() || args.key_index.is_some()) {
        anyhow::bail!("choose an explicit --unity-cn-key or a key-list index, not both");
    }
    if args.game.eq_ignore_ascii_case("UnityCN") && unity_cn_key.is_none() {
        let path = if let Some(path) = &args.keys_file {
            path.clone()
        } else {
            UnityCnKeys::beside_executable(
                &std::env::current_exe().context("locate default UnityCN key list")?,
            )?
        };
        let keys = UnityCnKeys::from_path(&path)?;
        let index = usize::try_from(args.key_index.unwrap_or(0))?;
        unity_cn_key = Some(keys.key(index)?.to_owned());
        key_source = Some(path);
    }
    Ok((
        LoadOptions {
            game: args.game.clone(),
            unity_version: args.unity_version.clone(),
            unity_cn_key,
            xor_key: args.key.unwrap_or(0),
            continue_on_error: args.continue_on_error,
            maximum_assets: args.maximum_assets as usize,
        },
        key_source,
    ))
}

fn write_requested_maps(
    session: &Session,
    keys: &[AssetKey],
    args: &Args,
    validated: &Validated,
    loaded_cache: Option<&CabMap>,
) -> Result<u8> {
    let mut code = 0;
    if matches!(validated.map_plan, MapPlan::Asset | MapPlan::Both) {
        let path = output_directory(args)?.join(&validated.map_filename);
        code = print_report(
            &session.write_asset_cache(keys, &path, map_format(args.map_type), &args.game)?,
            validated.info_logs,
        );
    }
    if matches!(validated.map_plan, MapPlan::Cab | MapPlan::Both) {
        let base = if let Some(base) = &args.cab_base {
            base.clone()
        } else if let Some(cache) = loaded_cache {
            cache.resolved_base(None)?
        } else {
            let input =
                std::fs::canonicalize(&args.input_path).context("resolve CAB input base")?;
            if input.is_dir() {
                input
            } else {
                input
                    .parent()
                    .context("CAB input has no base directory")?
                    .to_owned()
            }
        };
        let path = output_directory(args)?.join(cab_map_filename(&args.map_name)?);
        code = code.max(print_report(
            &session.write_cab_map(&base, &path, args.overwrite)?,
            validated.info_logs,
        ));
    }
    Ok(code)
}

fn cab_map_filename(name: &str) -> Result<String> {
    // Reuse the same traversal and portable-name checks as asset maps.
    map_filename(name, Map::Json)?;
    if name.to_ascii_lowercase().ends_with(".bin") {
        Ok(name.to_owned())
    } else {
        Ok(format!("{name}.bin"))
    }
}

fn output_directory(args: &Args) -> Result<&Path> {
    args.output_path
        .as_deref()
        .context("an output_path is required for this operation")
}

fn parse_byte(value: &str) -> std::result::Result<u8, String> {
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u8::from_str_radix(hex, 16)
    } else {
        value.parse::<u8>()
    };
    parsed.map_err(|_| "expected a byte in 0..255 or 0x00..0xFF".to_owned())
}

fn read_patterns(values: &[String]) -> Result<(Vec<String>, Option<PathBuf>)> {
    if values.len() > MAX_FILTER_PATTERNS {
        anyhow::bail!("too many filter patterns");
    }
    if values.len() == 1 {
        let path = Path::new(&values[0]);
        if path.is_file() {
            let file = File::open(path)
                .with_context(|| format!("cannot open filter file {}", path.display()))?;
            if file.metadata()?.len() > MAX_FILTER_FILE_BYTES {
                anyhow::bail!("filter file exceeds the 4 MiB limit");
            }
            let mut contents = String::new();
            file.take(MAX_FILTER_FILE_BYTES + 1)
                .read_to_string(&mut contents)
                .context("filter file must contain valid UTF-8")?;
            if contents.len() as u64 > MAX_FILTER_FILE_BYTES {
                anyhow::bail!("filter file exceeds the 4 MiB limit");
            }
            let mut patterns = Vec::new();
            for line in contents.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if patterns.len() == MAX_FILTER_PATTERNS {
                    anyhow::bail!("filter file has too many patterns");
                }
                patterns.push(line.to_owned());
            }
            if patterns.is_empty() {
                anyhow::bail!("filter file contains no patterns");
            }
            return Ok((patterns, Some(std::fs::canonicalize(path)?)));
        }
    }
    if values.iter().any(|value| value.trim().is_empty()) {
        anyhow::bail!("an empty regex is invalid; omit the filter to select all assets");
    }
    Ok((values.to_vec(), None))
}

fn map_filename(name: &str, format: Map) -> Result<String> {
    if name.is_empty()
        || name.len() > 180
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || name.ends_with(' ')
        || name
            .chars()
            .any(|value| value.is_control() || "\\/:*?\"<>|".contains(value))
    {
        anyhow::bail!("--map_name must be a plain portable filename, without directories");
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let serial = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || serial.is_some_and(|number| {
        matches!(
            number,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    }) {
        anyhow::bail!("--map_name is a reserved device name");
    }
    let extension = match format {
        Map::Json => "json",
        Map::Xml => "xml",
        Map::MessagePack => "map",
    };
    let suffix = format!(".{extension}");
    if name.to_lowercase().ends_with(&suffix) {
        Ok(name.to_owned())
    } else {
        Ok(format!("{name}{suffix}"))
    }
}

fn map_format(value: Map) -> AssetMapFormat {
    match value {
        Map::Json => AssetMapFormat::Json,
        Map::Xml => AssetMapFormat::Xml,
        Map::MessagePack => AssetMapFormat::MessagePack,
    }
}

fn empty_cached_selection(
    args: &Args,
    validated: &Validated,
    output: &mut impl Write,
) -> std::result::Result<u8, Failure> {
    match validated.operation {
        Operation::Model => {
            return Err(Failure::Runtime(anyhow::anyhow!(
                "no GameObject or Animator roots matched the model filters"
            )));
        }
        Operation::List => write_listing(&[], args.json, output)?,
        Operation::Inspect if args.json => write_json(
            output,
            &json!({"input":args.input_path.to_string_lossy(),"assets":0,"classes":{},"diagnostics":[]}),
        )?,
        Operation::Inspect => {
            writeln!(output, "Selected assets: 0").context("write empty asset-map summary")?;
        }
        _ => {
            if validated.info_logs {
                eprintln!("No cached assets matched; no input files were loaded");
            }
        }
    }
    Ok(0)
}

fn print_report(report: &OperationReport, info_logs: bool) -> u8 {
    for failure in &report.failed {
        eprintln!("error: {failure}");
    }
    if info_logs {
        for skipped in &report.skipped {
            eprintln!("skipped: {skipped}");
        }
        eprintln!(
            "Written: {}, skipped: {}, failed: {}",
            report.written.len(),
            report.skipped.len(),
            report.failed.len()
        );
    }
    if report.failed.is_empty() { 0 } else { 3 }
}

fn write_listing(
    assets: &[&AssetRecord],
    json_output: bool,
    output: &mut impl Write,
) -> Result<()> {
    if json_output {
        let values = assets.iter().map(|asset| {
            json!({
                "file_index": asset.key.file_index,
                "path_id": asset.key.path_id,
                "class_id": asset.class_id,
                "class_name": asset.class_name,
                "name": asset.name,
                "container": asset.container,
                "source": asset.source,
                "size": asset.size,
            })
        });
        write_json_sequence(output, values)
    } else {
        writeln!(output, "file\tpath_id\ttype\tname\tsize\tcontainer\tsource")?;
        for asset in assets {
            writeln!(
                output,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                asset.key.file_index,
                asset.key.path_id,
                visible_text(&asset.class_name),
                visible_text(&asset.name),
                asset.size,
                visible_text(&asset.container),
                visible_text(&asset.source)
            )?;
        }
        Ok(())
    }
}

fn write_json_sequence<T: serde::Serialize>(
    output: &mut impl Write,
    values: impl IntoIterator<Item = T>,
) -> Result<()> {
    write!(output, "[")?;
    let mut first = true;
    for value in values {
        if first {
            first = false;
        } else {
            write!(output, ",")?;
        }
        writeln!(output)?;
        serde_json::to_writer_pretty(&mut *output, &value).context("cannot write JSON entry")?;
    }
    if !first {
        writeln!(output)?;
    }
    writeln!(output, "]")?;
    Ok(())
}

fn write_json(output: &mut impl Write, value: &impl serde::Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut *output, value).context("cannot write JSON")?;
    writeln!(output).context("cannot finish JSON output")?;
    Ok(())
}

fn class_counts(assets: &[&AssetRecord]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for asset in assets {
        *counts.entry(asset.class_name.clone()).or_default() += 1;
    }
    counts
}

/// Names from files may contain terminal escape codes and TSV separators.
fn visible_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_values_cover_decimal_and_hex_without_wrapping() {
        assert_eq!(parse_byte("255").unwrap(), 255);
        assert_eq!(parse_byte("0xA5").unwrap(), 165);
        assert_eq!(parse_byte("0Xff").unwrap(), 255);
        assert!(parse_byte("256").is_err());
        assert!(parse_byte("-1").is_err());
        assert!(parse_byte("0x100").is_err());
    }

    #[test]
    fn portable_map_names_cannot_escape_output_or_address_windows_devices() {
        for name in [
            "../map",
            "..\\map",
            "C:map",
            "map\n",
            "NUL",
            "CON.json",
            "map.",
            "map ",
            "CON .txt",
            "LPT1 .bin",
            "COM¹",
            "LPT²",
            "CONIN$",
            "CONOUT$",
        ] {
            assert!(map_filename(name, Map::Json).is_err(), "accepted {name:?}");
        }
        assert_eq!(map_filename("test", Map::Json).unwrap(), "test.json");
        assert_eq!(map_filename("test.JSON", Map::Json).unwrap(), "test.JSON");
    }

    #[test]
    fn escaped_terminal_text_cannot_inject_rows_or_escape_sequences() {
        assert_eq!(visible_text("a\tb\n\u{1b}[31m"), "a\\tb\\n\\u{1b}[31m");
        assert_eq!(visible_text("åäö"), "åäö");
    }

    #[test]
    fn legacy_aliases_and_enum_spelling_parse() {
        let args = Args::try_parse_from([
            "AssetStudioCLI",
            "input",
            "out",
            "--type",
            "Texture2D:Both",
            "--group_assets_type",
            "BySource",
            "--export_type",
            "JSON",
            "--xor_key",
            "0x42",
        ])
        .unwrap();
        assert_eq!(args.key, Some(0x42));
        assert!(matches!(args.group_assets, Group::BySource));
        assert!(matches!(args.export_type, Mode::Json));
    }

    #[test]
    fn json_broken_pipe_keeps_the_underlying_io_classification() {
        struct ClosedPipe;
        impl Write for ClosedPipe {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = write_json(&mut ClosedPipe, &json!({"entry": "value"})).unwrap_err();
        assert!(is_broken_pipe(&error));
        assert!(!is_broken_pipe(&anyhow::anyhow!("unrelated parser error")));
    }
}
