//! Regression tests for the handler surface.

use crate::{PresenceManager, RoomManager};
use std::sync::Arc;

use super::*;
use ferris_core::permissions::{Permission, PermissionEngine, Policy, PolicyStatement, Principal};

#[test]
fn test_handler_creation() {
    let room_manager = Arc::new(RoomManager::new());
    let presence_manager = Arc::new(PresenceManager::new());

    let handler = RealtimeHandler::new(room_manager, presence_manager);

    assert!(handler.room_manager.get("test").is_none());
}

/// Build an `AuthState` with inputs strong enough for structural tests
/// that never mint a token.
fn test_auth_state() -> ferris_auth::AuthState {
    ferris_auth::AuthState::new(ferris_auth::SessionConfig::default())
}

/// V3-RT-1: auth attached through one `RealtimeHandler` clone must be seen
/// by every other clone.
///
/// The server moves one clone into the Socket.IO namespace closure and
/// attaches auth later via `RealtimeServer::with_auth`. Before the fix,
/// `auth_state` was a by-value field, so the later write landed on the
/// server's copy while `authenticate` read the closure's — every socket
/// stayed anonymous. Sharing the cell through an `Arc` is what this pins.
#[test]
fn auth_attached_to_one_handler_clone_is_visible_to_another() {
    let handler = RealtimeHandler::new(
        Arc::new(RoomManager::new()),
        Arc::new(PresenceManager::new()),
    );
    // The clone that stands in for the one moved into the closure.
    let live = handler.clone();

    assert!(!handler.has_auth());
    live.set_auth(test_auth_state());
    assert!(
        handler.has_auth(),
        "auth set on the closure's clone must be visible to the server's copy"
    );

    let replaced = handler.clone().with_auth(test_auth_state());
    assert!(
        replaced.has_auth() && live.has_auth(),
        "with_auth shares the same cell as set_auth"
    );
}

#[test]
fn test_can_read_doctype_is_deny_by_default() {
    // Unknown doctype, no matching rule -> deny.
    assert!(!RealtimeHandler::can_read_doctype(
        "alice@example.com",
        &["User".to_string()],
        "Nonexistent DocType"
    ));
    // System Manager bypasses the rule lookup.
    assert!(RealtimeHandler::can_read_doctype(
        "alice@example.com",
        &["System Manager".to_string()],
        "Nonexistent DocType"
    ));
}

/// A grant that names no role at all must still open the room.
///
/// Frappe hands `ALL_USER_ROLE` to every signed-in user
/// (`frappe/permissions.py:568`), roles or not. Ferris maps it onto
/// [`Principal::everyone`], and a caller can hold **zero** concrete roles, so
/// the gate answers about an engine whose only matching statement says
/// `Everyone`. `read_scope` matches an `Everyone` pattern against any
/// principal (`ferris-core/src/permissions.rs:648-651`), so this must pass
/// even though the principal list holds only the pushed user id.
///
/// The grant here uses the *document* shape (`("Broadcast", "*")`), not the
/// DocType-instance shape (`("doctype", "Broadcast")`): the room question —
/// "may this caller receive every document of this DocType" — is a question
/// about documents, and `reaches_doctype` deliberately excludes
/// `resource_type == "doctype"` (`ferris-core/src/permissions.rs:296-309`).
///
/// Each of these built an engine of its own and asks
/// [`Self::can_read_doctype_with`] against it: the process-wide engine is
/// shared state and tests run in parallel, so no test here installs one.
#[test]
fn an_all_scoped_grant_reaches_a_caller_with_no_roles() {
    let mut engine = PermissionEngine::default();
    engine.add_policy(Policy::new(vec![PolicyStatement::allow(
        vec![Principal::everyone()],
        vec![Permission::Read],
        vec![ferris_core::permissions::Resource::new("Broadcast", "*")],
    )]));

    assert!(
        RealtimeHandler::can_read_doctype_with(&engine, "", &[], "Broadcast"),
        "an `All` grant must reach a caller holding no role record at all"
    );
    assert!(
        !RealtimeHandler::can_read_doctype_with(&engine, "", &[], "Something Else"),
        "…and only for the DocType it names"
    );
}

/// The room is DocType-wide: every member receives **every** document of the
/// DocType (`notify_doc_insert`/`update`/`delete` all target
/// `EventTarget::Doctype` and carry the full `data` — plus `previous_data`
/// on delete), so *subscription is the whole authorization*. There is no
/// per-row check on the emit path, which is why `can_read_doctype` asks for
/// a scope, not a decision: a grant that reaches only the rows the caller
/// owns is enough to read *those* rows over the REST path — `list()` and
/// `read_row` resolve `OWNER_CONDITION` per row — and must never open a room
/// that streams everybody's.
#[test]
fn an_if_owner_grant_cannot_join_the_doctype_room() {
    use ferris_core::permissions::{
        Decision, DocTypeCaps, OWNER_CONDITION, PermissionRule, doc_type_policy,
    };
    let rule = PermissionRule {
        role: "Sales".to_string(),
        read: true,
        if_owner: true,
        ..PermissionRule::default()
    };
    let mut engine = PermissionEngine::default();
    engine.add_policy(Policy::new(
        doc_type_policy("ToDo", &[rule], DocTypeCaps::default()).statements,
    ));

    assert!(
        !RealtimeHandler::can_read_doctype_with(
            &engine,
            "alice@example.com",
            &["Sales".to_string()],
            "ToDo"
        ),
        "an owner-scoped grant must not open a room that streams every document"
    );

    // The contrast: the same caller *can* read a document they own over the
    // REST path, where the row supplies the `is_owner` context. The room
    // must refuse what the row-by-row path may allow.
    let document = ferris_core::permissions::Resource::new("ToDo", "TODO-0001");
    let mut owned = std::collections::HashMap::new();
    owned.insert(OWNER_CONDITION.to_string(), ferris_core::Value::Bool(true));
    assert_eq!(
        engine.check_user(&["Sales".to_string()], Permission::Read, &document, &owned),
        Decision::Allow,
        "the owner does read their own row over REST"
    );
    assert_eq!(
        engine.check_user(
            &["Sales".to_string()],
            Permission::Read,
            &document,
            &Default::default(),
        ),
        Decision::NotApplicable,
        "without the context the statement matches nothing, so somebody else's row is denied"
    );
}

/// A grant naming one concrete document reaches exactly one row; the room
/// streams the whole DocType. `read_scope` classifies single-name grants as
/// [`ReadScope::PerRow`] and the gate refuses.
#[test]
fn a_name_scoped_grant_cannot_join_the_doctype_room() {
    use ferris_core::permissions::{Decision, Resource};
    let mut engine = PermissionEngine::default();
    engine.add_policy(Policy::new(vec![PolicyStatement::allow(
        vec![Principal::role("Sales")],
        vec![Permission::Read],
        vec![Resource::new("ToDo", "TODO-0001")],
    )]));

    assert!(
        !RealtimeHandler::can_read_doctype_with(
            &engine,
            "alice@example.com",
            &["Sales".to_string()],
            "ToDo"
        ),
        "a grant naming one document reaches one row; the room streams all of them"
    );
    assert_eq!(
        engine.check_user(
            &["Sales".to_string()],
            Permission::Read,
            &Resource::new("ToDo", "TODO-0001"),
            &Default::default(),
        ),
        Decision::Allow,
        "…while the same caller does read that one row over REST"
    );
}

/// The happy path: an *unconditional* DocType read grant is row-independent
/// (`ReadScope::All`), so the room opens for it.
#[test]
fn a_doctype_wide_grant_can_join_the_doctype_room() {
    use ferris_core::permissions::{DocTypeCaps, PermissionRule, doc_type_policy};
    let rule = PermissionRule {
        role: "Sales".to_string(),
        read: true,
        ..PermissionRule::default()
    };
    let mut engine = PermissionEngine::default();
    engine.add_policy(Policy::new(
        doc_type_policy("ToDo", &[rule], DocTypeCaps::default()).statements,
    ));

    assert!(
        RealtimeHandler::can_read_doctype_with(
            &engine,
            "alice@example.com",
            &["Sales".to_string()],
            "ToDo"
        ),
        "an unconditional DocType read grant is row-independent, so the room opens"
    );
}

/// A grant that names the caller by *user id*, not by role, must open the
/// room — `check_permission` reaches such grants over REST
/// (`ferris-auth/src/permissions.rs:60` pushes `Principal::user(..)`), and
/// the socket gate must answer about the same caller the REST path would
/// have been asked about.
///
/// This is the regression test behind threading `user_id` into the gate:
/// before that, `can_read_doctype` built principals from `roles` alone, so a
/// per-user grant matched nothing here — it was enforceable over REST and
/// silently absent over the socket.
#[test]
fn a_per_user_grant_can_join_the_doctype_room() {
    let mut engine = PermissionEngine::default();
    engine.add_policy(Policy::new(vec![PolicyStatement::allow(
        vec![Principal::user("alice@example.com")],
        vec![Permission::Read],
        vec![ferris_core::permissions::Resource::new("ToDo", "*")],
    )]));

    assert!(
        RealtimeHandler::can_read_doctype_with(&engine, "alice@example.com", &[], "ToDo"),
        "a grant naming the caller directly must reach the socket gate too"
    );
    assert!(
        !RealtimeHandler::can_read_doctype_with(&engine, "bob@example.com", &[], "ToDo"),
        "…and only the caller it names"
    );
}

#[test]
fn test_comments_room_is_public_and_notify() {
    let room_manager = Arc::new(RoomManager::new());
    // Comment streams are public to read.
    assert!(room_manager.can_join("comments:/about", "guest", |_| false));

    let handler = RealtimeHandler::new(room_manager, Arc::new(PresenceManager::new()));
    let data = crate::events::CommentEventData {
        route: "/about".to_string(),
        comment_id: "c1".to_string(),
        parent: None,
        author: "user@example.com".to_string(),
        body: "hi".to_string(),
    };
    assert!(handler.notify_comment("/about", data).is_ok());
}

#[test]
fn test_room_authz_denies_list_rooms_without_access() {
    let manager = RoomManager::new();
    // `doctype:`/`list:` rooms are deny-by-default.
    assert!(!manager.can_join("list:Todo:abc", "user1", |_| false));
    assert!(manager.can_join("list:Todo:abc", "user1", |dt| dt == "Todo"));
    // Unknown room kinds are rejected.
    assert!(!manager.can_join("random", "user1", |_| true));
}

// -- Handshake credentials --------------------------------------------------
//
// The socket upgrade is handled before the HTTP auth layers, so the handshake
// reads the identity from the one `tower-sessions` record by cookie id. These
// tests write that record directly and drive `authenticate_headers`, the pure
// half of the handshake.

use axum::http::{HeaderMap, HeaderValue, header};

async fn session_auth() -> (ferris_auth::AuthState, ferris_auth::login::DbSessionStore) {
    let pool = ferris_db::memory_db().await.expect("in-memory db");
    let store = ferris_auth::login::DbSessionStore::new(pool);
    store.ensure_table().await.expect("session table");
    let config = ferris_auth::SessionConfig::default();
    (
        ferris_auth::AuthState::new(config).with_session_store(store.clone()),
        store,
    )
}

/// A live session for `cookie@example.com` with the identity the login stamps.
async fn cookie_session(store: &ferris_auth::login::DbSessionStore) -> String {
    use tower_sessions::SessionStore;
    let mut record = tower_sessions::session::Record {
        id: Default::default(),
        data: std::collections::HashMap::new(),
        expiry_date: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
    };
    record.data.insert(
        ferris_auth::login::keys::USER_ID_KEY.to_string(),
        serde_json::json!("cookie@example.com"),
    );
    record.data.insert(
        ferris_auth::login::keys::EMAIL_KEY.to_string(),
        serde_json::json!("cookie@example.com"),
    );
    record.data.insert(
        ferris_auth::login::keys::ROLES_KEY.to_string(),
        serde_json::json!(["System Manager"]),
    );
    store.create(&mut record).await.expect("session");
    record.id.to_string()
}

fn cookie_header(auth: &ferris_auth::AuthState, session_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{}={session_id}", auth.config.cookie_name))
            .expect("cookie header"),
    );
    headers
}

#[tokio::test]
async fn a_session_cookie_authenticates_the_handshake() {
    let (auth, store) = session_auth().await;
    let session = cookie_session(&store).await;

    let identity = authenticate_headers(&auth, &cookie_header(&auth, &session))
        .await
        .expect("the session cookie must authenticate the socket");

    assert_eq!(identity.user_id, "cookie@example.com");
    assert_eq!(identity.email, "cookie@example.com");
    assert_eq!(identity.roles, vec!["System Manager".to_string()]);
    assert_eq!(identity.session_id, session);
}

#[tokio::test]
async fn an_unknown_session_cookie_stays_anonymous() {
    let (auth, _store) = session_auth().await;
    assert!(
        authenticate_headers(&auth, &cookie_header(&auth, "not-a-session"))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn an_expired_session_stays_anonymous() {
    use tower_sessions::SessionStore;
    let (auth, store) = session_auth().await;
    let mut record = tower_sessions::session::Record {
        id: Default::default(),
        data: std::collections::HashMap::new(),
        expiry_date: time::OffsetDateTime::now_utc() - time::Duration::minutes(1),
    };
    record.data.insert(
        ferris_auth::login::keys::USER_ID_KEY.to_string(),
        serde_json::json!("cookie@example.com"),
    );
    store.save(&record).await.expect("expired session");
    assert!(
        authenticate_headers(&auth, &cookie_header(&auth, &record.id.to_string()))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_handshake_without_credentials_stays_anonymous() {
    let (auth, _store) = session_auth().await;
    assert!(
        authenticate_headers(&auth, &HeaderMap::new())
            .await
            .is_none()
    );
    // A cookie jar without the session cookie is not a credential either.
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, HeaderValue::from_static("other=1"));
    assert!(authenticate_headers(&auth, &headers).await.is_none());
}

/// A disabled account is refused on the cookie path too — the socket must not
/// outlive the account.
#[tokio::test]
async fn a_disabled_user_is_rejected_on_the_cookie_path() {
    struct Disabled;
    #[async_trait::async_trait]
    impl ferris_auth::middleware::UserValidator for Disabled {
        async fn validate(&self, user_id: &str) -> Option<ferris_core::User> {
            let mut user =
                ferris_core::User::system_user(user_id.to_string(), "Disabled".to_string());
            user.id = user_id.to_string();
            user.enabled = false;
            Some(user)
        }
    }

    let (auth, store) = session_auth().await;
    let auth = auth.with_user_validator(std::sync::Arc::new(Disabled));
    let session = cookie_session(&store).await;
    assert!(
        authenticate_headers(&auth, &cookie_header(&auth, &session))
            .await
            .is_none(),
        "a disabled account must not hold a socket open"
    );
}
