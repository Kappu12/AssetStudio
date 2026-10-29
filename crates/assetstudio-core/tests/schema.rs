//! External-schema acceptance uses a real stripped Unity v22 object table.
//! The JSON describes its complete MonoBehaviour body, not executable code.

use std::fs;

use assetstudio_core::{
    ExportMode, ExportOptions, LoadOptions, Preview, Session,
    schema::{
        SchemaLimits, extend_registry, extend_registry_with_limits, load_registries,
        load_registries_with_limits, load_registry,
    },
};
use serde_json::json;
use unity_rs_core::{
    mono_schema::{MonoBehaviourSchemaIdentity, MonoBehaviourSchemaProvider},
    source::Region,
};

fn simple_document(class: &str, field: &str, version: Option<&str>) -> Vec<u8> {
    let mut entry = json!({
        "assembly": "Assembly-CSharp.dll", "namespace": "Game", "class": class,
        "nodes": [{"level":0,"type":"MonoBehaviour","name":"Base"},
                  {"level":1,"type":"int","name":field}]
    });
    if let Some(version) = version {
        entry["unity_version"] = json!(version);
    }
    serde_json::to_vec(&json!({"version":1,"entries":[entry]})).unwrap()
}

fn identity(version: &str) -> MonoBehaviourSchemaIdentity<'_> {
    MonoBehaviourSchemaIdentity {
        unity_version: version,
        assembly_name: "some/path/assembly-csharp.DLL",
        namespace: "Game",
        class_name: "Stats",
    }
}

#[test]
fn files_preserve_first_priority_and_exact_versions_outrank_fallbacks() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("fallback.json");
    let second = directory.path().join("later.json");
    let exact = directory.path().join("exact.json");
    fs::write(&first, simple_document("Stats", "first", None)).unwrap();
    fs::write(&second, simple_document("Stats", "second", None)).unwrap();
    fs::write(
        &exact,
        simple_document("Stats", "exact", Some("2022.3.62f1")),
    )
    .unwrap();
    let registry = load_registries(&[first, second, exact]).unwrap();
    assert_eq!(registry.entries().len(), 3);
    assert!(!registry.overrides_embedded_tree());
    assert_eq!(
        registry
            .schema(identity("2022.3.62f1"))
            .unwrap()
            .unwrap()
            .nodes[1]
            .field_name,
        "exact"
    );
    assert_eq!(
        registry
            .schema(identity("6000.0.0f1"))
            .unwrap()
            .unwrap()
            .nodes[1]
            .field_name,
        "first"
    );
    let mut missing = identity("2022.3.62f1");
    missing.class_name = "Other";
    assert!(registry.schema(missing).unwrap().is_none());
}

#[test]
fn limits_cover_files_encoded_bytes_and_cumulative_entries() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema.json");
    let bytes = simple_document("Stats", "count", None);
    fs::write(&path, &bytes).unwrap();
    let paths = [path.clone(), path.clone()];
    let mut limits = SchemaLimits {
        maximum_files: 1,
        ..SchemaLimits::default()
    };
    assert!(load_registries_with_limits(&paths, limits).is_err());
    limits = SchemaLimits::default();
    limits.document.maximum_document_bytes = bytes.len() - 1;
    assert!(load_registries_with_limits(&paths[..1], limits).is_err());
    limits = SchemaLimits::default();
    limits.maximum_total_document_bytes = bytes.len();
    assert!(load_registries_with_limits(&paths, limits).is_err());
    limits = SchemaLimits::default();
    limits.document.maximum_entries = 1;
    assert!(load_registries_with_limits(&paths, limits).is_err());
    assert_eq!(load_registry(&path).unwrap().entries().len(), 1);
}

#[test]
fn structural_and_decoded_string_budgets_are_enforced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema.json");
    fs::write(&path, simple_document("Stats", "count", None)).unwrap();
    let paths = [path];
    let mut limits = SchemaLimits::default();
    limits.document.maximum_nodes_per_entry = 1;
    assert!(load_registries_with_limits(&paths, limits).is_err());
    limits = SchemaLimits::default();
    limits.document.maximum_total_nodes = 1;
    assert!(load_registries_with_limits(&paths, limits).is_err());
    limits = SchemaLimits::default();
    limits.document.maximum_string_bytes = 3;
    assert!(load_registries_with_limits(&paths, limits).is_err());
    limits = SchemaLimits::default();
    limits.document.maximum_total_string_bytes = 10;
    assert!(load_registries_with_limits(&paths, limits).is_err());
}

#[test]
fn incremental_loads_are_transactional_and_share_retained_limits() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.json");
    let second = directory.path().join("second.json");
    let bad = directory.path().join("bad.json");
    fs::write(&first, simple_document("Stats", "one", None)).unwrap();
    fs::write(&second, simple_document("Other", "two", None)).unwrap();
    fs::write(&bad, b"not JSON").unwrap();
    let mut registry = load_registry(&first).unwrap();
    let original = registry.clone();
    assert!(extend_registry(&mut registry, &bad).is_err());
    assert_eq!(registry, original);
    let mut limits = SchemaLimits::default();
    limits.document.maximum_entries = 1;
    assert!(extend_registry_with_limits(&mut registry, &second, limits).is_err());
    assert_eq!(registry, original);
    assert_eq!(extend_registry(&mut registry, &second).unwrap(), 1);
    assert_eq!(registry.entries().len(), 2);
}

#[test]
fn malformed_documents_and_nonregular_inputs_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    assert!(load_registry(directory.path()).is_err());
    assert!(load_registry(&directory.path().join("missing.json")).is_err());
    let path = directory.path().join("schema.json");
    for bytes in [
        br#"{"version":2,"entries":[]}"#.as_slice(),
        br#"{"version":1,"entries":[{"assembly":"A","class":"C","nodes":[]}]}"#.as_slice(),
        br#"{"version":1,"entries":[{"assembly":"A","class":"C","nodes":[{"level":1,"type":"int","name":"x"}]}]}"#.as_slice(),
        br#"{"version":1,"entries":[]} trailing"#.as_slice(),
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(load_registry(&path).is_err());
    }
    assert!(load_registries(&[]).unwrap().entries().is_empty());
}

#[cfg(unix)]
#[test]
fn schema_file_selection_rejects_symlinks_and_directories() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real.json");
    let linked = directory.path().join("link.json");
    fs::write(&real, simple_document("Stats", "count", None)).unwrap();
    symlink(&real, &linked).unwrap();
    assert!(load_registry(&linked).is_err());
    // A directory is another nonregular input, checked before File::open.
    assert!(load_registry(directory.path()).is_err());
}

const COMPLETE_SCHEMA: &str = r#"{
 "version":1,"entries":[{"assembly":"Assembly-CSharp.dll","namespace":"Game","class":"Stats",
 "unity_version":"2022.3.62f1","nodes":[
 {"level":0,"type":"MonoBehaviour","name":"Base"},
 {"level":1,"type":"PPtr<GameObject>","name":"m_GameObject"},
 {"level":2,"type":"int","name":"m_FileID"},
 {"level":2,"type":"SInt64","name":"m_PathID"},
 {"level":1,"type":"UInt8","name":"m_Enabled","meta_flags":16384},
 {"level":1,"type":"PPtr<MonoScript>","name":"m_Script"},
 {"level":2,"type":"int","name":"m_FileID"},
 {"level":2,"type":"SInt64","name":"m_PathID"},
 {"level":1,"type":"string","name":"m_Name"},
 {"level":2,"type":"Array","name":"Array"},
 {"level":3,"type":"int","name":"size"},
 {"level":3,"type":"char","name":"data"},
 {"level":1,"type":"SInt32","name":"score"}
 ]}]}"#;

#[test]
fn stripped_mono_behaviour_previews_and_exports_with_external_schema() {
    let fixture = mono_fixture();
    let mut session = Session::from_regions(
        [("CAB-schema".to_owned(), Region::from_bytes(fixture))],
        &LoadOptions::default(),
    )
    .unwrap();
    let key = session
        .assets()
        .iter()
        .find(|asset| asset.class_id == 114)
        .unwrap()
        .key;
    let output = tempfile::tempdir().unwrap();
    let options = ExportOptions {
        mode: ExportMode::Json,
        ..ExportOptions::default()
    };
    let before = session.export(&[key], output.path(), &options).unwrap();
    assert!(before.written.is_empty());
    assert_eq!(before.failed.len(), 1);

    let schema = output.path().join("schema.json");
    fs::write(&schema, COMPLETE_SCHEMA).unwrap();
    assert_eq!(session.load_schemas(&schema).unwrap(), 1);
    let Preview::Text(text) = session.preview(key).unwrap() else {
        panic!("expected JSON preview")
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["m_Name"], "Hero");
    assert_eq!(value["score"], 123);
    for mode in [ExportMode::Json, ExportMode::Convert, ExportMode::Dump] {
        let directory = output.path().join(format!("{mode:?}"));
        let report = session
            .export(&[key], &directory, &ExportOptions { mode, ..options })
            .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.written.len(), 1);
        let bytes = fs::read(&report.written[0]).unwrap();
        if mode == ExportMode::Dump {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("SInt32 score = 123"), "{text}");
            assert!(text.contains("m_Name") && text.contains("Hero"), "{text}");
        } else {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["score"], 123);
        }
    }
}

#[cfg(unix)]
#[test]
fn native_helper_cannot_report_success_while_omitting_requested_types() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("empty-schema-helper");
    fs::write(
        &helper,
        b"#!/bin/sh\nprintf '%s' '{\"version\":1,\"entries\":[]}'\n",
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let mut session = Session::from_regions(
        [("CAB-schema".into(), Region::from_bytes(mono_fixture()))],
        &LoadOptions::default(),
    )
    .unwrap();
    let error = session
        .load_dummy_dlls(directory.path(), &helper)
        .unwrap_err();
    assert!(error.to_string().contains("omitted requested type"));
}

fn align(bytes: &mut Vec<u8>, boundary: usize) {
    while !bytes.len().is_multiple_of(boundary) {
        bytes.push(0);
    }
}

fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as i32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    align(bytes, 4);
}

fn pointer(bytes: &mut Vec<u8>, path_id: i64) {
    bytes.extend_from_slice(&0_i32.to_le_bytes());
    bytes.extend_from_slice(&path_id.to_le_bytes());
}

fn mono_fixture() -> Vec<u8> {
    let mut behaviour = Vec::new();
    pointer(&mut behaviour, 1);
    behaviour.push(1);
    align(&mut behaviour, 4);
    pointer(&mut behaviour, 8);
    string(&mut behaviour, "Hero");
    behaviour.extend_from_slice(&123_i32.to_le_bytes());
    let mut script = Vec::new();
    string(&mut script, "Stats script");
    script.extend_from_slice(&0_i32.to_le_bytes());
    script.extend_from_slice(&[0; 16]);
    string(&mut script, "Stats");
    string(&mut script, "Game");
    string(&mut script, "Assembly-CSharp.dll");
    let objects = [(114_i32, 7_i64, behaviour), (115_i32, 8_i64, script)];
    let mut metadata = b"2022.3.62f1\0".to_vec();
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0); // Player file, type trees stripped.
    metadata.extend_from_slice(&2_i32.to_le_bytes());
    for class in [114_i32, 115_i32] {
        metadata.extend_from_slice(&class.to_le_bytes());
        metadata.push(0);
        metadata.extend_from_slice(&(-1_i16).to_le_bytes());
        if class == 114 {
            metadata.extend_from_slice(&[0; 16]);
        }
        metadata.extend_from_slice(&[0; 16]);
    }
    metadata.extend_from_slice(&2_i32.to_le_bytes());
    let mut data = Vec::new();
    for (index, (_, path_id, payload)) in objects.iter().enumerate() {
        align(&mut metadata, 4);
        align(&mut data, 4);
        metadata.extend_from_slice(&path_id.to_le_bytes());
        metadata.extend_from_slice(&(data.len() as i64).to_le_bytes());
        metadata.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        metadata.extend_from_slice(&(index as i32).to_le_bytes());
        data.extend_from_slice(payload);
    }
    metadata.extend_from_slice(&[0; 12]);
    metadata.push(0);
    let offset = (48 + metadata.len()).next_multiple_of(16);
    let size = offset + data.len();
    let mut header = vec![0; 48];
    header[8..12].copy_from_slice(&22_u32.to_be_bytes());
    header[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    header[24..32].copy_from_slice(&(size as i64).to_be_bytes());
    header[32..40].copy_from_slice(&(offset as i64).to_be_bytes());
    header.extend_from_slice(&metadata);
    header.resize(offset, 0);
    header.extend_from_slice(&data);
    header
}
