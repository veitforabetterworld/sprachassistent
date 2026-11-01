//! Postgres: settings, AI providers, devices with their memory, and the ESP keys of @veit/api-keys.
//! The schema comes from db/schema.sql (shared with apps/api); account tables belong to apps/api.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::llm::Provider;

const SCHEMA: &str = include_str!("../db/schema.sql");
/// Same number as SCHEMA_LOCK in apps/api/src/db.ts.
const SCHEMA_LOCK: i64 = 7_351_402;
/// Same prefix and permission as apps/api/src/accounts.ts.
const KEY_PREFIX: &str = "spa_";
const DEVICE_TALK: &str = "device.talk";
/// Older messages are deleted; what matters long-term ends up in the facts.
const KEEP_MESSAGES: i64 = 1000;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub voice: String,
    pub speed: f32,
    pub persona: String,
    pub provider_id: Option<Uuid>,
    pub history_messages: i32,
    pub memory_extraction: bool,
}

#[derive(Debug, Clone, FromRow, Serialize, PartialEq)]
pub struct Fact {
    pub id: i64,
    pub content: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
}

/// Changes the model proposes after a conversation.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FactChanges {
    pub add: Vec<String>,
    pub update: Vec<FactUpdate>,
    pub delete: Vec<i64>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct FactUpdate {
    pub id: i64,
    pub content: String,
}

#[derive(FromRow)]
struct ProviderRow {
    name: String,
    base_url: String,
    api_key: String,
    model: String,
}

impl From<ProviderRow> for Provider {
    fn from(r: ProviderRow) -> Self {
        Provider::new(&r.name, &r.base_url, r.api_key, r.model)
    }
}

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

    pub async fn device(&self, id: Uuid) -> Result<Option<Device>> {
        Ok(sqlx::query_as(
            "SELECT id, name, voice, speed, persona, provider_id, history_messages, memory_extraction
             FROM devices WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// The device's own provider first, then the others by priority. Providers without a key are only
    /// used when they run locally (Ollama).
    pub async fn providers_for(&self, device: &Device) -> Result<Vec<Provider>> {
        let rows: Vec<ProviderRow> = sqlx::query_as(
            "SELECT name, base_url, api_key, model FROM llm_providers
             WHERE enabled AND (api_key <> '' OR base_url ~ '^https?://(localhost|127\\.0\\.0\\.1)[:/]')
             ORDER BY (id = $1) DESC, priority, name",
        )
        .bind(device.provider_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Provider::from).collect())
    }

    pub async fn provider(&self, id: Uuid) -> Result<Option<Provider>> {
        let row: Option<ProviderRow> = sqlx::query_as(
            "SELECT name, base_url, api_key, model FROM llm_providers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Provider::from))
    }

    pub async fn provider_names(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM llm_providers
             WHERE enabled AND (api_key <> '' OR base_url ~ '^https?://(localhost|127\\.0\\.0\\.1)[:/]')
             ORDER BY priority, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }

    /// The device an ESP key may talk as. A key of an admin with `device.talk`; `requested` picks one when
    /// the key covers several devices.
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
        let key: Option<(Uuid, bool, Vec<String>)> = sqlx::query_as(
            "UPDATE user_api_keys k SET last_used_at = NOW() FROM users u
             WHERE k.user_id = u.id AND u.is_admin AND k.key_hash = $1 AND k.revoked_at IS NULL
               AND $2 = ANY(k.permissions)
             RETURNING k.id, k.all_resources, k.resources",
        )
        .bind(&hash)
        .bind(DEVICE_TALK)
        .fetch_optional(&self.pool)
        .await?;
        let Some((_, all, resources)) = key else {
            bail!("unbekannter Schluessel");
        };
        let id = match requested {
            Some(id) if all || resources.contains(&id.to_string()) => id,
            Some(_) => bail!("Schluessel gilt nicht fuer dieses Geraet"),
            None if resources.len() == 1 => resources[0].parse()?,
            None => {
                let ids: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM devices LIMIT 2")
                    .fetch_all(&self.pool)
                    .await?;
                match ids.as_slice() {
                    [(only,)] if all => *only,
                    _ => bail!("Geraet fehlt (?device=<id>)"),
                }
            }
        };
        match self.device(id).await? {
            Some(device) => Ok(device),
            None => bail!("Geraet gibt es nicht"),
        }
    }

    pub async fn add_message(&self, device: Uuid, role: &str, content: &str) -> Result<()> {
        sqlx::query("INSERT INTO device_messages (device_id, role, content) VALUES ($1, $2, $3)")
            .bind(device)
            .bind(role)
            .bind(content)
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

    /// The last `limit` messages, oldest first.
    pub async fn recent_messages(&self, device: Uuid, limit: i32) -> Result<Vec<StoredMessage>> {
        Ok(sqlx::query_as(
            "SELECT role, content FROM
               (SELECT id, role, content FROM device_messages WHERE device_id = $1 ORDER BY id DESC LIMIT $2) m
             ORDER BY id",
        )
        .bind(device)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn facts(&self, device: Uuid) -> Result<Vec<Fact>> {
        Ok(
            sqlx::query_as("SELECT id, content FROM device_facts WHERE device_id = $1 ORDER BY id")
                .bind(device)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn apply(&self, device: Uuid, changes: &FactChanges) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for content in changes
            .add
            .iter()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
        {
            sqlx::query("INSERT INTO device_facts (device_id, content) VALUES ($1, $2)")
                .bind(device)
                .bind(content)
                .execute(&mut *tx)
                .await?;
        }
        for u in &changes.update {
            sqlx::query(
                "UPDATE device_facts SET content = $3, updated_at = NOW() WHERE device_id = $1 AND id = $2",
            )
            .bind(device)
            .bind(u.id)
            .bind(u.content.trim())
            .execute(&mut *tx)
            .await?;
        }
        for id in &changes.delete {
            sqlx::query("DELETE FROM device_facts WHERE device_id = $1 AND id = $2")
                .bind(device)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
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
