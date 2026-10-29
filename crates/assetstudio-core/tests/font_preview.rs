//! Real embedded TrueType data through the public Session Font preview path.

use assetstudio_core::{LoadOptions, Preview, Session};
use unity_rs_core::source::Region;

const HACK: &[u8] = include_bytes!("fixtures/fonts/Hack-Regular.ttf");

#[test]
fn embedded_font_renders_visible_samples_at_multiple_sizes() {
    let session = font_session(HACK);
    let Preview::Image {
        width,
        height,
        rgba,
    } = session.preview(session.assets()[0].key).unwrap()
    else {
        panic!("Font preview must be an image");
    };
    assert!(width <= 1200 && height <= 600);
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    assert!(rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
    // Every sample row has visible ink. This checks actual rasterization, not
    // merely successful parsing or allocation of the background canvas.
    for (start, end) in [(10, 70), (75, 140), (145, 230), (250, 312), (325, 380)] {
        let ink = (start..end)
            .flat_map(|y| {
                let offset = y * width as usize * 4;
                rgba[offset..offset + width as usize * 4].as_chunks::<4>().0
            })
            .filter(|pixel| pixel[..3] != [22, 27, 34])
            .count();
        assert!(ink > 100, "sample row {start}..{end} has no readable ink");
    }
}

#[test]
fn broken_embedded_font_returns_an_error_without_panicking() {
    let session = font_session(&HACK[..128]);
    assert!(session.preview(session.assets()[0].key).is_err());
}

fn font_session(font: &[u8]) -> Session {
    let mut payload = Vec::new();
    payload.extend_from_slice(&7_i32.to_le_bytes());
    payload.extend_from_slice(b"Fixture");
    align(&mut payload, 4);
    payload.extend_from_slice(&[0; 4 + 12 + 4 + 12 + 20]);
    payload.extend_from_slice(&0_i32.to_le_bytes()); // Character rectangles.
    payload.extend_from_slice(&0_i32.to_le_bytes()); // Kerning values.
    payload.extend_from_slice(&1_f32.to_le_bytes()); // Pixel scale.
    payload.extend_from_slice(&(font.len() as i32).to_le_bytes());
    payload.extend_from_slice(font);

    let mut metadata = b"2022.3.62f1\0".to_vec();
    metadata.extend_from_slice(&13_i32.to_le_bytes());
    metadata.push(0); // Player asset; stripped type trees.
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    metadata.extend_from_slice(&128_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&(-1_i16).to_le_bytes());
    metadata.extend_from_slice(&[0; 16]);
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    align(&mut metadata, 4);
    metadata.extend_from_slice(&1_i64.to_le_bytes());
    metadata.extend_from_slice(&0_i64.to_le_bytes());
    metadata.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    metadata.extend_from_slice(&0_i32.to_le_bytes());
    metadata.extend_from_slice(&[0; 12]);
    metadata.push(0);

    let offset = (48 + metadata.len()).next_multiple_of(16);
    let mut fixture = vec![0; 48];
    fixture[8..12].copy_from_slice(&22_u32.to_be_bytes());
    fixture[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    fixture[24..32].copy_from_slice(&((offset + payload.len()) as i64).to_be_bytes());
    fixture[32..40].copy_from_slice(&(offset as i64).to_be_bytes());
    fixture.extend_from_slice(&metadata);
    fixture.resize(offset, 0);
    fixture.extend_from_slice(&payload);
    Session::from_regions(
        [("CAB-font".to_owned(), Region::from_bytes(fixture))],
        &LoadOptions::default(),
    )
    .unwrap()
}

fn align(bytes: &mut Vec<u8>, boundary: usize) {
    while !bytes.len().is_multiple_of(boundary) {
        bytes.push(0);
    }
}
