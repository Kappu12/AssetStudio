//! Real SerializedFile type-tree bytes, encoded from the original field order.
use std::fs;

use assetstudio_core::class_structures::ClassStructureLimits;
use assetstudio_core::{LoadOptions, Session};
use unity_rs_core::source::Region;

#[test]
fn embedded_definitions_preserve_version_script_identity_fields_and_last_loaded_priority() {
    let first = fixture(
        "2022.3.62f1",
        &[
            (49, -1, "TextAsset", "Base"),
            (114, 2, "ScriptData", "Base"),
        ],
    );
    let second = fixture("2022.3.62f1", &[(49, -1, "Replacement", "Base")]);
    let other = fixture("2021.3.1f1", &[(49, -1, "TextAsset", "Other")]);
    let session = Session::from_regions(
        [
            ("first.assets".to_owned(), Region::from_bytes(first)),
            ("second.assets".to_owned(), Region::from_bytes(second)),
            ("other.assets".to_owned(), Region::from_bytes(other)),
        ],
        &LoadOptions::default(),
    )
    .unwrap();
    let definitions = session.class_structures().unwrap();
    assert_eq!(definitions.len(), 3);
    assert_eq!(
        definitions
            .iter()
            .map(|d| (d.unity_version.as_str(), d.class_id))
            .collect::<Vec<_>>(),
        vec![("2021.3.1f1", 49), ("2022.3.62f1", -3), ("2022.3.62f1", 49)]
    );
    let replacement = &definitions[2];
    assert_eq!(replacement.name, "Replacement Base");
    assert_eq!(replacement.source, "second.assets");
    assert_eq!(
        replacement.text,
        "Replacement Base -1 False\r\n\tint m_Value 4 False\r\n\tstring m_Name -1 True\r\n"
    );
    assert_eq!(definitions[1].serialized_class_id, 114);
    assert_eq!(definitions[1].name, "ScriptData Base");
}

#[test]
fn class_structure_limits_cover_scans_nodes_strings_individual_and_cumulative_text() {
    let session = Session::from_regions(
        [(
            "fixture.assets".into(),
            Region::from_bytes(fixture(
                "2022.3.62f1",
                &[
                    (49, -1, "TextAsset", "Base"),
                    (114, 2, "ScriptData", "Base"),
                ],
            )),
        )],
        &LoadOptions::default(),
    )
    .unwrap();
    let defaults = ClassStructureLimits::default();
    for limits in [
        ClassStructureLimits {
            maximum_entries: 1,
            ..defaults
        },
        ClassStructureLimits {
            maximum_types_scanned: 1,
            ..defaults
        },
        ClassStructureLimits {
            maximum_total_nodes: 5,
            ..defaults
        },
        ClassStructureLimits {
            maximum_node_level: 0,
            ..defaults
        },
        ClassStructureLimits {
            maximum_string_bytes: 3,
            ..defaults
        },
        ClassStructureLimits {
            maximum_entry_text_bytes: 16,
            ..defaults
        },
        ClassStructureLimits {
            maximum_total_text_bytes: 100,
            ..defaults
        },
    ] {
        assert!(session.class_structures_with_limits(limits).is_err());
    }
    assert_eq!(
        session.class_structures().unwrap().len(),
        2,
        "failed budget checks must not mutate the session"
    );
}

#[test]
fn class_structure_exports_are_atomic_portable_and_preserve_existing_or_original_inputs() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("source.assets");
    let bytes = fixture("2022.3.62f1", &[(49, -1, "../CON:Type", "Base")]);
    fs::write(&input, &bytes).unwrap();
    let session = Session::open(&input, &LoadOptions::default()).unwrap();
    let output = root.path().join("export");
    let first = session.export_class_structures(&output).unwrap();
    assert!(first.failed.is_empty(), "{:?}", first.failed);
    assert_eq!(first.written.len(), 1);
    let file = &first.written[0];
    assert!(file.starts_with(output.canonicalize().unwrap().join("2022.3.62f1")));
    assert!(
        !file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(['/', ':'])
    );
    let saved = fs::read(file).unwrap();
    let repeated = session.export_class_structures(&output).unwrap();
    assert!(repeated.written.is_empty());
    assert_eq!(repeated.skipped.len(), 1);
    assert_eq!(fs::read(file).unwrap(), saved);
    assert_eq!(fs::read(&input).unwrap(), bytes);

    // Put a source identity at the exact derived output name. No-clobber must
    // not hide an attempt to publish over an original input (including aliases).
    let alias_root = root.path().join("alias");
    let version = alias_root.join("2022.3.62f1");
    fs::create_dir_all(&version).unwrap();
    let alias = version.join(file.file_name().unwrap());
    fs::hard_link(&input, &alias).unwrap();
    let report = session.export_class_structures(&alias_root).unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read(alias).unwrap(), bytes);
}

#[test]
fn unavailable_trees_do_not_create_fake_definitions_or_an_output_folder() {
    let session = Session::from_regions(
        [(
            "empty.assets".into(),
            Region::from_bytes(fixture("2022.3.62f1", &[])),
        )],
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(session.class_structures().unwrap().is_empty());
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("unused");
    let report = session.export_class_structures(&output).unwrap();
    assert!(report.written.is_empty());
    assert!(!output.exists());
}

fn fixture(version: &str, types: &[(i32, i16, &str, &str)]) -> Vec<u8> {
    let mut metadata = version.as_bytes().to_vec();
    metadata.push(0);
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    let tree_enabled = !types.is_empty();
    metadata.push(u8::from(tree_enabled));
    let fallback = [(49, -1, "TextAsset", "Base")];
    let types = if types.is_empty() {
        fallback.as_slice()
    } else {
        types
    };
    metadata.extend_from_slice(&(types.len() as i32).to_le_bytes());
    for &(class_id, script_index, root_type, root_field) in types {
        metadata.extend_from_slice(&class_id.to_le_bytes());
        metadata.push(0);
        metadata.extend_from_slice(&script_index.to_le_bytes());
        if class_id == 114 {
            metadata.extend_from_slice(&[0; 16]);
        }
        metadata.extend_from_slice(&[0; 16]);
        if tree_enabled {
            let mut strings = Vec::new();
            let mut string = |value: &str| {
                let offset = strings.len() as u32;
                strings.extend_from_slice(value.as_bytes());
                strings.push(0);
                offset
            };
            let nodes = [
                (0, root_type, root_field, -1_i32, 0_i32),
                (1, "int", "m_Value", 4, 0),
                (1, "string", "m_Name", -1, 0x4000),
            ];
            let encoded = nodes.map(|(level, kind, name, size, flags)| {
                (level, string(kind), string(name), size, flags)
            });
            metadata.extend_from_slice(&3_i32.to_le_bytes());
            metadata.extend_from_slice(&(strings.len() as i32).to_le_bytes());
            for (index, (level, kind, name, size, flags)) in encoded.into_iter().enumerate() {
                metadata.extend_from_slice(&1_u16.to_le_bytes());
                metadata.push(level);
                metadata.push(0);
                metadata.extend_from_slice(&kind.to_le_bytes());
                metadata.extend_from_slice(&name.to_le_bytes());
                metadata.extend_from_slice(&size.to_le_bytes());
                metadata.extend_from_slice(&(index as i32).to_le_bytes());
                metadata.extend_from_slice(&flags.to_le_bytes());
                metadata.extend_from_slice(&0_u64.to_le_bytes());
            }
            metadata.extend_from_slice(&strings);
            metadata.extend_from_slice(&0_i32.to_le_bytes());
        }
    }
    let mut payload = 7_i32.to_le_bytes().to_vec();
    payload.extend_from_slice(b"fixture");
    while !payload.len().is_multiple_of(4) {
        payload.push(0);
    }
    payload.extend_from_slice(&4_i32.to_le_bytes());
    payload.extend_from_slice(b"text");
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    while !metadata.len().is_multiple_of(4) {
        metadata.push(0);
    }
    metadata.extend_from_slice(&7_i64.to_le_bytes());
    metadata.extend_from_slice(&0_i64.to_le_bytes());
    metadata.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    metadata.extend_from_slice(&0_i32.to_le_bytes());
    metadata.extend_from_slice(&[0; 12]); // Script types, externals, reference types.
    metadata.push(0);
    let offset = (48 + metadata.len()).next_multiple_of(16);
    let mut data = vec![0; 48];
    data[8..12].copy_from_slice(&22_u32.to_be_bytes());
    data[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    data[24..32].copy_from_slice(&((offset + payload.len()) as i64).to_be_bytes());
    data[32..40].copy_from_slice(&(offset as i64).to_be_bytes());
    data.extend_from_slice(&metadata);
    data.resize(offset, 0);
    data.extend_from_slice(&payload);
    data
}
