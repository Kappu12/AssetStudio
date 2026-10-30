//! Constructed Unity 2022.3 player clips, using the baseline C# field order.
//! These are deterministic format fixtures, not proprietary game compatibility.
use assetstudio_core::{ExportOptions, LoadOptions, Session};
use std::fs;
use unity_rs_core::source::Region;

#[derive(Clone)]
struct Binding {
    path: u32,
    attribute: u32,
    class: i32,
    custom: u8,
    pptr: u8,
}
struct Clip {
    words: Vec<u32>,
    stream: u16,
    discrete: u16,
    dense: Vec<f32>,
    dense_width: u32,
    frames: u32,
    rate: f32,
    begin: f32,
    constants: Vec<f32>,
    bindings: Vec<Binding>,
    mapping: Vec<i64>,
}
impl Clip {
    fn dense_root() -> Self {
        Self {
            words: Vec::new(),
            stream: 0,
            discrete: 0,
            dense: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            dense_width: 3,
            frames: 2,
            rate: 2.0,
            begin: 0.25,
            constants: Vec::new(),
            mapping: Vec::new(),
            bindings: vec![Binding {
                path: 0,
                attribute: 1,
                class: 4,
                custom: 4,
                pptr: 0,
            }],
        }
    }
}
fn i32v(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn u32v(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn floats(out: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}
fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}
fn string(out: &mut Vec<u8>, value: &str) {
    i32v(out, value.len() as i32);
    out.extend_from_slice(value.as_bytes());
    pad(out);
}
fn ptr(out: &mut Vec<u8>, id: i64) {
    i32v(out, 0);
    out.extend_from_slice(&id.to_le_bytes());
}
fn streamed_frame(out: &mut Vec<u32>, time: f32, values: &[f32]) {
    out.push(time.to_bits());
    out.push(values.len() as u32);
    for (index, &value) in values.iter().enumerate() {
        out.push(index as u32);
        out.extend_from_slice(&[0, 0, 0, value.to_bits()]);
    }
}
fn clip_bytes(clip: &Clip) -> Vec<u8> {
    let mut out = Vec::new();
    string(&mut out, "Modern");
    out.extend_from_slice(&[0, 0, 1]);
    pad(&mut out);
    for _ in 0..7 {
        i32v(&mut out, 0);
    } // Explicit rotation/packed/Euler/position/scale/float/PPtr curves.
    floats(&mut out, &[30.0]);
    i32v(&mut out, 0);
    floats(&mut out, &[0.0; 6]);
    u32v(&mut out, 0);
    floats(&mut out, &[0.0; 10]);
    floats(&mut out, &[0.0; 7]);
    i32v(&mut out, 0);
    for _ in 0..2 {
        floats(&mut out, &[0.0; 10]);
        i32v(&mut out, 0);
        floats(&mut out, &[0.0; 4]);
    }
    i32v(&mut out, 0);
    i32v(&mut out, 0);
    for _ in 0..4 {
        floats(&mut out, &[0.0; 10]);
    }
    floats(&mut out, &[0.0; 3]);
    i32v(&mut out, clip.words.len() as i32);
    for &word in &clip.words {
        u32v(&mut out, word);
    }
    out.extend_from_slice(&clip.stream.to_le_bytes());
    out.extend_from_slice(&clip.discrete.to_le_bytes());
    u32v(&mut out, clip.frames);
    u32v(&mut out, clip.dense_width);
    floats(&mut out, &[clip.rate, clip.begin]);
    i32v(&mut out, clip.dense.len() as i32);
    floats(&mut out, &clip.dense);
    i32v(&mut out, clip.constants.len() as i32);
    floats(&mut out, &clip.constants);
    floats(&mut out, &[0.0, 1.5, 0.0, 0.0, 0.0, 0.0]);
    for _ in 0..3 {
        i32v(&mut out, 0);
    }
    out.extend_from_slice(&[0; 11]);
    pad(&mut out);
    i32v(&mut out, clip.bindings.len() as i32);
    for binding in &clip.bindings {
        u32v(&mut out, binding.path);
        u32v(&mut out, binding.attribute);
        ptr(&mut out, 0);
        i32v(&mut out, binding.class);
        out.extend_from_slice(&[binding.custom, binding.pptr, 0, 0]);
    }
    i32v(&mut out, clip.mapping.len() as i32);
    for &reference in &clip.mapping {
        ptr(&mut out, reference);
    }
    out.extend_from_slice(&[0, 0]);
    pad(&mut out);
    i32v(&mut out, 0);
    pad(&mut out);
    out
}
fn serialized(objects: &[(i32, i64, Vec<u8>)]) -> Vec<u8> {
    let mut classes = Vec::new();
    for (class, _, _) in objects {
        if !classes.contains(class) {
            classes.push(*class);
        }
    }
    let mut metadata = b"2022.3.62f1\0".to_vec();
    i32v(&mut metadata, 13);
    metadata.push(0);
    i32v(&mut metadata, classes.len() as i32);
    for class in &classes {
        i32v(&mut metadata, *class);
        metadata.push(0);
        metadata.extend_from_slice(&(-1i16).to_le_bytes());
        metadata.extend_from_slice(&[0; 16]);
    }
    let mut data = Vec::new();
    let mut entries = Vec::new();
    for (class, id, payload) in objects {
        pad(&mut data);
        entries.push((
            *id,
            data.len(),
            payload.len(),
            classes.iter().position(|value| value == class).unwrap(),
        ));
        data.extend_from_slice(payload);
    }
    i32v(&mut metadata, entries.len() as i32);
    for (id, start, len, ty) in entries {
        pad(&mut metadata);
        metadata.extend_from_slice(&id.to_le_bytes());
        metadata.extend_from_slice(&(start as i64).to_le_bytes());
        u32v(&mut metadata, len as u32);
        i32v(&mut metadata, ty as i32);
    }
    for _ in 0..3 {
        i32v(&mut metadata, 0);
    }
    metadata.push(0);
    let offset = (48 + metadata.len()).next_multiple_of(16);
    let mut result = vec![0; 48];
    result[8..12].copy_from_slice(&22u32.to_be_bytes());
    result[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    result[24..32].copy_from_slice(&((offset + data.len()) as i64).to_be_bytes());
    result[32..40].copy_from_slice(&(offset as i64).to_be_bytes());
    result.extend_from_slice(&metadata);
    result.resize(offset, 0);
    result.extend_from_slice(&data);
    result
}
fn session(bytes: Vec<u8>) -> Session {
    Session::from_regions(
        [("CAB-modern".into(), Region::from_bytes(bytes))],
        &LoadOptions::default(),
    )
    .unwrap()
}
fn export(session: &Session) -> Result<String, String> {
    let key = session
        .assets()
        .iter()
        .find(|asset| asset.class_id == 74)
        .unwrap()
        .key;
    let root = tempfile::tempdir().unwrap();
    let report = session
        .export(&[key], root.path(), &ExportOptions::default())
        .unwrap();
    if !report.failed.is_empty() {
        assert!(report.written.is_empty());
        return Err(report.failed.join("\n"));
    }
    assert_eq!(report.written.len(), 1);
    Ok(fs::read_to_string(&report.written[0]).unwrap())
}
fn section<'a>(text: &'a str, field: &str, next: &str) -> &'a str {
    text.split_once(field)
        .unwrap()
        .1
        .split_once(next)
        .unwrap()
        .0
}

#[test]
fn modern_dense_streamed_and_constant_curves_export_real_unmirrored_yaml_samples() {
    let mut clip = Clip::dense_root();
    clip.stream = 4;
    streamed_frame(&mut clip.words, f32::NEG_INFINITY, &[0.0, 0.0, 0.0, 1.0]);
    streamed_frame(&mut clip.words, 0.0, &[0.0, 0.0, 0.0, 1.0]);
    streamed_frame(&mut clip.words, 1.0, &[0.0, 0.0, 0.70710677, 0.70710677]);
    streamed_frame(&mut clip.words, f32::INFINITY, &[]);
    clip.constants = vec![1.0, 2.0, 3.0, 1.0, 0.5];
    clip.bindings.insert(
        0,
        Binding {
            path: 0,
            attribute: 2,
            class: 4,
            custom: 4,
            pptr: 0,
        },
    );
    clip.bindings.extend([
        Binding {
            path: 0,
            attribute: 3,
            class: 4,
            custom: 4,
            pptr: 0,
        },
        Binding {
            path: 0,
            attribute: 0x7c5a22f6,
            class: 1,
            custom: 0,
            pptr: 0,
        },
        Binding {
            path: 0,
            attribute: 42,
            class: 95,
            custom: 8,
            pptr: 0,
        },
    ]);
    // IEEE/ZIP CRC32 of m_IsActive, independently encoded from the baseline CRC.

    let bytes = serialized(&[(74, 91, clip_bytes(&clip))]);
    let text = export(&session(bytes.clone())).unwrap();
    let positions = section(&text, "m_PositionCurves:", "m_ScaleCurves:");
    assert!(positions.contains("time: 0.25"));
    assert!(positions.contains("time: 0.75"));
    assert!(
        positions.contains("x: 1\n")
            && positions.contains("y: 2\n")
            && positions.contains("z: 3\n")
    );
    assert!(
        positions.contains("x: 4\n")
            && positions.contains("y: 5\n")
            && positions.contains("z: 6\n")
    );
    let rotations = section(&text, "m_RotationCurves:", "m_CompressedRotationCurves:");
    assert!(rotations.contains("time: 1"));
    assert!(rotations.contains("0.70710677"));
    let scales = section(&text, "m_ScaleCurves:", "m_FloatCurves:");
    assert!(scales.contains("time: 1.5"));
    let floats = section(&text, "m_FloatCurves:", "m_PPtrCurves:");
    assert!(floats.contains("m_IsActive"));
    assert!(floats.contains("Spine Front-Back"));
    assert!(floats.contains("value: 0.5"));
    assert!(text.contains("m_Compressed: 0"));
    if let Some(folder) = std::env::var_os("ASSETSTUDIO_ANIMATION_TEST_OUT") {
        let folder = std::path::PathBuf::from(folder);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("Modern.anim"), text).unwrap();
        fs::write(folder.join("modern.assets"), bytes).unwrap();
    }
}

#[test]
fn terminal_only_streams_preserve_dense_curves() {
    let mut clip = Clip::dense_root();
    clip.words = vec![f32::INFINITY.to_bits(), 0];
    let text = export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).unwrap();
    let positions = section(&text, "m_PositionCurves:", "m_ScaleCurves:");
    assert!(positions.contains("x: 1\n"));
    assert!(positions.contains("x: 4\n"));
    clip.stream = 1;
    assert!(export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).is_err());
}

#[test]
fn discrete_streams_use_minimum_finite_sentinel_and_offset_following_dense_curves() {
    let mut clip = Clip::dense_root();
    clip.discrete = 1;
    clip.bindings.insert(
        0,
        Binding {
            path: 0,
            attribute: 0,
            class: 212,
            custom: 23,
            pptr: 1,
        },
    );
    clip.mapping = vec![0, 0];
    streamed_frame(&mut clip.words, f32::MIN, &[0.0]);
    streamed_frame(&mut clip.words, 0.0, &[0.0]);
    streamed_frame(&mut clip.words, 1.0, &[1.0]);
    streamed_frame(&mut clip.words, f32::INFINITY, &[]);
    let text = export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).unwrap();
    let pointers = section(&text, "m_PPtrCurves:", "m_SampleRate:");
    assert!(pointers.contains("m_Sprite"));
    assert!(pointers.contains("time: 1\n"));
    assert_eq!(pointers.matches("fileID: 0").count(), 3); // Two values and the null script.
    let positions = section(&text, "m_PositionCurves:", "m_ScaleCurves:");
    assert!(positions.contains("x: 1\n"));
    assert!(positions.contains("x: 4\n"));
}

#[test]
fn invalid_modern_sample_shapes_are_failures_without_partial_anim_files() {
    for mutate in [0, 1, 2, 3, 4] {
        let mut clip = Clip::dense_root();
        match mutate {
            0 => clip.rate = 0.0,
            1 => clip.dense[0] = f32::NAN,
            2 => clip.dense.pop().map(|_| ()).unwrap(),
            3 => clip.bindings[0].attribute = 6,
            4 => clip.bindings[0].path = 123456,
            _ => {}
        }
        assert!(
            export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).is_err(),
            "mutation{mutate}"
        );
    }
}

#[test]
fn declared_stream_bindings_require_real_finite_samples() {
    for omitted in [None, Some(0), Some(2)] {
        let mut clip = Clip::dense_root();
        clip.dense.clear();
        clip.frames = 0;
        clip.dense_width = 0;
        clip.stream = 3;
        streamed_frame(&mut clip.words, f32::NEG_INFINITY, &[0.0; 3]);
        if let Some(omitted) = omitted {
            clip.words.push(0.0f32.to_bits());
            clip.words.push(2);
            for index in (0..3).filter(|index| *index != omitted) {
                clip.words.push(index);
                clip.words.extend_from_slice(&[0, 0, 0, 1.0f32.to_bits()]);
            }
        }
        streamed_frame(&mut clip.words, f32::INFINITY, &[]);
        let error = export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).unwrap_err();
        assert!(error.contains("no finite sample keys"), "{error}");
    }
}
fn game_object(name: &str, components: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    i32v(&mut out, components.len() as i32);
    for &id in components {
        ptr(&mut out, id);
    }
    i32v(&mut out, 0);
    string(&mut out, name);
    out
}
fn transform(owner: i64, parent: i64, children: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    ptr(&mut out, owner);
    floats(&mut out, &[0.0, 0.0, 0.0, 1.0]);
    floats(&mut out, &[0.0; 3]);
    floats(&mut out, &[1.0; 3]);
    i32v(&mut out, children.len() as i32);
    for &id in children {
        ptr(&mut out, id);
    }
    ptr(&mut out, parent);
    out
}
#[test]
fn bound_hierarchy_recovers_actual_child_paths_instead_of_hashed_placeholders() {
    let mut clip = Clip::dense_root();
    clip.bindings[0].path = 0xe3727b2d; // CRC32("Child")
    let mut animation = Vec::new();
    ptr(&mut animation, 1);
    animation.push(1);
    pad(&mut animation);
    ptr(&mut animation, 91);
    i32v(&mut animation, 1);
    ptr(&mut animation, 91);
    let input = serialized(&[
        (1, 1, game_object("Root", &[11, 81])),
        (4, 11, transform(1, 0, &[12])),
        (1, 2, game_object("Child", &[12])),
        (4, 12, transform(2, 11, &[])),
        (111, 81, animation),
        (74, 91, clip_bytes(&clip)),
    ]);
    let text = export(&session(input)).unwrap();
    assert!(section(&text, "m_PositionCurves:", "m_ScaleCurves:").contains("path: \"Child\""));
    assert!(!text.contains("path_"));
}

#[test]
fn modern_reference_curves_preserve_nulls_and_never_silently_erase_missing_guids() {
    let mut clip = Clip::dense_root();
    clip.frames = 0;
    clip.dense_width = 0;
    clip.dense.clear();
    clip.constants = vec![0.0];
    clip.bindings = vec![Binding {
        path: 0,
        attribute: 0,
        class: 212,
        custom: 23,
        pptr: 1,
    }];
    clip.mapping = vec![0];
    let text = export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).unwrap();
    let pointers = section(&text, "m_PPtrCurves:", "m_SampleRate:");
    assert!(pointers.contains("m_Sprite"));
    assert!(pointers.contains("fileID: 0"));
    assert!(pointers.contains("time: 1.5"));
    clip.mapping = vec![42];
    let error = export(&session(serialized(&[(74, 91, clip_bytes(&clip))]))).unwrap_err();
    assert!(error.contains("GUID"));
}
