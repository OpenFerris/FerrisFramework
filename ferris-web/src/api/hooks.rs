//! Data fetching hooks for the Desk.
//!
//! These hooks encapsulate the platform-aware data access patterns, providing
//! a clean reactive API for views without exposing the DocStore implementation
//! details. They handle loading states, errors, and caching automatically.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use serde_json::Value;

mod list;

pub use list::{InfiniteListData, use_engagement, use_infinite_list_data, use_list_data};

/// Tracks which in-flight request is the current one.
///
/// Every fetch hook below starts a request per effect run, and the responses
/// can land in any order. Without a guard, a slow request for page 1 overwrites
/// the rows a fast request for page 2 already delivered, so the list shows the
/// wrong page. Each run claims a generation and only writes back if no newer
/// run claimed one in the meantime.
///
/// `pub(crate)` because `FormService` runs the same race: its load re-runs on
/// every parameter change, and two documents can be in flight at once.
#[derive(Clone, Default)]
pub(crate) struct Generation(Arc<AtomicU64>);

impl Generation {
    pub(crate) fn claim(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn is_current(&self, mine: u64) -> bool {
        self.0.load(Ordering::SeqCst) == mine
    }

    /// The generation a new non-owning request should be tagged with.
    ///
    /// Used by append-style fetches that must not invalidate a sibling append
    /// but must still be discarded when a reset claims a newer generation.
    pub(crate) fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Hook for fetching a single document by name.
pub fn use_document(
    doctype: ReadSignal<String>,
    name: ReadSignal<String>,
) -> (
    ReadSignal<Option<Value>>,
    ReadSignal<bool>,
    ReadSignal<Option<String>>,
) {
    let doc = RwSignal::new(None::<Value>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let generation = Generation::default();

    Effect::new(move |_| {
        let dt = doctype.get();
        let n = name.get();
        if dt.is_empty() || n.is_empty() || n == "new" {
            doc.set(None);
            loading.set(false);
            return;
        }

        let mine = generation.claim();
        let guard = generation.clone();
        loading.set(true);
        error.set(None);

        spawn_local(async move {
            let outcome = match DocStore::runtime() {
                Ok(store) => store.get(&dt, &n).await,
                Err(e) => Err(e),
            };
            if !guard.is_current(mine) {
                return;
            }
            match outcome {
                Ok(d) => {
                    doc.set(Some(d));
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    });

    (doc.read_only(), loading.read_only(), error.read_only())
}

/// Hook for performing mutations (create, update, delete, submit, cancel, amend).
///
/// Returns a function to execute the mutation and signals for the mutation state.
pub fn use_mutation() -> (
    Arc<dyn Fn(String, String, Option<Value>, MutationType) + Send + Sync>,
    ReadSignal<bool>,
    ReadSignal<Option<String>>,
    ReadSignal<Option<Value>>,
) {
    let mutating = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let result = RwSignal::new(None::<Value>);

    let execute = {
        let mutating = mutating;
        let error = error;
        let result = result;
        Arc::new(
            move |doctype: String, name: String, payload: Option<Value>, mut_type: MutationType| {
                mutating.set(true);
                error.set(None);
                result.set(None);

                let dt = doctype.clone();
                let n = name.clone();
                spawn_local(async move {
                    let store_result = DocStore::runtime();
                    let mut res: Result<Value, String> =
                        Err(crate::i18n::t("components.error.no_runtime"));

                    if let Ok(store) = store_result {
                        res = match mut_type {
                            MutationType::Create => {
                                store.create(&dt, payload.unwrap_or_default()).await
                            }
                            MutationType::Update => {
                                store.update(&dt, &n, payload.unwrap_or_default()).await
                            }
                            MutationType::SaveSingle => {
                                store.save_single(&dt, payload.unwrap_or_default()).await
                            }
                            MutationType::Delete => {
                                store.delete(&dt, &n).await.map(|_| Value::Null)
                            }
                            MutationType::Submit => store.submit(&dt, &n).await,
                            MutationType::Cancel => store.cancel(&dt, &n).await,
                            MutationType::Amend => store.amend(&dt, &n).await,
                        };
                    }

                    match res {
                        Ok(v) => {
                            result.set(Some(v));
                            mutating.set(false);
                        }
                        Err(e) => {
                            error.set(Some(e));
                            mutating.set(false);
                        }
                    }
                });
            },
        )
    };

    (
        execute,
        mutating.read_only(),
        error.read_only(),
        result.read_only(),
    )
}

#[derive(Debug, Clone, Copy)]
pub enum MutationType {
    Create,
    Update,
    /// Upsert the one document of a Single DocType (`PUT /api/single/{doctype}`).
    SaveSingle,
    Delete,
    Submit,
    Cancel,
    Amend,
}

/// View-specific state (filters, sort, columns) kept in localStorage.
///
/// The returned signal is seeded from storage and written back on every change.
/// Returning a separate save function instead meant the one caller discarded it,
/// so nothing was ever persisted; making the write automatic removes the way to
/// get that wrong.
///
/// `key` is a signal because the view outlives a single doctype: the router keeps
/// the component alive when only the route params change, so a frozen key would
/// write one doctype's view state into another doctype's entry.
pub fn use_persisted_view_state<
    T: serde::Serialize
        + for<'de> serde::Deserialize<'de>
        + Clone
        + Default
        + PartialEq
        + Send
        + Sync
        + 'static,
>(
    key: Signal<String>,
) -> RwSignal<T> {
    let state = RwSignal::new(T::default());

    #[cfg(target_arch = "wasm32")]
    {
        // Seed before returning: an effect that ran first would see the empty
        // default and overwrite the stored value before anything could read it.
        if let Some(storage) = web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .flatten()
        {
            if let Ok(Some(raw)) = storage.get_item(&key.get_untracked()) {
                if let Ok(parsed) = serde_json::from_str::<T>(&raw) {
                    state.set(parsed);
                }
            }
        }

        // Re-seed when the key changes. Created before the writer below so that
        // on a key change the new entry is loaded before anything can flush the
        // previous key's value into it — without this, a list that outlived a
        // doctype change kept writing the old doctype's state to the new slot.
        let state_read = state;
        Effect::new(move |_| {
            let key_now = key.get();
            let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            else {
                return;
            };
            let parsed = storage
                .get_item(&key_now)
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<T>(&raw).ok())
                .unwrap_or_default();
            if state_read.get_untracked() != parsed {
                state_read.set(parsed);
            }
        });

        let state_write = state;
        Effect::new(move |_| {
            let key_now = key.get();
            let current = state_write.get();
            // Nothing worth keeping: writing the default back would pin an empty
            // view state into storage on the very first visit.
            if current == Default::default() {
                return;
            }
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            {
                if let Ok(json) = serde_json::to_string(&current) {
                    let _ = storage.set_item(&key_now, &json);
                }
            }
        });
    }

    // Only the browser has a localStorage to read or write.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;

    state
}
