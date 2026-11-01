use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;
use tracing_subscriber::EnvFilter;

use sprachassistent::api::{self, AppState};
use sprachassistent::assistant::{Assistant, PersonaStore};
use sprachassistent::config::Config;
use sprachassistent::llm::Llm;
use sprachassistent::memory::MemoryStore;
use sprachassistent::speech::{SpeechToText, TextToSpeech};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;
    if config.api_key.is_empty() {
        tracing::warn!("API_KEY ist leer, die API ist ohne Schluessel erreichbar");
    }
    if config.providers.is_empty() {
        tracing::warn!("kein KI-Anbieter konfiguriert (LLM_PROVIDERS und API-Schluessel pruefen)");
    }

    let memory = Arc::new(MemoryStore::open(&config.data_dir.join("memory.sqlite"))?);
    let assistant = Arc::new(Assistant {
        llm: Arc::new(Llm::new(config.providers.clone())?),
        memory,
        persona: PersonaStore::new(config.data_dir.join("persona.md")),
        history_messages: config.history_messages,
        memory_extraction: config.memory_extraction,
    });

    let (stt, tts) = tokio::task::spawn_blocking({
        let config = config.clone();
        move || (load_stt(&config), load_tts(&config))
    })
    .await?;

    let state = AppState {
        assistant,
        stt,
        tts,
        api_key: config.api_key.clone().into(),
        stt_slot: Arc::new(Semaphore::new(1)),
    };
    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("{addr} belegt?"))?;
    tracing::info!("hoert auf http://{addr}");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// A missing model disables the endpoint instead of stopping the server, so chat and memory
/// keep working and `/v1/health` shows what is missing.
#[allow(unused_variables)]
fn load_stt(config: &Config) -> Option<Arc<dyn SpeechToText>> {
    #[cfg(feature = "whisper")]
    {
        let started = std::time::Instant::now();
        match sprachassistent::speech::WhisperStt::load(
            &config.whisper_model,
            config.whisper_threads,
            config.whisper_beam_size,
            &config.whisper_language,
        ) {
            Ok(stt) => {
                tracing::info!(
                    "Whisper geladen in {:.1} s",
                    started.elapsed().as_secs_f32()
                );
                return Some(Arc::new(stt));
            }
            Err(e) => tracing::error!("{e:#}"),
        }
    }
    #[cfg(not(feature = "whisper"))]
    tracing::warn!("ohne Feature whisper gebaut, keine Spracherkennung");
    None
}

#[allow(unused_variables)]
fn load_tts(config: &Config) -> Option<Arc<dyn TextToSpeech>> {
    #[cfg(feature = "piper")]
    {
        let started = std::time::Instant::now();
        match sprachassistent::speech::PiperTts::load(
            &config.piper_model,
            config.piper_threads,
            config.piper_speed,
        ) {
            Ok(tts) => {
                tracing::info!("Piper geladen in {:.1} s", started.elapsed().as_secs_f32());
                return Some(Arc::new(tts));
            }
            Err(e) => tracing::error!("{e:#}"),
        }
    }
    #[cfg(not(feature = "piper"))]
    tracing::warn!("ohne Feature piper gebaut, keine Sprachausgabe");
    None
}
