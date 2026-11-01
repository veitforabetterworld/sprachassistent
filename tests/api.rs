//! API tests against a real Postgres (TEST_DATABASE_URL) with fake speech and language models.
//! Without TEST_DATABASE_URL they are skipped.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;

use sprachassistent::api::{router, AppState};
use sprachassistent::assistant::Assistant;
use sprachassistent::llm::{ChatMessage, ChatModel, Provider, TokenStream};
use sprachassistent::speech::{SpeechToText, TextToSpeech, VoiceBank};
use sprachassistent::store::{hash_api_key, Store};

const PEPPER: &str = "test-pepper";
const VOICE: &str = "vits-piper-de_DE-test-medium";

#[derive(Default)]
struct FakeLlm {
    seen: Mutex<Vec<(Vec<String>, Vec<ChatMessage>)>>,
}

#[async_trait]
impl ChatModel for FakeLlm {
    async fn stream(
        &self,
        providers: &[Provider],
        messages: &[ChatMessage],
    ) -> Result<TokenStream> {
        let names = providers.iter().map(|p| p.name.clone()).collect();
        self.seen.lock().unwrap().push((names, messages.to_vec()));
        let tokens = ["Hallo Veit, schön dich ", "zu hören. Wie geht's?"];
        Ok(Box::pin(futures_util::stream::iter(
            tokens.map(|t| Ok(t.to_string())),
        )))
    }

    async fn complete(&self, _p: &[Provider], _m: &[ChatMessage], _json: bool) -> Result<String> {
        Ok(r#"{"add": ["Heißt Veit"]}"#.into())
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
    fn synthesize(&self, text: &str, _speed: f32) -> Result<Vec<f32>> {
        Ok(vec![0.1; text.chars().count()])
    }
}

struct FakeVoices;

impl VoiceBank for FakeVoices {
    fn list(&self) -> Vec<String> {
        vec![VOICE.into()]
    }

    fn voice(&self, id: &str) -> Result<Arc<dyn TextToSpeech>> {
        anyhow::ensure!(id == VOICE, "Stimme {id} ist nicht installiert");
        Ok(Arc::new(FakeTts))
    }
}

struct Setup {
    app: Router,
    llm: Arc<FakeLlm>,
    store: Store,
    pool: PgPool,
}

/// The account tables of apps/api, reduced to what the voice server reads.
const ACCOUNT_TABLES: &str = "
CREATE TABLE IF NOT EXISTS users (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  is_admin BOOLEAN NOT NULL DEFAULT false
);
CREATE TABLE IF NOT EXISTS user_api_keys (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  key_hash TEXT NOT NULL UNIQUE,
  permissions TEXT[] NOT NULL,
  all_resources BOOLEAN NOT NULL,
  resources TEXT[] NOT NULL DEFAULT '{}',
  last_used_at TIMESTAMPTZ,
  revoked_at TIMESTAMPTZ
);";

async fn setup() -> Option<Setup> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL fehlt, Test uebersprungen");
        return None;
    };
    let store = Store::connect(&url).await.unwrap();
    store.migrate().await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    // Tests run in parallel; the same lock as Store::migrate keeps the CREATEs apart.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(7351402)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(ACCOUNT_TABLES)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    sqlx::query(
        "INSERT INTO app_settings (key, value) VALUES ('api_key_pepper', $1)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(PEPPER)
    .execute(&pool)
    .await
    .unwrap();
    let llm = Arc::new(FakeLlm::default());
    let app = router(AppState {
        assistant: Arc::new(Assistant {
            llm: llm.clone(),
            store: store.clone(),
        }),
        stt: Some(Arc::new(FakeStt)),
        voices: Arc::new(FakeVoices),
        stt_slot: Arc::new(Semaphore::new(1)),
    });
    Some(Setup {
        app,
        llm,
        store,
        pool,
    })
}

impl Setup {
    async fn device(&self, persona: &str) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO devices (name, voice, persona, memory_extraction) VALUES ($1, $2, $3, false) RETURNING id",
        )
        .bind(format!("Test {}", Uuid::new_v4()))
        .bind(VOICE)
        .bind(persona)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        id
    }

    /// An ESP key for `devices` (empty = all) of a new user.
    async fn key(&self, admin: bool, devices: &[Uuid]) -> String {
        let (user,): (i64,) =
            sqlx::query_as("INSERT INTO users (is_admin) VALUES ($1) RETURNING id")
                .bind(admin)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        let key = format!("spa_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO user_api_keys (user_id, key_hash, permissions, all_resources, resources)
             VALUES ($1, $2, '{device.talk}', $3, $4)",
        )
        .bind(user)
        .bind(hash_api_key(&key, PEPPER))
        .bind(devices.is_empty())
        .bind(devices.iter().map(Uuid::to_string).collect::<Vec<_>>())
        .execute(&self.pool)
        .await
        .unwrap();
        key
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
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
}

fn talk(key: &str, query: &str, body: Vec<u8>) -> Request<Body> {
    Request::post(format!("/v1/talk{query}"))
        .header("authorization", format!("Bearer {key}"))
        .body(Body::from(body))
        .unwrap()
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
async fn talk_needs_an_admin_key_for_the_device() {
    let Some(s) = setup().await else { return };
    let (a, b) = (s.device("").await, s.device("").await);
    let no_key = Request::post("/v1/talk").body(Body::from(tone())).unwrap();
    assert_eq!(s.send(no_key).await.0, StatusCode::UNAUTHORIZED);
    let not_admin = s.key(false, &[a]).await;
    assert_eq!(
        s.send(talk(&not_admin, "", tone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    let only_a = s.key(true, &[a]).await;
    let other = format!("?device={b}");
    assert_eq!(
        s.send(talk(&only_a, &other, tone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    let both = s.key(true, &[a, b]).await;
    assert_eq!(
        s.send(talk(&both, "", tone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(s.send(talk(&both, &other, tone())).await.0, StatusCode::OK);
}

#[tokio::test]
async fn talk_streams_speech_and_remembers_per_device() {
    let Some(s) = setup().await else { return };
    let (a, b) = (s.device("").await, s.device("").await);
    let key = s.key(true, &[a]).await;
    let (status, headers, body) = s.send(talk(&key, "", tone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-sample-rate"], "22050");
    assert!(headers["x-transcript"]
        .to_str()
        .unwrap()
        .starts_with("Ich%20bin%20Veit"));
    let spoken = "Hallo Veit, schön dich zu hören.".chars().count() + "Wie geht's?".chars().count();
    assert_eq!(body.len(), spoken * 2);

    let stored = s.store.recent_messages(a, 10).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[1].content,
        "Hallo Veit, schön dich zu hören. Wie geht's?"
    );
    assert!(s.store.recent_messages(b, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn silence_is_answered_without_asking_the_model() {
    let Some(s) = setup().await else { return };
    let device = s.device("").await;
    let key = s.key(true, &[device]).await;
    let (status, headers, body) = s.send(talk(&key, "?format=wav", vec![0u8; 32_000])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "audio/wav");
    assert_eq!(&body[..4], b"RIFF");
    assert!(s.llm.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prompt_has_the_device_persona_facts_history_and_provider_order() {
    let Some(s) = setup().await else { return };
    let device = s.device("Du bist Frieda, eine Piratin.").await;
    let other = s.device("").await;
    sqlx::query("INSERT INTO device_facts (device_id, content) VALUES ($1, 'Mag Kaffee'), ($2, 'Fremder Fakt')")
        .bind(device)
        .bind(other)
        .execute(&s.pool)
        .await
        .unwrap();
    s.store
        .add_message(device, "user", "Vorher gesagt")
        .await
        .unwrap();
    let (local,): (Uuid,) = sqlx::query_as(
        "INSERT INTO llm_providers (name, base_url, model, priority) VALUES ($1, 'http://127.0.0.1:11434/v1', 'x', 999)
         RETURNING id",
    )
    .bind(format!("Lokal {device}"))
    .fetch_one(&s.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE devices SET provider_id = $2 WHERE id = $1")
        .bind(device)
        .bind(local)
        .execute(&s.pool)
        .await
        .unwrap();

    let req = Request::post(format!("/internal/devices/{device}/chat"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"text":"Hallo"}"#))
        .unwrap();
    let (status, _, body) = s.send(req).await;
    assert_eq!(status, StatusCode::OK);
    let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        reply["reply"],
        "Hallo Veit, schön dich zu hören. Wie geht's?"
    );

    let seen = s.llm.seen.lock().unwrap();
    let (providers, messages) = seen.last().unwrap();
    assert_eq!(providers[0], format!("Lokal {device}"));
    let system = &messages[0].content;
    assert!(system.starts_with("Du bist Frieda, eine Piratin."));
    assert!(system.contains("- Mag Kaffee"));
    assert!(!system.contains("Fremder Fakt"));
    assert_eq!(messages[1].content, "Vorher gesagt");
    assert_eq!(messages.last().unwrap().content, "Hallo");
}

#[tokio::test]
async fn internal_routes_are_not_reachable_through_nginx() {
    let Some(s) = setup().await else { return };
    let req = Request::get("/internal/voices")
        .header("x-forwarded-for", "203.0.113.7")
        .body(Body::empty())
        .unwrap();
    assert_eq!(s.send(req).await.0, StatusCode::NOT_FOUND);
    let (status, _, body) = s
        .send(
            Request::get("/internal/voices")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(body)
        .unwrap()
        .contains("test (de_DE, medium)"));
}

#[tokio::test]
async fn extracted_facts_are_stored_for_the_device() {
    let Some(s) = setup().await else { return };
    let id = s.device("").await;
    let device = s.store.device(id).await.unwrap().unwrap();
    let assistant = Assistant {
        llm: s.llm.clone(),
        store: s.store.clone(),
    };
    assistant
        .extract_facts(&device, "Ich heiße Veit", "Freut mich")
        .await
        .unwrap();
    assert_eq!(s.store.facts(id).await.unwrap()[0].content, "Heißt Veit");
}
