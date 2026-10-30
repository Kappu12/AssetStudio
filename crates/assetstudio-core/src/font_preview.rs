//! Bounded, in-memory font samples. Fonts are never installed or registered.

use ab_glyph::{
    Font, FontRef, Outline, OutlineCurve, OutlinedGlyph, Point, PxScale, PxScaleFactor, ScaleFont,
    point,
};
use anyhow::{Context, Result, ensure};

use crate::Preview;

pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
const WIDTH: usize = 1100;
const HEIGHT: usize = 420;
const MAX_GLYPH_SIDE: f32 = 512.0;
const MAX_GLYPH_CURVES: usize = 10_000;
const MAX_TOTAL_CURVES: usize = 100_000;
const MAX_RASTER_PIXELS: usize = 8 * 1024 * 1024;
const BACKGROUND: [u8; 4] = [22, 27, 34, 255];

/// Renders fixed samples at several sizes using only a bounded RGBA canvas.
/// Bitmap-only or unavailable outline formats return an explicit error.
pub fn render(bytes: &[u8]) -> Result<Preview> {
    ensure!(
        bytes.len() <= MAX_FONT_BYTES,
        "Font preview source exceeds 32 MiB"
    );
    let font =
        FontRef::try_from_slice(bytes).context("Font has no readable TrueType/OpenType face")?;
    let outlines = TrueTypeOutlines::parse(bytes)?;
    let height = font.height_unscaled();
    ensure!(height.is_finite() && height > 0.0, "Font height is invalid");
    ensure!(
        font.units_per_em()
            .is_some_and(|units| units.is_finite() && units > 0.0),
        "Font units per em are invalid"
    );
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(WIDTH * HEIGHT * 4)?;
    pixels.resize(WIDTH * HEIGHT * 4, 0);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&BACKGROUND);
    }
    let samples = [
        (24.0, 62.0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        (40.0, 130.0, "abcdefghijklmnopqrstuvwxyz"),
        (56.0, 220.0, "Aa Bb Cc 0123456789"),
        (28.0, 302.0, "The quick brown fox jumps over the lazy dog."),
        (24.0, 368.0, "0123456789 !@#$%&*() +-=[]{}"),
    ];
    let mut curves = 0_usize;
    let mut outline_work = MAX_TOTAL_CURVES;
    let mut raster_pixels = 0_usize;
    let mut rendered = 0_usize;
    let mut ink = 0_usize;
    for (size, baseline, text) in samples {
        let scaled = font.as_scaled(PxScale::from(size));
        let factor = scaled.scale_factor();
        ensure!(
            factor.horizontal.is_finite()
                && factor.vertical.is_finite()
                && factor.horizontal > 0.0
                && factor.vertical > 0.0,
            "Font scale is invalid"
        );
        let mut x = 36.0_f32;
        let mut previous = None;
        for character in text.chars() {
            let id = font.glyph_id(character);
            if let Some(previous) = previous {
                let kerning = scaled.kern(previous, id);
                ensure!(
                    kerning.is_finite() && kerning.abs() <= MAX_GLYPH_SIDE,
                    "Font kerning exceeds preview limits"
                );
                x += kerning;
            }
            let advance = scaled.h_advance(id);
            ensure!(
                advance.is_finite() && (0.0..=MAX_GLYPH_SIDE).contains(&advance),
                "Font advance exceeds preview limits"
            );
            ensure!(
                x.is_finite() && x.abs() <= 4096.0,
                "Font layout exceeds preview limits"
            );
            if x + advance > WIDTH as f32 - 24.0 {
                break;
            }
            if id.0 != 0 {
                // ab_glyph materializes a whole outline Vec. Bound composite
                // expansion before entering that decoder, not after allocation.
                let mut glyph_work = MAX_GLYPH_CURVES;
                outlines.check(id.0, 0, &mut glyph_work, &mut outline_work)?;
            }
            if id.0 != 0
                && let Some(outline) = font.outline(id)
            {
                ensure!(
                    outline.curves.len() <= MAX_GLYPH_CURVES,
                    "Font glyph has too many outline curves"
                );
                curves = curves
                    .checked_add(outline.curves.len())
                    .context("Font outline work count overflow")?;
                ensure!(
                    curves <= MAX_TOTAL_CURVES,
                    "Font preview exceeds its outline work limit"
                );
                let glyph = id.with_scale_and_position(size, point(x, baseline));
                let bounds = outline.px_bounds(factor, glyph.position);
                validate_outline(&outline, factor, glyph.position, bounds)?;
                let area = (bounds.width() as usize)
                    .checked_mul(bounds.height() as usize)
                    .context("Font raster size overflow")?;
                raster_pixels = raster_pixels
                    .checked_add(area)
                    .context("Font raster work count overflow")?;
                ensure!(
                    raster_pixels <= MAX_RASTER_PIXELS,
                    "Font preview exceeds its raster work limit"
                );
                if area != 0 {
                    let outlined = OutlinedGlyph::new(glyph, outline, factor);
                    let mut invalid_coverage = false;
                    outlined.draw(|gx, gy, coverage| {
                        if !coverage.is_finite() {
                            invalid_coverage = true;
                            return;
                        }
                        let px = bounds.min.x as i64 + i64::from(gx);
                        let py = bounds.min.y as i64 + i64::from(gy);
                        if px < 0 || py < 0 || px >= WIDTH as i64 || py >= HEIGHT as i64 {
                            return;
                        }
                        let coverage = coverage.clamp(0.0, 1.0);
                        if coverage <= 0.0 {
                            return;
                        }
                        let index = (py as usize * WIDTH + px as usize) * 4;
                        for (channel, text_color) in
                            [225.0_f32, 233.0, 240.0].into_iter().enumerate()
                        {
                            pixels[index + channel] =
                                (f32::from(pixels[index + channel]) * (1.0 - coverage)
                                    + text_color * coverage)
                                    .round() as u8;
                        }
                        ink += 1;
                    });
                    ensure!(
                        !invalid_coverage,
                        "Font rasterizer returned invalid coverage"
                    );
                    rendered += 1;
                }
            }
            x += advance;
            previous = Some(id);
        }
    }
    ensure!(
        rendered != 0 && ink != 0,
        "Font has no renderable outline glyphs for the preview sample"
    );
    Ok(Preview::Image {
        width: WIDTH as u32,
        height: HEIGHT as u32,
        rgba: pixels,
    })
}

/// Conservative glyf work preflight. CFF/CFF2 and variable outline programs are explicitly
/// unsupported for preview because their subroutine work cannot be bounded by
/// the ab_glyph API. Their original font bytes remain exportable.
struct TrueTypeOutlines<'a> {
    glyf: &'a [u8],
    loca: &'a [u8],
    long_offsets: bool,
    glyphs: usize,
}

impl<'a> TrueTypeOutlines<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        let face = if bytes.starts_with(b"ttcf") {
            ensure!(read_u32(bytes, 8)? != 0, "Font collection is empty");
            read_u32(bytes, 12)? as usize
        } else {
            0
        };
        let count = usize::from(read_u16(
            bytes,
            face.checked_add(4).context("Font table offset overflow")?,
        )?);
        let start = face.checked_add(12).context("Font table offset overflow")?;
        let records = bytes
            .get(
                start
                    ..start
                        .checked_add(count * 16)
                        .context("Font table size overflow")?,
            )
            .context("Font table directory is truncated")?;
        for record in records.as_chunks::<16>().0 {
            // ttf-parser can prefer variation outlines or fall back to CFF
            // even when raw glyf/loca records also exist. Do not allow an
            // alternative outline engine to bypass the glyf work preflight.
            ensure!(
                !matches!(&record[..4], b"CFF " | b"CFF2" | b"gvar"),
                "CFF and variable font outline previews are unavailable"
            );
        }
        let table = |tag: &[u8; 4]| -> Result<&'a [u8]> {
            let mut found = None;
            for record in records.as_chunks::<16>().0 {
                if &record[..4] != tag {
                    continue;
                }
                ensure!(found.is_none(), "Font has duplicate outline tables");
                let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
                let size = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
                found = Some(
                    bytes
                        .get(
                            offset
                                ..offset
                                    .checked_add(size)
                                    .context("Font table size overflow")?,
                        )
                        .context("Font outline table is truncated")?,
                );
            }
            found.context("Font has no renderable TrueType outline tables; CFF/bitmap previews are unavailable")
        };
        let head = table(b"head")?;
        let format = read_u16(head, 50)?;
        ensure!(format <= 1, "Font glyph offset format is invalid");
        let glyphs = usize::from(read_u16(table(b"maxp")?, 4)?);
        let loca = table(b"loca")?;
        let long_offsets = format == 1;
        ensure!(
            loca.len() >= (glyphs + 1) * if long_offsets { 4 } else { 2 },
            "Font glyph offset table is truncated"
        );
        Ok(Self {
            glyf: table(b"glyf")?,
            loca,
            long_offsets,
            glyphs,
        })
    }

    fn offset(&self, index: usize) -> Result<usize> {
        Ok(if self.long_offsets {
            read_u32(self.loca, index * 4)? as usize
        } else {
            usize::from(read_u16(self.loca, index * 2)?) * 2
        })
    }

    fn check(
        &self,
        glyph: u16,
        depth: usize,
        remaining: &mut usize,
        total: &mut usize,
    ) -> Result<()> {
        ensure!(
            depth < 32,
            "Font composite glyph nesting exceeds preview limits"
        );
        ensure!(
            usize::from(glyph) < self.glyphs,
            "Font composite glyph index is invalid"
        );
        let start = self.offset(usize::from(glyph))?;
        let end = self.offset(usize::from(glyph) + 1)?;
        let data = self
            .glyf
            .get(start..end)
            .context("Font glyph range is invalid")?;
        charge_outline_work(1, remaining, total)?;
        if data.is_empty() {
            return Ok(());
        }
        let contours = read_u16(data, 0)? as i16;
        ensure!(data.len() >= 10, "Font glyph header is truncated");
        if contours > 0 {
            let contours = contours as usize;
            charge_outline_work(contours * 2, remaining, total)?;
            let mut previous = None;
            for index in 0..contours {
                let last = read_u16(data, 10 + index * 2)?;
                ensure!(
                    previous.is_none_or(|value| last > value),
                    "Font contour endpoints are invalid"
                );
                previous = Some(last);
            }
            charge_outline_work(usize::from(previous.unwrap()) + 1, remaining, total)?;
        } else if contours < 0 {
            let mut cursor = 10;
            loop {
                let flags = read_u16(data, cursor)?;
                let child = read_u16(data, cursor + 2)?;
                let arguments = if flags & 1 != 0 { 4 } else { 2 };
                let transform = if flags & 8 != 0 {
                    2
                } else if flags & 64 != 0 {
                    4
                } else if flags & 128 != 0 {
                    8
                } else {
                    0
                };
                cursor += 4 + arguments + transform;
                ensure!(cursor <= data.len(), "Font composite glyph is truncated");
                self.check(child, depth + 1, remaining, total)?;
                if flags & 32 == 0 {
                    break;
                }
            }
        }
        Ok(())
    }
}

fn charge_outline_work(work: usize, remaining: &mut usize, total: &mut usize) -> Result<()> {
    ensure!(
        work <= *remaining && work <= *total,
        "Font outline expansion exceeds preview work limits"
    );
    *remaining -= work;
    *total -= work;
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset.checked_add(2).context("Font offset overflow")?)
        .context("Font outline metadata is truncated")?;
    Ok(u16::from_be_bytes(value.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset.checked_add(4).context("Font offset overflow")?)
        .context("Font outline metadata is truncated")?;
    Ok(u32::from_be_bytes(value.try_into().unwrap()))
}

fn validate_outline(
    outline: &Outline,
    factor: PxScaleFactor,
    position: Point,
    bounds: ab_glyph::Rect,
) -> Result<()> {
    for value in [bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y] {
        ensure!(
            value.is_finite() && value.abs() <= 4096.0,
            "Font glyph bounds exceed preview limits"
        );
    }
    ensure!(
        (0.0..=MAX_GLYPH_SIDE).contains(&bounds.width())
            && (0.0..=MAX_GLYPH_SIDE).contains(&bounds.height()),
        "Font glyph raster dimensions exceed preview limits"
    );
    let check = |point: Point| -> Result<()> {
        let x = point.x * factor.horizontal + position.x;
        let y = point.y * -factor.vertical + position.y;
        ensure!(
            x.is_finite()
                && y.is_finite()
                && x >= bounds.min.x - MAX_GLYPH_SIDE
                && x <= bounds.max.x + MAX_GLYPH_SIDE
                && y >= bounds.min.y - MAX_GLYPH_SIDE
                && y <= bounds.max.y + MAX_GLYPH_SIDE,
            "Font curve control point exceeds preview bounds"
        );
        Ok(())
    };
    for curve in &outline.curves {
        match *curve {
            OutlineCurve::Line(a, b) => {
                check(a)?;
                check(b)?;
            }
            OutlineCurve::Quad(a, b, c) => {
                check(a)?;
                check(b)?;
                check(c)?;
            }
            OutlineCurve::Cubic(a, b, c, d) => {
                check(a)?;
                check(b)?;
                check(c)?;
                check(d)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HACK: &[u8] = include_bytes!("../tests/fixtures/fonts/Hack-Regular.ttf");

    #[test]
    fn malformed_and_oversized_font_programs_return_errors() {
        for bytes in [
            b"".as_slice(),
            b"not a font",
            &HACK[..4],
            &HACK[..12],
            &HACK[..128],
        ] {
            assert!(render(bytes).is_err());
        }
        assert!(
            render(&vec![0; MAX_FONT_BYTES + 1])
                .unwrap_err()
                .to_string()
                .contains("exceeds 32 MiB")
        );
    }

    #[test]
    fn valid_face_without_outlines_reports_unavailable_sample() {
        let mut bytes = HACK.to_vec();
        let tables = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
        let mut removed = false;
        for table in bytes[12..12 + tables * 16].as_chunks_mut::<16>().0 {
            if &table[..4] == b"glyf" {
                table[..4].copy_from_slice(b"none");
                removed = true;
            }
        }
        assert!(removed);
        let error = render(&bytes).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no renderable TrueType outline tables"),
            "{error}"
        );
    }

    #[test]
    fn alternative_outline_engines_cannot_bypass_glyf_preflight() {
        for tag in [b"CFF ", b"CFF2", b"gvar"] {
            let mut bytes = HACK.to_vec();
            let tables = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
            let table = bytes[12..12 + tables * 16]
                .as_chunks_mut::<16>()
                .0
                .iter_mut()
                .find(|record| &record[..4] == b"name")
                .unwrap();
            table[..4].copy_from_slice(tag);
            let error = render(&bytes).unwrap_err();
            assert!(error.to_string().contains("CFF and variable"), "{error}");
        }
    }

    #[test]
    fn repeated_composite_expansion_is_rejected_before_outline_decoding() {
        // Each glyph repeats the previous glyph twice; depth 16 is only a few
        // hundred encoded bytes but expands to over 65,000 base outlines.
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        loca.extend_from_slice(&0_u32.to_be_bytes());
        glyf.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        for glyph in 1_u16..=16 {
            glyf.extend_from_slice(&[0xff, 0xff, 0, 0, 0, 0, 0, 1, 0, 1]);
            for flags in [32_u16, 0] {
                glyf.extend_from_slice(&flags.to_be_bytes());
                glyf.extend_from_slice(&(glyph - 1).to_be_bytes());
                glyf.extend_from_slice(&[0, 0]);
            }
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        }
        let outlines = TrueTypeOutlines {
            glyf: &glyf,
            loca: &loca,
            long_offsets: true,
            glyphs: 17,
        };
        let mut remaining = MAX_GLYPH_CURVES;
        let mut total = MAX_TOTAL_CURVES;
        let error = outlines
            .check(16, 0, &mut remaining, &mut total)
            .unwrap_err();
        assert!(error.to_string().contains("outline expansion"));
        assert!(glyf.len() < 512);
    }

    #[test]
    fn oversized_and_nonfinite_bounds_fail_before_raster_allocation() {
        let outline = Outline {
            bounds: ab_glyph::Rect {
                min: point(0.0, 1.0),
                max: point(1.0, 0.0),
            },
            curves: Vec::new(),
        };
        let factor = PxScaleFactor {
            horizontal: 1.0,
            vertical: 1.0,
        };
        for rect in [
            ab_glyph::Rect {
                min: point(0.0, 0.0),
                max: point(513.0, 1.0),
            },
            ab_glyph::Rect {
                min: point(f32::NAN, 0.0),
                max: point(1.0, 1.0),
            },
        ] {
            assert!(validate_outline(&outline, factor, point(0.0, 0.0), rect).is_err());
        }
        let malformed = Outline {
            curves: vec![OutlineCurve::Line(
                point(f32::INFINITY, 0.0),
                point(0.0, 0.0),
            )],
            ..outline
        };
        assert!(
            validate_outline(
                &malformed,
                factor,
                point(0.0, 0.0),
                ab_glyph::Rect {
                    min: point(0.0, 0.0),
                    max: point(1.0, 1.0)
                }
            )
            .is_err()
        );
    }
}
