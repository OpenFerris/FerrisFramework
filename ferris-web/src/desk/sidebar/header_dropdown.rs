use crate::app::use_site_state;
use crate::app::{DeskNav, Workspace, WorkspaceItem, WorkspaceItemKind};
use crate::components::dropdown_builder::DropdownBuilder;
use crate::components::dropdown_builder::DropdownItem;
use crate::components::dropdown_builder::DropdownSpec;
use crate::components::dropdown_builder::DropdownTriggerSpec;
use crate::components::ui::dropdown_menu::DropdownMenuAlign;
use crate::components::ui::icon::Icon;
use crate::desk::session::DeskIcon;
use crate::desk::theme_dialog::ThemeDialog;
use crate::ext::{SLOT_SHELL_HEADER, UiCtx, UiHeaderSlot};
use crate::i18n::{DeskI18n, use_i18n};
use icons::ChevronDown;
use leptos::prelude::*;
use leptos_router::components::A;
use wasm_bindgen::JsCast;

use ferris_web_shared::url::safe_href;

/// Dropdown menu for the module header: shows current module and provides
/// navigation actions (App, workspaces, website, display options, reload, help).
///
/// The trigger mirrors the footer's `UserMenu` rhythm — a full-width row while
/// the sidebar is expanded, a centred module tile when it is collapsed to its
/// rail. An app may replace that visual: a `shell.header` extension is mounted
/// *inside* the dropdown trigger, so clicking the app's element opens the same
/// menu, and the element receives the active workspace and the sidebar state
/// as live props (see [`UiHeaderSlot`]).
#[component]
pub(super) fn HeaderDropdown(
    workspaces: ReadSignal<Vec<Workspace>>,
    active_idx: Memo<usize>,
    open: ReadSignal<bool>,
    on_pin: Callback<String>,
    on_toggle_sidebar: WriteSignal<bool>,
    on_reload: Callback<()>,
) -> impl IntoView {
    let theme_open = RwSignal::new(false);
    let ui_ctx = expect_context::<UiCtx>();
    // The arranged desk, so the workspaces menu mirrors it: folder grouping
    // included, loose workspaces at the top level.
    let desk = expect_context::<DeskNav>();
    // The install's site settings, read from the shell's own signal: the website
    // and help rows point wherever `Website Settings.home_page` and
    // `Navbar Settings.help_link` say, and are omitted when unset. Hardcoded `#`
    // rows used to sit here and went nowhere.
    let site = use_site_state();
    // `StoredValue` keeps the translator `Copy`, so the render effect that
    // builds the menu can capture it.
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));

    // The workspace the header currently names, resolved once so the trigger,
    // the slot props and the workspace rows all read the same value. Falls back
    // to the first workspace exactly like the active-index logic above it.
    let active_workspace = Memo::new(move |_| {
        let list = workspaces.get();
        let idx = active_idx.get();
        list.get(idx).or_else(|| list.first()).cloned()
    });
    let active_workspace_signal = Signal::derive(move || active_workspace.get());

    // An app declared into `shell.header`: it replaces the default trigger
    // visual but stays wrapped by the dropdown trigger, so the menu opens
    // normally. Empty on SSR and until the feed lands, like every slot.
    let has_app_header =
        Memo::new(move |_| !ui_ctx.extensions_for(SLOT_SHELL_HEADER, None).is_empty());

    // Built here, not inside `dropdown_spec`: that closure is a render effect,
    // and a `Callback` created while it runs is owned by the effect's arena,
    // which is emptied on every re-run — leaving the row's listener holding a
    // disposed value.
    let on_themes = Callback::new(move |_| theme_open.set(true));
    let on_fullscreen = Callback::new(|_| toggle_fullscreen());
    let on_sidebar = Callback::new(move |_| on_toggle_sidebar.update(|s| *s = !*s));

    // 1. Derivamos la especificación de forma reactiva mediante un Memo o Closure
    let dropdown_spec = move || {
        let ws_list = workspaces.get();
        let idx = active_idx.get();

        let icons = desk.icons.get();
        // The workspace the header names, resolved to a name so the rows can
        // mark it without re-deriving the index.
        let active_name = ws_list
            .get(idx.min(ws_list.len().saturating_sub(1)))
            .map(|w| w.name.clone());

        // The menu mirrors the desk's first layer — folders and loose
        // workspaces, in the arranged order — and unfolds each workspace into
        // all of its links. Every link points at its own route and pins its
        // workspace through `data-workspace`, so choosing a link lands on both
        // the workspace and that link.
        let items_workspaces = workspaces_menu(&icons, &ws_list, active_name.as_deref());

        // The site rows are conditional: an empty setting pushes nothing, so the
        // menu never carries a row that leads nowhere.
        let home_page = site.with(|s| s.home_page.trim().to_string());
        let help_link = site.with(|s| s.help_link.trim().to_string());

        let mut items = vec![
            DropdownItem::link(tr("nav.desk"), "/desk").with_icon("house"),
            DropdownItem::submenu(tr("nav.workspaces"), items_workspaces).with_icon("layers"),
        ];
        if let Some(home_page) = safe_href(&home_page) {
            // An in-app route (`/status`, `/planta`): no external-link glyph.
            items.push(DropdownItem::link(tr("desk.header.website"), home_page));
        }
        items.push(DropdownItem::separator());
        items.push(
            DropdownItem::submenu(
                tr("desk.header.appearance"),
                vec![
                    DropdownItem::action(tr("desk.theme.change"), on_themes).with_icon("palette"),
                    DropdownItem::action(tr("desk.header.fullscreen"), on_fullscreen)
                        .with_icon("maximize"),
                    DropdownItem::action(tr("desk.sidebar.toggle"), on_sidebar)
                        .with_icon("panel-left"),
                ],
            )
            .with_icon("layout-grid"),
        );
        items.push(
            DropdownItem::action(tr("desk.header.reload"), on_reload).with_icon("refresh-cw"),
        );
        if let Some(help_link) = safe_href(&help_link) {
            items.push(
                DropdownItem::link(tr("desk.header.help"), help_link)
                    .with_icon("circle")
                    .muted(),
            );
        }

        DropdownSpec {
            align: DropdownMenuAlign::Center,
            trigger: DropdownTriggerSpec::Custom {
                view: view! {
                    {move || header_trigger(has_app_header.get(), active_workspace_signal, open.into(), &tr("desk.header.workspace_fallback"))}
                }.into_any()
            },
            items,
        }
    };

    view! {
        <div
            data-name="HeaderDropdown"
            on:click=move |ev: web_sys::MouseEvent| {
                // Cada fila de workspace dice cuál es con `data-workspace`, en
                // lugar de llevar su propio `Callback`: las filas se construyen
                // dentro de `dropdown_spec`, que es un efecto de render, y un
                // `Callback` creado ahí pertenece a la arena del efecto, que se
                // vacía en cada re-ejecución. Un solo listener delegado —en un
                // elemento normal, no en un componente— resuelve cuál se pulsó.
                // La fila es un `<A>` con su propia ruta: el listener solo fija
                // el workspace; la navegación la hace el enlace.
                let Some(target) = ev.target() else { return };
                let Ok(target) = target.dyn_into::<web_sys::Element>() else { return };
                let Some(row) = target.closest("[data-workspace]").ok().flatten() else { return };
                let Some(name) = row.get_attribute("data-workspace") else { return };
                on_pin.run(name);
            }
        >
            {move || view! { <DropdownBuilder spec=dropdown_spec() /> }}
        </div>

        {/* ---------- Theme dialog ---------- */}
        <ThemeDialog open=theme_open />
    }
}

/// The "Workspaces" submenu, mirroring the desk's first layer: one entry per
/// arranged desk icon, in order — a folder unfolds the workspaces it groups, a
/// loose workspace unfolds its own links. Any workspace the arrangement does not
/// mention (a feed that has not landed, or one added after the grid was saved)
/// is appended so no workspace is unreachable.
fn workspaces_menu(
    icons: &[DeskIcon],
    workspaces: &[Workspace],
    active: Option<&str>,
) -> Vec<DropdownItem> {
    let mut items = Vec::new();
    let mut placed: Vec<&str> = Vec::new();

    for icon in icons {
        if icon.is_folder() {
            items.push(folder_entry(icon, workspaces, active));
            for member in &icon.items {
                placed.push(member.id.as_str());
            }
        } else {
            items.push(workspace_entry(
                &icon.id,
                icon.icon.as_deref(),
                &icon.label,
                workspaces,
                active,
            ));
            placed.push(icon.id.as_str());
        }
    }

    for ws in workspaces {
        if !placed.contains(&ws.name.as_str()) {
            items.push(workspace_entry(
                &ws.name,
                ws.icon.as_deref(),
                &ws.label,
                workspaces,
                active,
            ));
        }
    }

    items
}

/// One desk folder: a submenu of the workspaces it groups, each of which unfolds
/// its own links.
fn folder_entry(icon: &DeskIcon, workspaces: &[Workspace], active: Option<&str>) -> DropdownItem {
    let members = icon
        .items
        .iter()
        .map(|member| {
            workspace_entry(
                &member.id,
                member.icon.as_deref(),
                &member.label,
                workspaces,
                active,
            )
        })
        .collect();

    DropdownItem::submenu(icon.label.clone(), members)
        .with_icon(icon.icon.clone().unwrap_or_else(|| "folder".to_string()))
}

/// One workspace: a submenu holding all of its links, or a plain row to the desk
/// when it declares none. The trigger only unfolds the submenu; the links inside
/// are what navigate, and each carries `data-workspace` so the click also pins
/// the workspace (see the delegated listener).
fn workspace_entry(
    name: &str,
    icon: Option<&str>,
    label: &str,
    workspaces: &[Workspace],
    active: Option<&str>,
) -> DropdownItem {
    let icon = icon.unwrap_or_default().to_string();
    let label = label.to_string();

    let links = workspaces
        .iter()
        .find(|ws| ws.name == name)
        .map(|ws| ws.items.as_slice())
        .unwrap_or(&[]);

    if links.is_empty() {
        return workspace_link(name, &label, &icon, "/desk", active == Some(name));
    }

    let is_active = active == Some(name);
    DropdownItem::submenu_custom(
        view! {
            <span class="flex w-full min-w-0 items-center gap-2">
                <Icon name=icon.clone() class="size-4 shrink-0" />
                <span class="min-w-0 flex-1 truncate">{label.clone()}</span>
                {is_active.then(|| view! {
                    <Icon name="check" class="size-4 shrink-0" />
                })}
            </span>
        },
        workspace_links(name, links),
    )
}

/// The links of a workspace: leaves as rows, groups as further submenus — the
/// same tree the sidebar draws.
fn workspace_links(ws_name: &str, items: &[WorkspaceItem]) -> Vec<DropdownItem> {
    items
        .iter()
        .filter_map(|item| {
            let label = item.label.clone();
            let icon = item.icon.clone().unwrap_or_default();
            match item.kind {
                WorkspaceItemKind::Group => Some(
                    DropdownItem::submenu(label, workspace_links(ws_name, &item.children))
                        .with_icon(icon),
                ),
                // An external link is a plain anchor: the desk router would try
                // to resolve an `https:` URL as an in-app route. An unsafe
                // destination is dropped rather than offered as a row that does
                // nothing (the URL is unvalidated workspace metadata).
                WorkspaceItemKind::Url => item
                    .href
                    .as_deref()
                    .and_then(safe_href)
                    .map(|href| DropdownItem::link(label, href).with_icon(icon)),
                WorkspaceItemKind::Doctype | WorkspaceItemKind::Route => Some(workspace_link(
                    ws_name,
                    &label,
                    &icon,
                    item.route.as_deref().unwrap_or(""),
                    false,
                )),
            }
        })
        .collect()
}

/// A navigable workspace row: the link is the destination, `data-workspace`
/// pins the workspace on click, and `active` draws the tick for the workspace
/// the header currently names.
fn workspace_link(
    ws_name: &str,
    label: &str,
    icon: &str,
    href: &str,
    active: bool,
) -> DropdownItem {
    let name = ws_name.to_string();
    let label = label.to_string();
    let icon = icon.to_string();
    let Some(href) = safe_href(href) else {
        // A route from metadata that fails the scheme allow-list: keep the
        // label visible, but do not build a link from it.
        return DropdownItem::custom(
            view! {
                <span class="flex w-full min-w-0 items-center gap-2 text-muted-foreground" title=label>
                    <Icon name=icon class="size-4 shrink-0" />
                    <span class="min-w-0 flex-1 truncate">{label.clone()}</span>
                </span>
            },
            None,
        );
    };

    DropdownItem::custom(
        view! {
            <A
                href=href
                // `attr:` on a component: bare attributes here would be parsed
                // as Rust expressions, `data-workspace` as a function call.
                attr:data-workspace=name
                attr:data-dropdown-close="true"
                attr:class="flex w-full min-w-0 items-center gap-2 text-inherit no-underline focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
            >
                <Icon name=icon class="size-4 shrink-0" />
                <span class="min-w-0 flex-1 truncate">{label}</span>
                {active.then(|| view! {
                    <Icon name="check" class="size-4 shrink-0" />
                })}
            </A>
        },
        None,
    )
}

/// The trigger visual: the app's `shell.header` element when one is declared
/// (mounted inside the dropdown trigger by [`UiHeaderSlot`]), otherwise the
/// built-in tile that replicates the footer's `UserMenu` rhythm — a full-width
/// row while the sidebar is expanded, a centred module tile on the collapsed
/// rail. Both are wrapped by `DropdownMenuTrigger`, so either opens the menu.
fn header_trigger(
    app_visual: bool,
    workspace: Signal<Option<Workspace>>,
    open: Signal<bool>,
    workspace_fallback: &str,
) -> AnyView {
    if app_visual {
        return view! {
            <UiHeaderSlot workspace=workspace open=open />
        }
        .into_any();
    }

    let ws = workspace.get();
    let expanded = open.get();
    let icon = ws.as_ref().and_then(|w| w.icon.clone()).unwrap_or_default();
    let label = ws
        .as_ref()
        .map(|w| w.label.clone())
        .unwrap_or_else(|| workspace_fallback.to_string());
    let module = ws.as_ref().map(|w| w.module.clone()).unwrap_or_default();

    // The exact structure of the footer's `UserMenu` button: a rounded hover
    // row with the icon tile, a truncating text block and the chevron — only
    // the tile survives the collapsed rail.
    if expanded {
        view! {
            <button
                type="button"
                title=label.clone()
                class="w-full flex items-center gap-2.5 p-2 rounded-lg hover:bg-accent transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
            >
                <span class="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground">
                    <Icon name=icon class="size-4" />
                </span>
                <span class="flex-1 text-left min-w-0">
                    <span class="block truncate text-sm font-medium text-foreground">{label.clone()}</span>
                    <span class="block truncate text-xs text-muted-foreground">{module}</span>
                </span>
                <span class="inline-flex shrink-0 text-muted-foreground transition-transform duration-200 group-data-[state=open]:rotate-180">
                    <ChevronDown class="size-4" />
                </span>
            </button>
        }
        .into_any()
    } else {
        view! {
            <button
                type="button"
                title=label
                class="w-10 h-10 mx-auto flex items-center justify-center rounded-lg hover:bg-accent transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
            >
                <span class="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground">
                    <Icon name=icon class="size-4" />
                </span>
            </button>
        }
        .into_any()
    }
}

/// Enters the browser's fullscreen mode, or leaves it when already in it.
/// `exitFullscreen` reports failure through the promise it returns rather than by
/// throwing, so a refusal (no user gesture, or a permissions policy that forbids
/// it) is simply left alone.
fn toggle_fullscreen() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };

    if document.fullscreen_element().is_some() {
        document.exit_fullscreen();
    } else if let Some(root) = document.document_element() {
        root.request_fullscreen().ok();
    }
}
