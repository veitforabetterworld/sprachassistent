//! API tests against a real Postgres (TEST_DATABASE_URL) with fake speech and language models.
//! Without TEST_DATABASE_URL they are skipped.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use serde_json::{json, Value};
use sprachassistent::api::{router, AppState};
use sprachassistent::assistant::Assistant;
use sprachassistent::authz::Effect;
use sprachassistent::llm::{ChatMessage, ChatModel, ModelTurn, Provider, TokenStream, ToolCall};
use sprachassistent::secrets::Secrets;
use sprachassistent::speech::{SpeechToText, TextToSpeech, Transcription, VoiceBank};
use sprachassistent::store::{hash_api_key, migration_names, Store};
use sprachassistent::tools::{EmptyTools, ToolExecutor, ToolHints, ToolOffer};

const PEPPER: &str = "test-pepper";
/// CREDENTIALS_KEY of the tests; apps/api stores the pepper encrypted with it.
const CREDENTIALS: &str = "test-credentials";
const VOICE: &str = "vits-piper-de_DE-test-medium";

#[derive(Default)]
struct FakeLlm {
    seen: Mutex<Vec<(Vec<String>, Vec<ChatMessage>)>>,
    /// Per `complete`: how many tools were offered and whether one was forced.
    offered: Mutex<Vec<(usize, bool)>>,
    script: Mutex<VecDeque<ModelTurn>>,
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
        let tokens = [
            "Hallo Veit, schön dich ",
            "zu hören. Wie geht's? [kon",
            "text]Nutzer heißt Veit.[/kontext]",
        ];
        Ok(Box::pin(futures_util::stream::iter(
            tokens.map(|t| Ok(t.to_string())),
        )))
    }

    async fn complete(
        &self,
        providers: &[Provider],
        messages: &[ChatMessage],
        tools: &[sprachassistent::llm::ToolDef],
        _json: bool,
        force_tool: bool,
    ) -> Result<ModelTurn> {
        let names = providers.iter().map(|p| p.name.clone()).collect();
        self.seen.lock().unwrap().push((names, messages.to_vec()));
        self.offered.lock().unwrap().push((tools.len(), force_tool));
        let next = self.script.lock().unwrap().pop_front();
        if let Some(mut turn) = next {
            if turn.provider.is_empty() {
                turn.provider = providers
                    .first()
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
            }
            return Ok(turn);
        }
        Ok(ModelTurn {
            content: r#"{"add": ["Heißt Veit"]}"#.into(),
            provider: providers
                .first()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            ..ModelTurn::default()
        })
    }
}

impl FakeLlm {
    fn push(&self, turn: ModelTurn) {
        self.script.lock().unwrap().push_back(turn);
    }
}

struct ScriptTools {
    name: &'static str,
    effect: Effect,
    hints: ToolHints,
    guide: &'static str,
    calls: Mutex<u32>,
}

#[async_trait]
impl ToolExecutor for ScriptTools {
    async fn list(&self, _user_id: i64) -> Result<Vec<ToolOffer>> {
        Ok(vec![ToolOffer {
            name: self.name.into(),
            description: "Wiederholt einen Text.".into(),
            parameters: json!({"type":"object","properties":{"text":{"type":"string"}}}),
            effect: self.effect,
            hints: self.hints,
            guide: self.guide.into(),
        }])
    }

    async fn execute(&self, _user_id: i64, _name: &str, _args: Value) -> Result<String> {
        *self.calls.lock().unwrap() += 1;
        Ok("pong".into())
    }
}

struct FakeStt;

impl SpeechToText for FakeStt {
    fn start(&self) -> Box<dyn Transcription> {
        Box::new(FakeTranscription(0))
    }
}

/// Counts what arrives, piece by piece.
struct FakeTranscription(usize);

impl Transcription for FakeTranscription {
    fn push(&mut self, audio: &[f32]) {
        self.0 += audio.len();
    }

    fn finish(self: Box<Self>) -> String {
        format!("Ich bin Veit, {} Samples", self.0)
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

/// Where the tests of this run live: a fresh database next to TEST_DATABASE_URL, migrated once like
/// apps/api does at deploy. Each test then only adds rows of its own (a user, a chatbot), so the tests
/// run side by side without touching the schema. None without TEST_DATABASE_URL.
static DATABASE: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();
const TEST_DB: &str = "sa_api_tests";

async fn test_database() -> Option<String> {
    DATABASE
        .get_or_init(|| async {
            let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
                eprintln!("TEST_DATABASE_URL fehlt, Test uebersprungen");
                return None;
            };
            let server = PgPool::connect(&url).await.unwrap();
            // Two statements: together they would be one transaction, which CREATE DATABASE refuses.
            for sql in [
                format!("DROP DATABASE IF EXISTS {TEST_DB} WITH (FORCE)"),
                format!("CREATE DATABASE {TEST_DB}"),
            ] {
                sqlx::raw_sql(&sql).execute(&server).await.unwrap();
            }
            server.close().await;
            // postgres://user@host/db?options -> the same server, the test database.
            let (host, _) = url.split('?').next().unwrap().rsplit_once('/').unwrap();
            let fresh = format!("{host}/{TEST_DB}");
            let pool = PgPool::connect(&fresh).await.unwrap();
            sqlx::raw_sql(ACCOUNT_TABLES).execute(&pool).await.unwrap();
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("db/migrations");
            for name in migration_names(&dir).unwrap() {
                let sql = std::fs::read_to_string(dir.join(&name)).unwrap();
                sqlx::raw_sql(&sql)
                    .execute(&pool)
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            sqlx::query("INSERT INTO app_settings (key, value) VALUES ('api_key_pepper', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
                .bind(Secrets::new(CREDENTIALS).encrypt(PEPPER).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
            Some(fresh)
        })
        .await
        .clone()
}

async fn setup() -> Option<Setup> {
    let url = test_database().await?;
    let store = Store::connect(&url).await.unwrap();
    let pool = store.pool().clone();
    let llm = Arc::new(FakeLlm::default());
    let app = router(AppState {
        assistant: Arc::new(Assistant {
            llm: llm.clone(),
            store: store.clone(),
            tools: Arc::new(EmptyTools),
            secrets: Secrets::new(CREDENTIALS),
        }),
        stt: Some(Arc::new(FakeStt)),
        voices: Arc::new(FakeVoices),
    });
    Some(Setup {
        app,
        llm,
        store,
        pool,
    })
}

impl Setup {
    async fn user(&self, admin: bool) -> i64 {
        let (id,): (i64,) = sqlx::query_as("INSERT INTO users (is_admin) VALUES ($1) RETURNING id")
            .bind(admin)
            .fetch_one(&self.pool)
            .await
            .unwrap();
        id
    }

    async fn device(&self, user: i64, persona: &str) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO devices (name, voice, persona, user_id)
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(format!("Test {}", Uuid::new_v4()))
        .bind(VOICE)
        .bind(persona)
        .bind(user)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        id
    }

    /// An ESP key for `devices` (empty = all of this user).
    async fn key(&self, user: i64, devices: &[Uuid]) -> String {
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

/// Statuses and the number of audio bytes in a `/v1/talk` answer.
fn frames(body: &[u8]) -> (Vec<String>, usize) {
    let (mut statuses, mut audio, mut rest) = (Vec::new(), 0, body);
    while !rest.is_empty() {
        let len = u32::from_le_bytes(rest[1..5].try_into().unwrap()) as usize;
        let payload = &rest[5..5 + len];
        match rest[0] {
            1 => audio += len,
            2 => statuses.push(String::from_utf8(payload.to_vec()).unwrap()),
            other => panic!("unbekannter Rahmen {other}"),
        }
        rest = &rest[5 + len..];
    }
    (statuses, audio)
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
async fn talk_needs_the_owners_key() {
    let Some(s) = setup().await else { return };
    let owner = s.user(false).await;
    let stranger = s.user(false).await;
    let (a, b) = (s.device(owner, "").await, s.device(owner, "").await);
    let no_key = Request::post("/v1/talk").body(Body::from(tone())).unwrap();
    assert_eq!(s.send(no_key).await.0, StatusCode::UNAUTHORIZED);
    let foreign = s.key(stranger, &[a]).await;
    assert_eq!(
        s.send(talk(&foreign, "", tone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    let only_a = s.key(owner, &[a]).await;
    let other = format!("?device={b}");
    assert_eq!(
        s.send(talk(&only_a, &other, tone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(s.send(talk(&only_a, "", tone())).await.0, StatusCode::OK);
    let both = s.key(owner, &[a, b]).await;
    assert_eq!(
        s.send(talk(&both, "", tone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(s.send(talk(&both, &other, tone())).await.0, StatusCode::OK);
}

#[tokio::test]
async fn talk_streams_speech_and_remembers_per_device() {
    let Some(s) = setup().await else { return };
    let owner = s.user(false).await;
    let (a, b) = (s.device(owner, "").await, s.device(owner, "").await);
    let key = s.key(owner, &[a]).await;
    let (status, headers, body) = s.send(talk(&key, "", tone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-sample-rate"], "22050");
    assert!(headers["x-transcript"]
        .to_str()
        .unwrap()
        .starts_with("Ich%20bin%20Veit"));
    let spoken = "Hallo Veit, schön dich zu hören.".chars().count() + "Wie geht's?".chars().count();
    let (statuses, audio) = frames(&body);
    assert_eq!(audio, spoken * 2);
    assert_eq!(statuses, ["thinking", "speaking"]);

    // The context block is neither spoken nor stored with the reply; it becomes the context.
    let stored = s.store.last_exchange(a).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[1].content,
        "Hallo Veit, schön dich zu hören. Wie geht's?"
    );
    let device = s.store.device(a).await.unwrap().unwrap();
    assert_eq!(device.context, "Nutzer heißt Veit.");
    assert!(s.store.last_exchange(b).await.unwrap().is_empty());
    let seen = s.llm.seen.lock().unwrap();
    assert!(seen.last().unwrap().1[0].content.contains("Lautsprecher"));
}

#[tokio::test]
async fn silence_is_answered_without_asking_the_model() {
    let Some(s) = setup().await else { return };
    let owner = s.user(false).await;
    let device = s.device(owner, "").await;
    let key = s.key(owner, &[device]).await;
    let (status, headers, body) = s.send(talk(&key, "", vec![0u8; 32_000])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/octet-stream");
    let (statuses, audio) = frames(&body);
    assert_eq!(statuses, ["speaking"]);
    assert_eq!(
        audio,
        "Das habe ich leider nicht verstanden.".chars().count() * 2
    );
    assert!(s.llm.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prompt_has_the_persona_context_last_exchange_and_provider_order() {
    let Some(s) = setup().await else { return };
    let owner = s.user(false).await;
    let device = s.device(owner, "Du bist Frieda, eine Piratin.").await;
    let other = s.device(s.user(false).await, "").await;
    for (id, context) in [(device, "Mag Kaffee"), (other, "Fremder Kontext")] {
        s.store.set_context(id, context).await.unwrap();
    }
    for (role, text) in [
        ("user", "Ganz alt"),
        ("assistant", "Alte Antwort"),
        ("tool", "Werkzeug"),
        ("user", "Vorher gesagt"),
    ] {
        s.store.add_message(device, role, text).await.unwrap();
    }
    let (local,): (Uuid,) = sqlx::query_as(
        "INSERT INTO llm_providers (user_id, name, base_url, model, priority)
         VALUES ($1, $2, 'http://127.0.0.1:11434/v1', 'x', 999)
         RETURNING id",
    )
    .bind(owner)
    .bind(format!("Lokal {device}"))
    .fetch_one(&s.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO llm_providers (user_id, name, base_url, model, priority)
         VALUES ($1, $2, 'http://127.0.0.1:11434/v1', 'x', 1)",
    )
    .bind(s.user(false).await)
    .bind(format!("Fremd {device}"))
    .execute(&s.pool)
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
    assert!(!providers.iter().any(|name| name.starts_with("Fremd ")));
    let system = &messages[0].content;
    assert!(system.starts_with("Du bist Frieda, eine Piratin."));
    assert!(!system.contains("Mag Kaffee"));
    assert!(messages.iter().all(|m| !m.content.contains("Fremder")));
    assert!(messages.iter().all(|m| !m.content.contains("Ganz alt")));
    assert_eq!(messages[1].content, "Alte Antwort");
    assert_eq!(messages[2].content, "Vorher gesagt");
    assert_eq!(
        messages.last().unwrap().content,
        "[kontext]\nMag Kaffee\n[/kontext]\n\nHallo"
    );
    assert!(!system.contains("Lautsprecher"));
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

fn tool_turn(call: bool, text: &str) -> ModelTurn {
    if call {
        ModelTurn {
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
                thought_signature: None,
            }],
            ..ModelTurn::default()
        }
    } else {
        ModelTurn {
            content: text.into(),
            ..ModelTurn::default()
        }
    }
}

async fn tool_setup(effect: Effect) -> Option<(Setup, Arc<ScriptTools>, Uuid, String)> {
    named_tool_setup("echo", effect, ToolHints::default(), "").await
}

/// Reads from outside, like the built-in internet research.
async fn research_setup() -> Option<(Setup, Arc<ScriptTools>, Uuid, String)> {
    let hints = ToolHints {
        read_only: Some(true),
        open_world: Some(true),
        ..ToolHints::default()
    };
    named_tool_setup("web_research", Effect::Auto, hints, "").await
}

async fn named_tool_setup(
    tool: &'static str,
    effect: Effect,
    hints: ToolHints,
    guide: &'static str,
) -> Option<(Setup, Arc<ScriptTools>, Uuid, String)> {
    let s = setup().await?;
    let owner = s.user(false).await;
    let device = s.device(owner, "Du bist Kurt.").await;
    let name = format!("Tools {device}");
    let (provider,): (Uuid,) = sqlx::query_as(
        "INSERT INTO llm_providers (user_id, name, base_url, model, priority)
         VALUES ($1, $2, 'http://127.0.0.1:11434/v1', 'x', 1) RETURNING id",
    )
    .bind(owner)
    .bind(&name)
    .fetch_one(&s.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE devices SET provider_id = $2 WHERE id = $1")
        .bind(device)
        .bind(provider)
        .execute(&s.pool)
        .await
        .unwrap();
    let tools = Arc::new(ScriptTools {
        name: tool,
        effect,
        hints,
        guide,
        calls: Mutex::new(0),
    });
    let llm = Arc::new(FakeLlm::default());
    let app = router(AppState {
        assistant: Arc::new(Assistant {
            llm: llm.clone(),
            store: s.store.clone(),
            tools: tools.clone(),
            secrets: Secrets::new(CREDENTIALS),
        }),
        stt: Some(Arc::new(FakeStt)),
        voices: Arc::new(FakeVoices),
    });
    let key = s.key(owner, &[device]).await;
    Some((
        Setup {
            app,
            llm,
            store: s.store,
            pool: s.pool,
        },
        tools,
        device,
        key,
    ))
}

async fn chat(s: &Setup, device: Uuid, text: &str) -> Value {
    let req = Request::post(format!("/internal/devices/{device}/chat"))
        .header("content-type", "application/json")
        .body(Body::from(format!(r#"{{"text":"{text}"}}"#)))
        .unwrap();
    let (status, _, body) = s.send(req).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).unwrap()
}

fn research_turn() -> ModelTurn {
    ModelTurn {
        tool_calls: vec![ToolCall {
            id: "r1".into(),
            name: "web_research".into(),
            arguments: r#"{"action":"news"}"#.into(),
            thought_signature: None,
        }],
        ..ModelTurn::default()
    }
}

#[tokio::test]
async fn small_talk_is_answered_at_once_without_nudge() {
    let Some((s, tools, device, _)) = research_setup().await else {
        return;
    };
    s.llm.push(tool_turn(false, "Hallo!"));
    let reply = chat(&s, device, "Hallo").await;
    assert_eq!(reply["reply"], "Hallo!");
    assert_eq!(*tools.calls.lock().unwrap(), 0);
    assert_eq!(*s.llm.offered.lock().unwrap(), vec![(1, false)]);
    // A new chatbot starts with the example context in front of its first question.
    let seen = s.llm.seen.lock().unwrap();
    assert!(seen[0]
        .1
        .last()
        .unwrap()
        .content
        .starts_with("[kontext]\nNeuer Chat, noch kein Gespräch."));
}

#[tokio::test]
async fn a_model_with_nudge_gets_its_first_reply_back_once_never_forced() {
    let Some((s, tools, device, _)) = research_setup().await else {
        return;
    };
    s.llm.push(ModelTurn {
        nudge_tools: true,
        ..tool_turn(false, "Licht ist an.")
    });
    s.llm.push(ModelTurn {
        nudge_tools: true,
        ..tool_turn(false, "Hallo, schön dich zu hören.")
    });
    let reply = chat(&s, device, "Hallo").await;
    assert_eq!(reply["reply"], "Hallo, schön dich zu hören.");
    assert_eq!(*tools.calls.lock().unwrap(), 0);
    let offered = s.llm.offered.lock().unwrap();
    assert_eq!(offered.len(), 2);
    assert!(offered.iter().all(|(_, forced)| !forced));
    let second = &s.llm.seen.lock().unwrap()[1].1;
    assert!(second
        .iter()
        .any(|m| m.content.starts_with("Neue Anfrage.")));
}

#[tokio::test]
async fn asking_again_for_the_same_research_ends_in_an_answer() {
    let Some((s, tools, device, _)) = research_setup().await else {
        return;
    };
    s.llm.push(research_turn());
    s.llm.push(research_turn());
    s.llm.push(tool_turn(false, "Die Schlagzeilen sind ..."));
    let reply = chat(&s, device, "Was gibt es Neues?").await;
    assert_eq!(reply["reply"], "Die Schlagzeilen sind ...");
    assert_eq!(*tools.calls.lock().unwrap(), 1);
    // After the repeat the model only gets to answer, without tools.
    let offered = s.llm.offered.lock().unwrap();
    assert_eq!(
        offered.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![1, 1, 0]
    );
}

#[tokio::test]
async fn older_pages_leave_the_prompt_once_the_model_wrote_its_context() {
    let Some((s, tools, device, _)) = research_setup().await else {
        return;
    };
    s.llm.push(research_turn());
    s.llm.push(ModelTurn {
        tool_calls: vec![ToolCall {
            id: "r2".into(),
            name: "web_research".into(),
            arguments: r#"{"action":"search","query":"Wahl","kontext":"Wahl am Sonntag"}"#.into(),
            thought_signature: None,
        }],
        ..ModelTurn::default()
    });
    s.llm.push(tool_turn(false, "Die Wahl ist am Sonntag."));
    let reply = chat(&s, device, "Wann ist die Wahl?").await;
    assert_eq!(reply["reply"], "Die Wahl ist am Sonntag.");
    assert_eq!(*tools.calls.lock().unwrap(), 2);
    // The reply brought no block, so the context from the research step is kept.
    let stored = s.store.device(device).await.unwrap().unwrap();
    assert_eq!(stored.context, "Wahl am Sonntag");
    let seen = s.llm.seen.lock().unwrap();
    let result = |messages: &[ChatMessage], id: &str| {
        messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some(id))
            .map(|m| m.content.clone())
            .unwrap()
    };
    // The second step still sees the first page in full.
    assert!(result(&seen[1].1, "r1").contains("pong"));
    let third = &seen[2].1;
    assert!(!result(third, "r1").contains("pong"));
    assert!(result(third, "r1").contains("Kontext"));
    assert!(result(third, "r2").contains("pong"));
    assert!(third.iter().any(|m| m
        .tool_calls
        .iter()
        .any(|c| c.arguments.contains("Wahl am Sonntag"))));
}

#[tokio::test]
async fn a_reply_with_tools_keeps_its_block_as_context_and_out_of_the_reply() {
    let Some((s, _, device, _)) = research_setup().await else {
        return;
    };
    s.store.set_context(device, "Alt").await.unwrap();
    s.llm.push(tool_turn(
        false,
        "Hallo Veit!\n[kontext]Heißt Veit.[/kontext]",
    ));
    let reply = chat(&s, device, "Ich bin Veit").await;
    assert_eq!(reply["reply"], "Hallo Veit!");
    let last = s.store.last_exchange(device).await.unwrap();
    assert_eq!(last[1].content, "Hallo Veit!");
    assert_eq!(
        s.store.device(device).await.unwrap().unwrap().context,
        "Heißt Veit."
    );
    let seen = s.llm.seen.lock().unwrap();
    assert!(seen[0]
        .1
        .last()
        .unwrap()
        .content
        .starts_with("[kontext]\nAlt\n"));
}

#[tokio::test]
async fn tool_call_speaks_the_busy_line_before_the_answer() {
    let Some((s, tools, device, key)) = tool_setup(Effect::Auto).await else {
        return;
    };
    s.llm.push(tool_turn(true, ""));
    s.llm.push(tool_turn(false, "Erledigt."));
    let (status, _, body) = s.send(talk(&key, "", tone())).await;
    assert_eq!(status, StatusCode::OK);
    let busy = "Einen Moment.".chars().count();
    let answer = "Erledigt.".chars().count();
    assert_eq!(frames(&body).1, (busy + answer) * 2);
    assert_eq!(*tools.calls.lock().unwrap(), 1);
    let seen = s.llm.seen.lock().unwrap();
    let second = &seen[1].1;
    assert!(second
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("pong")));
    assert!(second.iter().any(|m| m.content.contains("tool_result")));
    let _ = device;
}

#[tokio::test]
async fn frames_tell_the_device_what_happens_before_it_speaks() {
    let Some((s, _, _, key)) = tool_setup(Effect::Auto).await else {
        return;
    };
    s.llm.push(tool_turn(true, ""));
    s.llm.push(tool_turn(false, "Erledigt."));
    let (status, _, body) = s.send(talk(&key, "", tone())).await;
    assert_eq!(status, StatusCode::OK);
    let (statuses, audio) = frames(&body);
    // The busy line is spoken right after the first answer, the tool runs, then the reply.
    assert_eq!(
        statuses,
        ["thinking", "speaking", "tool:echo", "thinking"],
        "{statuses:?}"
    );
    let spoken = "Einen Moment.".chars().count() + "Erledigt.".chars().count();
    assert_eq!(audio, spoken * 2);
}

#[tokio::test]
async fn confirmation_runs_only_after_yes() {
    let Some((s, tools, device, _)) = tool_setup(Effect::Confirm).await else {
        return;
    };
    s.llm.push(tool_turn(true, ""));
    let first = chat(&s, device, "Mach das").await;
    assert_eq!(first["status"], "waiting_for_approval");
    assert!(first["reply"].as_str().unwrap().contains("ja oder nein"));
    assert_eq!(*tools.calls.lock().unwrap(), 0);

    s.llm.push(tool_turn(false, "Gemacht."));
    let yes = chat(&s, device, "ja").await;
    assert_eq!(yes["reply"], "Gemacht.");
    assert_eq!(yes["status"], "completed");
    assert_eq!(*tools.calls.lock().unwrap(), 1);
    let framed = s.llm.seen.lock().unwrap().last().unwrap().1.clone();
    assert!(framed
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("[tool_result")));
}

#[tokio::test]
async fn confirmation_no_and_other_do_not_run_the_tool() {
    let Some((s, tools, device, _)) = tool_setup(Effect::Confirm).await else {
        return;
    };
    s.llm.push(tool_turn(true, ""));
    chat(&s, device, "Mach das").await;
    s.llm.push(tool_turn(false, "Alles klar."));
    let no = chat(&s, device, "nein").await;
    assert_eq!(no["reply"], "Alles klar.");
    assert_eq!(*tools.calls.lock().unwrap(), 0);
    assert!(s
        .llm
        .seen
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .1
        .iter()
        .any(|m| m.content.contains("abgelehnt")));

    s.llm.push(tool_turn(true, ""));
    chat(&s, device, "Noch einmal").await;
    s.llm.push(tool_turn(false, "Neue Frage."));
    let other = chat(&s, device, "Was gibt es zu essen").await;
    assert_eq!(other["reply"], "Neue Frage.");
    assert_eq!(*tools.calls.lock().unwrap(), 0);
    let waiting: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_runs WHERE device_id = $1 AND status = 'waiting_for_approval'",
    )
    .bind(device)
    .fetch_one(&s.pool)
    .await
    .unwrap();
    assert_eq!(waiting, 0);
}

#[tokio::test]
async fn denied_tool_is_not_executed() {
    let Some((s, tools, device, _)) = tool_setup(Effect::Deny).await else {
        return;
    };
    s.llm.push(tool_turn(true, ""));
    s.llm.push(tool_turn(false, "Geht nicht."));
    let reply = chat(&s, device, "Lösch das").await;
    assert_eq!(reply["reply"], "Geht nicht.");
    assert_eq!(*tools.calls.lock().unwrap(), 0);
    assert!(s
        .llm
        .seen
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .1
        .iter()
        .any(|m| m.content.contains("gesperrt")));
}

#[tokio::test]
async fn iteration_limit_stops_the_loop() {
    let Some((s, tools, device, _)) = tool_setup(Effect::Auto).await else {
        return;
    };
    sqlx::query("UPDATE devices SET max_iterations = 1 WHERE id = $1")
        .bind(device)
        .execute(&s.pool)
        .await
        .unwrap();
    s.llm.push(tool_turn(true, ""));
    s.llm.push(tool_turn(false, "Sollte nicht drankommen."));
    let reply = chat(&s, device, "Loop").await;
    assert_eq!(reply["reply"], sprachassistent::phrases::of("").too_long);
    assert_eq!(reply["status"], "failed");
    assert_eq!(*tools.calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_run_stays_on_the_provider_that_answered() {
    let Some((s, _, device, _)) = tool_setup(Effect::Auto).await else {
        return;
    };
    let other = format!("Zweiter {device}");
    sqlx::query(
        "INSERT INTO llm_providers (user_id, name, base_url, model, priority)
         SELECT user_id, $1, 'http://127.0.0.1:11434/v1', 'y', 2 FROM devices WHERE id = $2",
    )
    .bind(&other)
    .bind(device)
    .execute(&s.pool)
    .await
    .unwrap();
    s.llm.push(tool_turn(true, ""));
    s.llm.push(tool_turn(false, "Fertig."));
    let reply = chat(&s, device, "Such").await;
    assert_eq!(reply["reply"], "Fertig.");
    let seen = s.llm.seen.lock().unwrap();
    let first = &seen[0].0;
    assert!(first.len() >= 2, "{first:?}");
    // The one that answered goes first; the others stay behind it for a limit or an outage.
    assert_eq!(seen[1].0[0], first[0]);
    assert_eq!(seen[1].0.len(), first.len());
}

#[tokio::test]
async fn any_enabled_tool_is_offered_and_brings_its_guide() {
    // A name that once was filtered, without annotations: offered like any other tool.
    let Some((s, tools, device, _)) = named_tool_setup(
        "server__call_tool",
        Effect::Auto,
        ToolHints::default(),
        "Rufe server__call_tool nur mit einem Namen aus der Liste auf.",
    )
    .await
    else {
        return;
    };
    s.llm.push(ModelTurn {
        tool_calls: vec![ToolCall {
            id: "u1".into(),
            name: "server_call_tool".into(),
            arguments: "{}".into(),
            thought_signature: None,
        }],
        ..ModelTurn::default()
    });
    s.llm.push(tool_turn(false, "Fertig."));
    let reply = chat(&s, device, "Mach was").await;
    assert_eq!(reply["reply"], "Fertig.");
    assert_eq!(*tools.calls.lock().unwrap(), 1);
    // A tool that says nothing about itself stays on offer after it ran.
    assert_eq!(
        s.llm
            .offered
            .lock()
            .unwrap()
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>(),
        vec![1, 1]
    );
    let seen = s.llm.seen.lock().unwrap();
    let system = &seen[0].1[0].content;
    assert!(system.contains("nur mit einem Namen aus der Liste"));
    assert!(!system.contains("Lesewerkzeug"));
}

#[tokio::test]
async fn a_tool_that_changes_something_brings_the_rules_for_changes() {
    let hints = ToolHints {
        read_only: Some(false),
        ..ToolHints::default()
    };
    let Some((s, tools, device, _)) = named_tool_setup("get_lamp", Effect::Auto, hints, "").await
    else {
        return;
    };
    s.llm.push(ModelTurn {
        tool_calls: vec![ToolCall {
            id: "a1".into(),
            name: "get_lamp".into(),
            arguments: "{}".into(),
            thought_signature: None,
        }],
        ..ModelTurn::default()
    });
    s.llm.push(tool_turn(false, "Lampe ist an."));
    let reply = chat(&s, device, "Mach die Lampe an").await;
    assert_eq!(reply["reply"], "Lampe ist an.");
    assert_eq!(*tools.calls.lock().unwrap(), 1);
    // `get_` in the name does not make it a read. With nothing to read the state, the change
    // is offered at once and the model answers after it ran.
    let offered = s.llm.offered.lock().unwrap();
    assert_eq!(
        offered.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![1, 0]
    );
    let seen = s.llm.seen.lock().unwrap();
    assert!(seen[0].1[0].content.contains("Lesewerkzeug"));
}

#[tokio::test]
async fn the_server_sees_whether_a_migration_is_missing() {
    let Some(s) = setup().await else { return };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("db/migrations");
    // The test database got the files but no ledger, like a database apps/api never migrated.
    let pending = s.store.pending_migrations(&dir).await.unwrap();
    assert_eq!(pending, migration_names(&dir).unwrap());
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
    )
    .execute(&s.pool)
    .await
    .unwrap();
    for name in migration_names(&dir).unwrap() {
        sqlx::query("INSERT INTO schema_migrations (name) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(name)
            .execute(&s.pool)
            .await
            .unwrap();
    }
    assert!(s.store.pending_migrations(&dir).await.unwrap().is_empty());
}
