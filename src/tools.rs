//! One list of tools for the model. A tool is a name, a description and a JSON schema.
//! The model does not see whether the call goes to MCP, SearXNG or a function in this file.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::authz::{validate_args, Effect};
use crate::mcp;
use crate::page;
use crate::secrets::Secrets;
use crate::store::{Store, ToolRow};
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
    /// Only for `follow`: the model picks these addresses, so nothing inside the Pi or the home network.
    page_http: reqwest::Client,
    rate: RateGate,
    /// Self-hosted SearXNG for `news` and `search`, without a trailing slash.
    searxng_url: String,
}

impl LiveTools {
    pub fn new(store: Store, secrets: Secrets, searxng_url: &str) -> Result<Self> {
        Ok(Self {
            store,
            secrets,
            http: mcp::http_client()?,
            page_http: public_page_client()?,
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
search: Websuche, query nötig. follow: url einer https-Seite lesen, das Ergebnis nennt weitere Links. \
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
                    "url": {"type": "string", "description": "https-Adresse für follow."}
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
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                follow_page(&self.page_http, url).await
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

fn worth_reading(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let path = parsed.path().trim_matches('/');
    path.contains('/') || path.len() > 12
}

const FOLLOW_LINKS: usize = 5;
/// Pages are read up to this size; the head alone of a news page can take 200 KB.
const PAGE_BYTES: usize = 2_000_000;

async fn follow_page(http: &reqwest::Client, url: &str) -> Result<String> {
    if url.is_empty() {
        anyhow::bail!("url fehlt");
    }
    let html = fetch_html(http, url).await?;
    let page = page::readable(&html);
    let mut out = if page.text.is_empty() {
        "Kein lesbarer Text.".to_string()
    } else {
        page.text
    };
    let links = next_links(page.region, url);
    if !links.is_empty() {
        out.push_str("\n\nWeitere Links:\n");
        for link in links {
            out.push_str("- ");
            out.push_str(&link);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Article links from the content of a page. Homepages, files and non-public addresses stay out.
fn next_links(html: &str, base: &str) -> Vec<String> {
    let Ok(base_url) = reqwest::Url::parse(base) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut rest = html;
    while out.len() < FOLLOW_LINKS {
        let Some(at) = rest.find("href=\"") else {
            break;
        };
        rest = &rest[at + 6..];
        let Some(end) = rest.find('"') else {
            break;
        };
        let raw = page::decode_entities(&rest[..end]);
        rest = &rest[end..];
        let Ok(mut absolute) = base_url.join(raw.trim()) else {
            continue;
        };
        absolute.set_fragment(None);
        let url = absolute.to_string();
        if !public_url_ok(&absolute)
            || !worth_reading(&url)
            || is_file(&absolute)
            || url == base
            || out.iter().any(|have| have == &url)
        {
            continue;
        }
        out.push(url);
    }
    out
}

fn is_file(url: &reqwest::Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".avif", ".ico", ".css", ".js", ".json",
        ".xml", ".woff", ".woff2", ".pdf", ".zip", ".mp3", ".mp4",
    ]
    .iter()
    .any(|ext| path.ends_with(ext))
}

/// https on a public host. The resolver and the redirect policy of `public_page_client` check the rest.
fn public_url_ok(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // The URL parser already turned spellings like 0x7f.1 into 127.0.0.1.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return public_ip(ip);
    }
    let domain = host.trim_end_matches('.').to_ascii_lowercase();
    domain != "localhost" && !domain.ends_with(".localhost")
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (b & 0xc0) == 64)
                || (a == 198 && (b & 0xfe) == 18))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Resolves host names to public addresses only, so a name pointing into the home network fails
/// and a name with both kinds of addresses is reached on the public ones.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|addr| public_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} ist keine öffentliche Adresse").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn public_page_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .no_proxy()
        .dns_resolver(Arc::new(PublicOnly))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                attempt.error("zu viele Weiterleitungen")
            } else if public_url_ok(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("Weiterleitung auf eine nicht öffentliche Adresse")
            }
        }))
        .build()?)
}

async fn fetch_html(http: &reqwest::Client, url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("keine gültige Adresse"))?;
    if !public_url_ok(&parsed) {
        anyhow::bail!("nur öffentliche https-Adressen");
    }
    let mut resp = http
        .get(url)
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Seite: HTTP {}", resp.status());
    }
    let kind = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(kind.starts_with("text/") || kind.contains("xhtml") || kind.is_empty()) {
        anyhow::bail!("keine Webseite");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() >= PAGE_BYTES {
            bytes.truncate(PAGE_BYTES);
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
    fn follow_only_reaches_public_https_addresses() {
        let ok = |url: &str| public_url_ok(&reqwest::Url::parse(url).unwrap());
        assert!(ok("https://www.tagesschau.de/inland/haushalt-100.html"));
        assert!(ok("https://8.8.8.8/"));
        for blocked in [
            "http://www.tagesschau.de/",
            "https://localhost/",
            "https://LOCALHOST./",
            "https://api.localhost/",
            "https://127.0.0.1:3020/v1/health",
            "https://0x7f.1/",
            "https://0.0.0.0/",
            "https://10.0.0.5/",
            "https://172.16.1.1/",
            "https://192.168.178.1/",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/",
            "https://[::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "file:///etc/passwd",
        ] {
            assert!(!ok(blocked), "{blocked}");
        }
    }

    #[tokio::test]
    async fn follow_client_refuses_names_of_private_addresses() {
        let client = public_page_client().unwrap();
        let err = client.get("https://localhost:9/").send().await.unwrap_err();
        assert!(
            format!("{err:?}").contains("keine öffentliche Adresse"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn follow_refuses_internal_pages() {
        let client = public_page_client().unwrap();
        for url in ["http://127.0.0.1:3020/v1/health", "https://192.168.178.1/"] {
            let err = follow_page(&client, url).await.unwrap_err();
            assert!(err.to_string().contains("öffentliche"), "{err}");
        }
    }

    #[test]
    fn follow_keeps_articles_and_drops_homepages() {
        let html = r#"
            <a href="https://www.tagesschau.de/">Start</a>
            <a href="/inland/haushalt-100.html">Artikel</a>
            <a href="javascript:void(0)">Skript</a>
            <a href="http://example.com/a/b">Klartext</a>
            <a href="https://www.zdfheute.de/newsticker/schlagzeilen-100.html">ZDF</a>
        "#;
        let links = next_links(html, "https://www.tagesschau.de/inland/index.html");
        assert_eq!(
            links,
            vec![
                "https://www.tagesschau.de/inland/haushalt-100.html".to_string(),
                "https://www.zdfheute.de/newsticker/schlagzeilen-100.html".to_string(),
            ]
        );
    }

    #[test]
    fn article_paths_are_read_before_homepages() {
        assert!(worth_reading(
            "https://www.zdfheute.de/newsticker/schlagzeilen-100.html"
        ));
        assert!(!worth_reading("https://www.tagesschau.de/"));
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
