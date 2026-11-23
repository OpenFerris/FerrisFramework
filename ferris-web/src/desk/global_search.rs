//! Global search overlay: Ctrl+K (or the sidebar/top-bar lenses) opens a
//! centered palette. Results come from `/api/search`; recently opened records
//! and starred favourites persist in `localStorage` (hydrate only — SSR renders
//! the empty palette so hydration stays in sync).
//!
//! The palette is one list with one keyboard cursor: the stored favourites and
//! recents while the query is empty, the server hits once it is not. Arrow keys
//! move the cursor, Enter opens, Escape closes — the footer says exactly that,
//! so the hint and the behaviour cannot drift apart. Each row is two sibling
//! buttons (open / star) instead of a button inside a button, which is invalid
//! markup and left the star unreachable by keyboard.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::drive;
use crate::app::{doctype_display, module_display};
use crate::components::ui::icon::Icon;
use crate::desk::session::{self as desk_session, DeskSearchHit};
use crate::i18n::{DeskI18n, use_i18n};
use icons::Search;

const RECENT_KEY: &str = "ferris.search.recents";
const FAVORITES_KEY: &str = "ferris.search.favorites";
/// How many palette rows the search requests.
const RESULT_LIMIT: usize = 20;

/// A saved palette entry (recent or favorite).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedEntry {
    pub label: String,
    pub route: String,
    pub doctype: String,
}

/// One row of the palette, flattened so the keyboard cursor is a single index.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    label: String,
    route: String,
    subtitle: String,
    favorite: bool,
    group: &'static str,
    saved: SavedEntry,
}

/// Palette overlay bound to shared signals from the Desk shell.
#[component]
pub fn GlobalSearch(open: RwSignal<bool>, query: RwSignal<String>) -> impl IntoView {
    // `StoredValue` keeps the translator `Copy`, so it can be captured by the
    // effect and by every closure in the palette.
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));
    let results = RwSignal::new(Vec::<DeskSearchHit>::new());
    let recents = RwSignal::new(Vec::<SavedEntry>::new());
    let favorites = RwSignal::new(Vec::<SavedEntry>::new());
    let (loading, set_loading) = signal::<bool>(false);
    // Why the last search failed. Without it an unreachable `/api/search` or a
    // disabled search feature renders as "Sin resultados.", which reads as a
    // successful empty answer instead of a failure.
    let search_error = RwSignal::new(None::<String>);
    // The index the keyboard cursor is on. Clamped on every read, so a list
    // that shrank under it (a favourite removed, a result set replaced) can
    // never point past the end.
    let active = RwSignal::new(0usize);
    // Guards against out-of-order answers: a slow request must not overwrite
    // the results of the one typed after it.
    let request_seq = RwSignal::new(0u64);
    let input_ref = NodeRef::<leptos::html::Input>::new();
    // Resolved once, in the component body: a hook called from inside an event
    // handler reads router context after the owner is gone.
    let navigate = leptos_router::hooks::use_navigate();
    // `Callback` (Copy) so the row closures below can be moved into a
    // re-running `view!` closure without consuming the router handle.
    let go = Callback::new(move |route: String| navigate(&route, Default::default()));

    // Debounced remote search when the query changes.
    Effect::new(move |_| {
        let q = query.get();
        if q.trim().is_empty() {
            results.set(Vec::new());
            search_error.set(None);
            set_loading.set(false);
            return;
        }
        set_loading.set(true);
        // A new query invalidates the previous answer: clear the list and the
        // failure so a slow or failed request can never leave the rows of an
        // older query on screen.
        results.set(Vec::new());
        search_error.set(None);
        let q = q.trim().to_string();
        request_seq.update(|seq| *seq += 1);
        let mine = request_seq.get_untracked();
        set_timeout(
            move || {
                drive(async move {
                    let found = desk_session::search(&q).await;
                    // A newer keystroke owns the palette now: drop this answer.
                    if request_seq.get_untracked() != mine {
                        return;
                    }
                    match found {
                        Ok(hits) => results.set(hits),
                        Err(_) => search_error.set(Some(tr("desk.search.unavailable"))),
                    }
                    set_loading.set(false);
                });
            },
            std::time::Duration::from_millis(200),
        );
    });

    // Restore local lists once after mount (client only; no-ops on SSR).
    Effect::new(move |_| {
        drive(async move {
            recents.set(load_list(RECENT_KEY));
            favorites.set(load_list(FAVORITES_KEY));
        });
    });

    // Focus the input when the palette opens.
    Effect::new(move |_| {
        if open.get() {
            // Reopening starts at the top of the list, not wherever the last
            // visit left the cursor.
            active.set(0);
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
                let _ = el.select();
            }
        }
    });

    // Escape closes the palette even when the focus is on a row's star button
    // (the input's own handler cannot see that key). The listener lives as long
    // as the component and does nothing while it is shut.
    let _escape = window_event_listener(leptos::ev::keydown, move |event| {
        if open.get() && event.key() == "Escape" {
            open.set(false);
        }
    });

    // The rows the palette shows, flattened. Grouping is only a heading on the
    // first row of each group; navigation stays a single index.
    let items = Signal::derive(move || {
        let mut out: Vec<Item> = Vec::new();
        let favs = favorites.get();
        if query.get().trim().is_empty() {
            for entry in &favs {
                out.push(Item {
                    label: entry.label.clone(),
                    route: entry.route.clone(),
                    subtitle: subtitle_of(entry),
                    favorite: true,
                    group: "desk.search.group.favorites",
                    saved: entry.clone(),
                });
            }
            for entry in recents.get() {
                out.push(Item {
                    label: entry.label.clone(),
                    route: entry.route.clone(),
                    subtitle: subtitle_of(&entry),
                    favorite: favs.iter().any(|f| f.route == entry.route),
                    group: "desk.search.group.recents",
                    saved: entry,
                });
            }
            return out;
        }
        for hit in results.get() {
            let label = if hit.title.is_empty() {
                hit.name.clone()
            } else {
                hit.title.clone()
            };
            out.push(Item {
                label: label.clone(),
                route: hit.route.clone(),
                subtitle: format!(
                    "{} · {}",
                    module_display(&hit.module),
                    doctype_display(&hit.doctype)
                ),
                favorite: favs.iter().any(|f| f.route == hit.route),
                group: "desk.search.group.results",
                saved: SavedEntry {
                    label,
                    route: hit.route,
                    doctype: hit.doctype,
                },
            });
        }
        out.truncate(RESULT_LIMIT);
        out
    });

    let close = move || open.set(false);

    // Open one row: close, remember it as a recent, and navigate.
    let open_item = move |item: &Item| {
        close();
        save_recent(RECENT_KEY, &item.saved);
        recents.set(load_list(RECENT_KEY));
        go.run(item.route.clone());
    };

    // Keyboard cursor over the flattened rows.
    let move_cursor = move |delta: isize| {
        let len = items.with(Vec::len);
        if len == 0 {
            return;
        }
        let current = active.get_untracked().min(len - 1);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta.unsigned_abs())
        }
        .min(len - 1);
        active.set(next);
    };

    let open_active = move || {
        let chosen = items.with(|list| {
            list.get(active.get_untracked().min(list.len().saturating_sub(1)))
                .cloned()
        });
        if let Some(item) = chosen {
            open_item(&item);
        }
    };

    view! {
        {move || {
            if !open.get() {
                return AnyView::from(Fragment::new(Vec::new()));
            }
            view! {
                <div class="fixed inset-0 z-100 flex items-start justify-center p-3 pt-[6vh] sm:pt-24">
                    <div
                        class="ferris-overlay absolute inset-0 backdrop-blur-sm"
                        aria-hidden="true"
                        on:click=move |_| close()
                    ></div>
                    <div
                        role="dialog"
                        aria-modal="true"
                        aria-label=tr("desk.search.dialog")
                        class="relative flex max-h-[85dvh] w-full max-w-xl flex-col overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-2xl"
                        on:click=move |ev| ev.stop_propagation()
                    >
                        <div class="flex shrink-0 items-center gap-3 border-b border-border px-4 py-3">
                            <Search class="size-5 shrink-0 text-muted-foreground" />
                            <input
                                node_ref=input_ref
                                type="text"
                                placeholder=tr("desk.search.placeholder")
                                aria-label=tr("desk.search.input_label")
                                aria-controls="global-search-results"
                                class="min-w-0 flex-1 bg-transparent text-base text-foreground outline-none placeholder:text-muted-foreground md:text-sm"
                                value=query
                                on:input=move |ev| {
                                    query.set(event_target_value(&ev));
                                    active.set(0);
                                }
                                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                    match ev.key().as_str() {
                                        "Escape" => close(),
                                        "ArrowDown" => {
                                            ev.prevent_default();
                                            move_cursor(1);
                                        }
                                        "ArrowUp" => {
                                            ev.prevent_default();
                                            move_cursor(-1);
                                        }
                                        "Home" => {
                                            ev.prevent_default();
                                            active.set(0);
                                        }
                                        "End" => {
                                            ev.prevent_default();
                                            let last = items.with(Vec::len).saturating_sub(1);
                                            active.set(last);
                                        }
                                        "Enter" => {
                                            ev.prevent_default();
                                            open_active();
                                        }
                                        _ => {}
                                    }
                                }
                            />
                            <span class="shrink-0 rounded border border-border px-1.5 py-0.5 text-xs text-muted-foreground">
                                "Esc"
                            </span>
                        </div>

                        <div
                            id="global-search-results"
                            role="region"
                            aria-label=tr("desk.search.results")
                            class="min-h-0 flex-1 overflow-y-auto overscroll-contain"
                        >
                            {move || {
                                if loading.get() && !query.get().trim().is_empty() {
                                    return view! {
                                        <p class="px-4 py-6 text-sm text-muted-foreground">
                                            {tr("desk.search.searching")}
                                        </p>
                                    }
                                    .into_any();
                                }
                                // A failed request is not an empty answer: say
                                // the search is unavailable instead of "Sin
                                // resultados.", which would claim the index ran
                                // and found nothing.
                                if let Some(message) = search_error.get() {
                                    return view! {
                                        <p class="px-4 py-6 text-sm text-destructive" role="alert">
                                            {message}
                                        </p>
                                    }
                                    .into_any();
                                }
                                if items.with(Vec::is_empty) {
                                    return if query.get().trim().is_empty() {
                                        view! { <EmptyHint /> }.into_any()
                                    } else {
                                        view! {
                                            <p class="px-4 py-6 text-sm text-muted-foreground">
                                                {tr("desk.search.no_results")}
                                            </p>
                                        }
                                        .into_any()
                                    };
                                }
                                AnyView::from(Fragment::new(Vec::new()))
                            }}
                            {move || {
                                let list = items.get();
                                let cursor = active.get();
                                let mut last_group: Option<&'static str> = None;
                                let mut rows: Vec<AnyView> = Vec::with_capacity(list.len());
                                for (index, item) in list.into_iter().enumerate() {
                                    let heading = if last_group == Some(item.group) {
                                        None
                                    } else {
                                        last_group = Some(item.group);
                                        Some(item.group)
                                    };
                                    let selected = index == cursor;
                                    let open_row = item.clone();
                                    let star = item.clone();
                                    let fav = item.favorite;
                                    rows.push(
                                        view! {
                                            {heading.map(|group| view! {
                                                <p class="px-4 pb-1 pt-3 text-xs font-medium text-muted-foreground">
                                                    {tr(group)}
                                                </p>
                                            })}
                                            <div
                                                class=if selected {
                                                    "flex items-stretch bg-muted"
                                                } else {
                                                    "flex items-stretch"
                                                }
                                                data-name="GlobalSearchRow"
                                            >
                                                <button
                                                    type="button"
                                                    class="flex min-w-0 flex-1 items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                                    on:click=move |_| open_row_item(&open_row, &open, &recents, go)
                                                >
                                                    <span class="min-w-0 flex-1 truncate text-sm text-foreground">
                                                        {item.label.clone()}
                                                    </span>
                                                    <span class="max-w-[45%] shrink-0 truncate text-xs text-muted-foreground">
                                                        {item.subtitle.clone()}
                                                    </span>
                                                </button>
                                                <button
                                                    type="button"
                                                    class="shrink-0 px-3 text-muted-foreground transition-colors hover:text-warning focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                                    aria-pressed=fav.to_string()
                                                    aria-label=if fav {
                                                        tr("desk.search.favorite_remove")
                                                    } else {
                                                        tr("desk.search.favorite_add")
                                                    }
                                                    on:click=move |_| {
                                                        favorites.set(toggle_list(
                                                            FAVORITES_KEY,
                                                            &star.saved,
                                                        ));
                                                    }
                                                >
                                                    <Icon
                                                        name="Star"
                                                        class=if fav {
                                                            "size-4 fill-current text-warning"
                                                        } else {
                                                            "size-4"
                                                        }
                                                    />
                                                </button>
                                            </div>
                                        }
                                        .into_any(),
                                    );
                                }
                                rows.into_iter().collect_view()
                            }}
                        </div>

                        <div class="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-4 py-2 text-xs text-muted-foreground">
                            <span>{tr("desk.search.hint_nav")}</span>
                            <span class="ml-auto hidden sm:inline">{tr("desk.search.hint_toggle")}</span>
                        </div>
                    </div>
                </div>
            }.into_any()
        }}
    }
}

/// What the palette says before anything is typed and nothing is stored yet: a
/// blank panel would read as a broken search rather than an empty history.
#[component]
fn EmptyHint() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <p class="px-4 py-6 text-sm text-muted-foreground">
            {i18n.t("desk.search.empty_hint")}
        </p>
    }
}

/// The subtitle of a stored entry: its doctype, or the route when it has none.
fn subtitle_of(entry: &SavedEntry) -> String {
    if entry.doctype.is_empty() {
        entry.route.clone()
    } else {
        entry.doctype.clone()
    }
}

/// Open one palette row from a click: close, remember it, navigate.
///
/// A free function so the per-row closure only captures `Copy` handles (the
/// item and the signals) and the re-running `view!` block stays `Fn`.
fn open_row_item(
    item: &Item,
    open: &RwSignal<bool>,
    recents: &RwSignal<Vec<SavedEntry>>,
    go: Callback<String>,
) {
    open.set(false);
    save_recent(RECENT_KEY, &item.saved);
    recents.set(load_list(RECENT_KEY));
    go.run(item.route.clone());
}

// --- localStorage helpers (wasm only; no-ops on SSR) ---

fn storage() -> Option<web_sys::Storage> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

fn load_list(key: &str) -> Vec<SavedEntry> {
    let Some(storage) = storage() else {
        return Vec::new();
    };
    let Ok(Some(text)) = storage.get_item(key) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_list(key: &str, entries: &[SavedEntry]) {
    if let Some(value) = serde_json::to_string(entries).ok()
        && let Some(storage) = storage()
    {
        let _ = storage.set_item(key, &value);
    }
}

fn save_recent(key: &str, entry: &SavedEntry) {
    let mut list = load_list(key);
    list.retain(|e| e.route != entry.route);
    list.insert(0, entry.clone());
    list.truncate(8);
    save_list(key, &list);
}

fn toggle_list(key: &str, entry: &SavedEntry) -> Vec<SavedEntry> {
    let mut list = load_list(key);
    if let Some(pos) = list.iter().position(|e| e.route == entry.route) {
        list.remove(pos);
    } else {
        list.push(entry.clone());
    }
    save_list(key, &list);
    list
}
