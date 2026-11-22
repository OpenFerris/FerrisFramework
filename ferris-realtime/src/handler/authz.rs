//! The DocType-room authorization question.
//!
//! Kept apart from the connection lifecycle because the reasoning (a room
//! streams every row, so the question is `ReadScope::All`, not one decision)
//! is the part that must not drift from `check_permission`.

use super::RealtimeHandler;

impl RealtimeHandler {
    /// Whether this connection may subscribe to the DocType-wide room for
    /// `doctype`.
    ///
    /// # Why the question is not "may it read this DocType"
    ///
    /// The room is `doctype:<DocType>` (built in the `subscribe-doc` handler
    /// below) and every member of it receives **every** document of that
    /// DocType: `notify_doc_insert`, `notify_doc_update` and
    /// `notify_doc_delete` all target `EventTarget::Doctype(..)` and carry the
    /// document's `data` — and, on a delete, its `previous_data` too (see the
    /// `notify_doc_*` helpers further down). There is no per-document check on
    /// the emit path, so *subscription is the whole authorization*. A caller
    /// who may read only their own documents must
    /// therefore not be able to join, because once inside they would receive
    /// everybody's.
    ///
    /// That is exactly the [`ReadScope::All`] question, and it is why this asks
    /// the engine for a scope rather than for one decision: a single `check` on
    /// a synthetic resource cannot tell a row-independent grant from a
    /// row-dependent one. Answering the room's real question — *could this
    /// caller read every row of this DocType* — is what
    /// [`PermissionEngine::read_scope`] classifies, and it classifies it over the
    /// installed statements rather than by probing one resource, so a grant
    /// scoped to a single `name` cannot be mistaken for a DocType-wide one.
    ///
    /// # What changed, and why it was already safe
    ///
    /// This used to ask `check_user(.., Resource::new("doctype", dt), &{}))` and
    /// accept `Allow`. That reached the same verdict for `if_owner` — an empty
    /// context cannot satisfy the `is_owner` condition, so the statement did not
    /// match — but by accident: the correctness rested on `matches_context`
    /// treating a missing key as "no match", on a resource shape
    /// (`("doctype", <DocType>)`) that a document-shaped grant does not match,
    /// and on `Everyone` being appended by `check_principals`. Three unrelated
    /// properties had to hold at once, and none of them was load-bearing by
    /// design. Worse, the old call could not see a **per-user** grant at all:
    /// `Principal::user(..)` was never constructed, so a policy naming
    /// `ada@example.com` directly was enforceable over REST and silently absent
    /// here. `check_permission` pushes that principal
    /// (`ferris-auth/src/permissions.rs:60`), and this now does too.
    ///
    /// `System Manager` still bypasses the lookup, because
    /// `check_permission` bypasses it (`ferris-auth/src/permissions.rs:45`) and
    /// the two paths must agree on that too. It is safe under the room's own
    /// rule: `PermissionEngine::default()` installs
    /// `Resource::new("*", "*")` for it, so `read_scope` would answer `All` even
    /// if the short-circuit were removed.
    ///
    /// [`ReadScope::All`]: ferris_core::permissions::ReadScope::All
    /// [`PermissionEngine::read_scope`]: ferris_core::permissions::PermissionEngine::read_scope
    pub fn can_read_doctype(user_id: &str, roles: &[String], doctype: &str) -> bool {
        let engine = ferris_core::permissions::global_engine_ref();
        let engine = engine.read().unwrap_or_else(|e| e.into_inner());
        Self::can_read_doctype_with(&engine, user_id, roles, doctype)
    }

    /// The same question against a specific engine — the engine the realtime
    /// signal will act on is the process-wide one, but tests build their own so
    /// they never mutate global state a parallel test may be reading.
    pub(super) fn can_read_doctype_with(
        engine: &ferris_core::permissions::PermissionEngine,
        user_id: &str,
        roles: &[String],
        doctype: &str,
    ) -> bool {
        if roles.iter().any(|r| r == "System Manager") {
            return true;
        }
        use ferris_core::permissions::ReadScope;
        // The same construction the REST path uses, so the room gate cannot
        // drift from `check_permission` (§C12).
        let principals = ferris_core::permissions::principals_for(user_id, roles);
        // `owner` is the caller's own id. It only affects the `Owner`/`PerRow`
        // classifications, both of which are refused below — but passing it is
        // what makes the analysis the same analysis `list()` performs.
        engine.read_scope(&principals, doctype, user_id) == ReadScope::All
    }
}
