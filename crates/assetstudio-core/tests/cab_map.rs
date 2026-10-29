use std::fs;

use assetstudio_core::cab_map::{CabEntry, CabMap};
use assetstudio_core::{LoadOptions, Session};
use unity_rs_core::source::Region;

fn entry(cab: &str, path: &str, dependencies: &[&str]) -> CabEntry {
    CabEntry {
        cab: cab.into(),
        path: path.into(),
        offset: 0,
        dependencies: dependencies
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

/// Written byte-for-byte as BinaryWriter.Write(string/Int32/Int64) in the old
/// AssetsHelper.DumpCABMap, independently of the Rust writer under test.
fn original_binary_fixture() -> Vec<u8> {
    let mut bytes = b"\x07C:\\Game\x01\0\0\0\x05CAB-a\x0dData\\a.bundle".to_vec();
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&[1, 0, 0, 0]);
    bytes.extend_from_slice(b"\x05CAB-b");
    bytes
}

#[test]
fn reads_and_writes_original_dotnet_binary_layout() {
    let bytes = original_binary_fixture();
    let map = CabMap::from_bytes(&bytes).unwrap();
    assert_eq!(map.base_folder(), "C:\\Game");
    assert_eq!(
        map.entries(),
        &[entry("CAB-a", "Data\\a.bundle", &["CAB-b"])]
    );
    assert_eq!(map.to_bytes().unwrap(), bytes);
}

#[test]
fn seven_bit_lengths_count_utf8_bytes_and_cross_the_one_byte_boundary() {
    let base = format!("/{}", "x".repeat(127));
    let map = CabMap::new(base, Vec::new()).unwrap();
    let bytes = map.to_bytes().unwrap();
    assert_eq!(&bytes[..2], &[0x80, 0x01]);
    assert_eq!(CabMap::from_bytes(&bytes).unwrap(), map);
    let unicode = CabMap::new("/tmp/ä".into(), Vec::new()).unwrap();
    let bytes = unicode.to_bytes().unwrap();
    assert_eq!(bytes[0], 7);
    assert_eq!(CabMap::from_bytes(&bytes).unwrap(), unicode);
}

#[test]
fn malformed_lengths_counts_utf8_and_truncation_are_errors() {
    let original = original_binary_fixture();
    for end in 0..original.len() {
        assert!(
            CabMap::from_bytes(&original[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    for bytes in [
        vec![0x80; 6],
        vec![0xff, 0xff, 0xff, 0xff, 0x08],
        vec![1, 0xff, 0, 0, 0, 0],
        vec![1, b'x', 0xff, 0xff, 0xff, 0xff],
        vec![1, b'x', 0xff, 0xff, 0xff, 0x7f],
    ] {
        assert!(CabMap::from_bytes(&bytes).is_err(), "accepted {bytes:?}");
    }
    let mut trailing = original;
    trailing.push(0);
    assert!(CabMap::from_bytes(&trailing).is_err());
    assert!(CabMap::new("x".repeat(4097), Vec::new()).is_err());
}

#[test]
fn duplicate_cab_names_and_unsafe_source_paths_are_rejected() {
    assert!(
        CabMap::new(
            "/game".into(),
            vec![entry("CAB-a", "a", &[]), entry("cab-A", "b", &[])]
        )
        .is_err()
    );
    for path in [
        "../escape",
        "folder/../escape",
        "/absolute",
        "C:\\escape",
        "\\\\server\\share",
        "a//b",
        "./a",
        "a:stream",
        "a\0b",
        "CON",
        "folder/NUL.txt",
        "COM1",
        "COM¹.dat",
        "LPT²",
        "CON .txt",
        "LPT1 .bin",
        "CONIN$",
        "CONOUT$",
    ] {
        assert!(
            CabMap::new("/game".into(), vec![entry("CAB-a", path, &[])]).is_err(),
            "accepted {path:?}"
        );
    }
    let mut negative = entry("CAB-a", "a", &[]);
    negative.offset = -1;
    assert!(CabMap::new("/game".into(), vec![negative]).is_err());
}

#[test]
fn transitive_dependencies_are_case_insensitive_cycle_safe_and_deduplicated() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("data")).unwrap();
    for path in ["a.bundle", "data/b.bundle", "data/c.bundle"] {
        fs::write(directory.path().join(path), b"source").unwrap();
    }
    let map = CabMap::new(
        directory.path().to_str().unwrap().into(),
        vec![
            entry("CAB-a", "a.bundle", &["cab-B", "CAB-c", "CAB-b"]),
            entry("CAB-b", "data/b.bundle", &["CAB-c"]),
            entry("CAB-c", "data/c.bundle", &["CAB-A"]),
        ],
    )
    .unwrap();
    let a = fs::canonicalize(directory.path().join("a.bundle")).unwrap();
    let paths = map.resolve_paths(&[a.clone(), a.clone()], None).unwrap();
    assert_eq!(
        paths,
        vec![
            a,
            fs::canonicalize(directory.path().join("data/b.bundle")).unwrap(),
            fs::canonicalize(directory.path().join("data/c.bundle")).unwrap()
        ]
    );
    let directory_paths = map
        .resolve_paths(&[directory.path().to_owned()], None)
        .unwrap();
    assert_eq!(
        directory_paths,
        vec![fs::canonicalize(directory.path()).unwrap()]
    );
}

#[test]
fn missing_cabs_and_nonzero_offsets_do_not_silently_load_incomplete_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("a.bundle");
    fs::write(&source, b"source").unwrap();
    let missing = CabMap::new(
        directory.path().to_str().unwrap().into(),
        vec![entry("CAB-a", "a.bundle", &["CAB-missing"])],
    )
    .unwrap();
    assert!(
        missing
            .resolve_paths(std::slice::from_ref(&source), None)
            .unwrap_err()
            .to_string()
            .contains("missing CAB")
    );
    let mut specialized = entry("CAB-a", "a.bundle", &[]);
    specialized.offset = 0x1234;
    let specialized =
        CabMap::new(directory.path().to_str().unwrap().into(), vec![specialized]).unwrap();
    assert!(
        specialized
            .resolve_paths(std::slice::from_ref(&source), None)
            .unwrap_err()
            .to_string()
            .contains("specialized source offset")
    );
    let round_trip = CabMap::from_bytes(&specialized.to_bytes().unwrap()).unwrap();
    assert_eq!(round_trip.entries()[0].offset, 0x1234);
}

#[test]
fn explicit_base_override_relocates_a_cache_without_expanding_its_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("a.bundle");
    fs::write(&source, b"source").unwrap();
    let map = CabMap::new("C:\\OldGame".into(), vec![entry("CAB-a", "a.bundle", &[])]).unwrap();
    let paths = map
        .resolve_paths(std::slice::from_ref(&source), Some(directory.path()))
        .unwrap();
    assert_eq!(paths, vec![fs::canonicalize(source).unwrap()]);
    let outside = tempfile::tempdir().unwrap();
    let other = outside.path().join("outside.bundle");
    fs::write(&other, b"source").unwrap();
    assert!(map.resolve_paths(&[other], Some(directory.path())).is_err());
}

#[cfg(unix)]
#[test]
fn dependency_symlinks_cannot_escape_the_base_folder() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = directory.path().join("a.bundle");
    fs::write(&source, b"source").unwrap();
    let target = outside.path().join("outside.bundle");
    fs::write(&target, b"outside").unwrap();
    std::os::unix::fs::symlink(target, directory.path().join("escape.bundle")).unwrap();
    let map = CabMap::new(
        directory.path().to_str().unwrap().into(),
        vec![
            entry("CAB-a", "a.bundle", &["CAB-escape"]),
            entry("CAB-escape", "escape.bundle", &[]),
        ],
    )
    .unwrap();
    assert!(
        map.resolve_paths(&[source], None)
            .unwrap_err()
            .to_string()
            .contains("escapes the base")
    );
}

#[test]
fn map_generation_uses_real_serialized_externals_and_whole_bundle_sources() {
    let directory = tempfile::tempdir().unwrap();
    let a = directory.path().join("a.bundle");
    let b = directory.path().join("b.bundle");
    fs::write(
        &a,
        unity_fs(&[("CAB-a", &serialized(&["archive:/CAB-b/CAB-b"]))]),
    )
    .unwrap();
    fs::write(&b, unity_fs(&[("CAB-b", &serialized(&[]))])).unwrap();
    let session = Session::open(directory.path(), &LoadOptions::default()).unwrap();
    let map = session.build_cab_map(directory.path()).unwrap();
    assert_eq!(
        map.entries(),
        &[
            entry("CAB-a", "a.bundle", &["CAB-b"]),
            entry("CAB-b", "b.bundle", &[])
        ]
    );
    let output = directory.path().join("maps/cache.bin");
    assert_eq!(map.write_path(&output, false).unwrap().written.len(), 1);
    assert_eq!(map.write_path(&output, false).unwrap().skipped.len(), 1);
    assert_eq!(CabMap::from_path(&output).unwrap(), map);
    let resolved = map.resolve_paths(&[a], None).unwrap();
    assert_eq!(resolved.len(), 2);
    let reopened = Session::open_paths(&resolved, &LoadOptions::default()).unwrap();
    assert_eq!(reopened.assets().len(), 2);
}

#[test]
fn anonymous_regions_cannot_be_misrepresented_as_filesystem_cache_sources() {
    let directory = tempfile::tempdir().unwrap();
    let session = Session::from_regions(
        [("CAB-anonymous".into(), Region::from_bytes(serialized(&[])))],
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(
        session
            .build_cab_map(directory.path())
            .unwrap_err()
            .to_string()
            .contains("provenance")
    );
}

#[test]
fn generated_map_retains_a_split_files_first_segment_for_full_file_loading() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = unity_fs(&[("CAB-split", &serialized(&[]))]);
    let middle = bundle.len() / 2;
    let first = directory.path().join("bundle.split0");
    fs::write(&first, &bundle[..middle]).unwrap();
    let second = directory.path().join("bundle.split1");
    fs::write(&second, &bundle[middle..]).unwrap();
    let session = Session::open(&second, &LoadOptions::default()).unwrap();
    let map = session.build_cab_map(directory.path()).unwrap();
    assert_eq!(map.entries()[0].path, "bundle.split0");
    assert_eq!(map.entries()[0].offset, 0);
    let paths = map.resolve_paths(&[second], None).unwrap();
    assert_eq!(
        Session::open_paths(&paths, &LoadOptions::default())
            .unwrap()
            .assets()
            .len(),
        1
    );
}

#[test]
fn cache_writer_never_replaces_an_indexed_source_even_with_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("assets_map.bin");
    let original = serialized(&[]);
    fs::write(&source, &original).unwrap();
    let session = Session::open(&source, &LoadOptions::default()).unwrap();
    let map = session.build_cab_map(directory.path()).unwrap();
    assert!(
        map.write_path(&source, true)
            .unwrap_err()
            .to_string()
            .contains("indexed CAB source")
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(
        session
            .write_cab_map(directory.path(), &source, true)
            .is_err()
    );
    assert_eq!(fs::read(source).unwrap(), original);
}

fn serialized(externals: &[&str]) -> Vec<u8> {
    let mut object = Vec::new();
    string(&mut object, "hello");
    object.extend_from_slice(&7_i32.to_le_bytes());
    object.extend_from_slice(b"payload");
    let mut metadata = b"2022.3.62f1\0".to_vec();
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    metadata.extend_from_slice(&49_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&(-1_i16).to_le_bytes());
    metadata.extend_from_slice(&[0; 16]);
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    while !(48 + metadata.len()).is_multiple_of(4) {
        metadata.push(0);
    }
    metadata.extend_from_slice(&7_i64.to_le_bytes());
    metadata.extend_from_slice(&0_i64.to_le_bytes());
    metadata.extend_from_slice(&(object.len() as u32).to_le_bytes());
    metadata.extend_from_slice(&0_i32.to_le_bytes());
    metadata.extend_from_slice(&0_i32.to_le_bytes()); // Script identifiers.
    metadata.extend_from_slice(&(externals.len() as i32).to_le_bytes());
    for path in externals {
        metadata.push(0); // External prefix.
        metadata.extend_from_slice(&[0; 16]);
        metadata.extend_from_slice(&0_i32.to_le_bytes());
        metadata.extend_from_slice(path.as_bytes());
        metadata.push(0);
    }
    metadata.extend_from_slice(&0_i32.to_le_bytes()); // Reference types.
    metadata.push(0);
    let offset = (48 + metadata.len()).next_multiple_of(16);
    let size = offset + object.len();
    let mut bytes = vec![0; 48];
    bytes[8..12].copy_from_slice(&22_u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    bytes[24..32].copy_from_slice(&(size as i64).to_be_bytes());
    bytes[32..40].copy_from_slice(&(offset as i64).to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.resize(offset, 0);
    bytes.extend_from_slice(&object);
    bytes
}

fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as i32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn unity_fs(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut directory = Vec::new();
    for (name, bytes) in entries {
        directory.extend_from_slice(&(data.len() as i64).to_be_bytes());
        directory.extend_from_slice(&(bytes.len() as i64).to_be_bytes());
        directory.extend_from_slice(&4_u32.to_be_bytes());
        directory.extend_from_slice(name.as_bytes());
        directory.push(0);
        data.extend_from_slice(bytes);
    }
    let mut info = vec![0; 16];
    info.extend_from_slice(&1_i32.to_be_bytes());
    info.extend_from_slice(&(data.len() as u32).to_be_bytes());
    info.extend_from_slice(&(data.len() as u32).to_be_bytes());
    info.extend_from_slice(&0_u16.to_be_bytes());
    info.extend_from_slice(&(entries.len() as i32).to_be_bytes());
    info.extend_from_slice(&directory);
    let mut bytes = b"UnityFS\0".to_vec();
    bytes.extend_from_slice(&7_u32.to_be_bytes());
    bytes.extend_from_slice(b"5.x.x\x002022.3.62f1\0");
    let size_position = bytes.len();
    bytes.extend_from_slice(&0_i64.to_be_bytes());
    bytes.extend_from_slice(&(info.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(info.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&0x40_u32.to_be_bytes());
    while !bytes.len().is_multiple_of(16) {
        bytes.push(0);
    }
    bytes.extend_from_slice(&info);
    bytes.extend_from_slice(&data);
    let size = bytes.len() as i64;
    bytes[size_position..size_position + 8].copy_from_slice(&size.to_be_bytes());
    bytes
}
