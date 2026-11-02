//! Headless Chromium for pages that only show their content after JavaScript ran.
//!
//! Chromium runs as its own unprivileged user with its sandbox and reaches the network only through
//! a CONNECT proxy in this process. The proxy lets https to public hosts through and nothing else,
//! so a script on a foreign page cannot reach the Pi, the home network or plain http.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::web::{public_addrs, public_url_ok};

const RENDER_TIMEOUT: Duration = Duration::from_secs(25);
/// Page time scripts get before the DOM is taken. Virtual time: idle waiting is skipped.
const SCRIPT_BUDGET_MS: u32 = 8000;
const DOM_BYTES: usize = 3_000_000;
const PROXY_HEAD_BYTES: usize = 8192;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 (KHTML, like Gecko) \
Chrome/130.0.0.0 Safari/537.36";
const FLAGS: &[&str] = &[
    "--headless=new",
    "--disable-gpu",
    "--disable-dev-shm-usage",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-sync",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--mute-audio",
    "--hide-scrollbars",
    "--lang=de-DE",
    "--blink-settings=imagesEnabled=false",
    // WebRTC would otherwise open UDP past the proxy.
    "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
    // Without this Chromium talks to localhost directly instead of through the proxy.
    "--proxy-bypass-list=<-loopback>",
];

static RUNS: AtomicU64 = AtomicU64::new(0);

pub struct Browser {
    bin: PathBuf,
    /// Account Chromium runs as when this process is root.
    run_as: Option<String>,
    proxy_port: u16,
    /// One Chromium at a time; each takes a few hundred MB on the Pi.
    slot: Semaphore,
}

impl Browser {
    /// `None` when no Chromium is installed, or when this process is root and the browser account
    /// is missing: Chromium never runs as root.
    pub async fn start(bin: Option<&str>, user: &str) -> Option<Self> {
        let Some(bin) = find_bin(bin) else {
            tracing::warn!("kein Chromium gefunden, Seiten mit JavaScript bleiben leer");
            return None;
        };
        let run_as = if is_root() {
            if !user_exists(user) {
                tracing::warn!("Benutzer {user} fehlt, Chromium läuft nicht als root");
                return None;
            }
            Some(user.to_string())
        } else {
            None
        };
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(e) => {
                tracing::warn!("Browser-Proxy startet nicht: {e}");
                return None;
            }
        };
        let proxy_port = listener.local_addr().ok()?.port();
        tokio::spawn(serve_proxy(listener));
        tracing::info!("Browser: {} über Proxy :{proxy_port}", bin.display());
        Some(Self {
            bin,
            run_as,
            proxy_port,
            slot: Semaphore::new(1),
        })
    }

    /// The page's DOM after its scripts ran.
    pub async fn render(&self, url: &str) -> Result<String> {
        let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("keine gültige Adresse"))?;
        if !public_url_ok(&parsed) {
            anyhow::bail!("nur öffentliche https-Adressen");
        }
        let _slot = self.slot.acquire().await?;
        let dir = std::env::temp_dir().join(format!(
            "sprachassistent-browser-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed)
        ));
        let result = self.run(parsed.as_str(), &dir).await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        result
    }

    async fn run(&self, url: &str, dir: &Path) -> Result<String> {
        // setpriv execs Chromium in place, so killing the child on timeout kills the browser.
        let mut cmd = match &self.run_as {
            Some(user) => {
                let mut cmd = Command::new("setpriv");
                cmd.args(["--reuid", user, "--regid", user, "--clear-groups", "--"])
                    .arg(&self.bin);
                cmd
            }
            None => Command::new(&self.bin),
        };
        cmd.args(FLAGS)
            .arg(format!("--virtual-time-budget={SCRIPT_BUDGET_MS}"))
            .arg(format!("--user-data-dir={}", dir.display()))
            .arg(format!(
                "--proxy-server=http://127.0.0.1:{}",
                self.proxy_port
            ))
            .arg(format!("--user-agent={USER_AGENT}"))
            .arg("--dump-dom")
            .arg(url)
            .env("HOME", dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = cmd.spawn().context("Chromium startet nicht")?;
        let output = tokio::time::timeout(RENDER_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("Browser: Zeitüberschreitung"))??;
        if output.stdout.is_empty() {
            anyhow::bail!("Browser: leere Seite ({})", output.status);
        }
        let cut = output.stdout.len().min(DOM_BYTES);
        Ok(String::from_utf8_lossy(&output.stdout[..cut]).into_owned())
    }
}

fn find_bin(configured: Option<&str>) -> Option<PathBuf> {
    match configured.map(str::trim).filter(|bin| !bin.is_empty()) {
        Some(bin) => Some(PathBuf::from(bin)).filter(|path| path.exists()),
        None => ["/usr/bin/chromium", "/usr/bin/chromium-browser"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists()),
    }
}

#[cfg(unix)]
fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|meta| meta.uid() == 0)
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

fn user_exists(user: &str) -> bool {
    let prefix = format!("{user}:");
    std::fs::read_to_string("/etc/passwd")
        .is_ok_and(|passwd| passwd.lines().any(|line| line.starts_with(&prefix)))
}

async fn serve_proxy(listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((conn, _)) => {
                tokio::spawn(async move {
                    if let Err(e) = tunnel(conn).await {
                        tracing::debug!("Browser-Proxy: {e:#}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("Browser-Proxy: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

const REFUSED: &[u8] = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// One CONNECT request: a tunnel to a public host on port 443, or a refusal.
async fn tunnel(mut client: TcpStream) -> Result<()> {
    let head = tokio::time::timeout(Duration::from_secs(10), read_head(&mut client))
        .await
        .map_err(|_| anyhow!("Kopf kam nicht"))??;
    let Some((host, port)) = connect_target(&head) else {
        client.write_all(REFUSED).await?;
        return Ok(());
    };
    let upstream = match open_public(&host, port).await {
        Ok(upstream) => upstream,
        Err(e) => {
            client.write_all(REFUSED).await?;
            return Err(e);
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    let mut upstream = upstream;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

async fn read_head(client: &mut TcpStream) -> Result<String> {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let n = client.read(&mut buf).await?;
        if n == 0 {
            anyhow::bail!("Verbindung zu");
        }
        head.extend_from_slice(&buf[..n]);
        if head.len() > PROXY_HEAD_BYTES {
            anyhow::bail!("Kopf zu groß");
        }
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// Host and port of `CONNECT host:443`, only for public hosts. Everything else is refused.
fn connect_target(head: &str) -> Option<(String, u16)> {
    let mut first = head.lines().next()?.split_whitespace();
    if first.next()? != "CONNECT" {
        return None;
    }
    let authority = first.next()?;
    let (host, port) = authority.rsplit_once(':')?;
    if port.parse::<u16>().ok()? != 443 {
        return None;
    }
    let url = reqwest::Url::parse(&format!("https://{authority}/")).ok()?;
    if !public_url_ok(&url) {
        return None;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), 443))
}

async fn open_public(host: &str, port: u16) -> Result<TcpStream> {
    let mut last = anyhow!("{host} nicht erreichbar");
    for addr in public_addrs(host, port).await? {
        match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => last = e.into(),
            Err(_) => last = anyhow!("{addr}: Zeitüberschreitung"),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_only_tunnels_https_to_public_hosts() {
        let target = |line: &str| connect_target(&format!("{line}\r\nHost: x\r\n\r\n"));
        assert_eq!(
            target("CONNECT www.gelbe-liste.de:443 HTTP/1.1"),
            Some(("www.gelbe-liste.de".to_string(), 443))
        );
        assert_eq!(
            target("CONNECT [2a00:1450::1]:443 HTTP/1.1"),
            Some(("2a00:1450::1".to_string(), 443))
        );
        for refused in [
            "GET http://example.com/ HTTP/1.1",
            "CONNECT example.com:80 HTTP/1.1",
            "CONNECT example.com:8443 HTTP/1.1",
            "CONNECT localhost:443 HTTP/1.1",
            "CONNECT 127.0.0.1:443 HTTP/1.1",
            "CONNECT 192.168.0.141:443 HTTP/1.1",
            "CONNECT [::1]:443 HTTP/1.1",
            "CONNECT example.com HTTP/1.1",
            "",
        ] {
            assert_eq!(target(refused), None, "{refused}");
        }
    }

    #[tokio::test]
    async fn the_proxy_answers_403_to_internal_targets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve_proxy(listener));
        for request in [
            "CONNECT 127.0.0.1:3020 HTTP/1.1\r\n\r\n",
            "CONNECT localhost:443 HTTP/1.1\r\n\r\n",
            "GET http://127.0.0.1:3020/v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        ] {
            let mut conn = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            conn.write_all(request.as_bytes()).await.unwrap();
            let mut answer = String::new();
            conn.read_to_string(&mut answer).await.unwrap();
            assert!(answer.starts_with("HTTP/1.1 403"), "{request}: {answer}");
        }
    }

    #[test]
    fn a_configured_browser_must_exist() {
        assert_eq!(find_bin(Some("/gibt/es/nicht/chromium")), None);
    }
}
