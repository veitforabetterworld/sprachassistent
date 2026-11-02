//! The few settings that have to come from the environment (`.env` in the working directory).
//! Everything else lives in the database and is changed in the admin area.

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub host: String,
    pub port: u16,
    /// Whisper model and Piper voices (`vits-piper-*` directories).
    pub models_dir: PathBuf,
    /// Self-hosted SearXNG with the JSON format enabled (`deployments/pi/searxng`).
    pub searxng_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            database_url: var(
                "DATABASE_URL",
                "postgres://sprachassistent:sprachassistent@localhost:5432/sprachassistent",
            ),
            host: var("API_HOST", "127.0.0.1"),
            port: var("API_PORT", "3020")
                .parse()
                .context("API_PORT ist keine Zahl")?,
            models_dir: var("MODELS_DIR", "models").into(),
            searxng_url: var("SEARXNG_URL", "http://127.0.0.1:8888"),
        })
    }
}

fn var(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}
