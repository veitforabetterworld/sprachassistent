//! API tests with fake speech and language models (no models or network needed).

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tokio::sync::Semaphore;
use tower::ServiceExt;

use sprachassistent::api::{router, AppState};
use sprachassistent::assistant::{Assistant, PersonaStore};
use sprachassistent::llm::{ChatMessage, ChatModel, TokenStream};
use sprachassistent::memory::MemoryStore;
use sprachassistent::speech::{SpeechToText, TextToSpeech};

const KEY: &str = "test-key";

#[derive(Default)]
struct FakeLlm {
    seen: Mutex<Vec<Vec<ChatMessage>>>,
}

#[async_trait]
impl ChatModel for FakeLlm {
    async fn stream(&self, messages: &[ChatMessage]) -> Result<TokenStream> {
        self.seen.lock().unwrap().push(messages.to_vec());
        let tokens = ["Hallo Veit, schön dich ", "zu hören. Wie geht's?"];
        Ok(Box::pin(futures_util::stream::iter(
            tokens.map(|t| Ok(t.to_string())),
        )))
    }

    async fn complete(&self, _messages: &[ChatMessage], _json: bool) -> Result<String> {
        Ok(r#"{"add": ["Heißt Veit"]}"#.into())
    }

    fn names(&self) -> Vec<String> {
        vec!["fake".into()]
    }
}

struct FakeStt;

impl SpeechToText for FakeStt {
    fn transcribe(&self, audio: &[f32]) -> Result<String> {
        Ok(format!("Ich bin Veit, {} Samples", audio.len()))
    }
}

struct FakeTts;

impl TextToSpeech for FakeTts {
    fn sample_rate(&self) -> u32 {
        22_050
    }

    /// One sample per character, so the response length tells what was spoken.
    fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1; text.chars().count()])
    }
}

struct Setup {
    app: Router,
    llm: Arc<FakeLlm>,
    memory: Arc<MemoryStore>,
    _dir: tempfile::TempDir,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(FakeLlm::default());
    let memory = Arc::new(MemoryStore::in_memory().unwrap());
    let assistant = Arc::new(Assistant {
        llm: llm.clone(),
        memory: memory.clone(),
        persona: PersonaStore::new(dir.path().join("persona.md")),
        history_messages: 12,
        memory_extraction: false,
    });
    let app = router(AppState {
        assistant,
        stt: Some(Arc::new(FakeStt)),
        tts: Some(Arc::new(FakeTts)),
        api_key: KEY.into(),
        stt_slot: Arc::new(Semaphore::new(1)),
    });
    Setup {
        app,
        llm,
        memory,
        _dir: dir,
    }
}

fn req(method: &str, uri: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {KEY}"))
        .header("content-type", "application/json")
        .body(body.into())
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

/// One second of a 440 Hz tone as raw s16le at 16 kHz.
fn tone() -> Vec<u8> {
    (0..16_000)
        .flat_map(|i| {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8000.0;
            (s as i16).to_le_bytes()
        })
        .collect()
}

#[tokio::test]
async fn health_needs_no_key_but_the_rest_does() {
    let s = setup();
    let health = Request::get("/v1/health").body(Body::empty()).unwrap();
    assert_eq!(send(&s.app, health).await.0, StatusCode::OK);
    let no_key = Request::get("/v1/memory").body(Body::empty()).unwrap();
    assert_eq!(send(&s.app, no_key).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn talk_streams_speech_and_remembers_the_conversation() {
    let s = setup();
    let (status, headers, body) = send(&s.app, req("POST", "/v1/talk", tone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-sample-rate"], "22050");
    assert!(headers["x-transcript"]
        .to_str()
        .unwrap()
        .starts_with("Ich%20bin%20Veit"));
    let spoken = "Hallo Veit, schön dich zu hören.".chars().count() + "Wie geht's?".chars().count();
    assert_eq!(body.len(), spoken * 2);

    let stored = s.memory.recent_messages(10).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[1].content,
        "Hallo Veit, schön dich zu hören. Wie geht's?"
    );
}

#[tokio::test]
async fn silence_is_answered_without_asking_the_model() {
    let s = setup();
    let (status, headers, body) = send(
        &s.app,
        req("POST", "/v1/talk?format=wav", vec![0u8; 32_000]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "audio/wav");
    assert_eq!(&body[..4], b"RIFF");
    assert!(s.llm.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prompt_contains_persona_facts_and_history() {
    let s = setup();
    assert_eq!(
        send(
            &s.app,
            req("PUT", "/v1/persona", "Du bist Frieda, eine Piratin.")
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, _, _) = send(
        &s.app,
        req("POST", "/v1/memory/facts", r#"{"content":"Mag Kaffee"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    s.memory.add_message("user", "Vorher gesagt").unwrap();

    let (status, _, body) = send(&s.app, req("POST", "/v1/chat", r#"{"text":"Hallo"}"#)).await;
    assert_eq!(status, StatusCode::OK);
    let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        reply["reply"],
        "Hallo Veit, schön dich zu hören. Wie geht's?"
    );

    let seen = s.llm.seen.lock().unwrap();
    let system = &seen[0][0].content;
    assert!(system.starts_with("Du bist Frieda, eine Piratin."));
    assert!(system.contains("- Mag Kaffee"));
    assert_eq!(seen[0][1].content, "Vorher gesagt");
    assert_eq!(seen[0].last().unwrap().content, "Hallo");
}

#[tokio::test]
async fn facts_can_be_edited_and_deleted() {
    let s = setup();
    let id = s.memory.add_fact("Mag Tee").unwrap();
    let uri = format!("/v1/memory/facts/{id}");
    assert_eq!(
        send(&s.app, req("PUT", &uri, r#"{"content":"Mag Kaffee"}"#))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(s.memory.facts().unwrap()[0].content, "Mag Kaffee");
    assert_eq!(
        send(&s.app, req("DELETE", &uri, Body::empty())).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&s.app, req("DELETE", &uri, Body::empty())).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn extracted_facts_are_stored() {
    let s = setup();
    let assistant = Assistant {
        llm: s.llm.clone(),
        memory: s.memory.clone(),
        persona: PersonaStore::new(s._dir.path().join("p.md")),
        history_messages: 12,
        memory_extraction: true,
    };
    assistant
        .extract_facts("Ich heiße Veit", "Freut mich")
        .await
        .unwrap();
    assert_eq!(s.memory.facts().unwrap()[0].content, "Heißt Veit");
}
