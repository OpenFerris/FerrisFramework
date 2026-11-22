//! The board configuration drawer: geometry (gap, padding, radius), surface
//! and full-screen title — for **this board** or for the **Desk's global
//! style**.
//!
//! It is a drawer over the board, not a page: the board stays visible behind
//! it, and the drawer edits a **draft** the board previews live
//! (`DashboardGrid` swaps the resolved style for the draft while this is
//! open). "Aplicar" copies the board draft into the saved configuration,
//! "Cancelar" drops it and "Restaurar global" empties it again — the last one
//! does not touch the server: it only resets the draft, so the user can inspect
//! the Desk's own look before committing it.
//!
//! ## Two scopes, one form
//!
//! The same controls edit two different `DashboardStyle`s (C8):
//!
//! - **Este tablero** — the dashboard's own override, part of the layout the
//!   top bar's "Guardar" persists. "Restaurar global" empties the override so
//!   the board falls back to its declared style and the Desk's base.
//! - **Global del Desk** — the one shared row that is the base of every board.
//!   It is saved by its own button (there is no layout save to ride on) and the
//!   toggle is only drawn for a user the server says may edit it. "Restaurar
//!   por defecto" empties the draft, previewing the built-in defaults.
//!
//! Sharing the markup is the point: the two scopes are the same fields, and
//! the preview shows exactly what the active one is about to change.
//!
//! The overlay is hand-rolled, like the widget settings panel, because the
//! dashboard is client-only and the library's `Sheet`/`Dialog` rely on an SSR
//! `<script>` that never runs here. The shape is deliberately the same one
//! (backdrop button, `role="dialog"` aside, footer actions), so both panels
//! behave and read the same.
//!
//! Only the fields the board's [`DashboardStyle`] declares appear here: the
//! per-widget look belongs to each instance's settings (the `Estilo` section
//! of `WidgetProps`).

use leptos::prelude::*;

use ferris_schema::contract::DashboardStyle;
use ferris_schema::contract::style::{MAX_GAP, MAX_PADDING, MAX_RADIUS};

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;

use super::style_fields::{BackgroundFields, NumberField};

/// Which style the drawer edits.
///
/// Both variants are the same `DashboardStyle`, so the controls are shared; the
/// difference is where the draft is committed (the board's layout save, or the
/// global settings endpoint) and what "Restaurar" means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum BoardScope {
    /// The dashboard's own override.
    #[default]
    Board,
    /// The Desk's shared global style.
    Global,
}

/// The board configuration drawer.
#[component]
pub(super) fn BoardConfig(
    /// Whether the drawer is open.
    open: RwSignal<bool>,
    /// Which style the drawer edits.
    scope: RwSignal<BoardScope>,
    /// The board's live draft (and what the board previews while active).
    draft: RwSignal<DashboardStyle>,
    /// The global live draft.
    global_draft: RwSignal<DashboardStyle>,
    /// Whether the caller may edit the global style (the server's `Create`
    /// grant on `Dashboard Settings`). When false the scope toggle is not drawn:
    /// the drawer stays the per-board one, and no button can 403.
    global_editable: Signal<bool>,
    /// Commit the board draft into the saved configuration and close.
    on_apply: Callback<()>,
    /// Persist the global draft and close.
    on_save_global: Callback<()>,
    /// Close and discard the drafts.
    on_cancel: Callback<()>,
    /// Empty the draft of the active scope (without closing).
    on_restore: Callback<()>,
) -> impl IntoView {
    // Escape closes the drawer. The listener lives as long as the component and
    // stays inert while the drawer is shut — the same convention the
    // notifications panel keeps.
    let _escape = window_event_listener(leptos::ev::keydown, move |event| {
        if open.get() && event.key() == "Escape" {
            on_cancel.run(());
        }
    });

    let is_global = Signal::derive(move || scope.get() == BoardScope::Global);

    view! {
        {move || {
            open.get()
                .then(|| {
                    view! {
                        <div class="fixed inset-0 z-50">
                            <button
                                type="button"
                                class="absolute inset-0 bg-foreground/40 animate-in fade-in duration-200 motion-reduce:animate-none"
                                on:click=move |_| on_cancel.run(())
                                aria-label="Cerrar la configuración"
                            ></button>
                            <aside
                                role="dialog"
                                aria-modal="true"
                                aria-label=move || {
                                    if is_global.get() {
                                        "Configuración global del Desk"
                                    } else {
                                        "Configuración del tablero"
                                    }
                                }
                                class="absolute inset-y-0 right-0 flex w-80 max-w-[90vw] flex-col border-l border-border bg-card text-card-foreground shadow-lg animate-in fade-in slide-in-from-right-4 duration-200 motion-reduce:animate-none"
                            >
                                <header class="flex items-center justify-between gap-2 border-b border-border px-4 py-3">
                                    <h2 class="text-sm font-semibold">
                                        {move || {
                                            if is_global.get() {
                                                "Configuración global"
                                            } else {
                                                "Configuración"
                                            }
                                        }}
                                    </h2>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::IconSm
                                        on:click=move |_| on_cancel.run(())
                                        attr:aria-label="Cerrar la configuración"
                                    >
                                        <icons::X class="size-4" />
                                    </Button>
                                </header>

                                <div class="min-h-0 flex-1 space-y-5 overflow-y-auto px-4 py-3">
                                    // The scope switch is the server's answer:
                                    // a user who may edit dashboards but not the
                                    // global never sees it.
                                    {move || {
                                        global_editable
                                            .get()
                                            .then(|| {
                                                view! {
                                                    <div
                                                        role="group"
                                                        aria-label="Ámbito de la configuración"
                                                        class="grid grid-cols-2 gap-1 rounded-md bg-muted p-1"
                                                    >
                                                        <ScopeButton
                                                            scope=scope
                                                            value=BoardScope::Board
                                                            label="Este tablero"
                                                        />
                                                        <ScopeButton
                                                            scope=scope
                                                            value=BoardScope::Global
                                                            label="Global del Desk"
                                                        />
                                                    </div>
                                                }
                                            })
                                    }}

                                    {move || {
                                        if is_global.get() {
                                            view! { <BoardStyleFields draft=global_draft /> }
                                                .into_any()
                                        } else {
                                            view! { <BoardStyleFields draft=draft /> }.into_any()
                                        }
                                    }}

                                    {move || {
                                        is_global
                                            .get()
                                            .then(|| {
                                                view! {
                                                    <p class="text-[0.65rem] text-muted-foreground">
                                                        "La base de todos los tableros: cada uno puede sobrescribir estos valores."
                                                    </p>
                                                }
                                            })
                                    }}
                                </div>

                                <footer class="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        class="mr-auto"
                                        on:click=move |_| on_restore.run(())
                                    >
                                        {move || {
                                            if scope.get() == BoardScope::Global {
                                                "Restaurar por defecto"
                                            } else {
                                                "Restaurar global"
                                            }
                                        }}
                                    </Button>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        on:click=move |_| on_cancel.run(())
                                    >
                                        "Cancelar"
                                    </Button>
                                    {move || {
                                        if scope.get() == BoardScope::Global {
                                            view! {
                                                <Button
                                                    variant=ButtonVariant::Default
                                                    size=ButtonSize::Sm
                                                    on:click=move |_| on_save_global.run(())
                                                >
                                                    "Guardar global"
                                                </Button>
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <Button
                                                    variant=ButtonVariant::Default
                                                    size=ButtonSize::Sm
                                                    on:click=move |_| on_apply.run(())
                                                >
                                                    "Aplicar"
                                                </Button>
                                            }
                                                .into_any()
                                        }
                                    }}
                                </footer>
                            </aside>
                        </div>
                    }
                })
        }}
    }
}

/// One half of the scope switch: a real button with `aria-pressed`, so the
/// active scope is not conveyed by colour alone.
#[component]
fn ScopeButton(
    scope: RwSignal<BoardScope>,
    value: BoardScope,
    label: &'static str,
) -> impl IntoView {
    let active = Signal::derive(move || scope.get() == value);
    view! {
        <button
            type="button"
            aria-pressed=move || active.get().to_string()
            class=move || {
                if active.get() {
                    "h-8 rounded-sm bg-card px-3 text-xs font-medium text-foreground shadow-xs focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                } else {
                    "h-8 rounded-sm px-3 text-xs font-medium text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                }
            }
            on:click=move |_| scope.set(value)
        >
            {label}
        </button>
    }
}

/// The fields shared by both scopes: geometry, surface and chrome.
///
/// A separate component (rather than a closure) because the drawer mounts a
/// different draft per scope: switching scope must rebuild the controls around
/// the other signal, not keep the previous one's `Signal::derive` bindings.
#[component]
fn BoardStyleFields(draft: RwSignal<DashboardStyle>) -> impl IntoView {
    view! {
        <div class="space-y-5">
            <section class="space-y-3">
                <h3 class="text-[0.65rem] font-medium tracking-wide text-muted-foreground uppercase">
                    "Disposición"
                </h3>
                <NumberField
                    label="Separación entre widgets"
                    value=Signal::derive(move || draft.get().gap_px)
                    max=MAX_GAP
                    hint="Vacío = el valor global del Desk."
                    on_change=Callback::new(move |gap_px| {
                        draft.update(|style| style.gap_px = gap_px);
                    })
                />
                <NumberField
                    label="Padding del tablero"
                    value=Signal::derive(move || draft.get().padding_px)
                    max=MAX_PADDING
                    hint="Espacio interior alrededor de la rejilla. Vacío = 0."
                    on_change=Callback::new(move |padding_px| {
                        draft.update(|style| style.padding_px = padding_px);
                    })
                />
                <NumberField
                    label="Radio del tablero"
                    value=Signal::derive(move || draft.get().border_radius)
                    max=MAX_RADIUS
                    hint="Vacío = sin radio."
                    on_change=Callback::new(move |border_radius| {
                        draft.update(|style| style.border_radius = border_radius);
                    })
                />
            </section>

            <section class="space-y-3">
                <h3 class="text-[0.65rem] font-medium tracking-wide text-muted-foreground uppercase">
                    "Color"
                </h3>
                <BackgroundFields
                    label="Fondo del tablero"
                    value=Signal::derive(move || draft.get().background)
                    custom=Signal::derive(move || draft.get().background_custom)
                    on_change=Callback::new(move |(background, custom)| {
                        draft.update(|style| {
                            style.background = background;
                            style.background_custom = custom;
                        });
                    })
                />
            </section>

            <section class="space-y-3">
                <h3 class="text-[0.65rem] font-medium tracking-wide text-muted-foreground uppercase">
                    "Pantalla completa"
                </h3>
                <div class="flex items-center gap-2">
                    <Checkbox
                        aria_label="Mostrar el título en pantalla completa"
                        checked=Signal::derive(move || {
                            draft.get().title_full_screen.unwrap_or(true)
                        })
                        on_checked_change=Callback::new(move |checked: bool| {
                            draft.update(|style| style.title_full_screen = Some(checked));
                        })
                    />
                    <span class="text-xs">"Mostrar el título en pantalla completa"</span>
                </div>
                <p class="text-[0.65rem] text-muted-foreground">
                    "En pantalla completa sólo quedan este título y el botón para salir."
                </p>
            </section>
        </div>
    }
}
