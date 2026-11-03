use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use sprachassistent::api::{self, AppState};
use sprachassistent::assistant::Assistant;
use sprachassistent::browser::Browser;
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
    let pending = store
        .pending_migrations(std::path::Path::new("db/migrations"))
        .await
        .context("Migrationen nicht lesbar (db/migrations)")?;
    if !pending.is_empty() {
        anyhow::bail!(
            "Datenbank nicht migriert ({}): pnpm --filter @sprachassistent/api migrate",
            pending.join(", ")
        );
    }
    // Speech settings are read once; the admin area says that they need a restart.
    let settings = store.settings().await?;

    let stt = tokio::task::spawn_blocking({
        let (config, settings) = (config.clone(), settings.clone());
        move || load_stt(&config, &settings)
    })
    .await?;
    let voices = load_voices(&config, &settings);
    tracing::info!("Stimmen: {:?}", voices.list());
    // Loading a voice takes about 2 s on the Pi; the first answer after a restart should not wait.
    if let Ok(used) = store.device_voices().await {
        let voices = Arc::clone(&voices);
        tokio::task::spawn_blocking(move || {
            for id in used {
                if let Err(e) = voices.voice(&id) {
                    tracing::warn!("Stimme {id} nicht vorgeladen: {e:#}");
                }
            }
        });
    }

    let secrets = Secrets::from_env();
    let browser = Browser::start(Some(&config.browser_bin), &config.browser_user)
        .await
        .map(Arc::new);
    let tools = Arc::new(LiveTools::new(
        store.clone(),
        secrets.clone(),
        &config.searxng_url,
        browser,
    )?);
    let state = AppState {
        assistant: Arc::new(Assistant {
            llm: Arc::new(Llm::new()?),
            store,
            tools,
            secrets,
        }),
        stt,
        voices,
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

#[cfg_attr(not(feature = "speech"), allow(dead_code))]
fn setting<T: std::str::FromStr>(settings: &HashMap<String, String>, key: &str, default: T) -> T {
    settings
        .get(key)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// A missing model disables recognition instead of stopping the server; `/v1/health` shows it.
#[allow(unused_variables)]
fn load_stt(config: &Config, settings: &HashMap<String, String>) -> Option<Arc<dyn SpeechToText>> {
    #[cfg(feature = "speech")]
    {
        let started = std::time::Instant::now();
        let model: String = setting(settings, "stt_model", String::new());
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
        match sprachassistent::speech::KrokoStt::load(&model, setting(settings, "stt_threads", 1)) {
            Ok(stt) => {
                tracing::info!(
                    "Spracherkennung geladen in {:.1} s",
                    started.elapsed().as_secs_f32()
                );
                return Some(Arc::new(stt));
            }
            Err(e) => tracing::error!("{e:#}"),
        }
    }
    #[cfg(not(feature = "speech"))]
    tracing::warn!("ohne Feature speech gebaut, keine Spracherkennung");
    None
}

#[allow(unused_variables)]
fn load_voices(config: &Config, settings: &HashMap<String, String>) -> Arc<dyn VoiceBank> {
    #[cfg(feature = "speech")]
    return Arc::new(sprachassistent::speech::PiperVoices::new(
        config.models_dir.clone(),
        setting(settings, "tts_threads", 2),
    ));
    #[cfg(not(feature = "speech"))]
    {
        tracing::warn!("ohne Feature speech gebaut, keine Sprachausgabe");
        Arc::new(NoVoices)
    }
}

#[cfg(not(feature = "speech"))]
struct NoVoices;

#[cfg(not(feature = "speech"))]
impl VoiceBank for NoVoices {
    fn list(&self) -> Vec<String> {
        Vec::new()
    }

    fn voice(&self, id: &str) -> Result<Arc<dyn sprachassistent::speech::TextToSpeech>> {
        anyhow::bail!("ohne Feature speech gebaut, Stimme {id} gibt es nicht")
    }
}
