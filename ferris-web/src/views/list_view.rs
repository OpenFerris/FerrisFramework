//! Metadata-driven list view — wired to `ListService` (over `ViewCore`).
//!
//! The index route (`/desk/:module/:doctype`) is the list, except when the
//! doctype declares a `default_view`: the first visit in a session redirects
//! once to that view's own URL, so a board-first doctype opens on its board
//! while `…/list` keeps working as the explicit way back. The guard is
//! per-session and per-doctype: a redirect that ran on every mount would fight
//! the user who deliberately picked "Lista".

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_navigate, use_params_map};

use crate::WebUser;
use crate::api::Filter;
use crate::api::hooks::use_engagement;
use crate::api::meta_types::CollectionView;
use crate::components::desk::top_bar::{
    ButtonSpec, DropdownItem, DropdownSpec, SelectionMenuSpec, SelectionState, TopBarAction,
    use_top_bar_actions,
};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
};
use crate::components::views::common::{ListErrorBanner, ListLoadingSkeleton};
use crate::components::views::list::{
    ListColumnPicker, ListFilterGroup, ListOrderByGroup, ListQuickFilters, ListTable,
};
use crate::ext::{SLOT_LIST_BEFORE_TABLE, SLOT_LIST_TOOLBAR_BULK, UiSlot};
use crate::i18n::use_i18n;
use crate::views::FormView;
use crate::views::services::{ListService, view_core::ViewService};

/// Doctypes whose declared default view has already redirected this session.
///
/// A `Once` per mount is not enough (the view is rebuilt on navigation) and a
/// redirect on every mount would bounce the user out of an explicitly chosen
/// list, so the guard is a session-scoped set keyed `module/doctype`.
static DEFAULT_VIEW_REDIRECTS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[component]
pub fn ListView() -> impl IntoView {
    let params = use_params_map();
    let i18n = use_i18n();

    // Signals, not values: the router does not rebuild this component when only the
    // params change, so the service has to re-read them or
    // the list keeps showing the previous doctype under the new URL. `with_untracked`
    // here used to declare that non-reactivity on purpose, which is exactly the bug.
    let doctype = Signal::derive(move || params.with(|p| p.get("doctype").unwrap_or_default()));
    let module = Signal::derive(move || params.with(|p| p.get("module").unwrap_or_default()));

    let service = StoredValue::new(ListService::new(doctype, module));

    let user = expect_context::<RwSignal<WebUser>>();

    // Grab the registry while the owner is still live: `on_cleanup` runs after
    // the owner is torn down, and reading a context there has no owner to walk,
    // so it would panic. The signal handle itself stays usable.
    let top_actions = use_top_bar_actions();

    // use_navigate() -> Fn(&str, NavigateOptions) + Clone. The desk actions need
    // an owned path, so it is wrapped once in a Callback and shared.
    let navigate_fn = use_navigate();
    let navigate: Callback<(String, NavigateOptions)> =
        Callback::new(move |(url, opts): (String, NavigateOptions)| {
            navigate_fn(&url, opts);
        });

    // ── Top bar ─────────────────────────────────────────────────────────
    // One `Callback` per action, created HERE — once, in the component body.
    //
    // A `Callback` is a `StoredValue` owned by the owner current at creation,
    // and *both* an effect run and a derived signal dispose whatever their
    // previous run owned (`reactive_graph`'s `Owner::with_cleanup` drops the
    // run's arena values before each new run). Creating an action inside the
    // effect below disposed it on every state change while the bar still drew
    // the previous generation: clicking the visible button read a disposed
    // `StoredValue` and panicked the whole wasm module ("reactive value …
    // already been disposed" — the form's "Guardar" hit this, and so did
    // selecting a second row and pressing "Eliminar"). Owned by the view, they
    // die exactly when its `on_cleanup` empties the registry, so the slot holds
    // live callbacks by construction.
    let on_reload = {
        let svc = service.get_value();
        Callback::new(move |_| svc.reload())
    };
    let export_page = {
        let svc = service.get_value();
        Callback::new(move |_| svc.export_current_page())
    };
    let delete_selection = {
        let svc = service.get_value();
        Callback::new(move |_| svc.bulk_delete())
    };
    let export_selection = {
        let svc = service.get_value();
        Callback::new(move |_| {
            let cols = svc.visible_columns.get_untracked();
            svc.bulk_export(cols);
        })
    };
    let on_new = {
        let svc = service.get_value();
        Callback::new(move |_| {
            // Route segments are encoded: a doctype may be `Sales Invoice`.
            let url = format!(
                "/desk/{}/{}/new",
                crate::desk::session::path_segment(&svc.core.module.get()),
                crate::desk::session::path_segment(&svc.core.doctype.get())
            );
            navigate.run((url, NavigateOptions::default()));
        })
    };
    let i18n_top = i18n;
    Effect::new(move |_| {
        let svc = service.get_value();
        let Some(meta) = svc.meta().get() else {
            top_actions.set(Vec::new());
            return;
        };
        // A Single has no list affordances (view switcher, refresh of a list,
        // create/bulk). Its own form view paints the actions it needs.
        if meta.is_single {
            top_actions.set(Vec::new());
            return;
        }

        let mut out: Vec<TopBarAction> = Vec::new();

        // View switcher. Its per-view items are still built per run — the view
        // list arrives with the metadata — so a click on a stale item is
        // tolerated rather than trusted: the top bar runs actions with
        // `try_run` (a no-op once disposed), never `run`.
        if let Some(switcher) =
            TopBarAction::view_switcher(&svc.view_modes(), ListService::VIEW_ID, navigate)
        {
            out.push(switcher);
        }

        // Refresh
        out.push(TopBarAction::Button(ButtonSpec {
            label: String::new(),
            aria_label: Some(i18n_top.t("views.common.reload")),
            icon: Some("RefreshCw"),
            variant: ButtonVariant::Outline,
            disabled: false,
            on_click: on_reload,
        }));

        // Export actions that work on the loaded page; the selection export is
        // also available from the bulk menu when rows are checked.
        out.push(TopBarAction::Dropdown(DropdownSpec {
            label: String::new(),
            aria_label: Some(i18n_top.t("views.list.export")),
            icon: Some("Download"),
            items: vec![
                DropdownItem::action(i18n_top.t("views.list.export_page"), export_page)
                    .with_icon("FileDown"),
            ],
            selectable: false,
        }));

        // Create / bulk menu
        let roles = user.with(|u| u.roles.clone());
        if svc.can_create(&roles) {
            // A single signal collapsing has_selection + active_label + items.
            // The items carry the component-scoped callbacks, so the derive's
            // own re-runs cannot dispose them.
            let selection = Signal::derive({
                let svc = svc.clone();
                let i18n_selection = i18n_top;
                move || {
                    let selected = svc.selected().get();
                    if selected.is_empty() {
                        return None;
                    }

                    let label = if selected.len() == 1 {
                        i18n_selection.t("views.list.selection_one")
                    } else {
                        i18n_selection.t_with(
                            "views.list.selection_many",
                            &[("count", &selected.len().to_string())],
                        )
                    };
                    Some(SelectionState {
                        label,
                        items: vec![
                            DropdownItem::action(
                                i18n_selection.t("views.common.delete"),
                                delete_selection,
                            )
                            .with_icon("Trash2")
                            .destructive(),
                            DropdownItem::action(
                                i18n_selection.t("views.list.export_selection"),
                                export_selection,
                            )
                            .with_icon("Download"),
                        ],
                    })
                }
            });

            out.push(TopBarAction::SelectionMenu(SelectionMenuSpec {
                idle_label: i18n_top.t("views.common.new"),
                idle_icon: Some("Plus"),
                on_idle_click: on_new,
                selection,
            }));
        }

        top_actions.set(out);
    });

    on_cleanup(move || top_actions.set(Vec::new()));

    // ── Service handle ──────────────────────────────────────────────────
    let svc = service.get_value();

    // The doctype's declared default collection view. Once per session and
    // doctype, and never on the explicit `/list` URL: a user who picks "Lista"
    // means it.
    //
    // The effect owns its copy of the service (it outlives the component body),
    // which clippy reads as a clone nothing consumes.
    #[allow(clippy::redundant_clone)]
    {
        let navigate = navigate;
        let params = params;
        let svc = svc.clone();
        Effect::new(move |_| {
            let Some(meta) = svc.meta().get() else {
                return;
            };
            if meta.is_single {
                return;
            }
            let view = crate::api::meta::default_view(&meta);
            if view == CollectionView::List {
                return;
            }
            let explicit_list = params.with(|p| p.get("name").as_deref() == Some("list"));
            if explicit_list {
                return;
            }
            let doctype = svc.core.doctype.get();
            let key = format!("{}/{}", svc.core.module.get(), doctype);
            let first_visit = DEFAULT_VIEW_REDIRECTS
                .lock()
                .map(|mut seen| seen.insert(key))
                .unwrap_or(false);
            if !first_visit {
                return;
            }
            let module = if meta.module.is_empty() {
                svc.core.module.get()
            } else {
                meta.module.clone()
            };
            let url = crate::api::meta::collection_route(view, &module, &doctype);
            navigate.run((
                url,
                NavigateOptions {
                    replace: true,
                    ..NavigateOptions::default()
                },
            ));
        });
    }

    let query = svc.core.query;
    // Two filter owners, merged by `ViewQuery::to_list_params`: the advanced
    // dropdown writes `query.filters`, the quick rows write the service's own
    // signal. Keeping them apart is what makes "Aplicar filtros" replace only
    // the rows it owns.
    let filters = Signal::derive(move || query.get().filters);
    let quick_filters = svc.quick_filters.read_only();

    // Copy handles: the view closures never need the service itself.
    let loading = svc.loading();
    let rows = svc.rows();
    let total = svc.total();
    let error = svc.error();
    let selected = svc.selected();
    let meta = svc.meta();
    let visible_columns = svc.visible_columns;
    let sort_state = svc.sort_state;
    let loading_more = svc.core.loading_more;
    let has_more = svc.core.has_more;
    let load_more = svc.core.load_more;
    let doctype = svc.core.doctype;
    let module = svc.core.module;
    let only_liked = svc.only_liked.read_only();
    // El handle escribible se queda: `DialogContent::open` escribe en él cuando
    // llega el ESC o el clic en el fondo, así que el estado tiene un solo dueño.
    let bulk_confirm = svc.core.bulk_confirm;
    let bulk_progress = svc.core.bulk_progress.read_only();
    let pending_like = svc.pending_like.read_only();
    let engagement = svc.engagement.read_only();

    // Like and comment state for the rows currently loaded. Refetched by the
    // hook whenever the rows change, so paging, filtering and sorting all bring
    // fresh counts without this view tracking them. It writes the service's own
    // signal, which is the one the table reads and the like action updates.
    {
        use_engagement(move || doctype.get(), svc.rows(), svc.engagement);
    }

    // The quick rows are seeded by the doctype's standard filters. The advanced
    // dropdown gets every field: `list_view` only decides which columns the
    // table opens with, never which fields a filter may target.
    let quick_fields = {
        let svc = svc.clone();
        Signal::derive(move || svc.standard_filters.get())
    };
    let filter_fields = {
        let svc = svc.clone();
        Signal::derive(move || svc.filter_fields.get())
    };

    let order_fields = {
        let svc = svc.clone();
        Signal::derive(move || {
            svc.all_columns
                .get()
                .into_iter()
                .filter(|c| c.sortable)
                .collect::<Vec<_>>()
        })
    };

    let can_create = {
        let svc = svc.clone();
        Signal::derive(move || {
            let roles = user.with(|u| u.roles.clone());
            svc.can_create(&roles)
        })
    };

    // Whether an empty page means "no matches" rather than "no documents".
    // Both states keep the table and its header: the empty row says which one
    // it is and offers the way out (create, and clear the filters when there
    // are any).
    let filtered = Signal::derive(move || {
        !filters.get().is_empty() || !quick_filters.get().is_empty() || only_liked.get()
    });

    // The doctype as a reactive signal for the OpenUI slot mount.
    let doctype_slot = Signal::derive(move || doctype.get());

    // ── Callbacks ───────────────────────────────────────────────────────
    let on_quick_filters = {
        let svc = svc.clone();
        Callback::new(move |filters: Vec<Filter>| svc.apply_quick_filters(filters))
    };

    let on_toggle_like = {
        let svc = svc.clone();
        Callback::new(move |name: String| svc.toggle_like(&doctype.get(), &name))
    };

    let on_toggle_only_liked = {
        let svc = svc.clone();
        Callback::new(move |()| svc.toggle_only_liked())
    };

    let on_apply_filters = {
        let svc = svc.clone();
        Callback::new(move |filters: Vec<Filter>| svc.apply_filters(filters))
    };

    let on_reset_filters = {
        let svc = svc.clone();
        Callback::new(move |()| svc.clear_filtering())
    };

    let on_set_sort = {
        let svc = svc.clone();
        Callback::new(move |(field, desc): (String, bool)| svc.set_sort(field, desc))
    };

    let on_toggle_column = {
        let svc = svc.clone();
        Callback::new(move |(field, on): (String, bool)| svc.toggle_column(field, on))
    };

    let on_reset_columns = {
        let svc = svc.clone();
        Callback::new(move |()| svc.reset_columns())
    };

    let on_sort = {
        let svc = svc.clone();
        Callback::new(move |field: String| svc.sort_column(field))
    };

    let on_select_all = {
        let svc = svc.clone();
        Callback::new(move |on: bool| svc.toggle_all(on))
    };

    let on_select_row = {
        let svc = svc.clone();
        Callback::new(move |(name, on): (String, bool)| svc.toggle_row(name, on))
    };

    let on_create = {
        // Built when the button is pressed, not when the view was built: the URL
        // has to name the doctype the list is showing now.
        Callback::new(move |_| {
            let url = format!(
                "/desk/{}/{}/new",
                crate::desk::session::path_segment(&module.get()),
                crate::desk::session::path_segment(&doctype.get())
            );
            navigate.run((url, Default::default()))
        })
    };

    // Reading and writing fail differently, and the banner takes both.
    // `bulk_failure` is the write side: until the bulk delete was rebuilt
    // (hito 9) a refused bulk delete left no trace at all, because N calls
    // through `use_mutation` shared one error signal and erased each other
    //. `error` is the list hook's, the read side.
    // The write error wins because it is the consequence of the action the user
    // just took, and a read error would already have been on screen when it
    // happened.
    let banner_error = Signal::derive(move || svc.core.bulk_failure.get().or_else(|| error.get()));

    // "Eliminando 3 de 12…" mientras corre. Durante el borrado la selección sigue
    // intacta, así que el menú dice cuántas hay y no cuántas van.
    let i18n_bulk = i18n;
    let i18n_title = i18n;
    let bulk_status = Signal::derive(move || {
        let p = bulk_progress.get();
        p.running.then(|| {
            i18n_bulk.t_with(
                "views.list.deleting_progress",
                &[
                    ("done", &p.done.to_string()),
                    ("total", &p.total.to_string()),
                ],
            )
        })
    });
    let bulk_confirm_count = Signal::derive(move || selected.get().len());

    // Whether this doctype has a list at all. A Single has no list: its one
    // document is edited in place by the form view, so a deep link to
    // `/desk/:module/:doctype` lands on the right editor. It is a `Memo<bool>`
    // on purpose: reading `meta` directly in the view made every metadata load
    // re-render the block, tearing the whole list down and rebuilding it even
    // when the bit had not changed — which is what disposed the pagination's
    // prop signals out from under its effects.
    let is_single = Memo::new(move |_| meta.get().map(|m| m.is_single).unwrap_or(false));

    view! {
        {move || {
            if is_single.get() {
                return view! { <FormView /> }.into_any();
            }
            view! {
                <div class="mx-auto flex h-full min-h-0 w-full min-w-0 flex-col gap-4 p-4 [&>.ferris-ui-slot]:contents">
                    // The toolbar stacks on a phone: the quick-filter strip
                    // takes the full width and the advanced buttons wrap
                    // underneath it instead of forcing the whole page into a
                    // horizontal scroll.
                    <div class="flex min-w-0 flex-wrap items-center justify-between gap-2">
                        <ListQuickFilters
                            fields=quick_fields
                            filters=quick_filters
                            on_change=on_quick_filters
                        />
                        <div class="flex w-full flex-wrap items-center gap-2 sm:w-auto sm:shrink-0 sm:justify-end">
                            <ListFilterGroup
                                filters=filters
                                meta_fields=filter_fields
                                on_apply=on_apply_filters
                                on_clear=on_reset_filters
                            />
                            <ListOrderByGroup
                                sort_state=sort_state
                                fields=order_fields
                                on_set=on_set_sort
                            />
                            <ListColumnPicker
                                columns=svc.all_columns
                                visible=svc.visible_columns
                                on_toggle=on_toggle_column
                                on_reset=on_reset_columns
                            />
                            // OpenUI slot: plugins can add doctype-scoped bulk
                            // toolbar actions here.
                            <UiSlot
                                slot_name=SLOT_LIST_TOOLBAR_BULK
                                doctype=doctype_slot
                            />
                        </div>
                    </div>

                    <ListErrorBanner error=banner_error />

                    <Show when=move || bulk_status.get().is_some()>
                        <div
                            role="status"
                            aria-live="polite"
                            class="rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground"
                        >
                            {move || bulk_status.get().unwrap_or_default()}
                        </div>
                    </Show>

                    // El borrado masivo es irreversible y no tiene vuelta atrás, así que
                    // pide confirmación. Va controlado por señal, no por `DialogTrigger`:
                    // la fila vive dentro de un desplegable y ahí no hay sitio donde
                    // colgar el disparador (ver `DialogContent::open`).
                    // `contents` keeps the always-rendered dialog root out of
                    // the column's flex flow; otherwise its empty box adds a
                    // gap between the toolbar and the table.
                    <Dialog class="contents">
                        <DialogContent
                            open=bulk_confirm
                            class="max-w-md max-h-[85dvh] overflow-y-auto"
                            close_on_backdrop_click=false
                            data_name_prefix="AlertDialog"
                        >
                            <DialogHeader>
                                <DialogTitle>
                                    {{
                                        let i18n_title = i18n_title;
                                        move || {
                                            let count = bulk_confirm_count.get();
                                            let key = if count == 1 {
                                                "views.list.delete_title_one"
                                            } else {
                                                "views.list.delete_title_many"
                                            };
                                            i18n_title.t_with(key, &[("count", &count.to_string())])
                                        }
                                    }}
                                </DialogTitle>
                                <DialogDescription>
                                    {i18n.t("views.list.delete_warning")}
                                </DialogDescription>
                            </DialogHeader>
                            <DialogBody>
                                <p class="text-sm text-muted-foreground">
                                    {i18n.t("views.list.delete_partial")}
                                </p>
                            </DialogBody>
                            <DialogFooter>
                                // Botones propios y no `DialogClose`: el cierre lo
                                // gobiernan `bulk_cancel` y `bulk_delete_confirm`, que
                                // ademas tienen que hacer su parte del trabajo. Delegar el
                                // cierre en el script del dialogo dejaria el estado con dos
                                // dueños, que es el tipo de acuerdo que C3 no puede pagar.
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Default
                                    on:click=move |_| svc.core.bulk_cancel.run(())
                                >
                                    {i18n.t("common.cancel")}
                                </Button>
                                <Button
                                    variant=ButtonVariant::Destructive
                                    size=ButtonSize::Default
                                    on:click=move |_| svc.core.bulk_delete_confirm.run(())
                                >
                                    {i18n.t("views.common.delete")}
                                </Button>
                            </DialogFooter>
                        </DialogContent>
                    </Dialog>

                    <Show when=move || loading.get()>
                        <ListLoadingSkeleton row_count=6 />
                    </Show>

                    <Show when=move || !loading.get()>
                        // OpenUI slot (Fase 5): banners and toolbars that belong to
                        // the result set, between the list toolbar and the table.
                        <UiSlot slot_name=SLOT_LIST_BEFORE_TABLE doctype=doctype_slot />
                        // The result set is a surface like every card in the
                        // Desk: same radius, border and card colour, so the
                        // table does not float on the muted page background.
                        <div class="flex min-h-0 w-full min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border bg-card">
                            <ListTable
                                visible_columns=visible_columns
                                rows=rows
                                selected=selected
                                total=total
                                engagement=engagement
                                only_liked=only_liked
                                pending_like=pending_like
                                sort_state=sort_state
                                meta=meta
                                doctype=doctype
                                module=module
                                filtered=filtered
                                can_create=can_create
                                loading_more=loading_more
                                has_more=has_more
                                on_sort=on_sort
                                on_select_all=on_select_all
                                on_select_row=on_select_row
                                on_toggle_like=on_toggle_like
                                on_toggle_only_liked=on_toggle_only_liked
                                on_create=on_create
                                on_reset_filters=on_reset_filters
                                on_load_more=load_more
                            />
                        </div>
                    </Show>
                </div>
            }
            .into_any()
        }}
    }
}
