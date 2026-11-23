//! The widget catalog: the feed's definitions as a **two-level card
//! browser** (D3 of the dashboard widget), to add an instance to the
//! dashboard.
//!
//! Level one (the root shelf) shows one card per [`model::catalog_by_category`]
//! group — `WidgetDef::category`, `Otros` last — plus a leading
//! "Todos los widgets" card; each card is a header (title + count) over a
//! mosaic of up to four preview images from its widgets. Level two shows the
//! chosen group as a grid of widget cards — header (title + publisher badge)
//! over the preview image — and clicking one adds the instance and closes the
//! panel. The image policy (declared `preview` → kind fallback →
//! `generic.svg` on error) lives in [`cards`].
//!
//! It is its own side panel (not the shared `Sheet`/`Dialog`) for the same
//! two reasons as before: those primitives close with a `<script>` that only
//! runs in SSR, and the whole dashboard is client-side (its feed arrives via
//! `/api/ui-extensions`), so ESC and clicking the backdrop would not work; and
//! this panel is not a new primitive but the concrete composition of the
//! builder. It nevertheless carries `role="dialog"`/`aria-modal` and the real
//! close button.

mod cards;

use leptos::prelude::*;

use ferris_schema::contract::WidgetDef;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::ext::UiCtx;
use crate::i18n::use_i18n;

use super::model;
use cards::{card_title, preview_any, widget_card};

/// The catalogue key of the "every widget" card and of its level-two shelf.
const ALL: &str = "views.dashboard.all_widgets";

/// Where the browser is: the root shelf, one category's shelf, or every
/// widget flat. Reset to `Root` on every close, so the next open starts
/// where the user expects.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Browse {
    Root,
    Category(String),
    All,
}

/// The "Añadir widget" panel. It renders only when open.
#[component]
pub(super) fn WidgetPalette(
    /// Panel visibility.
    open: RwSignal<bool>,
    /// Add `(owner, def)` to the dashboard.
    on_add: Callback<(String, WidgetDef)>,
) -> impl IntoView {
    let apps = expect_context::<UiCtx>().apps;
    let i18n = use_i18n();
    let groups = Signal::derive(move || model::catalog_by_category(&apps.get()));
    let browse: RwSignal<Browse> = RwSignal::new(Browse::Root);
    // One close path for the backdrop, the X and a finished add: closing also
    // returns the browser to the root shelf. The captures are signals, so the
    // closure is `Copy` and every button can hold its own move-in copy.
    let close = move |_| {
        browse.set(Browse::Root);
        open.set(false);
    };
    // What a widget card runs: place the instance, then close the panel the
    // same way everything else does.
    let add_and_close = Callback::new(move |(owner, def): (String, WidgetDef)| {
        on_add.run((owner, def));
        browse.set(Browse::Root);
        open.set(false);
    });

    view! {
        {move || {
            if !open.get() {
                return ().into_any();
            }
            let groups = groups.get();
            let current = browse.get();
            let at_root = matches!(&current, Browse::Root);
            let heading = match &current {
                Browse::Root => i18n.t("views.dashboard.add_widget"),
                Browse::Category(category) if category == model::UNGROUPED => i18n.t(category),
                Browse::Category(category) => category.clone(),
                Browse::All => i18n.t(ALL),
            };
            let body = match current {
                Browse::Root if groups.is_empty() => view! {
                    <p class="text-xs text-muted-foreground">
                        {i18n.t("views.dashboard.catalog_empty")}
                    </p>
                }
                .into_any(),
                Browse::Root => root_shelf(&groups, browse, &i18n).into_any(),
                Browse::Category(category) => {
                    let entries = groups
                        .iter()
                        .find(|(name, _)| *name == category)
                        .map(|(_, entries)| entries.as_slice())
                        .unwrap_or(&[]);
                    if entries.is_empty() {
                        // The feed refreshed while this shelf was open and the
                        // category disappeared with it.
                        view! {
                            <p class="text-xs text-muted-foreground">
                                {i18n.t("views.dashboard.category_empty")}
                            </p>
                        }
                        .into_any()
                    } else {
                        widget_shelf(entries.to_vec(), add_and_close, &i18n).into_any()
                    }
                }
                Browse::All => {
                    let entries: Vec<(String, WidgetDef)> = groups
                        .iter()
                        .flat_map(|(_, entries)| entries.iter().cloned())
                        .collect();
                    widget_shelf(entries, add_and_close, &i18n).into_any()
                }
            };
            view! {
                <div class="fixed inset-0 z-50">
                    <button
                        type="button"
                        class="absolute inset-0 bg-foreground/40 animate-in fade-in duration-200 motion-reduce:animate-none"
                        on:click=close
                        aria-label=i18n.t("views.dashboard.close_palette")
                    ></button>
                    <aside
                        role="dialog"
                        aria-modal="true"
                        aria-label=i18n.t("views.dashboard.add_widget")
                        class="absolute inset-y-0 right-0 flex w-96 max-w-[92vw] flex-col border-l border-border bg-card text-card-foreground shadow-lg animate-in fade-in slide-in-from-right-4 duration-200 motion-reduce:animate-none"
                    >
                        <header class="flex items-center gap-2 border-b border-border px-4 py-3">
                            {(!at_root).then(|| view! {
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::IconSm
                                    on:click=move |_| browse.set(Browse::Root)
                                    attr:aria-label=i18n.t("views.dashboard.back_categories")
                                    attr:title=i18n.t("views.dashboard.back")
                                >
                                    <icons::ArrowLeft class="size-4" />
                                </Button>
                            })}
                            <h2 class="min-w-0 flex-1 truncate text-sm font-semibold">{heading}</h2>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::IconSm
                                on:click=close
                                attr:aria-label=i18n.t("views.dashboard.close_palette")
                            >
                                <icons::X class="size-4" />
                            </Button>
                        </header>
                        <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3">{body}</div>
                    </aside>
                </div>
            }
            .into_any()
        }}
    }
}

/// Level one: the "Todos" card first, then one card per category, each over
/// a mosaic of up to four previews from its own widgets.
fn root_shelf(
    groups: &[(String, Vec<(String, WidgetDef)>)],
    browse: RwSignal<Browse>,
    i18n: &(impl crate::i18n::Translate + Sync),
) -> impl IntoView {
    let total: usize = groups.iter().map(|(_, entries)| entries.len()).sum();
    let first_four: Vec<(String, WidgetDef)> = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().cloned())
        .take(4)
        .collect();
    view! {
        <ul class="flex flex-col gap-3">
            {shelf_card(
                i18n.t(ALL),
                total,
                first_four,
                browse,
                Browse::All,
                i18n,
            )}
            {groups
                .iter()
                .map(|(name, entries)| {
                    let mosaic: Vec<(String, WidgetDef)> =
                        entries.iter().take(4).cloned().collect();
                    let display = if name == model::UNGROUPED {
                        i18n.t(name)
                    } else {
                        name.clone()
                    };
                    shelf_card(
                        display,
                        entries.len(),
                        mosaic,
                        browse,
                        Browse::Category(name.clone()),
                        i18n,
                    )
                })
                .collect_view()}
        </ul>
    }
}

/// One root-shelf card: header (title + widget count) over a content mosaic
/// of preview images. The whole card navigates into its level-two shelf.
fn shelf_card(
    title: String,
    count: usize,
    defs: Vec<(String, WidgetDef)>,
    browse: RwSignal<Browse>,
    target: Browse,
    i18n: &(impl crate::i18n::Translate + Sync),
) -> impl IntoView {
    let previews: Vec<_> = defs
        .iter()
        .map(|(owner, def)| preview_any(&card_title(owner, def), def, i18n))
        .collect();
    // Two literal classes, never a runtime-built one (Tailwind purge): a
    // single widget takes the full row instead of half of an empty grid.
    let grid_class = if previews.len() <= 1 {
        "grid grid-cols-1 gap-1 bg-background p-2"
    } else {
        "grid grid-cols-2 gap-1 bg-background p-2"
    };
    view! {
        <li>
            <button
                type="button"
                data-name="__CategoryCard"
                class="flex w-full flex-col overflow-hidden rounded-lg border border-border bg-card text-left transition-colors hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                on:click=move |_| browse.set(target.clone())
            >
                <span class="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
                    <span class="min-w-0 truncate text-sm font-medium">{title}</span>
                    <span class="shrink-0 text-xs text-muted-foreground">{i18n.t_with("views.dashboard.widget_count", &[("count", &count.to_string())])}</span>
                </span>
                <span class=grid_class>{previews.into_iter().collect_view()}</span>
            </button>
        </li>
    }
}

/// Level two: the group's widgets as a grid of preview cards.
fn widget_shelf(
    entries: Vec<(String, WidgetDef)>,
    on_add: Callback<(String, WidgetDef)>,
    i18n: &(impl crate::i18n::Translate + Sync),
) -> impl IntoView {
    view! {
        <ul class="grid grid-cols-2 gap-3">
            {entries
                .into_iter()
                .map(|(owner, def)| widget_card(owner, def, on_add, i18n))
                .collect_view()}
        </ul>
    }
}
