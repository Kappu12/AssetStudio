// Fixture builders adapted under MIT from seiunx-dev/unity-rs commit b3e509ed.
// Copyright (c)2016 Radu; (c)2016-2022 Perfare; (c)2021-2025 aelurum.
// These synthetic layouts are regression evidence, not an independent game oracle.

use assetstudio_core::{AssetKey, LoadOptions, ModelFormat, Preview, Session};
use std::fs;
use unity_rs_core::source::Region;

fn objects_file(objects: &[(i32, i64, Vec<u8>)]) -> Vec<u8> {
    let mut classes: Vec<i32> = Vec::new();
    for (class_id, _, _) in objects {
        if !classes.contains(class_id) {
            classes.push(*class_id);
        }
    }
    let mut metadata = Vec::new();
    metadata.extend_from_slice(b"2022.3.62f1\0");
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&i32::try_from(classes.len()).unwrap().to_le_bytes());
    for class_id in &classes {
        metadata.extend_from_slice(&class_id.to_le_bytes());
        metadata.push(0);
        metadata.extend_from_slice(&(-1_i16).to_le_bytes());
        // A MonoBehaviour record carries a script hash before the type hash.
        if *class_id == 114 {
            metadata.extend_from_slice(&[0; 16]);
        }
        metadata.extend_from_slice(&[0; 16]);
    }

    let mut data = Vec::new();
    let mut records = Vec::new();
    for (class_id, path_id, payload) in objects {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        records.push((
            *path_id,
            i64::try_from(data.len()).unwrap(),
            u32::try_from(payload.len()).unwrap(),
            i32::try_from(classes.iter().position(|value| value == class_id).unwrap()).unwrap(),
        ));
        data.extend_from_slice(payload);
    }
    metadata.extend_from_slice(&i32::try_from(records.len()).unwrap().to_le_bytes());
    for (path_id, offset, size, type_index) in records {
        while !(48 + metadata.len()).is_multiple_of(4) {
            metadata.push(0);
        }
        metadata.extend_from_slice(&path_id.to_le_bytes());
        metadata.extend_from_slice(&offset.to_le_bytes());
        metadata.extend_from_slice(&size.to_le_bytes());
        metadata.extend_from_slice(&type_index.to_le_bytes());
    }
    for _ in 0..3 {
        metadata.extend_from_slice(&0_i32.to_le_bytes());
    }
    metadata.push(0);

    let data_offset = (48 + metadata.len()).next_multiple_of(16);
    let file_size = data_offset + data.len();
    let mut output = vec![0_u8; 48];
    output[8..12].copy_from_slice(&22_u32.to_be_bytes());
    output[20..24].copy_from_slice(&u32::try_from(metadata.len()).unwrap().to_be_bytes());
    output[24..32].copy_from_slice(&i64::try_from(file_size).unwrap().to_be_bytes());
    output[32..40].copy_from_slice(&i64::try_from(data_offset).unwrap().to_be_bytes());
    output.extend_from_slice(&metadata);
    output.resize(data_offset, 0);
    output.extend_from_slice(&data);
    output
}

fn push_aligned_string(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
    output.extend_from_slice(value.as_bytes());
    pad(output);
}

fn pad(output: &mut Vec<u8>) {
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
}

fn model_game_object(components: &[i64]) -> Vec<u8> {
    model_game_object_named(components, "root")
}
fn model_game_object_named(components: &[i64], name: &str) -> Vec<u8> {
    let mut output = Vec::new();
    push_i32(&mut output, i32::try_from(components.len()).unwrap());
    for component in components {
        push_pptr(&mut output, *component);
    }
    push_i32(&mut output, 0);
    push_aligned_string(&mut output, name);
    output
}

fn model_transform() -> Vec<u8> {
    let mut output = Vec::new();
    push_pptr(&mut output, 1);
    push_floats(&mut output, &[0.0, 0.0, 0.0, 1.0]);
    push_floats(&mut output, &[2.0, 3.0, 4.0]);
    push_floats(&mut output, &[1.0, 1.0, 1.0]);
    push_i32(&mut output, 0);
    push_pptr(&mut output, 0);
    output
}

fn model_mesh_filter() -> Vec<u8> {
    let mut output = Vec::new();
    push_pptr(&mut output, 1);
    push_pptr(&mut output, 51);
    output
}

fn model_renderer() -> Vec<u8> {
    model_renderer_with_batch(0, 0)
}
fn model_renderer_with_batch(first: u16, count: u16) -> Vec<u8> {
    let mut output = Vec::new();
    push_pptr(&mut output, 1);
    output.extend_from_slice(&[1, 2, 1, 0, 0, 0, 0, 0, 0, 0]);
    pad(&mut output);
    output.extend_from_slice(&u32::MAX.to_le_bytes());
    push_i32(&mut output, 0);
    output.extend_from_slice(&[0_u8; 36]);
    push_i32(&mut output, 1);
    push_pptr(&mut output, 61);
    output.extend_from_slice(&first.to_le_bytes());
    output.extend_from_slice(&count.to_le_bytes());
    for _ in 0..3 {
        push_pptr(&mut output, 0);
    }
    output.extend_from_slice(&[0_u8; 8]);
    pad(&mut output);
    output
}

fn model_mesh() -> Vec<u8> {
    let mut output = Vec::new();
    push_aligned_string(&mut output, "tri");
    push_i32(&mut output, 1);
    for value in [0_u32, 3, 0, 0, 0, 3] {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output.extend_from_slice(&[0_u8; 24]);
    for _ in 0..3 {
        push_i32(&mut output, 0);
    }
    output.extend_from_slice(&0_u32.to_le_bytes());
    for _ in 0..5 {
        push_i32(&mut output, 0);
    }
    output.extend_from_slice(&[0, 1, 0, 0]);
    pad(&mut output);
    push_i32(&mut output, 0);
    push_i32(&mut output, 6);
    for index in 0..3_u16 {
        output.extend_from_slice(&index.to_le_bytes());
    }
    pad(&mut output);

    // Vertex data: three positions in one stream.
    output.extend_from_slice(&3_u32.to_le_bytes());
    push_i32(&mut output, 5);
    output.extend_from_slice(&[0, 0, 0, 3]);
    for _ in 0..4 {
        output.extend_from_slice(&[0_u8; 4]);
    }
    push_i32(&mut output, 36);
    for vertex in [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        push_floats(&mut output, &vertex);
    }
    pad(&mut output);

    // The compressed-mesh block and the tail every version carries.
    for _ in 0..4 {
        push_empty_packed_float(&mut output);
    }
    for _ in 0..3 {
        push_empty_packed_int(&mut output);
    }
    push_empty_packed_float(&mut output);
    for _ in 0..2 {
        push_empty_packed_int(&mut output);
    }
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&[0_u8; 24]);
    for _ in 0..3 {
        push_i32(&mut output, 0);
    }
    pad(&mut output);
    push_i32(&mut output, 0);
    pad(&mut output);
    output.extend_from_slice(&[0_u8; 8]);
    pad(&mut output);
    output.extend_from_slice(&0_i64.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    push_aligned_string(&mut output, "");
    output
}

fn push_empty_packed_float(output: &mut Vec<u8>) {
    output.extend_from_slice(&0_u32.to_le_bytes());
    push_floats(output, &[0.0, 0.0]);
    push_i32(output, 0);
    pad(output);
    output.push(0);
    pad(output);
}

fn push_empty_packed_int(output: &mut Vec<u8>) {
    output.extend_from_slice(&0_u32.to_le_bytes());
    push_i32(output, 0);
    pad(output);
    output.push(0);
    pad(output);
}

fn push_pptr(output: &mut Vec<u8>, path_id: i64) {
    push_i32(output, 0);
    output.extend_from_slice(&path_id.to_le_bytes());
}

fn push_floats(output: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        output.extend_from_slice(&value.to_le_bytes());
    }
}

fn push_i32(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn texture_object(name: &str, data: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    push_aligned_string(&mut output, name);
    output.extend_from_slice(&0_i32.to_le_bytes());
    output.push(0);
    output.push(0);
    align(&mut output, 4);
    output.extend_from_slice(&1_i32.to_le_bytes());
    output.extend_from_slice(&1_i32.to_le_bytes());
    output.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    output.extend_from_slice(&0_i32.to_le_bytes());
    output.extend_from_slice(&4_i32.to_le_bytes());
    output.extend_from_slice(&1_i32.to_le_bytes());
    output.extend_from_slice(&[0, 0, 0]);
    align(&mut output, 4);
    push_aligned_string(&mut output, "");
    output.push(0);
    align(&mut output, 4);
    for value in [0_i32, 1, 2] {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output.extend_from_slice(&[0_u8; 24]);
    for _ in 0..3 {
        output.extend_from_slice(&0_i32.to_le_bytes());
    }
    align(&mut output, 4);
    output.extend_from_slice(&i32::try_from(data.len()).unwrap().to_le_bytes());
    output.extend_from_slice(data);
    output
}

fn align(bytes: &mut Vec<u8>, alignment: usize) {
    while !bytes.len().is_multiple_of(alignment) {
        bytes.push(0);
    }
}
fn material_object() -> Vec<u8> {
    let mut out = Vec::new();
    push_aligned_string(&mut out, "Material");
    push_pptr(&mut out, 0);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0); // Valid and invalid keywords.
    push_i32(&mut out, 0);
    out.push(0);
    pad(&mut out); // Lightmap and instancing.
    push_i32(&mut out, -1);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0);
    push_i32(&mut out, 1);
    push_aligned_string(&mut out, "_MainTex");
    push_pptr(&mut out, 71);
    push_floats(&mut out, &[1.0, 1.0, 0.0, 0.0]);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0);
    push_i32(&mut out, 1);
    push_aligned_string(&mut out, "_Color");
    push_floats(&mut out, &[1.0, 1.0, 1.0, 1.0]);
    out
}
fn animation_component() -> Vec<u8> {
    let mut out = Vec::new();
    push_pptr(&mut out, 1);
    out.push(1);
    pad(&mut out);
    push_pptr(&mut out, 91);
    push_i32(&mut out, 1);
    push_pptr(&mut out, 91);
    out
}
fn clip_object() -> Vec<u8> {
    clip_object_with_constant(false)
}
fn clip_object_with_constant(constant: bool) -> Vec<u8> {
    // Explicit legacy position curve for the root, plus an empty modern muscle block.
    let mut out = Vec::new();
    push_aligned_string(&mut out, "Move");
    out.extend_from_slice(&[1, 0, 1]);
    pad(&mut out);
    for _ in 0..3 {
        push_i32(&mut out, 0);
    } // Rotation, packed rotation, Euler.
    push_i32(&mut out, 1);
    push_i32(&mut out, 2);
    for (time, value) in [(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 2.0, 3.0])] {
        push_floats(&mut out, &[time]);
        push_floats(&mut out, &value);
        push_floats(&mut out, &[0.0; 6]);
        push_i32(&mut out, 0);
        push_floats(&mut out, &[1.0 / 3.0; 6]);
    }
    for value in [2, 2, 4] {
        push_i32(&mut out, value);
    }
    push_aligned_string(&mut out, "");
    for _ in 0..3 {
        push_i32(&mut out, 0);
    } // Scale, float, pointer curves.
    push_floats(&mut out, &[30.0]);
    push_i32(&mut out, 0);
    push_floats(&mut out, &[0.0; 6]);
    push_i32(&mut out, 0);
    push_floats(&mut out, &[0.0; 10]);
    push_floats(&mut out, &[0.0; 7]);
    push_i32(&mut out, 0);
    for _ in 0..2 {
        push_floats(&mut out, &[0.0; 10]);
        push_i32(&mut out, 0);
        push_floats(&mut out, &[0.0; 4]);
    }
    push_i32(&mut out, 0);
    push_i32(&mut out, 0);
    for _ in 0..4 {
        push_floats(&mut out, &[0.0; 10]);
    }
    push_floats(&mut out, &[0.0; 3]);
    push_i32(&mut out, 0); // Speed, streamed data.
    out.extend_from_slice(&[0; 4]); // Split streamed curve counts.
    push_i32(&mut out, 0);
    push_i32(&mut out, 0);
    push_floats(&mut out, &[30.0, 0.0]);
    push_i32(&mut out, 0); // Empty dense array.
    push_i32(&mut out, i32::from(constant));
    if constant {
        push_floats(&mut out, &[42.0]);
    }
    push_floats(&mut out, &[0.0; 6]);
    for _ in 0..3 {
        push_i32(&mut out, 0);
    }
    out.extend_from_slice(&[0; 11]);
    pad(&mut out);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0); // Binding and PPtr maps.
    out.extend_from_slice(&[0, 0]);
    pad(&mut out);
    push_i32(&mut out, 0);
    pad(&mut out);
    out
}
fn model_session() -> Session {
    model_session_with_clip(clip_object())
}
fn model_session_with_clip(clip: Vec<u8>) -> Session {
    let preserve_fixture = clip == clip_object();
    let data = objects_file(&[
        (1, 1, model_game_object(&[11, 21, 31, 81])),
        (4, 11, model_transform()),
        (33, 21, model_mesh_filter()),
        (23, 31, model_renderer()),
        (43, 51, model_mesh()),
        (21, 61, material_object()),
        (28, 71, texture_object("color", &[255, 0, 0, 255])),
        (111, 81, animation_component()),
        (74, 91, clip),
    ]);
    if preserve_fixture && let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
        let folder = std::path::PathBuf::from(path);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("model.assets"), &data).unwrap();
    }
    Session::from_regions(
        [("CAB-model".into(), Region::from_bytes(data))],
        &LoadOptions::default(),
    )
    .unwrap()
}

#[test]
fn scene_mesh_preview_and_obj_material_textures_are_consistent() {
    let session = model_session();
    assert_eq!(session.scene().unwrap().len(), 1);
    let preview = session
        .preview(AssetKey {
            file_index: 0,
            path_id: 51,
        })
        .unwrap();
    let Preview::Mesh { vertices, indices } = preview else {
        panic!("mesh preview missing")
    };
    assert_eq!(vertices.len(), 3);
    assert_eq!(indices.len(), 3);
    let dir = tempfile::tempdir().unwrap();
    let report = session
        .export_scene(dir.path(), ModelFormat::Obj, &[])
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let obj = report
        .written
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "obj"))
        .unwrap();
    let text = fs::read_to_string(obj).unwrap();
    let points = text
        .lines()
        .filter(|line| line.starts_with("v "))
        .map(|line| {
            line.split_whitespace()
                .skip(1)
                .map(|number| number.parse::<f32>().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(points.len(), 3);
    assert_eq!(points[0], [-2.0, 3.0, 4.0]);
    for line in text.lines().filter(|line| line.starts_with("f ")) {
        let indices = line
            .split_whitespace()
            .skip(1)
            .map(|s| s.split('/').next().unwrap().parse::<usize>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(indices.len(), 3);
        assert!(indices.iter().all(|&i| (1..=3).contains(&i)));
    }
    let mtl_name = text
        .lines()
        .find_map(|line| line.strip_prefix("mtllib "))
        .unwrap();
    let mtl = fs::read_to_string(obj.parent().unwrap().join(mtl_name)).unwrap();
    let texture_name = mtl
        .lines()
        .find_map(|line| line.strip_prefix("map_Kd "))
        .unwrap();
    let png = fs::read(obj.parent().unwrap().join(texture_name)).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    if let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
        let folder = std::path::PathBuf::from(path).join("obj");
        fs::create_dir_all(&folder).unwrap();
        for path in &report.written {
            fs::copy(path, folder.join(path.file_name().unwrap())).unwrap();
        }
    }
    let again = session
        .export_scene(dir.path(), ModelFormat::Obj, &[])
        .unwrap();
    assert!(!again.skipped.is_empty());
    assert!(again.written.is_empty());
}

#[test]
fn ascii_and_binary_fbx_include_materials_and_animation_tracks() {
    let session = model_session();
    for format in [ModelFormat::FbxAscii, ModelFormat::FbxBinary] {
        let dir = tempfile::tempdir().unwrap();
        let report = session
            .export_scene(
                dir.path(),
                format,
                &[AssetKey {
                    file_index: 0,
                    path_id: 1,
                }],
            )
            .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let model = report
            .written
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "fbx"))
            .unwrap();
        let data = fs::read(model).unwrap();
        assert_eq!(
            data.windows(b"KeyTime".len())
                .filter(|window| *window == b"KeyTime")
                .count(),
            3,
            "each FBX encoding must contain three actual root translation curves"
        );
        assert_eq!(
            data.windows(b"KeyValueFloat".len())
                .filter(|window| *window == b"KeyValueFloat")
                .count(),
            3
        );
        if format == ModelFormat::FbxBinary {
            assert!(data.starts_with(b"Kaydara FBX Binary  \0\x1a\0"));
        } else {
            let text = std::str::from_utf8(&data).unwrap();
            assert!(text.contains("AnimationStack"));
            assert!(text.contains("KeyTime:"));
            assert!(text.contains("46186158000"));
            assert!(text.contains("Material"));
            assert!(text.contains("Move"));
        }
        assert!(
            report
                .written
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "png"))
        );
        if let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
            let folder = std::path::PathBuf::from(path).join(if format == ModelFormat::FbxBinary {
                "binary"
            } else {
                "ascii"
            });
            fs::create_dir_all(&folder).unwrap();
            for path in &report.written {
                fs::copy(path, folder.join(path.file_name().unwrap())).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let invalid = session
        .export_scene(
            dir.path(),
            ModelFormat::Obj,
            &[AssetKey {
                file_index: 0,
                path_id: 71,
            }],
        )
        .unwrap();
    assert_eq!(invalid.failed.len(), 1);
    assert!(invalid.written.is_empty());
}

#[test]
fn explicit_legacy_animation_is_exported_as_unity_yaml() {
    let session = model_session();
    let dir = tempfile::tempdir().unwrap();
    let report = session
        .export(
            &[AssetKey {
                file_index: 0,
                path_id: 91,
            }],
            dir.path(),
            &assetstudio_core::ExportOptions::default(),
        )
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.written.len(), 1);
    let data = fs::read_to_string(&report.written[0]).unwrap();
    assert!(data.starts_with(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!74 &7400000\nAnimationClip:\n"
    ));
    assert!(data.contains("m_PositionCurves:"));
    assert!(data.contains("m_AnimationClipSettings:"));
    assert!(data.contains("weightedMode: 0"));
    if let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
        fs::copy(
            &report.written[0],
            std::path::PathBuf::from(path).join("Move.anim"),
        )
        .unwrap();
    }
}

fn animation_session(payload: Vec<u8>) -> Session {
    Session::from_regions(
        [(
            "CAB-clip".into(),
            Region::from_bytes(objects_file(&[(74, 91, payload)])),
        )],
        &LoadOptions::default(),
    )
    .unwrap()
}
fn push_bits(bytes: &mut Vec<u8>, position: &mut usize, value: u32, bits: u8) {
    for bit in 0..bits {
        let index = *position / 8;
        if bytes.len() <= index {
            bytes.push(0);
        }
        bytes[index] |= (((value >> bit) & 1) as u8) << (*position % 8);
        *position += 1;
    }
}
fn packed_clip_object(path: &str) -> (Vec<u8>, Vec<u8>) {
    let mut curve = Vec::new();
    push_aligned_string(&mut curve, path);
    push_i32(&mut curve, 2);
    push_i32(&mut curve, 2);
    curve.extend_from_slice(&[0, 50]);
    pad(&mut curve);
    curve.push(8);
    pad(&mut curve);
    let mut quaternions = Vec::new();
    let mut bit = 0;
    for last in [512, 873] {
        push_bits(&mut quaternions, &mut bit, 3, 3);
        push_bits(&mut quaternions, &mut bit, 256, 9);
        push_bits(&mut quaternions, &mut bit, 512, 10);
        push_bits(&mut quaternions, &mut bit, last, 10);
    }
    push_i32(&mut curve, 2);
    push_i32(&mut curve, quaternions.len() as i32);
    curve.extend_from_slice(&quaternions);
    pad(&mut curve);
    push_i32(&mut curve, 0);
    push_floats(&mut curve, &[0.0, 0.0]);
    push_i32(&mut curve, 0);
    curve.push(0);
    pad(&mut curve);
    push_i32(&mut curve, 0);
    push_i32(&mut curve, 0);
    let mut clip = clip_object();
    clip[9] = 1;
    clip[16..20].copy_from_slice(&1_i32.to_le_bytes());
    clip.splice(20..20, curve);
    (clip, quaternions)
}
#[test]
fn packed_legacy_rotation_keeps_its_exact_bitstream_in_anim_yaml() {
    let (clip, quaternions) = packed_clip_object("Arm");
    let session = animation_session(clip);
    let root = tempfile::tempdir().unwrap();
    let report = session
        .export(
            &[session.assets()[0].key],
            root.path(),
            &assetstudio_core::ExportOptions::default(),
        )
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let text = fs::read_to_string(&report.written[0]).unwrap();
    assert!(text.contains("m_Data: \"0032\""));
    assert!(text.contains("path: \"Arm\""));
    let expected = quaternions
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert!(text.contains(&expected));
    if let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
        fs::copy(
            &report.written[0],
            std::path::PathBuf::from(path).join("Packed.anim"),
        )
        .unwrap();
    }
}
#[test]
fn unexpanded_muscle_curves_fail_instead_of_writing_empty_anim() {
    let session = animation_session(clip_object_with_constant(true));
    let root = tempfile::tempdir().unwrap();
    let report = session
        .export(
            &[session.assets()[0].key],
            root.path(),
            &assetstudio_core::ExportOptions::default(),
        )
        .unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].contains("compressed muscle"));
}

#[test]
fn shared_legacy_clip_animates_each_game_object_owner() {
    fn owner(mut bytes: Vec<u8>) -> Vec<u8> {
        bytes[4..12].copy_from_slice(&2_i64.to_le_bytes());
        bytes
    }
    let data = objects_file(&[
        (1, 1, model_game_object(&[11, 21, 31, 81])),
        (4, 11, model_transform()),
        (33, 21, model_mesh_filter()),
        (23, 31, model_renderer()),
        (1, 2, model_game_object_named(&[12, 22, 32, 82], "other")),
        (4, 12, owner(model_transform())),
        (33, 22, owner(model_mesh_filter())),
        (23, 32, owner(model_renderer())),
        (43, 51, model_mesh()),
        (21, 61, material_object()),
        (28, 71, texture_object("color", &[255, 0, 0, 255])),
        (111, 81, animation_component()),
        (111, 82, owner(animation_component())),
        (74, 91, clip_object()),
    ]);
    let session = Session::from_regions(
        [("CAB-shared".into(), Region::from_bytes(data))],
        &LoadOptions::default(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let report = session
        .export_scene(root.path(), ModelFormat::FbxAscii, &[])
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let model = report
        .written
        .iter()
        .find(|file| file.extension().is_some_and(|extension| extension == "fbx"))
        .unwrap();
    let text = fs::read_to_string(model).unwrap();
    assert_eq!(
        text.matches("KeyTime:").count(),
        6,
        "one XYZ translation track is required for each owner"
    );
    if let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
        fs::copy(model, std::path::PathBuf::from(path).join("Shared.fbx")).unwrap();
    }
}

#[test]
fn static_models_skip_unsupported_attached_animation_and_optional_textures() {
    let mut broken = Vec::new();
    push_aligned_string(&mut broken, "Broken Clip");
    broken.push(1);
    let session = model_session_with_clip(broken);
    let animated = tempfile::tempdir().unwrap();
    assert!(
        session
            .export_scene(animated.path(), ModelFormat::FbxBinary, &[])
            .is_err()
    );
    for format in [ModelFormat::FbxAscii, ModelFormat::FbxBinary] {
        let output = tempfile::tempdir().unwrap();
        let options = assetstudio_core::ModelExportOptions {
            include_animations: false,
            include_textures: false,
        };
        let report = session
            .export_scene_with_options(output.path(), format, &[], &options)
            .unwrap();
        assert!(report.failed.is_empty());
        assert_eq!(report.written.len(), 1);
        assert!(
            report.written[0]
                .extension()
                .is_some_and(|extension| extension == "fbx")
        );
    }
    let output = tempfile::tempdir().unwrap();
    let report = session
        .export_with_model_options(
            &[AssetKey {
                file_index: 0,
                path_id: 1,
            }],
            output.path(),
            &assetstudio_core::ExportOptions::default(),
            &assetstudio_core::ModelExportOptions {
                include_animations: false,
                include_textures: true,
            },
        )
        .unwrap();
    assert!(report.failed.is_empty());
    assert!(
        report
            .written
            .iter()
            .any(|path| path.extension().is_some_and(|extension| extension == "png"))
    );
}

fn explicit_root_rotation_clip(quaternion: bool) -> Vec<u8> {
    let mut curve = Vec::new();
    push_i32(&mut curve, 2);
    for final_key in [false, true] {
        push_floats(&mut curve, &[if final_key { 1.0 } else { 0.0 }]);
        if quaternion {
            let (z, w) = if final_key {
                (
                    std::f32::consts::FRAC_1_SQRT_2,
                    std::f32::consts::FRAC_1_SQRT_2,
                )
            } else {
                (0.0, 1.0)
            };
            push_floats(&mut curve, &[0.0, 0.0, z, w]);
            push_floats(&mut curve, &[0.0; 8]);
            push_i32(&mut curve, 0);
            push_floats(&mut curve, &[1.0 / 3.0; 8]);
        } else {
            push_floats(&mut curve, &[0.0, 0.0, if final_key { 90.0 } else { 0.0 }]);
            push_floats(&mut curve, &[0.0; 6]);
            push_i32(&mut curve, 0);
            push_floats(&mut curve, &[1.0 / 3.0; 6]);
        }
    }
    for value in [2, 2, 4] {
        push_i32(&mut curve, value);
    }
    push_aligned_string(&mut curve, "");
    let mut clip = clip_object();
    let count = if quaternion { 12 } else { 20 };
    clip[count..count + 4].copy_from_slice(&1_i32.to_le_bytes());
    clip.splice(count + 4..count + 4, curve);
    clip
}
#[test]
fn packed_quaternion_and_euler_root_tracks_reach_real_fbx_curves() {
    for (name, clip) in [
        ("PackedRoot", packed_clip_object("").0),
        ("QuaternionRoot", explicit_root_rotation_clip(true)),
        ("EulerRoot", explicit_root_rotation_clip(false)),
    ] {
        let session = model_session_with_clip(clip);
        let root = tempfile::tempdir().unwrap();
        let report = session
            .export_scene(root.path(), ModelFormat::FbxAscii, &[])
            .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let file = report
            .written
            .iter()
            .find(|path| path.extension().is_some_and(|extension| extension == "fbx"))
            .unwrap();
        let text = fs::read_to_string(file).unwrap();
        assert_eq!(
            text.matches("KeyTime:").count(),
            6,
            "{name} must retain XYZ rotation and translation keys"
        );
        if let Some(path) = std::env::var_os("ASSETSTUDIO_MODEL_TEST_OUT") {
            fs::copy(
                file,
                std::path::PathBuf::from(path).join(format!("{name}.fbx")),
            )
            .unwrap();
        }
    }
}

#[test]
fn game_object_and_animator_static_previews_use_world_geometry_without_animations() {
    let mut broken = Vec::new();
    push_aligned_string(&mut broken, "Broken Clip");
    broken.push(1);
    let session = model_session_with_clip(broken);
    let preview = session
        .preview(AssetKey {
            file_index: 0,
            path_id: 1,
        })
        .unwrap();
    let Preview::Mesh { vertices, indices } = preview else {
        panic!("GameObject preview is not geometry")
    };
    assert_eq!(vertices.len(), 3);
    assert_eq!(indices.len(), 3);
    assert_eq!(vertices[0], [2.0, 3.0, 4.0]);
    let mut animator = Vec::new();
    push_pptr(&mut animator, 1);
    animator.push(1);
    pad(&mut animator);
    push_pptr(&mut animator, 0);
    push_pptr(&mut animator, 0);
    let data = objects_file(&[
        (1, 1, model_game_object(&[11, 21, 31, 81])),
        (4, 11, model_transform()),
        (33, 21, model_mesh_filter()),
        (23, 31, model_renderer()),
        (43, 51, model_mesh()),
        (21, 61, material_object()),
        (28, 71, texture_object("color", &[255, 0, 0, 255])),
        (95, 81, animator),
    ]);
    let session = Session::from_regions(
        [("CAB-animator-preview".into(), Region::from_bytes(data))],
        &LoadOptions::default(),
    )
    .unwrap();
    let Preview::Mesh { vertices, indices } = session
        .preview(AssetKey {
            file_index: 0,
            path_id: 81,
        })
        .unwrap()
    else {
        panic!("Animator preview is not geometry")
    };
    assert_eq!(vertices.len(), 3);
    assert_eq!(indices.len(), 3);
    assert_eq!(vertices[0], [2.0, 3.0, 4.0]);
}

#[test]
fn static_batches_fail_explicitly_before_preview_or_model_publication() {
    let data = objects_file(&[
        (1, 1, model_game_object(&[11, 21, 31])),
        (4, 11, model_transform()),
        (33, 21, model_mesh_filter()),
        (23, 31, model_renderer_with_batch(0, 1)),
        (43, 51, model_mesh()),
        (21, 61, material_object()),
        (28, 71, texture_object("color", &[255, 0, 0, 255])),
    ]);
    let session = Session::from_regions(
        [("CAB-static-batch".into(), Region::from_bytes(data))],
        &LoadOptions::default(),
    )
    .unwrap();
    let error = session
        .preview(AssetKey {
            file_index: 0,
            path_id: 1,
        })
        .unwrap_err()
        .to_string();
    assert!(
        error.to_ascii_lowercase().contains("static batch"),
        "{error}"
    );
    for format in [
        ModelFormat::FbxAscii,
        ModelFormat::FbxBinary,
        ModelFormat::Obj,
    ] {
        let output = tempfile::tempdir().unwrap();
        let error = session
            .export_scene(output.path(), format, &[])
            .unwrap_err()
            .to_string();
        assert!(
            error.to_ascii_lowercase().contains("static batch"),
            "{error}"
        );
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    }
    let output = tempfile::tempdir().unwrap();
    let report = session
        .export(
            &[AssetKey {
                file_index: 0,
                path_id: 1,
            }],
            output.path(),
            &assetstudio_core::ExportOptions::default(),
        )
        .unwrap();
    assert!(report.written.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.failed[0]
            .to_ascii_lowercase()
            .contains("static batch")
    );
}
