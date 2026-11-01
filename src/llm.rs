//! OpenAI-compatible chat completions (Groq, Gemini, Ollama) with fallback to the next provider.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
        }
    }
}

pub type TokenStream = BoxStream<'static, Result<String>>;

#[async_trait]
pub trait ChatModel: Send + Sync {
    /// Streams the reply token by token.
    async fn stream(&self, messages: &[ChatMessage]) -> Result<TokenStream>;
    /// Whole reply at once; `json` asks for a JSON object.
    async fn complete(&self, messages: &[ChatMessage], json: bool) -> Result<String>;
    fn names(&self) -> Vec<String>;
}

#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Provider {
    pub fn new(name: &str, base_url: &str, api_key: String, model: String) -> Self {
        Self {
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
        }
    }
}

pub struct Llm {
    client: reqwest::Client,
    providers: Vec<Provider>,
}

impl Llm {
    pub fn new(providers: Vec<Provider>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { client, providers })
    }

    async fn post(
        &self,
        p: &Provider,
        body: &Value,
        timeout: Duration,
    ) -> Result<reqwest::Response> {
        let mut req = self
            .client
            .post(format!("{}/chat/completions", p.base_url))
            .timeout(timeout)
            .json(body);
        if !p.api_key.is_empty() {
            req = req.bearer_auth(&p.api_key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!(
                "HTTP {status}: {}",
                text.chars().take(300).collect::<String>()
            );
        }
        Ok(resp)
    }

    fn body(p: &Provider, messages: &[ChatMessage], stream: bool, json_mode: bool) -> Value {
        let mut body = json!({
            "model": p.model,
            "messages": messages,
            "stream": stream,
            "temperature": if json_mode { 0.2 } else { 0.8 },
            "max_tokens": if json_mode { 600 } else { 400 },
        });
        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        body
    }
}

#[async_trait]
impl ChatModel for Llm {
    async fn stream(&self, messages: &[ChatMessage]) -> Result<TokenStream> {
        // Fallback only covers failures before the reply starts; a stream that breaks midway ends.
        let mut errors = Vec::new();
        for p in &self.providers {
            let body = Self::body(p, messages, true, false);
            match self.post(p, &body, Duration::from_secs(60)).await {
                Ok(resp) => {
                    tracing::debug!("LLM-Stream von {}", p.name);
                    return Ok(sse_tokens(resp));
                }
                Err(e) => {
                    tracing::warn!("{} fehlgeschlagen: {e:#}", p.name);
                    errors.push(format!("{}: {e}", p.name));
                }
            }
        }
        Err(anyhow!(
            "kein KI-Anbieter erreichbar ({})",
            errors.join("; ")
        ))
    }

    async fn complete(&self, messages: &[ChatMessage], json_mode: bool) -> Result<String> {
        let mut errors = Vec::new();
        for p in &self.providers {
            let body = Self::body(p, messages, false, json_mode);
            let result = async {
                let v: Value = self
                    .post(p, &body, Duration::from_secs(60))
                    .await?
                    .json()
                    .await?;
                v["choices"][0]["message"]["content"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("Antwort ohne Inhalt"))
            }
            .await;
            match result {
                Ok(text) => return Ok(text),
                Err(e) => {
                    tracing::warn!("{} fehlgeschlagen: {e:#}", p.name);
                    errors.push(format!("{}: {e}", p.name));
                }
            }
        }
        Err(anyhow!(
            "kein KI-Anbieter erreichbar ({})",
            errors.join("; ")
        ))
    }

    fn names(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.name.clone()).collect()
    }
}

#[derive(Debug, PartialEq)]
enum SseEvent {
    Token(String),
    Done,
    Skip,
}

fn parse_sse_line(line: &str) -> SseEvent {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return SseEvent::Skip;
    };
    if data == "[DONE]" {
        return SseEvent::Done;
    }
    match serde_json::from_str::<Value>(data) {
        Ok(v) => match v["choices"][0]["delta"]["content"].as_str() {
            Some(t) if !t.is_empty() => SseEvent::Token(t.to_string()),
            _ => SseEvent::Skip,
        },
        Err(_) => SseEvent::Skip,
    }
}

fn sse_tokens(resp: reqwest::Response) -> TokenStream {
    Box::pin(async_stream::try_stream! {
        let mut bytes = resp.bytes_stream();
        // Bytes, not String: a UTF-8 character can be split across network chunks.
        let mut buf: Vec<u8> = Vec::new();
        'read: while let Some(chunk) = bytes.next().await {
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                match parse_sse_line(String::from_utf8_lossy(&line).trim()) {
                    SseEvent::Token(t) => yield t,
                    SseEvent::Done => break 'read,
                    SseEvent::Skip => {}
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_lines() {
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Hal"}}]}"#),
            SseEvent::Token("Hal".into())
        );
        assert_eq!(parse_sse_line("data: [DONE]"), SseEvent::Done);
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{}}]}"#),
            SseEvent::Skip
        );
        assert_eq!(parse_sse_line(": keep-alive"), SseEvent::Skip);
    }
}
