//! Native child-process acceptance: metadata comes from a real MIT test DLL,
//! while the asset payload is encoded independently from its known C# fields.
use std::{fs, path::Path};

use assetstudio_core::{ExportMode, ExportOptions, LoadOptions, Preview, Session};

#[test]
fn dummy_dll_child_recovers_stripped_data_and_exits_without_managed_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("fixture.assets");
    fs::write(&input, fixture()).unwrap();
    let mut session = Session::open(&input, &LoadOptions::default()).unwrap();
    let key = session
        .assets()
        .iter()
        .find(|asset| asset.class_id == 114)
        .unwrap()
        .key;
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/managed");
    let executable = Path::new(env!("CARGO_BIN_EXE_AssetStudioCLI"));
    assert_eq!(session.load_dummy_dlls(&folder, executable).unwrap(), 1);
    let Preview::Text(text) = session.preview(key).unwrap() else {
        panic!("expected structured preview")
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["m_Name"], "Native");
    assert_eq!(value["b"], 1);
    assert_eq!(value["i32"], -123456);
    assert_eq!(value["u32"], 3_000_000);
    assert_eq!(value["i64"], -10_000_000_000_i64);
    assert_eq!(value["u64"], 10_000_000_000_u64);
    assert_eq!(value["f32"], 3.5);
    assert_eq!(value["f64"], 6.25);
    assert_eq!(value["c"], 65);
    let options = ExportOptions {
        mode: ExportMode::Json,
        ..ExportOptions::default()
    };
    let report = session
        .export(&[key], &directory.path().join("export"), &options)
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.written.len(), 1);
    let output: serde_json::Value =
        serde_json::from_slice(&fs::read(&report.written[0]).unwrap()).unwrap();
    assert_eq!(output, value);
    // Matching registry entries make repeated requests a no-op, not a new
    // long-lived metadata cache or duplicate schema installation.
    assert_eq!(
        session
            .load_dummy_dlls(&folder, Path::new("does-not-need-to-run"))
            .unwrap(),
        0
    );
}

#[test]
fn child_failures_are_reported_instead_of_claiming_schema_success() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("fixture.assets");
    fs::write(&input, fixture()).unwrap();
    let mut session = Session::open(&input, &LoadOptions::default()).unwrap();
    assert!(
        session
            .load_dummy_dlls(
                directory.path(),
                Path::new(env!("CARGO_BIN_EXE_AssetStudioCLI"))
            )
            .is_err()
    );
    let key = session
        .assets()
        .iter()
        .find(|asset| asset.class_id == 114)
        .unwrap()
        .key;
    let report = session
        .export(
            &[key],
            &directory.path().join("export"),
            &ExportOptions {
                mode: ExportMode::Json,
                ..ExportOptions::default()
            },
        )
        .unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
}

#[test]
fn native_child_recovers_arrays_strings_references_and_float_structs() {
    let mut fields = Vec::new();
    fields.extend_from_slice(&2_i32.to_le_bytes());
    for value in [1_i32, -2] {
        fields.extend_from_slice(&value.to_le_bytes());
    }
    fields.extend_from_slice(&1_i32.to_le_bytes());
    fields.extend_from_slice(&3.5_f32.to_le_bytes());
    fields.extend_from_slice(&2_i32.to_le_bytes());
    string(&mut fields, "alpha");
    string(&mut fields, "b");
    fields.extend_from_slice(&1_i32.to_le_bytes());
    pointer(&mut fields, 0);
    fields.extend_from_slice(&2_i32.to_le_bytes());
    for value in [1_f32, 2., 3., 4., 5., 6.] {
        fields.extend_from_slice(&value.to_le_bytes());
    }
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("arrays.assets");
    fs::write(&input, fixture_for("Arrays", &fields)).unwrap();
    let mut session = Session::open(&input, &LoadOptions::default()).unwrap();
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/managed");
    session
        .load_dummy_dlls(&folder, Path::new(env!("CARGO_BIN_EXE_AssetStudioCLI")))
        .unwrap();
    let key = session
        .assets()
        .iter()
        .find(|asset| asset.class_id == 114)
        .unwrap()
        .key;
    let Preview::Text(text) = session.preview(key).unwrap() else {
        panic!("expected JSON")
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["ints"], serde_json::json!([1, -2]));
    assert_eq!(value["floats"], serde_json::json!([3.5]));
    assert_eq!(value["strings"], serde_json::json!(["alpha", "b"]));
    assert_eq!(value["gos"][0]["m_PathID"], 0);
    assert_eq!(
        value["verts"][1],
        serde_json::json!({"x":4.0,"y":5.0,"z":6.0})
    );
}

fn align(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}
fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as i32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    align(bytes);
}
fn pointer(bytes: &mut Vec<u8>, value: i64) {
    bytes.extend_from_slice(&0_i32.to_le_bytes());
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn fixture() -> Vec<u8> {
    let mut fields = Vec::new();
    for value in [1, 20, (-3_i8) as u8] {
        fields.push(value);
        align(&mut fields);
    }
    fields.extend_from_slice(&(-12_i16).to_le_bytes());
    align(&mut fields);
    fields.extend_from_slice(&65500_u16.to_le_bytes());
    align(&mut fields);
    fields.extend_from_slice(&(-123456_i32).to_le_bytes());
    fields.extend_from_slice(&3_000_000_u32.to_le_bytes());
    fields.extend_from_slice(&(-10_000_000_000_i64).to_le_bytes());
    fields.extend_from_slice(&10_000_000_000_u64.to_le_bytes());
    fields.extend_from_slice(&3.5_f32.to_le_bytes());
    fields.extend_from_slice(&6.25_f64.to_le_bytes());
    fields.extend_from_slice(&65_u16.to_le_bytes());
    align(&mut fields);
    fixture_for("Primitives", &fields)
}

fn fixture_for(class: &str, fields: &[u8]) -> Vec<u8> {
    let mut behaviour = Vec::new();
    pointer(&mut behaviour, 1);
    behaviour.push(1);
    align(&mut behaviour);
    pointer(&mut behaviour, 8);
    string(&mut behaviour, "Native");
    behaviour.extend_from_slice(fields);
    let mut script = Vec::new();
    string(&mut script, &format!("{class} script"));
    script.extend_from_slice(&0_i32.to_le_bytes());
    script.extend_from_slice(&[0; 16]);
    string(&mut script, class);
    string(&mut script, "Fixtures");
    string(&mut script, "Fixtures.dll");
    let objects = [(114_i32, 7_i64, behaviour), (115_i32, 8_i64, script)];
    let mut metadata = b"6000.0.0f1\0".to_vec();
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&2_i32.to_le_bytes());
    for class in [114_i32, 115] {
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
        align(&mut metadata);
        align(&mut data);
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
    let mut bytes = vec![0; 48];
    bytes[8..12].copy_from_slice(&22_u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    bytes[24..32].copy_from_slice(&(size as i64).to_be_bytes());
    bytes[32..40].copy_from_slice(&(offset as i64).to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.resize(offset, 0);
    bytes.extend_from_slice(&data);
    bytes
}
