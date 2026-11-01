//! HTTP API of the voice server.
//!
//! Public (nginx forwards them): `GET /v1/health` and `POST /v1/talk` for the ESPs, authenticated with an ESP key
//! from the admin area (`Authorization: Bearer spa_...` or `X-API-Key`).
//! Internal (localhost only, used by apps/api for the admin area): `/internal/...`.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::assistant::{Assistant, NOT_UNDERSTOOD};
use crate::audio;
use crate::llm::ChatMessage;
use crate::sentences::SentenceSplitter;
use crate::speech::{is_hallucination, voice_label, SpeechToText, TextToSpeech, VoiceBank};
use crate::store::Device;

/// 30 s of 16 kHz 16-bit audio is under 1 MB; WAV uploads from a PC may be stereo/44.1 kHz.
const MAX_UPLOAD: usize = 12 * 1024 * 1024;
/// Recordings quieter than this are treated as silence.
const SILENCE_RMS: f32 = 0.002;

#[derive(Clone)]
pub struct AppState {
    pub assistant: Arc<Assistant>,
    pub stt: Option<Arc<dyn SpeechToText>>,
    pub voices: Arc<dyn VoiceBank>,
    /// One recognition at a time; two in parallel would only slow both down on the Pi.
    pub stt_slot: Arc<Semaphore>,
}

pub fn router(state: AppState) -> Router {
    let internal = Router::new()
        .route("/internal/voices", get(voices))
        .route("/internal/devices/{id}/chat", post(device_chat))
        .route("/internal/devices/{id}/speak", post(device_speak))
        .route("/internal/providers/{id}/test", post(provider_test))
        .route_layer(middleware::from_fn(local_only));
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/talk", post(talk))
        .merge(internal)
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

/// nginx sets X-Forwarded-For on everything it forwards; internal routes are for apps/api on the same host.
async fn local_only(req: Request, next: Next) -> Response {
    if req.headers().contains_key("x-forwarded-for") {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let store = &state.assistant.store;
    Json(json!({
        "status": "ok",
        "db": store.ping().await,
        "stt": state.stt.is_some(),
        "voices": state.voices.list(),
        "providers": store.provider_names().await.unwrap_or_default(),
    }))
}

async fn voices(State(state): State<AppState>) -> Json<serde_json::Value> {
    let voices: Vec<_> = state
        .voices
        .list()
        .into_iter()
        .map(|id| json!({ "id": id, "label": voice_label(&id) }))
        .collect();
    Json(json!({ "voices": voices }))
}

#[derive(Deserialize, Default)]
struct TalkQuery {
    /// Needed when the key covers several devices.
    device: Option<Uuid>,
    /// `pcm` (default, raw s16le mono for the ESP32) or `wav`
    format: Option<String>,
    /// Sample rate of a raw PCM upload, default 16000
    rate: Option<u32>,
}

fn device_key(headers: &HeaderMap) -> &str {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or("")
}

async fn voice_of(state: &AppState, device: &Device) -> ApiResult<Arc<dyn TextToSpeech>> {
    let voices = Arc::clone(&state.voices);
    let id = device.voice.clone();
    tokio::task::spawn_blocking(move || voices.voice(&id))
        .await
        .map_err(|e| anyhow::anyhow!(e))?
        .map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")))
}

async fn recognize(state: &AppState, body: &[u8], rate: u32) -> ApiResult<String> {
    let stt = state.stt.clone().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Spracherkennung nicht verfuegbar".into(),
        )
    })?;
    let samples = audio::decode_upload(body, rate)
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

/// Speaks each sentence as soon as it is synthesized. Raw PCM by default, WAV with `wav`.
fn audio_response(
    tts: Arc<dyn TextToSpeech>,
    speed: f32,
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
            match tokio::task::spawn_blocking(move || tts.synthesize(&sentence, speed)).await {
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
    Query(q): Query<TalkQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let device = state
        .assistant
        .store
        .authenticate_device(device_key(&headers), q.device)
        .await
        .map_err(|e| ApiError(StatusCode::UNAUTHORIZED, format!("{e:#}")))?;
    let tts = voice_of(&state, &device).await?;
    let wav = q.format.as_deref() == Some("wav");
    let text = recognize(&state, &body, q.rate.unwrap_or(audio::STT_RATE)).await?;
    if text.is_empty() {
        let sentences = futures_util::stream::iter([NOT_UNDERSTOOD.to_string()]);
        return Ok(audio_response(tts, device.speed, sentences, wav, Some("")));
    }
    let speed = device.speed;
    let sentences = Arc::clone(&state.assistant).reply_stream(device, text.clone());
    Ok(audio_response(tts, speed, sentences, wav, Some(&text)))
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

async fn device_or_404(state: &AppState, id: Uuid) -> ApiResult<Device> {
    state
        .assistant
        .store
        .device(id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Geraet nicht gefunden".into()))
}

/// Text chat with a device's personality and memory (admin area).
async fn device_chat(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<TextBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let device = device_or_404(&state, id).await?;
    let started = std::time::Instant::now();
    let reply = Arc::clone(&state.assistant)
        .reply(device, body.text.trim().to_string())
        .await;
    Ok(Json(
        json!({ "reply": reply, "ms": started.elapsed().as_millis() as u64 }),
    ))
}

/// Text in the device's voice as WAV (admin area).
async fn device_speak(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<TextBody>,
) -> ApiResult<Response> {
    let device = device_or_404(&state, id).await?;
    let tts = voice_of(&state, &device).await?;
    let mut splitter = SentenceSplitter::default();
    let mut sentences = splitter.push(&body.text);
    sentences.extend(splitter.finish());
    Ok(audio_response(
        tts,
        device.speed,
        futures_util::stream::iter(sentences),
        true,
        None,
    ))
}

/// One short request to a provider, to check key and model (admin area).
async fn provider_test(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let provider = state
        .assistant
        .store
        .provider(id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Anbieter nicht gefunden".into()))?;
    let messages = [ChatMessage::new(
        "user",
        "Antworte nur mit dem Wort: funktioniert",
    )];
    let started = std::time::Instant::now();
    let result = state
        .assistant
        .llm
        .complete(&[provider], &messages, false)
        .await;
    let ms = started.elapsed().as_millis() as u64;
    Ok(Json(match result {
        Ok(reply) => json!({ "ok": true, "reply": reply.trim(), "ms": ms }),
        Err(e) => json!({ "ok": false, "error": format!("{e:#}"), "ms": ms }),
    }))
}
