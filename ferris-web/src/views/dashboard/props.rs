//! An instance's settings: title, size in the grid, its own appearance (the
//! `Estilo` section, resolved over the definition's declared style) and the
//! parameters its definition declares, grouped by `group`.
//!
//! The panel works on a *draft* (`draft`) instead of writing directly
//! to the effective list: "Aplicar" confirms it, "Cancelar" discards it. The size
//! does preview live — it moves the grid's `preview`, which only changes
//! geometry — so as not to request the widget's data again on every keystroke.
//!
//! Each declared parameter is edited through the **form-field system**
//! (D2 of the dashboard widget): [`params`] turns a `WidgetParam` into
//! a `FormField`, the panel renders it with the form renderer (label,
//! required marker, error and description included) and "Aplicar" validates
//! with the form's own rules before [`params::sync_params`] writes the
//! values back into the instance.

use leptos::prelude::*;

use ferris_schema::contract::{AppUi, Placement, WidgetInstance};

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::views::form::{FormTabs, render_form_field};
use crate::ext::UiCtx;
use crate::views::services::{form_layout, validate_form};

use super::model;
use super::params;
use super::style_fields::WidgetStyleFields;

/// The minimum and maximum columns/rows the builder allows.
const MIN_COLUMNS: u8 = 1;

/// The settings panel of the selected instance. It renders nothing if none
/// is selected.
#[component]
pub(super) fn WidgetProps(
    /// App that owns the dashboard.
    owner: String,
    /// The effective list: destination of "Aplicar".
    instances: RwSignal<Vec<WidgetInstance>>,
    /// The instance whose settings are open (`None` = panel closed).
    selected: RwSignal<Option<String>>,
    /// In-flight placement, to preview the size.
    preview: RwSignal<Option<(String, Placement)>>,
    /// Remove the instance (the panel closes).
    on_remove: Callback<String>,
) -> impl IntoView {
    let apps = expect_context::<UiCtx>().apps;
    let draft = RwSignal::new(None::<WidgetInstance>);

    // The draft follows the selection: on open (or when the confirmed
    // list changes, e.g. after a drag) the instance is copied again.
    Effect::new(move |_| {
        let next = selected
            .get()
            .and_then(|id| instances.get().into_iter().find(|widget| widget.id == id));
        draft.set(next);
    });

    let close = Callback::new(move |_: ()| {
        preview.set(None);
        selected.set(None);
    });

    let remove = Callback::new(move |_: ()| {
        if let Some(id) = draft.get_untracked().map(|widget| widget.id) {
            on_remove.run(id);
        }
        preview.set(None);
        selected.set(None);
    });

    view! {
        {move || {
            // Only `selected` decides open/closed: this closure does not re-run
            // on typing (the parameters' values live in their own
            // `FormField` signals), so the inputs below are not recreated and
            // focus is preserved.
            if selected.get().is_none() {
                return ().into_any();
            }
            let Some(widget) = draft.get_untracked() else {
                return ().into_any();
            };
            let owner = owner.clone();
            // Each handle takes its own clone: the handlers' `move` closures
            // consume what they capture, and `owner` is still needed to
            // resolve the definition and the `Sizing`.
            let owner_span = owner.clone();
            let owner_rows = owner.clone();
            let owner_apply = owner.clone();
            // The catalog is read *untracked* here: a feed refresh while the
            // panel is open would otherwise rebuild the form fields and drop
            // whatever the user had typed without applying. The definition
            // being edited is a snapshot, like today's controls.
            let definition = crate::ext::resolve_ref_in(&apps.get_untracked(), &owner, &widget.widget)
                .map(|(_, def)| def);
            let defs = definition
                .as_ref()
                .map(|def| def.params.clone())
                .unwrap_or_default();
            let sizing = model::sizing_of(&apps.get_untracked(), &owner, &widget.widget);
            // One `FormField` per parameter, with the widget's own layout
            // markers applied (or the legacy `group` sections synthesized).
            // Typing mutates each field's own `value`/`checked` signal, so this
            // closure does not re-run and the inputs keep their focus.
            let field_list = params::param_fields(&defs, &widget);
            let (tab_labels, default_tab, sections) = form_layout(&field_list);
            let fields = RwSignal::new(field_list);

            view! {
                <div class="fixed inset-0 z-50">
                    <button
                        type="button"
                        class="absolute inset-0 bg-foreground/40 animate-in fade-in duration-200 motion-reduce:animate-none"
                        on:click=move |_| close.run(())
                        aria-label="Cerrar los ajustes"
                    ></button>
                    <aside
                        role="dialog"
                        aria-modal="true"
                        aria-label="Ajustes del widget"
                        class="absolute inset-y-0 right-0 flex w-80 max-w-[90vw] flex-col border-l border-border bg-card text-card-foreground shadow-lg animate-in fade-in slide-in-from-right-4 duration-200 motion-reduce:animate-none"
                    >
                        <header class="flex items-center justify-between gap-2 border-b border-border px-4 py-3">
                            <h2 class="text-sm font-semibold">"Ajustes"</h2>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::IconSm
                                on:click=move |_| close.run(())
                                attr:aria-label="Cerrar los ajustes"
                            >
                                <icons::X class="size-4" />
                            </Button>
                        </header>

                        <div class="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-3">
                            <p class="truncate text-[0.7rem] text-muted-foreground">
                                <code>{widget.widget.clone()}</code>
                            </p>

                            <div class="space-y-2">
                                <label class="block space-y-1">
                                    <span class="block text-xs font-medium">"Título"</span>
                                    <input
                                        type="text"
                                        class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                                        placeholder="Vacío = el título del widget"
                                        prop:value=label_value(draft)
                                        on:input=move |ev| {
                                            set_label(&draft, event_target_value(&ev));
                                        }
                                    />
                                </label>
                                <div class="flex items-center gap-2">
                                    <Checkbox
                                        aria_label="Mostrar el título"
                                        checked=show_title_value(draft)
                                        on_checked_change=Callback::new(move |checked: bool| {
                                            set_show_title(&draft, checked);
                                        })
                                    />
                                    <span class="text-xs">"Mostrar título"</span>
                                </div>
                                <p class="text-[0.65rem] text-muted-foreground">
                                    "El host pinta el título y el subtítulo; el widget sólo aporta el contenido."
                                </p>
                            </div>

                            <div class="space-y-2">
                                <label class="block space-y-1">
                                    <span class="block text-xs font-medium">"Subtítulo"</span>
                                    <input
                                        type="text"
                                        class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                                        placeholder="Vacío = la descripción de la definición"
                                        prop:value=subtitle_value(draft)
                                        on:input=move |ev| {
                                            set_subtitle(&draft, event_target_value(&ev));
                                        }
                                    />
                                </label>
                                <div class="flex items-center gap-2">
                                    <Checkbox
                                        aria_label="Mostrar el subtítulo"
                                        checked=show_subtitle_value(draft)
                                        on_checked_change=Callback::new(move |checked: bool| {
                                            set_show_subtitle(&draft, checked);
                                        })
                                    />
                                    <span class="text-xs">"Mostrar subtítulo"</span>
                                </div>
                            </div>

                            <div class="flex gap-3">
                                <label class="block flex-1 space-y-1">
                                    <span class="block text-xs font-medium">"Columnas"</span>
                                    <select
                                        class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                                        prop:value=span_value(draft, sizing).to_string()
                                        on:change=move |ev| {
                                            if let Ok(span) = event_target_value(&ev).parse::<u8>() {
                                                set_span(&draft, &preview, &apps, &owner_span, span);
                                            }
                                        }
                                    >
                                        {(MIN_COLUMNS..=ferris_schema::contract::layout::COLUMN)
                                            .map(|n| view! {
                                                <option value=n.to_string()>{n}</option>
                                            })
                                            .collect_view()}
                                    </select>
                                </label>
                                <label class="block flex-1 space-y-1">
                                    <span class="block text-xs font-medium">"Filas"</span>
                                    <input
                                        type="number"
                                        min="1"
                                        max=ferris_schema::contract::layout::MAX_ROWS.to_string()
                                        class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                                        prop:value=rows_value(draft, sizing).to_string()
                                        on:input=move |ev| {
                                            if let Ok(rows) = event_target_value(&ev).parse::<u8>() {
                                                set_rows(&draft, &preview, &apps, &owner_rows, rows);
                                            }
                                        }
                                    />
                                </label>
                            </div>

                            <WidgetStyleFields draft=draft />

                            <FormTabs
                                tab_labels=tab_labels
                                default_tab=default_tab
                                sections=sections
                                fields=fields
                                render_field=render_form_field
                            />
                        </div>

                        <footer class="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
                            <Button
                                variant=ButtonVariant::Destructive
                                size=ButtonSize::Sm
                                class="mr-auto"
                                on:click=move |_| remove.run(())
                            >
                                "Quitar"
                            </Button>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                on:click=move |_| close.run(())
                            >
                                "Cancelar"
                            </Button>
                            <Button
                                variant=ButtonVariant::Default
                                size=ButtonSize::Sm
                                on:click=move |_| {
                                    let field_list = fields.get_untracked();
                                    // The form's own validation: a failure
                                    // paints through each field's `error`
                                    // signal and keeps the panel open.
                                    if !validate_form(&field_list) {
                                        return;
                                    }
                                    let Some(mut widget) = draft.get_untracked() else {
                                        return;
                                    };
                                    params::sync_params(&mut widget, &field_list, &defs);
                                    let id = widget.id.clone();
                                    let catalog = apps.get_untracked();
                                    let owner = owner_apply.clone();
                                    instances.update(|list| {
                                        if let Some(slot) =
                                            list.iter_mut().find(|slot| slot.id == id)
                                        {
                                            *slot = widget;
                                        }
                                        model::normalize(&catalog, &owner, list);
                                    });
                                    preview.set(None);
                                    selected.set(None);
                                }
                            >
                                "Aplicar"
                            </Button>
                        </footer>
                    </aside>
                </div>
            }
            .into_any()
        }}
    }
}

/// The draft's title (empty when it declares none).
fn label_value(draft: RwSignal<Option<WidgetInstance>>) -> Signal<String> {
    Signal::derive(move || {
        draft.with(|draft| {
            draft
                .as_ref()
                .and_then(|widget| widget.label.clone())
                .unwrap_or_default()
        })
    })
}

/// The draft's effective width: the declared one or the definition's default.
///
/// *Non*-reactive read: these values are painted once when the panel opens, and
/// subscribing them to the draft would recreate the fields on every keystroke.
/// This is what allows typing without losing focus.
fn span_value(
    draft: RwSignal<Option<WidgetInstance>>,
    sizing: ferris_schema::contract::layout::Sizing,
) -> u8 {
    draft.with_untracked(|draft| {
        draft
            .as_ref()
            .and_then(|widget| widget.span)
            .unwrap_or(sizing.size.span)
    })
}

/// The draft's effective height (non-reactive read, like [`span_value`]).
fn rows_value(
    draft: RwSignal<Option<WidgetInstance>>,
    sizing: ferris_schema::contract::layout::Sizing,
) -> u8 {
    draft.with_untracked(|draft| {
        draft
            .as_ref()
            .and_then(|widget| widget.rows)
            .unwrap_or(sizing.size.rows)
    })
}

/// Writes the title: empty goes back to the definition's label.
fn set_label(draft: &RwSignal<Option<WidgetInstance>>, value: String) {
    draft.update(|draft| {
        if let Some(widget) = draft {
            widget.label = if value.is_empty() { None } else { Some(value) };
        }
    });
}

/// Whether the host paints the title (the default: yes).
fn show_title_value(draft: RwSignal<Option<WidgetInstance>>) -> Signal<bool> {
    Signal::derive(move || {
        draft.with(|draft| {
            draft
                .as_ref()
                .and_then(|widget| widget.show_title)
                .unwrap_or(true)
        })
    })
}

/// Toggles the title without discarding its text, so turning it back on
/// restores the same heading.
fn set_show_title(draft: &RwSignal<Option<WidgetInstance>>, value: bool) {
    draft.update(|draft| {
        if let Some(widget) = draft {
            widget.show_title = Some(value);
        }
    });
}

/// The draft's subtitle text (empty when it declares none).
fn subtitle_value(draft: RwSignal<Option<WidgetInstance>>) -> Signal<String> {
    Signal::derive(move || {
        draft.with(|draft| {
            draft
                .as_ref()
                .and_then(|widget| widget.subtitle.clone())
                .unwrap_or_default()
        })
    })
}

/// Writes the subtitle: empty goes back to the definition's description.
///
/// A first non-empty text also turns the subtitle on when the editor had not
/// decided yet (`show_subtitle` absent), so typing shows the result instead of
/// silently keeping it hidden; an explicit "off" survives the edit.
fn set_subtitle(draft: &RwSignal<Option<WidgetInstance>>, value: String) {
    // Keep the text as typed (a controlled input that trims on every keystroke
    // would fight the cursor); "empty" is decided on the trimmed view and the
    // API trims it again on save.
    let has_text = !value.trim().is_empty();
    draft.update(|draft| {
        if let Some(widget) = draft {
            let was_unset = widget.show_subtitle.is_none();
            widget.subtitle = has_text.then_some(value);
            if was_unset && has_text {
                widget.show_subtitle = Some(true);
            }
        }
    });
}

/// Whether the host paints the subtitle (the default: no; opt-in).
fn show_subtitle_value(draft: RwSignal<Option<WidgetInstance>>) -> Signal<bool> {
    Signal::derive(move || {
        draft.with(|draft| {
            draft
                .as_ref()
                .and_then(|widget| widget.show_subtitle)
                .unwrap_or(false)
        })
    })
}

/// Toggles the subtitle without discarding its text.
fn set_show_subtitle(draft: &RwSignal<Option<WidgetInstance>>, value: bool) {
    draft.update(|draft| {
        if let Some(widget) = draft {
            widget.show_subtitle = Some(value);
        }
    });
}

/// Changes the width and previews the new box in the grid (without confirming).
fn set_span(
    draft: &RwSignal<Option<WidgetInstance>>,
    preview: &RwSignal<Option<(String, Placement)>>,
    apps: &RwSignal<Vec<AppUi>>,
    owner: &str,
    span: u8,
) {
    draft.update(|draft| {
        if let Some(widget) = draft {
            widget.span = Some(span);
        }
    });
    push_preview(draft, preview, apps, owner);
}

/// Changes the height and previews the new box.
fn set_rows(
    draft: &RwSignal<Option<WidgetInstance>>,
    preview: &RwSignal<Option<(String, Placement)>>,
    apps: &RwSignal<Vec<AppUi>>,
    owner: &str,
    rows: u8,
) {
    draft.update(|draft| {
        if let Some(widget) = draft {
            widget.rows = Some(rows);
        }
    });
    push_preview(draft, preview, apps, owner);
}

/// Publishes to `preview` the box the draft would produce.
fn push_preview(
    draft: &RwSignal<Option<WidgetInstance>>,
    preview: &RwSignal<Option<(String, Placement)>>,
    apps: &RwSignal<Vec<AppUi>>,
    owner: &str,
) {
    let catalog = apps.get_untracked();
    if let Some(widget) = draft.get_untracked() {
        let placed = model::placement_of(&catalog, owner, &widget);
        preview.set(Some((widget.id.clone(), placed)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_with(widget: WidgetInstance) -> RwSignal<Option<WidgetInstance>> {
        RwSignal::new(Some(widget))
    }

    fn instance(id: &str) -> WidgetInstance {
        WidgetInstance {
            id: id.to_string(),
            ..Default::default()
        }
    }

    /// The title is on by default and its toggle preserves the text, so the
    /// editor can hide and restore the same heading.
    #[test]
    fn the_title_toggle_keeps_the_text() {
        let _owner = crate::test_support::owner();
        let draft = draft_with(instance("a"));
        assert!(show_title_value(draft).get_untracked());

        set_show_title(&draft, false);
        let widget = draft.get_untracked().expect("draft");
        assert_eq!(widget.show_title, Some(false));
        assert!(widget.label.is_none());

        set_show_title(&draft, true);
        assert_eq!(draft.get_untracked().expect("draft").show_title, Some(true));
    }

    /// Typing a subtitle enables it only when the editor had not decided yet;
    /// an explicit "off" survives the edit, and clearing the text falls back
    /// to the definition's description.
    #[test]
    fn typing_a_subtitle_enables_it_unless_explicitly_hidden() {
        let _owner = crate::test_support::owner();
        let draft = draft_with(instance("a"));
        set_subtitle(&draft, "En vivo".to_string());
        let widget = draft.get_untracked().expect("draft");
        assert_eq!(widget.subtitle.as_deref(), Some("En vivo"));
        assert_eq!(widget.show_subtitle, Some(true));

        let draft = draft_with(WidgetInstance {
            show_subtitle: Some(false),
            ..instance("b")
        });
        set_subtitle(&draft, "En vivo".to_string());
        assert_eq!(
            draft.get_untracked().expect("draft").show_subtitle,
            Some(false)
        );

        let draft = draft_with(WidgetInstance {
            subtitle: Some("Viejo".to_string()),
            ..instance("c")
        });
        set_subtitle(&draft, "   ".to_string());
        assert!(draft.get_untracked().expect("draft").subtitle.is_none());
    }
}
