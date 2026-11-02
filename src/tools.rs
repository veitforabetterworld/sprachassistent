//! One list of tools for the model. A tool is a name, a description and a JSON schema.
//! The model does not see whether the call goes to MCP, SearXNG or a function in this file.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::authz::{validate_args, Effect};
use crate::browser::Browser;
use crate::mcp;
use crate::secrets::Secrets;
use crate::store::{Store, ToolRow};
use crate::web::{worth_reading, Reader, FOLLOW_MAX};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

const RATE_WINDOW: Duration = Duration::from_secs(300);
const RATE_MAX: usize = 30;

struct RateGate {
    hits: Mutex<HashMap<i64, Vec<Instant>>>,
}

impl RateGate {
    fn new() -> Self {
        Self {
            hits: Mutex::new(HashMap::new()),
        }
    }

    fn allow(&self, user_id: i64) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = hits.entry(user_id).or_default();
        entry.retain(|t| now.duration_since(*t) < RATE_WINDOW);
        if entry.len() >= RATE_MAX {
            return false;
        }
        entry.push(now);
        true
    }
}
const SEARCH_LIMIT: usize = 5;

#[derive(Debug, Clone)]
pub struct ToolOffer {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub effect: Effect,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn list(&self, user_id: i64) -> Result<Vec<ToolOffer>>;
    async fn execute(&self, user_id: i64, name: &str, args: Value) -> Result<String>;
}

pub struct EmptyTools;

#[async_trait]
impl ToolExecutor for EmptyTools {
    async fn list(&self, _user_id: i64) -> Result<Vec<ToolOffer>> {
        Ok(Vec::new())
    }

    async fn execute(&self, _user_id: i64, name: &str, _args: Value) -> Result<String> {
        anyhow::bail!("Werkzeug {name} gibt es nicht");
    }
}

pub struct LiveTools {
    store: Store,
    secrets: Secrets,
    http: reqwest::Client,
    /// `follow`: the model picks these addresses, so nothing inside the Pi or the home network.
    reader: Reader,
    rate: RateGate,
    /// Self-hosted SearXNG for `news` and `search`, without a trailing slash.
    searxng_url: String,
}

impl LiveTools {
    pub fn new(
        store: Store,
        secrets: Secrets,
        searxng_url: &str,
        browser: Option<Arc<Browser>>,
    ) -> Result<Self> {
        Ok(Self {
            store,
            secrets,
            http: mcp::http_client()?,
            reader: Reader::new(browser)?,
            rate: RateGate::new(),
            searxng_url: searxng_url.trim_end_matches('/').to_string(),
        })
    }
}

#[async_trait]
impl ToolExecutor for LiveTools {
    async fn list(&self, user_id: i64) -> Result<Vec<ToolOffer>> {
        let mut offers = vec![native_offer(
            "web_research",
            "Internetrecherche. action ist news, search oder follow. news: Schlagzeilen, query optional. \
search: Websuche, query nötig. follow: eine https-Seite lesen (url) oder bis zu drei auf einmal (urls). \
Jede Seite endet mit Links samt Titel; interessanten Links folgst du wieder mit follow. \
Mehrere Schritte sind erlaubt. Nicht für Geräte im Haus.",
            json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["news", "search", "follow"],
                        "description": "news: Schlagzeilen. search: Websuche. follow: eine Seite lesen."
                    },
                    "query": {"type": "string", "description": "Suchbegriff für news oder search."},
                    "url": {"type": "string", "description": "https-Adresse für follow."},
                    "urls": {
                        "type": "array",
                        "items": {"type": "string"},
                        "maxItems": FOLLOW_MAX,
                        "description": "Mehrere https-Adressen für follow, höchstens drei."
                    }
                }
            }),
        )];
        for row in self.store.mcp_tools_for(user_id).await? {
            offers.push(offer_from_row(row));
        }
        for offer in &mut offers {
            if offer.name == "web_research" {
                offer.effect = self.research_effect(user_id).await?;
                continue;
            }
            if let Some((effect, enabled)) = self.store.native_policy(user_id, &offer.name).await? {
                if !enabled {
                    offer.effect = Effect::Deny;
                } else if let Some(parsed) = Effect::parse(&effect) {
                    offer.effect = parsed;
                }
            }
        }
        // A denied tool is not offered. The backend would refuse the call anyway.
        offers.retain(|o| o.effect != Effect::Deny);
        Ok(offers)
    }

    async fn execute(&self, user_id: i64, name: &str, args: Value) -> Result<String> {
        if !self.rate.allow(user_id) {
            anyhow::bail!("Zu viele Werkzeugaufrufe, kurz warten");
        }
        if name == "web_research" {
            if self.research_effect(user_id).await? == Effect::Deny {
                anyhow::bail!("Diese Aktion ist gesperrt");
            }
        } else if let Some((effect, enabled)) = self.store.native_policy(user_id, name).await? {
            if !enabled || effect == "deny" {
                anyhow::bail!("Diese Aktion ist gesperrt");
            }
        }
        match name {
            "web_research" => self.web_research(args).await,
            _ => {
                let Some(tool) = self.store.mcp_tool(user_id, name).await? else {
                    anyhow::bail!("Werkzeug {name} gibt es nicht");
                };
                if !tool.enabled {
                    anyhow::bail!("Werkzeug {name} ist aus");
                }
                validate_args(&tool.input_schema, &args).map_err(|e| anyhow!(e))?;
                mcp::call(&self.store, &self.secrets, tool.server_id, name, args).await
            }
        }
    }
}

fn native_offer(name: &str, description: &str, parameters: Value) -> ToolOffer {
    ToolOffer {
        name: name.into(),
        description: description.into(),
        parameters,
        effect: Effect::Auto,
    }
}

fn offer_from_row(row: ToolRow) -> ToolOffer {
    let effect = row
        .effect
        .as_deref()
        .and_then(Effect::parse)
        .unwrap_or_else(|| default_effect_from_row(&row));
    ToolOffer {
        name: row.name,
        description: row.description,
        parameters: row.input_schema,
        effect,
    }
}

fn default_effect_from_row(row: &ToolRow) -> Effect {
    crate::authz::default_effect(&row.name, &row.annotations)
}

impl LiveTools {
    /// Saved `web_research` wins. Until then a blocked `web_search` or `fetch_url` blocks the whole tool.
    async fn research_effect(&self, user_id: i64) -> Result<Effect> {
        if let Some((effect, enabled)) = self.store.native_policy(user_id, "web_research").await? {
            if !enabled {
                return Ok(Effect::Deny);
            }
            return Ok(Effect::parse(&effect).unwrap_or(Effect::Auto));
        }
        for name in ["web_search", "fetch_url"] {
            if let Some((effect, enabled)) = self.store.native_policy(user_id, name).await? {
                if !enabled || effect == "deny" {
                    return Ok(Effect::Deny);
                }
            }
        }
        Ok(Effect::Auto)
    }

    async fn web_research(&self, args: Value) -> Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        match action {
            "news" => self.news_feed(query).await,
            "search" => self.web_search(query).await,
            "follow" => {
                let mut urls: Vec<String> = args
                    .get("urls")
                    .and_then(|v| v.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
                    urls.insert(0, url.to_string());
                }
                self.reader.follow(&urls).await
            }
            _ => anyhow::bail!("action muss news, search oder follow sein"),
        }
    }

    async fn news_feed(&self, query: &str) -> Result<String> {
        // SearXNG needs a query; without one the day's German news stand in for the front page.
        let (query, range) = if query.is_empty() {
            ("Nachrichten Deutschland", "day")
        } else {
            (query, "week")
        };
        let hits = self.searxng(query, "news", Some(range)).await?;
        if hits.is_empty() {
            return Ok("Keine Schlagzeilen.".into());
        }
        Ok(format_hits(hits))
    }

    async fn web_search(&self, query: &str) -> Result<String> {
        if query.is_empty() {
            anyhow::bail!("query fehlt");
        }
        let mut hits = self.searxng(query, "general", None).await?;
        hits.sort_by_key(|hit| !worth_reading(&hit.url));
        if hits.is_empty() {
            return Ok("Keine Treffer.".into());
        }
        Ok(format_hits(hits))
    }

    async fn searxng(
        &self,
        query: &str,
        category: &str,
        time_range: Option<&str>,
    ) -> Result<Vec<SearxHit>> {
        let mut params = vec![
            ("q", query),
            ("format", "json"),
            ("language", "de"),
            ("categories", category),
        ];
        if let Some(range) = time_range {
            params.push(("time_range", range));
        }
        let resp = self
            .http
            .get(format!("{}/search", self.searxng_url))
            .query(&params)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| anyhow!("Suche nicht erreichbar: {e}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("Suche: HTTP {}", resp.status());
        }
        Ok(searx_hits(resp.json().await?))
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxHit>,
}

#[derive(Debug, serde::Deserialize)]
struct SearxHit {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

/// Results without title or web address are dropped, the same page from two engines counts once.
fn searx_hits(resp: SearxResponse) -> Vec<SearxHit> {
    let mut hits: Vec<SearxHit> = Vec::new();
    for hit in resp.results {
        if hit.title.trim().is_empty() || !hit.url.starts_with("http") {
            continue;
        }
        if hits.iter().any(|have| have.url == hit.url) {
            continue;
        }
        hits.push(hit);
        if hits.len() >= SEARCH_LIMIT {
            break;
        }
    }
    hits
}

fn format_hits(hits: Vec<SearxHit>) -> String {
    hits.into_iter()
        .map(|hit| {
            let snippet = crate::authz::truncate(hit.content.trim(), 300);
            if snippet.is_empty() {
                format!("- {}\n  {}", hit.title.trim(), hit.url)
            } else {
                format!("- {}\n  {}\n  {snippet}", hit.title.trim(), hit.url)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searxng_results_are_deduplicated_and_capped() {
        let resp: SearxResponse = serde_json::from_value(json!({
            "query": "Haushalt",
            "results": [
                {"title": "Haushalt beschlossen", "url": "https://www.tagesschau.de/inland/haushalt-100.html", "content": "Der Bundestag", "engine": "bing"},
                {"title": "Haushalt beschlossen", "url": "https://www.tagesschau.de/inland/haushalt-100.html", "engine": "brave"},
                {"title": "", "url": "https://example.com/leer"},
                {"title": "Kein Link", "url": "javascript:void(0)"},
                {"title": "Zwei", "url": "https://example.com/a/2"},
                {"title": "Drei", "url": "https://example.com/a/3"},
                {"title": "Vier", "url": "https://example.com/a/4"},
                {"title": "Fünf", "url": "https://example.com/a/5"},
                {"title": "Sechs", "url": "https://example.com/a/6"}
            ]
        }))
        .unwrap();
        let hits = searx_hits(resp);
        assert_eq!(hits.len(), SEARCH_LIMIT);
        assert_eq!(hits[0].content, "Der Bundestag");
        assert_eq!(hits[1].title, "Zwei");
        assert!(hits.iter().all(|hit| hit.url.starts_with("https://")));
    }

    #[test]
    fn searxng_without_results_is_empty() {
        let resp: SearxResponse = serde_json::from_value(json!({"query": "x"})).unwrap();
        assert!(searx_hits(resp).is_empty());
    }

    #[test]
    fn hits_read_as_title_address_snippet() {
        let text = format_hits(vec![
            SearxHit {
                title: " Titel ".into(),
                url: "https://example.com/a/b".into(),
                content: "kurz".into(),
            },
            SearxHit {
                title: "Ohne".into(),
                url: "https://example.com/c/d".into(),
                content: String::new(),
            },
        ]);
        assert_eq!(
            text,
            "- Titel
  https://example.com/a/b
  kurz
- Ohne
  https://example.com/c/d"
        );
    }

    #[test]
    fn rate_limit_stops_at_thirty() {
        let gate = RateGate::new();
        for _ in 0..RATE_MAX {
            assert!(gate.allow(7));
        }
        assert!(!gate.allow(7));
        assert!(gate.allow(8));
    }
}
