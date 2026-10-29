//! Lazy native playback for the bounded WAV preview supplied by the asset core.
//!
//! Constructing the player and all decoder tests work without an audio device.

use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

const MAX_PREVIEW_BYTES: usize = 128 * 1024 * 1024;

#[derive(Default)]
pub struct AudioPlayer {
    // A sink must be dropped before its output stream.
    sink: Option<Sink>,
    stream: Option<OutputStream>,
    duration: Option<Duration>,
}

impl AudioPlayer {
    /// Validates the preview before opening a device or replacing a playing clip.
    pub fn play(&mut self, wav: Vec<u8>) -> Result<()> {
        let source = decode_wav(wav, MAX_PREVIEW_BYTES)?;
        let duration = source.total_duration();
        if self.stream.is_none() {
            let mut stream = OutputStreamBuilder::open_default_stream()
                .context("Cannot open an audio output device")?;
            stream.log_on_drop(false);
            self.stream = Some(stream);
        }
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        let stream = self
            .stream
            .as_ref()
            .context("Audio output was not initialized")?;
        let sink = Sink::connect_new(stream.mixer());
        sink.append(FiniteSamples(source));
        self.sink = Some(sink);
        self.duration = duration;
        Ok(())
    }

    pub fn pause(&self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    /// Stops the clip and releases the output device, including its worker.
    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.stream = None;
        self.duration = None;
    }

    pub fn is_paused(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|sink| !sink.empty() && sink.is_paused())
    }

    pub fn is_playing(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|sink| !sink.empty() && !sink.is_paused())
    }

    pub fn position(&self) -> Duration {
        self.sink.as_ref().map_or(Duration::ZERO, |sink| {
            if sink.empty() {
                self.duration.unwrap_or_default()
            } else {
                sink.get_pos()
            }
        })
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

fn decode_wav(wav: Vec<u8>, limit: usize) -> Result<Decoder<Cursor<Vec<u8>>>> {
    ensure!(
        wav.len() <= limit,
        "Audio preview exceeds the {limit} byte limit"
    );
    validate_wav(&wav)?;
    Decoder::new_wav(Cursor::new(wav)).context("Cannot decode this WAV preview")
}

/// Rodio's hound adapter assumes nonzero channel/rate values and can divide by
/// zero for a malicious WAV header. Check those fields and every chunk range
/// before the decoder sees a file. A truncated data chunk must not look like a
/// successfully completed preview either.
fn validate_wav(wav: &[u8]) -> Result<()> {
    ensure!(
        wav.len() >= 12 && &wav[..4] == b"RIFF" && &wav[8..12] == b"WAVE",
        "Audio preview is not a RIFF/WAVE file"
    );
    let declared = u64::from(le_u32(&wav[4..8])) + 8;
    ensure!(
        declared == wav.len() as u64,
        "WAV file size is inconsistent"
    );

    let mut cursor = 12_usize;
    let mut block_align = None;
    let mut data_length = None;
    while cursor < wav.len() {
        let header_end = cursor.checked_add(8).context("WAV chunk offset overflow")?;
        ensure!(header_end <= wav.len(), "WAV chunk header is truncated");
        let size = usize::try_from(le_u32(&wav[cursor + 4..header_end]))?;
        let end = header_end
            .checked_add(size)
            .context("WAV chunk length overflow")?;
        ensure!(end <= wav.len(), "WAV chunk payload is truncated");
        let payload = &wav[header_end..end];
        match &wav[cursor..cursor + 4] {
            b"fmt " => {
                ensure!(block_align.is_none(), "WAV has more than one format chunk");
                block_align = Some(validate_format(payload)?);
            }
            b"data" => {
                ensure!(block_align.is_some(), "WAV data appears before its format");
                ensure!(data_length.is_none(), "WAV has more than one data chunk");
                data_length = Some(size);
            }
            _ => {}
        }
        // Some valid engine WAV payloads omit the final odd-byte RIFF pad.
        // Padding is required between chunks, where it affects interpretation.
        cursor = if end == wav.len() {
            end
        } else {
            end.checked_add(size % 2)
                .context("WAV padding offset overflow")?
        };
        ensure!(cursor <= wav.len(), "WAV chunk padding is truncated");
    }
    let block_align = block_align.context("WAV format chunk is missing")?;
    let data_length = data_length.context("WAV sample data is missing")?;
    ensure!(data_length != 0, "Audio clip contains no samples");
    ensure!(
        data_length % usize::from(block_align) == 0,
        "WAV has an incomplete sample frame"
    );
    Ok(())
}

fn validate_format(fmt: &[u8]) -> Result<u16> {
    ensure!(fmt.len() >= 16, "WAV format chunk is truncated");
    let mut encoding = le_u16(&fmt[..2]);
    let channels = le_u16(&fmt[2..4]);
    let rate = le_u32(&fmt[4..8]);
    let bytes_per_second = le_u32(&fmt[8..12]);
    let block_align = le_u16(&fmt[12..14]);
    let bits = le_u16(&fmt[14..16]);
    ensure!(
        (1..=8).contains(&channels),
        "WAV must have between 1 and 8 channels"
    );
    ensure!((1..=384_000).contains(&rate), "WAV sample rate is invalid");
    if encoding == 0xfffe {
        ensure!(
            fmt.len() >= 40 && le_u16(&fmt[16..18]) >= 22,
            "Extensible WAV format is truncated"
        );
        ensure!(
            fmt[28..40] == [0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113],
            "WAV subformat is not PCM or IEEE float"
        );
        let tag = le_u32(&fmt[24..28]);
        encoding = u16::try_from(tag).context("WAV subformat is invalid")?;
        let valid_bits = le_u16(&fmt[18..20]);
        ensure!(
            valid_bits != 0 && valid_bits <= bits,
            "WAV valid sample width is invalid"
        );
    }
    match encoding {
        1 => ensure!(
            [8, 16, 24, 32].contains(&bits),
            "WAV PCM width is not supported"
        ),
        3 => ensure!(bits == 32, "Only 32-bit WAV floating point is supported"),
        _ => bail!("This WAV encoding is not supported for preview"),
    }
    let expected_align = channels
        .checked_mul(bits / 8)
        .context("WAV frame size overflow")?;
    ensure!(
        block_align == expected_align,
        "WAV frame size is inconsistent"
    );
    ensure!(
        rate.checked_mul(u32::from(block_align)) == Some(bytes_per_second),
        "WAV byte rate is inconsistent"
    );
    Ok(block_align)
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Untrusted floating-point sample values never reach the output device as
/// NaN/infinity or above full scale. The decoder remains streaming.
struct FiniteSamples<S>(S);

impl<S: Source> Iterator for FiniteSamples<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|sample| {
            if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        })
    }
}

impl<S: Source> Source for FiniteSamples<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.0.current_span_len()
    }
    fn channels(&self) -> u16 {
        self.0.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.0.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm_wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
        let size = u32::try_from(samples.len() * 2).unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        wav.extend_from_slice(&(channels * 2).to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&size.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    #[test]
    fn decoder_reads_stereo_channels_samples_and_duration_without_device() {
        let mut source = decode_wav(pcm_wav(2, 8_000, &[0, i16::MAX, i16::MIN, 0]), 1024).unwrap();
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), 8_000);
        assert_eq!(source.total_duration(), Some(Duration::from_micros(250)));
        let samples: Vec<_> = source.by_ref().collect();
        assert_eq!(samples.len(), 4);
        assert_eq!(samples[0], 0.0);
        assert!(samples[1] > 0.99);
        assert_eq!(samples[2], -1.0);
    }

    #[test]
    fn rejects_zero_rate_and_channels_before_hound_can_divide_by_zero() {
        for offset in [22, 24] {
            let mut wav = pcm_wav(1, 8_000, &[0]);
            let length = if offset == 22 { 2 } else { 4 };
            wav[offset..offset + length].fill(0);
            assert!(decode_wav(wav, 1024).is_err());
        }
    }

    #[test]
    fn truncated_sample_payload_is_an_error_not_a_short_success() {
        let mut wav = pcm_wav(1, 8_000, &[12, 34]);
        wav.pop();
        let size = u32::try_from(wav.len() - 8).unwrap();
        wav[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(
            decode_wav(wav, 1024)
                .err()
                .unwrap()
                .to_string()
                .contains("truncated")
        );
    }

    #[test]
    fn all_truncations_and_header_mutations_return_without_panicking() {
        let wav = pcm_wav(1, 8_000, &[0, 1, -1]);
        for end in 0..wav.len() {
            assert!(decode_wav(wav[..end].to_vec(), 1024).is_err());
        }
        for offset in 0..44 {
            let mut changed = wav.clone();
            changed[offset] ^= 0xff;
            let _ = decode_wav(changed, 1024);
        }
    }

    #[test]
    fn no_device_is_opened_for_empty_state_or_invalid_playback() {
        let mut player = AudioPlayer::default();
        player.pause();
        player.resume();
        player.stop();
        assert!(!player.is_playing());
        assert!(!player.is_paused());
        assert_eq!(player.position(), Duration::ZERO);
        assert_eq!(player.duration(), None);
        assert!(player.play(b"invalid".to_vec()).is_err());
        assert!(player.stream.is_none());
        assert!(player.sink.is_none());
    }

    #[test]
    fn refuses_budget_before_parsing() {
        let wav = pcm_wav(1, 8_000, &[0]);
        assert!(
            decode_wav(wav, 8)
                .err()
                .unwrap()
                .to_string()
                .contains("limit")
        );
    }

    #[test]
    fn nonfinite_and_loud_float_samples_are_safe_for_output() {
        let source = rodio::buffer::SamplesBuffer::new(
            1,
            8_000,
            vec![f32::NAN, f32::INFINITY, -2.0, 2.0, 0.5],
        );
        assert_eq!(
            FiniteSamples(source).collect::<Vec<_>>(),
            vec![0.0, 0.0, -1.0, 1.0, 0.5]
        );
    }

    #[test]
    fn floating_point_wav_decodes_and_sanitizes_without_device() {
        let mut wav = pcm_wav(1, 8_000, &[0, 0, 0, 0]);
        wav[20..22].copy_from_slice(&3_u16.to_le_bytes());
        wav[28..32].copy_from_slice(&32_000_u32.to_le_bytes());
        wav[32..34].copy_from_slice(&4_u16.to_le_bytes());
        wav[34..36].copy_from_slice(&32_u16.to_le_bytes());
        wav[44..48].copy_from_slice(&f32::NAN.to_le_bytes());
        wav[48..52].copy_from_slice(&2.0_f32.to_le_bytes());
        let source = decode_wav(wav, 1024).unwrap();
        assert_eq!(source.total_duration(), Some(Duration::from_micros(250)));
        assert_eq!(FiniteSamples(source).collect::<Vec<_>>(), vec![0.0, 1.0]);
    }

    #[test]
    fn pause_resume_and_stop_control_the_real_sink_without_hardware() {
        let (sink, _output) = Sink::new();
        sink.append(rodio::buffer::SamplesBuffer::new(1, 8_000, vec![0.0; 32]));
        let mut player = AudioPlayer {
            sink: Some(sink),
            duration: Some(Duration::from_millis(4)),
            ..AudioPlayer::default()
        };
        assert!(player.is_playing());
        player.pause();
        assert!(player.is_paused());
        assert!(!player.is_playing());
        player.resume();
        assert!(player.is_playing());
        player.stop();
        assert!(!player.is_playing());
        assert!(!player.is_paused());
        assert_eq!(player.duration(), None);
        assert_eq!(player.position(), Duration::ZERO);
    }
}
