//! The complete dashboard: loading of the effective layout, the shared top
//! bar and the wiring between the grid, the palette and the settings.
//!
//! `DashboardGrid` is what `dashboard_view` mounts (from [`crate::views::PluginPage`]
//! and [`crate::views::HomeView`]): it loads the effective layout for its
//! [`LayoutTarget`] — an app dashboard's user override or declared seed, or a
//! document-format dashboard's own `widgets` field — and draws the grid.
//! Whoever has permission (`Dashboard Layout`, or `Write` on the document)
//! also sees the builder: drag/resize the tiles, a palette to add widgets from
//! the global catalog and per-instance settings.
//!
//! The title, the "personalizado" badge and the builder's controls live in
//! the desk's shared top bar (the same slots the form and the list publish
//! into), so the board owns the whole page width below it.
//!
//! Saving is a `PUT` of the complete list; the server's response (already
//! normalized) is what is drawn, so what is seen is what will be reloaded.

// Only the widgets' heartbeat uses the clock, and it only runs in the browser.
#[cfg(target_arch = "wasm32")]
use std::time::Duration;

use leptos::prelude::*;

use ferris_schema::contract::{
    DashboardConfig, DashboardStyle, Placement, WidgetDef, WidgetInstance,
};

use crate::api::dashboard_layout::{self as layout_api, LayoutTarget};
use crate::components::desk::top_bar::{
    ButtonSpec, FullScreenToggleSpec, TopBarAction, TopBarChromeSpec, TopBarStatus,
    use_top_bar_actions, use_top_bar_chrome, use_top_bar_status, use_top_bar_title,
};
use crate::components::ui::badge::BadgeVariant;
use crate::components::ui::button::ButtonVariant;
use crate::ext::UiCtx;

use super::canvas::DashboardBoard;
use super::card::ActiveDrag;
use super::config::{BoardConfig, BoardScope};
use super::drawer::WidgetPalette;
use super::props::WidgetProps;
use super::{model, style};

/// A complete dashboard of an app: the `grid-cols-12` board with the
/// effective instances, plus the palette and the settings dialog the builder
/// opens. The title and the edit controls are the shared top bar's job (see
/// the module docs).
#[component]
pub fn DashboardGrid(
    /// App that declared the dashboard; a loose `name` reference is resolved
    /// in its catalog.
    #[prop(into)]
    owner: String,
    /// Id of the dashboard within that app (key together with `owner`).
    #[prop(into)]
    dashboard_id: String,
    /// Declared label of the dashboard.
    #[prop(into)]
    label: String,
    /// Instances seeded by the app: the state until the override loads.
    seed: Vec<WidgetInstance>,
    /// `(doctype, name)` of the document that *is* this dashboard (the
    /// document-format binding): the layout is read from and written to the
    /// doctype document itself, and `owner`/`dashboard_id` then only feed the
    /// catalog and the header. `None` for every other dashboard.
    #[prop(default = None)]
    document: Option<(String, String)>,
    /// The board style the app — or the native `Dashboard` document — declares.
    /// The saved override layers over it; `None` means it declares none.
    #[prop(default = None)]
    declared_style: Option<DashboardStyle>,
) -> impl IntoView {
    let apps = expect_context::<UiCtx>().apps;

    // Where the effective layout lives: the document itself when this is a
    // document-format dashboard, else the app's `(owner, id)` pair.
    let target = document.map_or_else(
        || LayoutTarget::App {
            owner: owner.clone(),
            id: dashboard_id.clone(),
        },
        |(doctype, name)| LayoutTarget::Document { doctype, name },
    );

    // The effective grid. `seed` renders until the response arrives — and it
    // is also what a failed load leaves on screen — so it is normalized here,
    // once, like every other list this component draws: a declared seed
    // carries references and usually no cells, and an unnormalized list draws
    // every widget at `(0, 0)`, piled on the same corner. `snapshot` is that
    // normalized seed (the state on entering edit mode, so that "Cancelar"
    // restores it and not the raw declaration).
    let instances = RwSignal::new({
        let mut seed = seed;
        model::normalize(&apps.get_untracked(), &owner, &mut seed);
        seed
    });
    let snapshot = RwSignal::new(instances.get_untracked());
    // The board's shared configuration (style/geometry override), as the server
    // answers it: absent until the load lands, and `snapshot_config` is what
    // "Cancelar" restores.
    let config = RwSignal::new(DashboardConfig::default());
    let snapshot_config = RwSignal::new(DashboardConfig::default());
    // The configuration drawer and the draft it edits. The draft is separate
    // from `config` so the panel previews live (the board reads the draft while
    // it is open) and "Cancelar" is a plain close: the saved value is never
    // touched until "Aplicar".
    let config_open = RwSignal::new(false);
    let style_draft = RwSignal::new(DashboardStyle::default());
    // The Desk's global board style (one row shared by the install) and whether
    // this user may edit it. The built-in default renders until the load lands,
    // which is exactly how boards looked before the global existed.
    let global_style = RwSignal::new(DashboardStyle::default());
    let global_editable = RwSignal::new(false);
    // The configuration drawer's scope and the global draft it edits when the
    // user switches to it. Both reset when the drawer opens or closes, so the
    // board never previews a global that is not being edited.
    let config_scope = RwSignal::new(BoardScope::Board);
    let global_draft = RwSignal::new(DashboardStyle::default());
    // The resolved appearance every consumer reads: the saved override over the
    // app's declared style over the Desk's global style — or, while the
    // configuration drawer is open, the draft of whichever scope it is editing
    // (the board's override, or the global base under it). One signal, so the
    // gap, the board frame and every tile cannot disagree.
    let board = Signal::derive(move || {
        let preview = config_open.get();
        let scope = config_scope.get();
        let style = if preview && scope == BoardScope::Board {
            style_draft.get()
        } else {
            config.get().style
        };
        let base = if preview && scope == BoardScope::Global {
            global_draft.get()
        } else {
            global_style.get()
        };
        style::board_style(
            &base,
            declared_style.as_ref(),
            &DashboardConfig {
                dashboard_id: None,
                style,
            },
        )
    });
    let customized = RwSignal::new(false);
    let editable = RwSignal::new(false);
    let editing = RwSignal::new(false);
    // Full screen is per dashboard and lives with the view: leaving the page
    // drops it with the component, and the shell reads the published chrome to
    // hide the sidebar. It is not persisted — a full-screen board is a mode of
    // this visit, not part of the board's style.
    let fullscreen = RwSignal::new(false);
    let toggle_fullscreen = Callback::new(move |_: ()| fullscreen.update(|on| *on = !*on));
    // Closing the configuration drawer always returns it to the board scope:
    // the next open must not start on "Global del Desk".
    let close_config = move || {
        config_open.set(false);
        config_scope.set(BoardScope::Board);
    };
    let palette_open = RwSignal::new(false);
    let selected = RwSignal::new(None::<String>);
    let preview = RwSignal::new(None::<(String, Placement)>);
    let dragging = RwSignal::new(None::<ActiveDrag>);
    let notice = RwSignal::new(None::<String>);
    // Tick that native widgets follow to reload; it ticks only in read mode,
    // so as not to repaint data mid-edit.
    let refresh = RwSignal::new(0_u32);

    // One load per mount (client): the seed renders until the
    // response arrives. The [`LayoutTarget`] decides the endpoint — the app
    // pair or the document's own `widgets` field — and the rest is the same.
    //
    // The response is drawn in *both* cases, not only when an override
    // exists: the server answers the effective list either way (the saved
    // override, or the seed the dashboard is declared with), so it is what
    // the user will reload — and it is the only source that reaches a
    // dashboard whose seed the caller did not have. That is the case of a
    // `Dashboard` document opened by name: its widgets live in the database,
    // and a document the UI feed dropped (an unusable or duplicate route)
    // has no declared seed to start from.
    //
    // Browser-only, like the feed that decides whether this page has a
    // dashboard at all: the layout rides the HTTP client, while the SSR pass
    // would have to talk to itself. The server therefore renders the seed and
    // the client loads the effective layout — the same division every other
    // client-only fetch in the Desk keeps.
    let load_target = target.clone();
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            let target = load_target.clone();
            crate::api::drive(async move {
                // The reply can outlive this component (a route change or a
                // parent re-render disposes its signals). `try_set` makes the
                // late write a no-op instead of a panic on a disposed signal.
                if let Ok(layout) = layout_api::dashboard_layout(&target).await {
                    if editable.try_set(layout.editable).is_none() {
                        return;
                    }
                    let _ = customized.try_set(layout.customized);
                    let _ = snapshot.try_set(layout.widgets.clone());
                    let _ = instances.try_set(layout.widgets);
                    let _ = snapshot_config.try_set(layout.config.clone());
                    let _ = config.try_set(layout.config);
                }
                // The global base is a separate row: a failure keeps the
                // built-in default, so the board still renders.
                if let Ok(settings) = layout_api::dashboard_settings().await {
                    if global_editable.try_set(settings.editable).is_none() {
                        return;
                    }
                    // A drawer opened before the answer arrived seeded its
                    // global draft from the built-in default; refresh it unless
                    // the user is already editing that scope.
                    if config_open.try_get_untracked() == Some(false) {
                        let _ = global_draft.try_set(settings.style.clone());
                    }
                    let _ = global_style.try_set(settings.style);
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = &load_target;
    });

    // Heartbeat of the native widgets, outside edit mode. The interval does not
    // own the reactive tree: `get_untracked` avoids subscribing to `editing`.
    #[cfg(target_arch = "wasm32")]
    {
        let tick = move || {
            // The interval can outlive the component by one queued tick after
            // `on_cleanup` clears it; a disposed signal must not panic here.
            if editing.try_get_untracked() == Some(false) {
                let _ = refresh.try_update(|tick| *tick = tick.wrapping_add(1));
            }
        };
        if let Ok(handle) = set_interval_with_handle(tick, Duration::from_secs(15)) {
            on_cleanup(move || handle.clear());
        }
    }

    let owner_key = owner.clone();
    let owner_edit = owner_key.clone();

    let start_edit = Callback::new(move |_: ()| {
        let catalog = apps.get_untracked();
        instances.update(|list| model::normalize(&catalog, &owner_edit, list));
        snapshot.set(instances.get_untracked());
        snapshot_config.set(config.get_untracked());
        editing.set(true);
        notice.set(None);
    });

    let cancel_edit = Callback::new(move |_: ()| {
        instances.set(snapshot.get_untracked());
        config.set(snapshot_config.get_untracked());
        close_config();
        editing.set(false);
        palette_open.set(false);
        selected.set(None);
        preview.set(None);
        notice.set(None);
    });

    let save = {
        let target = target.clone();
        Callback::new(move |_: ()| {
            let target = target.clone();
            let widgets = instances.get_untracked();
            let board = config.get_untracked();
            crate::api::drive(async move {
                match layout_api::save_layout(&target, &widgets, &board).await {
                    Ok(layout) => {
                        // The server responds with what it actually saved
                        // (dropped references, out-of-range boxes, clamped
                        // config), so we draw its response: what we see = what
                        // will reload.
                        instances.set(layout.widgets.clone());
                        snapshot.set(layout.widgets);
                        config.set(layout.config.clone());
                        snapshot_config.set(layout.config);
                        // The server's own verdict: an app dashboard now has
                        // a saved row ("Restaurar app" applies), a document
                        // dashboard has nothing to restore — its fields *are*
                        // the state — so the flag stays off.
                        customized.set(layout.customized);
                        close_config();
                        editing.set(false);
                        palette_open.set(false);
                        selected.set(None);
                        preview.set(None);
                        notice.set(None);
                    }
                    Err(error) => notice.set(Some(error)),
                }
            });
        })
    };

    let restore = {
        let target = target.clone();
        Callback::new(move |_: ()| {
            let target = target.clone();
            crate::api::drive(async move {
                match layout_api::restore_layout(&target).await {
                    Ok(layout) => {
                        instances.set(layout.widgets.clone());
                        snapshot.set(layout.widgets);
                        config.set(layout.config.clone());
                        snapshot_config.set(layout.config);
                        customized.set(false);
                        close_config();
                        editing.set(false);
                        palette_open.set(false);
                        selected.set(None);
                        preview.set(None);
                        notice.set(None);
                    }
                    Err(error) => notice.set(Some(error)),
                }
            });
        })
    };

    let add_widget = {
        let owner = owner.clone();
        Callback::new(move |(publisher, def): (String, WidgetDef)| {
            let catalog = apps.get_untracked();
            instances.update(|list| {
                let instance = model::new_instance(list, &publisher, &def);
                list.push(instance);
                model::normalize(&catalog, &owner, list);
            });
            palette_open.set(false);
            notice.set(None);
        })
    };

    let remove_widget = Callback::new(move |id: String| {
        let catalog = apps.get_untracked();
        instances.update(|list| {
            list.retain(|widget| widget.id != id);
            model::normalize(&catalog, &owner_key, list);
        });
    });

    let toggle_palette = Callback::new(move |_: ()| palette_open.update(|open| *open = !*open));

    // The configuration drawer: opening snapshots the saved board style into
    // the draft the controls edit (and the board previews) and re-reads the
    // global one, so a global saved from another tab is not overwritten by a
    // stale draft. "Aplicar" copies the board draft back and closes;
    // "Restaurar" only empties the draft of the active scope, so the user can
    // see the result before committing it.
    let open_config = Callback::new(move |_: ()| {
        style_draft.set(config.get_untracked().style);
        global_draft.set(global_style.get_untracked());
        config_scope.set(BoardScope::Board);
        config_open.set(true);
    });
    let apply_config = Callback::new(move |_: ()| {
        let style = style_draft.get_untracked();
        config.update(|current| current.style = style);
        close_config();
    });
    let cancel_config = Callback::new(move |_: ()| close_config());
    let restore_config = Callback::new(move |_: ()| {
        if config_scope.get_untracked() == BoardScope::Global {
            // The built-in defaults, previewed on the board until "Guardar
            // global" persists them.
            global_draft.set(DashboardStyle::default());
        } else {
            style_draft.set(DashboardStyle::default());
        }
    });
    // "Guardar global" is its own write: unlike the board's "Aplicar" (which
    // stages into the layout saved by the top bar's "Guardar"), the global row
    // has no second step, so the drawer owns the request and closes only after
    // the server accepted it.
    let save_global = Callback::new(move |_: ()| {
        let style = global_draft.get_untracked().clamped();
        crate::api::drive(async move {
            match layout_api::save_dashboard_settings(&style).await {
                Ok(settings) => {
                    global_style.set(settings.style);
                    global_editable.set(settings.editable);
                    close_config();
                    notice.set(None);
                }
                Err(error) => {
                    notice.set(Some(format!(
                        "No se pudo guardar la configuración global: {error}"
                    )));
                }
            }
        });
    });

    let editing_signal: Signal<bool> = editing.into();
    let refresh_signal: Signal<u32> = refresh.into();

    // ---- The shared top bar --------------------------------------------
    // The in-page header (title + edit controls) moved into the desk's top
    // bar, on the same contract as the form and the list: the *callbacks* are
    // created here, once, under this component's owner — an action created
    // inside the effect would be disposed on its next run while the bar still
    // drew the previous generation — and the effect only picks the specs the
    // current state calls for. Everything is cleared on teardown, so the next
    // view starts from an empty bar.
    let top_actions = use_top_bar_actions();
    let top_status = use_top_bar_status();
    let top_title = use_top_bar_title();
    let top_chrome = use_top_bar_chrome();

    Effect::new(move |_| {
        // Crumbs say *where* the view is; this is *what* it is.
        top_title.set(Some(label.clone()));

        // The bar's chrome has two inputs: the board's look (whether the title
        // survives full screen) and this visit's mode. The drawer previews the
        // flag live because `board` reads the draft while it is open.
        // `get_untracked` + `set` keeps an unchanged spec from re-notifying the
        // shell on every unrelated edit of the board.
        let chrome = style::chrome(&board.get(), fullscreen.get());
        if top_chrome.get_untracked() != chrome {
            top_chrome.set(chrome);
        }

        // The badge the in-page header used to draw: an app dashboard with a
        // saved override is personalized; a document-format dashboard's
        // fields *are* the state, so the server keeps the flag off for it.
        top_status.set(customized.get().then(|| TopBarStatus {
            label: "personalizado".to_string(),
            variant: BadgeVariant::Muted,
        }));

        let mut actions = Vec::new();
        if editable.get() {
            if editing.get() {
                actions.push(TopBarAction::Button(ButtonSpec {
                    label: "Guardar".to_string(),
                    aria_label: None,
                    icon: Some("Check"),
                    variant: ButtonVariant::Default,
                    disabled: false,
                    on_click: save,
                }));
                actions.push(TopBarAction::Button(ButtonSpec {
                    label: "Cancelar".to_string(),
                    aria_label: None,
                    icon: Some("X"),
                    variant: ButtonVariant::Ghost,
                    disabled: false,
                    on_click: cancel_edit,
                }));
                actions.push(TopBarAction::Button(ButtonSpec {
                    label: "Añadir widget".to_string(),
                    aria_label: None,
                    icon: Some("Plus"),
                    variant: ButtonVariant::Outline,
                    disabled: false,
                    on_click: toggle_palette,
                }));
                actions.push(TopBarAction::Button(ButtonSpec {
                    label: "Configuración".to_string(),
                    aria_label: None,
                    icon: Some("Settings"),
                    variant: ButtonVariant::Outline,
                    disabled: false,
                    on_click: open_config,
                }));
                if customized.get() {
                    actions.push(TopBarAction::Button(ButtonSpec {
                        label: "Restaurar app".to_string(),
                        aria_label: None,
                        icon: Some("RotateCcw"),
                        variant: ButtonVariant::Outline,
                        disabled: false,
                        on_click: restore,
                    }));
                }
            } else {
                actions.push(TopBarAction::Button(ButtonSpec {
                    label: "Editar".to_string(),
                    aria_label: None,
                    icon: Some("Pencil"),
                    variant: ButtonVariant::Outline,
                    disabled: false,
                    on_click: start_edit,
                }));
            }
        }
        // The one action that survives full screen. Readers get it too: going
        // full screen is a presentation choice, not an edit.
        actions.push(TopBarAction::FullScreenToggle(FullScreenToggleSpec {
            active: fullscreen.get(),
            on_click: toggle_fullscreen,
        }));
        top_actions.set(actions);
    });

    on_cleanup(move || {
        top_actions.set(Vec::new());
        top_status.set(None);
        top_title.set(None);
        top_chrome.set(TopBarChromeSpec::default());
    });

    view! {
        // The board fills the content area (the shell owns the viewport height);
        // `min-h-0` lets it shrink so the content column, not the page, scrolls
        // when a board is taller than the screen.
        <section class="ferris-dashboard flex h-full min-h-0 flex-col gap-3">
            <DashboardBoard
                owner=owner.clone()
                instances=instances
                board=board
                editing=editing_signal
                preview=preview
                dragging=dragging
                selected=selected
                on_remove=remove_widget
                refresh=refresh_signal
            />

            <WidgetPalette open=palette_open on_add=add_widget />

            <BoardConfig
                open=config_open
                scope=config_scope
                draft=style_draft
                global_draft=global_draft
                global_editable=Signal::derive(move || global_editable.get())
                on_apply=apply_config
                on_save_global=save_global
                on_cancel=cancel_config
                on_restore=restore_config
            />

            <WidgetProps
                owner=owner.clone()
                instances=instances
                selected=selected
                preview=preview
                on_remove=remove_widget
            />

            {move || {
                notice.get().map(|message| {
                    view! {
                        <div class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                            {message}
                        </div>
                    }
                })
            }}
        </section>
    }
}
