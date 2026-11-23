//! The editable controls of the field inspector, grouped by concern.
//!
//! Split from `field.rs` so the file that owns the diff-and-save state machine
//! stays readable, and so the set of exposed properties has exactly one home.
//! Every control here maps to a property the engine applies: a control added
//! for anything else would write a setter that changes nothing.

use leptos::prelude::*;

use crate::api::types::Customization;
use crate::components::ui::select_native::SelectNative;
use crate::i18n::t;

use super::super::customize::FIELD_TYPES;
use super::props::{PropCheck, PropText};

/// The setter lookups the controls use for their "editado" markers.
///
/// A small value instead of a closure because Leptos props are per-component:
/// every control asks the same question (`does this field override X?`) and the
/// answer is read once, when the panel is rebuilt. `Copy` because the markup
/// uses it from several closures.
#[derive(Clone, Copy)]
pub(super) struct Overrides {
    pub custom: RwSignal<Option<Customization>>,
    pub fieldname: StoredValue<String>,
}

impl Overrides {
    pub(super) fn has(&self, property: &str) -> bool {
        let name = self.fieldname.get_value();
        self.custom.get_untracked().is_some_and(|custom| {
            custom.properties.iter().any(|setter| {
                setter.fieldname.as_deref() == Some(name.as_str()) && setter.property == property
            })
        })
    }

    /// Whether this field has *any* setter, which is what "Restablecer" undoes.
    pub(super) fn has_any(&self) -> bool {
        let name = self.fieldname.get_value();
        self.custom.get_untracked().is_some_and(|custom| {
            custom
                .properties
                .iter()
                .any(|setter| setter.fieldname.as_deref() == Some(name.as_str()))
        })
    }
}

/// One prop per editable property: a bag struct would hide which control owns
/// which signal, and the inspector passes them straight through.
#[allow(clippy::too_many_arguments)]
#[component]
pub(super) fn FieldPropertyInputs(
    fieldtype: RwSignal<String>,
    label: RwSignal<String>,
    description: RwSignal<String>,
    options: RwSignal<String>,
    default_value: RwSignal<String>,
    placeholder: RwSignal<String>,
    depends_on: RwSignal<String>,
    reqd: RwSignal<bool>,
    read_only: RwSignal<bool>,
    hidden: RwSignal<bool>,
    unique: RwSignal<bool>,
    in_list_view: RwSignal<bool>,
    in_standard_filter: RwSignal<bool>,
    insert_after: RwSignal<String>,
    overrides: Overrides,
    is_break: bool,
) -> impl IntoView {
    // A break keeps its label (a tab's displayed name, a section's heading) and
    // its position; the value-bearing properties mean nothing for it.
    // The declared vocabulary plus the field's current type: a compiled field
    // can be of a type this list does not curate (a child table, a heading),
    // and a select that silently dropped its own value would rewrite the field
    // on the first save.
    let type_choices = Signal::derive(move || {
        let current = fieldtype.get();
        let mut choices: Vec<String> = FIELD_TYPES.iter().map(|name| (*name).to_string()).collect();
        if !current.is_empty() && !choices.contains(&current) {
            choices.push(current);
        }
        choices
    });

    view! {
        <section class="flex flex-col gap-2" aria-label=t("components.form.edit_mode.section_content")>
            <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                {t("components.form.edit_mode.section_content")}
            </h4>
            <div class="flex flex-col gap-1">
                <div class="flex items-center justify-between gap-2">
                    <label class="text-xs font-medium text-muted-foreground" for="prop-tipo">
                        {t("components.form.edit_mode.type")}
                    </label>
                    <Show when=move || overrides.has("fieldtype")>
                        <span class="rounded bg-secondary px-1.5 py-0.5 text-xs text-secondary-foreground">
                            {t("components.form.edit_mode.edited")}
                        </span>
                    </Show>
                </div>
                <SelectNative
                    id="prop-tipo"
                    value=fieldtype.read_only()
                    aria_label=t("components.form.edit_mode.field_type")
                    on_change=Callback::new(move |event: leptos::ev::Event| {
                        fieldtype.set(event_target_value(&event));
                    })
                >
                    {move || {
                        type_choices
                            .get()
                            .into_iter()
                            .map(|name| {
                                let value = name.clone();
                                view! { <option value=value>{name}</option> }
                            })
                            .collect_view()
                    }}
                </SelectNative>
            </div>
            <PropText
                title=t("components.form.edit_mode.label")
                overridden=overrides.has("label")
                value=label
                hint=if is_break {
                    t("components.form.edit_mode.break_label_hint")
                } else {
                    String::new()
                }
            />
            <PropText
                title=t("components.form.edit_mode.description")
                overridden=overrides.has("description")
                value=description
                multiline=true
            />
            <Show when=move || !is_break>
                <PropText
                    title=t("components.form.edit_mode.options")
                    overridden=overrides.has("options")
                    value=options
                    multiline=true
                    hint=t("components.form.edit_mode.options_hint")
                />
                <PropText
                    title=t("components.form.edit_mode.default_value")
                    overridden=overrides.has("default")
                    value=default_value
                />
                <PropText
                    title=t("components.form.edit_mode.placeholder_field")
                    overridden=overrides.has("placeholder")
                    value=placeholder
                />
            </Show>
        </section>

        <section class="flex flex-col gap-2 border-t pt-3" aria-label=t("components.form.edit_mode.section_behavior")>
            <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                {t("components.form.edit_mode.section_behavior")}
            </h4>
            <Show when=move || !is_break>
                <PropText
                    title=t("components.form.edit_mode.visibility_condition")
                    overridden=overrides.has("depends_on")
                    value=depends_on
                    hint=t("components.form.edit_mode.visibility_hint")
                    example=t("components.form.edit_mode.depends_example")
                />
            </Show>
            <PropCheck title=t("components.form.edit_mode.required") overridden=overrides.has("reqd") value=reqd />
            <PropCheck
                title=t("components.form.edit_mode.read_only")
                overridden=overrides.has("read_only")
                value=read_only
            />
            <PropCheck title=t("components.form.edit_mode.hidden") overridden=overrides.has("hidden") value=hidden />
            <PropCheck title=t("components.form.edit_mode.unique") overridden=overrides.has("unique") value=unique />
        </section>

        <section class="flex flex-col gap-2 border-t pt-3" aria-label=t("components.form.edit_mode.section_listing")>
            <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                {t("components.form.edit_mode.section_listing")}
            </h4>
            <PropCheck
                title=t("components.form.edit_mode.in_list_view")
                overridden=overrides.has("in_list_view")
                value=in_list_view
            />
            <PropCheck
                title=t("components.form.edit_mode.in_filters")
                overridden=overrides.has("in_standard_filter")
                value=in_standard_filter
            />
        </section>

        <section class="flex flex-col gap-2 border-t pt-3" aria-label=t("components.form.edit_mode.section_order")>
            <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                {t("components.form.edit_mode.section_order")}
            </h4>
            <PropText
                title=t("components.form.edit_mode.insert_after")
                overridden=overrides.has("insert_after")
                value=insert_after
                hint=t("components.form.edit_mode.insert_after_hint")
                example=t("components.form.edit_mode.insert_after_example")
            />
        </section>
    }
}
