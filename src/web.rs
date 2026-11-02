//! Reading web pages for `web_research follow`. Every address comes from the model or from a page,
//! so only public https hosts are reached: checked on the address, on DNS and on every redirect.
//! Pages that build their content with JavaScript are read again in the headless browser.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::browser::Browser;
use crate::page;

/// Pages are read up to this size; the head alone of a news page can take 200 KB.
const PAGE_BYTES: usize = 2_000_000;
/// Pages per `follow` call.
pub const FOLLOW_MAX: usize = 3;
/// Text and links of one page, and of each page when several are read at once.
const ONE_PAGE_CHARS: usize = 3000;
const MANY_PAGE_CHARS: usize = 1500;
const ONE_PAGE_LINKS: usize = 8;
const MANY_PAGE_LINKS: usize = 4;
const LINK_TITLE_CHARS: usize = 80;
/// With less readable text than this a page is read again in the browser.
const SCRIPT_PAGE_CHARS: usize = 300;

pub struct Reader {
    http: reqwest::Client,
    browser: Option<Arc<Browser>>,
}

impl Reader {
    pub fn new(browser: Option<Arc<Browser>>) -> Result<Self> {
        Ok(Self {
            http: public_page_client()?,
            browser,
        })
    }

    /// One page or up to `FOLLOW_MAX` at once, each with the links worth following next.
    pub async fn follow(&self, urls: &[String]) -> Result<String> {
        let mut wanted: Vec<&str> = Vec::new();
        for url in urls.iter().map(|url| url.trim()) {
            if !url.is_empty() && !wanted.contains(&url) {
                wanted.push(url);
            }
        }
        match wanted.len() {
            0 => anyhow::bail!("url fehlt"),
            1 => {
                return self
                    .read(wanted[0], ONE_PAGE_CHARS, ONE_PAGE_LINKS, &[])
                    .await
            }
            n if n > FOLLOW_MAX => anyhow::bail!("höchstens {FOLLOW_MAX} Adressen auf einmal"),
            _ => {}
        }
        let reads = wanted
            .iter()
            .map(|url| self.read(url, MANY_PAGE_CHARS, MANY_PAGE_LINKS, &wanted));
        let pages = futures_util::future::join_all(reads).await;
        Ok(wanted
            .iter()
            .zip(pages)
            .map(|(url, page)| match page {
                Ok(text) => format!("### {url}\n{text}"),
                Err(e) => format!("### {url}\nFehler: {e}"),
            })
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    /// The page's text and its links, without the addresses in `skip` (read in the same call).
    async fn read(
        &self,
        url: &str,
        max_chars: usize,
        max_links: usize,
        skip: &[&str],
    ) -> Result<String> {
        let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("keine gültige Adresse"))?;
        if !public_url_ok(&parsed) {
            anyhow::bail!("nur öffentliche https-Adressen");
        }
        let html = self.html(url).await?;
        let page = page::readable(&html);
        let mut out = if page.text.is_empty() {
            "Kein lesbarer Text.".to_string()
        } else {
            crate::authz::truncate(&page.text, max_chars)
        };
        let links = next_links(page.region, url, max_links, skip);
        if !links.is_empty() {
            out.push_str("\n\nLinks:");
            for (link, title) in links {
                out.push_str("\n- ");
                if !title.is_empty() {
                    out.push_str(&title);
                    out.push_str(": ");
                }
                out.push_str(&link);
            }
        }
        Ok(out)
    }

    /// The plain download, or the browser's DOM when that shows hardly any text or was refused.
    async fn html(&self, url: &str) -> Result<String> {
        let plain = fetch_html(&self.http, url).await;
        let enough = match &plain {
            Ok(html) => page::readable(html).text.chars().count() >= SCRIPT_PAGE_CHARS,
            Err(e) => e.to_string() == NOT_A_PAGE,
        };
        let Some(browser) = self.browser.as_ref().filter(|_| !enough) else {
            return plain;
        };
        match browser.render(url).await {
            Ok(rendered) => Ok(rendered),
            Err(e) => {
                tracing::warn!("Browser für {url}: {e:#}");
                plain
            }
        }
    }
}

const NOT_A_PAGE: &str = "keine Webseite";

/// Article links from the content of a page with their titles. Homepages, files, non-public
/// addresses and the pages in `skip` stay out.
fn next_links(html: &str, base: &str, max: usize, skip: &[&str]) -> Vec<(String, String)> {
    let Ok(base_url) = reqwest::Url::parse(base) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (href, title) in page::links(html) {
        if out.len() >= max {
            break;
        }
        let Ok(mut absolute) = base_url.join(&href) else {
            continue;
        };
        absolute.set_fragment(None);
        let url = absolute.to_string();
        if !public_url_ok(&absolute)
            || !worth_reading(&url)
            || is_file(&absolute)
            || url == base
            || skip.contains(&url.as_str())
        {
            continue;
        }
        let title = crate::authz::truncate(title.trim(), LINK_TITLE_CHARS);
        match out.iter_mut().find(|(have, _)| have == &url) {
            // The same article is often linked from its picture first, then from its headline.
            Some(seen) if seen.1.is_empty() => seen.1 = title,
            Some(_) => {}
            None => out.push((url, title)),
        }
    }
    out
}

/// Deeper paths are articles, the bare domain or a short section is a homepage.
pub fn worth_reading(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let path = parsed.path().trim_matches('/');
    path.contains('/') || path.len() > 12
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

/// https on a public host. The resolver and the redirect policy of `public_page_client` check the
/// rest, and the browser's proxy checks every connection again.
pub fn public_url_ok(url: &reqwest::Url) -> bool {
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

pub fn public_ip(ip: IpAddr) -> bool {
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

/// The public addresses of a host. A name pointing into the home network has none.
pub async fn public_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await?
        .filter(|addr| public_ip(addr.ip()))
        .collect();
    if addrs.is_empty() {
        anyhow::bail!("{host} ist keine öffentliche Adresse");
    }
    Ok(addrs)
}

/// Resolves host names to public addresses only, so a name pointing into the home network fails
/// and a name with both kinds of addresses is reached on the public ones.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs = public_addrs(name.as_str(), 0)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
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
        anyhow::bail!("{NOT_A_PAGE}");
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
    fn only_public_https_addresses_are_reached() {
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
    async fn the_client_refuses_names_of_private_addresses() {
        let client = public_page_client().unwrap();
        let err = client.get("https://localhost:9/").send().await.unwrap_err();
        assert!(
            format!("{err:?}").contains("keine öffentliche Adresse"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn follow_refuses_internal_pages() {
        let reader = Reader::new(None).unwrap();
        for url in ["http://127.0.0.1:3020/v1/health", "https://192.168.178.1/"] {
            let err = reader.follow(&[url.to_string()]).await.unwrap_err();
            assert!(err.to_string().contains("öffentliche"), "{err}");
        }
    }

    #[tokio::test]
    async fn several_pages_report_each_error_and_the_limit_holds() {
        let reader = Reader::new(None).unwrap();
        let text = reader
            .follow(&[
                "https://127.0.0.1/".to_string(),
                "http://example.com/a/b".to_string(),
                "https://127.0.0.1/".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(
            text,
            "### https://127.0.0.1/\nFehler: nur öffentliche https-Adressen\n\n\
             ### http://example.com/a/b\nFehler: nur öffentliche https-Adressen"
        );
        let four = ["a", "b", "c", "d"].map(|p| format!("https://example.com/x/{p}"));
        let err = reader.follow(&four).await.unwrap_err();
        assert!(err.to_string().contains("höchstens 3"), "{err}");
    }

    #[test]
    fn links_keep_articles_with_titles_and_drop_the_rest() {
        let html = r#"
            <a href="https://www.tagesschau.de/">Start</a>
            <a href="/inland/haushalt-100.html"><img src="bild.jpg"></a>
            <a href="/inland/haushalt-100.html#kommentare">Haushalt: <b>Einigung</b> erzielt</a>
            <a href="javascript:void(0)">Skript</a>
            <a href="http://example.com/a/b">Klartext</a>
            <a href='/inland/gelesen-100.html'>Schon gelesen</a>
            <a href="/media/bild-100.jpg">Bild</a>
            <a class="x" href=https://www.zdfheute.de/newsticker/schlagzeilen-100.html>ZDF</a>
            <a href="/inland/drei-100.html">Drei</a>
        "#;
        let links = next_links(
            html,
            "https://www.tagesschau.de/inland/index.html",
            2,
            &["https://www.tagesschau.de/inland/gelesen-100.html"],
        );
        assert_eq!(
            links,
            vec![
                (
                    "https://www.tagesschau.de/inland/haushalt-100.html".to_string(),
                    "Haushalt: Einigung erzielt".to_string()
                ),
                (
                    "https://www.zdfheute.de/newsticker/schlagzeilen-100.html".to_string(),
                    "ZDF".to_string()
                ),
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
}
