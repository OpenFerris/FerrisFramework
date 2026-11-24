//! The `DocType`'s own settings, as a dialog of the layout editor.
//!
//! These are the properties that apply to the whole form rather than to one
//! field, plus the ones a collection view reads. They used to sit in a
//! collapsed "Ajustes del formulario" card between the top bar and the fields,
//! which put a rarely used form between the user and the canvas; the editor's
//! `⋯` menu opens them in a dialog now, next to the other mode-level actions.
//!
//! Every input maps to a property the engine **applies**
//! (`ferris_schema::customization::apply_doctype_property`), and only those are
//! rendered — see [`views`] for the per-view rule. A save writes each changed
//! property as a `Property Setter` (deleting it when the value was cleared, so
//! "empty" means "the declared value again"), records the inverse in the layout
//! session's undo stack, and reports through the same toast channel as the
//! other layout writes.
//!
//! The dialog loads the `DocType`'s setters before mounting the form: the
//! "editado" markers and the undo entries need the value each property is
//! about to replace, and reading them *before* the write is what makes undo
//! restore the exact previous state instead of deleting a customization the
//! user never removed.

mod field_picker;
mod views;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::PropertySetterSpec;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::icon::Icon;
use crate::components::ui::skeleton::Skeleton;
use crate::components::ui::tabs::{Tabs, TabsContent, TabsList, TabsTrigger, TabsVariant};
use crate::i18n::{t, use_i18n};
use crate::views::services::FormService;
use crate::views::services::form_service::{LayoutOp, WriteOutcome, WriteVerb};

use super::props::PropText;
use views::{DefaultViewPicker, ViewKind, ViewSettings, available_views};

/// The dialog: a header, and the settings form mounted while it is open.
///
/// The form is built per opening, so it always seeds from the metadata the
/// editor is showing; a closed dialog costs nothing.
#[component]
pub(crate) fn DocSettingsDialog(service: FormService, open: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let dialog_i18n = i18n;
    // `StoredValue` is `Copy`: the loader and the panel are built at different
    // times but address the same service.
    let service = StoredValue::new(service);
    let setters = RwSignal::new(None::<Vec<PropertySetterSpec>>);
    let load_error = RwSignal::new(None::<String>);
    let reload = Callback::new(move |()| load_setters(service, setters, load_error, i18n));

    // Opening is the moment the contents have to be true, and the moment the
    // previous values (for undo) have to be read.
    Effect::new(move |_| {
        if open.get() {
            reload.run(());
        } else {
            setters.set(None);
            load_error.set(None);
        }
    });

    let doctype = service.get_value().doctype;

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-2xl">
                <DialogHeader>
                    <DialogTitle>{t("components.form.menu.doctype_settings")}</DialogTitle>
                    <DialogDescription>
                        {move || {
                            dialog_i18n.t_with(
                                "components.form.settings.description",
                                &[("doctype", &doctype.get())],
                            )
                        }}
                    </DialogDescription>
                </DialogHeader>

                <Show when=move || open.get()>
                    {move || match (setters.get(), load_error.get()) {
                        (_, Some(message)) => view! {
                            <div class="mt-3 flex flex-col gap-3">
                                <div
                                    class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                                    role="alert"
                                >
                                    {message}
                                </div>
                                <div class="flex justify-end">
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        on:click=move |_| reload.run(())
                                    >
                                        {t("components.form.gallery.retry")}
                                    </Button>
                                </div>
                            </div>
                        }
                        .into_any(),
                        (Some(setters), None) => view! {
                            <DocSettingsPanel
                                service=service.get_value()
                                setters=setters
                                on_close=Callback::new(move |()| open.set(false))
                            />
                        }
                        .into_any(),
                        (None, None) => view! {
                            <div class="mt-3 flex flex-col gap-2" aria-busy="true">
                                <Skeleton class="h-9 w-full" />
                                <Skeleton class="h-24 w-full" />
                                <Skeleton class="h-9 w-40 self-end" />
                            </div>
                        }
                        .into_any(),
                    }}
                </Show>
            </DialogContent>
        </Dialog>
    }
}

/// Read the `DocType`'s own setters (`fieldname` empty) into `setters`.
fn load_setters(
    service: StoredValue<FormService>,
    setters: RwSignal<Option<Vec<PropertySetterSpec>>>,
    error: RwSignal<Option<String>>,
    i18n: crate::i18n::I18n,
) {
    let doctype = service
        .try_with_value(|svc| svc.doctype.get_untracked())
        .unwrap_or_default();
    if doctype.is_empty() {
        setters.set(Some(Vec::new()));
        return;
    }
    setters.set(None);
    error.set(None);
    spawn_local(async move {
        let Ok(store) = DocStore::runtime() else {
            error.set(Some(i18n.t("components.form.settings.unavailable")));
            return;
        };
        match store.get_customization(&doctype).await {
            Ok(found) => {
                let own = found
                    .properties
                    .into_iter()
                    .filter(|setter| setter.fieldname.is_none())
                    .collect();
                setters.set(Some(own));
            }
            Err(message) => error.set(Some(message)),
        }
    });
}

/// The form of the dialog: the view selector, the view's settings and the
/// global ones.
#[component]
fn DocSettingsPanel(
    service: FormService,
    /// The setters in force when the dialog opened.
    setters: Vec<PropertySetterSpec>,
    on_close: Callback<()>,
) -> impl IntoView {
    let panel_i18n = use_i18n();
    let view_labels_i18n = panel_i18n;
    let doctype = service.doctype;
    let meta = service.meta;
    // The metadata the panel is seeded from: one snapshot, so a reload that
    // lands while the dialog is open cannot rewrite what the fields compare
    // against. `get_untracked` because this is a mount-time read.
    let seed = meta.get_untracked();
    let draft = SettingsDraft::from_meta(seed.as_ref());
    // What each property looked like when the dialog opened: the diff at save
    // time is "what the user changed", not "what differs from the engine".
    let baseline = StoredValue::new(draft.values());

    let views = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map_or_else(|| vec![ViewKind::List], available_views)
    });
    let first_view = views
        .get_untracked()
        .first()
        .map_or_else(|| ViewKind::List.value(), |view| view.value())
        .to_string();

    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let service_store = StoredValue::new(service);
    let setters_store = StoredValue::new(setters);

    let save = Callback::new(move |()| {
        let changes = pending_changes(&baseline.get_value(), &draft.values());
        if changes.is_empty() {
            on_close.run(());
            return;
        }
        let previous: Vec<(String, Option<String>)> = changes
            .iter()
            .map(|(property, _, _)| {
                let found = setters_store.get_value();
                let previous = found
                    .iter()
                    .find(|setter| setter.property == *property)
                    .map(|setter| setter.value.clone());
                ((*property).to_string(), previous)
            })
            .collect();
        let dt = doctype.get_untracked();
        let service = service_store.get_value();
        error.set(None);
        saving.set(true);
        let i18n = panel_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                saving.set(false);
                error.set(Some(i18n.t("components.form.settings.unavailable")));
                return;
            };
            let mut failure = None;
            for ((property, value, clear), (_, previous)) in changes.into_iter().zip(previous) {
                let result = if clear {
                    store.delete_property_setter(&dt, None, property).await
                } else {
                    store
                        .put_property_setter(
                            &dt,
                            &PropertySetterSpec {
                                fieldname: None,
                                property: property.to_string(),
                                value: value.clone(),
                            },
                        )
                        .await
                        .map(|_| ())
                };
                if let Err(message) = result {
                    failure = Some(message);
                    break;
                }
                // Recorded only after the server accepted it: an undo must
                // never delete a setter that was never written.
                service.record_layout_op(LayoutOp::PropertySet {
                    fieldname: None,
                    property: property.to_string(),
                    previous,
                    value,
                });
            }
            saving.set(false);
            if let Some(message) = failure {
                error.set(Some(message.clone()));
                service
                    .last_write
                    .set(Some(WriteOutcome::Failed(WriteVerb::Settings, message)));
            } else {
                // The effective metadata changed: the form re-reads it and the
                // next opening of this dialog seeds from the new one.
                service.reload.update(|n| *n += 1);
                service
                    .last_write
                    .set(Some(WriteOutcome::Succeeded(WriteVerb::Settings)));
                on_close.run(());
            }
        });
    });

    view! {
        <div class="mt-3 flex flex-col gap-4">
            <Tabs id="doctype-settings-tabs" default_value=first_view class="w-full">
                <TabsList variant=TabsVariant::Line class="w-full justify-start overflow-x-auto">
                    {move || {
                        views
                            .get()
                            .into_iter()
                            .map(|view| view! {
                                <TabsTrigger value=view.value().to_string()>
                                    <Icon name=view.icon() class="size-4" />
                                    {view.label(&view_labels_i18n)}
                                </TabsTrigger>
                            })
                            .collect_view()
                    }}
                </TabsList>
                {move || {
                    views
                        .get()
                        .into_iter()
                        .map(|view| view! {
                            <TabsContent value=view.value().to_string()>
                                <div class="flex flex-col gap-4 pt-1 pb-2">
                                    <ViewSettings
                                        kind=view
                                        meta=Signal::derive(move || meta.get())
                                        draft=draft
                                    />
                                </div>
                            </TabsContent>
                        })
                        .collect_view()
                }}
            </Tabs>

            // Global: applies to the form itself, not to a collection view, so
            // it stays visible whichever tab is selected.
            <section
                class="flex flex-col gap-3 border-t pt-4"
                aria-label=t("components.form.settings.general_aria")
            >
                <h4 class="flex items-center gap-2 text-sm font-semibold">
                    <Icon name="settings-2" class="size-4 text-muted-foreground" />
                    {t("components.form.settings.general")}
                </h4>
                <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
                    <PropText
                        title=t("components.form.settings.title_field")
                        overridden=false
                        value=draft.title_field
                        hint=t("components.form.settings.title_field_hint")
                    />
                    <PropText
                        title=t("components.form.settings.naming_series")
                        overridden=false
                        value=draft.naming_series
                        hint=t("components.form.settings.naming_series_hint")
                    />
                    <DefaultViewPicker draft=draft meta=Signal::derive(move || meta.get()) />
                </div>
                <PropText
                    title=t("components.form.settings.doctype_description")
                    overridden=false
                    value=draft.description
                    multiline=true
                />
            </section>

            <Show when=move || error.get().is_some()>
                <p
                    class="rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive"
                    role="alert"
                >
                    {move || error.get().unwrap_or_default()}
                </p>
            </Show>

            <div class="flex justify-end gap-2 border-t pt-3">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    on:click=move |_| on_close.run(())
                >
                    {t("common.cancel")}
                </Button>
                <Button
                    variant=ButtonVariant::Default
                    size=ButtonSize::Sm
                    attr:disabled=move || saving.get()
                    on:click=move |_| save.run(())
                >
                    {move || {
                        if saving.get() {
                            t("components.form.settings.saving")
                        } else {
                            t("components.form.settings.save")
                        }
                    }}
                </Button>
            </div>
        </div>
    }
}

/// The settings the form shows, one signal per input.
///
/// `Copy` because the panel hands the same draft to every tab: the inputs of
/// the selected view and the global section are two readers of one state. It is
/// a bundle of `Copy` handles, so passing it by value is the point; the
/// components that take it say so with a targeted `allow`.
#[derive(Clone, Copy)]
pub(super) struct SettingsDraft {
    pub title_field: RwSignal<String>,
    pub naming_series: RwSignal<String>,
    pub description: RwSignal<String>,
    pub sort_field: RwSignal<String>,
    pub sort_order: RwSignal<String>,
    pub search_fields: RwSignal<String>,
    pub date_field: RwSignal<String>,
    pub default_view: RwSignal<String>,
    pub kanban_group_field: RwSignal<String>,
    pub kanban_card_fields: RwSignal<String>,
    pub calendar_end_field: RwSignal<String>,
    pub calendar_title_field: RwSignal<String>,
    pub tree_parent_field: RwSignal<String>,
    pub tree_label_field: RwSignal<String>,
    pub tree_expanded_levels: RwSignal<String>,
    pub gantt_start_field: RwSignal<String>,
    pub gantt_end_field: RwSignal<String>,
    pub gantt_dependency_field: RwSignal<String>,
    pub gantt_progress_field: RwSignal<String>,
}

impl SettingsDraft {
    /// Seed every input from the effective metadata.
    fn from_meta(meta: Option<&crate::api::DynMeta>) -> Self {
        Self {
            title_field: RwSignal::new(
                meta.and_then(|m| m.title_field.clone()).unwrap_or_default(),
            ),
            naming_series: RwSignal::new(meta.map(|m| m.naming_series.clone()).unwrap_or_default()),
            description: RwSignal::new(
                meta.and_then(|m| m.description.clone()).unwrap_or_default(),
            ),
            sort_field: RwSignal::new(meta.and_then(|m| m.sort_field.clone()).unwrap_or_default()),
            sort_order: RwSignal::new(meta.and_then(|m| m.sort_order.clone()).unwrap_or_default()),
            search_fields: RwSignal::new(
                meta.map(|m| m.search_fields.join(", ")).unwrap_or_default(),
            ),
            date_field: RwSignal::new(meta.and_then(|m| m.date_field.clone()).unwrap_or_default()),
            default_view: RwSignal::new(
                meta.and_then(|m| m.default_view)
                    .map(|view| view.segment().to_string())
                    .unwrap_or_default(),
            ),
            kanban_group_field: RwSignal::new(
                meta.and_then(|m| m.kanban_group_field.clone())
                    .unwrap_or_default(),
            ),
            kanban_card_fields: RwSignal::new(
                meta.map(|m| m.kanban_card_fields.join(", "))
                    .unwrap_or_default(),
            ),
            calendar_end_field: RwSignal::new(
                meta.and_then(|m| m.calendar_end_field.clone())
                    .unwrap_or_default(),
            ),
            calendar_title_field: RwSignal::new(
                meta.and_then(|m| m.calendar_title_field.clone())
                    .unwrap_or_default(),
            ),
            tree_parent_field: RwSignal::new(
                meta.and_then(|m| m.tree_parent_field.clone())
                    .unwrap_or_default(),
            ),
            tree_label_field: RwSignal::new(
                meta.and_then(|m| m.tree_label_field.clone())
                    .unwrap_or_default(),
            ),
            tree_expanded_levels: RwSignal::new(
                meta.and_then(|m| m.tree_expanded_levels)
                    .map(|levels| levels.to_string())
                    .unwrap_or_default(),
            ),
            gantt_start_field: RwSignal::new(
                meta.and_then(|m| m.gantt_start_field.clone())
                    .unwrap_or_default(),
            ),
            gantt_end_field: RwSignal::new(
                meta.and_then(|m| m.gantt_end_field.clone())
                    .unwrap_or_default(),
            ),
            gantt_dependency_field: RwSignal::new(
                meta.and_then(|m| m.gantt_dependency_field.clone())
                    .unwrap_or_default(),
            ),
            gantt_progress_field: RwSignal::new(
                meta.and_then(|m| m.gantt_progress_field.clone())
                    .unwrap_or_default(),
            ),
        }
    }

    /// The draft as `(property, value)` pairs: exactly the properties
    /// `apply_doctype_property` applies (a name missing from there would store
    /// a value nothing reads).
    fn values(&self) -> Vec<(&'static str, String)> {
        vec![
            ("title_field", self.title_field.get_untracked()),
            ("naming_series", self.naming_series.get_untracked()),
            ("description", self.description.get_untracked()),
            ("sort_field", self.sort_field.get_untracked()),
            ("sort_order", self.sort_order.get_untracked()),
            ("search_fields", self.search_fields.get_untracked()),
            ("date_field", self.date_field.get_untracked()),
            ("default_view", self.default_view.get_untracked()),
            (
                "kanban_group_field",
                self.kanban_group_field.get_untracked(),
            ),
            (
                "kanban_card_fields",
                self.kanban_card_fields.get_untracked(),
            ),
            (
                "calendar_end_field",
                self.calendar_end_field.get_untracked(),
            ),
            (
                "calendar_title_field",
                self.calendar_title_field.get_untracked(),
            ),
            ("tree_parent_field", self.tree_parent_field.get_untracked()),
            ("tree_label_field", self.tree_label_field.get_untracked()),
            (
                "tree_expanded_levels",
                self.tree_expanded_levels.get_untracked(),
            ),
            ("gantt_start_field", self.gantt_start_field.get_untracked()),
            ("gantt_end_field", self.gantt_end_field.get_untracked()),
            (
                "gantt_dependency_field",
                self.gantt_dependency_field.get_untracked(),
            ),
            (
                "gantt_progress_field",
                self.gantt_progress_field.get_untracked(),
            ),
        ]
    }
}

/// The writes a save performs: every property whose draft value differs from
/// the one the dialog opened with, with `clear` set when the user emptied it.
///
/// Pure so the "empty means delete the setter" rule is testable without a
/// server: clearing an input restores the declared value instead of writing an
/// empty override the engine would then apply.
fn pending_changes(
    before: &[(&'static str, String)],
    after: &[(&'static str, String)],
) -> Vec<(&'static str, String, bool)> {
    after
        .iter()
        .filter_map(|(property, value)| {
            let previous = before
                .iter()
                .find(|(name, _)| name == property)
                .map(|(_, value)| value.as_str())
                .unwrap_or_default();
            (previous != value.as_str())
                .then(|| (*property, value.clone(), value.trim().is_empty()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pending_changes;

    fn pairs(values: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        values
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect()
    }

    #[test]
    fn an_untouched_form_writes_nothing() {
        let before = pairs(&[("title_field", "titulo"), ("sort_order", "asc")]);
        assert!(pending_changes(&before, &before).is_empty());
    }

    #[test]
    fn a_changed_value_is_written_and_an_emptied_one_clears_its_setter() {
        let before = pairs(&[("title_field", "titulo"), ("sort_field", "creation")]);
        let after = pairs(&[("title_field", "asunto"), ("sort_field", "")]);
        assert_eq!(
            pending_changes(&before, &after),
            vec![
                ("title_field", "asunto".to_string(), false),
                ("sort_field", String::new(), true)
            ]
        );
    }

    /// A property the payload does not carry at all (an older dialog, a new
    /// property) is compared against the empty string, so leaving it empty is
    /// not a write.
    #[test]
    fn a_property_missing_from_the_baseline_is_only_written_when_it_has_a_value() {
        let before = pairs(&[]);
        assert!(pending_changes(&before, &pairs(&[("date_field", "")])).is_empty());
        assert_eq!(
            pending_changes(&before, &pairs(&[("date_field", "fecha")])),
            vec![("date_field", "fecha".to_string(), false)]
        );
    }
}
