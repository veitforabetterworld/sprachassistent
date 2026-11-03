//! HTTP API of the voice server.
//!
//! Public (nginx forwards them): `GET /v1/health` and `POST /v1/talk` for the ESPs, authenticated with an ESP key
//! from the admin area (`Authorization: Bearer spa_...` or `X-API-Key`).
//!
//! `/v1/talk` answers in frames, so the device can show what the assistant is doing before it
//! speaks: one byte kind (1 audio as s16le mono at `X-Sample-Rate`, 2 status as UTF-8), four
//! bytes length (little endian), then the payload. Statuses: `thinking`, `tool:<name>`, `speaking`.
//! They name the phase of the reply, never what a tool does: each chatbot has its own tools.
//!
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
use uuid::Uuid;

use crate::agent::{Channel, Status, Voice};
use crate::assistant::Assistant;
use crate::audio;
use crate::failure::{self, Failure};
use crate::llm::{ChatMessage, Provider, ToolDef};
use crate::mcp;
use crate::phrases;
use crate::sentences::SentenceSplitter;
use crate::speech::{voice_label, SpeechToText, TextToSpeech, VoiceBank};
use crate::store::Device;

/// 30 s of 16 kHz 16-bit audio is under 1 MB; WAV recordings from the browser may be stereo/48 kHz.
const MAX_UPLOAD: usize = 12 * 1024 * 1024;
/// Recordings quieter than this are treated as silence.
const SILENCE_RMS: f32 = 0.002;

#[derive(Clone)]
pub struct AppState {
    pub assistant: Arc<Assistant>,
    pub stt: Option<Arc<dyn SpeechToText>>,
    pub voices: Arc<dyn VoiceBank>,
}

pub fn router(state: AppState) -> Router {
    let internal = Router::new()
        .route("/internal/voices", get(voices))
        .route("/internal/devices/{id}/chat", post(device_chat))
        .route("/internal/devices/{id}/speak", post(device_speak))
        .route("/internal/transcribe", post(transcribe))
        .route("/internal/providers/test", post(provider_test))
        .route("/internal/providers/tool-test", post(provider_tool_test))
        .route("/internal/mcp/{id}/discover", post(mcp_discover))
        .route_layer(middleware::from_fn(local_only));
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/talk", post(talk))
        .merge(internal)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .with_state(state)
}

/// `{ "code": ..., "detail": ... }`. The code is a key the admin area words (see `failure`); the
/// detail is technical and only sent for errors that carry a code.
struct ApiError {
    status: StatusCode,
    code: &'static str,
    detail: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str) -> Self {
        Self {
            status,
            code,
            detail: String::new(),
        }
    }

    /// A coded error keeps its code and detail; anything else is only "internal" outside the log.
    fn of(status: StatusCode, e: anyhow::Error) -> Self {
        tracing::warn!("{e:#}");
        match e.chain().find_map(|cause| cause.downcast_ref::<Failure>()) {
            Some(f) => Self {
                status,
                code: f.code,
                detail: f.detail.clone(),
            },
            None => Self::new(status, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = if self.detail.is_empty() {
            json!({ "code": self.code })
        } else {
            json!({ "code": self.code, "detail": self.detail })
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("{e:#}");
        Self::of(StatusCode::INTERNAL_SERVER_ERROR, e)
    }
}

/// The failed result of a test from the admin area: the code and what the other side said.
fn test_failure(e: &anyhow::Error) -> serde_json::Value {
    json!({
        "code": failure::code_of(e).unwrap_or("internal"),
        "detail": format!("{e:#}"),
    })
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
    Json(json!({
        "status": "ok",
        "db": state.assistant.store.ping().await,
        "stt": state.stt.is_some(),
        "voices": state.voices.list(),
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
        .map_err(|e| {
            tracing::error!("Stimme {}: {e:#}", device.voice);
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "speech.voice_unavailable")
        })
}

fn recognizer(state: &AppState) -> ApiResult<Arc<dyn SpeechToText>> {
    state
        .stt
        .clone()
        .ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "speech.stt_unavailable"))
}

/// Silence and a press that is too short are answered without asking the model.
fn is_silence(seconds: f32, rms: f32) -> bool {
    seconds < 0.3 || rms < SILENCE_RMS
}

/// The device's recording (raw s16le mono, 16 kHz) goes to the recognizer while it is still
/// arriving; after the last piece only the end is left to decode.
async fn recognize_live(state: &AppState, body: Body) -> ApiResult<String> {
    let stt = recognizer(state)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
    let worker = tokio::task::spawn_blocking(move || {
        let mut transcription = stt.start();
        let (mut samples, mut energy) = (0usize, 0f64);
        while let Some(piece) = rx.blocking_recv() {
            samples += piece.len();
            energy += piece.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
            transcription.push(&piece);
        }
        let ended = std::time::Instant::now();
        let text = transcription.finish();
        (text, samples, energy, ended.elapsed())
    });
    let mut pieces = audio::PcmPieces::default();
    let mut upload = body.into_data_stream();
    let mut size = 0;
    while let Some(chunk) = upload.next().await {
        let chunk = chunk.map_err(|e| {
            tracing::warn!("Upload: {e}");
            ApiError::new(StatusCode::BAD_REQUEST, "speech.upload_broken")
        })?;
        size += chunk.len();
        if size > MAX_UPLOAD {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "speech.recording_too_long",
            ));
        }
        let _ = tx.send(pieces.push(&chunk));
    }
    drop(tx);
    let (text, samples, energy, tail) = worker.await.map_err(|e| anyhow::anyhow!(e))?;
    let seconds = samples as f32 / audio::STT_RATE as f32;
    let rms = (energy / samples.max(1) as f64).sqrt() as f32;
    if is_silence(seconds, rms) {
        tracing::info!("Stille oder zu kurz ({seconds:.1} s)");
        return Ok(String::new());
    }
    tracing::info!(
        "Erkannt {} ms nach dem Ende ({seconds:.1} s Audio): {text}",
        tail.as_millis()
    );
    Ok(text)
}

/// How the spoken answer is sent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioFormat {
    /// For the browser in the admin area.
    Wav,
    /// Audio and status frames for the devices, see the module docs.
    Frames,
}

const FRAME_AUDIO: u8 = 1;
const FRAME_STATUS: u8 = 2;

fn frame(kind: u8, payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    Bytes::from(out)
}

/// Speaks each sentence as soon as it is synthesized. Statuses only go out as frames.
fn audio_response(
    tts: Arc<dyn TextToSpeech>,
    speed: f32,
    voice: impl futures_util::Stream<Item = Voice> + Send + 'static,
    format: AudioFormat,
    transcript: Option<&str>,
) -> Response {
    let rate = tts.sample_rate();
    let body = async_stream::stream! {
        if format == AudioFormat::Wav {
            yield Ok::<Bytes, std::io::Error>(Bytes::from(audio::streaming_wav_header(rate)));
        }
        let mut voice = Box::pin(voice);
        let mut spoken = false;
        while let Some(item) = voice.next().await {
            let sentence = match item {
                Voice::Say(sentence) => sentence,
                Voice::Status(status) => {
                    if format == AudioFormat::Frames {
                        yield Ok(frame(FRAME_STATUS, status.label().as_bytes()));
                    }
                    continue;
                }
            };
            if format == AudioFormat::Frames && !spoken {
                yield Ok(frame(FRAME_STATUS, Status::Speaking.label().as_bytes()));
            }
            let tts = Arc::clone(&tts);
            match tokio::task::spawn_blocking(move || tts.synthesize(&sentence, speed)).await {
                Ok(Ok(samples)) => {
                    spoken = true;
                    let pcm = audio::f32_to_s16le(&samples);
                    yield Ok(match format {
                        AudioFormat::Frames => frame(FRAME_AUDIO, &pcm),
                        AudioFormat::Wav => Bytes::from(pcm),
                    });
                }
                Ok(Err(e)) => tracing::error!("Sprachausgabe: {e:#}"),
                Err(e) => tracing::error!("Sprachausgabe: {e}"),
            }
        }
    };
    let mut headers = HeaderMap::new();
    let content_type = match format {
        AudioFormat::Wav => "audio/wav",
        AudioFormat::Frames => "application/octet-stream",
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
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
    body: Body,
) -> ApiResult<Response> {
    let device = state
        .assistant
        .store
        .authenticate_device(device_key(&headers), q.device)
        .await
        .map_err(|e| ApiError::of(StatusCode::UNAUTHORIZED, e))?;
    let tts = voice_of(&state, &device).await?;
    let format = AudioFormat::Frames;
    let words = phrases::of(&device.voice);
    // Said out loud, so the speaker does not only blink.
    if state.stt.is_none() {
        let sentences = futures_util::stream::iter([Voice::Say(words.stt_unavailable.to_string())]);
        return Ok(audio_response(
            tts,
            device.speed,
            sentences,
            format,
            Some(""),
        ));
    }
    let text = recognize_live(&state, body).await?;
    if text.is_empty() {
        let sentences = futures_util::stream::iter([Voice::Say(words.not_understood.to_string())]);
        return Ok(audio_response(
            tts,
            device.speed,
            sentences,
            format,
            Some(""),
        ));
    }
    let speed = device.speed;
    let sentences = Arc::clone(&state.assistant).reply_stream(device, text.clone(), Channel::Voice);
    Ok(audio_response(tts, speed, sentences, format, Some(&text)))
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
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "device.not_found"))
}

/// Text chat with a device's personality and memory (admin area).
async fn device_chat(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<TextBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let device = device_or_404(&state, id).await?;
    let started = std::time::Instant::now();
    let turn = Arc::clone(&state.assistant)
        .reply(device, body.text.trim().to_string(), Channel::Text)
        .await;
    Ok(Json(json!({
        "reply": turn.reply,
        "ms": started.elapsed().as_millis() as u64,
        "run_id": turn.run_id,
        "status": turn.status,
    })))
}

/// Browser recording as WAV in, recognized text out. Same recognizer the ESP uses on `/v1/talk`.
async fn transcribe(
    State(state): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let stt = recognizer(&state)?;
    let samples = audio::decode_upload(&body, audio::STT_RATE).map_err(|e| {
        tracing::warn!("Aufnahme: {e:#}");
        ApiError::new(StatusCode::BAD_REQUEST, "speech.bad_audio")
    })?;
    let seconds = samples.len() as f32 / audio::STT_RATE as f32;
    if is_silence(seconds, audio::rms(&samples)) {
        return Ok(Json(json!({ "text": "" })));
    }
    let started = std::time::Instant::now();
    let text = tokio::task::spawn_blocking(move || stt.transcribe(&samples))
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    tracing::info!(
        "Erkannt in {} ms ({seconds:.1} s Audio): {text}",
        started.elapsed().as_millis()
    );
    Ok(Json(json!({ "text": text })))
}

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
        futures_util::stream::iter(sentences.into_iter().map(Voice::Say)),
        AudioFormat::Wav,
        None,
    ))
}

/// What the edit dialog shows, saved or not. An empty `api_key` keeps the key stored under `id`.
/// apps/api has checked that `id` belongs to the user.
#[derive(Deserialize)]
struct ProviderDraft {
    id: Option<Uuid>,
    name: String,
    base_url: String,
    model: String,
    #[serde(default)]
    api_key: String,
}

async fn draft_provider(state: &AppState, draft: ProviderDraft) -> ApiResult<Provider> {
    let mut api_key = draft.api_key.trim().to_string();
    if let (true, Some(id)) = (api_key.is_empty(), draft.id) {
        api_key = state
            .assistant
            .store
            .provider(id, &state.assistant.secrets)
            .await?
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "provider.not_found"))?
            .api_key;
    }
    Ok(Provider::new(
        draft.name.trim(),
        draft.base_url.trim(),
        api_key,
        draft.model.trim().to_string(),
    ))
}

/// One short request to a provider, to check key and model before or after saving (admin area).
async fn provider_test(
    State(state): State<AppState>,
    Json(draft): Json<ProviderDraft>,
) -> ApiResult<Json<serde_json::Value>> {
    let provider = draft_provider(&state, draft).await?;
    let messages = [ChatMessage::new(
        "user",
        "Antworte nur mit dem Wort: funktioniert",
    )];
    let started = std::time::Instant::now();
    let result = state
        .assistant
        .llm
        .complete(&[provider], &messages, &[], false, false)
        .await;
    let ms = started.elapsed().as_millis() as u64;
    Ok(Json(match result {
        Ok(turn) => json!({ "ok": true, "reply": turn.content.trim(), "ms": ms }),
        Err(e) => json!({ "ok": false, "error": test_failure(&e), "ms": ms }),
    }))
}

/// Asks the provider to call one dummy tool. `called` is false when it answers in text instead.
async fn provider_tool_test(
    State(state): State<AppState>,
    Json(draft): Json<ProviderDraft>,
) -> ApiResult<Json<serde_json::Value>> {
    let provider = draft_provider(&state, draft).await?;
    let messages = [ChatMessage::new("user", "Rufe das Werkzeug ping auf.")];
    let tools = [ToolDef {
        name: "ping".into(),
        description: "Antwortet mit pong.".into(),
        parameters: json!({"type": "object", "properties": {}}),
    }];
    let started = std::time::Instant::now();
    let result = state
        .assistant
        .llm
        .complete(&[provider], &messages, &tools, false, true)
        .await;
    let ms = started.elapsed().as_millis() as u64;
    Ok(Json(match result {
        Ok(turn) => json!({
            "ok": true,
            "called": turn.tool_calls.iter().any(|call| call.name == "ping"),
            "reply": turn.content.trim(),
            "ms": ms,
        }),
        Err(e) => json!({ "ok": false, "error": test_failure(&e), "ms": ms }),
    }))
}

async fn mcp_discover(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    match mcp::discover(&state.assistant.store, &state.assistant.secrets, id).await {
        Ok(tools) => Ok(Json(json!({ "ok": true, "tools": tools }))),
        Err(e) => Ok(Json(json!({ "ok": false, "error": test_failure(&e) }))),
    }
}
