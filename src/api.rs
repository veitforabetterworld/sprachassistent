//! HTTP API. Everything except `/v1/health` needs `Authorization: Bearer <API_KEY>`
//! (or `X-API-Key`).

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

use crate::assistant::{Assistant, NOT_UNDERSTOOD};
use crate::audio;
use crate::speech::{is_hallucination, SpeechToText, TextToSpeech};

/// 30 s of 16 kHz 16-bit audio is under 1 MB; WAV uploads from a PC may be stereo/44.1 kHz.
const MAX_UPLOAD: usize = 12 * 1024 * 1024;
/// Recordings quieter than this are treated as silence.
const SILENCE_RMS: f32 = 0.002;

#[derive(Clone)]
pub struct AppState {
    pub assistant: Arc<Assistant>,
    pub stt: Option<Arc<dyn SpeechToText>>,
    pub tts: Option<Arc<dyn TextToSpeech>>,
    pub api_key: Arc<str>,
    /// One recognition at a time; two in parallel would only slow both down on the Pi.
    pub stt_slot: Arc<Semaphore>,
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/talk", post(talk))
        .route("/v1/transcribe", post(transcribe))
        .route("/v1/chat", post(chat))
        .route("/v1/speak", post(speak))
        .route("/v1/memory", get(memory).delete(clear_memory))
        .route("/v1/memory/facts", post(add_fact))
        .route(
            "/v1/memory/facts/{id}",
            delete(delete_fact).put(update_fact),
        )
        .route(
            "/v1/persona",
            get(get_persona).put(set_persona).delete(reset_persona),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_key));
    Router::new()
        .route("/v1/health", get(health))
        .merge(protected)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .with_state(state)
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("{e:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

type ApiResult<T> = Result<T, ApiError>;

async fn require_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if state.api_key.is_empty() {
        return next.run(req).await;
    }
    let headers = req.headers();
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or("");
    if constant_time_eq(given.as_bytes(), state.api_key.as_bytes()) {
        next.run(req).await
    } else {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "API-Schluessel fehlt oder ist falsch".into(),
        )
        .into_response()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "stt": state.stt.is_some(),
        "tts": state.tts.is_some(),
        "providers": state.assistant.llm.names(),
    }))
}

#[derive(Deserialize, Default)]
struct AudioQuery {
    /// `pcm` (default, raw s16le mono for the ESP32) or `wav`
    format: Option<String>,
    /// Sample rate of a raw PCM upload, default 16000
    rate: Option<u32>,
}

impl AudioQuery {
    fn wav(&self) -> bool {
        self.format.as_deref() == Some("wav")
    }
}

fn stt(state: &AppState) -> ApiResult<Arc<dyn SpeechToText>> {
    state.stt.clone().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Spracherkennung nicht verfuegbar".into(),
        )
    })
}

fn tts(state: &AppState) -> ApiResult<Arc<dyn TextToSpeech>> {
    state.tts.clone().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sprachausgabe nicht verfuegbar".into(),
        )
    })
}

async fn recognize(state: &AppState, body: &[u8], q: &AudioQuery) -> ApiResult<String> {
    let stt = stt(state)?;
    let samples = audio::decode_upload(body, q.rate.unwrap_or(audio::STT_RATE))
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    let seconds = samples.len() as f32 / audio::STT_RATE as f32;
    if seconds < 0.3 || audio::rms(&samples) < SILENCE_RMS {
        tracing::info!("Stille oder zu kurz ({seconds:.1} s)");
        return Ok(String::new());
    }
    let _slot = state
        .stt_slot
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let started = std::time::Instant::now();
    let text = tokio::task::spawn_blocking(move || stt.transcribe(&samples))
        .await
        .map_err(|e| anyhow::anyhow!(e))??;
    tracing::info!(
        "Erkannt in {:.1} s ({seconds:.1} s Audio): {text}",
        started.elapsed().as_secs_f32()
    );
    Ok(if is_hallucination(&text) {
        String::new()
    } else {
        text
    })
}

/// Speaks each sentence as soon as it is synthesized. Raw PCM by default, WAV with `format=wav`.
fn audio_response(
    tts: Arc<dyn TextToSpeech>,
    sentences: impl futures_util::Stream<Item = String> + Send + 'static,
    wav: bool,
    transcript: Option<&str>,
) -> Response {
    let rate = tts.sample_rate();
    let body = async_stream::stream! {
        if wav {
            yield Ok::<Bytes, std::io::Error>(Bytes::from(audio::streaming_wav_header(rate)));
        }
        let mut sentences = Box::pin(sentences);
        while let Some(sentence) = sentences.next().await {
            let tts = Arc::clone(&tts);
            match tokio::task::spawn_blocking(move || tts.synthesize(&sentence)).await {
                Ok(Ok(samples)) => yield Ok(Bytes::from(audio::f32_to_s16le(&samples))),
                Ok(Err(e)) => tracing::error!("Sprachausgabe: {e:#}"),
                Err(e) => tracing::error!("Sprachausgabe: {e}"),
            }
        }
    };
    let mut headers = HeaderMap::new();
    let content_type = if wav {
        "audio/wav".to_string()
    } else {
        format!("audio/L16; rate={rate}; channels=1")
    };
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).unwrap(),
    );
    headers.insert("x-sample-rate", HeaderValue::from(rate));
    if let Some(t) = transcript {
        let encoded = utf8_percent_encode(t, NON_ALPHANUMERIC).to_string();
        if let Ok(v) = HeaderValue::from_str(&encoded) {
            headers.insert("x-transcript", v);
        }
    }
    (headers, Body::from_stream(body)).into_response()
}

/// Audio in, spoken answer out: the endpoint the ESP32 uses.
async fn talk(
    State(state): State<AppState>,
    Query(q): Query<AudioQuery>,
    body: Bytes,
) -> ApiResult<Response> {
    let tts = tts(&state)?;
    let text = recognize(&state, &body, &q).await?;
    if text.is_empty() {
        let sentences = futures_util::stream::iter([NOT_UNDERSTOOD.to_string()]);
        return Ok(audio_response(tts, sentences, q.wav(), Some("")));
    }
    let sentences = Arc::clone(&state.assistant).reply_stream(text.clone());
    Ok(audio_response(tts, sentences, q.wav(), Some(&text)))
}

async fn transcribe(
    State(state): State<AppState>,
    Query(q): Query<AudioQuery>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let text = recognize(&state, &body, &q).await?;
    Ok(Json(json!({ "text": text })))
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

async fn chat(
    State(state): State<AppState>,
    Json(body): Json<TextBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text ist leer".into()));
    }
    let reply = Arc::clone(&state.assistant)
        .reply(body.text.trim().to_string())
        .await;
    Ok(Json(json!({ "reply": reply })))
}

async fn speak(
    State(state): State<AppState>,
    Query(q): Query<AudioQuery>,
    Json(body): Json<TextBody>,
) -> ApiResult<Response> {
    let tts = tts(&state)?;
    let mut splitter = crate::sentences::SentenceSplitter::default();
    let mut sentences = splitter.push(&body.text);
    sentences.extend(splitter.finish());
    Ok(audio_response(
        tts,
        futures_util::stream::iter(sentences),
        q.wav(),
        None,
    ))
}

#[derive(Deserialize)]
struct MemoryQuery {
    messages: Option<usize>,
}

async fn memory(
    State(state): State<AppState>,
    Query(q): Query<MemoryQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let m = &state.assistant.memory;
    Ok(Json(json!({
        "facts": m.facts()?,
        "messages": m.recent_messages(q.messages.unwrap_or(50))?,
    })))
}

#[derive(Deserialize)]
struct ClearQuery {
    /// `facts`, `messages` or `all` (default)
    what: Option<String>,
}

async fn clear_memory(
    State(state): State<AppState>,
    Query(q): Query<ClearQuery>,
) -> ApiResult<StatusCode> {
    let m = &state.assistant.memory;
    match q.what.as_deref().unwrap_or("all") {
        "facts" => m.clear_facts()?,
        "messages" => m.clear_messages()?,
        "all" => {
            m.clear_facts()?;
            m.clear_messages()?;
        }
        other => return Err(ApiError(StatusCode::BAD_REQUEST, format!("what={other}?"))),
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FactBody {
    content: String,
}

async fn add_fact(
    State(state): State<AppState>,
    Json(body): Json<FactBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if body.content.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "content ist leer".into()));
    }
    let id = state.assistant.memory.add_fact(&body.content)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

async fn update_fact(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<FactBody>,
) -> ApiResult<StatusCode> {
    if state.assistant.memory.update_fact(id, &body.content)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("Fakt {id} gibt es nicht"),
        ))
    }
}

async fn delete_fact(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    if state.assistant.memory.delete_fact(id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("Fakt {id} gibt es nicht"),
        ))
    }
}

async fn get_persona(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        state.assistant.persona.get(),
    )
}

async fn set_persona(State(state): State<AppState>, body: String) -> ApiResult<StatusCode> {
    if body.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Persoenlichkeit ist leer".into(),
        ));
    }
    state.assistant.persona.set(&body)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_persona(State(state): State<AppState>) -> ApiResult<StatusCode> {
    state.assistant.persona.reset()?;
    Ok(StatusCode::NO_CONTENT)
}
