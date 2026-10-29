use std::fs;

use assetstudio_core::asset_map::{AssetMap, AssetMapEntry, AssetMapFormat};
use assetstudio_core::{LoadOptions, Session};

fn entry(source: &str, id: i64) -> AssetMapEntry {
    AssetMapEntry {
        name: "Name<&Å".into(),
        container: "Assets/UI/file.txt".into(),
        source: source.into(),
        path_id: id,
        class_id: 49,
        class_name: "TextAsset".into(),
        serialized_source: None,
        file_index: None,
        size: 0,
    }
}

#[test]
fn genuine_csharp_plain_and_lz4_array_single_and_multiblock_frames_decode() {
    let fixtures: [(&[u8], usize); 4] = [
        (include_bytes!("fixtures/asset_maps/csharp_plain.map"), 1),
        (
            include_bytes!("fixtures/asset_maps/csharp_lz4_array.map"),
            2,
        ),
        (
            include_bytes!("fixtures/asset_maps/csharp_lz4_single.map"),
            2,
        ),
        (
            include_bytes!("fixtures/asset_maps/csharp_lz4_multiblock.map"),
            3000,
        ),
    ];
    for (bytes, count) in fixtures {
        let map = AssetMap::from_bytes(bytes, AssetMapFormat::MessagePack).unwrap();
        assert_eq!(map.game_type(), Some(0));
        assert_eq!(map.entries().len(), count);
        if count == 1 {
            assert_eq!(map.entries()[0].name, "a");
            assert_eq!(map.entries()[0].path_id, 7);
        } else {
            for (index, entry) in map.entries().iter().enumerate() {
                assert_eq!(entry.name, format!("Icon_Åäö_{index}{}", "x".repeat(80)));
                assert_eq!(entry.container, format!("Assets/UI/{index}.txt"));
                assert_eq!(
                    entry.source,
                    format!("fixtures/source_{}.bundle", index % 3)
                );
                assert_eq!(entry.class_id, if index % 2 == 0 { 49 } else { -1 });
                assert_eq!(
                    entry.path_id,
                    if index == 0 {
                        i64::MIN
                    } else if index == count - 1 {
                        i64::MAX
                    } else {
                        index as i64
                    }
                );
            }
        }
    }
    let json = AssetMap::from_bytes(
        include_bytes!("fixtures/asset_maps/csharp_lz4_array.json"),
        AssetMapFormat::Json,
    )
    .unwrap();
    let binary = AssetMap::from_bytes(
        include_bytes!("fixtures/asset_maps/csharp_lz4_array.map"),
        AssetMapFormat::MessagePack,
    )
    .unwrap();
    assert_eq!(json, binary);
}

#[test]
fn json_xml_and_messagepack_writers_roundtrip_ids_and_escaped_names() {
    let mut first = entry("source.assets", i64::MIN);
    first.name = "name<&\"'Å".into();
    let map = AssetMap::new(vec![first, entry("other.assets", i64::MAX)], Some(0)).unwrap();
    for format in [
        AssetMapFormat::Json,
        AssetMapFormat::Xml,
        AssetMapFormat::MessagePack,
    ] {
        let bytes = map.to_bytes(format).unwrap();
        let reopened = AssetMap::from_bytes(&bytes, format).unwrap();
        assert_eq!(reopened.entries(), map.entries());
        let detected = AssetMap::from_bytes(&bytes, AssetMapFormat::Auto).unwrap();
        assert_eq!(detected, reopened);
    }
}

#[test]
fn legacy_and_rust_json_and_xml_shapes_are_interoperable() {
    let legacy =
        br#"[{"Name":"n","Container":"c","Source":"s.assets","PathID":7,"Type":"TextAsset"}]"#;
    let native=br#"[{"name":"n","container":"c","source":"s.assets","key":{"file_index":9,"path_id":7},"class_id":49,"class_name":"TextAsset","size":12}]"#;
    let original_xml=br#"<?xml version="1.0"?><Assets filename="index.xml" createdAt="2026-10-29"><Asset><Name>n</Name><Container>c</Container><Type id="49">TextAsset</Type><PathID>7</PathID><Source>s.assets</Source></Asset></Assets>"#;
    let native_xml=br#"<Assets><Asset file="9" pathId="7" classId="49" size="12"><Name>n</Name><Type>TextAsset</Type><Container>c</Container><Source>s.assets</Source></Asset></Assets>"#;
    for (bytes, format) in [
        (legacy.as_slice(), AssetMapFormat::Json),
        (native.as_slice(), AssetMapFormat::Json),
        (original_xml.as_slice(), AssetMapFormat::Xml),
        (native_xml.as_slice(), AssetMapFormat::Xml),
    ] {
        let map = AssetMap::from_bytes(bytes, format).unwrap();
        let record = &map.entries()[0];
        assert_eq!(record.name, "n");
        assert_eq!(record.container, "c");
        assert_eq!(record.class_id, 49);
        assert_eq!(record.path_id, 7);
    }
}

#[test]
fn json_rejects_structural_expansion_and_conflicting_class_identities() {
    let valid = r#"{"Name":"n","Container":"","Source":"s.assets","PathID":7,"Type":49}"#;
    let oversized_entry = format!("[{{\"extra\":[{}]}}]", "0,".repeat(100_000) + "0");
    let escaped_string = format!("[{{\"Name\":\"{}\"}}]", "\\u0061".repeat(4097));
    let oversized_string = format!("[{{\"Name\":\"{}\"}}]", "x".repeat(4096 * 6 + 1));
    let conflict =
        r#"[{"Name":"n","Source":"s.assets","PathID":7,"class_id":49,"class_name":"Texture2D"}]"#;
    for json in [
        oversized_entry.as_str(),
        escaped_string.as_str(),
        oversized_string.as_str(),
        conflict,
        r#"[{"Name":"n","Name":"duplicate"}]"#,
        r#"{"AssetEntries":[],"entries":[]}"#,
        r#"[{"extra":{"a":{"b":{"c":{"d":0}}}}}]"#,
    ] {
        assert!(AssetMap::from_bytes(json.as_bytes(), AssetMapFormat::Json).is_err());
    }
    let wrapper = format!("{{\"GameType\":\"Normal\",\"AssetEntries\":[{valid}]}}");
    let map = AssetMap::from_bytes(wrapper.as_bytes(), AssetMapFormat::Json).unwrap();
    assert_eq!(map.entries().len(), 1);
    assert_eq!(map.game_type(), Some(0));
}

#[test]
fn xml_external_entities_and_malformed_frames_do_not_expand_or_panic() {
    for bytes in [
        br#"<!DOCTYPE Assets [<!ENTITY x SYSTEM "file:///etc/passwd">]><Assets/>"#.as_slice(),
        br#"<Assets><Asset><Name>&external;</Name></Asset></Assets>"#.as_slice(),
        br#"<Assets><Asset><Name><Nested/></Name></Asset></Assets>"#.as_slice(),
        br#"<Assets><Asset><Name>a</Name><Name>b</Name></Asset></Assets>"#.as_slice(),
        br#"<Assets/>trailing"#.as_slice(),
        br#"<Assets><Asset/></Assets>"#.as_slice(),
    ] {
        assert!(AssetMap::from_bytes(bytes, AssetMapFormat::Xml).is_err());
    }
    let fixture = include_bytes!("fixtures/asset_maps/csharp_lz4_array.map");
    for length in 0..fixture.len() {
        assert!(
            AssetMap::from_bytes(&fixture[..length], AssetMapFormat::MessagePack).is_err(),
            "accepted truncation at {length}"
        );
    }
    let too_large = [0x92, 0xd6, 98, 0xce, 0x7f, 0xff, 0xff, 0xff, 0xc4, 0];
    assert!(AssetMap::from_bytes(&too_large, AssetMapFormat::MessagePack).is_err());
}

#[test]
fn selected_sources_are_confined_and_exact_pathid_class_selection_is_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("sample.assets");
    fs::write(&source, serialized(&[("first", 7), ("second", 9)])).unwrap();
    let map = AssetMap::new(vec![entry(source.to_str().unwrap(), 9)], Some(0)).unwrap();
    assert!(
        map.resolve_sources(&[], directory.path())
            .unwrap()
            .is_empty()
    );
    let paths = map.resolve_sources(&[0], directory.path()).unwrap();
    let session = Session::open_paths(&paths, &LoadOptions::default()).unwrap();
    let keys = map
        .keys_in_session(&[0], &session, directory.path())
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].path_id, 9);
    let outside = tempfile::tempdir().unwrap();
    let other = outside.path().join("outside.assets");
    fs::write(&other, serialized(&[("other", 7)])).unwrap();
    let map = AssetMap::new(vec![entry(other.to_str().unwrap(), 7)], None).unwrap();
    assert!(map.resolve_sources(&[0], directory.path()).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&other, directory.path().join("escape.assets")).unwrap();
        let map = AssetMap::new(vec![entry("escape.assets", 7)], None).unwrap();
        assert!(map.resolve_sources(&[0], directory.path()).is_err());
    }
}

#[test]
fn precise_json_xml_identity_disambiguates_two_cabs_with_identical_pathids() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("bundle");
    let first = serialized(&[("first", 7)]);
    let second = serialized(&[("second", 7)]);
    fs::write(
        &source,
        bundle(&[("CAB-first", &first), ("CAB-second", &second)]),
    )
    .unwrap();
    let session = Session::open(&source, &LoadOptions::default()).unwrap();
    let keys: Vec<_> = session.assets().iter().map(|record| record.key).collect();
    let precise = AssetMap::from_session(&session, &keys, "Normal").unwrap();
    for format in [AssetMapFormat::Json, AssetMapFormat::Xml] {
        let map = AssetMap::from_bytes(&precise.to_bytes(format).unwrap(), format).unwrap();
        let reopened = map
            .keys_in_session(&[1], &session, directory.path())
            .unwrap();
        assert_eq!(reopened, [keys[1]]);
    }
    assert!(precise.to_bytes(AssetMapFormat::MessagePack).is_err());
    let old = AssetMap::new(vec![entry(source.to_str().unwrap(), 7)], Some(0)).unwrap();
    assert!(
        old.keys_in_session(&[0], &session, directory.path())
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
}

fn serialized(objects: &[(&str, i64)]) -> Vec<u8> {
    let mut metadata = b"2022.3.62f1\0".to_vec();
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    metadata.extend_from_slice(&49_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&(-1_i16).to_le_bytes());
    metadata.extend_from_slice(&[0; 16]);
    metadata.extend_from_slice(&(objects.len() as i32).to_le_bytes());
    let mut data = Vec::new();
    for (name, id) in objects {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let start = data.len();
        data.extend_from_slice(&(name.len() as i32).to_le_bytes());
        data.extend_from_slice(name.as_bytes());
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        data.extend_from_slice(&7_i32.to_le_bytes());
        data.extend_from_slice(b"payload");
        while !(48 + metadata.len()).is_multiple_of(4) {
            metadata.push(0);
        }
        metadata.extend_from_slice(&id.to_le_bytes());
        metadata.extend_from_slice(&(start as i64).to_le_bytes());
        metadata.extend_from_slice(&((data.len() - start) as u32).to_le_bytes());
        metadata.extend_from_slice(&0_i32.to_le_bytes());
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
fn bundle(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut directory = Vec::new();
    let mut data = Vec::new();
    for (name, bytes) in entries {
        directory.extend_from_slice(&(data.len() as i64).to_be_bytes());
        directory.extend_from_slice(&(bytes.len() as i64).to_be_bytes());
        directory.extend_from_slice(&4_u32.to_be_bytes());
        directory.extend_from_slice(name.as_bytes());
        directory.push(0);
        data.extend_from_slice(bytes);
    }
    let mut info = vec![0; 16];
    info.extend_from_slice(&1_u32.to_be_bytes());
    info.extend_from_slice(&(data.len() as u32).to_be_bytes());
    info.extend_from_slice(&(data.len() as u32).to_be_bytes());
    info.extend_from_slice(&0_u16.to_be_bytes());
    info.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    info.extend_from_slice(&directory);
    let mut bytes = b"UnityFS\0".to_vec();
    bytes.extend_from_slice(&7_u32.to_be_bytes());
    bytes.extend_from_slice(b"5.x.x\x002022.3.62f1\0");
    let position = bytes.len();
    bytes.extend_from_slice(&0_u64.to_be_bytes());
    bytes.extend_from_slice(&(info.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(info.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&0x40_u32.to_be_bytes());
    while !bytes.len().is_multiple_of(16) {
        bytes.push(0);
    }
    bytes.extend_from_slice(&info);
    bytes.extend_from_slice(&data);
    let size = bytes.len() as u64;
    bytes[position..position + 8].copy_from_slice(&size.to_be_bytes());
    bytes
}
