//! End-to-end synthetic game container/variant fixtures. Wire fields follow the
//! baseline C# formats, not mocked Session responses; current live builds are
//! separate acceptance work.
use assetstudio_core::{ExportMode, ExportOptions, LoadOptions, Preview, Session};
use std::fs;

fn pad(data: &mut Vec<u8>, alignment: usize) {
    data.resize(data.len().next_multiple_of(alignment), 0);
}
fn string(data: &mut Vec<u8>, text: &str) {
    data.extend_from_slice(&(text.len() as i32).to_le_bytes());
    data.extend_from_slice(text.as_bytes());
    pad(data, 4);
}
fn text(name: &str, payload: &str) -> Vec<u8> {
    let mut data = Vec::new();
    string(&mut data, name);
    string(&mut data, payload);
    data
}
fn file(objects: &[(i32, i64, Vec<u8>)], platform: i32) -> Vec<u8> {
    let mut classes = Vec::new();
    for (class, _, _) in objects {
        if !classes.contains(class) {
            classes.push(*class);
        }
    }
    let mut meta = b"2022.3.62f1\0".to_vec();
    meta.extend_from_slice(&platform.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(classes.len() as i32).to_le_bytes());
    for class in &classes {
        meta.extend_from_slice(&class.to_le_bytes());
        meta.push(0);
        meta.extend_from_slice(&(-1_i16).to_le_bytes());
        meta.extend_from_slice(&[0; 16]);
    }
    meta.extend_from_slice(&(objects.len() as i32).to_le_bytes());
    let mut data = Vec::new();
    for (class, id, payload) in objects {
        pad(&mut data, 4);
        while !(48 + meta.len()).is_multiple_of(4) {
            meta.push(0);
        }
        meta.extend_from_slice(&id.to_le_bytes());
        meta.extend_from_slice(&(data.len() as i64).to_le_bytes());
        meta.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        meta.extend_from_slice(
            &(classes.iter().position(|item| item == class).unwrap() as i32).to_le_bytes(),
        );
        data.extend_from_slice(payload);
    }
    meta.extend_from_slice(&[0; 12]);
    meta.push(0);
    let offset = (48 + meta.len()).next_multiple_of(16);
    let mut output = vec![0; 48];
    output[8..12].copy_from_slice(&22_u32.to_be_bytes());
    output[20..24].copy_from_slice(&(meta.len() as u32).to_be_bytes());
    output[24..32].copy_from_slice(&((offset + data.len()) as u64).to_be_bytes());
    output[32..40].copy_from_slice(&(offset as u64).to_be_bytes());
    output.extend_from_slice(&meta);
    output.resize(offset, 0);
    output.extend_from_slice(&data);
    output
}
fn bundle(payload: &[u8], name: &str, block_code: u16) -> (Vec<u8>, usize) {
    let mut compressed = lz4_flex::block::compress(payload);
    if block_code == 4 {
        // Literal-only swapped-token block avoids sharing a token walker with the reader.
        compressed = vec![0x0f];
        let mut length = payload.len() - 15;
        while length >= 255 {
            compressed.push(255);
            length -= 255;
        }
        compressed.push(length as u8);
        compressed.extend_from_slice(payload);
    }
    let mut info = vec![0; 16];
    info.extend_from_slice(&1_u32.to_be_bytes());
    info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    info.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    info.extend_from_slice(&block_code.to_be_bytes());
    info.extend_from_slice(&1_u32.to_be_bytes());
    info.extend_from_slice(&0_u64.to_be_bytes());
    info.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    info.extend_from_slice(&0_u32.to_be_bytes());
    info.extend_from_slice(name.as_bytes());
    info.push(0);
    let table = lz4_flex::block::compress(&info);
    let mut data = b"UnityFS\0".to_vec();
    data.extend_from_slice(&7_u32.to_be_bytes());
    data.extend_from_slice(b"5.x.x\0");
    data.extend_from_slice(b"2022.3.62f1\0");
    let size = data.len();
    data.extend_from_slice(&0_u64.to_be_bytes());
    data.extend_from_slice(&(table.len() as u32).to_be_bytes());
    data.extend_from_slice(&(info.len() as u32).to_be_bytes());
    data.extend_from_slice(&0x45_u32.to_be_bytes());
    pad(&mut data, 16);
    data.extend_from_slice(&table);
    data.extend_from_slice(&compressed);
    let length = data.len() as u64;
    data[size..size + 8].copy_from_slice(&length.to_be_bytes());
    (data, size)
}
fn naraka(mut data: Vec<u8>, size: usize) -> Vec<u8> {
    data[..7].copy_from_slice(&[0x15, 0x1e, 0x1c, 0x0d, 0x0d, 0x23, 0x21]);
    for index in [size + 8, size + 12] {
        let n = u32::from_be_bytes(data[index..index + 4].try_into().unwrap()) + 0xca;
        data[index..index + 4].copy_from_slice(&n.to_be_bytes());
    }
    data
}
fn lnd(data: &[u8]) -> Vec<u8> {
    let vector = [0x35, 0x6b, 0x05, 0];
    data.iter()
        .enumerate()
        .map(|(index, byte)| {
            byte ^ ((index % 16) as u8).wrapping_mul(17).wrapping_add(3) ^ vector[index / 16 % 4]
        })
        .collect()
}
struct Rng {
    seed: u32,
    state: [u32; 4],
}
impl Rng {
    fn new(seed: u32) -> Self {
        let mut state = [seed; 4];
        for index in 1..4 {
            state[index] = 0x6c078965u32.wrapping_mul(state[index - 1]).wrapping_add(1);
        }
        Self { seed, state }
    }
    fn value<const N: usize>(&mut self) -> [u8; N] {
        let key = ((N as u64) * u64::from(self.seed))
            .wrapping_sub(0x61c8864e7a143579)
            .to_le_bytes();
        let mut result = [0; N];
        for byte in &mut result {
            let [x, y, z, w] = self.state;
            let t = x ^ (x << 11);
            let next = w ^ (w >> 19) ^ t ^ (t >> 8);
            self.state = [y, z, w, next];
            *byte = key[next as usize % 8];
        }
        result
    }
}
fn bh3(data: &[u8], size: usize) -> Vec<u8> {
    let mut result = b"UnityFS\0".to_vec();
    result.resize(32, 0);
    result.extend_from_slice(b"encoded-version\0encoded-revision\0");
    let payload = &data[(size + 20).next_multiple_of(16)..];
    let length = result.len() + payload.len();
    let mut rng = Rng::new(length as u32);
    for (target, source) in [(8, size + 12), (12, size + 8), (16, size + 16)] {
        let raw = u32::from_be_bytes(data[source..source + 4].try_into().unwrap())
            ^ u32::from_le_bytes(rng.value());
        result[target..target + 4].copy_from_slice(&raw.to_be_bytes());
    }
    let raw = length as u64 ^ u64::from_le_bytes(rng.value());
    result[20..28].copy_from_slice(&raw.to_be_bytes());
    result[28..32].copy_from_slice(&6u32.to_be_bytes());
    result.extend_from_slice(payload);
    result
}
fn options(profile: &str) -> LoadOptions {
    LoadOptions {
        game: profile.into(),
        ..Default::default()
    }
}

#[test]
fn five_new_profiles_load_real_serialized_text_and_export_exact_payloads() {
    for profile in ["Naraka", "TOT", "BH3PrePre", "ExAstris", "LoveAndDeepspace"] {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let platform = if matches!(profile, "TOT" | "BH3PrePre") {
            -2
        } else {
            13
        };
        let payload = file(
            &[(49, 7, text("Selected", "exact original text"))],
            platform,
        );
        let (mut data, size) = bundle(
            &payload,
            "CAB-main",
            if profile == "ExAstris" { 4 } else { 3 },
        );
        match profile {
            "Naraka" => data = naraka(data, size),
            "BH3PrePre" => data = bh3(&data, size),
            "LoveAndDeepspace" => data = lnd(&data),
            _ => {}
        }
        let input = source.path().join("encoded.bundle");
        fs::write(&input, &data).unwrap();
        let session = Session::open(&input, &options(profile))
            .unwrap_or_else(|error| panic!("{profile}: {error:#}"));
        assert!(session.diagnostics().is_empty());
        assert_eq!(session.assets().len(), 1);
        assert_eq!(session.assets()[0].name, "Selected");
        assert!(
            matches!(session.preview(session.assets()[0].key).unwrap(),Preview::Text(text) if text=="exact original text")
        );
        let report = session
            .export(
                &[session.assets()[0].key],
                output.path(),
                &ExportOptions::default(),
            )
            .unwrap();
        assert!(report.failed.is_empty(), "{profile}: {:?}", report.failed);
        assert_eq!(report.written.len(), 1);
        assert_eq!(
            fs::read(&report.written[0]).unwrap(),
            b"exact original text"
        );
        assert_eq!(fs::read(input).unwrap(), data);
    }
}

#[test]
fn tot_concatenated_and_split_inputs_keep_both_exact_objects_and_deduplicate_parts() {
    let root = tempfile::tempdir().unwrap();
    let (mut data, _) = bundle(&file(&[(49, 7, text("First", "one"))], -2), "CAB-first", 5);
    let (second, _) = bundle(
        &file(&[(49, 9, text("Second", "two"))], -2),
        "CAB-second",
        5,
    );
    data.extend_from_slice(&second);
    let a = root.path().join("archive.bundle.split0");
    let b = root.path().join("archive.bundle.split1");
    fs::write(&a, &data[..100]).unwrap();
    fs::write(&b, &data[100..]).unwrap();
    let session = Session::open_paths(&[b.clone(), a.clone()], &options("TOT")).unwrap();
    assert!(session.diagnostics().is_empty());
    assert_eq!(session.assets().len(), 2);
    assert_eq!(
        session
            .assets()
            .iter()
            .map(|asset| asset.name.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert!(
        matches!(session.preview(session.assets()[0].key).unwrap(),Preview::Text(text) if text=="one")
    );
    assert!(
        matches!(session.preview(session.assets()[1].key).unwrap(),Preview::Text(text) if text=="two")
    );
    assert_eq!(fs::read(a).unwrap(), data[..100]);
    assert_eq!(fs::read(b).unwrap(), data[100..]);
}

fn material(profile: &str) -> Vec<u8> {
    let mut data = Vec::new();
    string(&mut data, "VariantMaterial");
    data.extend_from_slice(&0_i32.to_le_bytes());
    data.extend_from_slice(&0_i64.to_le_bytes());
    data.extend_from_slice(&[0; 12]);
    data.push(0);
    pad(&mut data, 4);
    data.extend_from_slice(&(-1_i32).to_le_bytes());
    if profile == "LoveAndDeepspace" {
        data.extend_from_slice(&12345_u32.to_le_bytes());
    }
    data.extend_from_slice(&0_i32.to_le_bytes()); // Tags.
    if profile == "Naraka" {
        data.extend_from_slice(&12345_i32.to_le_bytes());
    }
    data.extend_from_slice(&0_i32.to_le_bytes()); // Disabled passes.
    data.extend_from_slice(&[0; 12]); // Texture/int/float property counts.
    data.extend_from_slice(&1_i32.to_le_bytes());
    string(&mut data, "_Color");
    for value in [0.25_f32, 0.5, 0.75, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data
}

#[test]
fn wrapped_naraka_and_lnd_materials_use_the_actual_variant_reader() {
    for profile in ["Naraka", "LoveAndDeepspace"] {
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let payload = file(&[(21, 11, material(profile))], 13);
        let (data, size) = bundle(&payload, "CAB-material", 3);
        let data = if profile == "Naraka" {
            naraka(data, size)
        } else {
            lnd(&data)
        };
        let input = root.path().join("materials.bundle");
        fs::write(&input, data).unwrap();
        let session = Session::open(&input, &options(profile)).unwrap();
        assert_eq!(session.assets()[0].name, "VariantMaterial");
        let report = session
            .export(
                &[session.assets()[0].key],
                output.path(),
                &ExportOptions {
                    mode: ExportMode::Json,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(report.failed.is_empty(), "{profile}: {:?}", report.failed);
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&report.written[0]).unwrap()).unwrap();
        assert!(json.to_string().contains("_Color"));
        assert!(json.to_string().contains("0.25"));
    }
}
