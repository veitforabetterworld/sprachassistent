//! Speech recognition (streaming Kroko model) and synthesis (Piper voices), both local through
//! sherpa-onnx. Both run synchronously on the CPU and are called through `spawn_blocking`.

pub trait SpeechToText: Send + Sync {
    /// Starts a recognition that takes the audio while it is still being recorded.
    fn start(&self) -> Box<dyn Transcription>;

    /// Recognizes a finished recording (16 kHz mono) at once.
    fn transcribe(&self, audio: &[f32]) -> String {
        let mut transcription = self.start();
        transcription.push(audio);
        transcription.finish()
    }
}

/// One running recognition. Each piece is decoded as it arrives, so at the end only the last
/// moment is left to decode.
pub trait Transcription: Send {
    /// 16 kHz mono, pieces of any size.
    fn push(&mut self, audio: &[f32]);
    /// The recording has ended: the text of everything pushed.
    fn finish(self: Box<Self>) -> String;
}

pub trait TextToSpeech: Send + Sync {
    fn sample_rate(&self) -> u32;
    /// Mono f32 samples at `sample_rate()`; `speed` above 1 speaks faster.
    fn synthesize(&self, text: &str, speed: f32) -> anyhow::Result<Vec<f32>>;
}

/// The installed voices; each device picks one by id.
pub trait VoiceBank: Send + Sync {
    /// Ids of the installed voices, sorted.
    fn list(&self) -> Vec<String>;
    /// Loads the voice on first use.
    fn voice(&self, id: &str) -> anyhow::Result<std::sync::Arc<dyn TextToSpeech>>;
}

/// "vits-piper-de_DE-thorsten-medium" -> "thorsten (de_DE, medium)".
pub fn voice_label(id: &str) -> String {
    let name = id.strip_prefix("vits-piper-").unwrap_or(id);
    let parts: Vec<&str> = name.split('-').collect();
    match parts.as_slice() {
        [lang, speaker, quality] => format!("{speaker} ({lang}, {quality})"),
        _ => name.to_string(),
    }
}

#[cfg(feature = "speech")]
pub use sherpa::{KrokoStt, PiperVoices};

#[cfg(feature = "speech")]
mod sherpa {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use anyhow::{anyhow, Result};
    use sherpa_onnx::{
        GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
        OfflineTtsVitsModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    };

    use super::{TextToSpeech, Transcription};
    use crate::audio::STT_RATE;

    /// Silence after the recording: the model looks ahead and would otherwise drop the last word.
    const TAIL_SECONDS: f32 = 1.0;

    fn path(dir: &Path, file: &str) -> Result<String> {
        let path = dir.join(file);
        if !path.exists() {
            return Err(anyhow!(
                "{} fehlt (deployments/pi/download-models.sh)",
                path.display()
            ));
        }
        Ok(path.display().to_string())
    }

    /// German streaming zipformer by Kroko in the layout of the sherpa-onnx archives:
    /// `encoder.onnx`, `decoder.onnx`, `joiner.onnx`, `tokens.txt`.
    pub struct KrokoStt {
        recognizer: Arc<OnlineRecognizer>,
    }

    impl KrokoStt {
        pub fn load(dir: &Path, threads: i32) -> Result<Self> {
            let mut config = OnlineRecognizerConfig::default();
            config.model_config.transducer.encoder = Some(path(dir, "encoder.onnx")?);
            config.model_config.transducer.decoder = Some(path(dir, "decoder.onnx")?);
            config.model_config.transducer.joiner = Some(path(dir, "joiner.onnx")?);
            config.model_config.tokens = Some(path(dir, "tokens.txt")?);
            config.model_config.num_threads = threads;
            config.model_config.provider = Some("cpu".into());
            config.decoding_method = Some("greedy_search".into());
            let recognizer = OnlineRecognizer::create(&config)
                .ok_or_else(|| anyhow!("Spracherkennung {} nicht geladen", dir.display()))?;
            Ok(Self {
                recognizer: Arc::new(recognizer),
            })
        }
    }

    impl super::SpeechToText for KrokoStt {
        fn start(&self) -> Box<dyn Transcription> {
            Box::new(KrokoTranscription {
                stream: self.recognizer.create_stream(),
                recognizer: Arc::clone(&self.recognizer),
            })
        }
    }

    struct KrokoTranscription {
        // Declared first so it is dropped before the recognizer it belongs to.
        stream: OnlineStream,
        recognizer: Arc<OnlineRecognizer>,
    }

    impl KrokoTranscription {
        fn decode(&self) {
            while self.recognizer.is_ready(&self.stream) {
                self.recognizer.decode(&self.stream);
            }
        }
    }

    impl Transcription for KrokoTranscription {
        fn push(&mut self, audio: &[f32]) {
            self.stream.accept_waveform(STT_RATE as i32, audio);
            self.decode();
        }

        fn finish(self: Box<Self>) -> String {
            let tail = vec![0.0; (STT_RATE as f32 * TAIL_SECONDS) as usize];
            self.stream.accept_waveform(STT_RATE as i32, &tail);
            self.stream.input_finished();
            self.decode();
            self.recognizer
                .get_result(&self.stream)
                .map(|result| result.text.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default()
        }
    }

    /// Piper voices in the layout of the sherpa-onnx `vits-piper-*` archives: one directory per voice with
    /// the .onnx model, `tokens.txt` and `espeak-ng-data/`.
    pub struct PiperVoices {
        dir: PathBuf,
        threads: i32,
        loaded: Mutex<HashMap<String, Arc<PiperTts>>>,
    }

    impl PiperVoices {
        pub fn new(dir: PathBuf, threads: i32) -> Self {
            Self {
                dir,
                threads,
                loaded: Mutex::new(HashMap::new()),
            }
        }
    }

    impl super::VoiceBank for PiperVoices {
        fn list(&self) -> Vec<String> {
            let mut ids: Vec<String> = std::fs::read_dir(&self.dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.path().join("tokens.txt").exists())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|name| name.starts_with("vits-piper-"))
                        .collect()
                })
                .unwrap_or_default();
            ids.sort();
            ids
        }

        fn voice(&self, id: &str) -> Result<Arc<dyn TextToSpeech>> {
            if !self.list().iter().any(|v| v == id) {
                return Err(anyhow!("Stimme {id} ist nicht installiert"));
            }
            let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(voice) = loaded.get(id) {
                return Ok(voice.clone());
            }
            let started = std::time::Instant::now();
            let voice = Arc::new(PiperTts::load(&self.dir.join(id), self.threads)?);
            tracing::info!(
                "Stimme {id} geladen in {:.1} s",
                started.elapsed().as_secs_f32()
            );
            loaded.insert(id.to_string(), voice.clone());
            Ok(voice)
        }
    }

    pub struct PiperTts {
        tts: Mutex<OfflineTts>,
        sample_rate: u32,
    }

    impl PiperTts {
        fn load(dir: &Path, threads: i32) -> Result<Self> {
            let model = std::fs::read_dir(dir)?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|ext| ext == "onnx"))
                .ok_or_else(|| anyhow!("{}: kein .onnx-Modell", dir.display()))?;
            let config = OfflineTtsConfig {
                model: OfflineTtsModelConfig {
                    vits: OfflineTtsVitsModelConfig {
                        model: Some(model.display().to_string()),
                        tokens: Some(path(dir, "tokens.txt")?),
                        data_dir: Some(path(dir, "espeak-ng-data")?),
                        ..Default::default()
                    },
                    num_threads: threads,
                    provider: Some("cpu".into()),
                    ..Default::default()
                },
                max_num_sentences: 1,
                ..Default::default()
            };
            let tts = OfflineTts::create(&config)
                .ok_or_else(|| anyhow!("Stimme {} nicht geladen", dir.display()))?;
            let sample_rate = tts.sample_rate() as u32;
            let voice = Self {
                tts: Mutex::new(tts),
                sample_rate,
            };
            // The first sentence of a fresh model is slow; it should not be the first answer.
            voice.synthesize("Hallo.", 1.0)?;
            Ok(voice)
        }
    }

    impl TextToSpeech for PiperTts {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn synthesize(&self, text: &str, speed: f32) -> Result<Vec<f32>> {
            let tts = self.tts.lock().unwrap_or_else(|e| e.into_inner());
            let config = GenerationConfig {
                speed,
                ..Default::default()
            };
            let audio = tts
                .generate_with_config(text, &config, None::<fn(&[f32], f32) -> bool>)
                .ok_or_else(|| anyhow!("Piper: keine Sprachausgabe für {text:?}"))?;
            Ok(audio.samples().to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn labels_voices() {
        assert_eq!(
            super::voice_label("vits-piper-de_DE-thorsten-medium"),
            "thorsten (de_DE, medium)"
        );
    }
}
