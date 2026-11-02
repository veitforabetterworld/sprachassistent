//! One agent run per user message. The HTTP request drives it; dropping the request cancels a run
//! that is still `running`. A run waiting for approval stays until the next message.

use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::secrets::redact;
use crate::store::Store;

pub const RUNNING: &str = "running";
pub const WAITING: &str = "waiting_for_approval";
pub const COMPLETED: &str = "completed";
pub const FAILED: &str = "failed";
pub const CANCELLED: &str = "cancelled";

pub struct RunGuard {
    store: Store,
    id: Uuid,
    finished: bool,
}

impl RunGuard {
    pub async fn start(
        store: &Store,
        user_id: i64,
        device_id: Uuid,
        channel: &str,
        user_text: &str,
    ) -> Result<Self> {
        let id = store
            .insert_run(user_id, device_id, channel, user_text)
            .await?;
        Ok(Self {
            store: store.clone(),
            id,
            finished: false,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub async fn step(
        &self,
        seq: i32,
        kind: &str,
        name: &str,
        payload: Value,
        duration_ms: i32,
        error: &str,
    ) {
        let payload = redact_value(payload);
        if let Err(e) = self
            .store
            .add_step(self.id, seq, kind, name, payload, duration_ms, error)
            .await
        {
            tracing::warn!("Laufschritt nicht gespeichert: {e:#}");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish(
        &mut self,
        status: &str,
        reply: &str,
        error: &str,
        iterations: i32,
        input_tokens: i32,
        output_tokens: i32,
        pending: Option<Value>,
    ) {
        self.finished = true;
        if let Err(e) = self
            .store
            .finish_run(
                self.id,
                status,
                reply,
                error,
                iterations,
                input_tokens,
                output_tokens,
                pending,
            )
            .await
        {
            tracing::error!("Lauf nicht geschlossen: {e:#}");
        }
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let store = self.store.clone();
        let id = self.id;
        tokio::spawn(async move {
            if let Err(e) = store.cancel_run_if_running(id).await {
                tracing::warn!("Lauf nicht abgebrochen: {e:#}");
            }
        });
    }
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let v = if k.eq_ignore_ascii_case("authorization")
                        || k.eq_ignore_ascii_case("api_key")
                        || k.eq_ignore_ascii_case("token")
                        || k.eq_ignore_ascii_case("access_token")
                        || k.eq_ignore_ascii_case("refresh_token")
                    {
                        Value::String("***".into())
                    } else {
                        redact_value(v)
                    };
                    (k, v)
                })
                .collect(),
        ),
        other => other,
    }
}
