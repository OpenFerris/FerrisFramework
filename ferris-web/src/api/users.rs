//! Resolve user ids to the person the Desk shows, and to their document.
//!
//! Documents carry `owner`, `modified_by`, `assigned_to`, share users and
//! activity actors as **user ids** (a UUID or, for seeded users, an email). No
//! screen should print that id: the Desk resolves it to `full_name` (then
//! `email`) and keeps the answer in a bounded process-wide cache, because the
//! same few people appear in every row of every view.
//!
//! The cache also holds the small profile the sidebar header needs (name and
//! email), so one `GET /api/resource/User/{id}` serves every place the person is
//! named.
//!
//! # Cache scope, size and freshness (API-10)
//!
//! A bare `id → name` map that never expires is how one user's name and email
//! end up in another session's screen: the process serves every request, the
//! entries are never dropped, and a renamed or deleted person keeps the old
//! label forever. The cache here is therefore:
//!
//! * **scoped** — [`set_scope`] names the session/tenant the entries belong to
//!   and clears them when the scope changes (a different signed-in user, a
//!   different tenant on the same process);
//! * **bounded** — [`MAX_PROFILES`] entries, evicted a slice at a time, so a
//!   long-lived SSR process cannot grow it without limit;
//! * **expiring** — an entry older than [`PROFILE_TTL`] is re-read;
//! * **invalidatable** — [`forget`] drops one person (a profile update) and
//!   [`forget_all`] drops everything (a sign-out).
//!
//! Every name is a **link to the user's document**: the module that owns the
//! `User` DocType is resolved once (`ensure_module`) and reused to build
//! `/desk/:module/User/:id`. Until it is known, a name renders as text rather
//! than as a link to a guessed route.

use std::collections::HashMap;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_arch = "wasm32")]
use leptos::prelude::Callable;
use leptos::prelude::Callback;
#[cfg(target_arch = "wasm32")]
use leptos::task::spawn_local;
use serde_json::Value;

use super::DocStore;

/// The name and email of one user.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub label: String,
    pub email: String,
}

/// Cap on the profile cache. The Desk shows a handful of people per page; a
/// catalog larger than this is a long-lived process, and the cache is an
/// optimisation, not a store.
const MAX_PROFILES: usize = 512;

/// How long a resolved profile is trusted. A renamed user or a changed email
/// reaches the Desk within this window without a second invalidation path.
const PROFILE_TTL: Duration = Duration::from_secs(300);

/// One cached answer, with the identity it was filed under and when it was
/// read.
///
/// The id is stored next to the profile so a key mix-up (the bug this closes)
/// can never hand one person's name to another: a lookup whose stored id does
/// not match is a miss.
#[derive(Debug, Clone)]
struct Entry {
    id: String,
    profile: Profile,
    at: Instant,
}

impl Entry {
    fn fresh(&self, id: &str) -> bool {
        self.id == id && self.at.elapsed() < PROFILE_TTL
    }
}

/// The process-wide cache, scoped to the session/tenant [`set_scope`] named.
#[derive(Debug, Default)]
struct Cache {
    scope: String,
    entries: HashMap<String, Entry>,
}

static USER_PROFILES: OnceLock<Mutex<Cache>> = OnceLock::new();
static USER_MODULE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
#[cfg(target_arch = "wasm32")]
static MODULE_WAITERS: OnceLock<Mutex<Vec<Callback<String>>>> = OnceLock::new();
#[cfg(target_arch = "wasm32")]
static MODULE_LOADING: AtomicBool = AtomicBool::new(false);

fn profiles() -> &'static Mutex<Cache> {
    USER_PROFILES.get_or_init(|| Mutex::new(Cache::default()))
}

fn module_slot() -> &'static Mutex<Option<String>> {
    USER_MODULE.get_or_init(|| Mutex::new(None))
}

#[cfg(target_arch = "wasm32")]
fn waiters() -> &'static Mutex<Vec<Callback<String>>> {
    MODULE_WAITERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Name the session/tenant the cached profiles belong to.
///
/// Changing the scope drops every entry: a different signed-in user (or a
/// different tenant served by the same process) must not read names resolved
/// for the previous one. Calling it with the scope already in force is a no-op,
/// so a caller can set it on every request without flushing the cache.
pub fn set_scope(scope: &str) {
    let Ok(mut cache) = profiles().lock() else {
        return;
    };
    if cache.scope != scope {
        cache.scope = scope.to_string();
        cache.entries.clear();
    }
}

/// Drop one person's cached profile, for after that person is updated.
pub fn forget(id: &str) {
    if let Ok(mut cache) = profiles().lock() {
        cache.entries.remove(id.trim());
    }
}

/// Drop every cached profile, for a sign-out or a session change.
pub fn forget_all() {
    if let Ok(mut cache) = profiles().lock() {
        cache.entries.clear();
    }
}

/// The human label of one user document: full name, then email, then the name.
#[must_use]
pub fn label_for_doc(doc: &Value) -> String {
    for key in ["full_name", "display_name", "email", "username", "name"] {
        if let Some(text) = doc.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
        }
    }
    String::new()
}

/// The email of one user document, when it declares one.
#[must_use]
pub fn email_for_doc(doc: &Value) -> String {
    doc.get("email")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Fieldnames that hold a user id on a document.
///
/// A list cell for one of these is rendered with [`UserName`] instead of the
/// raw id: the engine stores `owner`/`modified_by`/`assigned_to` as the user's
/// document name, which for a seeded user is a UUID.
///
/// [`UserName`]: crate::components::views::user_identity::UserName
pub const USER_FIELDS: &[&str] = &[
    "owner",
    "modified_by",
    "created_by",
    "updated_by",
    "assigned_to",
    "commented_by",
    "comment_by",
    "raised_by",
    "reporter",
    "assignee",
    "user",
];

/// Whether a column holds a user id (see [`USER_FIELDS`]).
#[must_use]
pub fn is_user_field(fieldname: &str) -> bool {
    USER_FIELDS.contains(&fieldname)
}

/// First letters of a name, for an avatar fallback (`Administrator` → `A`).
#[must_use]
pub fn initials(label: &str) -> String {
    let stem = label.split('@').next().unwrap_or(label);
    let mut out = String::new();
    for part in stem.split(['.', '_', '-', ' ']) {
        if let Some(first) = part.chars().next() {
            out.extend(first.to_uppercase());
        }
        if out.chars().count() >= 2 {
            break;
        }
    }
    if out.is_empty() { "?".to_string() } else { out }
}

/// Store one `id → label` pair (no email).
pub fn remember(id: &str, label: &str) {
    remember_profile(id, label, "");
}

/// Store one `id → profile` pair.
///
/// The id is trimmed and stored next to the profile; an empty id or label is
/// not an answer, so it is not cached. When the cache is full the oldest-ish
/// tenth is dropped rather than the whole map, so one burst of new actors does
/// not throw away every name already on screen.
pub fn remember_profile(id: &str, label: &str, email: &str) {
    let id = id.trim();
    let label = label.trim();
    if id.is_empty() || label.is_empty() {
        return;
    }
    let Ok(mut cache) = profiles().lock() else {
        return;
    };
    if !cache.entries.contains_key(id) && cache.entries.len() >= MAX_PROFILES {
        let victims: Vec<String> = cache
            .entries
            .keys()
            .take(cache.entries.len() / 10 + 1)
            .cloned()
            .collect();
        for victim in victims {
            cache.entries.remove(&victim);
        }
    }
    cache.entries.insert(
        id.to_string(),
        Entry {
            id: id.to_string(),
            profile: Profile {
                label: label.to_string(),
                email: email.trim().to_string(),
            },
            at: Instant::now(),
        },
    );
}

/// The cached label, when the user has already been resolved.
#[must_use]
pub fn cached(id: &str) -> Option<String> {
    cached_profile(id).map(|profile| profile.label)
}

/// The cached profile, when the user has already been resolved and the entry is
/// still fresh.
///
/// A stale or mismatched entry is dropped on read, so the next [`profile`] call
/// re-reads the user instead of serving a name that no longer exists.
#[must_use]
pub fn cached_profile(id: &str) -> Option<Profile> {
    let id = id.trim();
    let mut cache = profiles().lock().ok()?;
    match cache.entries.get(id) {
        Some(entry) if entry.fresh(id) => Some(entry.profile.clone()),
        Some(_) => {
            cache.entries.remove(id);
            None
        }
        None => None,
    }
}

/// Resolve one user id, fetching it once and caching the answer.
///
/// Falls back to the id itself: a missing, deleted or unauthorised user must not
/// blank a provenance row.
pub async fn display(id: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    profile(id)
        .await
        .map_or_else(|| fallback_label(id, i18n), |profile| profile.label)
}

/// What to show when the user cannot be resolved.
///
/// A deleted or dangling id (an audit row whose user is gone) must not print a
/// UUID: an email or a readable name is kept, an opaque id becomes a neutral
/// label. The id stays in the tooltip, where it is useful and not in the way.
fn fallback_label(id: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    if id.contains('@') || !ferris_web_shared::meta::is_opaque_name(id) {
        id.to_string()
    } else {
        i18n.t("components.user.unknown")
    }
}

/// The profile of one user, fetching it once and caching the answer.
///
/// `None` only when the read fails and the id has never been cached; callers
/// that need a label fall back to [`display`].
pub async fn profile(id: &str) -> Option<Profile> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    if let Some(profile) = cached_profile(id) {
        return Some(profile);
    }
    let store = DocStore::runtime().ok()?;
    let doc = store.get("User", id).await.ok()?;
    let label = label_for_doc(&doc);
    // A document whose only readable field is its own opaque id has no name to
    // show: reporting it as unresolved lets `display` render the neutral label
    // instead of the UUID.
    if label.is_empty() || (label == id && ferris_web_shared::meta::is_opaque_name(id)) {
        return None;
    }
    let email = email_for_doc(&doc);
    remember_profile(id, &label, &email);
    cached_profile(id)
}

/// The module that owns the `User` DocType, when it has been resolved.
#[must_use]
pub fn module() -> Option<String> {
    module_slot().lock().ok()?.clone()
}

/// Ask for the module that owns `User`.
///
/// `ready` runs immediately when the answer is known, otherwise once it
/// arrives. The metadata is read at most once: every name on screen is a link,
/// so a per-name request would be a request per row.
#[cfg(target_arch = "wasm32")]
pub fn ensure_module(ready: Callback<String>) {
    if let Some(module) = module() {
        ready.run(module);
        return;
    }
    if let Ok(mut list) = waiters().lock() {
        list.push(ready);
    }
    if MODULE_LOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    spawn_local(async move {
        let resolved = match DocStore::runtime() {
            Ok(store) => store
                .meta("User")
                .await
                .ok()
                .flatten()
                .map(|meta| meta.module)
                .filter(|module| !module.trim().is_empty()),
            Err(_) => None,
        };
        if let Some(module) = resolved {
            if let Ok(mut slot) = module_slot().lock() {
                *slot = Some(module.clone());
            }
            let pending = waiters()
                .lock()
                .map(|mut list| std::mem::take(&mut *list))
                .unwrap_or_default();
            for ready in pending {
                ready.run(module.clone());
            }
        }
        MODULE_LOADING.store(false, Ordering::SeqCst);
    });
}

/// The server renders names as text: the link appears after hydration, when the
/// module can be fetched. Registering waiters in a process that never drains
/// them would only grow the vector.
#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_module(ready: Callback<String>) {
    let _ = ready;
}

/// The Desk URL of a user document, when its module is known.
///
/// `None` before the module resolves: a guessed route (`/desk/Users/User/…`)
/// would 404, and a link that does not go where it says is worse than a name.
#[must_use]
pub fn href(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let module = module()?;
    Some(format!(
        "/desk/{}/{}/{}",
        crate::desk::session::path_segment(&module),
        crate::desk::session::path_segment("User"),
        crate::desk::session::path_segment(id)
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PROFILES, cached, cached_profile, email_for_doc, forget, forget_all, initials,
        label_for_doc, profiles, remember_profile, set_scope,
    };

    /// The label prefers the visual name and never returns an empty string when
    /// the document has one of the known fields.
    #[test]
    fn a_user_label_prefers_the_full_name() {
        let doc = serde_json::json!({
            "name": "e025b6f7-d935-4ce9-a10f-27865f94909a",
            "full_name": "Administrator",
            "email": "admin@local"
        });
        assert_eq!(label_for_doc(&doc), "Administrator");
        assert_eq!(email_for_doc(&doc), "admin@local");
        let doc = serde_json::json!({ "name": "x", "email": "a@b.c" });
        assert_eq!(label_for_doc(&doc), "a@b.c");
        let doc = serde_json::json!({ "name": "only-id" });
        assert_eq!(label_for_doc(&doc), "only-id");
        assert_eq!(email_for_doc(&doc), "");
    }

    #[test]
    fn initials_come_from_the_name_not_the_id() {
        assert_eq!(initials("Administrator"), "A");
        assert_eq!(initials("Ana María Ruiz"), "AM");
        assert_eq!(initials("admin@local"), "A");
        assert_eq!(initials(""), "?");
    }

    /// API-10: two people never share an entry, and every invalidation path
    /// actually drops what it says it drops.
    ///
    /// The cache is process-wide, so the test starts and ends by clearing it;
    /// it is the only test that touches the profile map.
    #[test]
    fn profiles_are_isolated_and_invalidated() {
        forget_all();
        set_scope("test-tenant");

        remember_profile("ada@example.com", "Ada Lovelace", "ada@example.com");
        remember_profile("grace@example.com", "Grace Hopper", "grace@example.com");

        let ada = cached_profile("ada@example.com").expect("ada is cached");
        let grace = cached_profile("grace@example.com").expect("grace is cached");
        assert_eq!(ada.label, "Ada Lovelace");
        assert_eq!(ada.email, "ada@example.com");
        assert_eq!(grace.label, "Grace Hopper");
        assert_ne!(cached("ada@example.com"), cached("grace@example.com"));

        // A profile update forgets exactly that person.
        forget("ada@example.com");
        assert!(cached_profile("ada@example.com").is_none());
        assert!(cached_profile("grace@example.com").is_some());

        // A session/tenant change drops the whole cache.
        remember_profile("ada@example.com", "Ada Lovelace", "ada@example.com");
        set_scope("another-tenant");
        assert!(cached("ada@example.com").is_none());
        assert!(cached("grace@example.com").is_none());

        // Re-setting the same scope is not a flush.
        remember_profile("ada@example.com", "Ada", "");
        set_scope("another-tenant");
        assert!(cached("ada@example.com").is_some());

        forget_all();
        assert!(cached("ada@example.com").is_none());
    }

    /// The cache is bounded: a long-lived process cannot grow it without limit.
    #[test]
    fn the_profile_cache_stays_bounded() {
        forget_all();
        set_scope("bounded-test");
        for index in 0..MAX_PROFILES + 50 {
            remember_profile(&format!("user-{index}"), "Somebody", "");
        }
        let len = profiles().lock().expect("cache lock").entries.len();
        assert!(
            len <= MAX_PROFILES,
            "the cache grew to {len}, above the {MAX_PROFILES} cap"
        );
        assert!(len > 0, "eviction keeps the cache usable");
        forget_all();
    }
}
