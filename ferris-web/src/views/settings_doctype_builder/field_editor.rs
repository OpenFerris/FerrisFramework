//! One field row of the `DocType` builder.
//!
//! The row owns local signals so typing does not rebuild the list; every change
//! is published to the parent as a whole [`FieldDraft`], which keeps the pure
//! model the single source of truth for saving.

use leptos::prelude::*;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::select_native::SelectNative;
use crate::i18n::I18n;

use super::draft::{FIELD_TYPES, FieldDraft, needs_options};
use super::labels::{text, text_with};

/// One row of the repeatable field editor.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::literal_string_with_formatting_args)]
#[component]
pub(super) fn FieldEditor(
    field: FieldDraft,
    #[prop(into)] index: Signal<usize>,
    #[prop(into)] can_move_up: Signal<bool>,
    #[prop(into)] can_move_down: Signal<bool>,
    on_change: Callback<FieldDraft>,
    on_remove: Callback<u64>,
    on_move: Callback<(u64, i32)>,
    i18n: I18n,
) -> impl IntoView {
    let id = field.id;
    let fieldname = RwSignal::new(field.fieldname.clone());
    let label = RwSignal::new(field.label.clone());
    let fieldtype = RwSignal::new(field.fieldtype.clone());
    let options = RwSignal::new(field.options.clone());
    let reqd = RwSignal::new(field.reqd);
    let unique = RwSignal::new(field.unique);
    let in_list_view = RwSignal::new(field.in_list_view);
    let extra = StoredValue::new(field.extra);

    let on_move = StoredValue::new(on_move);
    let on_remove = StoredValue::new(on_remove);

    // One place publishes the row: every bound signal is read here, so a
    // keystroke, a checkbox and a select all reach the draft the same way.
    Effect::new(move |_| {
        let _ = fieldname.get();
        let _ = label.get();
        let _ = fieldtype.get();
        let _ = options.get();
        let _ = reqd.get();
        let _ = unique.get();
        let _ = in_list_view.get();
        on_change.run(FieldDraft {
            id,
            fieldname: fieldname.get_untracked(),
            fieldtype: fieldtype.get_untracked(),
            label: label.get_untracked(),
            options: options.get_untracked(),
            reqd: reqd.get_untracked(),
            unique: unique.get_untracked(),
            in_list_view: in_list_view.get_untracked(),
            extra: extra.get_value(),
        });
    });

    let shows_options = Signal::derive(move || needs_options(&fieldtype.get()));
    let name_id = format!("doctype-field-{id}-fieldname");
    let type_id = format!("doctype-field-{id}-fieldtype");
    let label_id = format!("doctype-field-{id}-label");
    let options_id = format!("doctype-field-{id}-options");
    let reqd_id = format!("doctype-field-{id}-reqd");
    let unique_id = format!("doctype-field-{id}-unique");
    let list_id = format!("doctype-field-{id}-in-list-view");

    let type_options: Vec<String> = FIELD_TYPES.iter().map(ToString::to_string).collect();

    view! {
        <div class="flex flex-col gap-3 rounded-lg border border-border bg-background p-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
                <h4 class="text-sm font-medium">
                    {move || {
                        text_with(
                            i18n,
                            "views.doctype_builder.field_heading",
                            "Field {index}",
                            &[("index", &(index.get() + 1).to_string())],
                        )
                    }}
                </h4>
                <div class="flex flex-wrap items-center gap-1">
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        attr:disabled=move || !can_move_up.get()
                        on:click=move |_| on_move.with_value(|callback| callback.run((id, -1)))
                    >
                        {text(i18n, "views.doctype_builder.move_up", "Move up")}
                    </Button>
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        attr:disabled=move || !can_move_down.get()
                        on:click=move |_| on_move.with_value(|callback| callback.run((id, 1)))
                    >
                        {text(i18n, "views.doctype_builder.move_down", "Move down")}
                    </Button>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| on_remove.with_value(|callback| callback.run(id))
                    >
                        {text(i18n, "views.doctype_builder.remove_field", "Remove")}
                    </Button>
                </div>
            </div>

            <div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
                <div class="flex min-w-0 flex-col gap-1">
                    <label for=name_id.clone() class="text-xs font-medium">
                        {text(i18n, "views.doctype_builder.field_fieldname", "Fieldname")}
                    </label>
                    <Input
                        id=name_id
                        r#type=InputType::Text
                        class="h-8"
                        bind_value=fieldname
                        aria_label=text(
                            i18n,
                            "views.doctype_builder.field_fieldname",
                            "Fieldname",
                        )
                        placeholder="title"
                    />
                </div>
                <div class="flex min-w-0 flex-col gap-1">
                    <label for=type_id.clone() class="text-xs font-medium">
                        {text(i18n, "views.doctype_builder.field_fieldtype", "Type")}
                    </label>
                    <SelectNative
                        id=type_id
                        class="h-8"
                        aria_label=text(i18n, "views.doctype_builder.field_fieldtype", "Type")
                        value=fieldtype.read_only()
                        on_change=Callback::new(move |event: leptos::ev::Event| {
                            fieldtype.set(event_target_value(&event));
                        })
                    >
                        {type_options
                            .into_iter()
                            .map(|name| {
                                let value = name.clone();
                                view! { <option value=value>{name}</option> }
                            })
                            .collect_view()}
                    </SelectNative>
                </div>
                <div class="flex min-w-0 flex-col gap-1">
                    <label for=label_id.clone() class="text-xs font-medium">
                        {text(i18n, "views.doctype_builder.field_label", "Label")}
                    </label>
                    <Input
                        id=label_id
                        r#type=InputType::Text
                        class="h-8"
                        bind_value=label
                        aria_label=text(i18n, "views.doctype_builder.field_label", "Label")
                    />
                </div>
                <div class="flex min-w-0 flex-col gap-1">
                    <label for=list_id.clone() class="text-xs font-medium">
                        {text(
                            i18n,
                            "views.doctype_builder.field_in_list_view",
                            "List column",
                        )}
                    </label>
                    <div class="flex h-8 items-center">
                        <Checkbox
                            id=list_id
                            aria_label=text(
                                i18n,
                                "views.doctype_builder.field_in_list_view",
                                "List column",
                            )
                            aria_labelledby=format!("{id}-list-label")
                            checked=Signal::derive(move || in_list_view.get())
                            on_checked_change=Callback::new(move |value| {
                                in_list_view.set(value);
                            })
                        />
                        <span id=format!("{id}-list-label") class="ms-2 text-sm">
                            {text(
                                i18n,
                                "views.doctype_builder.field_in_list_view_hint",
                                "Show in list view",
                            )}
                        </span>
                    </div>
                </div>
            </div>

            <Show when=move || shows_options.get()>
                <div class="flex min-w-0 flex-col gap-1">
                    <label for=options_id.clone() class="text-xs font-medium">
                        {text(i18n, "views.doctype_builder.field_options", "Options")}
                    </label>
                    <Input
                        id=options_id.clone()
                        r#type=InputType::Text
                        class="h-8"
                        bind_value=options
                        aria_label=text(
                            i18n,
                            "views.doctype_builder.field_options",
                            "Options",
                        )
                        aria_describedby=format!("{id}-options-hint")
                    />
                    <p id=format!("{id}-options-hint") class="text-xs text-muted-foreground">
                        {text(
                            i18n,
                            "views.doctype_builder.field_options_hint",
                            "Comma-separated values for Select; a DocType name for Link or Table.",
                        )}
                    </p>
                </div>
            </Show>

            <div class="flex flex-wrap items-center gap-4">
                <div class="flex items-center gap-2">
                    <Checkbox
                        id=reqd_id
                        aria_label=text(i18n, "views.doctype_builder.field_reqd", "Required")
                        checked=Signal::derive(move || reqd.get())
                        on_checked_change=Callback::new(move |value| {
                            reqd.set(value);
                        })
                    />
                    <span class="text-sm">
                        {text(i18n, "views.doctype_builder.field_reqd", "Required")}
                    </span>
                </div>
                <div class="flex items-center gap-2">
                    <Checkbox
                        id=unique_id
                        aria_label=text(i18n, "views.doctype_builder.field_unique", "Unique")
                        checked=Signal::derive(move || unique.get())
                        on_checked_change=Callback::new(move |value| {
                            unique.set(value);
                        })
                    />
                    <span class="text-sm">
                        {text(i18n, "views.doctype_builder.field_unique", "Unique")}
                    </span>
                </div>
            </div>
        </div>
    }
}
