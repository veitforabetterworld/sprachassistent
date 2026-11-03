//! MCP client. Streamable HTTP via the official Rust SDK (`rmcp`), which negotiates the current
//! spec and older revisions. stdio is stored but not started: a child process would share the
//! voice server's OS user and sit next to Whisper on the Pi.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rmcp::model::{CallToolRequestParams, ToolAnnotations};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::authz::{default_effect, Effect};
use crate::failure::{self, failure};
use crate::secrets::{redact, Secrets};
use crate::store::{McpServer, Store};

const TOOL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    pub effect: Effect,
}

pub async fn discover(store: &Store, secrets: &Secrets, server_id: Uuid) -> Result<usize> {
    let Some(server) = store.mcp_server(server_id).await? else {
        return Err(failure("mcp.not_found", "MCP-Server nicht gefunden"));
    };
    if !server.enabled {
        return Err(failure("mcp.disabled", "MCP-Server ist aus"));
    }
    let found = match connect_and_list(&server, secrets, store).await {
        Ok(tools) => tools,
        Err(e) => {
            let detail = redact(&format!("{e:#}"));
            store
                .set_mcp_status(
                    server.id,
                    "error",
                    failure::code_of(&e).unwrap_or("mcp.failed"),
                    &detail.chars().take(300).collect::<String>(),
                )
                .await?;
            return Err(e);
        }
    };
    store.replace_mcp_tools(server.id, &found).await?;
    store.set_mcp_status(server.id, "ok", "", "").await?;
    Ok(found.len())
}

pub async fn call(
    store: &Store,
    secrets: &Secrets,
    server_id: Uuid,
    name: &str,
    args: Value,
) -> Result<String> {
    let Some(server) = store.mcp_server(server_id).await? else {
        return Err(failure("mcp.not_found", "MCP-Server nicht gefunden"));
    };
    let bearer = bearer_for(&server, secrets, store).await?;
    let mut last = None;
    for attempt in 0..2 {
        match call_once(&server.url, &bearer, name, args.clone()).await {
            Ok(text) => return Ok(text),
            Err(e) => {
                let transient = is_transient(&e);
                last = Some(e);
                if attempt == 0 && transient {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    continue;
                }
                break;
            }
        }
    }
    Err(last.unwrap_or_else(|| failure("mcp.failed", "MCP-Aufruf fehlgeschlagen")))
}

async fn connect_and_list(
    server: &McpServer,
    secrets: &Secrets,
    store: &Store,
) -> Result<Vec<DiscoveredTool>> {
    if server.transport != "http" {
        return Err(failure(
            "mcp.stdio_disabled",
            "stdio ist nicht freigeschaltet",
        ));
    }
    if server.url.trim().is_empty() {
        return Err(failure("mcp.no_url", "Adresse fehlt"));
    }
    let bearer = bearer_for(server, secrets, store).await?;
    let mut running = connect(&server.url, &bearer).await?;
    let tools = tokio::time::timeout(TOOL_TIMEOUT, running.list_all_tools())
        .await
        .map_err(|_| failure("mcp.timeout", "Zeitüberschreitung bei der Werkzeugliste"))?
        .map_err(|e| remote_failure(&e.to_string()))?;
    let _ = running.close().await;
    Ok(tools
        .into_iter()
        .map(|tool| {
            let annotations = annotations_json(tool.annotations.as_ref());
            let effect = default_effect(&tool.name, &annotations);
            DiscoveredTool {
                name: tool.name.to_string(),
                description: tool.description.unwrap_or_default().to_string(),
                input_schema: Value::Object((*tool.input_schema).clone()),
                annotations,
                effect,
            }
        })
        .collect())
}

fn annotations_json(annotations: Option<&ToolAnnotations>) -> Value {
    let Some(a) = annotations else {
        return json!({});
    };
    json!({
        "readOnlyHint": a.read_only_hint,
        "destructiveHint": a.destructive_hint,
        "idempotentHint": a.idempotent_hint,
        "openWorldHint": a.open_world_hint,
    })
}

async fn call_once(url: &str, bearer: &str, name: &str, args: Value) -> Result<String> {
    let mut running = connect(url, bearer).await?;
    let object = match args {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let params = CallToolRequestParams::new(name.to_string()).with_arguments(object);
    let result = tokio::time::timeout(TOOL_TIMEOUT, running.call_tool(params))
        .await
        .map_err(|_| failure("mcp.timeout", "Zeitüberschreitung"))?
        .map_err(|e| remote_failure(&e.to_string()))?;
    let _ = running.close().await;
    let mut parts = Vec::new();
    for block in &result.content {
        if let Some(text) = block.as_text() {
            parts.push(text.text.clone());
        }
    }
    if let Some(structured) = &result.structured_content {
        if parts.is_empty() {
            parts.push(structured.to_string());
        }
    }
    let body = parts.join("\n");
    if result.is_error == Some(true) {
        anyhow::bail!(if body.is_empty() {
            "Werkzeug meldet einen Fehler".to_string()
        } else {
            body
        });
    }
    Ok(body)
}

fn install_tls_provider() {
    // rmcp builds reqwest with rustls and no default crypto provider. Without this, the first
    // MCP connection panics and the admin API reports the voice server as unreachable.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

async fn connect(
    url: &str,
    bearer: &str,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    install_tls_provider();
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    if !bearer.is_empty() {
        // rmcp calls reqwest bearer_auth, which adds the "Bearer " prefix itself.
        config = config.auth_header(bearer);
    }
    let transport = StreamableHttpClientTransport::from_config(config);
    ().serve(transport)
        .await
        .map_err(|e| remote_failure(&e.to_string()))
}

/// rmcp only gives text; the HTTP status and connection errors in it are enough for a code.
fn remote_failure(text: &str) -> anyhow::Error {
    let lower = text.to_ascii_lowercase();
    let code = if ["401", "403", "unauthorized", "forbidden"]
        .iter()
        .any(|hint| lower.contains(hint))
    {
        "mcp.unauthorized"
    } else if lower.contains("timed out") {
        "mcp.timeout"
    } else if [
        "connect",
        "dns",
        "refused",
        "resolve",
        // reqwest says this for every request that never got out: unknown name, no route, TLS.
        "error sending request",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
    {
        "mcp.unreachable"
    } else {
        "mcp.failed"
    };
    failure(code, redact(text))
}

async fn bearer_for(server: &McpServer, secrets: &Secrets, store: &Store) -> Result<String> {
    match server.auth_type.as_str() {
        "none" | "" => Ok(String::new()),
        "bearer" => secrets.decrypt(&server.secret_enc),
        "oauth" => oauth_access(server, secrets, store).await,
        other => Err(failure(
            "mcp.failed",
            format!("unbekannte Anmeldung {other}"),
        )),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct OAuthBlob {
    refresh_token: String,
    access_token: String,
    expires_at: i64,
}

async fn oauth_access(server: &McpServer, secrets: &Secrets, store: &Store) -> Result<String> {
    let raw = secrets.decrypt(&server.oauth_enc)?;
    let mut blob: OAuthBlob = if raw.is_empty() {
        return Err(failure(
            "mcp.oauth_not_connected",
            "Google ist nicht verbunden",
        ));
    } else {
        serde_json::from_str(&raw).unwrap_or_default()
    };
    let now = chrono::Utc::now().timestamp();
    if !blob.access_token.is_empty() && blob.expires_at > now + 60 {
        return Ok(blob.access_token);
    }
    if blob.refresh_token.is_empty() {
        return Err(failure(
            "mcp.oauth_not_connected",
            "Google ist nicht verbunden",
        ));
    }
    let settings = store.settings().await?;
    let client_id = settings
        .get("google_oauth_client_id")
        .cloned()
        .unwrap_or_default();
    let client_secret = secrets.decrypt(
        settings
            .get("google_oauth_client_secret")
            .map(String::as_str)
            .unwrap_or(""),
    )?;
    if client_id.is_empty() || client_secret.is_empty() {
        return Err(failure(
            "mcp.oauth_not_configured",
            "Google-OAuth ist nicht eingerichtet",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", blob.refresh_token.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(failure(
            "mcp.oauth_refresh_failed",
            format!("Google-Token erneuern: HTTP {status}"),
        ));
    }
    let v: Value = resp.json().await?;
    blob.access_token = v["access_token"].as_str().unwrap_or("").to_string();
    if blob.access_token.is_empty() {
        return Err(failure(
            "mcp.oauth_refresh_failed",
            "Google-Token ohne access_token",
        ));
    }
    if let Some(refresh) = v["refresh_token"].as_str() {
        if !refresh.is_empty() {
            blob.refresh_token = refresh.to_string();
        }
    }
    let ttl = v["expires_in"].as_i64().unwrap_or(3600);
    blob.expires_at = now + ttl;
    let sealed = secrets.seal(&serde_json::to_string(&blob)?);
    store.set_oauth(server.id, &sealed).await?;
    Ok(blob.access_token)
}

pub fn is_transient(err: &anyhow::Error) -> bool {
    let s = err.to_string().to_ascii_lowercase();
    s.contains("429")
        || s.contains("503")
        || s.contains("502")
        || s.contains("timeout")
        || s.contains("zeitüberschreitung")
        || s.contains("timed out")
}

/// Keeps the reqwest client type out of the signature of callers that only need a list.
pub fn server_url_ok(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(parsed.host_str(), Some("localhost" | "127.0.0.1")),
        _ => false,
    }
}

/// Shared HTTP client for native tools. `rmcp` builds its own.
pub fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?)
}

#[allow(dead_code)]
pub fn arc_client() -> Result<Arc<reqwest::Client>> {
    Ok(Arc::new(http_client()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::code_of;

    #[test]
    fn rmcp_errors_get_a_code() {
        let code = |text: &str| code_of(&remote_failure(text));
        // As rmcp reported a Home Assistant whose name did not resolve.
        assert_eq!(
            code("Send message error Transport [rmcp::transport::worker::WorkerTransport<...>] error: Client error: error sending request for url (http://ha.example:8123/api/mcp), when send initialize request"),
            Some("mcp.unreachable")
        );
        assert_eq!(
            code("HTTP status client error (401 Unauthorized)"),
            Some("mcp.unauthorized")
        );
        assert_eq!(code("operation timed out"), Some("mcp.timeout"));
        assert_eq!(code("tool returned garbage"), Some("mcp.failed"));
    }
}
