//! AES-256-GCM for tokens and provider keys. Same layout as the Node helper in `apps/api/src/secrets.ts`:
//! `enc1:` plus base64 of `{"iv","tag","d"}`. The key is sha256(`${CREDENTIALS_KEY}|sprachassistent-credentials`).
//! Values without the prefix are still plain text, so a database from before the key keeps working.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc1:";
const CONTEXT: &str = "sprachassistent-credentials";

#[derive(Clone)]
pub struct Secrets {
    key: Option<[u8; 32]>,
}

impl Secrets {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        Self::new(std::env::var("CREDENTIALS_KEY").unwrap_or_default())
    }

    pub fn new(material: impl AsRef<str>) -> Self {
        let material = material.as_ref().trim();
        if material.is_empty() {
            return Self { key: None };
        }
        let digest = Sha256::digest(format!("{material}|{CONTEXT}").as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Self { key: Some(key) }
    }

    pub fn enabled(&self) -> bool {
        self.key.is_some()
    }

    pub fn encrypt(&self, plain: &str) -> Result<String> {
        if plain.is_empty() {
            return Ok(String::new());
        }
        let key = self
            .key
            .ok_or_else(|| anyhow!("CREDENTIALS_KEY fehlt, Geheimnis bleibt unverschlüsselt"))?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("{e}"))?;
        let mut iv = [0u8; 12];
        fill_random(&mut iv)?;
        let packed = cipher
            .encrypt(Nonce::from_slice(&iv), plain.as_bytes())
            .map_err(|e| anyhow!("verschlüsseln: {e}"))?;
        let (data, tag) = packed.split_at(packed.len().saturating_sub(16));
        let payload = serde_json::json!({
            "iv": STANDARD.encode(iv),
            "tag": STANDARD.encode(tag),
            "d": STANDARD.encode(data),
        });
        Ok(format!(
            "{PREFIX}{}",
            STANDARD.encode(payload.to_string().as_bytes())
        ))
    }

    /// Plain text passes through. `enc1:` without a key is an error, so a missing env var cannot
    /// silently turn a token into garbage that gets sent to a provider.
    pub fn decrypt(&self, stored: &str) -> Result<String> {
        if stored.is_empty() || !stored.starts_with(PREFIX) {
            return Ok(stored.to_string());
        }
        let key = self.key.ok_or_else(|| {
            anyhow!("CREDENTIALS_KEY fehlt, gespeichertes Geheimnis nicht lesbar")
        })?;
        let raw = STANDARD
            .decode(stored.trim_start_matches(PREFIX))
            .map_err(|e| anyhow!("Geheimnis nicht lesbar: {e}"))?;
        let payload: Payload = serde_json::from_slice(&raw)?;
        let iv = STANDARD.decode(payload.iv).unwrap_or_default();
        let tag = STANDARD.decode(payload.tag).unwrap_or_default();
        let data = STANDARD.decode(payload.d).unwrap_or_default();
        if iv.len() != 12 || tag.len() != 16 {
            anyhow::bail!("Geheimnis nicht lesbar");
        }
        let mut packed = data;
        packed.extend_from_slice(&tag);
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("{e}"))?;
        let plain = cipher
            .decrypt(Nonce::from_slice(&iv), packed.as_slice())
            .map_err(|_| anyhow!("Geheimnis nicht lesbar"))?;
        String::from_utf8(plain).map_err(|e| anyhow!("{e}"))
    }

    /// Encrypt when a key is configured. Otherwise keep the plain value and warn once per process.
    pub fn seal(&self, plain: &str) -> String {
        if plain.is_empty() || !self.enabled() {
            if !plain.is_empty() && !self.enabled() {
                tracing::warn!("CREDENTIALS_KEY fehlt, ein Geheimnis liegt im Klartext");
            }
            return plain.to_string();
        }
        self.encrypt(plain).unwrap_or_else(|e| {
            tracing::error!("{e:#}");
            plain.to_string()
        })
    }
}

#[derive(Deserialize)]
struct Payload {
    iv: String,
    tag: String,
    d: String,
}

fn fill_random(buf: &mut [u8]) -> Result<()> {
    let mut f = std::fs::File::open("/dev/urandom")?;
    std::io::Read::read_exact(&mut f, buf)?;
    Ok(())
}

/// Drops bearer tokens and stored ciphertext before anything is logged or stored.
pub fn redact(text: &str) -> String {
    hide_prefixed(
        &hide_prefixed(text, "bearer ", "Bearer ***"),
        "enc1:",
        "enc1:***",
    )
}

fn hide_prefixed(text: &str, marker: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let lower_marker = marker.to_ascii_lowercase();
    while let Some(rel) = rest.to_ascii_lowercase().find(&lower_marker) {
        let (head, after) = rest.split_at(rel);
        out.push_str(head);
        if after.len() < marker.len() || !after[..marker.len()].eq_ignore_ascii_case(marker) {
            break;
        }
        out.push_str(replacement);
        let token = &after[marker.len()..];
        let len = token
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(token.len());
        rest = &token[len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_and_leaves_plaintext() {
        let secrets = Secrets::new("test-material");
        let stored = secrets.encrypt("gsk_live_123").unwrap();
        assert!(stored.starts_with(PREFIX));
        assert_eq!(secrets.decrypt(&stored).unwrap(), "gsk_live_123");
        assert_eq!(secrets.decrypt("plain-key").unwrap(), "plain-key");
        assert!(Secrets::new("").decrypt(&stored).is_err());
    }

    #[test]
    fn redact_hides_bearer_tokens() {
        let s = redact("Authorization: Bearer spa_secret123 and again Bearer othersecret");
        assert!(!s.contains("spa_secret123"));
        assert!(!s.contains("othersecret"));
        assert!(s.contains("Bearer ***"));
        let sealed = redact("token enc1:aabbccdd extra");
        assert!(!sealed.contains("aabbccdd"));
        assert!(sealed.contains("enc1:***"));
    }
}
