//! Non-destructive channel inspection of bounded RGBA preview pixels.

const MAX_PREVIEW_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channels {
    pub red: bool,
    pub green: bool,
    pub blue: bool,
    pub alpha: bool,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            red: true,
            green: true,
            blue: true,
            alpha: true,
        }
    }
}

pub fn apply_channels(original_rgba: &[u8], channels: Channels) -> Result<Vec<u8>, String> {
    if original_rgba.len() > MAX_PREVIEW_BYTES || !original_rgba.len().is_multiple_of(4) {
        return Err("Image preview pixels exceed the limit or have an invalid RGBA length.".into());
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(original_rgba.len())
        .map_err(|_| "Cannot allocate image channel preview.".to_owned())?;
    let alpha_only = channels.alpha && !channels.red && !channels.green && !channels.blue;
    let hidden_rgb = if alpha_only { 255 } else { 0 };
    for pixel in original_rgba.as_chunks::<4>().0 {
        output.extend_from_slice(&[
            if channels.red { pixel[0] } else { hidden_rgb },
            if channels.green { pixel[1] } else { hidden_rgb },
            if channels.blue { pixel[2] } else { hidden_rgb },
            if channels.alpha { pixel[3] } else { 255 },
        ]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_toggles_preserve_the_original_and_alpha_contract() {
        let pixels = [20, 40, 80, 120, 5, 10, 15, 0];
        let red = Channels {
            red: true,
            green: false,
            blue: false,
            alpha: false,
        };
        assert_eq!(
            apply_channels(&pixels, red).unwrap(),
            [20, 0, 0, 255, 5, 0, 0, 255]
        );
        let alpha = Channels {
            red: false,
            green: false,
            blue: false,
            alpha: true,
        };
        assert_eq!(
            apply_channels(&pixels, alpha).unwrap(),
            [255, 255, 255, 120, 255, 255, 255, 0]
        );
        assert_eq!(
            apply_channels(&pixels, Channels::default()).unwrap(),
            pixels
        );
    }

    #[test]
    fn disabling_every_channel_is_opaque_black_and_bad_lengths_fail() {
        let channels = Channels {
            red: false,
            green: false,
            blue: false,
            alpha: false,
        };
        assert_eq!(
            apply_channels(&[12, 34, 56, 78], channels).unwrap(),
            [0, 0, 0, 255]
        );
        assert!(apply_channels(&[1, 2, 3], channels).is_err());
    }
}
