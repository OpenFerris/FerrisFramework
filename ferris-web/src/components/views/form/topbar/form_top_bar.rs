//! The [`FormTopBar`] component and the per-mount [`FormActions`] it
//! publishes into the shared bar.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::api::DocStore;
use crate::components::desk::top_bar::{
    TOP_BAR_OWNER_FORM, TopBarStatus, use_top_bar_actions, use_top_bar_doctype, use_top_bar_owner,
    use_top_bar_status,
};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::views::services::FormService;
use crate::views::services::form_service::{LAYOUT_SEGMENT, WriteOutcome};

use super::actions::{form_actions, status_of};
use super::dialogs::{JumpToFieldDialog, LinkedDocumentsDialog, copy_to_clipboard};
use super::export_dialog::ExportCustomizationDialog;
// Browser-only: the listeners do not exist on the SSR build and the calls are
// compiled out, so the imports follow them.
use super::leave_dialog::LeaveLayoutDialog;
#[cfg(target_arch = "wasm32")]
use super::leave_guard::install_leave_guard;
use super::menu::{FormMenuAction, FormMenuKind, menu_actions};
use super::rename_dialog::RenameDialog;
#[cfg(target_arch = "wasm32")]
use super::shortcuts::install_layout_shortcuts;
use super::toast::Toasts;
use crate::components::views::form::edit_mode::DocSettingsDialog;

/// The form's contribution to the shared top bar: a status indicator and the
/// mode-dependent actions. Renders only the dialogs the menu opens.
#[component]
pub fn FormTopBar(service: FormService) -> impl IntoView {
    let service = StoredValue::new(service);
    let svc = service.get_value();

    let top_actions = use_top_bar_actions();
    let top_status = use_top_bar_status();
    let top_owner = use_top_bar_owner();
    let top_doctype = use_top_bar_doctype();

    // Claim the bar: this is what mounts the form-scoped OpenUI anchors
    // (`form.toolbar.left`/`right`) in the shared top bar. Cleared on teardown.
    top_owner.set(Some(TOP_BAR_OWNER_FORM));

    // The toast host captured here, once: the actions run from click handlers
    // and from spawned tasks, where `use_context` has no owner to read.
    let toasts = Toasts::from_context();

    // The action callbacks, created here — once, under this component's owner.
    // `FormActions` documents why the creation site is load-bearing.
    let actions = FormActions::new(
        service,
        // `use_navigate` returns an `Fn + Clone` value, but it is neither
        // `Copy` nor usable by move from more than one closure. Wrapping it
        // once in a `Callback` (which is `Copy`) lets every action capture it.
        {
            let navigate_fn = use_navigate();
            Callback::new(move |(url, opts): (String, NavigateOptions)| {
                navigate_fn(&url, opts);
            })
        },
        toasts.clone(),
    );

    // The shell mounts those anchors doctype-filtered but cannot read the route
    // param itself (the bar is outside the matched `<Route>`), so publish the
    // form's own doctype. Reactive, because the same route serves every doctype.
    Effect::new(move |_| {
        top_doctype.set(service.get_value().doctype.get());
    });

    // Every completed write becomes one transient toast. The service reports a
    // verb plus an optional server message; the toast names it. Cleared as it is
    // read, so a remount cannot replay the previous answer.
    {
        let toasts = toasts.clone();
        Effect::new(move |_| {
            let s = service.get_value();
            let Some(outcome) = s.last_write.get() else {
                return;
            };
            s.last_write.set(None);
            match outcome {
                WriteOutcome::Succeeded(verb) => toasts.success(verb.done_message()),
                WriteOutcome::Failed(verb, message) => {
                    toasts.error(format!("{}: {message}", verb.error_prefix()));
                }
            }
        });
    }

    // Ctrl+Z / Ctrl+Shift+Z over the layout operation stack. `form_view.rs`
    // arranges the page, so the listener lives with the bar that owns the
    // buttons, in the browser only. It is inert outside the layout session.
    #[cfg(target_arch = "wasm32")]
    install_layout_shortcuts(service);
    // Leaving the editor with a non-empty history asks first — from the menu,
    // from a breadcrumb and from the browser's own unload. Installed once, it
    // reads the live state per event.
    #[cfg(target_arch = "wasm32")]
    install_leave_guard(service, actions.leave_open, actions.leave_target);

    // The linked documents the "Mostrar vínculos" dialog lists: every `Link`
    // field with a value names another document. Derived from the form fields,
    // so it follows the document the form is bound to.
    let linked = Memo::new(move |_| {
        service
            .get_value()
            .fields
            .get()
            .into_iter()
            .filter(|f| f.fieldtype == "Link")
            .filter_map(|f| {
                let target = f.options.clone().unwrap_or_default();
                let value = f.value.get();
                let value = value.trim().to_string();
                if target.is_empty() || value.is_empty() {
                    None
                } else {
                    Some((f.label.clone(), target, value))
                }
            })
            .collect::<Vec<_>>()
    });

    // ---- Status indicator (Frappe's page indicator, beside the breadcrumbs) ----
    Effect::new(move |_| {
        let s = service.get_value();
        // The layout editor has no document, so it has no docstatus either: a
        // "Borrador" badge there would describe a record that does not exist.
        if s.layout_mode.get() {
            top_status.set(None);
            return;
        }
        // A Single DocType has no lifecycle: it is never submitted, so the
        // "Borrador" badge described a state it can never leave. Keep the
        // indicator only for unsaved changes.
        if s.is_single.get() && !s.dirty.get() {
            top_status.set(None);
            return;
        }
        let (label, variant) = status_of(s.docstatus.get(), s.dirty.get(), s.is_new.get());
        top_status.set(Some(TopBarStatus {
            label: label.to_string(),
            variant,
        }));
    });

    // ---- Actions ----
    // Rebuilt whenever a signal the actions depend on changes: this effect
    // picks the *specs* for the current state and wraps the callbacks
    // `FormActions` already owns. It never creates a callback — anything it
    // created would be disposed on the next run, leaving the bar drawing a
    // handler that no longer exists.
    Effect::new(move |_| {
        let s = service.get_value();
        top_actions.set(form_actions(&s, &actions));
    });

    on_cleanup(move || {
        top_actions.set(Vec::new());
        top_status.set(None);
        top_owner.set(None);
        top_doctype.set(String::new());
    });

    // The settings dialog lives behind a `Show`; the closure must stay `Fn`, so
    // it reads a copyable handle instead of moving the shared service.
    let settings_service = StoredValue::new(svc.clone());
    let layout_open: Memo<bool> = {
        let layout_mode = svc.layout_mode;
        Memo::new(move |_| layout_mode.get())
    };
    view! {
        <LinkedDocumentsDialog open=actions.links_open module=svc.module linked=linked />
        <JumpToFieldDialog open=actions.jump_open fields=svc.fields />
        <ExportCustomizationDialog open=actions.export_open doctype=svc.doctype />
        <RenameDialog open=actions.rename_open service=svc.clone() />
        // The DocType's settings (and its per-view ones) are the layout
        // editor's dialog: the document view never opens it.
        <Show when=move || layout_open.get()>
            <DocSettingsDialog open=actions.doc_settings_open service=settings_service.get_value() />
        </Show>
        <LeaveLayoutDialog
            open=actions.leave_open
            target=actions.leave_target
            service=svc
        />
        <MenuActionError error=actions.action_error />
    }
}

/// The inline failure of a DocType-declared action.
///
/// The toast announces it (assertively, via the shared `role="alert"` toast),
/// and this row keeps it on screen while the menu is closed — a server method
/// can take seconds, and a message that vanished with the toast would leave the
/// user with nothing to read. It is dismissible: the next action clears it too.
#[component]
fn MenuActionError(error: RwSignal<Option<String>>) -> impl IntoView {
    view! {
        <Show when=move || error.get().is_some()>
            <div
                role="alert"
                data-name="FormMenuActionError"
                class="mt-2 flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
            >
                <Icon name="circle-alert" class="mt-0.5 size-4 shrink-0" />
                <span class="min-w-0 flex-1 break-words">
                    {move || error.get().unwrap_or_default()}
                </span>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::IconSm
                    attr:aria-label="Cerrar el aviso"
                    on:click=move |_| error.set(None)
                >
                    <Icon name="x" class="size-4" />
                </Button>
            </div>
        </Show>
    }
}

/// The form's action callbacks, created once per mount.
///
/// A `Callback` stores its closure in a `StoredValue` owned by the reactive
/// owner that is current at creation, and *every* owner run disposes what the
/// previous one owned: `Effect::new` executes each pass under
/// `Owner::with_cleanup`, and `Owner::cleanup` drops the run's arena values
/// (`reactive_graph` 0.2, `owner.rs`). Creating these inside the action effect
/// — where they logically belong — disposed them on every state change (a
/// keystroke flips `dirty` and re-runs the effect) while the bar still drew
/// the previous generation, so clicking the visible button read a disposed
/// `StoredValue` and panicked the whole wasm module.
///
/// Created here, they share the component's owner: they die exactly when
/// `FormTopBar` unmounts, which is the same teardown whose `on_cleanup` empties
/// the registry. "The slot holds live callbacks" then holds by construction
/// instead of by timing, and a re-run only re-derives the spec (label, icon,
/// disabled) around the same callbacks.
#[derive(Clone, Copy)]
pub(super) struct FormActions {
    /// The dialogs the "⋯" menu opens; owned here, next to the menu.
    pub(super) links_open: RwSignal<bool>,
    pub(super) jump_open: RwSignal<bool>,
    pub(super) rename_open: RwSignal<bool>,
    /// "Exportar fixtures" — the layout editor's only document-less write.
    pub(super) export_open: RwSignal<bool>,
    /// "Ajustes del DocType" — the layout editor's settings dialog.
    pub(super) doc_settings_open: RwSignal<bool>,
    /// Leaving the editor with layout edits in the session: the confirmation is
    /// open, and `leave_target` is where "Salir" goes.
    pub(super) leave_open: RwSignal<bool>,
    pub(super) leave_target: RwSignal<String>,

    pub(super) save: Callback<()>,
    pub(super) submit: Callback<()>,
    pub(super) cancel: Callback<()>,
    pub(super) amend: Callback<()>,
    pub(super) delete: Callback<()>,
    pub(super) duplicate: Callback<()>,
    pub(super) reload: Callback<()>,
    pub(super) copy_doc: Callback<()>,
    pub(super) sidebar: Callback<()>,
    pub(super) prev: Callback<()>,
    pub(super) next: Callback<()>,
    pub(super) new_doc: Callback<()>,
    pub(super) doc_settings: Callback<()>,
    pub(super) jump: Callback<()>,
    pub(super) links: Callback<()>,
    /// Navigate to the reserved `/layout` route: the layout editor, shareable.
    pub(super) layout_route: Callback<()>,
    /// Leave the field-edit / layout mode (navigates out on the layout route).
    pub(super) exit_edit: Callback<()>,
    pub(super) export: Callback<()>,
    pub(super) undo: Callback<()>,
    pub(super) redo: Callback<()>,
    pub(super) rename: Callback<()>,
    /// The DocType's own menu entries, derived once per metadata change.
    ///
    /// A memo, not a read of `svc.meta` in the spec effect: that effect re-runs
    /// on every keystroke (`dirty`), and cloning the whole `DynMeta` there for
    /// a two-row menu was a per-keystroke cost for nothing.
    pub(super) declared: Memo<Vec<FormMenuAction>>,
    /// Run the DocType action at this index (see the `declared` memo).
    pub(super) run_action: Callback<usize>,
    /// The declared action currently running, for the menu's disabled state.
    pub(super) running_action: RwSignal<Option<usize>>,
    /// The last declared action's failure, for the inline message.
    pub(super) action_error: RwSignal<Option<String>>,
    /// The shared no-op behind every entry the backend cannot back yet.
    pub(super) inert: Callback<()>,
}

impl FormActions {
    fn new(
        service: StoredValue<FormService>,
        navigate: Callback<(String, NavigateOptions)>,
        toasts: Toasts,
    ) -> Self {
        // A callback that runs one `FormService` method on the service this
        // component holds.
        fn over(service: StoredValue<FormService>, run: fn(&FormService)) -> Callback<()> {
            Callback::new(move |_| run(&service.get_value()))
        }

        let jump_open = RwSignal::new(false);
        let links_open = RwSignal::new(false);
        let rename_open = RwSignal::new(false);
        let export_open = RwSignal::new(false);
        let doc_settings_open = RwSignal::new(false);
        let leave_open = RwSignal::new(false);
        let leave_target = RwSignal::new(String::new());
        let running_action = RwSignal::new(None::<usize>);
        let action_error = RwSignal::new(None::<String>);
        let declared = Memo::new(move |_| menu_actions(service.get_value().meta.get().as_ref()));

        // The neighbours are resolved when the click happens, so prev/next
        // always walk the current neighbours rather than the ones the bar was
        // last rebuilt with.
        let step = move |next: bool| {
            let (service, navigate) = (service, navigate);
            Callback::new(move |_| {
                let svc = service.get_value();
                let neighbors = svc.neighbors.get_untracked();
                let target = if next { neighbors.next } else { neighbors.prev };
                if let Some(name) = target {
                    navigate.run((svc.route(&name), NavigateOptions::default()));
                }
            })
        };

        let new_doc = {
            let (service, navigate) = (service, navigate);
            Callback::new(move |_| {
                let svc = service.get_value();
                let url = format!(
                    "/desk/{}/{}/new",
                    svc.module.get_untracked(),
                    svc.doctype.get_untracked()
                );
                navigate.run((url, NavigateOptions::default()));
            })
        };

        let exit_edit = exit_edit_callback(service, navigate, leave_open, leave_target);
        let layout_route = layout_route_callback(service, navigate);
        // The declared actions are dispatched by *index* through one callback:
        // the list itself comes from the metadata, which arrives after the first
        // paint, so it cannot own a callback per row.
        let run_action = run_action_callback(
            service,
            declared,
            navigate,
            toasts.clone(),
            running_action,
            action_error,
        );

        let copy_doc = {
            let service = service;
            Callback::new(move |_| {
                let text = serde_json::to_string_pretty(&service.get_value().doc.get())
                    .unwrap_or_default();
                copy_to_clipboard(&text);
            })
        };

        let sidebar = {
            let service = service;
            // On a phone the sidebar is hidden by a per-device preference
            // (Ajustes → Móvil), so flipping only the runtime flag made the
            // menu item look dead: nothing appeared. The action drives the
            // preference there and keeps the flag in step; on a wide window it
            // keeps toggling the runtime flag.
            let prefs = crate::app::use_mobile_prefs();
            let is_mobile = crate::components::hooks::use_is_mobile::use_is_mobile();
            Callback::new(move |_| {
                let svc = service.get_value();
                if is_mobile.get_untracked() {
                    let next = !prefs.get_untracked().form_sidebar;
                    prefs.update(|p| p.form_sidebar = next);
                    svc.sidebar_visible.set(next);
                } else {
                    svc.sidebar_visible.update(|visible| *visible = !*visible);
                }
            })
        };

        Self {
            links_open,
            jump_open,
            export_open,
            doc_settings_open,
            leave_open,
            leave_target,
            save: over(service, FormService::save),
            submit: over(service, FormService::submit),
            cancel: over(service, FormService::cancel),
            amend: over(service, FormService::amend),
            delete: over(service, FormService::delete),
            duplicate: over(service, FormService::duplicate),
            reload: over(service, FormService::reload),
            copy_doc,
            sidebar,
            prev: step(false),
            next: step(true),
            new_doc,
            doc_settings: Callback::new(move |_| doc_settings_open.set(true)),
            rename_open,
            jump: {
                let jump_open = jump_open;
                Callback::new(move |_| jump_open.set(true))
            },
            links: {
                let links_open = links_open;
                Callback::new(move |_| links_open.set(true))
            },
            layout_route,
            exit_edit,
            export: {
                let export_open = export_open;
                Callback::new(move |_| export_open.set(true))
            },
            undo: over(service, FormService::undo_layout),
            redo: over(service, FormService::redo_layout),
            rename: {
                let rename_open = rename_open;
                Callback::new(move |_| rename_open.set(true))
            },
            declared,
            run_action,
            running_action,
            action_error,
            inert: Callback::new(|_| {}),
        }
    }
}

/// Leave the layout editor: the route has no document to return to, so leaving
/// it means leaving the page (back to the DocType's list).
///
/// With layout edits recorded this session, leaving asks first: the menu's
/// "Salir" opens the confirmation dialog instead of navigating, and the
/// confirmed target is resolved here (the dialog only navigates). With an empty
/// history there is nothing to warn about and the click leaves directly.
///
/// The cleared flags in the `else` branch are the defensive half — the service
/// already drops `editing` when the route stops being `/layout`.
fn exit_edit_callback(
    service: StoredValue<FormService>,
    navigate: Callback<(String, NavigateOptions)>,
    leave_open: RwSignal<bool>,
    leave_target: RwSignal<String>,
) -> Callback<()> {
    Callback::new(move |_| {
        let svc = service.get_value();
        if svc.layout_mode.get_untracked() {
            let url = format!(
                "/desk/{}/{}",
                crate::desk::session::path_segment(&svc.module.get_untracked()),
                crate::desk::session::path_segment(&svc.doctype.get_untracked())
            );
            if svc.has_layout_history() {
                leave_target.set(url);
                leave_open.set(true);
            } else {
                svc.discard_layout_history();
                navigate.run((url, NavigateOptions::default()));
            }
        } else {
            svc.editing.set(false);
            svc.selected_field.set(None);
        }
    })
}

/// Navigate to the reserved `/layout` route.
///
/// The route is the editor's address: entering the mode is a navigation, never
/// an in-place flag, so the editor can be linked and left with the browser.
fn layout_route_callback(
    service: StoredValue<FormService>,
    navigate: Callback<(String, NavigateOptions)>,
) -> Callback<()> {
    Callback::new(move |_| {
        let svc = service.get_value();
        let url = format!(
            "/desk/{}/{}/{}",
            crate::desk::session::path_segment(&svc.module.get_untracked()),
            crate::desk::session::path_segment(&svc.doctype.get_untracked()),
            LAYOUT_SEGMENT
        );
        navigate.run((url, NavigateOptions::default()));
    })
}

/// The one callback that runs a declared action, dispatched by index.
///
/// The index is resolved against the *same* memo the menu was painted from, so
/// the row that runs is the row that was clicked even after a rebuild.
fn run_action_callback(
    service: StoredValue<FormService>,
    declared: Memo<Vec<FormMenuAction>>,
    navigate: Callback<(String, NavigateOptions)>,
    toasts: Toasts,
    running: RwSignal<Option<usize>>,
    action_error: RwSignal<Option<String>>,
) -> Callback<usize> {
    Callback::new(move |index: usize| {
        run_declared_action(
            service,
            declared,
            index,
            navigate,
            toasts.clone(),
            running,
            action_error,
        );
    })
}

/// Execute one declared action, reporting it through the toast host and the
/// inline error row.
///
/// `declared` is the memo the menu was painted from, so the entry resolved here
/// is the entry that was drawn — a metadata update between paint and click
/// cannot make the click run a different action.
fn run_declared_action(
    service: StoredValue<FormService>,
    declared: Memo<Vec<FormMenuAction>>,
    index: usize,
    navigate: Callback<(String, NavigateOptions)>,
    toasts: Toasts,
    running: RwSignal<Option<usize>>,
    action_error: RwSignal<Option<String>>,
) {
    let Some(action) = declared.with_untracked(|actions| actions.get(index).cloned()) else {
        return;
    };
    action_error.set(None);
    match action.kind {
        // An in-app route: the browser does the work, there is nothing to wait
        // for and no failure to report.
        FormMenuKind::Navigate(href) => {
            let _ = navigate.try_run((href, NavigateOptions::default()));
        }
        FormMenuKind::Widget { app, widget } => {
            // A widget belongs to an app's world; a declaration that names no
            // app means "the one this form is in".
            let app = app.unwrap_or_else(|| {
                service
                    .try_with_value(|svc| svc.module.get_untracked())
                    .unwrap_or_default()
            });
            let href = format!(
                "/plugins/{}/widget/{}",
                crate::desk::session::path_segment(&app),
                crate::desk::session::path_segment(&widget)
            );
            let _ = navigate.try_run((href, NavigateOptions::default()));
        }
        FormMenuKind::Method { name } => {
            running.set(Some(index));
            let label = action.label;
            spawn_local(async move {
                let result = match DocStore::runtime() {
                    Ok(store) => store.call_method(&name, serde_json::json!({})).await,
                    Err(message) => Err(message),
                };
                running.set(None);
                match result {
                    // No reload on purpose: the parameters are empty, so the
                    // method cannot assume a document, and a reload would throw
                    // away unsaved field edits for a call that may only read.
                    Ok(_) => toasts.success(format!("Acción «{label}» ejecutada.")),
                    Err(message) => {
                        action_error.set(Some(message.clone()));
                        toasts.error(format!("No se pudo ejecutar «{label}»: {message}"));
                    }
                }
            });
        }
    }
}
