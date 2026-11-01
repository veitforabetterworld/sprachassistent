//! Settings from the environment; `.env` in the working directory is loaded first.

use std::env;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::llm::Provider;

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    /// Empty disables the check (LAN testing only).
    pub api_key: String,
    pub data_dir: PathBuf,
    pub whisper_model: PathBuf,
    pub whisper_threads: i32,
    pub whisper_beam_size: i32,
    pub whisper_language: String,
    pub piper_model: PathBuf,
    pub piper_threads: i32,
    pub piper_speed: f32,
    pub providers: Vec<Provider>,
    pub history_messages: usize,
    pub memory_extraction: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            host: var("API_HOST", "127.0.0.1"),
            port: parse("API_PORT", 3020)?,
            api_key: var("API_KEY", ""),
            data_dir: var("DATA_DIR", "data").into(),
            whisper_model: var("WHISPER_MODEL", "models/whisper/ggml-small-q5_1.bin").into(),
            whisper_threads: parse("WHISPER_THREADS", 4)?,
            whisper_beam_size: parse("WHISPER_BEAM_SIZE", 1)?,
            whisper_language: var("WHISPER_LANGUAGE", "de"),
            piper_model: var(
                "PIPER_MODEL",
                "models/vits-piper-de_DE-thorsten-medium/de_DE-thorsten-medium.onnx",
            )
            .into(),
            piper_threads: parse("PIPER_THREADS", 2)?,
            piper_speed: parse("PIPER_SPEED", 1.0)?,
            providers: providers_from_env()?,
            history_messages: parse("HISTORY_MESSAGES", 12)?,
            memory_extraction: parse("MEMORY_EXTRACTION", true)?,
        })
    }
}

fn var(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn parse<T: std::str::FromStr>(key: &str, default: T) -> Result<T> {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .trim()
            .parse()
            .ok()
            .with_context(|| format!("{key}: ungueltiger Wert {v:?}")),
        _ => Ok(default),
    }
}

/// `LLM_PROVIDERS=groq,gemini` in fallback order. Providers without a key are skipped.
fn providers_from_env() -> Result<Vec<Provider>> {
    let mut providers = Vec::new();
    for name in var("LLM_PROVIDERS", "groq,gemini")
        .split(',')
        .map(str::trim)
    {
        let provider = match name {
            "groq" => Provider::new(
                "groq",
                "https://api.groq.com/openai/v1",
                var("GROQ_API_KEY", ""),
                var("GROQ_MODEL", "llama-3.3-70b-versatile"),
            ),
            "gemini" => Provider::new(
                "gemini",
                "https://generativelanguage.googleapis.com/v1beta/openai",
                var("GEMINI_API_KEY", ""),
                var("GEMINI_MODEL", "gemini-2.5-flash-lite"),
            ),
            "ollama" => Provider::new(
                "ollama",
                &var("OLLAMA_BASE_URL", "http://127.0.0.1:11434/v1"),
                String::new(),
                var("OLLAMA_MODEL", "qwen2.5:3b"),
            ),
            "" => continue,
            other => bail!("LLM_PROVIDERS: unbekannter Anbieter {other:?} (groq, gemini, ollama)"),
        };
        if provider.api_key.is_empty() && provider.name != "ollama" {
            tracing::warn!("{name}: kein API-Schluessel gesetzt, Anbieter wird uebersprungen");
            continue;
        }
        providers.push(provider);
    }
    Ok(providers)
}
