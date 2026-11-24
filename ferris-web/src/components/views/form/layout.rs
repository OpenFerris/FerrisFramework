//! Form view layout components.
//!
//! The layout is also where the form is *edited*: in edit mode every field gets
//! a click target (select it for the inspector), a hairline above it with a `+`
//! (create a field at that position) and a drag handle (move the field before
//! another one). All three affordances exist only while the [`FieldEditCtx`] is
//! present and editing, so the dashboard's parameter panel — which renders the
//! very same fields without that context — stays read-only.

use leptos::prelude::*;

use crate::components::ui::icon::Icon;
use crate::components::views::form::FieldEditCtx;
use crate::i18n::{t, use_i18n};
use crate::views::services::{FormField, FormSection, FormSectionItem};

/// Whether the field at `field` is visible, kept reactive to the sibling
/// fields its `depends_on` reads.
///
/// `fields.with` avoids cloning the whole field list on each keystroke; the
/// inner `value`/`checked` reads are what the memo actually subscribes to, so
/// only the referenced fields re-run it.
fn visible(fields: RwSignal<Vec<FormField>>, field: &FormField) -> Memo<bool> {
    let expression = field.depends_on.clone().unwrap_or_default();
    if expression.trim().is_empty() {
        return Memo::new(move |_| true);
    }
    Memo::new(move |_| {
        fields.with(|list| {
            crate::views::services::condition::evaluate(&expression, &|name| {
                list.iter()
                    .find(|candidate| candidate.fieldname == name)
                    .map(crate::views::services::condition::field_value)
            })
        })
    })
}

/// The `insert_after` anchor that creates a field *before* the field at
/// `index`: the previous field's name, or `__first__` when there is none.
///
/// Read from the flat field list, not from the section: the server applies
/// `insert_after` to the effective metadata, where a section is only a run of
/// that list, so the neighbour is the previous entry even across a section or
/// tab break.
fn anchor_before(fields: RwSignal<Vec<FormField>>, index: usize) -> String {
    if index == 0 {
        return "__first__".to_string();
    }
    fields
        .with_untracked(|list| list.get(index - 1).map(|prev| prev.fieldname.clone()))
        .unwrap_or_default()
}

/// A hairline between two fields with a `+` that appears on hover or keyboard
/// focus, and opens the create-field dialog anchored at that boundary.
///
/// Hidden outright outside edit mode: a form being read must not grow rules it
/// does not have. The component resolves the context itself so a host that
/// renders the fields without edit mode (the dashboard parameter panel) needs
/// no branch of its own.
#[component]
fn InsertFieldRule(#[prop(into)] anchor: String, #[prop(into)] label: String) -> impl IntoView {
    let edit = use_context::<FieldEditCtx>();
    let editing = Signal::derive(move || edit.is_some_and(|ctx| ctx.editing.get()));

    view! {
        <div
            class=move || {
                if editing.get() {
                    // Centred on the boundary (`-translate-y-1/2`), so the rule
                    // reads as "insert between these two fields". `z-20` keeps
                    // it above the field's own click-to-select target (`z-10`).
                    "group/insert absolute inset-x-0 top-0 z-20 flex h-3 -translate-y-1/2 items-center"
                } else {
                    "hidden"
                }
            }
        >
            <span class="pointer-events-none h-px flex-1 bg-border"></span>
            <button
                type="button"
                // Revealed by the strip's hover or by the button's own focus:
                // `pointer-events-none` while invisible keeps the field's click
                // target whole, and keyboard users still reach it with Tab.
                class="pointer-events-none flex size-5 shrink-0 items-center justify-center rounded-full border bg-card text-muted-foreground opacity-0 shadow-xs transition-opacity hover:bg-accent hover:text-accent-foreground focus-visible:pointer-events-auto focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none group-hover/insert:pointer-events-auto group-hover/insert:opacity-100"
                aria-label=label
                on:click=move |_| {
                    if let Some(ctx) = edit {
                        ctx.creating_after.set(anchor.clone());
                        ctx.creating.set(true);
                    }
                }
            >
                <Icon name="plus" class="size-3" />
            </button>
            <span class="pointer-events-none h-px flex-1 bg-border"></span>
        </div>
    }
}

/// The transparent target that covers one field in edit mode.
///
/// It is the field's click target (select it for the inspector) **and** its
/// drag handle: the whole surface can be dragged onto another field, which
/// writes an `insert_after` setter for the drop position. A keyboard user has
/// the same move through the panel's `insert_after` property, so the drag is an
/// accelerator, never the only way.
///
/// The class is one reactive expression (not `class:` toggles next to a reactive
/// `class`): two writers to one attribute would drop each other on the next
/// change.
#[component]
fn FieldTarget(name: String, label: String) -> impl IntoView {
    let i18n = use_i18n();
    let Some(ctx) = use_context::<FieldEditCtx>() else {
        return view! { <span></span> }.into_any();
    };
    let editing = ctx.editing;
    let selected = ctx.selected;
    let dragging = ctx.dragging;
    let drop_target = ctx.drop_target;
    let reorder = ctx.reorder;
    // `StoredValue` is `Copy`: the same name is read by the class and by every
    // DOM handler without being moved into one of them.
    let click_name = StoredValue::new(name.clone());
    let drag_name = StoredValue::new(name.clone());
    let drop_name = StoredValue::new(name.clone());
    let active_name = StoredValue::new(name);

    let class = move || {
        if !editing.get() {
            "hidden"
        } else if drop_target.get().as_deref() == Some(active_name.get_value().as_str()) {
            "absolute inset-0 z-10 cursor-grabbing rounded-md border-2 border-primary bg-primary/10 focus-visible:outline-none"
        } else if selected.get().as_deref() == Some(active_name.get_value().as_str()) {
            "absolute inset-0 z-10 cursor-grab rounded-md border-2 border-primary bg-primary/5 focus-visible:outline-none"
        } else {
            "absolute inset-0 z-10 cursor-grab rounded-md border border-dashed border-border bg-muted/20 hover:border-primary/40 focus-visible:border-primary focus-visible:outline-none"
        }
    };

    view! {
        <button
            type="button"
            data-name="FieldEditOverlay"
            data-fieldname=drag_name.get_value()
            draggable="true"
            class=class
            aria-label=i18n.t_with(
                "components.form.layout.edit_field",
                &[("label", &label)],
            )
            on:click=move |_| {
                // Selecting a field takes the column; there is no list to drop.
                selected.set(Some(click_name.get_value()));
            }
            on:dragstart=move |ev: web_sys::DragEvent| {
                let name = drag_name.get_value();
                dragging.set(Some(name.clone()));
                if let Some(data) = ev.data_transfer() {
                    let _ = data.set_data("text/plain", &name);
                }
                ev.stop_propagation();
            }
            on:dragover=move |ev: web_sys::DragEvent| {
                if dragging.get_untracked().is_none() {
                    return;
                }
                // Without this the browser refuses the drop and cancels the drag.
                ev.prevent_default();
                ev.stop_propagation();
                let target = drop_name.get_value();
                if drop_target.get_untracked().as_deref() != Some(target.as_str()) {
                    drop_target.set(Some(target));
                }
            }
            on:dragleave=move |_| {
                let target = drop_name.get_value();
                if drop_target.get_untracked().as_deref() == Some(target.as_str()) {
                    drop_target.set(None);
                }
            }
            on:drop=move |ev: web_sys::DragEvent| {
                ev.prevent_default();
                ev.stop_propagation();
                let target = drop_name.get_value();
                let dragged = dragging.get_untracked();
                dragging.set(None);
                drop_target.set(None);
                if let Some(dragged) = dragged
                    && dragged != target
                {
                    reorder.run((dragged, target));
                }
            }
            on:dragend=move |_| {
                dragging.set(None);
                drop_target.set(None);
            }
        ></button>
    }
    .into_any()
}

/// Render a form section (grid of fields).
#[component]
pub fn FormSectionView(
    section: FormSection,
    fields: RwSignal<Vec<FormField>>,
    render_field: impl Fn(&FormField) -> AnyView + 'static + Copy + Send + Sync,
) -> impl IntoView {
    let i18n = use_i18n();
    let cols = section.cols.max(1) as usize;
    // Edit mode: every field gets a transparent target that selects it for the
    // inspector. Forms that do not provide the context (the dashboard's
    // parameter panel renders the same fields) keep their normal controls.
    let edit = use_context::<FieldEditCtx>();
    // Fields keep their declaration order. Distributing them round-robin into
    // per-column buckets and letting CSS grid place the flattened result row by
    // row scrambled every multi-column section (with two columns the DOM read
    // 0, 2, 4, 1, 3, 5, so a phone's single column rendered exactly that order).
    // The grid fills row-major, so the declared order is the visual order.
    let heading = StoredValue::new(section.heading.clone());
    // Responsive on purpose: a four-column section was a fixed
    // `grid-template-columns: repeat(4, …)`, so on a phone every control was
    // squeezed to a quarter of the width. One column on small screens, the
    // declared count from `md` up.
    let grid_class = match cols {
        1 => "grid grid-cols-1 gap-4",
        2 => "grid grid-cols-1 gap-4 md:grid-cols-2",
        3 => "grid grid-cols-1 gap-4 md:grid-cols-3",
        _ => "grid grid-cols-1 gap-4 md:grid-cols-4",
    };
    // The last rendered entry of this section, used by the closing insert rule:
    // "after the last field" is a position too, and it is the only one that can
    // append instead of inserting between two entries. Tables render below the
    // fields grid, so a section that has one ends with its last table.
    let last_table = section.items.iter().rev().find_map(|item| match item {
        FormSectionItem::Table(index) => Some(*index),
        FormSectionItem::Field(_) => None,
    });
    let last_field = section.items.iter().rev().find_map(|item| match item {
        FormSectionItem::Field(index) => Some(*index),
        FormSectionItem::Table(_) => None,
    });
    let end_anchor = last_table.or(last_field).and_then(|index| {
        fields.with_untracked(|list| list.get(index).map(|field| field.fieldname.clone()))
    });

    view! {
        // A plain section, not a card: the form is full-size, so its parts are
        // separated by a hairline rule and a heading rather than nested cards.
        <section class="min-w-0 border-b py-4 last:border-b-0">
            <Show when=move || !heading.with_value(|h| h.is_empty())>
                <h3 class="mb-3 break-words text-sm font-semibold">{move || heading.with_value(String::clone)}</h3>
            </Show>
            <div class=grid_class>
                {section
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        FormSectionItem::Field(i) => Some(*i),
                        FormSectionItem::Table(_) => None,
                    })
                    .filter_map(|i| {
                    // The sections were built from an earlier snapshot. A
                    // metadata reload can leave an index pointing past the new
                    // field list; skip it instead of indexing a signal that no
                    // longer has that slot.
                    // A one-shot read while this section renders: the field
                    // list is not a tracking dependency here, and `.with` used
                    // to fire the "outside a reactive context" warning on every
                    // form render.
                    let field = fields.with_untracked(|fields| fields.get(i).cloned())?;
                    let is_visible = visible(fields, &field);
                    let rendered = render_field(&field);
                    let fieldname = field.fieldname.clone();
                    let name = field.fieldname;
                    let field_label = field.label;
                    let anchor = anchor_before(fields, i);
                    let insert_label = i18n.t_with(
                        "components.form.layout.insert_before",
                        &[("field", &field_label)],
                    );
                    let target = edit.map(move |_| {
                        view! { <FieldTarget name=name label=field_label /> }
                    });
                    Some(view! {
                        // `px-1 py-2`: the field keeps breathing room inside
                        // its cell, which is what the hairline above it hangs
                        // from and what makes the dashed target read as a box
                        // around the field instead of a frame on the control.
                        <div
                            class="relative min-w-0 px-1 py-2"
                            class:hidden=move || !is_visible.get()
                            data-fieldname=fieldname
                        >
                            <InsertFieldRule anchor=anchor label=insert_label />
                            {rendered}
                            {target}
                        </div>
                    })
                }).collect_view()}
            </div>
            {section
                .items
                .iter()
                .filter_map(|item| match item {
                    FormSectionItem::Table(i) => Some(*i),
                    FormSectionItem::Field(_) => None,
                })
                .filter_map(|i| {
                // One-shot snapshot while the section renders; the table owns
                // its rows afterwards. `untrack` establishes the reactive scope
                // the read needs, so it does not warn on every form render.
                let field = untrack(|| fields.with_untracked(|fields| fields.get(i).cloned()))?;
                let read_only = field.read_only;
                let field_label = field.label.clone();
                let anchor = anchor_before(fields, i);
                let insert_label = i18n.t_with(
                    "components.form.layout.insert_before",
                    &[("field", &field_label)],
                );
                let name = field.fieldname.clone();
                let tour_name = name.clone();
                let target = edit.map(|_| {
                    view! { <FieldTarget name=name.clone() label=field_label.clone() /> }
                });
                Some(view! {
                    <div class="relative mt-4 px-1" data-fieldname=tour_name>
                        <InsertFieldRule anchor=anchor label=insert_label />
                        <crate::components::views::form::child_table::ChildTableField
                            field=field
                            read_only=read_only
                        />
                        {target}
                    </div>
                }.into_any())
            }).collect_view()}
            // The last boundary of the section, so a field can be appended
            // where the user is looking instead of only between two entries.
            // `h-0`: outside edit mode the rule is hidden and this must not
            // leave a gap.
            {end_anchor.map(|anchor| view! {
                <div class="relative h-0">
                    <InsertFieldRule anchor=anchor label=t("components.form.layout.insert_at_end") />
                </div>
            })}
        </section>
    }
}

/// Render form tabs with sections.
///
/// `default_tab` is only ever read to seed the underlying tabs widget, which
/// owns the selection from then on. It used to be an `RwSignal<String>` that
/// was read once and dropped, which forced callers to mint a fresh signal per
/// render for a value that was never observed again.
///
/// `tab_labels`, `sections` and `default_tab` are signals, not values: the
/// form reads them while building the page, so a value would rebuild the whole
/// form (inspector included) every time a metadata reload produced a new list.
/// Here only the tab strip and the section children re-render.
#[component]
pub fn FormTabs(
    #[prop(into)] tab_labels: Signal<Vec<(String, String)>>,
    #[prop(into)] default_tab: Signal<String>,
    #[prop(into)] sections: Signal<Vec<FormSection>>,
    fields: RwSignal<Vec<FormField>>,
    render_field: impl Fn(&FormField) -> AnyView + 'static + Copy + Send + Sync,
) -> impl IntoView {
    use crate::components::ui::tabs::{Tabs, TabsContent, TabsList, TabsTrigger, TabsVariant};

    // Read once on purpose: the tabs widget owns the selection after the first
    // paint (see the doc comment above).
    let default_now = default_tab.get_untracked();
    let render_field_stored = StoredValue::new(render_field);

    view! {
        <Tabs id="form-tabs" default_value=default_now class="w-full">
            <Show when=move || { tab_labels.get().len() > 1 }>
                <TabsList variant=TabsVariant::Line class="w-full justify-start overflow-x-auto">
                    {move || tab_labels.get().into_iter().map(|(val, label)| {
                        view! { <TabsTrigger value=val>{label}</TabsTrigger> }
                    }).collect_view()}
                </TabsList>
            </Show>

            {move || {
                // Subscribe to the field list itself. A reorder (`insert_after`)
                // keeps the section shape identical — the item indices are
                // `0..n` either way — so the derived `sections` value compares
                // equal and would never repaint; the *names* are what moved.
                let _ = fields.with(Vec::len);
                let tabs = tab_labels.get();
                let sects = sections.get();
                let render_field = render_field_stored.get_value();
                tabs.into_iter().map(|(val, _)| {
                    let sects: Vec<FormSection> = sects
                        .iter()
                        .filter(|s| s.tab == val)
                        .cloned()
                        .collect();
                    view! {
                        <TabsContent value=val>
                            {move || {
                                sects.iter().map(|s| view! {
                                    <FormSectionView
                                        section=s.clone()
                                        fields=fields
                                        render_field=render_field
                                    />
                                }).collect_view()
                            }}
                        </TabsContent>
                    }
                }).collect_view()
            }}
        </Tabs>
    }
}
