//! `ferris serve --all-sites`: one public port, one `Host` → site dispatcher.
//!
//! # Why a dispatcher and not a second request pipeline
//!
//! A site is isolated by construction: its own database pool, files root, Redis
//! and [`ferris_core::site::SiteManager`] instances. The schema registry is
//! process-global, so serving two sites from one server instance would let one
//! site's app `DocType`s (and their versions) shadow another's. The supported
//! model is therefore still **one site per server process**; this module adds
//! the missing piece — a single address and `Host` routing — by supervising one
//! child process per site and splicing the TCP connection to the right one.
//!
//! The splice is deliberate: it forwards the first request *and* everything
//! after it unchanged, so keep-alive and WebSocket upgrades work without
//! parsing HTTP/2 frames or re-implementing the proxy. The trade-off is that a
//! connection stays bound to the site named in its first `Host` header (what
//! browsers do anyway) and that TLS must terminate in front of this process,
//! where the `Host` is visible.
//!
//! Unknown hosts get `421 Misdirected Request`, the same answer a single-site
//! process gives, so the failure mode does not change with the flag.

use std::collections::HashMap;
use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::ServeArgs;
use crate::config::CliConfig;

/// Largest request head buffered before the proxy gives up. A normal head is a
/// few hundred bytes; the cap keeps a hostile client from making the dispatcher
/// allocate without bound before it has chosen a site.
const MAX_HEAD: usize = 16 * 1024;

/// How long a child has to start accepting connections.
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the proxy waits for the request head of one connection.
const HEAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Run the dispatcher until `Ctrl+C` or `SIGTERM`.
///
/// # Errors
///
/// No enabled site, a site that does not start, a duplicate `Host`, or an
/// error binding the public address.
pub fn run(args: ServeArgs, config: &CliConfig) -> Result<()> {
    let manager = super::site::manager(config);
    let sites: Vec<_> = manager
        .list()?
        .into_iter()
        .filter(|site| site.enabled)
        .collect();
    if sites.is_empty() {
        anyhow::bail!(
            "no enabled site in {}; create one with `ferris site new <name>`",
            manager.sites_root().display()
        );
    }

    // Every started child is killed when this guard drops, including the
    // early return of a later site failing to boot.
    let mut children = Children::default();
    let mut routes: HashMap<String, u16> = HashMap::with_capacity(sites.len() * 2);
    for site in sites {
        let port = free_loopback_port()?;
        let mut child = spawn_site(&args, config, &site.name, port)?;
        wait_until_listening(&mut child, port)
            .with_context(|| format!("site `{}` did not start", site.name))?;

        let host = if site.host_name.trim().is_empty() {
            site.name.clone()
        } else {
            site.host_name.clone()
        };
        for key in [host.to_ascii_lowercase(), site.name.to_ascii_lowercase()] {
            if let Some(previous) = routes.insert(key.clone(), port) {
                anyhow::bail!(
                    "`{key}` is served by two sites (ports {previous} and {port}); \
                     fix their host_name in sites/<site>/site_config.json"
                );
            }
        }
        tracing::info!(
            site = %site.name,
            host = %host,
            upstream = port,
            "multi-site route registered"
        );
        children.0.push(child);
    }

    let routes = Arc::new(routes);
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(serve(args.host, args.port, routes));
    // `children` drops here (also on the accept error path): the process owns
    // its site processes, and a leaked child would keep a pool open after the
    // operator stopped the front door.
    drop(children);
    result
}

/// The site processes, killed on drop.
#[derive(Default)]
struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// One child `ferris serve --site <name>` on a private loopback port.
fn spawn_site(args: &ServeArgs, config: &CliConfig, site: &str, port: u16) -> Result<Child> {
    let exe = std::env::current_exe().context("cannot locate the running ferris binary")?;
    let mut command = Command::new(exe);
    command
        .arg("serve")
        .arg("--site")
        .arg(site)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // The parent's config must be the child's: `--config` is only forwarded
    // when it points at a real file, because an explicit missing path is an
    // error in the child while the parent may legitimately run on defaults.
    if config.source.config_path.is_file() {
        command.arg("--config").arg(&config.source.config_path);
    }
    if args.all {
        command.arg("--all");
    } else {
        for (flag, name) in [
            (args.api, "--api"),
            (args.web, "--web"),
            (args.realtime, "--realtime"),
            (args.worker, "--worker"),
            (args.scheduler, "--scheduler"),
        ] {
            if flag {
                command.arg(name);
            }
        }
    }
    for origin in &args.cors_origin {
        command.arg("--cors-origin").arg(origin);
    }
    command.spawn().with_context(|| {
        format!("cannot start the process for site `{site}` (looked for the ferris binary)")
    })
}

/// Bind a loopback port, learn its number, and release it for the child.
///
/// The small window between the release and the child's bind is inherent to
/// "pick a port for someone else"; a collision fails the child's bind and is
/// reported by [`wait_until_listening`], not silently ignored.
fn free_loopback_port() -> Result<u16> {
    let listener = StdTcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Wait for the child to accept on its port, failing fast if it exited.
fn wait_until_listening(child: &mut Child, port: u16) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!(
                "the site process exited before listening on 127.0.0.1:{port} ({status}); \
                 see its log above"
            );
        }
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}")
                .parse()
                .context("invalid loopback address")?,
            Duration::from_millis(250),
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("timed out waiting for 127.0.0.1:{port}")
}

/// Accept connections and hand each one to the site its `Host` names.
async fn serve(host: String, port: u16, routes: Arc<HashMap<String, u16>>) -> Result<()> {
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!(
        address = %listener.local_addr()?,
        routes = routes.len(),
        "multi-site dispatcher listening"
    );
    // SIGINT and SIGTERM both mean "stop": the second is what `docker stop` and
    // systemd send, and without it the parent dies before the guard can kill
    // the site processes. SIGKILL still leaks them; no user-space code can run
    // there, so the operator should use one of the two graceful signals.
    let shutdown = async {
        #[cfg(unix)]
        {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut terminate) => {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = terminate.recv() => {}
                    }
                }
                // Without a SIGTERM handler, Ctrl+C still shuts down cleanly;
                // the alternative (failing the accept loop) would be worse.
                Err(error) => {
                    tracing::warn!(%error, "cannot install the SIGTERM handler");
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("shutting down the multi-site dispatcher");
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let routes = Arc::clone(&routes);
                tokio::spawn(async move {
                    if let Err(error) = forward(stream, routes).await {
                        tracing::debug!(%peer, %error, "connection closed");
                    }
                });
            }
        }
    }
}

/// Read the request head, choose the upstream, and splice the connection.
async fn forward(mut client: TcpStream, routes: Arc<HashMap<String, u16>>) -> Result<()> {
    let Some(head) = read_head(&mut client).await? else {
        return Ok(());
    };
    // A TLS ClientHello has no cleartext Host. The dispatcher sits behind the
    // TLS terminator; saying so is better than an opaque reset.
    if head.first() == Some(&0x16) {
        return reject(
            &mut client,
            "400 Bad Request",
            "TLS must terminate before the dispatcher",
        )
        .await;
    }
    let Some(host) = host_header(&head) else {
        return reject(&mut client, "400 Bad Request", "missing Host header").await;
    };
    let Some(port) = routes.get(&normalize_host(&host)) else {
        return reject(
            &mut client,
            "421 Misdirected Request",
            "no site is configured for this Host",
        )
        .await;
    };

    let mut upstream = TcpStream::connect(("127.0.0.1", *port))
        .await
        .with_context(|| format!("cannot reach the site process on port {port}"))?;
    upstream.write_all(&head).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Read up to the end of the request head, returning the bytes consumed.
///
/// `None` means the peer closed before sending anything (a health probe, a
/// port scan); that is not an error worth logging.
async fn read_head(client: &mut TcpStream) -> Result<Option<Vec<u8>>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    loop {
        let read = tokio::time::timeout(HEAD_TIMEOUT, client.read(&mut chunk))
            .await
            .context("timed out reading the request head")??;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() >= MAX_HEAD {
            return Ok(Some(buffer));
        }
    }
}

/// The `Host` value of a raw request head.
fn host_header(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("host")
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Lowercase the host and drop the `:port` a client may append.
fn normalize_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    match host.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            name.to_string()
        }
        _ => host,
    }
}

async fn reject(client: &mut TcpStream, status: &str, message: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{message}",
        message.len()
    );
    client.write_all(response.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{host_header, normalize_host};

    #[test]
    fn host_header_is_case_insensitive_and_ignores_the_request_line() {
        let head = b"GET / HTTP/1.1\r\nHost: ERP.Example.com:8000\r\n\r\n";
        assert_eq!(host_header(head).as_deref(), Some("ERP.Example.com:8000"));
    }

    #[test]
    fn a_missing_host_is_none() {
        assert_eq!(host_header(b"GET / HTTP/1.0\r\n\r\n"), None);
    }

    #[test]
    fn normalize_drops_a_numeric_port_and_lowercases() {
        assert_eq!(normalize_host("ERP.Example.com:8000"), "erp.example.com");
        assert_eq!(normalize_host("localhost"), "localhost");
        // An IPv6 literal keeps its brackets and colons; only a trailing
        // numeric port is stripped.
        assert_eq!(normalize_host("[::1]:8000"), "[::1]");
        assert_eq!(normalize_host("[::1]"), "[::1]");
    }
}
