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
fn loading_and_export_progress_tracks_real_work_and_preserves_payloads() {
    use std::sync::{Arc, Mutex};
    let source = tempfile::tempdir().unwrap();
    let expected = [
        ("a.assets", 7, b"alpha".as_slice()),
        ("b.assets", 8, b"beta".as_slice()),
    ];
    for (name, id, bytes) in expected {
        fs::write(
            source.path().join(name),
            file(&[(49, id, text(name, bytes))]),
        )
        .unwrap();
    }
    let updates = Arc::new(Mutex::new(Vec::new()));
    let captured = updates.clone();
    let observer: assetstudio_core::ProgressObserver =
        Arc::new(move |update| captured.lock().unwrap().push(update));
    let session = Session::open_with_progress(
        source.path(),
        &LoadOptions::default(),
        Some(observer.clone()),
    )
    .unwrap();
    assert_eq!(session.assets().len(), 2);
    for stage in ["Reading files", "Reading asset names", "Building catalog"] {
        let updates = updates.lock().unwrap();
        let stage: Vec<_> = updates
            .iter()
            .filter(|update| update.stage == stage)
            .collect();
        assert!(!stage.is_empty());
        assert_eq!(stage.last().unwrap().completed, 2);
        assert!(
            stage
                .iter()
                .all(|update| update.total == Some(2) && update.completed <= 2)
        );
        assert!(
            stage
                .windows(2)
                .all(|pair| pair[0].completed <= pair[1].completed)
        );
    }
    updates.lock().unwrap().clear();
    let output = tempfile::tempdir().unwrap();
    let keys: Vec<_> = session.assets().iter().map(|asset| asset.key).collect();
    let report = session
        .export_with_progress(
            &keys,
            output.path(),
            &ExportOptions::default(),
            &Default::default(),
            Some(&observer),
        )
        .unwrap();
    assert!(report.failed.is_empty());
    assert_eq!(report.written.len(), 2);
    assert_eq!(fs::read(&report.written[0]).unwrap(), b"alpha");
    assert_eq!(fs::read(&report.written[1]).unwrap(), b"beta");
    assert_eq!(updates.lock().unwrap().last().unwrap().completed, 2);
    assert_eq!(updates.lock().unwrap().last().unwrap().total, Some(2));

    // A failed second input must not report a completed file-reading phase.
    fs::write(source.path().join("b.assets"), b"UnityFS\0").unwrap();
    updates.lock().unwrap().clear();
    assert!(
        Session::open_with_progress(source.path(), &LoadOptions::default(), Some(observer))
            .is_err()
    );
    let updates = updates.lock().unwrap();
    let last = updates
        .iter()
        .rev()
        .find(|update| update.stage == "Reading files")
        .unwrap();
    assert!(last.completed < last.total.unwrap());
}

#[test]
fn continued_multi_input_load_and_extraction_cannot_swallow_shared_game_limits() {
    let root = tempfile::tempdir().unwrap();
    let valid = root.path().join("valid.assets");
    fs::write(&valid, file(&[(49, 7, text("hello", b"payload"))])).unwrap();
    let oversized = root.path().join("oversized.bundle");
    let sparse = fs::File::create(&oversized).unwrap();
    sparse.set_len(512 * 1024 * 1024 + 1).unwrap();
    drop(sparse);
    let paths = [valid, oversized];
    let options = LoadOptions {
        game: "CodenameJump".into(),
        continue_on_error: true,
        ..LoadOptions::default()
    };
    let error = Session::open_paths(&paths, &options).unwrap_err();
    assert!(format!("{error:#}").contains("budget"));
    let error = Session::extract_paths_with_auxiliary_inputs(
        &paths,
        &root.path().join("outputs"),
        &options,
        false,
        &[],
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("budget"));
    assert_eq!(
        fs::metadata(&paths[1]).unwrap().len(),
        512 * 1024 * 1024 + 1
    );
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
fn multi_file_load_preserves_identity_and_can_skip_deleted_inputs() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.assets");
    let second = root.path().join("second.assets");
    fs::write(&first, file(&[(49, 7, text("first", b"1"))])).unwrap();
    fs::write(&second, file(&[(49, 7, text("second", b"2"))])).unwrap();
    let paths = [first.clone(), second.clone(), first];
    let session = Session::open_paths(&paths, &LoadOptions::default()).unwrap();
    assert_eq!(session.assets().len(), 2);
    assert_ne!(session.assets()[0].key, session.assets()[1].key);
    assert_eq!(session.input_paths().len(), 2);
    let missing = root.path().join("deleted.assets");
    assert!(
        Session::open_paths(&[second.clone(), missing.clone()], &LoadOptions::default()).is_err()
    );
    let options = LoadOptions {
        continue_on_error: true,
        ..LoadOptions::default()
    };
    let partial = Session::open_paths(&[second, missing], &options).unwrap();
    assert_eq!(partial.assets().len(), 1);
    assert_eq!(partial.diagnostics().len(), 1);
}

#[test]
fn overwrite_cannot_replace_serialized_source_or_hardlink_alias() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("hello_0_7.txt");
    let original = file(&[(49, 7, text("hello", b"payload"))]);
    fs::write(&source, &original).unwrap();
    let session = Session::open(&source, &LoadOptions::default()).unwrap();
    let options = ExportOptions {
        overwrite: true,
        group: GroupMode::None,
        ..ExportOptions::default()
    };
    let report = session
        .export(&[session.assets()[0].key], root.path(), &options)
        .unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(
        session
            .write_map(&[session.assets()[0].key], &source, MapFormat::Json)
            .is_err()
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    let alias_root = tempfile::tempdir().unwrap();
    let alias = alias_root.path().join("hello_0_7.txt");
    fs::hard_link(&source, &alias).unwrap();
    let report = session
        .export(&[session.assets()[0].key], alias_root.path(), &options)
        .unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(fs::read(alias).unwrap(), original);
}

fn school_encode(data: &[u8]) -> Vec<u8> {
    let stride = data.len() % 7 + 3;
    let mut out = data.to_vec();
    let mut key = 0xff;
    for (index, byte) in out.iter_mut().enumerate().skip(1) {
        if index % stride != 0 {
            *byte ^= key;
        } else {
            key = !key;
        }
    }
    out
}

#[test]
fn wrapper_extraction_never_replaces_original_encrypted_source() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("wrapper.assets");
    let encrypted = school_encode(&file(&[(49, 7, text("hello", b"payload"))]));
    fs::write(&source, &encrypted).unwrap();
    let options = LoadOptions {
        game: "SchoolGirlStrikers".into(),
        ..LoadOptions::default()
    };
    let report = Session::extract(&source, root.path(), &options, true).unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].contains("original input"));
    assert_eq!(fs::read(&source).unwrap(), encrypted);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    assert!(Session::extract(root.path(), &root.path().join("inside"), &options, true).is_err());
    assert_eq!(fs::read(source).unwrap(), encrypted);
}

#[test]
fn multiple_extraction_inputs_share_original_source_protection() {
    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("source");
    let output = root.path().join("out");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(output.join("input-0001")).unwrap();
    let first = source_dir.join("victim.assets");
    let second = output.join("input-0001/victim.assets");
    let plain = file(&[(49, 7, text("hello", b"payload"))]);
    let encrypted = school_encode(&plain);
    fs::write(&first, &encrypted).unwrap();
    fs::write(&second, &plain).unwrap();
    let options = LoadOptions {
        game: "SchoolGirlStrikers".into(),
        ..LoadOptions::default()
    };
    let report =
        Session::extract_paths(&[first.clone(), second.clone()], &output, &options, true).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.written.len(), 1);
    assert_eq!(fs::read(first).unwrap(), encrypted);
    assert_eq!(fs::read(second).unwrap(), plain);
}

#[test]
fn selected_schema_and_index_inputs_cannot_be_replaced_by_exports() {
    for schema in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("input.assets");
        let auxiliary = root.path().join("hello_0_7.txt");
        fs::write(&source, file(&[(49, 7, text("hello", b"payload"))])).unwrap();
        let bytes = if schema {
            b"{\"version\":1,\"entries\":[]}".as_slice()
        } else {
            b"{}".as_slice()
        };
        fs::write(&auxiliary, bytes).unwrap();
        let mut session = Session::open(&source, &LoadOptions::default()).unwrap();
        if schema {
            session.load_schemas(&auxiliary).unwrap();
        } else {
            session.recover_containers(&auxiliary).unwrap();
        }
        let report = session
            .export(
                &[session.assets()[0].key],
                root.path(),
                &ExportOptions {
                    group: GroupMode::None,
                    overwrite: true,
                    ..ExportOptions::default()
                },
            )
            .unwrap();
        assert!(report.written.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(fs::read(auxiliary).unwrap(), bytes);
    }
}

#[test]
fn successful_game_profile_load_has_advisory_without_partial_failure() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("test.assets");
    fs::write(&path, file(&[(49, 7, text("hello", b"payload"))])).unwrap();
    let options = LoadOptions {
        game: "FakeHeader".into(),
        ..LoadOptions::default()
    };
    let session = Session::open(&path, &options).unwrap();
    assert_eq!(session.assets().len(), 1);
    assert!(session.diagnostics().is_empty());
    assert_eq!(session.warnings().len(), 1);
}

#[test]
fn extraction_preserves_explicit_auxiliary_inputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("wrapper.assets");
    let auxiliary = root.path().join("out/input-0001/wrapper.assets");
    fs::create_dir_all(auxiliary.parent().unwrap()).unwrap();
    let encrypted = school_encode(&file(&[(49, 7, text("hello", b"payload"))]));
    fs::write(&source, encrypted).unwrap();
    fs::write(&auxiliary, b"important configuration").unwrap();
    let options = LoadOptions {
        game: "SchoolGirlStrikers".into(),
        ..LoadOptions::default()
    };
    let report = Session::extract_paths_with_auxiliary_inputs(
        &[source],
        &root.path().join("out/input-0001"),
        &options,
        true,
        std::slice::from_ref(&auxiliary),
    )
    .unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read(auxiliary).unwrap(), b"important configuration");
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
