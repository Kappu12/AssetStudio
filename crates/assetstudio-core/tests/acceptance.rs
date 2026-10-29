//! Independent binary-layout checks against the managed reference's field order.
//! These synthetic fixtures are original test data, not copyrighted game assets.
use std::{fs, path::Path};

use assetstudio_core::{ExportOptions, LoadOptions, Preview, Session};
use unity_rs_core::source::Region;

#[derive(Clone, Copy)]
enum Order {
    Little,
    Big,
}

impl Order {
    fn u16(self, out: &mut Vec<u8>, value: u16) {
        out.extend(match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        });
    }
    fn u32(self, out: &mut Vec<u8>, value: u32) {
        out.extend(match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        });
    }
    fn u64(self, out: &mut Vec<u8>, value: u64) {
        out.extend(match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        });
    }
}

fn align(bytes: &mut Vec<u8>, base: usize, alignment: usize) {
    while !(base + bytes.len()).is_multiple_of(alignment) {
        bytes.push(0);
    }
}
fn string(out: &mut Vec<u8>, value: &str, order: Order) {
    order.u32(out, value.len() as u32);
    out.extend(value.as_bytes());
    align(out, 0, 4);
}

// SerializedFile.cs: header is always big endian; metadata and object payload
// follow the endian byte. Type dependencies exist ONLY with enabled type trees.
fn serialized(format: u32, order: Order, class_id: u32, payload: &[u8]) -> Vec<u8> {
    let header_len = if format == 22 { 48 } else { 20 };
    let mut metadata = b"6000.0.23f1\0".to_vec();
    order.u32(&mut metadata, 19); // StandaloneWindows64, no editor object prefix.
    metadata.push(0); // Stripped type tree.
    order.u32(&mut metadata, 1);
    order.u32(&mut metadata, class_id);
    metadata.push(0);
    order.u16(&mut metadata, u16::MAX);
    metadata.extend([0; 16]);
    order.u32(&mut metadata, 1); // One object, not a type-dependency count.
    align(&mut metadata, header_len, 4);
    order.u64(&mut metadata, 7);
    if format == 22 {
        order.u64(&mut metadata, 0);
    } else {
        order.u32(&mut metadata, 0);
    }
    order.u32(&mut metadata, payload.len() as u32);
    order.u32(&mut metadata, 0); // Type index.
    order.u32(&mut metadata, 0); // Script types.
    order.u32(&mut metadata, 0); // Externals.
    if format >= 20 {
        order.u32(&mut metadata, 0);
    }
    metadata.push(0); // User information.
    let data_offset = (header_len + metadata.len()).next_multiple_of(16);
    let file_size = data_offset + payload.len();
    let mut file = vec![0; header_len];
    file[8..12].copy_from_slice(&format.to_be_bytes());
    file[16] = u8::from(matches!(order, Order::Big));
    if format == 22 {
        file[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
        file[24..32].copy_from_slice(&(file_size as u64).to_be_bytes());
        file[32..40].copy_from_slice(&(data_offset as u64).to_be_bytes());
    } else {
        file[0..4].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
        file[4..8].copy_from_slice(&(file_size as u32).to_be_bytes());
        file[12..16].copy_from_slice(&(data_offset as u32).to_be_bytes());
    }
    file.extend(metadata);
    file.resize(data_offset, 0);
    file.extend(payload);
    file
}

fn texture(streamed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    string(&mut out, "tiny_texture", Order::Little);
    // Texture.cs: Unity 6000 REMOVED forcedFallbackFormat/downscaleFallback.
    out.push(0); // Alpha channel optional.
    align(&mut out, 0, 4);
    for value in [2, 2, 16, 0, 4, 1] {
        Order::Little.u32(&mut out, value);
    }
    out.extend([0, 0, 0]); // Readable, preprocessed, ignore mipmap limit.
    align(&mut out, 0, 4);
    string(&mut out, "", Order::Little); // Mipmap limit group.
    out.push(0); // Streaming mipmaps.
    align(&mut out, 0, 4);
    // Priority, image count/dimension, filter/aniso/bias, wrap U/V/W,
    // lightmap format, color space, platform blob length.
    for value in [0, 1, 2, 1, 1, 0, 0, 0, 0, 0, 1, 0] {
        Order::Little.u32(&mut out, value);
    }
    if streamed {
        Order::Little.u32(&mut out, 0);
        Order::Little.u64(&mut out, 0);
        Order::Little.u32(&mut out, 16);
        string(&mut out, "pixels.resS", Order::Little);
    } else {
        Order::Little.u32(&mut out, 16);
        out.extend(PIXELS);
    }
    out
}

const PIXELS: [u8; 16] = [
    0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255,
];
const DISPLAY_PIXELS: [u8; 16] = [
    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
];

fn literal_lz4(bytes: &[u8]) -> Vec<u8> {
    let mut output = vec![(bytes.len().min(15) as u8) << 4];
    if bytes.len() >= 15 {
        let mut remaining = bytes.len() - 15;
        while remaining >= 255 {
            output.push(255);
            remaining -= 255;
        }
        output.push(remaining as u8);
    }
    output.extend(bytes);
    output
}

fn bundle(entries: &[(&str, &[u8])], compressed: bool) -> Vec<u8> {
    let mut data = Vec::new();
    let mut nodes = Vec::new();
    for &(name, bytes) in entries {
        Order::Big.u64(&mut nodes, data.len() as u64);
        Order::Big.u64(&mut nodes, bytes.len() as u64);
        Order::Big.u32(&mut nodes, 0);
        nodes.extend(name.as_bytes());
        nodes.push(0);
        data.extend(bytes);
    }
    let encoded = if compressed {
        literal_lz4(&data)
    } else {
        data.clone()
    };
    let codec = if compressed { 2 } else { 0 };
    let mut info = vec![0; 16];
    Order::Big.u32(&mut info, 1);
    Order::Big.u32(&mut info, data.len() as u32);
    Order::Big.u32(&mut info, encoded.len() as u32);
    Order::Big.u16(&mut info, codec);
    Order::Big.u32(&mut info, entries.len() as u32);
    info.extend(nodes);
    let encoded_info = if compressed {
        literal_lz4(&info)
    } else {
        info.clone()
    };
    let mut header = b"UnityFS\0".to_vec();
    Order::Big.u32(&mut header, 6);
    header.extend(b"5.x.x\0");
    header.extend(b"6000.0.23f1\0");
    let size_position = header.len();
    Order::Big.u64(&mut header, 0);
    Order::Big.u32(&mut header, encoded_info.len() as u32);
    Order::Big.u32(&mut header, info.len() as u32);
    Order::Big.u32(&mut header, 0x40 | codec as u32);
    let size = (header.len() + encoded_info.len() + encoded.len()) as u64;
    header[size_position..size_position + 8].copy_from_slice(&size.to_be_bytes());
    header.extend(encoded_info);
    header.extend(encoded);
    header
}

fn session(bytes: Vec<u8>) -> Session {
    Session::from_regions(
        [("test-input".into(), Region::from_bytes(bytes))],
        &LoadOptions::default(),
    )
    .unwrap()
}

fn first_file(root: &Path) -> std::path::PathBuf {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            return path;
        }
        if path.is_dir() {
            return first_file(&path);
        }
    }
    panic!("no output file")
}

#[test]
fn serialized_v17_v22_and_both_endians_have_identical_text_payloads() {
    for format in [17, 22] {
        for order in [Order::Little, Order::Big] {
            let mut payload = Vec::new();
            string(&mut payload, "text", order);
            order.u32(&mut payload, 5);
            payload.extend(b"hello");
            let session = session(serialized(format, order, 49, &payload));
            assert_eq!(session.assets()[0].name, "text");
            assert!(
                matches!(session.preview(session.assets()[0].key).unwrap(), Preview::Text(text) if text == "hello")
            );
        }
    }
}

#[test]
fn unity6000_inline_texture_preview_flips_rows_once_and_exports_png() {
    let session = session(serialized(22, Order::Little, 28, &texture(false)));
    let key = session.assets()[0].key;
    let Preview::Image {
        width,
        height,
        rgba,
    } = session.preview(key).unwrap()
    else {
        panic!("expected image")
    };
    assert_eq!((width, height), (2, 2));
    assert_eq!(rgba, DISPLAY_PIXELS);
    let output = tempfile::tempdir().unwrap();
    let report = session
        .export(&[key], output.path(), &ExportOptions::default())
        .unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.written.len(), 1);
    let png = fs::read(first_file(output.path())).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 2);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 2);
}

#[test]
fn unityfs_plain_and_lz4_resolve_streamed_texture_resources_and_extract() {
    let asset = serialized(22, Order::Little, 28, &texture(true));
    for compressed in [false, true] {
        let bytes = bundle(
            &[("CAB-fixture", &asset), ("pixels.resS", &PIXELS)],
            compressed,
        );
        let session = session(bytes.clone());
        let Preview::Image { rgba, .. } = session.preview(session.assets()[0].key).unwrap() else {
            panic!("expected image")
        };
        assert_eq!(rgba, DISPLAY_PIXELS);
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.bundle");
        fs::write(&input, bytes).unwrap();
        let output = temp.path().join("extracted");
        let report = Session::extract(&input, &output, &LoadOptions::default(), false).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(report.written.len(), 2);
        assert!(
            report
                .written
                .iter()
                .any(|path| fs::read(path).unwrap() == PIXELS)
        );
    }
}

#[test]
fn malformed_texture_is_an_explicit_failure_instead_of_a_raw_success() {
    let mut payload = texture(false);
    payload.truncate(payload.len() - 9);
    let session = session(serialized(22, Order::Little, 28, &payload));
    let output = tempfile::tempdir().unwrap();
    assert!(session.preview(session.assets()[0].key).is_err());
    let report = session
        .export(
            &[session.assets()[0].key],
            output.path(),
            &ExportOptions::default(),
        )
        .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.written.is_empty());
}
