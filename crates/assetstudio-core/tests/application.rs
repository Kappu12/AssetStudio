//! Application acceptance checks use small Unity v22 records encoded from the
//! documented header/object-table layout, never a mock Session.
use std::{fs, path::Path};

use assetstudio_core::{
    AssetKey, ExportMode, ExportOptions, GroupMode, LoadOptions, MapFormat, Preview, Session,
};
use unity_rs_core::source::Region;

fn align(bytes: &mut Vec<u8>, multiple: usize) {
    while !bytes.len().is_multiple_of(multiple) {
        bytes.push(0);
    }
}
fn string(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as i32).to_le_bytes());
    bytes.extend_from_slice(value);
    align(bytes, 4);
}
fn file(objects: &[(i32, i64, Vec<u8>)]) -> Vec<u8> {
    let mut classes = Vec::new();
    for (class, _, _) in objects {
        if !classes.contains(class) {
            classes.push(*class);
        }
    }
    let mut metadata = b"6000.0.0f1\0".to_vec();
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&(classes.len() as i32).to_le_bytes());
    for class in &classes {
        metadata.extend_from_slice(&class.to_le_bytes());
        metadata.push(0);
        metadata.extend_from_slice(&(-1_i16).to_le_bytes());
        if *class == 114 {
            metadata.extend_from_slice(&[0; 16]);
        }
        metadata.extend_from_slice(&[0; 16]);
    }
    metadata.extend_from_slice(&(objects.len() as i32).to_le_bytes());
    let mut data = Vec::new();
    for (class, path_id, payload) in objects {
        align(&mut data, 4);
        while !(48 + metadata.len()).is_multiple_of(4) {
            metadata.push(0);
        }
        metadata.extend_from_slice(&path_id.to_le_bytes());
        metadata.extend_from_slice(&(data.len() as i64).to_le_bytes());
        metadata.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        metadata.extend_from_slice(
            &(classes.iter().position(|value| value == class).unwrap() as i32).to_le_bytes(),
        );
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
fn text(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    string(&mut bytes, name.as_bytes());
    string(&mut bytes, payload);
    bytes
}
fn session(objects: &[(i32, i64, Vec<u8>)]) -> Session {
    Session::from_regions(
        [("CAB-test".into(), Region::from_bytes(file(objects)))],
        &LoadOptions::default(),
    )
    .unwrap()
}

#[test]
fn unity6_text_roundtrip_raw_and_converted_and_selection() {
    let first = text("first", b"hello\0binary\xff");
    let second = text("second", b"other");
    let session = session(&[(49, 7, first.clone()), (49, 9, second)]);
    assert_eq!(session.assets().len(), 2);
    assert_eq!(session.assets()[0].name, "first");
    let output = tempfile::tempdir().unwrap();
    let converted = session
        .export(
            &[AssetKey {
                file_index: 0,
                path_id: 7,
            }],
            output.path(),
            &ExportOptions::default(),
        )
        .unwrap();
    assert!(converted.failed.is_empty(), "{:?}", converted.failed);
    assert_eq!(converted.written.len(), 1);
    assert_eq!(
        fs::read(&converted.written[0]).unwrap(),
        b"hello\0binary\xff"
    );
    let raw = session
        .export(
            &[session.assets()[0].key],
            output.path(),
            &ExportOptions {
                mode: ExportMode::Raw,
                ..ExportOptions::default()
            },
        )
        .unwrap();
    assert_eq!(fs::read(&raw.written[0]).unwrap(), first);
    let empty = session
        .export(&[], output.path(), &ExportOptions::default())
        .unwrap();
    assert!(empty.written.is_empty());
    let repeat = session
        .export(
            &[session.assets()[0].key],
            output.path(),
            &ExportOptions::default(),
        )
        .unwrap();
    assert_eq!(repeat.skipped.len(), 1);
    assert!(
        session
            .export(
                &[AssetKey {
                    file_index: 999,
                    path_id: 7
                }],
                output.path(),
                &ExportOptions::default()
            )
            .is_err()
    );
}

#[test]
fn map_escaping_atomic_no_overwrite_and_no_fake_json_success() {
    let session = session(&[(49, 7, text("name<&\"'", b"payload"))]);
    let output = tempfile::tempdir().unwrap();
    let key = session.assets()[0].key;
    let path = output.path().join("assets.xml");
    let report = session.write_map(&[key], &path, MapFormat::Xml).unwrap();
    assert_eq!(report.written.len(), 1);
    let xml = fs::read_to_string(&path).unwrap();
    assert!(xml.contains("name&lt;&amp;&quot;&apos;"));
    let report = session.write_map(&[key], &path, MapFormat::Json).unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), xml);
    let structured = session
        .export(
            &[key],
            output.path(),
            &ExportOptions {
                mode: ExportMode::Json,
                ..ExportOptions::default()
            },
        )
        .unwrap();
    assert_eq!(structured.failed.len(), 1);
    assert!(structured.written.is_empty());
}

#[test]
fn path_like_names_and_collisions_cannot_escape_output() {
    let session = session(&[
        (49, 7, text("../../CON", b"first")),
        (49, 8, text("../../CON", b"second")),
    ]);
    let output = tempfile::tempdir().unwrap();
    let keys = session
        .assets()
        .iter()
        .map(|entry| entry.key)
        .collect::<Vec<_>>();
    let report = session
        .export(
            &keys,
            output.path(),
            &ExportOptions {
                group: GroupMode::None,
                ..ExportOptions::default()
            },
        )
        .unwrap();
    assert!(report.failed.is_empty());
    assert_eq!(report.written.len(), 2);
    assert_ne!(report.written[0], report.written[1]);
    for path in report.written {
        assert_eq!(
            path.parent(),
            Some(std::fs::canonicalize(output.path()).unwrap().as_path())
        );
    }
}

#[test]
fn bin_data_xor_has_exact_bounds_and_preserves_payload() {
    let data = b"{\"hello\":true}";
    let mut payload = Vec::new();
    payload.extend_from_slice(&(data.len() as i32).to_le_bytes());
    payload.extend(data.iter().map(|byte| byte ^ 0x45));
    let options = LoadOptions {
        xor_key: 0x45,
        ..LoadOptions::default()
    };
    let session = Session::from_regions(
        [(
            "CAB-test".into(),
            Region::from_bytes(file(&[(1208, 7, payload)])),
        )],
        &options,
    )
    .unwrap();
    assert_eq!(
        session.preview(session.assets()[0].key).unwrap(),
        Preview::Text(String::from_utf8_lossy(data).into_owned())
    );
    let output = tempfile::tempdir().unwrap();
    let report = session
        .export(
            &[session.assets()[0].key],
            output.path(),
            &ExportOptions::default(),
        )
        .unwrap();
    assert!(report.failed.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&report.written[0]).unwrap())
            .unwrap(),
        serde_json::json!({"hello":true})
    );
    let bad = session_with_raw_bin(vec![255, 255, 255, 255]);
    assert!(bad.preview(bad.assets()[0].key).is_err());
}
fn session_with_raw_bin(payload: Vec<u8>) -> Session {
    session(&[(1208, 7, payload)])
}

#[test]
fn malformed_headers_do_not_panic_or_produce_fake_success() {
    let seed = file(&[(49, 7, text("hello", b"world"))]);
    for length in 0..48 {
        let bytes = seed[..length].to_vec();
        let result = std::panic::catch_unwind(move || {
            Session::from_regions(
                [("truncated".into(), Region::from_bytes(bytes))],
                &LoadOptions::default(),
            )
        });
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
    for index in 0..48 {
        let mut bytes = seed.clone();
        bytes[index] ^= 0xff;
        assert!(
            std::panic::catch_unwind(move || Session::from_regions(
                [("mutated".into(), Region::from_bytes(bytes))],
                &LoadOptions::default()
            ))
            .is_ok()
        );
    }
    assert!(
        Session::open(
            Path::new("/this/file/does/not/exist"),
            &LoadOptions::default()
        )
        .is_err()
    );
}
