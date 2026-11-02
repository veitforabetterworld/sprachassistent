use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;
use tracing_subscriber::EnvFilter;

use sprachassistent::api::{self, AppState};
use sprachassistent::assistant::Assistant;
use sprachassistent::config::Config;
use sprachassistent::llm::Llm;
use sprachassistent::secrets::Secrets;
use sprachassistent::speech::{SpeechToText, VoiceBank};
use sprachassistent::store::Store;
use sprachassistent::tools::LiveTools;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;
    let store = Store::connect(&config.database_url)
        .await
        .context("Datenbank nicht erreichbar (DATABASE_URL)")?;
    store.migrate().await?;
    // Speech settings are read once; the admin area says that they need a restart.
    let settings = store.settings().await?;

    let stt = tokio::task::spawn_blocking({
        let (config, settings) = (config.clone(), settings.clone());
        move || load_stt(&config, &settings)
    })
    .await?;
    let voices = load_voices(&config, &settings);
    tracing::info!("Stimmen: {:?}", voices.list());

    let secrets = Secrets::from_env();
    let tools = Arc::new(LiveTools::new(store.clone(), secrets.clone())?);
    let state = AppState {
        assistant: Arc::new(Assistant {
            llm: Arc::new(Llm::new()?),
            store,
            tools,
            secrets,
        }),
        stt,
        voices,
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

#[cfg_attr(not(any(feature = "whisper", feature = "piper")), allow(dead_code))]
fn setting<T: std::str::FromStr>(settings: &HashMap<String, String>, key: &str, default: T) -> T {
    settings
        .get(key)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// A missing model disables recognition instead of stopping the server; `/v1/health` shows it.
#[allow(unused_variables)]
fn load_stt(config: &Config, settings: &HashMap<String, String>) -> Option<Arc<dyn SpeechToText>> {
    #[cfg(feature = "whisper")]
    {
        let started = std::time::Instant::now();
        let model: String = setting(settings, "whisper_model", String::new());
        let model = if std::path::Path::new(&model).is_absolute() {
            model.into()
        } else {
            // Relative paths in the settings are relative to the models directory's parent (the repo).
            config
                .models_dir
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(model)
        };
        match sprachassistent::speech::WhisperStt::load(
            &model,
            setting(settings, "whisper_threads", 4),
            setting(settings, "whisper_beam_size", 1),
            &setting(settings, "whisper_language", "de".to_string()),
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
fn load_voices(config: &Config, settings: &HashMap<String, String>) -> Arc<dyn VoiceBank> {
    #[cfg(feature = "piper")]
    return Arc::new(sprachassistent::speech::PiperVoices::new(
        config.models_dir.clone(),
        setting(settings, "tts_threads", 2),
    ));
    #[cfg(not(feature = "piper"))]
    {
        tracing::warn!("ohne Feature piper gebaut, keine Sprachausgabe");
        Arc::new(NoVoices)
    }
}

#[cfg(not(feature = "piper"))]
struct NoVoices;

#[cfg(not(feature = "piper"))]
impl VoiceBank for NoVoices {
    fn list(&self) -> Vec<String> {
        Vec::new()
    }

    fn voice(&self, id: &str) -> Result<Arc<dyn sprachassistent::speech::TextToSpeech>> {
        anyhow::bail!("ohne Feature piper gebaut, Stimme {id} gibt es nicht")
    }
}
