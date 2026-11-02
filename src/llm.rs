//! OpenAI-compatible chat completions (Groq, Gemini, Ollama, ...) with fallback to the next provider.
//! Tool calls use the `tools` field of chat completions, which those endpoints share.
//! The Responses API is not used: the fallback chain is not an OpenAI-only API.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::secrets::redact;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON object as text, the shape chat completions return.
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn tool_result(id: &str, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_call_id: Some(id.to_string()),
            tool_calls: Vec::new(),
        }
    }

    fn to_api(&self) -> Value {
        if self.role == "tool" {
            return json!({
                "role": "tool",
                "tool_call_id": self.tool_call_id.clone().unwrap_or_default(),
                "content": self.content,
            });
        }
        if !self.tool_calls.is_empty() {
            let calls: Vec<Value> = self
                .tool_calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.arguments },
                    })
                })
                .collect();
            return json!({
                "role": "assistant",
                "content": if self.content.is_empty() { Value::Null } else { Value::String(self.content.clone()) },
                "tool_calls": calls,
            });
        }
        json!({ "role": self.role, "content": self.content })
    }
}

pub type TokenStream = BoxStream<'static, Result<String>>;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelTurn {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    /// Provider that produced this turn. The agent pins it for the rest of the run.
    pub provider: String,
}

/// Providers come with every call: each device has its own order (its provider first, then the others).
#[async_trait]
pub trait ChatModel: Send + Sync {
    /// Streams the reply token by token. Used when the turn has no tools.
    async fn stream(&self, providers: &[Provider], messages: &[ChatMessage])
        -> Result<TokenStream>;
    /// Whole reply at once. `json` asks for a JSON object. `tools` enables tool calls.
    async fn complete(
        &self,
        providers: &[Provider],
        messages: &[ChatMessage],
        tools: &[ToolDef],
        json: bool,
        force_tool: bool,
    ) -> Result<ModelTurn>;
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
}

impl Llm {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { client })
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
        // reqwest says "error sending request" for both; the admin test should name the reason.
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                anyhow!("keine Antwort nach {} s", timeout.as_secs())
            } else if e.is_connect() {
                anyhow!("keine Verbindung zu {}", p.base_url)
            } else {
                e.into()
            }
        })?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!(
                "HTTP {status}: {}",
                redact(&text.chars().take(1200).collect::<String>())
            );
        }
        Ok(resp)
    }

    fn body(
        p: &Provider,
        messages: &[ChatMessage],
        stream: bool,
        json_mode: bool,
        tools: &[ToolDef],
        force_tool: bool,
    ) -> Value {
        let api_messages: Vec<Value> = messages.iter().map(ChatMessage::to_api).collect();
        let max_tokens = if json_mode {
            600
        } else if tools.is_empty() {
            400
        } else {
            1024
        };
        let mut body = json!({
            "model": p.model,
            "messages": api_messages,
            "stream": stream,
            "temperature": if json_mode { 0.2 } else { 0.8 },
            "max_tokens": max_tokens,
        });
        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        if !tools.is_empty() {
            let chosen = select_tools(tools, messages);
            body["tools"] = json!(chosen.iter().map(|t| tool_function(t)).collect::<Vec<_>>());
            body["tool_choice"] = json!(if force_tool { "required" } else { "auto" });
        }
        body
    }
}

/// Home Assistant MCP tools ship multi-kilobyte descriptions. Small Groq models reject the
/// whole request once the input exceeds their tokens-per-minute budget (HTTP 413), and a tool
/// round needs two such requests inside the same minute.
const TOOL_DESCRIPTION_CHARS: usize = 120;
/// All compact Assist tools go to the model. Only a schema that is still huge after slimming
/// is left out. Phrasing must not decide which actions the model can see.
const TOOL_BUDGET_CHARS: usize = 6000;

fn tool_function(tool: &ToolDef) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": cap_text(&tool.description, TOOL_DESCRIPTION_CHARS),
            "parameters": slim_schema(&tool.parameters),
        }
    })
}

fn cap_text(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head).unwrap_or(&cut);
    format!("{cut}…")
}

fn slim_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if matches!(
                    key.as_str(),
                    "title"
                        | "examples"
                        | "example"
                        | "$schema"
                        | "$comment"
                        | "deprecated"
                        | "description"
                ) {
                    continue;
                }
                out.insert(key.clone(), slim_schema(child));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(slim_schema).collect()),
        other => other.clone(),
    }
}

fn model_tool_name(name: &str) -> String {
    let mut out = String::new();
    let mut underscore = false;
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '_' };
        if c == '_' {
            if underscore || out.is_empty() {
                continue;
            }
            underscore = true;
        } else {
            underscore = false;
        }
        out.push(c);
        if out.len() >= 64 {
            break;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "tool".into()
    } else {
        out
    }
}

/// `real name -> alias`. Groq rejects a call when the name still contains `--` or repeated `_`.
pub fn model_tool_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> std::collections::HashMap<String, String> {
    let mut used = std::collections::HashSet::new();
    let mut map = std::collections::HashMap::new();
    for name in names {
        let base = model_tool_name(name);
        let mut alias = base.clone();
        let mut n = 2;
        while !used.insert(alias.clone()) {
            let suffix = format!("_{n}");
            let keep = 64usize.saturating_sub(suffix.len());
            alias = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
            n += 1;
        }
        map.insert(name.to_string(), alias);
    }
    map
}

fn select_tools<'a>(tools: &'a [ToolDef], messages: &[ChatMessage]) -> Vec<&'a ToolDef> {
    let mentioned: Vec<&str> = messages
        .iter()
        .flat_map(|message| message.tool_calls.iter().map(|call| call.name.as_str()))
        .collect();
    let mut ranked: Vec<(usize, &ToolDef)> = tools
        .iter()
        .map(|tool| (tool_function(tool).to_string().len(), tool))
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
    let mut chosen: Vec<&ToolDef> = Vec::new();
    let mut used = 0;
    for (len, tool) in &ranked {
        if !mentioned.iter().any(|name| *name == tool.name) {
            continue;
        }
        used += len;
        chosen.push(*tool);
    }
    for (len, tool) in ranked {
        if chosen.iter().any(|have| have.name == tool.name) {
            continue;
        }
        if !chosen.is_empty() && used + len > TOOL_BUDGET_CHARS {
            continue;
        }
        used += len;
        chosen.push(tool);
    }
    chosen
}

/// Groq answers HTTP 400 when the model invents a tool name (`assist_home Dover`) or prints
/// the call as text. Keep the arguments when the body still contains them.
fn salvage_tool_call(error: &str, tools: &[ToolDef]) -> Option<ToolCall> {
    if error.contains("parameters for tool web_search")
        && tools.iter().any(|tool| tool.name == "web_search")
    {
        return Some(ToolCall {
            id: "salvage".into(),
            name: "web_search".into(),
            arguments: "{}".into(),
        });
    }
    let inline = inline_tool_call(error);
    if let Some(mut call) = inline.clone() {
        if let Some(name) = match_tool_name(&call.name, tools) {
            call.name = name;
            if call.arguments == "{}" {
                call.arguments = arguments_near(error);
            }
            return Some(call);
        }
    }
    let name = salvage_tool_name(error, tools)?;
    let arguments = inline
        .map(|call| call.arguments)
        .filter(|args| args != "{}")
        .unwrap_or_else(|| arguments_near(error));
    Some(ToolCall {
        id: "salvage".into(),
        name,
        arguments,
    })
}

fn arguments_near(error: &str) -> String {
    let error = unescape_jsonish(error);
    let mut args = serde_json::Map::new();
    for key in ["position", "name", "area"] {
        if let Some(value) = field_near(&error, key) {
            args.insert(key.to_string(), value);
        }
    }
    Value::Object(args).to_string()
}

fn unescape_jsonish(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    None => {
                        out.push('u');
                        out.push_str(&hex);
                    }
                }
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn field_near(error: &str, key: &str) -> Option<Value> {
    let at = error.find(key)? + key.len();
    let rest = error[at..].trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
    let token: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == ' ' || *c == '_' || *c == '-')
        .collect();
    let token = token.trim();
    if token.is_empty()
        || token.starts_with('u') && token[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    if let Ok(number) = token.parse::<i64>() {
        return Some(Value::Number(number.into()));
    }
    Some(Value::String(token.to_string()))
}

pub fn inline_tool_call(content: &str) -> Option<ToolCall> {
    let start = content.find("<function=")?;
    let rest = &content[start + "<function=".len()..];
    let end = rest.find('>')?;
    let name = rest[..end].trim();
    if name.is_empty() {
        return None;
    }
    let mut args = serde_json::Map::new();
    let mut cursor = &rest[end + 1..];
    while let Some(at) = cursor.find("<parameter=") {
        let body = &cursor[at + "<parameter=".len()..];
        let key_end = body.find('>')?;
        let key = body[..key_end].trim();
        let value_src = &body[key_end + 1..];
        let value_end = value_src.find("</parameter>").unwrap_or(value_src.len());
        let value = value_src[..value_end].trim();
        if !key.is_empty() {
            args.insert(key.to_string(), Value::String(value.to_string()));
        }
        if value_end >= value_src.len() {
            break;
        }
        cursor = &value_src[value_end + "</parameter>".len()..];
    }
    Some(ToolCall {
        id: "text".into(),
        name: name.to_string(),
        arguments: Value::Object(args).to_string(),
    })
}

fn match_tool_name(attempt: &str, tools: &[ToolDef]) -> Option<String> {
    if let Some(tool) = tools.iter().find(|tool| tool.name == attempt) {
        return Some(tool.name.clone());
    }
    salvage_tool_name(&format!("attempted to call tool '{attempt}'"), tools)
}

/// Groq answers HTTP 400 when the model invents a tool name (`assist_home Dover`). If that
/// string still matches exactly one offered tool by a long common prefix, use that tool.
fn salvage_tool_name(error: &str, tools: &[ToolDef]) -> Option<String> {
    let marker = "attempted to call tool '";
    let start = error.find(marker)? + marker.len();
    let rest = error.get(start..)?;
    let end = rest.find('\'')?;
    let attempt = alnum(&rest[..end]);
    if attempt.len() < 8 {
        return None;
    }
    let mut scored: Vec<(&str, usize)> = tools
        .iter()
        .map(|tool| {
            (
                tool.name.as_str(),
                common_prefix(&attempt, &alnum(&tool.name)),
            )
        })
        .filter(|(_, score)| *score >= 8)
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let (name, score) = scored.first().copied()?;
    if scored.iter().filter(|(_, other)| *other == score).count() != 1 {
        return None;
    }
    Some(name.to_string())
}

fn alnum(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn common_prefix(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(a, b)| a == b)
        .count()
}

fn no_provider(errors: Vec<String>) -> anyhow::Error {
    if errors.is_empty() {
        anyhow!("kein KI-Modell eingerichtet (Adminbereich: KI-Modelle, API-Key eintragen)")
    } else {
        anyhow!("kein KI-Modell erreichbar ({})", errors.join("; "))
    }
}

fn parse_turn(v: &Value, provider: &str) -> Result<ModelTurn> {
    let msg = &v["choices"][0]["message"];
    let content = match &msg["content"] {
        Value::String(s) => s.clone(),
        _ => String::new(),
    };
    let tool_calls: Vec<ToolCall> = msg["tool_calls"]
        .as_array()
        .map(|calls| calls.iter().filter_map(parse_tool_call).collect())
        .unwrap_or_default();
    if content.is_empty() && tool_calls.is_empty() {
        bail!("Antwort ohne Inhalt");
    }
    Ok(ModelTurn {
        content,
        tool_calls,
        input_tokens: v["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        output_tokens: v["usage"]["completion_tokens"].as_i64().unwrap_or(0) as i32,
        provider: provider.to_string(),
    })
}

fn parse_tool_call(v: &Value) -> Option<ToolCall> {
    let id = v["id"].as_str().unwrap_or("call").to_string();
    let name = v["function"]["name"].as_str()?.to_string();
    let arguments = match &v["function"]["arguments"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    Some(ToolCall {
        id,
        name,
        arguments,
    })
}

#[async_trait]
impl ChatModel for Llm {
    async fn stream(
        &self,
        providers: &[Provider],
        messages: &[ChatMessage],
    ) -> Result<TokenStream> {
        // Fallback only covers failures before the reply starts; a stream that breaks midway ends.
        let mut errors = Vec::new();
        for p in providers {
            let body = Self::body(p, messages, true, false, &[], false);
            match self.post(p, &body, Duration::from_secs(60)).await {
                Ok(resp) => {
                    tracing::debug!("LLM-Stream von {}", p.name);
                    return Ok(sse_tokens(resp));
                }
                Err(e) => {
                    tracing::warn!("{} fehlgeschlagen: {}", p.name, redact(&format!("{e:#}")));
                    errors.push(format!("{}: {}", p.name, redact(&e.to_string())));
                }
            }
        }
        Err(no_provider(errors))
    }

    async fn complete(
        &self,
        providers: &[Provider],
        messages: &[ChatMessage],
        tools: &[ToolDef],
        json_mode: bool,
        force_tool: bool,
    ) -> Result<ModelTurn> {
        let mut errors = Vec::new();
        for p in providers {
            match self.ask(p, messages, tools, json_mode, force_tool).await {
                Ok(turn) => return Ok(turn),
                Err(e) => {
                    tracing::warn!("{} fehlgeschlagen: {}", p.name, redact(&format!("{e:#}")));
                    errors.push(format!("{}: {}", p.name, redact(&e.to_string())));
                }
            }
        }
        Err(no_provider(errors))
    }
}

impl Llm {
    async fn ask(
        &self,
        provider: &Provider,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        json_mode: bool,
        force_tool: bool,
    ) -> Result<ModelTurn> {
        let body = Self::body(provider, messages, false, json_mode, tools, force_tool);
        match self.post(provider, &body, Duration::from_secs(60)).await {
            Ok(resp) => parse_turn(&resp.json().await?, &provider.name),
            Err(e) => {
                let text = e.to_string();
                if let Some(call) = salvage_tool_call(&text, tools) {
                    tracing::warn!(
                        "{}: Werkzeugaufruf korrigiert auf {}",
                        provider.name,
                        call.name
                    );
                    return Ok(ModelTurn {
                        content: String::new(),
                        tool_calls: vec![call],
                        input_tokens: 0,
                        output_tokens: 0,
                        provider: provider.name.clone(),
                    });
                }
                bail!(text)
            }
        }
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

    #[tokio::test]
    async fn timeouts_and_refused_connections_say_so() {
        let llm = Llm::new().unwrap();
        // Accepts and never answers, like an overloaded model.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((conn, _)) = silent.accept().await {
                held.push(conn);
            }
        });
        let provider = Provider::new(
            "Stumm",
            &format!("http://127.0.0.1:{port}"),
            String::new(),
            "m".into(),
        );
        let err = llm
            .post(&provider, &json!({}), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "keine Antwort nach 1 s");

        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let provider = Provider::new(
            "Weg",
            &format!("http://127.0.0.1:{port}"),
            String::new(),
            "m".into(),
        );
        let err = llm
            .post(&provider, &json!({}), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("keine Verbindung zu http://127.0.0.1:"),
            "{err}"
        );
    }

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

    #[test]
    fn tool_calls_roundtrip_into_the_api_body() {
        let message = ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: "{\"text\":\"x\"}".into(),
            }],
        };
        let api = message.to_api();
        assert_eq!(api["tool_calls"][0]["function"]["name"], "echo");
        assert!(api["content"].is_null());
    }

    #[test]
    fn parses_a_tool_turn() {
        let v = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "echo", "arguments": "{}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4}
        });
        let turn = parse_turn(&v, "Groq").unwrap();
        assert_eq!(turn.tool_calls[0].name, "echo");
        assert_eq!(turn.input_tokens, 3);
        assert_eq!(turn.provider, "Groq");
    }

    #[test]
    fn tool_schema_sent_to_the_model_drops_long_descriptions() {
        let tool = ToolDef {
            name: "ha_search".into(),
            description: "Suche ".repeat(80),
            parameters: json!({
                "type": "object",
                "description": "ausführliche Schema-Beschreibung die nicht mitgeschickt wird",
                "properties": {
                    "query": {"type": "string", "description": "lang", "examples": ["lampe"]}
                },
                "required": ["query"]
            }),
        };
        let sent = tool_function(&tool);
        let description = sent["function"]["description"].as_str().unwrap();
        assert!(description.chars().count() <= TOOL_DESCRIPTION_CHARS);
        assert!(description.ends_with('…'));
        assert!(sent["function"]["parameters"].get("description").is_none());
        let query = &sent["function"]["parameters"]["properties"]["query"];
        assert!(query.get("description").is_none());
        assert!(query.get("examples").is_none());
        assert_eq!(query["type"], "string");
        assert_eq!(sent["function"]["parameters"]["required"][0], "query");
    }

    #[test]
    fn phrasing_does_not_hide_device_tools() {
        let tools = vec![
            ToolDef {
                name: "assist_intent_HassSetPosition".into(),
                description: "Sets the position of a device".into(),
                parameters: json!({"type": "object", "properties": {"position": {"type": "integer"}}}),
            },
            ToolDef {
                name: "assist_intent_HassStopMoving".into(),
                description: "Stops a moving device".into(),
                parameters: json!({"type": "object"}),
            },
            ToolDef {
                name: "assist_intent_HassTurnOff".into(),
                description: "Turns off or closes a device".into(),
                parameters: json!({"type": "object", "properties": {"name": {"type": "string"}, "area": {"type": "string"}}}),
            },
            ToolDef {
                name: "assist_homeassistant_GetLiveContext".into(),
                description: "Current state of devices".into(),
                parameters: json!({"type": "object", "properties": {"area": {"type": "string"}}}),
            },
            ToolDef {
                name: "ha_search".into(),
                description: "Search for entities".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "blob": {
                            "type": "string",
                            "enum": (0..400).map(|i| format!("entity-{i}")).collect::<Vec<_>>()
                        }
                    }
                }),
            },
            ToolDef {
                name: "web_search".into(),
                description: "Schlagzeilen".into(),
                parameters: json!({"type": "object"}),
            },
            ToolDef {
                name: "assist_assist_satellite_HassBroadcast".into(),
                description: "Broadcast a message".into(),
                parameters: json!({"type": "object", "properties": {"message": {"type": "string"}}}),
            },
        ];
        let messages = [ChatMessage::new("user", "bitte das Büro etwas abdunkeln")];
        let chosen = select_tools(&tools, &messages);
        let names: Vec<_> = chosen.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains(&"assist_intent_HassSetPosition"));
        assert!(names.contains(&"assist_intent_HassTurnOff"));
        assert!(names.contains(&"assist_homeassistant_GetLiveContext"));
        assert!(names.contains(&"web_search"));
        assert!(!names.contains(&"ha_search"));
    }

    #[test]
    fn oversized_tool_list_keeps_the_state_tool_inside_the_budget() {
        let mut tools = vec![ToolDef {
            name: "assist__homeassistant__GetLiveContext".into(),
            description: "aktueller Zustand der Sensoren".into(),
            parameters: json!({"type": "object", "properties": {"area": {"type": "string"}}}),
        }];
        for i in 0..30 {
            tools.push(ToolDef {
                name: format!("assist__media_player__HassMedia{i}"),
                description: "x".repeat(400),
                parameters: json!({
                    "type": "object",
                    "properties": {"x": {"type": "string", "description": "y".repeat(200)}}
                }),
            });
        }
        let messages = [ChatMessage::new("user", "Wie ist die Temperatur im Büro?")];
        let chosen = select_tools(&tools, &messages);
        let size: usize = chosen
            .iter()
            .map(|tool| tool_function(tool).to_string().len())
            .sum();
        assert!(chosen
            .iter()
            .any(|tool| tool.name.contains("GetLiveContext")));
        assert!(size <= TOOL_BUDGET_CHARS);
        assert!(chosen.len() < tools.len());
    }

    #[test]
    fn model_tool_names_drop_hyphens_and_stay_unique() {
        let names = model_tool_names([
            "ha-mcp-server-tool-search__ha_search_tools",
            "assist__homeassistant__GetLiveContext",
        ]);
        assert_eq!(
            names["assist__homeassistant__GetLiveContext"],
            "assist_homeassistant_GetLiveContext"
        );
        assert_eq!(
            names["ha-mcp-server-tool-search__ha_search_tools"],
            "ha_mcp_server_tool_search_ha_search_tools"
        );
    }

    #[test]
    fn garbled_groq_tool_name_maps_to_the_one_matching_tool() {
        let tools = vec![
            ToolDef {
                name: "assist_homeassistant_GetLiveContext".into(),
                description: String::new(),
                parameters: json!({}),
            },
            ToolDef {
                name: "assist_intent_HassTurnOn".into(),
                description: String::new(),
                parameters: json!({}),
            },
            ToolDef {
                name: "assist_intent_HassTurnOff".into(),
                description: String::new(),
                parameters: json!({}),
            },
        ];
        let error = "HTTP 400: {\"message\":\"tool call validation failed: attempted to call tool 'assist_home Dover' which was not in request.tools\"}";
        assert_eq!(
            salvage_tool_name(error, &tools).as_deref(),
            Some("assist_homeassistant_GetLiveContext")
        );
        let ambiguous = "attempted to call tool 'assist_intent_HassTurn' which was not";
        assert!(salvage_tool_name(ambiguous, &tools).is_none());
    }

    #[test]
    fn printed_tool_call_keeps_its_query() {
        let tools = vec![ToolDef {
            name: "web_search".into(),
            description: String::new(),
            parameters: json!({}),
        }];
        let error = "HTTP 400: failed_generation <function=web_search><parameter=query>Schlagzeilen heute</parameter></function>";
        let call = salvage_tool_call(error, &tools).unwrap();
        assert_eq!(call.name, "web_search");
        assert!(call.arguments.contains("Schlagzeilen heute"));
    }

    #[test]
    fn salvaged_position_keeps_the_number() {
        let tools = vec![ToolDef {
            name: "assist_intent_HassSetPosition".into(),
            description: String::new(),
            parameters: json!({}),
        }];
        let error = "attempted to call tool 'assist_intent_HassSetPos' which was not in request.tools position\\\":70 name\\\":Rolladen";
        let call = salvage_tool_call(error, &tools).unwrap();
        assert_eq!(call.name, "assist_intent_HassSetPosition");
        assert!(call.arguments.contains("70"));
        assert!(call.arguments.contains("Rolladen"));
    }

    #[test]
    fn unicode_escape_is_not_an_area_name() {
        let tools = vec![ToolDef {
            name: "assist_homeassistant_GetLiveContext".into(),
            description: String::new(),
            parameters: json!({}),
        }];
        let error = "attempted to call tool 'assist_homeassistant_GetLive' which was not \\u003cparameter=area\\u003eBüro\\u003c/parameter\\u003e";
        let call = salvage_tool_call(error, &tools).unwrap();
        assert!(call.arguments.contains("Büro"));
        assert!(!call.arguments.contains("u003e"));
    }
}
