//! Speech recognition (whisper.cpp) and synthesis (Piper voices via sherpa-onnx), both local.
//! Both run synchronously on the CPU and are called through `spawn_blocking`.

use anyhow::Result;

pub trait SpeechToText: Send + Sync {
    /// `audio` is 16 kHz mono.
    fn transcribe(&self, audio: &[f32]) -> Result<String>;
}

pub trait TextToSpeech: Send + Sync {
    fn sample_rate(&self) -> u32;
    /// Mono f32 samples at `sample_rate()`.
    fn synthesize(&self, text: &str) -> Result<Vec<f32>>;
}

/// Whisper invents text on silence or noise, typically subtitle credits from its training data.
pub fn is_hallucination(text: &str) -> bool {
    let t = text.to_lowercase();
    t.is_empty()
        || [
            "untertitel",
            "zdf",
            "copyright",
            "vielen dank fürs zuschauen",
            "[musik]",
            "(musik)",
        ]
        .iter()
        .any(|h| t.contains(h))
}

/// Whisper's encoder always works on a 30 s window (1500 frames, 50 per second of audio).
/// Limiting it to the recording plus a margin makes short questions several times faster.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn audio_ctx(samples_16k: usize) -> i32 {
    const FRAMES_PER_SECOND: usize = 50;
    const MARGIN: usize = 128;
    let frames = samples_16k * FRAMES_PER_SECOND / 16_000 + MARGIN;
    frames.min(1500) as i32
}

#[cfg(feature = "whisper")]
pub use whisper::WhisperStt;

#[cfg(feature = "whisper")]
mod whisper {
    use std::path::Path;
    use std::sync::Mutex;

    use anyhow::{anyhow, Result};
    use whisper_rs::{
        FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    };

    pub struct WhisperStt {
        // Created once: a fresh state allocates several hundred MB of buffers per request,
        // which on the Pi costs seconds of page faults and memory compaction.
        state: Mutex<WhisperState>,
        _ctx: WhisperContext,
        threads: i32,
        beam_size: i32,
        language: String,
    }

    impl WhisperStt {
        pub fn load(model: &Path, threads: i32, beam_size: i32, language: &str) -> Result<Self> {
            let path = model
                .to_str()
                .ok_or_else(|| anyhow!("Modellpfad ist kein UTF-8"))?;
            if !model.exists() {
                return Err(anyhow!(
                    "Whisper-Modell {path} fehlt (deployments/pi/download-models.sh)"
                ));
            }
            let mut ctx_params = WhisperContextParameters::default();
            ctx_params.flash_attn(true);
            let ctx = WhisperContext::new_with_params(path, ctx_params)
                .map_err(|e| anyhow!("Whisper-Modell laden: {e:?}"))?;
            let state = ctx.create_state().map_err(|e| anyhow!("{e:?}"))?;
            Ok(Self {
                state: Mutex::new(state),
                _ctx: ctx,
                threads,
                beam_size,
                language: language.to_string(),
            })
        }
    }

    impl super::SpeechToText for WhisperStt {
        fn transcribe(&self, audio: &[f32]) -> Result<String> {
            let strategy = if self.beam_size > 1 {
                SamplingStrategy::BeamSearch {
                    beam_size: self.beam_size,
                    patience: -1.0,
                }
            } else {
                SamplingStrategy::Greedy { best_of: 1 }
            };
            let mut params = FullParams::new(strategy);
            params.set_n_threads(self.threads);
            params.set_language(Some(&self.language));
            params.set_translate(false);
            params.set_no_context(true);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_special(false);
            params.set_print_timestamps(false);
            params.set_audio_ctx(super::audio_ctx(audio.len()));

            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .full(params, audio)
                .map_err(|e| anyhow!("Whisper: {e:?}"))?;
            let text = state
                .as_iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
        }
    }
}

#[cfg(feature = "piper")]
pub use piper::PiperTts;

#[cfg(feature = "piper")]
mod piper {
    use std::path::Path;
    use std::sync::Mutex;

    use anyhow::{anyhow, Result};
    use sherpa_rs::tts::{VitsTts, VitsTtsConfig};
    use sherpa_rs::OnnxConfig;

    pub struct PiperTts {
        tts: Mutex<VitsTts>,
        sample_rate: u32,
        speed: f32,
    }

    impl PiperTts {
        /// `model` is the voice's .onnx; `tokens.txt` and `espeak-ng-data/` sit next to it
        /// (layout of the sherpa-onnx `vits-piper-*` archives).
        pub fn load(model: &Path, threads: i32, speed: f32) -> Result<Self> {
            let dir = model
                .parent()
                .ok_or_else(|| anyhow!("PIPER_MODEL ohne Verzeichnis"))?;
            let tokens = dir.join("tokens.txt");
            let data_dir = dir.join("espeak-ng-data");
            for p in [model, tokens.as_path(), data_dir.as_path()] {
                if !p.exists() {
                    return Err(anyhow!(
                        "Piper-Stimme unvollstaendig, {} fehlt (deployments/pi/download-models.sh)",
                        p.display()
                    ));
                }
            }
            let config = VitsTtsConfig {
                model: model.display().to_string(),
                tokens: tokens.display().to_string(),
                data_dir: data_dir.display().to_string(),
                length_scale: 1.0,
                onnx_config: OnnxConfig {
                    provider: "cpu".into(),
                    debug: false,
                    num_threads: threads,
                },
                ..Default::default()
            };
            let mut tts = VitsTts::new(config);
            // The sample rate is only reported with generated audio; this also warms up the model.
            let warmup = tts
                .create("Hallo.", 0, speed)
                .map_err(|e| anyhow!("Piper: {e}"))?;
            Ok(Self {
                tts: Mutex::new(tts),
                sample_rate: warmup.sample_rate,
                speed,
            })
        }
    }

    impl super::TextToSpeech for PiperTts {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
            let mut tts = self.tts.lock().unwrap_or_else(|e| e.into_inner());
            let audio = tts
                .create(text, 0, self.speed)
                .map_err(|e| anyhow!("Piper: {e}"))?;
            Ok(audio.samples)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_whisper_hallucinations() {
        assert!(super::is_hallucination(
            "Untertitel im Auftrag des ZDF, 2021"
        ));
        assert!(super::is_hallucination(""));
        assert!(!super::is_hallucination("Wie wird das Wetter morgen?"));
    }

    #[test]
    fn audio_ctx_follows_the_recording_length() {
        assert_eq!(super::audio_ctx(3 * 16_000), 278);
        assert_eq!(super::audio_ctx(60 * 16_000), 1500);
    }
}
