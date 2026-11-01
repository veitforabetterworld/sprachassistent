//! PCM helpers: decoding uploads (raw s16le or WAV) to 16 kHz mono f32 and encoding the reply.

use anyhow::{bail, Result};

/// Sample rate whisper expects and the ESP32 records with.
pub const STT_RATE: u32 = 16_000;

/// Decodes an upload into 16 kHz mono f32. A body starting with `RIFF` is read as WAV,
/// everything else as raw signed 16-bit little-endian mono at `raw_rate`.
pub fn decode_upload(body: &[u8], raw_rate: u32) -> Result<Vec<f32>> {
    let (samples, rate) = if body.starts_with(b"RIFF") {
        parse_wav(body)?
    } else {
        (s16le_to_f32(body), raw_rate)
    };
    Ok(resample(&samples, rate, STT_RATE))
}

fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
        .collect()
}

/// Reads a PCM 16-bit WAV, mixing all channels down to mono.
fn parse_wav(data: &[u8]) -> Result<(Vec<f32>, u32)> {
    if data.len() < 12 || &data[8..12] != b"WAVE" {
        bail!("keine WAV-Datei");
    }
    let mut pos = 12;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into()?) as usize;
        let body = &data[pos + 8..data.len().min(pos + 8 + size)];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = Some((
                    u16::from_le_bytes([body[0], body[1]]),
                    u16::from_le_bytes([body[2], body[3]]),
                    u32::from_le_bytes(body[4..8].try_into()?),
                    u16::from_le_bytes([body[14], body[15]]),
                ));
            }
            b"data" => {
                let Some((audio_format, channels, rate, bits)) = format else {
                    bail!("WAV: data vor fmt");
                };
                if audio_format != 1 || bits != 16 || channels == 0 {
                    bail!("WAV: nur PCM 16 Bit wird unterstuetzt");
                }
                let channels = channels as usize;
                let mono = s16le_to_f32(body)
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                    .collect();
                return Ok((mono, rate));
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        pos += 8 + size + (size & 1);
    }
    bail!("WAV: kein data-Chunk")
}

/// Linear interpolation; good enough for speech.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let len = (input.len() as f64 / ratio).floor() as usize;
    (0..len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src.floor() as usize;
            let frac = (src - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Root mean square, used to skip silent recordings before whisper hallucinates on them.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn f32_to_s16le(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect()
}

/// WAV header for a stream of unknown length (sizes set to the maximum, players accept that).
pub fn streaming_wav_header(rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&1u16.to_le_bytes()); // mono
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * 2).to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(u32::MAX - 36).to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2 * channels as u32).to_le_bytes());
        w.extend_from_slice(&(2 * channels).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        w
    }

    #[test]
    fn raw_pcm_passes_through_at_16k() {
        let body: Vec<u8> = [16384i16, -16384]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(decode_upload(&body, 16_000).unwrap(), vec![0.5, -0.5]);
    }

    #[test]
    fn stereo_wav_is_mixed_and_resampled() {
        let samples: Vec<i16> = (0..3200).flat_map(|_| [16384i16, 0]).collect();
        let out = decode_upload(&wav(32_000, 2, &samples), 16_000).unwrap();
        assert_eq!(out.len(), 1600);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn s16le_roundtrip_clamps() {
        assert_eq!(
            f32_to_s16le(&[2.0, -2.0]),
            [32767i16, -32767].map(i16::to_le_bytes).concat()
        );
    }
}
