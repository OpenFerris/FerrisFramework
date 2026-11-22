//! List and engagement data hooks.
//!
//! Split from `hooks.rs` so the generic document/mutation hooks and the two
//! list strategies stay readable. Both list hooks share `Generation` from the
//! parent module: it is what stops a slow page from overwriting a faster one.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use super::Generation;
use crate::api::{DocStore, EngagementPage, ListParams};

/// Hook for fetching paginated, filtered, sorted list data.
///
/// The three inputs are readers rather than `ReadSignal`s so callers can pass
/// an `RwSignal`, a `Memo` or a `Signal::derive` without wrapping them in a
/// throwaway signal just to call `.read_only()`. Refetching happens whenever
/// any of them changes. Returns the data, total count, and loading/error
/// states.
pub fn use_list_data(
    doctype: impl Fn() -> String + 'static,
    params: impl Fn() -> ListParams + 'static,
    reload_trigger: impl Fn() -> u64 + 'static,
) -> (
    ReadSignal<Vec<Value>>,
    ReadSignal<u64>,
    ReadSignal<bool>,
    ReadSignal<Option<String>>,
) {
    let rows = RwSignal::new(Vec::<Value>::new());
    let total = RwSignal::new(0u64);
    // Empieza en `true` para que SSR y la primera pasada de hidratación monten la
    // misma rama (skeleton / "Cargando…"). El efecto que la pasa a `false` puede
    // correr en el cliente antes de que termine la hidratación: si arrancase en
    // `false`, el cliente pintaría el skeleton en el hueco que el SSR reservó al
    // estado vacío/outro y la estructura saltaría (nodo de tipo inesperado y
    // panic en tachys al hidratar el `<script>` del diálogo siguiente).
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);
    let generation = Generation::default();

    Effect::new(move |_| {
        let dt = (doctype)();
        let _ = (reload_trigger)(); // Subscribe to reload trigger
        if dt.is_empty() {
            rows.set(Vec::new());
            total.set(0);
            loading.set(false);
            return;
        }

        let params = (params)();
        let mine = generation.claim();
        let guard = generation.clone();
        loading.set(true);
        error.set(None);

        spawn_local(async move {
            let outcome = match DocStore::runtime() {
                Ok(store) => store.list(&dt, &params).await,
                Err(e) => Err(e),
            };
            if !guard.is_current(mine) {
                return;
            }
            match outcome {
                Ok(resp) => {
                    rows.set(resp.data);
                    total.set(resp.total);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    });

    (
        rows.read_only(),
        total.read_only(),
        loading.read_only(),
        error.read_only(),
    )
}

/// Hook for an append-only list: the first page replaces the rows, every later
/// page is appended at the current length.
///
/// Kept separate from [`use_list_data`] so paged views keep their replace
/// semantics. The reset effect still watches the whole parameter set, which is
/// what makes a new filter, search term, sort or doctype start at offset zero;
/// [`InfiniteListData::load_more`] fetches the next page without touching the
/// query, so a growing list does not have to re-download the rows it already
/// has.
pub struct InfiniteListData {
    pub rows: ReadSignal<Vec<Value>>,
    pub total: ReadSignal<u64>,
    pub loading: ReadSignal<bool>,
    /// A next page is in flight.
    pub loading_more: Signal<bool>,
    /// There are still rows after the ones already loaded.
    pub has_more: Signal<bool>,
    pub error: ReadSignal<Option<String>>,
    /// Append the next page. Safe to call repeatedly: concurrent calls are
    /// ignored while one is already running.
    pub load_more: Callback<()>,
}

pub fn use_infinite_list_data(
    doctype: impl Fn() -> String + Clone + Send + Sync + 'static,
    params: impl Fn() -> ListParams + Clone + Send + Sync + 'static,
    reload_trigger: impl Fn() -> u64 + 'static,
) -> InfiniteListData {
    let rows = RwSignal::new(Vec::<Value>::new());
    let total = RwSignal::new(0u64);
    // `true` for the same SSR/hydration reason as `use_list_data`.
    let loading = RwSignal::new(true);
    let loading_more = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // The server may report a total larger than the rows it is willing to
    // return (or answer an offset with an empty page). Once that happens,
    // asking again would loop forever, so `has_more` also watches this flag.
    let end_reached = RwSignal::new(false);
    let generation = Generation::default();

    // Reset path: first page, or the parameters changed. Both are cloned for
    // the effect so the append callback keeps its own copy.
    {
        let params = params.clone();
        let doctype = doctype.clone();
        let generation = generation.clone();
        Effect::new(move |_| {
            let dt = (doctype)();
            let _ = (reload_trigger)();
            end_reached.set(false);
            loading_more.set(false);
            if dt.is_empty() {
                rows.set(Vec::new());
                total.set(0);
                loading.set(false);
                error.set(None);
                return;
            }

            let params = (params)();
            let mine = generation.claim();
            let guard = generation.clone();
            loading.set(true);
            error.set(None);

            spawn_local(async move {
                let outcome = match DocStore::runtime() {
                    Ok(store) => store.list(&dt, &params).await,
                    Err(e) => Err(e),
                };
                if !guard.is_current(mine) {
                    return;
                }
                match outcome {
                    Ok(resp) => {
                        end_reached
                            .set(resp.data.is_empty() || resp.data.len() as u64 >= resp.total);
                        rows.set(resp.data);
                        total.set(resp.total);
                        loading.set(false);
                    }
                    Err(e) => {
                        error.set(Some(e));
                        loading.set(false);
                    }
                }
            });
        });
    }

    let has_more = Signal::derive(move || {
        !end_reached.get() && rows.with(|rows| rows.len() as u64) < total.get()
    });

    let load_more = {
        let params = params.clone();
        let generation = generation.clone();
        Callback::new(move |_| {
            let dt = (doctype)();
            if dt.is_empty()
                || loading.get_untracked()
                || loading_more.get_untracked()
                || error.get_untracked().is_some()
                || !has_more.get_untracked()
            {
                return;
            }

            // The next page starts exactly where the loaded prefix ends. It is
            // the accumulated length, not a page counter, so a failed append
            // can simply be retried without skipping rows.
            let offset = rows.with_untracked(|rows| rows.len()) as u64;
            let mut params = (params)();
            params.limit_start = offset;

            let mine = generation.claim();
            let guard = generation.clone();
            loading_more.set(true);
            error.set(None);

            spawn_local(async move {
                let outcome = match DocStore::runtime() {
                    Ok(store) => store.list(&dt, &params).await,
                    Err(e) => Err(e),
                };
                if !guard.is_current(mine) {
                    return;
                }
                match outcome {
                    Ok(resp) => {
                        let empty = resp.data.is_empty();
                        let mut next = rows.get_untracked();
                        next.extend(resp.data);
                        let loaded = next.len() as u64;
                        rows.set(next);
                        total.set(resp.total);
                        end_reached.set(empty || loaded >= resp.total);
                        loading_more.set(false);
                    }
                    Err(e) => {
                        // Keep the rows already on screen: a failed next page
                        // must not blank a list the user can still read.
                        error.set(Some(e));
                        loading_more.set(false);
                    }
                }
            });
        })
    };

    InfiniteListData {
        rows: rows.read_only(),
        total: total.read_only(),
        loading: loading.read_only(),
        loading_more: loading_more.read_only().into(),
        has_more,
        error: error.read_only(),
        load_more,
    }
}

/// Hook for the like and comment state of a list page.
///
/// Kept separate from [`use_list_data`] because the two are fetched for
/// different reasons and have different failure modes: the rows must load for
/// the list to exist at all, while the engagement state is decoration on top
/// and must not blank the list when it fails. Refetches whenever `doctype` or
/// the loaded docnames change, which covers paging, filtering and sorting
/// because all of them change the rows.
///
/// `page` is written here and also by the like action, so it is passed in owned
/// by the caller: a signal this hook created would be a second, competing copy
/// of the state the table reads. Returns the read error, for the caller to
/// surface or ignore.
pub fn use_engagement(
    doctype: impl Fn() -> String + 'static,
    rows: ReadSignal<Vec<Value>>,
    page: RwSignal<EngagementPage>,
) -> ReadSignal<Option<String>> {
    let error = RwSignal::new(None::<String>);
    let generation = Generation::default();
    // The names the previous effect run asked for. An infinite list appends
    // rows, so requesting every loaded name on every page would make the
    // engagement payload grow quadratically. When the new row list merely
    // extends the previous one, only the suffix is fetched and merged.
    let previous_names = RwSignal::new(Vec::<String>::new());

    Effect::new(move |_| {
        let dt = (doctype)();
        if dt.is_empty() {
            return;
        }
        let names: Vec<String> = rows.with(|rows| {
            rows.iter()
                .filter_map(|row| row.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        });

        if names.is_empty() {
            page.set(EngagementPage::default());
            previous_names.set(Vec::new());
            error.set(None);
            return;
        }

        let previous = previous_names.with_untracked(|names| names.clone());
        let appended = names.len() > previous.len() && names.starts_with(previous.as_slice());
        let requested = if appended {
            names[previous.len()..].to_vec()
        } else {
            names.clone()
        };
        previous_names.set(names);

        let mine = if appended {
            // An append must not cancel another append that is still useful;
            // it only has to be discarded if a reset claims a newer generation.
            generation.current()
        } else {
            generation.claim()
        };
        let guard = generation.clone();
        spawn_local(async move {
            let outcome = match DocStore::runtime() {
                Ok(store) => store.engagement(&dt, &requested).await,
                Err(e) => Err(e),
            };
            if appended {
                if guard.current() != mine {
                    return;
                }
            } else if !guard.is_current(mine) {
                return;
            }
            match outcome {
                Ok(p) => {
                    if appended {
                        // Merge only the requested suffix. Replacing the whole
                        // page here would erase the state of every earlier row.
                        page.update(|current| {
                            for name in &requested {
                                if p.liked.contains(name) {
                                    current.liked.insert(name.clone());
                                } else {
                                    current.liked.remove(name);
                                }
                                current.like_counts.insert(
                                    name.clone(),
                                    p.like_counts.get(name).copied().unwrap_or(0),
                                );
                                current.comment_counts.insert(
                                    name.clone(),
                                    p.comment_counts.get(name).copied().unwrap_or(0),
                                );
                            }
                        });
                    } else {
                        page.set(p);
                    }
                    error.set(None);
                }
                // Keep whatever was already there: an empty table with an
                // error next to it beats a table of unliked hearts.
                Err(e) => error.set(Some(e)),
            }
        });
    });

    error.read_only()
}
