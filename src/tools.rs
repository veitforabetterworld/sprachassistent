//! One list of tools for the model. A tool is a name, a description and a JSON schema.
//! The model does not see whether the call goes to MCP, DuckDuckGo or a function in this file.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::authz::{validate_args, Effect};
use crate::mcp;
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
    rate: RateGate,
}

impl LiveTools {
    pub fn new(store: Store, secrets: Secrets) -> Result<Self> {
        Ok(Self {
            store,
            secrets,
            http: mcp::http_client()?,
            rate: RateGate::new(),
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
                follow_page(&self.http, url).await
            }
            _ => anyhow::bail!("action muss news, search oder follow sein"),
        }
    }

    async fn news_feed(&self, query: &str) -> Result<String> {
        let headlines = self.news_headlines(query).await?;
        if headlines.is_empty() {
            Ok("Keine Schlagzeilen.".into())
        } else {
            Ok(headlines
                .into_iter()
                .take(SEARCH_LIMIT)
                .map(|(title, url)| format!("- {title}\n  {url}"))
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }

    async fn web_search(&self, query: &str) -> Result<String> {
        if query.is_empty() {
            anyhow::bail!("query fehlt");
        }
        let resp = self
            .http
            .post("https://lite.duckduckgo.com/lite/")
            .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
            .form(&[("q", query), ("kl", "de-de")])
            .timeout(Duration::from_secs(10))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("Suche: HTTP {}", resp.status());
        }
        let mut hits = parse_ddg_lite(&resp.text().await?);
        hits.sort_by_key(|(_, url, _)| !worth_reading(url));
        if hits.is_empty() {
            Ok("Keine Treffer.".into())
        } else {
            Ok(hits
                .into_iter()
                .map(|(title, url, snippet)| format!("- {title}\n  {url}\n  {snippet}"))
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }

    async fn news_headlines(&self, query: &str) -> Result<Vec<(String, String)>> {
        let url = news_rss_url(query);
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
            .timeout(Duration::from_secs(10))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }
        Ok(parse_rss_items(&resp.text().await?))
    }
}

fn news_rss_url(query: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        "https://news.google.com/rss?hl=de&gl=DE&ceid=DE:de".to_string()
    } else {
        format!(
            "https://news.google.com/rss/search?q={}&hl=de&gl=DE&ceid=DE:de",
            urlencoding_query(query)
        )
    }
}

fn urlencoding_query(query: &str) -> String {
    let mut out = String::new();
    for byte in query.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn parse_rss_items(xml: &str) -> Vec<(String, String)> {
    let mut hits = Vec::new();
    let mut rest = xml;
    while hits.len() < SEARCH_LIMIT {
        let Some(start) = rest.find("<item>") else {
            break;
        };
        rest = &rest[start + "<item>".len()..];
        let Some(end) = rest.find("</item>") else {
            break;
        };
        let item = &rest[..end];
        rest = &rest[end + "</item>".len()..];
        let Some(title) = xml_tag(item, "title") else {
            continue;
        };
        let link = xml_tag(item, "link").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        hits.push((decode_entities(&title), decode_entities(&link)));
    }
    hits
}

fn xml_tag(item: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = item.find(&open)? + open.len();
    let end = item[start..].find(&close)?;
    Some(item[start..start + end].trim().to_string())
}

fn worth_reading(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let path = parsed.path().trim_matches('/');
    path.contains('/') || path.len() > 12
}

fn parse_ddg_lite(html: &str) -> Vec<(String, String, String)> {
    let mut hits = Vec::new();
    let mut rest = html;
    while hits.len() < SEARCH_LIMIT {
        let Some(marker) = rest.find("class='result-link'") else {
            break;
        };
        let tag_start = rest[..marker].rfind("<a").unwrap_or(marker);
        let tag = &rest[tag_start..];
        let Some(open) = tag.find('>') else {
            break;
        };
        let open_tag = &tag[..open];
        let Some(href_at) = open_tag.find("href=\"") else {
            rest = &rest[marker + 1..];
            continue;
        };
        let href_start = href_at + 6;
        let Some(href_end) = open_tag[href_start..].find('"') else {
            break;
        };
        let url = decode_entities(&open_tag[href_start..href_start + href_end]);
        let Some(close) = tag[open..].find("</a>") else {
            break;
        };
        let title = strip_html(&decode_entities(&tag[open + 1..open + close]));
        rest = &rest[tag_start + open + close..];
        let snippet = rest
            .find("class='result-snippet'")
            .and_then(|at| {
                let after = &rest[at..];
                let open = after.find('>')?;
                let close = after[open..].find("</td>")?;
                Some(strip_html(&decode_entities(&after[open + 1..open + close])))
            })
            .unwrap_or_default();
        if title.is_empty() || !url.starts_with("http") {
            continue;
        }
        hits.push((title, url, snippet));
    }
    hits
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

const FOLLOW_LINKS: usize = 5;

async fn follow_page(http: &reqwest::Client, url: &str) -> Result<String> {
    if url.is_empty() {
        anyhow::bail!("url fehlt");
    }
    let page = fetch_html(http, url).await?;
    let excerpts = article_excerpts(&page, 4000);
    let text = if excerpts.chars().count() > 80 {
        excerpts
    } else {
        page_text(&page, 4000)
    };
    let links = next_links(&page, url);
    if links.is_empty() {
        return Ok(text);
    }
    let mut out = text;
    out.push_str("\n\nWeitere Links:\n");
    for link in links {
        out.push_str("- ");
        out.push_str(&link);
        out.push('\n');
    }
    Ok(out)
}

/// Article links from a page. Homepages and non-https addresses stay out.
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
        let raw = decode_entities(&rest[..end]);
        rest = &rest[end..];
        let Ok(absolute) = base_url.join(raw.trim()) else {
            continue;
        };
        let url = absolute.to_string();
        if !mcp::server_url_ok(&url) || !worth_reading(&url) || out.iter().any(|have| have == &url)
        {
            continue;
        }
        out.push(url);
    }
    out
}

fn article_excerpts(html: &str, max_chars: usize) -> String {
    let lower = html.to_ascii_lowercase();
    let mut parts = Vec::new();
    let mut from = 0;
    while parts.len() < 4 {
        let Some(rel) = lower[from..].find("<article") else {
            break;
        };
        let start = from + rel;
        let Some(open) = html[start..].find('>') else {
            break;
        };
        let content = start + open + 1;
        let Some(end_rel) = lower[content..].find("</article") else {
            break;
        };
        let piece = page_text(&html[content..content + end_rel], 240);
        if piece.chars().count() > 40 {
            parts.push(piece);
        }
        from = content + end_rel + "</article".len();
        if from >= html.len() {
            break;
        }
    }
    crate::authz::truncate(&parts.join(" "), max_chars)
}

async fn fetch_html(http: &reqwest::Client, url: &str) -> Result<String> {
    if !mcp::server_url_ok(url) {
        anyhow::bail!("nur https-Adressen");
    }
    let resp = http
        .get(url)
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
        .timeout(Duration::from_secs(8))
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
    if !(kind.starts_with("text/")
        || kind.contains("json")
        || kind.contains("xml")
        || kind.is_empty())
    {
        anyhow::bail!("kein Text");
    }
    let bytes = resp.bytes().await?;
    let cut = bytes.len().min(100_000);
    Ok(String::from_utf8_lossy(&bytes[..cut]).into_owned())
}

fn strip_html(input: &str) -> String {
    page_text(input, 4000)
}

fn page_text(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut skip = false;
    let lower = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &lower[i..];
        if rest.starts_with("<script")
            || rest.starts_with("<style")
            || rest.starts_with("<nav")
            || rest.starts_with("<header")
            || rest.starts_with("<footer")
            || rest.starts_with("<aside")
            || rest.starts_with("<noscript")
            || rest.starts_with("<svg")
        {
            skip = true;
        }
        if skip
            && (rest.starts_with("</script")
                || rest.starts_with("</style")
                || rest.starts_with("</nav")
                || rest.starts_with("</header")
                || rest.starts_with("</footer")
                || rest.starts_with("</aside")
                || rest.starts_with("</noscript")
                || rest.starts_with("</svg"))
        {
            skip = false;
            in_tag = true;
        }
        let c = input[i..].chars().next().unwrap_or(' ');
        let w = c.len_utf8();
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
            out.push(' ');
        } else if !in_tag && !skip {
            out.push(c);
        }
        i += w;
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::authz::truncate(&collapsed, max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn news_without_a_query_is_the_front_page() {
        let front = news_rss_url("");
        assert!(front.contains("/rss?"));
        assert!(!front.contains("duckduckgo"));
        assert!(!front.contains("/rss/search"));
        let search = news_rss_url("Ukraine");
        assert!(search.contains("/rss/search"));
        assert!(search.contains("Ukraine"));
        assert!(!search.contains("duckduckgo"));
    }

    #[test]
    fn news_parser_and_search_parser_stay_apart() {
        let rss =
            r#"<item><title>Haushalt</title><link>https://news.google.com/rss/a</link></item>"#;
        assert!(parse_ddg_lite(rss).is_empty());
        let ddg = r#"<a href="https://example.com/a/b" class='result-link'>Titel</a><td class='result-snippet'>kurz</td>"#;
        assert!(parse_rss_items(ddg).is_empty());
        assert!(news_rss_url("Haushalt").starts_with("https://news.google.com/"));
        assert!(!news_rss_url("Haushalt").contains("duckduckgo"));
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
    fn rss_items_are_the_headlines() {
        let xml = r#"<channel><title>Top</title><item><title>Trump plant die Flucht - Rundschau</title><link>https://news.google.com/a</link></item><item><title>Bas und Merz - T-Online</title><link>https://news.google.com/b</link></item></channel>"#;
        let hits = parse_rss_items(xml);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].0.contains("Trump"));
        assert_eq!(hits[1].1, "https://news.google.com/b");
    }

    #[test]
    fn parses_duckduckgo_lite_results() {
        let html = r#"
            <a rel="nofollow" href="https://www.tagesschau.de/?a=1&amp;b=2" class='result-link'>Tagesschau <b>heute</b></a>
            <td class='result-snippet'>die wichtigsten <b>News</b></td>
            <a rel="nofollow" href="https://example.com/x" class='result-link'>Zweite</a>
            <td class='result-snippet'>kurz</td>
        "#;
        let hits = parse_ddg_lite(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "Tagesschau heute");
        assert_eq!(hits[0].1, "https://www.tagesschau.de/?a=1&b=2");
        assert!(hits[0].2.contains("News"));
        assert_eq!(hits[1].0, "Zweite");
    }

    #[test]
    fn article_paths_are_read_before_homepages() {
        assert!(worth_reading(
            "https://www.zdfheute.de/newsticker/schlagzeilen-100.html"
        ));
        assert!(!worth_reading("https://www.tagesschau.de/"));
    }

    #[test]
    fn article_excerpts_keep_the_story() {
        let html = "<nav>Menü</nav><article><h2>Haushaltsstreit in Belgien</h2><p>De Wever geht zum König.</p></article><article><p>Kurzer Krümel</p></article>";
        let text = article_excerpts(html, 400);
        assert!(text.contains("Haushaltsstreit"));
        assert!(text.contains("König"));
        assert!(!text.contains("Menü"));
        assert!(!text.contains("Krümel"));
    }

    #[test]
    fn page_text_drops_navigation() {
        let text = page_text("<nav>Menü</nav><p>Eilmeldung aus Berlin</p>", 200);
        assert!(text.contains("Eilmeldung"));
        assert!(!text.contains("Menü"));
    }

    #[test]
    fn strips_tags_and_scripts() {
        let text = strip_html("<p>Hallo</p><script>token</script><b>Welt</b>");
        assert!(text.contains("Hallo"));
        assert!(text.contains("Welt"));
        assert!(!text.contains("token"));
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
