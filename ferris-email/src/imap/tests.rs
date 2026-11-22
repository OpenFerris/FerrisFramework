//! Scripted TCP tests for the IMAP client. No network access: the server is a
//! local listener that replays a fixed conversation and records the commands.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::protocol::{Token, literal_marker, parse_fetch, tokenize_into};
use super::{ImapClient, ImapConfig, ImapError};

struct Mock {
    port: u16,
    received: Arc<Mutex<Vec<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl Mock {
    /// One connection, `steps` as `(expected substring, reply)`. `@TAG@` in the
    /// reply is replaced by the tag of the command that triggered it.
    fn start(steps: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock imap");
        let port = listener.local_addr().expect("mock addr").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recorded = received.clone();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            stream
                .write_all(b"* OK [CAPABILITY IMAP4rev1] mock ready\r\n")
                .expect("greeting");
            for (expected, reply) in steps {
                let Some(line) = read_line(&mut stream) else {
                    panic!("client closed before `{expected}`");
                };
                recorded.lock().expect("record").push(line.clone());
                assert!(line.contains(expected), "expected `{expected}` in `{line}`");
                let tag = line.split_whitespace().next().unwrap_or("A1");
                let reply = reply.replace("@TAG@", tag);
                stream.write_all(reply.as_bytes()).expect("reply");
            }
            // Final steps already answered every command; let the client close.
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
            handle.join().expect("mock imap thread");
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

fn plain_config(port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".to_string(),
        port,
        username: "ana@example.com".to_string(),
        password: "s3cret".to_string(),
        use_tls: false,
        use_starttls: false,
        timeout_secs: 5,
        mailbox: String::new(),
        max_message_bytes: 1024,
        allow_plaintext: true,
    }
}

fn literal_reply(uid: u32, body: &[u8]) -> String {
    format!("* 1 FETCH (UID {uid} RFC822 {{{}}}\r\n", body.len())
        + &String::from_utf8_lossy(body)
        + ")\r\n@TAG@ OK FETCH completed\r\n"
}

/// The answer to `BODY.PEEK[]`: the server names the item `BODY[]`, as RFC 3501
/// requires, and does not touch `\Seen`.
fn peek_reply(uid: u32, body: &[u8]) -> String {
    format!("* 1 FETCH (UID {uid} BODY[] {{{}}}\r\n", body.len())
        + &String::from_utf8_lossy(body)
        + ")\r\n@TAG@ OK FETCH completed\r\n"
}

#[tokio::test]
async fn a_fetch_does_not_flag_the_message_seen_before_the_store() {
    let body = b"Subject: peek\r\nMessage-ID: <peek@example.com>\r\n\r\nbody";
    let mock = Mock::start(vec![
        ("LOGIN", "@TAG@ OK\r\n".to_string()),
        ("SELECT", "@TAG@ OK\r\n".to_string()),
        (
            "UID SEARCH UNSEEN",
            "* SEARCH 7\r\n@TAG@ OK\r\n".to_string(),
        ),
        ("UID FETCH 7", peek_reply(7, body)),
        ("UID STORE 7", "@TAG@ OK\r\n".to_string()),
        ("LOGOUT", "* BYE\r\n@TAG@ OK\r\n".to_string()),
    ]);
    let recorded = mock.received.clone();
    let client = ImapClient::new(plain_config(mock.port)).expect("client");
    let mut session = client.connect().await.expect("connect");
    assert_eq!(session.unseen_uids(10).await.expect("search"), vec![7]);

    let raw = session.fetch_raw(7).await.expect("fetch").expect("message");
    {
        let commands = recorded.lock().expect("record");
        let fetch = commands
            .iter()
            .find(|line| line.contains("UID FETCH 7"))
            .expect("the fetch was sent");
        assert!(
            fetch.contains("BODY.PEEK[]"),
            "the fetch must be a peek: {fetch}"
        );
        assert!(
            !fetch.to_ascii_uppercase().contains("\\SEEN"),
            "fetching must not set \\Seen: {fetch}"
        );
        assert!(
            !commands
                .iter()
                .any(|line| line.to_ascii_uppercase().contains("\\SEEN")),
            "nothing flags the message before the store: {commands:?}"
        );
    }
    assert_eq!(raw, body);

    // Only the explicit STORE, after the caller persisted the message, may flag
    // it.
    session.mark_seen(7).await.expect("flag");
    let flagged = recorded
        .lock()
        .expect("record")
        .iter()
        .filter(|line| line.to_ascii_uppercase().contains("\\SEEN"))
        .count();
    assert_eq!(flagged, 1, "a single STORE sets the flag");
    session.logout().await.expect("logout");
    mock.finish();
}

#[tokio::test]
async fn login_select_search_fetch_store_and_logout_follow_the_protocol() {
    let first = b"Subject: first\r\nMessage-ID: <one@example.com>\r\n\r\nhello";
    let second = b"Subject: second\r\n\r\nworld";
    let mock = Mock::start(vec![
        ("LOGIN", "@TAG@ OK logged in\r\n".to_string()),
        (
            "SELECT",
            "* 2 EXISTS\r\n@TAG@ OK [READ-WRITE] selected\r\n".to_string(),
        ),
        (
            "UID SEARCH UNSEEN",
            "* SEARCH 4 9\r\n@TAG@ OK search done\r\n".to_string(),
        ),
        ("UID FETCH 4", literal_reply(4, first)),
        ("UID STORE 4", "@TAG@ OK stored\r\n".to_string()),
        ("UID FETCH 9", literal_reply(9, second)),
        ("UID STORE 9", "@TAG@ OK stored\r\n".to_string()),
        ("LOGOUT", "* BYE\r\n@TAG@ OK bye\r\n".to_string()),
    ]);
    let client = ImapClient::new(plain_config(mock.port)).expect("client");
    let mut session = client.connect().await.expect("connect");
    assert_eq!(session.unseen_uids(10).await.expect("search"), vec![4, 9]);
    let first_raw = session
        .fetch_raw(4)
        .await
        .expect("fetch 4")
        .expect("message");
    session.mark_seen(4).await.expect("flag 4");
    let second_raw = session
        .fetch_raw(9)
        .await
        .expect("fetch 9")
        .expect("message");
    session.mark_seen(9).await.expect("flag 9");
    session.logout().await.expect("logout");
    let commands = mock.finish();

    assert_eq!(first_raw, first);
    assert_eq!(second_raw, second);

    assert!(
        commands[0].contains("LOGIN \"ana@example.com\" \"s3cret\""),
        "{}",
        commands[0]
    );
    assert!(
        commands
            .iter()
            .any(|line| line.contains("SELECT \"INBOX\""))
    );
    assert!(
        commands
            .iter()
            .any(|line| line.contains("UID SEARCH UNSEEN"))
    );
}

#[tokio::test]
async fn an_oversized_message_is_drained_and_reported() {
    let body = vec![b'x'; 64];
    let mock = Mock::start(vec![
        ("LOGIN", "@TAG@ OK\r\n".to_string()),
        ("SELECT", "@TAG@ OK\r\n".to_string()),
        (
            "UID SEARCH UNSEEN",
            "* SEARCH 3\r\n@TAG@ OK\r\n".to_string(),
        ),
        ("UID FETCH 3", literal_reply(3, &body)),
        // Proves the oversized literal was drained: the next command still
        // lands on a response boundary.
        ("LOGOUT", "* BYE\r\n@TAG@ OK\r\n".to_string()),
    ]);
    let config = ImapConfig {
        max_message_bytes: 8,
        ..plain_config(mock.port)
    };
    let client = ImapClient::new(config).expect("client");
    let error = client.fetch_unread().await.expect_err("too large");
    match error {
        ImapError::MessageTooLarge { uid, .. } => assert_eq!(uid, 3),
        other => panic!("expected MessageTooLarge, got {other:?}"),
    }
    let commands = mock.finish();
    assert!(commands.iter().any(|line| line.contains("LOGOUT")));
}

#[tokio::test]
async fn a_rejected_login_keeps_the_server_text() {
    let mock = Mock::start(vec![(
        "LOGIN",
        "@TAG@ NO [AUTHENTICATIONFAILED] invalid credentials\r\n".to_string(),
    )]);
    let client = ImapClient::new(plain_config(mock.port)).expect("client");
    let error = client.fetch_unread().await.expect_err("login rejected");
    match error {
        ImapError::Rejected {
            command,
            status,
            text,
        } => {
            assert_eq!(command, "LOGIN");
            assert_eq!(status, "NO");
            assert!(text.contains("AUTHENTICATIONFAILED"), "{text}");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
    mock.finish();
}

#[tokio::test]
async fn starttls_is_requested_before_login() {
    let mock = Mock::start(vec![("STARTTLS", "@TAG@ OK begin TLS\r\n".to_string())]);
    let config = ImapConfig {
        use_starttls: true,
        allow_plaintext: false,
        ..plain_config(mock.port)
    };
    let client = ImapClient::new(config).expect("client");
    // The mock closes the socket instead of completing a TLS handshake, so the
    // upgrade fails — but the STARTTLS command was sent first, which is what
    // this test asserts.
    let error = client.connect().await.expect_err("no TLS server");
    assert!(
        matches!(
            error,
            ImapError::Tls(_) | ImapError::Timeout(_) | ImapError::Io(_)
        ),
        "{error:?}"
    );
    let commands = mock.finish();
    assert_eq!(commands.len(), 1);
    assert!(commands[0].contains("STARTTLS"));
}

#[test]
fn configuration_requires_encryption_and_a_real_host() {
    let unencrypted = ImapConfig {
        username: "ana".to_string(),
        password: "x".to_string(),
        use_tls: false,
        use_starttls: false,
        ..ImapConfig::default()
    };
    assert!(matches!(
        ImapClient::new(unencrypted.clone()),
        Err(ImapError::Config(_))
    ));

    let plain = ImapConfig {
        allow_plaintext: true,
        ..unencrypted
    };
    assert!(ImapClient::new(plain.clone()).is_ok());

    let both = ImapConfig {
        use_tls: true,
        use_starttls: true,
        ..plain
    };
    assert!(matches!(ImapClient::new(both), Err(ImapError::Config(_))));

    let blank = ImapConfig {
        username: "ana".to_string(),
        password: "x".to_string(),
        host: String::new(),
        ..ImapConfig::default()
    };
    assert!(matches!(ImapClient::new(blank), Err(ImapError::Config(_))));
}

#[test]
fn a_literal_is_assembled_across_lines_and_kept_out_of_the_next_command() {
    // The parser half of the read loop: a FETCH whose body contains CRLF (the
    // normal case) must not be mistaken for the end of the response.
    let body = b"Subject: hi\r\n\r\nbody\r\nwith lines";
    let response = format!("* 1 FETCH (UID 5 RFC822 {{{}}}\r\n", body.len());
    let (start, length) = literal_marker(&response).expect("literal marker");
    let mut tokens = Vec::new();
    tokenize_into(&response[..start], &mut tokens);
    tokens.push(Token::Literal(body.to_vec()));
    tokenize_into(")", &mut tokens);
    let data = parse_fetch(&tokens).expect("parse").expect("fetch");
    assert_eq!(data.uid, Some(5));
    assert_eq!(data.message.as_deref(), Some(body.as_slice()));
    assert_eq!(length, u64::try_from(body.len()).expect("len"));
}
