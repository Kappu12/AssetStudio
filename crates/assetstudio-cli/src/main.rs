use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use assetstudio_core::{
    AssetFilter, AssetKey, AssetRecord, ExportMode, ExportOptions, GroupMode, ImageFormat,
    LoadOptions, MapFormat, ModelFormat, OperationReport, Session,
};
use clap::{Parser, ValueEnum};
use serde_json::json;

const MAX_FILTER_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FILTER_PATTERNS: usize = 16_384;

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
    #[arg(long, alias = "type", num_args = 1.., action = clap::ArgAction::Append)]
    types: Vec<String>,
    /// Case-insensitive name regexes, or one UTF-8 file with a regex per line.
    #[arg(long, alias = "filter", num_args = 1.., action = clap::ArgAction::Append)]
    names: Vec<String>,
    /// Case-insensitive container regexes, or one UTF-8 filter file.
    #[arg(long, num_args = 1.., action = clap::ArgAction::Append)]
    containers: Vec<String>,
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

    /// Legacy map operation. AssetMap writes a map only; None exports normally.
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
    /// Model format, including native Rust FBX 7.4 output.
    #[arg(
        long = "model-format",
        value_enum,
        ignore_case = true,
        default_value = "fbx-binary"
    )]
    model_format: Model,

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

    /// Legacy automatic UnityCN key index; use --unity-cn-key instead.
    #[arg(long = "key_index", hide = true)]
    key_index: Option<u32>,
    /// Game asset-index JSON used to recover original container names.
    #[arg(long = "ai_file", alias = "ai-file")]
    ai_file: Option<PathBuf>,
    /// Legacy .NET assembly schema source; not executed by the Rust application.
    #[arg(long = "dummy_dlls", hide = true)]
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
}

struct Validated {
    operation: Operation,
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
    let args = Args::parse_from(std::env::args_os().map(|argument| {
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
    let load = load_options(args);
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
        let report = Session::extract(
            &args.input_path,
            output_directory(args)?,
            &load,
            args.overwrite,
        )?;
        return Ok(print_report(&report, validated.info_logs));
    }

    let mut session = Session::open(&args.input_path, &load)?;
    if let Some(index_path) = &args.ai_file {
        let recovered = session.recover_containers(index_path)?;
        if validated.info_logs {
            eprintln!("Recovered container names for {recovered} asset(s)");
        }
    }
    let diagnostics = session.diagnostics();
    for diagnostic in diagnostics {
        eprintln!("warning: {diagnostic}");
    }
    let partial_load = !diagnostics.is_empty();
    let assets: Vec<&AssetRecord> = session
        .assets()
        .iter()
        .filter(|asset| selected(asset, args, &validated.selection))
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
            let report = session.export(&keys, output_directory(args)?, &export)?;
            code = print_report(&report, validated.info_logs);
            if args.write_map {
                let path = output_directory(args)?.join(&validated.map_filename);
                let report = session.write_map(&keys, &path, map_format(args.map_type))?;
                code = code.max(print_report(&report, validated.info_logs));
            }
        }
        Operation::Map => {
            let path = output_directory(args)?.join(&validated.map_filename);
            let report = session.write_map(&keys, &path, map_format(args.map_type))?;
            code = print_report(&report, validated.info_logs);
        }
        Operation::Model => {
            let filtered = !args.types.is_empty()
                || !args.names.is_empty()
                || !args.containers.is_empty()
                || args.no_asset_bundle
                || args.no_index_object;
            // Empty roots mean the whole scene in Core. Never turn an empty
            // filtered selection into an accidental whole-scene export.
            let roots: Vec<_> = assets
                .iter()
                .filter(|asset| asset.class_id == 1)
                .map(|asset| asset.key)
                .collect();
            if filtered && roots.is_empty() {
                return Err(Failure::Runtime(anyhow::anyhow!(
                    "no GameObject roots matched the model filters"
                )));
            }
            let format = match args.model_format {
                Model::FbxAscii => ModelFormat::FbxAscii,
                Model::FbxBinary => ModelFormat::FbxBinary,
                Model::Obj => ModelFormat::Obj,
            };
            let report = session.export_scene(
                output_directory(args)?,
                format,
                if filtered { &roots } else { &[] },
            )?;
            code = print_report(&report, validated.info_logs);
        }
        Operation::Extract => {
            return Err(Failure::Runtime(anyhow::anyhow!("invalid operation state")));
        }
    }
    Ok(if partial_load { code.max(3) } else { code })
}

fn validate(args: &Args) -> Result<Validated> {
    if args.key_index.is_some() {
        anyhow::bail!("--key_index is not implemented; supply the explicit --unity-cn-key instead");
    }
    if args.dummy_dlls.is_some() {
        anyhow::bail!(
            "--dummy_dlls is not implemented: Rust does not load managed assemblies; embedded TypeTrees can still be exported as JSON"
        );
    }
    let operation = if args.map_op.eq_ignore_ascii_case("none") {
        args.operation
    } else if args.map_op.eq_ignore_ascii_case("assetmap") {
        if !matches!(args.operation, Operation::Export | Operation::Map) {
            anyhow::bail!("--map_op AssetMap conflicts with the selected --operation");
        }
        Operation::Map
    } else {
        anyhow::bail!(
            "--map_op {:?} is not implemented; use AssetMap for an asset list, or --write-map to combine a map with export. Legacy CABMap/cache loading is pending",
            args.map_op
        );
    };
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
            || args.no_asset_bundle
            || args.no_index_object)
    {
        anyhow::bail!("asset filters cannot be applied to container extraction");
    }
    if operation == Operation::Scene
        && (!args.types.is_empty()
            || !args.names.is_empty()
            || !args.containers.is_empty()
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
    load_options(args).validate()?;
    if args.overwrite && matches!(operation, Operation::Model | Operation::Map) {
        anyhow::bail!(
            "--overwrite is implemented for asset export and extraction; model and map exports preserve existing files"
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
    let names = read_patterns(&args.names)?;
    let containers = read_patterns(&args.containers)?;
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
    let map_filename = map_filename(&args.map_name, args.map_type)?;
    Ok(Validated {
        operation,
        selection: Selection {
            filter,
            type_flags,
            types_supplied: !args.types.is_empty(),
        },
        info_logs,
        map_filename,
    })
}

fn selected(asset: &AssetRecord, args: &Args, selection: &Selection) -> bool {
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
                    || class.parse::<i32>().ok() == Some(asset.class_id)
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

fn load_options(args: &Args) -> LoadOptions {
    LoadOptions {
        game: args.game.clone(),
        unity_version: args.unity_version.clone(),
        unity_cn_key: args.unity_cn_key.clone(),
        xor_key: args.key.unwrap_or(0),
        continue_on_error: args.continue_on_error,
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

fn read_patterns(values: &[String]) -> Result<Vec<String>> {
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
            return Ok(patterns);
        }
    }
    if values.iter().any(|value| value.trim().is_empty()) {
        anyhow::bail!("an empty regex is invalid; omit the filter to select all assets");
    }
    Ok(values.to_vec())
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
    let base = name.split('.').next().unwrap_or(name).to_uppercase();
    if [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&base.as_str())
    {
        anyhow::bail!("--map_name is a reserved device name");
    }
    let extension = match format {
        Map::Json => "json",
        Map::Xml => "xml",
    };
    let suffix = format!(".{extension}");
    if name.to_lowercase().ends_with(&suffix) {
        Ok(name.to_owned())
    } else {
        Ok(format!("{name}{suffix}"))
    }
}

fn map_format(value: Map) -> MapFormat {
    match value {
        Map::Json => MapFormat::Json,
        Map::Xml => MapFormat::Xml,
    }
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
            "../map", "..\\map", "C:map", "map\n", "NUL", "CON.json", "map.", "map ",
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
