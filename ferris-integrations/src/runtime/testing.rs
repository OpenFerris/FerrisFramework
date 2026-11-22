//! A tiny single-threaded HTTP server for tests.
//!
//! The runtime's outbound calls go through [`reqwest`], so a test needs a real
//! TCP endpoint — no wiremock dependency, no external process. This helper
//! binds an ephemeral port, serves every request with the configured responses
//! and records what it received, so a test can assert on method/path/headers
//! and body.
//!
//! Only compiled for tests (`#[cfg(test)]`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What one request looked like when it arrived.
#[derive(Debug, Clone, Default)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    /// The request body as text (lossy).
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

type Route = (String, String, u16, String);

/// A test HTTP server bound to `127.0.0.1:0`.
#[derive(Clone)]
pub struct TestServer {
    /// Base URL (`http://127.0.0.1:<port>`).
    pub url: String,
    received: Arc<Mutex<Vec<CapturedRequest>>>,
    shutdown: Arc<AtomicBool>,
    _join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl TestServer {
    /// Serve every request with `status`/`body`.
    pub fn start(status: u16, body: &'static str) -> Self {
        Self::start_routes(vec![(
            "*".to_string(),
            String::new(),
            status,
            body.to_string(),
        )])
    }

    /// Serve requests against `routes` — `(method or "*", path prefix, status,
    /// body)`, first match wins, everything else 404s.
    pub fn start_routes(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test HTTP server bind");
        listener
            .set_nonblocking(true)
            .expect("test HTTP server nonblocking");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("test server addr")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let recv = received.clone();
        let stop = shutdown.clone();
        let join = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &routes, &recv),
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });

        Self {
            url,
            received,
            shutdown,
            _join: Arc::new(Mutex::new(Some(join))),
        }
    }

    /// Every captured request, oldest first.
    pub fn received(&self) -> Vec<CapturedRequest> {
        let Ok(guard) = self.received.lock() else {
            return Vec::new();
        };
        guard.clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn serve(stream: TcpStream, routes: &[Route], received: &Arc<Mutex<Vec<CapturedRequest>>>) {
    let mut stream = stream;
    let Some(captured) = read_request(&mut stream) else {
        return;
    };
    if let Ok(mut guard) = received.lock() {
        guard.push(captured.clone());
    }
    let (status, body) = match_routes(routes, &captured);
    let _ = write_response(&mut stream, status, &body);
}

fn match_routes(routes: &[Route], captured: &CapturedRequest) -> (u16, String) {
    for (method, prefix, status, body) in routes {
        if (method == "*" || method.eq_ignore_ascii_case(&captured.method))
            && captured.path.starts_with(prefix)
        {
            return (*status, body.clone());
        }
    }
    (404, "not found".to_string())
}

fn read_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = find_header_end(&buf) {
                    break Some(end);
                }
            }
        }
    };
    let end = header_end?;
    let header_text = String::from_utf8_lossy(&buf[..end]).into_owned();
    let content_length = header_text
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < end + 4 + content_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }

    let mut lines = header_text.lines();
    let request_line = lines.next()?.to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let body = buf
        .get(end + 4..)
        .unwrap_or_default()
        .iter()
        .take(content_length)
        .copied()
        .collect();
    Some(CapturedRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "ERROR" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

// ---------------------------------------------------------------------------
// Shared database context for runtime tests
// ---------------------------------------------------------------------------

use super::state::IntegrationsContext;
use ferris_schema::{DoctypeMeta, FieldMeta, register_doctype};

/// Process-wide lock serialising every test that installs hooks or writes
/// documents. The global `HookRegistry` and this crate's `hooks::install`
/// replace the process-wide closures, and each test carries its own in-memory
/// `Db`; without the lock, a concurrent test's document write could fire a
/// hook closure bound to another test's database. Pure-logic tests (condition,
/// template, signature, date maths…) skip it and stay parallel.
pub static TEST_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

/// Hold while a test touches the global hook registry or the engine (`Db`).
pub async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// The `Notification` DocType meta the runtime writes to (mirror of
/// `ferris_api::doctypes::notification_meta`, spelled out here so a runtime
/// test does not have to pull the whole API crate in).
fn notification_meta() -> DoctypeMeta {
    DoctypeMeta {
        name: super::names::NOTIFICATION.to_string(),
        module: "Desk".to_string(),
        description: Some("A notification addressed to a single user.".to_string()),
        naming_series: String::new(),
        fields: vec![
            FieldMeta::data("user")
                .label("User")
                .required()
                .in_list_view(),
            FieldMeta::data("notification_type")
                .label("Notification Type")
                .default_value("General"),
            FieldMeta::data("title")
                .label("Title")
                .required()
                .in_list_view(),
            FieldMeta::text("description").label("Description"),
            FieldMeta::data("route").label("Route"),
            FieldMeta::check("is_read")
                .label("Is Read")
                .default_value("0"),
        ],
        hidden: true,
        allow_delete: true,
        permissions: Vec::new(),
        sort_field: Some("creation".to_string()),
        sort_order: Some("desc".to_string()),
        ..Default::default()
    }
}

/// A fixed 32-byte AES-256-GCM key for the test harness (64 hex characters).
///
/// Not a credential: it only encrypts the throwaway in-memory rows every test
/// creates, and it is the same value the document-engine tests install.
pub const TEST_SECRET_KEY: &str =
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// A context over an in-memory database with the integrations DocTypes (and the
/// Notifications meta the runtime writes) registered. Cheap to call per test.
pub async fn test_context() -> IntegrationsContext {
    // Several integration DocTypes declare a one-way `Password` field and the
    // engine refuses to write one without a hasher. Production installs Argon2
    // from `ferris-auth` during boot; these tests never run that boot, so
    // install a fast deterministic hook here. Nothing in this suite verifies a
    // real password against the stored value.
    let _ = ferris_core::password::set_password_hasher(|plain| Ok(format!("hashed:{plain}")));
    // The recoverable `Secret` fields (`client_secret`, `webhook_secret`, …)
    // need an AEAD keyring instead. One fixed test key, installed idempotently:
    // `install_keyring` refuses a *different* ring, and every test here wants
    // the same one.
    let _ = ferris_core::secret::install_keyring(ferris_core::secret::SecretKeyring::new(
        ferris_core::secret::SecretKey::parse(TEST_SECRET_KEY).expect("32-byte test key"),
        Vec::new(),
    ));
    crate::doctypes::register_doctypes();
    register_doctype(notification_meta());
    let db = ferris_db::memory_db()
        .await
        .expect("in-memory test database");
    IntegrationsContext::new(db)
}
