//! Postgres: settings, AI providers, devices with their memory, and the ESP keys of @veit/api-keys.
//! The schema comes from db/schema.sql (shared with apps/api); account tables belong to apps/api.

use std::collections::HashMap;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::llm::Provider;
use crate::mcp::DiscoveredTool;
use crate::secrets::Secrets;

const SCHEMA: &str = include_str!("../db/schema.sql");
/// Same number as SCHEMA_LOCK in apps/api/src/db.ts.
const SCHEMA_LOCK: i64 = 7_351_402;
/// Same prefix as apps/api/src/accounts.ts. Every key is stored with this permission; callers do not choose one.
const KEY_PREFIX: &str = "spa_";
const DEVICE_TALK: &str = "device.talk";
/// Older messages are deleted; what matters long-term is in the device's context.
const KEEP_MESSAGES: i64 = 1000;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Device {
    pub id: Uuid,
    pub user_id: Option<i64>,
    pub name: String,
    pub voice: String,
    pub speed: f32,
    pub persona: String,
    pub provider_id: Option<Uuid>,
    pub max_iterations: i32,
    pub busy_phrase: String,
    pub still_busy_phrase: String,
    pub tools_enabled: bool,
    /// What the model wrote down for later conversations, see `context`.
    pub context: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub metadata: Value,
}

#[derive(FromRow)]
struct ProviderRow {
    name: String,
    base_url: String,
    api_key: String,
    model: String,
    nudge_tools: bool,
}

const DEVICE_COLUMNS: &str =
    "id, user_id, name, voice, speed, persona, provider_id, max_iterations, busy_phrase, \
     still_busy_phrase, tools_enabled, context";

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
        Ok(Self { pool })
    }

    /// Serialized with apps/api by the same advisory lock: concurrent CREATE ... IF NOT EXISTS can collide.
    pub async fn migrate(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn ping(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }

    pub async fn settings(&self) -> Result<HashMap<String, String>> {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM app_settings")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().collect())
    }

    /// The voices the devices speak with, to load them before the first question.
    pub async fn device_voices(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT voice FROM devices")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(voice,)| voice).collect())
    }

    pub async fn device(&self, id: Uuid) -> Result<Option<Device>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// The device's own provider first, then the owner's others by priority. A provider id that
    /// belongs to someone else is ignored. Providers without a key are only used when they run locally.
    pub async fn providers_for(&self, device: &Device, secrets: &Secrets) -> Result<Vec<Provider>> {
        let Some(user_id) = device.user_id else {
            return Ok(Vec::new());
        };
        let rows: Vec<ProviderRow> = sqlx::query_as(
            "SELECT name, base_url, api_key, model, nudge_tools FROM llm_providers
             WHERE user_id = $2
               AND enabled AND (api_key <> '' OR base_url ~ '^https?://(localhost|127\\.0\\.0\\.1)[:/]')
             ORDER BY (id = $1) DESC, priority, name",
        )
        .bind(device.provider_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| self.provider_from(r, secrets))
            .collect())
    }

    pub async fn provider(&self, id: Uuid, secrets: &Secrets) -> Result<Option<Provider>> {
        let row: Option<ProviderRow> = sqlx::query_as(
            "SELECT name, base_url, api_key, model, nudge_tools FROM llm_providers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| self.provider_from(r, secrets)))
    }

    fn provider_from(&self, row: ProviderRow, secrets: &Secrets) -> Option<Provider> {
        let api_key = match secrets.decrypt(&row.api_key) {
            Ok(key) => key,
            Err(e) => {
                tracing::error!("{}: Schlüssel nicht lesbar: {e:#}", row.name);
                return None;
            }
        };
        let mut provider = Provider::new(&row.name, &row.base_url, api_key, row.model);
        provider.nudge_tools = row.nudge_tools;
        Some(provider)
    }

    /// The chatbot an API key may talk as. The key's user must own it. A key names one chatbot, so
    /// `requested` is only needed when an older key still covers more than one.
    pub async fn authenticate_device(
        &self,
        raw_key: &str,
        requested: Option<Uuid>,
    ) -> Result<Device> {
        if !raw_key.starts_with(KEY_PREFIX) {
            bail!("unbekannter Schluessel");
        }
        let pepper: Option<(String,)> =
            sqlx::query_as("SELECT value FROM app_settings WHERE key = 'api_key_pepper'")
                .fetch_optional(&self.pool)
                .await?;
        let hash = hash_api_key(raw_key, pepper.map(|p| p.0).as_deref().unwrap_or(""));
        let key: Option<(i64, bool, Vec<String>)> = sqlx::query_as(
            "UPDATE user_api_keys SET last_used_at = NOW()
             WHERE key_hash = $1 AND revoked_at IS NULL AND $2 = ANY(permissions)
             RETURNING user_id, all_resources, resources",
        )
        .bind(&hash)
        .bind(DEVICE_TALK)
        .fetch_optional(&self.pool)
        .await?;
        let Some((user_id, all, resources)) = key else {
            bail!("unbekannter Schluessel");
        };
        let id = self
            .owned_device(user_id, all, &resources, requested)
            .await?;
        match self.device(id).await? {
            Some(device) if device.user_id == Some(user_id) => Ok(device),
            Some(_) => bail!("Schluessel gilt nicht fuer dieses Geraet"),
            None => bail!("Geraet gibt es nicht"),
        }
    }

    async fn owned_device(
        &self,
        user_id: i64,
        all: bool,
        resources: &[String],
        requested: Option<Uuid>,
    ) -> Result<Uuid> {
        let ids: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM devices
             WHERE user_id = $1 AND ($2 OR id::text = ANY($3))
             ORDER BY created_at",
        )
        .bind(user_id)
        .bind(all)
        .bind(resources)
        .fetch_all(&self.pool)
        .await?;
        match requested {
            Some(id) if ids.iter().any(|(found,)| *found == id) => Ok(id),
            Some(_) => bail!("Schluessel gilt nicht fuer dieses Geraet"),
            None => match ids.as_slice() {
                [(only,)] => Ok(*only),
                _ => bail!("Geraet fehlt (?device=<id>)"),
            },
        }
    }

    pub async fn add_message(&self, device: Uuid, role: &str, content: &str) -> Result<()> {
        self.add_message_meta(device, role, content, json!({}))
            .await
    }

    pub async fn add_message_meta(
        &self,
        device: Uuid,
        role: &str,
        content: &str,
        metadata: Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO device_messages (device_id, role, content, metadata) VALUES ($1, $2, $3, $4)",
        )
        .bind(device)
        .bind(role)
        .bind(content)
        .bind(metadata)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM device_messages WHERE device_id = $1 AND id <=
               (SELECT MAX(id) FROM device_messages WHERE device_id = $1) - $2",
        )
        .bind(device)
        .bind(KEEP_MESSAGES)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The last question and the last reply, oldest first. Tool rows and empty tool-call
    /// turns are left out; everything older lives on only in the context.
    pub async fn last_exchange(&self, device: Uuid) -> Result<Vec<StoredMessage>> {
        Ok(sqlx::query_as(
            "SELECT role, content, metadata FROM
               (SELECT id, role, content, metadata FROM device_messages
                WHERE device_id = $1 AND role IN ('user', 'assistant') AND content <> ''
                ORDER BY id DESC LIMIT 2) m
             ORDER BY id",
        )
        .bind(device)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_context(&self, device: Uuid, context: &str) -> Result<()> {
        sqlx::query("UPDATE devices SET context = $2 WHERE id = $1")
            .bind(device)
            .bind(context)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_run(
        &self,
        user_id: i64,
        device_id: Uuid,
        channel: &str,
        user_text: &str,
    ) -> Result<Uuid> {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO agent_runs (user_id, device_id, channel, status, user_text)
             VALUES ($1, $2, $3, 'running', $4) RETURNING id",
        )
        .bind(user_id)
        .bind(device_id)
        .bind(channel)
        .bind(user_text)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_step(
        &self,
        run_id: Uuid,
        seq: i32,
        kind: &str,
        name: &str,
        payload: Value,
        duration_ms: i32,
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO agent_run_steps (run_id, seq, kind, name, payload, duration_ms, error)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(run_id)
        .bind(seq)
        .bind(kind)
        .bind(name)
        .bind(payload)
        .bind(duration_ms)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_run(
        &self,
        id: Uuid,
        status: &str,
        reply: &str,
        error: &str,
        iterations: i32,
        input_tokens: i32,
        output_tokens: i32,
        pending: Option<Value>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE agent_runs SET status = $2, final_text = $3, error = $4, iterations = $5,
                 input_tokens = $6, output_tokens = $7, pending_tools = $8,
                 finished_at = CASE WHEN $2 = 'waiting_for_approval' THEN NULL ELSE NOW() END
             WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(reply)
        .bind(error)
        .bind(iterations)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(pending)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn cancel_run_if_running(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE agent_runs SET status = 'cancelled', error = 'abgebrochen', finished_at = NOW()
             WHERE id = $1 AND status = 'running'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pending_approval(&self, device: Uuid) -> Result<Option<PendingRun>> {
        Ok(sqlx::query_as(
            "SELECT id, COALESCE(pending_tools, '{}'::jsonb) AS pending_tools, started_at FROM agent_runs
             WHERE device_id = $1 AND status = 'waiting_for_approval'
             ORDER BY started_at DESC LIMIT 1",
        )
        .bind(device)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn cancel_pending(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE agent_runs SET status = 'cancelled', error = 'neue Anfrage', finished_at = NOW()
             WHERE id = $1 AND status = 'waiting_for_approval'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The user answered the confirmation, so this run must not be offered again.
    pub async fn close_pending(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE agent_runs SET status = 'completed', finished_at = NOW()
             WHERE id = $1 AND status = 'waiting_for_approval'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mcp_server(&self, id: Uuid) -> Result<Option<McpServer>> {
        Ok(sqlx::query_as(
            "SELECT id, user_id, name, transport, url, enabled, auth_type, secret_enc, oauth_enc
             FROM mcp_servers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn set_mcp_status(&self, id: Uuid, status: &str, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE mcp_servers SET status = $2, last_error = $3,
                 last_ok_at = CASE WHEN $2 = 'ok' THEN NOW() ELSE last_ok_at END
             WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_oauth(&self, id: Uuid, oauth_enc: &str) -> Result<()> {
        sqlx::query("UPDATE mcp_servers SET oauth_enc = $2 WHERE id = $1")
            .bind(id)
            .bind(oauth_enc)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn replace_mcp_tools(&self, server: Uuid, tools: &[DiscoveredTool]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE mcp_tools SET present = false WHERE server_id = $1")
            .bind(server)
            .execute(&mut *tx)
            .await?;
        for tool in tools {
            sqlx::query(
                "INSERT INTO mcp_tools (server_id, name, description, input_schema, annotations, effect, present)
                 VALUES ($1, $2, $3, $4, $5, $6, true)
                 ON CONFLICT (server_id, name) DO UPDATE SET
                   description = EXCLUDED.description,
                   input_schema = EXCLUDED.input_schema,
                   annotations = EXCLUDED.annotations,
                   present = true",
            )
            .bind(server)
            .bind(&tool.name)
            .bind(&tool.description)
            .bind(&tool.input_schema)
            .bind(&tool.annotations)
            .bind(tool.effect.as_str())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn mcp_tools_for(&self, user_id: i64) -> Result<Vec<ToolRow>> {
        Ok(sqlx::query_as(
            "SELECT t.name, t.description, t.input_schema, t.annotations, t.effect, t.enabled, t.server_id
             FROM mcp_tools t JOIN mcp_servers s ON s.id = t.server_id
             WHERE s.user_id = $1 AND s.enabled AND t.enabled AND t.present
             ORDER BY s.name, t.name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn mcp_tool(&self, user_id: i64, name: &str) -> Result<Option<ToolRow>> {
        Ok(sqlx::query_as(
            "SELECT t.name, t.description, t.input_schema, t.annotations, t.effect, t.enabled, t.server_id
             FROM mcp_tools t JOIN mcp_servers s ON s.id = t.server_id
             WHERE s.user_id = $1 AND s.enabled AND t.present AND t.name = $2
             LIMIT 1",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn native_policy(&self, user_id: i64, name: &str) -> Result<Option<(String, bool)>> {
        let row: Option<(String, bool)> = sqlx::query_as(
            "SELECT effect, enabled FROM native_tool_policies WHERE user_id = $1 AND name = $2",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct PendingRun {
    pub id: Uuid,
    pub pending_tools: Value,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct McpServer {
    pub id: Uuid,
    pub user_id: i64,
    pub name: String,
    pub transport: String,
    pub url: String,
    pub enabled: bool,
    pub auth_type: String,
    pub secret_enc: String,
    pub oauth_enc: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ToolRow {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    pub effect: Option<String>,
    pub enabled: bool,
    pub server_id: Uuid,
}

/// `hashApiKey` of @veit/api-keys: sha256 of `${pepper}:${key}` as hex.
pub fn hash_api_key(key: &str, pepper: &str) -> String {
    let digest = Sha256::digest(format!("{pepper}:{key}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn hashes_like_the_framework() {
        // node -e "console.log(require('crypto').createHash('sha256').update('p:spa_abc').digest('hex'))"
        assert_eq!(
            super::hash_api_key("spa_abc", "p"),
            "6ed95a751c27aa5c619ae8e9d8b7f92cb33b4d8c60d27a5758d07affe060b2ac"
        );
    }
}
