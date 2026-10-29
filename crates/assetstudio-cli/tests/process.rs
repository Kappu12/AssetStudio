//! Process tests encode a small real SerializedFile layout (format 22,
//! little-endian, two TextAssets) rather than mocking the Rust backend.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_AssetStudioCLI"))
        .args(args)
        .output()
        .expect("launch AssetStudioCLI")
}

fn fixture(root: &Path) -> PathBuf {
    let path = root.join("sample.assets");
    fs::write(&path, serialized_text_assets()).unwrap();
    path
}

fn string(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn descendants(path: &Path) -> Vec<PathBuf> {
    if !path.is_dir() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap().path();
        if entry.is_dir() {
            found.extend(descendants(&entry));
        } else {
            found.push(entry);
        }
    }
    found.sort();
    found
}

#[test]
fn help_version_and_usage_errors_have_expected_exit_status() {
    assert!(run(&["--help"]).status.success());
    assert!(run(&["-?"]).status.success());
    let version = run(&["--version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
    assert_eq!(run(&[]).status.code(), Some(2));
    assert_eq!(run(&["missing", "--unknown-option"]).status.code(), Some(2));
    assert_eq!(
        run(&["missing", "out", "--key", "256"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&["missing", "--operation", "export"]).status.code(),
        Some(2)
    );
}

#[test]
fn list_reads_both_assets_and_keeps_stdout_valid_json() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let output = run(&[string(&input), "--operation", "list", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let list: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["name"], "hello");
    assert_eq!(list[0]["class_id"], 49);
    assert_eq!(list[0]["path_id"], 7);
    assert_eq!(list[1]["name"], "other");
}

#[cfg(unix)]
#[test]
fn inspect_json_handles_non_utf8_input_paths_without_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = TempDir::new().unwrap();
    let input = root
        .path()
        .join(OsString::from_vec(b"sample-\xff.assets".to_vec()));
    fs::write(&input, serialized_text_assets()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_AssetStudioCLI"))
        .arg(&input)
        .args(["--operation", "inspect", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["assets"], 2);
    assert!(value["input"].as_str().unwrap().contains('\u{fffd}'));
}

#[test]
fn legacy_aliases_export_only_the_name_selected_text_payload() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let output_dir = root.path().join("export");
    let output = run(&[
        string(&input),
        string(&output_dir),
        "--game",
        "Normal",
        "--type",
        "TextAsset",
        "--filter",
        "^HELLO$",
        "--group_assets_type",
        "None",
        "--export_type",
        "Convert",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files = descendants(&output_dir);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(fs::read(&files[0]).unwrap(), b"first payload");
}

#[test]
fn filter_file_and_parse_only_types_do_not_accidentally_export_everything() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let filters = root.path().join("names.txt");
    fs::write(&filters, "\n^other$\n").unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--json",
        "--names",
        string(&filters),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(values.as_array().unwrap().len(), 1);
    assert_eq!(values[0]["name"], "other");
    let destination = root.path().join("parse-only");
    let output = run(&[
        string(&input),
        string(&destination),
        "--types",
        "TextAsset:Parse",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(descendants(&destination).is_empty());

    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--json",
        "--types",
        "TextAsset:Both",
        "49:Parse",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(values.as_array().unwrap().is_empty());
}

#[test]
fn export_and_map_work_together_and_xml_maps_escape_names() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let destination = root.path().join("export");
    let output = run(&[
        string(&input),
        string(&destination),
        "--write-map",
        "--map_type",
        "JSON",
        "--map_name",
        "selected",
        "--names",
        "hello",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let map = destination.join("selected.json");
    assert!(map.is_file());
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&map).unwrap()).unwrap();
    assert!(value.to_string().contains("hello"));
    assert!(!value.to_string().contains("other"));
    assert_eq!(descendants(&destination).len(), 2);

    fs::write(&input, serialized_text_assets_named("A&B<quote>", "other")).unwrap();
    let map_destination = root.path().join("maps");
    let output = run(&[
        string(&input),
        string(&map_destination),
        "--map_op",
        "AssetMap",
        "--map_type",
        "XML",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let xml = fs::read_to_string(map_destination.join("assets_map.xml")).unwrap();
    assert!(xml.contains("A&amp;B&lt;quote&gt;"));
    assert!(!xml.contains("A&B<quote>"));
    assert!(xml.contains("other"));
    assert_eq!(descendants(&map_destination).len(), 1);
}

#[test]
fn asset_cache_formats_and_legacy_load_keep_exact_pathid_class_selection() {
    use assetstudio_core::asset_map::{AssetMap, AssetMapFormat};

    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    for (format, extension) in [("JSON", "json"), ("XML", "xml"), ("MessagePack", "map")] {
        let maps = root.path().join(format!("maps-{extension}"));
        let output = run(&[
            string(&input),
            string(&maps),
            "--map_op",
            "AssetMap",
            "--map_type",
            format,
            "--path-ids",
            "8",
            "--types",
            "TextAsset",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cache = maps.join(format!("assets_map.{extension}"));
        let parsed = AssetMap::from_path(&cache, AssetMapFormat::Auto).unwrap();
        assert_eq!(parsed.entries().len(), 1);
        assert_eq!(parsed.entries()[0].path_id, 8);
        assert_eq!(parsed.entries()[0].class_id, 49);

        let destination = root.path().join(format!("export-{extension}"));
        let output = run(&[
            string(&input),
            string(&destination),
            "--map_op",
            "AssetMap,Load",
            "--map_name",
            string(&cache),
            "--map_type",
            format,
            "--group_assets",
            "None",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let files = descendants(&destination);
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(fs::read(&files[0]).unwrap(), b"second payload");

        let output = run(&[
            string(&input),
            "--operation",
            "list",
            "--json",
            "--asset-map",
            string(&cache),
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["path_id"], 8);
        assert_eq!(listed[0]["name"], "other");
    }
}

#[test]
fn empty_cached_filters_do_not_load_all_sources_or_create_outputs() {
    let root = TempDir::new().unwrap();
    let cache = root.path().join("selection.json");
    // The source is deliberately missing: an empty selection must not open it.
    fs::write(&cache, br#"[{"Name":"cached","Container":"","Source":"missing.assets","PathID":8,"Type":"TextAsset"}]"#).unwrap();
    let input = root.path().join("missing-input");
    let destination = root.path().join("output");
    for filters in [
        vec!["--path-ids", "-9223372036854775808"],
        vec!["--path-ids", "8", "--names", "missing-name"],
        vec!["--path-ids", "8", "--types", "GameObject"],
    ] {
        let mut arguments = vec![
            string(&input),
            string(&destination),
            "--asset-map",
            string(&cache),
        ];
        arguments.extend(filters.clone());
        let output = run(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!destination.exists());

        let mut arguments = vec![
            string(&input),
            "--operation",
            "list",
            "--json",
            "--asset-map",
            string(&cache),
        ];
        arguments.extend(filters);
        let output = run(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(listed, serde_json::json!([]));
    }

    fs::write(&cache, b"[]").unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "inspect",
        "--json",
        "--asset-map",
        string(&cache),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inspected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspected["assets"], 0);
}

#[test]
fn invalid_cached_identities_and_outside_sources_fail_without_outputs() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let outside = TempDir::new().unwrap();
    let other = fixture(outside.path());
    let cache = root.path().join("selection.json");
    let destination = root.path().join("output");
    for (source, id, kind) in [
        (&input, 999999, "TextAsset"),
        (&input, 7, "GameObject"),
        (&other, 7, "TextAsset"),
    ] {
        fs::write(
            &cache,
            serde_json::to_vec(&serde_json::json!([{
                "Name":"cached", "Container":"", "Source":string(source), "PathID":id, "Type":kind,
            }]))
            .unwrap(),
        )
        .unwrap();
        let output = run(&[
            string(&input),
            string(&destination),
            "--asset-map",
            string(&cache),
        ]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!destination.exists());
    }
    let output = run(&[
        string(&input),
        "--operation",
        "scene",
        "--asset-map",
        string(&cache),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("scene listing does not apply asset filters")
    );
}

#[test]
fn overwrite_protects_regex_filter_inputs_for_asset_and_cab_exports() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    for filter in ["--names", "--containers"] {
        let destination = root.path().join(filter.trim_start_matches('-'));
        fs::create_dir(&destination).unwrap();
        let patterns = destination.join("hello_0_7.txt");
        let original = if filter == "--names" {
            b"^hello$\n".as_slice()
        } else {
            b".*\n".as_slice()
        };
        fs::write(&patterns, original).unwrap();
        let output = run(&[
            string(&input),
            string(&destination),
            "--group_assets",
            "None",
            "--overwrite",
            filter,
            string(&patterns),
        ]);
        assert!(
            !output.status.success(),
            "{filter} collision was successful"
        );
        assert_eq!(fs::read(&patterns).unwrap(), original);
    }
    let destination = root.path().join("cab");
    fs::create_dir(&destination).unwrap();
    let patterns = destination.join("assets_map.bin");
    fs::write(&patterns, b"^hello$\n").unwrap();
    let output = run(&[
        string(&input),
        string(&destination),
        "--map_op",
        "CABMap",
        "--overwrite",
        "--names",
        string(&patterns),
    ]);
    assert!(!output.status.success());
    assert_eq!(fs::read(&patterns).unwrap(), b"^hello$\n");
}

#[test]
fn unsupported_and_invalid_options_fail_before_creating_outputs() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    for (flag, value) in [
        ("--key_index", "0"),
        ("--map_op", "UnknownOperation"),
        ("--names", "["),
        ("--map_name", "../escape"),
        ("--types", "TextAsset:Broken"),
        ("--game", "MissingGameProfile"),
        ("--unity-cn-key", "not-a-key"),
    ] {
        let destination = root.path().join("must-not-exist");
        let output = run(&[string(&input), string(&destination), flag, value]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{flag} {value}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!destination.exists(), "{flag} caused output creation");
    }
}

#[test]
fn asset_index_recovery_runs_before_selection_and_rejects_invalid_indexes() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let index = root.path().join("asset_index.json");
    fs::write(&index, b"{}").unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--json",
        "--ai-file",
        string(&index),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let list: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Recovered container names"));

    fs::write(&index, b"not JSON").unwrap();
    let destination = root.path().join("must-not-exist");
    let output = run(&[
        string(&input),
        string(&destination),
        "--ai_file",
        string(&index),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!destination.exists());
}

#[test]
fn missing_inputs_and_unsupported_json_have_nonzero_exit_status() {
    assert_eq!(
        run(&["does-not-exist.assets", "--operation", "list"])
            .status
            .code(),
        Some(1)
    );
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let destination = root.path().join("json");
    let output = run(&[
        string(&input),
        string(&destination),
        "--export_type",
        "JSON",
    ]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(descendants(&destination).is_empty());
}

#[test]
fn repeated_export_preserves_existing_files_without_overwrite() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let destination = root.path().join("export");
    let first = run(&[
        string(&input),
        string(&destination),
        "--group_assets",
        "None",
        "--names",
        "hello",
    ]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let files = descendants(&destination);
    assert_eq!(files.len(), 1);
    fs::write(&files[0], b"user content").unwrap();
    let second = run(&[
        string(&input),
        string(&destination),
        "--group_assets",
        "None",
        "--names",
        "hello",
    ]);
    assert!([Some(0), Some(3)].contains(&second.status.code()));
    assert_eq!(fs::read(&files[0]).unwrap(), b"user content");
    assert_eq!(descendants(&destination), files);
    let third = run(&[
        string(&input),
        string(&destination),
        "--group_assets",
        "None",
        "--names",
        "hello",
        "--overwrite",
    ]);
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    assert_eq!(fs::read(&files[0]).unwrap(), b"first payload");
}

#[test]
fn empty_filtered_model_selection_never_exports_the_whole_scene() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let destination = root.path().join("model");
    for filters in [
        vec!["--names", "missing-name"],
        vec!["--path-ids", "999999"],
        vec!["--path-ids", "999999", "--names", "hello"],
        vec!["--path-ids", "999999", "--types", "GameObject", "Animator"],
        vec!["--path-ids", "7", "--types", "TextAsset"],
    ] {
        let mut arguments = vec![
            string(&input),
            string(&destination),
            "--operation",
            "model",
            "--no-animations",
            "--no-textures",
        ];
        arguments.extend(filters);
        let output = run(&arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("no GameObject or Animator roots matched")
        );
        assert!(!destination.exists(), "{arguments:?}");
    }
}

#[test]
fn cab_cache_and_legacy_both_all_operations_use_real_dependencies() {
    use assetstudio_core::cab_map::CabMap;
    let root = TempDir::new().unwrap();
    let game = root.path().join("game");
    fs::create_dir(&game).unwrap();
    let first = game.join("a.assets");
    let second = game.join("b.assets");
    fs::write(
        &first,
        serialized_text_assets_with_externals("first", "other", &["b.assets"]),
    )
    .unwrap();
    fs::write(&second, serialized_text_assets_named("dependency", "other")).unwrap();

    let maps = root.path().join("maps");
    let output = run(&[
        string(&game),
        string(&maps),
        "--map_op",
        "Both",
        "--map_type",
        "JSON",
        "--map_name",
        "cache",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(descendants(&maps).len(), 2);
    let cache = maps.join("cache.bin");
    let parsed = CabMap::from_path(&cache).unwrap();
    assert_eq!(parsed.entries().len(), 2);
    assert_eq!(parsed.entries()[0].cab, "a.assets");
    assert_eq!(parsed.entries()[0].dependencies, ["b.assets"]);
    assert_eq!(parsed.entries()[0].offset, 0);

    let output = run(&[
        string(&first),
        "--operation",
        "list",
        "--json",
        "--cab-map",
        string(&cache),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 4);
    assert!(listed.to_string().contains("dependency"));

    let exports = root.path().join("export");
    let output = run(&[
        string(&first),
        string(&exports),
        "--map_op",
        "CABMap,Load",
        "--cab-map",
        string(&cache),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(descendants(&exports).len(), 4);

    let all = root.path().join("all");
    let output = run(&[
        string(&game),
        string(&all),
        "--map_op",
        "All",
        "--map_type",
        "JSON",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(descendants(&all).len(), 6);
    assert!(all.join("assets_map.bin").is_file());
    assert!(all.join("assets_map.json").is_file());
}

#[test]
fn cab_only_map_creation_preserves_existing_files_until_overwrite() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let destination = root.path().join("maps");
    let output = run(&[string(&input), string(&destination), "--map_op", "CABMap"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cache = destination.join("assets_map.bin");
    assert_eq!(
        descendants(&destination).as_slice(),
        std::slice::from_ref(&cache)
    );
    let original = fs::read(&cache).unwrap();
    fs::write(&cache, b"user content").unwrap();
    assert!(
        run(&[string(&input), string(&destination), "--map_op", "CABMap"])
            .status
            .success()
    );
    assert_eq!(fs::read(&cache).unwrap(), b"user content");
    let output = run(&[
        string(&input),
        string(&destination),
        "--map_op",
        "CABMap",
        "--overwrite",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&cache).unwrap(), original);
}

#[test]
fn nonzero_cab_offsets_are_visible_errors_rather_than_full_file_guesses() {
    use assetstudio_core::cab_map::{CabEntry, CabMap};
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let map = CabMap::new(
        root.path().to_str().unwrap().into(),
        vec![CabEntry {
            cab: "sample.assets".into(),
            path: "sample.assets".into(),
            offset: 4096,
            dependencies: Vec::new(),
        }],
    )
    .unwrap();
    let cache = root.path().join("specialized.bin");
    map.write_path(&cache, false).unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--cab-map",
        string(&cache),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("specialized source offset"));
}

#[test]
fn cab_overwrite_cannot_replace_the_original_input_file() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("assets_map.bin");
    let original = serialized_text_assets();
    fs::write(&source, &original).unwrap();
    let output = run(&[
        string(&source),
        string(root.path()),
        "--map_op",
        "CABMap",
        "--overwrite",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn trusted_schema_files_and_internal_generator_have_real_cli_paths() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let schema = root.path().join("schemas.json");
    fs::write(&schema, br#"{"version":1,"entries":[]}"#).unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--json",
        "--mono-schema",
        string(&schema),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
    let assemblies = root.path().join("assemblies");
    fs::create_dir(&assemblies).unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--dummy_dlls",
        string(&assemblies),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = root.path().join("request.json");
    fs::write(
        &request,
        serde_json::to_vec(&serde_json::json!({
            "version":1, "dll_directory":assemblies, "targets":[],
        }))
        .unwrap(),
    )
    .unwrap();
    let output = run(&["--internal-schema-generator", string(&request)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["version"], 1);
    assert_eq!(document["entries"], serde_json::json!([]));
    assert_eq!(run(&["--internal-schema-generator"]).status.code(), Some(2));

    fs::write(&schema, b"not JSON").unwrap();
    let destination = root.path().join("must-not-exist");
    let output = run(&[
        string(&input),
        string(&destination),
        "--mono-schema",
        string(&schema),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!destination.exists());
}

#[test]
fn unitycn_key_indexes_validate_boundaries_without_echoing_material() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let keys = root.path().join("custom-keys.json");
    let material = "0123456789abcdef0123456789abcdef";
    fs::write(
        &keys,
        serde_json::to_vec(&serde_json::json!([
            {"Name":"first", "Key":"00000000000000000000000000000000"},
            {"Name":"second", "Key":material},
        ]))
        .unwrap(),
    )
    .unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--json",
        "--game",
        "UnityCN",
        "--keys-file",
        string(&keys),
        "--key_index",
        "1",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains(material));
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--game",
        "UnityCN",
        "--keys-file",
        string(&keys),
        "--key_index",
        "2",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(material));
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--game",
        "UnityCN",
        "--keys-file",
        string(&keys),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let invalid_material = "DO_NOT_PRINT_THIS_KEY";
    fs::write(
        &keys,
        serde_json::to_vec(&serde_json::json!([{"Name":"bad", "Key":invalid_material}])).unwrap(),
    )
    .unwrap();
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--game",
        "UnityCN",
        "--keys-file",
        string(&keys),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(invalid_material));
}

#[test]
fn profile_advisories_keep_successful_lists_successful_and_real_failures_partial() {
    let root = TempDir::new().unwrap();
    let input = fixture(root.path());
    let output = run(&[
        string(&input),
        "--operation",
        "list",
        "--json",
        "--game",
        "FakeHeader",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("verified"));

    fs::write(root.path().join("broken.bundle"), b"not a valid wrapper").unwrap();
    let output = run(&[
        string(root.path()),
        "--operation",
        "list",
        "--json",
        "--game",
        "FakeHeader",
        "--continue-on-error",
    ]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

fn serialized_text_assets() -> Vec<u8> {
    serialized_text_assets_named("hello", "other")
}

fn serialized_text_assets_named(first: &str, second: &str) -> Vec<u8> {
    serialized_text_assets_with_externals(first, second, &[])
}

fn serialized_text_assets_with_externals(first: &str, second: &str, externals: &[&str]) -> Vec<u8> {
    let mut objects = Vec::new();
    let mut table = Vec::new();
    for (path_id, name, payload) in [
        (7_i64, first, b"first payload".as_slice()),
        (8_i64, second, b"second payload".as_slice()),
    ] {
        align(&mut objects, 0, 4);
        let offset = objects.len() as i64;
        let mut object = Vec::new();
        push_i32(&mut object, name.len() as i32);
        object.extend_from_slice(name.as_bytes());
        align(&mut object, 0, 4);
        push_i32(&mut object, payload.len() as i32);
        object.extend_from_slice(payload);
        let size = object.len() as u32;
        objects.extend_from_slice(&object);
        table.push((path_id, offset, size));
    }
    let mut metadata = b"2022.3.62f1\0".to_vec();
    push_i32(&mut metadata, 13); // Android; no editor-only object prefix.
    metadata.push(0); // Embedded TypeTrees omitted, as in release game builds.
    push_i32(&mut metadata, 1); // Serialized types.
    push_i32(&mut metadata, 49); // TextAsset.
    metadata.push(0); // Not stripped.
    metadata.extend_from_slice(&(-1_i16).to_le_bytes());
    metadata.extend_from_slice(&[0; 16]); // Type hash.
    push_i32(&mut metadata, table.len() as i32);
    for (path_id, offset, size) in table {
        align(&mut metadata, 48, 4);
        metadata.extend_from_slice(&path_id.to_le_bytes());
        metadata.extend_from_slice(&offset.to_le_bytes());
        metadata.extend_from_slice(&size.to_le_bytes());
        push_i32(&mut metadata, 0); // Type index.
    }
    push_i32(&mut metadata, 0); // Script identifiers.
    push_i32(&mut metadata, externals.len() as i32);
    for external in externals {
        metadata.push(0); // External path prefix.
        metadata.extend_from_slice(&[0; 16]);
        push_i32(&mut metadata, 0);
        metadata.extend_from_slice(external.as_bytes());
        metadata.push(0);
    }
    push_i32(&mut metadata, 0); // Reference types.
    metadata.push(0); // User information.
    let data_offset = (48 + metadata.len()).div_ceil(16) * 16;
    let file_size = data_offset + objects.len();
    let mut file = vec![0; 48];
    file[8..12].copy_from_slice(&22_u32.to_be_bytes());
    file[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    file[24..32].copy_from_slice(&(file_size as i64).to_be_bytes());
    file[32..40].copy_from_slice(&(data_offset as i64).to_be_bytes());
    file.extend_from_slice(&metadata);
    file.resize(data_offset, 0);
    file.extend_from_slice(&objects);
    file
}

fn push_i32(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn align(output: &mut Vec<u8>, base: usize, alignment: usize) {
    while !(base + output.len()).is_multiple_of(alignment) {
        output.push(0);
    }
}
