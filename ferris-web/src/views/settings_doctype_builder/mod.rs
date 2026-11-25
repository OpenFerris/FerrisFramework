//! Settings → `DocType`s: the runtime `DocType` builder.
//!
//! Frappe's `DocType` builder creates metadata, not code. This screen is the
//! Desk's half of it:
//!
//! 1. the operator types the name of an existing `DocType` and loads it, or
//!    starts a new one;
//! 2. compiled declarations are read-only — changing them would disappear on
//!    the next boot — so the form is replaced by that message;
//! 3. a custom `DocType` is edited as name, module, child-table flag and a
//!    repeatable field list;
//! 4. **Save** sends the whole metadata body; creating it materializes the
//!    table, and the same screen continues as an edit.
//!
//! The warnings above the form state the schema's own semantics: an added field
//! is a nullable column, a removed field leaves its data, and `Table` needs a child.

mod draft;
mod field_editor;
mod labels;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::doctype_admin::{self, LoadedDocType};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::callout::{Callout, CalloutVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::toast_custom::_context::ToasterContext;
use crate::i18n::use_i18n;
use crate::views::views_components::{EmptyState, Feedback, FeedbackKind, LoadingLines};

use draft::{DocTypeDraft, FieldDraft};
use field_editor::FieldEditor;
use labels::{text, text_with};

/// The builder page. One screen, one draft, so every control edits the exact
/// body the next save sends.
#[allow(clippy::too_many_lines)]
#[allow(clippy::literal_string_with_formatting_args)]
#[allow(clippy::must_use_candidate)]
#[component]
pub fn DocTypeBuilder() -> impl IntoView {
    let i18n = use_i18n();
    let toaster = use_context::<ToasterContext>();

    let draft = RwSignal::new(DocTypeDraft::default());
    // Bound directly by the inputs; copied into the draft right before saving
    // so the text the user sees is the text that is validated.
    let name = RwSignal::new(String::new());
    let module = RwSignal::new(DocTypeDraft::default().module);
    let is_child_table = RwSignal::new(false);
    let lookup = RwSignal::new(String::new());
    let editing = RwSignal::new(None::<String>);
    let compiled = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);

    let load = Callback::new(move |()| {
        let requested = lookup.get_untracked().trim().to_string();
        if requested.is_empty() {
            error.set(Some(text(
                i18n,
                "views.doctype_builder.choose",
                "Type the name of a DocType to load.",
            )));
            return;
        }
        loading.set(true);
        error.set(None);
        notice.set(None);
        spawn_local(async move {
            let result = doctype_admin::load(&requested).await;
            loading.set(false);
            match result {
                Ok(LoadedDocType { custom: true, meta }) => {
                    let loaded_name = meta.name.clone();
                    name.set(loaded_name.clone());
                    module.set(meta.module.clone());
                    is_child_table.set(meta.is_child_table);
                    draft.set(DocTypeDraft::from_meta(meta));
                    editing.set(Some(loaded_name.clone()));
                    compiled.set(None);
                    lookup.set(loaded_name);
                    notice.set(Some(text_with(
                        i18n,
                        "views.doctype_builder.loaded",
                        "Loaded `{name}`.",
                        &[("name", &requested)],
                    )));
                }
                Ok(LoadedDocType { custom: false, .. }) => {
                    draft.set(DocTypeDraft::default());
                    name.set(String::new());
                    module.set(String::new());
                    is_child_table.set(false);
                    editing.set(None);
                    compiled.set(Some(requested));
                    notice.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
        });
    });

    let start_new = Callback::new(move |()| {
        draft.set(DocTypeDraft::default());
        name.set(String::new());
        module.set(DocTypeDraft::default().module);
        is_child_table.set(false);
        lookup.set(String::new());
        editing.set(None);
        compiled.set(None);
        error.set(None);
        notice.set(None);
    });

    let save = Callback::new(move |()| {
        if compiled.get_untracked().is_some() {
            return;
        }
        let mut current = draft.get_untracked();
        current.name = name.get_untracked();
        current.module = module.get_untracked();
        current.is_child_table = is_child_table.get_untracked();
        let target = editing.get_untracked();
        if let Some(target) = &target
            && current.name.trim() != target.as_str()
        {
            error.set(Some(text(
                i18n,
                "views.doctype_builder.rename_forbidden",
                "The name of an existing DocType cannot be changed here.",
            )));
            return;
        }
        let payload = match current.payload() {
            Ok(payload) => payload,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };

        let saved_message = if target.is_some() {
            text(
                i18n,
                "views.doctype_builder.saved",
                "DocType saved. The table and the registry are up to date.",
            )
        } else {
            text(
                i18n,
                "views.doctype_builder.created",
                "DocType created. Its table is ready for documents.",
            )
        };
        let toaster = toaster.clone();
        saving.set(true);
        error.set(None);
        notice.set(None);
        spawn_local(async move {
            let result = match target.as_deref() {
                Some(name) => doctype_admin::update(name, &payload).await,
                None => doctype_admin::create(&payload).await,
            };
            saving.set(false);
            match result {
                Ok(meta) => {
                    // The registry changed; the Desk's metadata cache must not
                    // keep serving the previous field list.
                    crate::api::client::clear_meta_cache();
                    let saved_name = meta.name.clone();
                    name.set(saved_name.clone());
                    module.set(meta.module.clone());
                    is_child_table.set(meta.is_child_table);
                    draft.set(DocTypeDraft::from_meta(meta));
                    editing.set(Some(saved_name));
                    compiled.set(None);
                    notice.set(Some(saved_message.clone()));
                    if let Some(toaster) = &toaster {
                        toaster.success(saved_message);
                    }
                }
                Err(message) => {
                    error.set(Some(message.clone()));
                    if let Some(toaster) = &toaster {
                        toaster.error(message);
                    }
                }
            }
        });
    });

    let change_field = Callback::new(move |field: FieldDraft| {
        draft.update(|draft| draft.replace_field(field));
    });
    let remove_field = Callback::new(move |id: u64| {
        draft.update(|draft| {
            draft.remove_field(id);
        });
    });
    let move_field = Callback::new(move |(id, delta): (u64, i32)| {
        draft.update(|draft| {
            draft.move_field(id, delta);
        });
    });
    let add_field = Callback::new(move |()| {
        draft.update(|draft| {
            draft.add_field();
        });
    });

    let busy = move || loading.get() || saving.get();
    let has_fields = Signal::derive(move || !draft.get().fields.is_empty());
    let compiled_notice = Signal::derive(move || {
        compiled.get().map(|name| {
            text_with(
                i18n,
                "views.doctype_builder.compiled",
                "`{name}` is declared in code, so it cannot be edited here. Only custom DocTypes are editable.",
                &[("name", &name)],
            )
        })
    });
    let editing_notice = Signal::derive(move || {
        editing.get().map(|name| {
            text_with(
                i18n,
                "views.doctype_builder.editing",
                "Editing `{name}`. The name is fixed; add or remove fields and save.",
                &[("name", &name)],
            )
        })
    });
    let name_readonly = Signal::derive(move || editing.get().is_some());
    let child_disabled = Signal::derive(move || editing.get().is_some());

    view! {
        <Card>
            <CardHeader>
                <CardTitle>
                    {text(i18n, "views.doctype_builder.title", "DocType builder")}
                </CardTitle>
                <CardDescription>
                    {text(
                        i18n,
                        "views.doctype_builder.description",
                        "Create a runtime DocType or edit a custom one. Requires System Manager or Administrator.",
                    )}
                </CardDescription>
            </CardHeader>
            <CardContent class="flex flex-col gap-4">
                <div class="flex flex-col gap-3 lg:flex-row lg:flex-wrap lg:items-end">
                    <div class="flex w-full min-w-0 flex-col gap-1 lg:w-64">
                        <label for="doctype-lookup" class="text-sm font-medium">
                            {text(
                                i18n,
                                "views.doctype_builder.lookup_label",
                                "Load an existing DocType",
                            )}
                        </label>
                        <Input
                            id="doctype-lookup"
                            r#type=InputType::Text
                            class="h-8"
                            bind_value=lookup
                            aria_label=text(
                                i18n,
                                "views.doctype_builder.lookup_label",
                                "Load an existing DocType",
                            )
                            placeholder=text(
                                i18n,
                                "views.doctype_builder.lookup_placeholder",
                                "e.g. Sales Invoice",
                            )
                        />
                    </div>
                    <div class="flex flex-wrap items-center gap-2">
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            attr:disabled=busy
                            on:click=move |_| load.run(())
                        >
                            {text(i18n, "views.doctype_builder.load", "Load")}
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            attr:disabled=busy
                            on:click=move |_| start_new.run(())
                        >
                            {text(i18n, "views.doctype_builder.new", "New DocType")}
                        </Button>
                    </div>
                </div>

                <Show when=move || loading.get()>
                    <LoadingLines lines=3 />
                </Show>

                <Feedback
                    kind=FeedbackKind::Error
                    message=Signal::derive(move || error.get())
                />
                <Feedback
                    kind=FeedbackKind::Success
                    message=Signal::derive(move || notice.get())
                />
                <Feedback kind=FeedbackKind::Warning message=compiled_notice />
                <Feedback kind=FeedbackKind::Info message=editing_notice />

                <div class="grid grid-cols-1 gap-2">
                    <Callout variant=CalloutVariant::Warning>
                        {text(
                            i18n,
                            "views.doctype_builder.warning_column",
                            "Adding a field adds a nullable column. Existing documents start empty; fill the field before making it required.",
                        )}
                    </Callout>
                    <Callout variant=CalloutVariant::Warning>
                        {text(
                            i18n,
                            "views.doctype_builder.warning_removal",
                            "Removing a field stops serving it, but its column and data stay in the database.",
                        )}
                    </Callout>
                    <Callout variant=CalloutVariant::Warning>
                        {text(
                            i18n,
                            "views.doctype_builder.warning_table",
                            "A Table field needs its child DocType created first; enter that DocType in Options.",
                        )}
                    </Callout>
                </div>

                <Show when=move || compiled.get().is_none()>
                    <div class="flex flex-col gap-4">
                        <div class="grid grid-cols-1 gap-3 lg:grid-cols-3">
                            <div class="flex min-w-0 flex-col gap-1">
                                <label for="doctype-name" class="text-sm font-medium">
                                    {text(
                                        i18n,
                                        "views.doctype_builder.name",
                                        "DocType name",
                                    )}
                                </label>
                                <Show
                                    when=move || name_readonly.get()
                                    fallback=move || {
                                        view! {
                                            <Input
                                                id="doctype-name"
                                                r#type=InputType::Text
                                                bind_value=name
                                                aria_label=text(
                                                    i18n,
                                                    "views.doctype_builder.name",
                                                    "DocType name",
                                                )
                                                placeholder="Invoice"
                                            />
                                        }
                                    }
                                >
                                    <Input
                                        id="doctype-name"
                                        r#type=InputType::Text
                                        bind_value=name
                                        readonly=true
                                        aria_label=text(
                                            i18n,
                                            "views.doctype_builder.name",
                                            "DocType name",
                                        )
                                    />
                                </Show>
                                <p class="text-xs text-muted-foreground">
                                    {text(
                                        i18n,
                                        "views.doctype_builder.name_hint",
                                        "Letters, digits, spaces and underscores. Names cannot be renamed later.",
                                    )}
                                </p>
                            </div>

                            <div class="flex min-w-0 flex-col gap-1">
                                <label for="doctype-module" class="text-sm font-medium">
                                    {text(
                                        i18n,
                                        "views.doctype_builder.module",
                                        "Module",
                                    )}
                                </label>
                                <Input
                                    id="doctype-module"
                                    r#type=InputType::Text
                                    bind_value=module
                                    aria_label=text(
                                        i18n,
                                        "views.doctype_builder.module",
                                        "Module",
                                    )
                                    placeholder="Custom"
                                />
                            </div>

                            <div class="flex flex-col gap-1">
                                <span class="text-sm font-medium">
                                    {text(
                                        i18n,
                                        "views.doctype_builder.child",
                                        "Child table",
                                    )}
                                </span>
                                <div class="flex items-start gap-3">
                                    <Checkbox
                                        id="doctype-child"
                                        class="mt-0.5"
                                        aria_label=text(
                                            i18n,
                                            "views.doctype_builder.child",
                                            "Child table",
                                        )
                                        aria_labelledby="doctype-child-label"
                                        checked=Signal::derive(move || is_child_table.get())
                                        disabled=child_disabled
                                        on_checked_change=Callback::new(move |value| {
                                            is_child_table.set(value);
                                        })
                                    />
                                    <div class="min-w-0">
                                        <span
                                            id="doctype-child-label"
                                            class="text-sm"
                                        >
                                            {text(
                                                i18n,
                                                "views.doctype_builder.child_hint",
                                                "Rows are stored under a parent document.",
                                            )}
                                        </span>
                                        <p class="text-xs text-muted-foreground">
                                            {text(
                                                i18n,
                                                "views.doctype_builder.child_fixed",
                                                "The child-table flag cannot be changed after creation.",
                                            )}
                                        </p>
                                    </div>
                                </div>
                            </div>
                        </div>

                        <div class="flex flex-col gap-3">
                            <div class="flex flex-wrap items-center justify-between gap-2">
                                <h3 class="text-sm font-semibold">
                                    {text(
                                        i18n,
                                        "views.doctype_builder.fields",
                                        "Fields",
                                    )}
                                </h3>
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    attr:disabled=move || saving.get()
                                    on:click=move |_| add_field.run(())
                                >
                                    {text(
                                        i18n,
                                        "views.doctype_builder.add_field",
                                        "Add field",
                                    )}
                                </Button>
                            </div>

                            <Show
                                when=move || has_fields.get()
                                fallback=move || {
                                    view! {
                                        <EmptyState
                                            icon="ListPlus"
                                            title=text(
                                                i18n,
                                                "views.doctype_builder.empty_title",
                                                "No fields yet",
                                            )
                                            description=text(
                                                i18n,
                                                "views.doctype_builder.empty_description",
                                                "Add the first field to describe what a document stores.",
                                            )
                                        >
                                            <Button
                                                variant=ButtonVariant::Default
                                                size=ButtonSize::Sm
                                                on:click=move |_| add_field.run(())
                                            >
                                                {text(
                                                    i18n,
                                                    "views.doctype_builder.add_first_field",
                                                    "Add the first field",
                                                )}
                                            </Button>
                                        </EmptyState>
                                    }
                                }
                            >
                                <div class="flex flex-col gap-3">
                                    <For
                                        each=move || draft.get().fields
                                        key=|field| field.id
                                        let:field
                                    >
                                        {
                                            let id = field.id;
                                            let index = Signal::derive(move || {
                                                draft
                                                    .get()
                                                    .fields
                                                    .iter()
                                                    .position(|row| row.id == id)
                                                    .unwrap_or(0)
                                            });
                                            let can_move_up = Signal::derive(move || {
                                                draft
                                                    .get()
                                                    .fields
                                                    .first()
                                                    .is_some_and(|row| row.id != id)
                                            });
                                            let can_move_down = Signal::derive(move || {
                                                draft
                                                    .get()
                                                    .fields
                                                    .last()
                                                    .is_some_and(|row| row.id != id)
                                            });
                                            view! {
                                                <FieldEditor
                                                    field=field
                                                    index=index
                                                    can_move_up=can_move_up
                                                    can_move_down=can_move_down
                                                    on_change=change_field
                                                    on_remove=remove_field
                                                    on_move=move_field
                                                    i18n=i18n
                                                />
                                            }
                                        }
                                    </For>
                                </div>
                            </Show>
                        </div>

                        <div class="flex flex-wrap items-center gap-2">
                            <Button
                                variant=ButtonVariant::Default
                                size=ButtonSize::Sm
                                attr:disabled=busy
                                on:click=move |_| save.run(())
                            >
                                {move || {
                                    if saving.get() {
                                        text(
                                            i18n,
                                            "views.doctype_builder.saving",
                                            "Saving…",
                                        )
                                    } else {
                                        text(i18n, "views.doctype_builder.save", "Save")
                                    }
                                }}
                            </Button>
                            <span class="text-xs text-muted-foreground">
                                {text(
                                    i18n,
                                    "views.doctype_builder.permissions_note",
                                    "Permission rules are preserved from the loaded DocType; a new DocType starts with none.",
                                )}
                            </span>
                        </div>
                    </div>
                </Show>
            </CardContent>
        </Card>
    }
}

// UI rendering is not unit-tested (`ferris-web` has no browser harness); the
// state machine lives in `draft` and the wire shapes in `api::doctype_admin`.
