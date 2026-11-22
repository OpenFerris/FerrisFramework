//! Inbound pipeline tests: MIME parsing, the engine store and the poller's
//! "never flag what was not stored" contract (against a scripted TCP server).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use ferris_core::{Result as CoreResult, ValidationError, Value};
use ferris_document::{DocumentEngine, EngineContext, ListQuery};
use ferris_schema::{DoctypeMeta, FieldMeta, register_doctype};

use super::*;
use crate::imap::{ImapClient, ImapConfig};

const RAW: &[u8] = b"From: Ann Example <ann@example.com>\r\n\
To: Team: bob@example.com, cy@example.com;\r\n\
Subject: Quarterly report\r\n\
Message-ID: <report@example.com>\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
See the attachment.\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--b--\r\n";

#[test]
fn a_multipart_message_yields_addresses_body_and_attachment_metadata() {
    let email = InboundEmail::from_raw("soporte", "INBOX", 7, RAW).expect("parse");
    assert_eq!(email.uid, 7);
    assert_eq!(email.account, "soporte");
    assert_eq!(email.message_id.as_deref(), Some("report@example.com"));
    assert_eq!(email.subject, "Quarterly report");
    assert_eq!(email.from, "Ann Example <ann@example.com>");
    assert_eq!(email.to, "bob@example.com, cy@example.com");
    assert_eq!(email.text, "See the attachment.");
    assert_eq!(email.attachments.len(), 1);
    assert_eq!(email.attachments[0].name, "report.pdf");
    assert_eq!(email.attachments[0].content_type, "application/pdf");
    assert_eq!(email.attachments[0].size, 9);
    assert_eq!(email.raw, RAW);
}

#[test]
fn a_message_without_a_header_block_is_malformed() {
    let error = InboundEmail::from_raw("soporte", "INBOX", 1, b"not a message")
        .expect_err("no header block");
    assert!(matches!(error, InboundError::Malformed(_)), "{error:?}");
}

#[test]
fn debug_never_prints_the_body_or_raw_source() {
    let email = InboundEmail::from_raw("soporte", "INBOX", 7, RAW).expect("parse");
    let rendered = format!("{email:?}");
    assert!(rendered.contains("Quarterly report"));
    assert!(!rendered.contains("See the attachment."), "{rendered}");
    assert!(!rendered.contains("JVBERi0xLjQK"), "{rendered}");
}

fn notification_meta() -> DoctypeMeta {
    DoctypeMeta {
        name: "Notification".to_string(),
        module: "Desk".to_string(),
        fields: vec![
            FieldMeta::data("user").label("User").required(),
            FieldMeta::data("notification_type")
                .label("Notification Type")
                .default_value("General"),
            FieldMeta::data("title").label("Title").required(),
            FieldMeta::text("description").label("Description"),
            FieldMeta::data("route").label("Route"),
            FieldMeta::check("is_read")
                .label("Is Read")
                .default_value("0"),
        ],
        hidden: true,
        allow_delete: true,
        sort_field: Some("creation".to_string()),
        sort_order: Some("desc".to_string()),
        ..Default::default()
    }
}

/// The `User` shape the notification resolves its recipient against. The real
/// `DocType` lives in `ferris-auth`; this test only needs `email` → `name`.
fn user_meta() -> DoctypeMeta {
    DoctypeMeta {
        name: "User".to_string(),
        module: "Core".to_string(),
        fields: vec![
            FieldMeta::data("email").label("Email").required(),
            FieldMeta::data("full_name").label("Full Name"),
        ],
        hidden: true,
        ..Default::default()
    }
}

/// Create one `User` and return its `name` (a UUID in production).
async fn seed_user(db: &ferris_db::Db, email: &str) -> String {
    register_doctype(user_meta());
    let engine = DocumentEngine::new(db.clone());
    let created = engine
        .create(
            &sys_context("User"),
            Value::from_json(serde_json::json!({ "email": email, "full_name": "Ana" })),
        )
        .await
        .expect("user");
    created
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

type PublishSink = Arc<Mutex<Vec<(String, Value)>>>;

/// A publisher installed once per test process; `set_publisher` is
/// first-call-wins, so tests share the sink and clear it before asserting.
fn published() -> &'static PublishSink {
    static SINK: OnceLock<PublishSink> = OnceLock::new();
    let sink = SINK.get_or_init(|| Arc::new(Mutex::new(Vec::new())));
    let forward = Arc::clone(sink);
    let _ = ferris_core::notifications::set_publisher(Arc::new(move |user, notification| {
        forward
            .lock()
            .expect("publish sink")
            .push((user.to_string(), notification));
    }));
    sink
}

fn sys_context(doctype: &str) -> EngineContext<'static> {
    let meta = ferris_schema::get_doctype(doctype);
    // The meta is leaked on purpose: this is a test context that only lives for
    // one assertion, and the engine borrows it for the duration of the call.
    let meta: &'static DoctypeMeta = Box::leak(Box::new(meta.expect("registered")));
    let mut ctx = EngineContext::new(doctype, "");
    ctx.meta = Some(meta);
    let mut user = ferris_core::User::system_user("test@ferris.local".into(), "Test".into());
    user.roles.push("System Manager".into());
    ctx.user = Some(user);
    ctx
}

async fn rows(db: &ferris_db::Db, doctype: &str) -> Vec<Value> {
    let engine = DocumentEngine::new(db.clone());
    let ctx = sys_context(doctype);
    engine
        .list(
            &ctx,
            &ListQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .await
        .expect("list")
        .0
}

#[tokio::test]
async fn the_engine_store_persists_the_document_notification_and_thread() {
    register_doctype(notification_meta());
    let db = ferris_db::memory_db().await.expect("db");
    let store = EngineInboundStore::new(db.clone());
    let processor = InboundProcessor::new(Arc::new(store));
    let sink = published();
    sink.lock().expect("sink").clear();
    // The notification recipient is a `User.name` (a UUID), resolved from the
    // account's address; writing the address itself was the bug (MAIL-03).
    let ana = seed_user(&db, "ana@example.com").await;

    let mut email = InboundEmail::from_raw("soporte", "INBOX", 42, RAW).expect("parse");
    email.notify_user = Some("ana@example.com".to_string());
    let processed = processor.process(&email).await.expect("process");
    assert!(!processed.duplicate);
    assert!(processed.notified, "the notification was raised");

    let stored = rows(&db, crate::doctypes::INBOUND_EMAIL).await;
    assert_eq!(stored.len(), 1);
    let doc = &stored[0];
    assert_eq!(
        doc.get("name").and_then(Value::as_str),
        Some(processed.name.as_str())
    );
    assert_eq!(
        doc.get("subject").and_then(Value::as_str),
        Some("Quarterly report")
    );
    assert_eq!(
        doc.get("message_id").and_then(Value::as_str),
        Some("report@example.com")
    );
    assert_eq!(doc.get("status").and_then(Value::as_str), Some("New"));
    assert_eq!(doc.get("imap_uid").and_then(Value::as_i64), Some(42));
    assert!(
        doc.get("raw")
            .and_then(Value::as_str)
            .is_some_and(|raw| raw.contains("JVBERi0xLjQK")),
        "raw source is archived"
    );

    let notifications = rows(&db, "Notification").await;
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0].get("user").and_then(Value::as_str),
        Some(ana.as_str())
    );
    assert_eq!(
        notifications[0].get("title").and_then(Value::as_str),
        Some("Quarterly report")
    );
    {
        let live = sink.lock().expect("sink");
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, ana);
    }

    // Same mailbox/UID again: idempotent, no duplicate document.
    let again = processor.process(&email).await.expect("process again");
    assert!(again.duplicate);
    assert_eq!(again.name, processed.name);
    assert_eq!(rows(&db, crate::doctypes::INBOUND_EMAIL).await.len(), 1);

    // A reply threads onto the stored Message-ID.
    let reply_raw = b"From: bob@example.com\r\nTo: ann@example.com\r\n\
Subject: Re: Quarterly report\r\nMessage-ID: <reply@example.com>\r\n\
In-Reply-To: <report@example.com>\r\n\r\nThanks!\r\n";
    let reply = InboundEmail::from_raw("soporte", "INBOX", 43, reply_raw).expect("parse");
    let threaded = processor.process(&reply).await.expect("reply");
    let stored = rows(&db, crate::doctypes::INBOUND_EMAIL).await;
    let doc = stored
        .iter()
        .find(|doc| doc.get("name").and_then(Value::as_str) == Some(threaded.name.as_str()))
        .expect("reply row");
    assert_eq!(
        doc.get("thread").and_then(Value::as_str),
        Some(processed.name.as_str())
    );
}

#[tokio::test]
async fn a_notification_for_an_unknown_address_is_not_faked() {
    register_doctype(notification_meta());
    let db = ferris_db::memory_db().await.expect("db");
    let store = EngineInboundStore::new(db.clone());
    let processor = InboundProcessor::new(Arc::new(store));
    // The `User` table exists (another account owns it), but nobody owns the
    // mailbox address, so there is no `User.name` to notify.
    let _other = seed_user(&db, "other@example.com").await;

    let mut email = InboundEmail::from_raw("soporte", "INBOX", 44, RAW).expect("parse");
    email.notify_user = Some("nobody@example.com".to_string());
    let processed = processor.process(&email).await.expect("stored");

    assert!(!processed.notified, "no user means no notification");
    assert_eq!(rows(&db, "Notification").await.len(), 0);
    assert_eq!(
        rows(&db, crate::doctypes::INBOUND_EMAIL).await.len(),
        1,
        "the mail itself is still durable"
    );
}

// ---------------------------------------------------------------------------
// Poller: a scripted IMAP server plus a store that fails for one UID.
// ---------------------------------------------------------------------------

struct MockImap {
    port: u16,
    received: Arc<Mutex<Vec<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl MockImap {
    fn start(steps: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recorded = received.clone();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            stream.write_all(b"* OK mock ready\r\n").expect("greeting");
            for (expected, reply) in steps {
                let line = read_line(&mut stream).expect("command");
                recorded.lock().expect("record").push(line.clone());
                assert!(line.contains(expected), "expected `{expected}` in `{line}`");
                let tag = line.split_whitespace().next().unwrap_or("A1");
                stream
                    .write_all(reply.replace("@TAG@", tag).as_bytes())
                    .expect("reply");
            }
            let mut sink = [0_u8; 64];
            while stream.read(&mut sink).is_ok_and(|n| n > 0) {}
        });
        Self {
            port,
            received,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Vec<String> {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("mock thread");
        }
        self.received.lock().expect("record").clone()
    }
}

fn read_line(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while stream.read(&mut byte).ok()? == 1 {
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len() - 2);
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    None
}

fn fetch_reply(uid: u32, body: &[u8]) -> String {
    // The literal's bytes sit between the marker's CRLF and the closing paren.
    let mut reply = format!("* 1 FETCH (UID {uid} RFC822 {{{}}}\r\n", body.len());
    reply.push_str(&String::from_utf8_lossy(body));
    reply.push_str(")\r\n@TAG@ OK FETCH\r\n");
    reply
}

struct FailingStore {
    fail_uid: u32,
    saved: Mutex<Vec<u32>>,
}

#[async_trait]
impl InboundStore for FailingStore {
    async fn existing(&self, _: &str, _: &str, _: u32) -> CoreResult<Option<String>> {
        Ok(None)
    }

    async fn find_thread(&self, _: &str) -> CoreResult<Option<String>> {
        Ok(None)
    }

    async fn save(&self, email: &InboundEmail, _: Option<&str>) -> CoreResult<String> {
        if email.uid == self.fail_uid {
            return Err(ferris_core::Error::Validation(ValidationError::Invalid(
                "simulated storage failure".to_string(),
            )));
        }
        self.saved.lock().expect("saved").push(email.uid);
        Ok(format!("INBOUND-{}", email.uid))
    }
}

#[tokio::test]
async fn a_message_that_fails_to_store_is_not_flagged_seen() {
    let body_one = b"From: a@example.com\r\nSubject: one\r\n\r\nfirst";
    let body_two = b"From: b@example.com\r\nSubject: two\r\n\r\nsecond";
    let mock = MockImap::start(vec![
        ("LOGIN", "@TAG@ OK\r\n".to_string()),
        ("SELECT", "@TAG@ OK\r\n".to_string()),
        (
            "UID SEARCH UNSEEN",
            "* SEARCH 1 2\r\n@TAG@ OK\r\n".to_string(),
        ),
        ("UID FETCH 1", fetch_reply(1, body_one)),
        ("UID FETCH 2", fetch_reply(2, body_two)),
        ("UID STORE 2", "@TAG@ OK\r\n".to_string()),
        ("LOGOUT", "* BYE\r\n@TAG@ OK\r\n".to_string()),
    ]);

    let store = Arc::new(FailingStore {
        fail_uid: 1,
        saved: Mutex::new(Vec::new()),
    });
    let processor = InboundProcessor::new(store.clone());
    let client = ImapClient::new(ImapConfig {
        host: "127.0.0.1".to_string(),
        port: mock.port,
        username: "ana".to_string(),
        password: "x".to_string(),
        use_tls: false,
        use_starttls: false,
        timeout_secs: 5,
        mailbox: String::new(),
        max_message_bytes: 4096,
        allow_plaintext: true,
    })
    .expect("client");
    let poller = ImapPoller::new("soporte", "ana@example.com", client).with_config(PollerConfig {
        interval: Duration::from_secs(60),
        retry_base: Duration::from_millis(10),
        max_backoff: Duration::from_secs(1),
        batch: 10,
    });

    let error = poller
        .poll_once(&processor)
        .await
        .expect_err("the storage failure surfaces for backoff");
    assert!(matches!(error, InboundError::Store(_)), "{error:?}");

    let commands = mock.finish();
    assert!(
        !commands.iter().any(|line| line.contains("UID STORE 1")),
        "a message that was not stored must not be flagged seen: {commands:?}"
    );
    assert!(
        commands.iter().any(|line| line.contains("UID STORE 2")),
        "the rest of the batch still progresses: {commands:?}"
    );
    assert_eq!(*store.saved.lock().expect("saved"), vec![2]);
}
