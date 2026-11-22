//! The specs the form publishes into the shared top bar, rebuilt on every
//! state change over the callbacks [`FormActions`] created once.
//!
//! Nothing here creates a long-lived `Callback`: see the struct's docs for why
//! the creation site is load-bearing. The one exception is documented where it
//! happens (the declared actions, whose number and labels arrive with the
//! metadata) and matches what the list view already does for its view switcher.

use leptos::prelude::*;

use crate::components::desk::top_bar::{ButtonSpec, DropdownItem, DropdownSpec, TopBarAction};
use crate::components::ui::badge::BadgeVariant;
use crate::components::ui::button::ButtonVariant;
use crate::views::services::FormService;

use super::form_top_bar::FormActions;
use super::menu::FormMenuAction;

/// The form's actions, in Frappe's page-head order: the primary action first,
/// then the record navigation, then the "⋯" menu.
pub(super) fn form_actions(svc: &FormService, actions: &FormActions) -> Vec<TopBarAction> {
    // The layout editor is not a document: there is no state to submit, no
    // neighbour to walk to and nothing to delete, so the whole document bar is
    // replaced instead of being built and then disabled.
    if svc.layout_mode.get() {
        return layout_actions(svc, actions);
    }

    let mut out = Vec::new();

    if let Some(spec) = primary_action(svc, actions) {
        out.push(TopBarAction::Button(spec));
    }

    // Frappe hides navigation for a new document and for a Single.
    if !svc.is_new.get() && !svc.is_single.get() {
        out.extend(navigation(svc, actions));
    }

    // A new document has nothing to act on yet: Frappe hides the menu too, and
    // a layout edit of a form that has not been saved is not a workflow this
    // bar offers.
    if !svc.is_new.get() {
        out.push(TopBarAction::Dropdown(menu_spec(svc, actions)));
    }

    out
}

/// The layout editor's bar: undo/redo plus the menu.
///
/// Undo/redo are *buttons* and not menu rows on purpose. The mode's menu is
/// deliberately reduced to the entries that belong to the editor (see
/// [`edit_mode_items`]); the stack controls belong next to the editing, and
/// they have to show their disabled state without opening a menu.
///
/// They are also **only** here: the stacks are the editor's session (see
/// [`FormService::can_undo`]), so the document view renders neither of them.
fn layout_actions(svc: &FormService, actions: &FormActions) -> Vec<TopBarAction> {
    layout_history_buttons(svc, actions)
        .into_iter()
        .chain(std::iter::once(TopBarAction::Dropdown(DropdownSpec {
            label: String::new(),
            aria_label: Some("Más".to_string()),
            icon: Some("ellipsis"),
            items: edit_mode_items(actions),
            selectable: false,
        })))
        .collect()
}

/// Undo/redo for the layout operation stack, as buttons.
///
/// The disabled state is visible without opening a menu, and the pair only ever
/// touches customization writes, never the document. Both are always drawn in
/// the editor (disabled while there is nothing to undo) so the controls do not
/// appear and disappear as the user edits.
fn layout_history_buttons(svc: &FormService, actions: &FormActions) -> Vec<TopBarAction> {
    // Tracked reads: the bar's spec effect re-runs when a stack moves, which is
    // what flips the disabled state after an edit, an undo or a redo.
    let in_session = svc.layout_mode.get();
    let can_undo = in_session && !svc.undo_stack.get().is_empty();
    let can_redo = in_session && !svc.redo_stack.get().is_empty();

    [
        (
            can_undo,
            "rotate-ccw",
            "Deshacer cambio de diseño",
            actions.undo,
        ),
        (
            can_redo,
            "rotate-cw",
            "Rehacer cambio de diseño",
            actions.redo,
        ),
    ]
    .into_iter()
    .map(|(enabled, icon, label, on_click)| {
        TopBarAction::Button(ButtonSpec {
            label: String::new(),
            aria_label: Some(label.to_string()),
            icon: Some(icon),
            variant: ButtonVariant::Ghost,
            disabled: !enabled,
            on_click,
        })
    })
    .collect()
}

/// Frappe's state-driven primary action.
///
/// The precedence — Enviar → Guardar → Actualizar → Cancelar → Amendar — is
/// Frappe's `Toolbar.get_action_status`, and the order matters: a submittable
/// draft that is *clean* offers "Enviar", and the moment it is edited the same
/// slot flips to "Guardar", because a dirty document cannot be submitted. That
/// flip is what stops a submit from silently dropping unsaved edits.
fn primary_action(svc: &FormService, actions: &FormActions) -> Option<ButtonSpec> {
    let status = svc.docstatus.get();
    let dirty = svc.dirty.get();
    let is_new = svc.is_new.get();
    let saving = svc.saving.get();
    let submittable = svc.is_submittable.get();

    let (label, icon, on_click): (&str, Option<&'static str>, Callback<()>) =
        if submittable && status == 0 && !is_new && !dirty {
            ("Enviar", None, actions.submit)
        } else if status == 0 {
            ("Guardar", None, actions.save)
        } else if status == 1 && submittable && dirty {
            ("Actualizar", Some("pencil"), actions.save)
        } else if status == 1 && svc.allow_cancel.get() {
            ("Cancelar", None, actions.cancel)
        } else if status == 2 && svc.allow_amend.get() {
            ("Amendar", None, actions.amend)
        } else {
            return None;
        };

    Some(ButtonSpec {
        label: label.to_string(),
        aria_label: None,
        icon,
        variant: ButtonVariant::Default,
        // Inert while a mutation is in flight, so a second click cannot fire a
        // second write against the same document.
        disabled: saving,
        on_click,
    })
}

/// Frappe's record navigation: icon-only prev/next, disabled when there is no
/// neighbour (an only document, or the end of the list). The click resolves the
/// neighbour itself, so the button always walks the current ones.
fn navigation(svc: &FormService, actions: &FormActions) -> Vec<TopBarAction> {
    let neighbors = svc.neighbors.get();

    [
        (
            neighbors.prev.is_none(),
            "chevron-left",
            "Documento anterior",
            actions.prev,
        ),
        (
            neighbors.next.is_none(),
            "chevron-right",
            "Documento siguiente",
            actions.next,
        ),
    ]
    .into_iter()
    .map(|(disabled, icon, label, on_click)| {
        TopBarAction::Button(ButtonSpec {
            label: String::new(),
            aria_label: Some(label.to_string()),
            icon: Some(icon),
            variant: ButtonVariant::Ghost,
            disabled,
            on_click,
        })
    })
    .collect()
}

/// The "⋯" menu.
fn menu_spec(svc: &FormService, actions: &FormActions) -> DropdownSpec {
    DropdownSpec {
        label: String::new(),
        aria_label: Some("Más".to_string()),
        icon: Some("ellipsis"),
        items: menu_items(svc, actions),
        selectable: false,
    }
}

/// The entries available while the layout is being edited.
///
/// The document entries (sidebar toggle, email, reload, new, copy, duplicate,
/// remember-me) are noise in a canvas and, worse, a way to lose the edit at
/// best; what is left is the editor's own configuration ("Ajustes del DocType",
/// which used to be a card between the bar and the fields), how the edit leaves
/// the machine (fixtures) and how the user leaves the mode. Undo/redo are
/// buttons in the bar itself, not rows here.
fn edit_mode_items(actions: &FormActions) -> Vec<DropdownItem> {
    vec![
        DropdownItem::action("Ajustes del DocType", actions.doc_settings).with_icon("settings-2"),
        DropdownItem::separator(),
        DropdownItem::action("Exportar fixtures", actions.export).with_icon("download"),
        DropdownItem::separator(),
        DropdownItem::action("Salir del editor de diseño", actions.exit_edit).with_icon("log-out"),
    ]
}

/// The "⋯" menu entries, grouped with dividers the way Frappe groups its own.
fn menu_items(svc: &FormService, actions: &FormActions) -> Vec<DropdownItem> {
    if svc.editing.get() {
        return edit_mode_items(actions);
    }

    let (is_single, is_new, ds) = (svc.is_single.get(), svc.is_new.get(), svc.docstatus.get());

    let mut items = Vec::new();

    // Toggle the static form sidebar.
    items.push(DropdownItem::action("Barra lateral", actions.sidebar).with_icon("panel-right"));

    // Rename is a real engine operation (`allow_rename` + `Write`, validated in
    // `ferris-document`), so the entry is offered exactly when the DocType
    // permits it.
    if svc.allow_rename.get() {
        items.push(DropdownItem::action("Renombrar", actions.rename).with_icon("pencil"));
    }

    // Not backed by the backend yet: shown inert and named, so the menu never
    // offers a click that does nothing.
    items.push(
        DropdownItem::action("Email (no disponible)", actions.inert)
            .with_icon("mail")
            .disabled(),
    );

    // Reload from the source of truth.
    items.push(DropdownItem::action("Recargar", actions.reload).with_icon("refresh-cw"));

    // Delete, gated on the doctype's own permission and on the document being a
    // draft (a submitted document is cancelled, not deleted).
    if !is_single && !is_new && svc.allow_delete.get() && ds == 0 {
        items.push(
            DropdownItem::action("Eliminar", actions.delete)
                .with_icon("trash")
                .destructive(),
        );
    }

    // Duplicate, gated like the list view's own create affordance.
    if !is_single && !is_new && svc.allow_copy.get() && ds == 0 {
        items.push(DropdownItem::action("Duplicar", actions.duplicate).with_icon("copy"));
    }

    // New document of this doctype (a Single has exactly one).
    if !is_single {
        items.push(
            DropdownItem::action(format!("Nuevo {}", svc.doctype.get()), actions.new_doc)
                .with_icon("plus"),
        );
    }

    items.push(DropdownItem::separator());

    // Jump to a field: Frappe opens a picker dialog for this, and so do we (the
    // shared menu model has no nested submenu).
    items.push(DropdownItem::action("Ir a campo", actions.jump).with_icon("list"));

    // Show the linked documents in a dialog.
    items.push(DropdownItem::action("Mostrar vínculos", actions.links).with_icon("link"));

    // Copy the document as JSON.
    items.push(DropdownItem::action("Copiar al portapapeles", actions.copy_doc).with_icon("copy"));

    items.push(
        DropdownItem::action("Recordarme (no disponible)", actions.inert)
            .with_icon("bell")
            .disabled(),
    );

    items.push(DropdownItem::separator());

    // The one entry to the layout editor. It *navigates* to `/layout` instead
    // of flipping a runtime flag: the editor owns a URL, so the mode can be
    // linked, bookmarked and left with the browser.
    //
    // The DocType's own settings are **not** offered here: they describe the
    // form and its views, so they live in the editor that shows the form
    // (`Ajustes del DocType` in this menu while `/layout` is open). The old
    // "Ajustes" row navigated to the global `/settings` page, which is where a
    // user looking for form layout never found it.
    items.push(
        DropdownItem::action("Diseño del formulario", actions.layout_route)
            .with_icon("layout-panel-left"),
    );

    // The DocType's own actions, last because they are the most specific group.
    // They come from a memo, so a keystroke does not re-derive them.
    let declared = actions.declared.get();
    if !declared.is_empty() {
        items.push(DropdownItem::separator());
        for (index, action) in declared.into_iter().enumerate() {
            items.push(declared_item(action, index, actions));
        }
    }

    // Not backed by the backend yet.
    items.push(
        DropdownItem::action("Documentación (no disponible)", actions.inert)
            .with_icon("book-open")
            .disabled(),
    );

    items
}

/// One DocType-declared action as a menu row.
///
/// The row is rebuilt on every run — the metadata that declares it arrives after
/// the first paint — so, exactly like the list view's view switcher, its
/// callback belongs to the run that drew it and a click on a stale row is
/// *tolerated* rather than trusted: the bar invokes every action with
/// `try_run`. The built-ins keep their component-scoped callbacks.
///
/// While any declared action runs, all of them are disabled and the running one
/// is marked with a trailing ellipsis, so a second click cannot fire a second
/// server method before the first answer lands.
fn declared_item(action: FormMenuAction, index: usize, actions: &FormActions) -> DropdownItem {
    let running = actions.running_action.get();
    let label = if running == Some(index) {
        format!("{}…", action.label)
    } else {
        action.label.clone()
    };
    let on_click = Callback::new({
        let run = actions.run_action;
        move |_| run.run(index)
    });
    let item = DropdownItem::action(label, on_click).with_icon(action.icon);
    if running.is_some() {
        item.disabled()
    } else {
        item
    }
}

/// Map the form state to the indicator's label and variant.
///
/// `No guardado` wins over the stored `docstatus` while the form holds unsaved
/// changes (or is a not-yet-saved new document); otherwise the document's place
/// in its workflow is shown.
pub(super) fn status_of(docstatus: i64, dirty: bool, is_new: bool) -> (&'static str, BadgeVariant) {
    if dirty || (is_new && docstatus == 0) {
        return ("No guardado", BadgeVariant::Warning);
    }
    match docstatus {
        1 => ("Enviado", BadgeVariant::Success),
        2 => ("Cancelado", BadgeVariant::Destructive),
        _ => ("Borrador", BadgeVariant::Secondary),
    }
}

#[cfg(test)]
mod tests {
    use leptos::prelude::{Callable, Callback, Owner};

    /// `Callback` is a `StoredValue`: it dies with the owner that created it,
    /// and `try_run` reports that as `None` instead of panicking. Two decisions
    /// rest on this — the actions are created in the component body
    /// ([`super::FormActions`]), and the top bar invokes them with `try_run` —
    /// because the alternative was a click on a bar still drawing a disposed
    /// callback, which panicked the whole wasm module.
    #[test]
    fn a_callback_dies_with_its_owner_and_try_run_reports_it() {
        let owner = Owner::new();
        owner.set();
        let callback: Callback<()> = Callback::new(|_| ());
        assert_eq!(callback.try_run(()), Some(()));

        // What an effect re-run or a view teardown does to what the run owned.
        owner.cleanup();
        assert_eq!(callback.try_run(()), None);
    }

    /// The inversion `FormActions` relies on: a callback owned by the outer
    /// scope survives the inner run scopes being cleaned up, so hoisting the
    /// creation to the component body takes the disposal out of every state
    /// change.
    #[test]
    fn a_callbacks_outer_scope_survives_inner_run_cleanups() {
        let component = Owner::new();
        component.set();
        let callback: Callback<()> = Callback::new(|_| ());

        // A child owner, as each effect run's scope is.
        let run = Owner::new();
        run.set();
        run.cleanup();

        assert_eq!(callback.try_run(()), Some(()));

        component.cleanup();
        assert_eq!(callback.try_run(()), None);
    }
}
