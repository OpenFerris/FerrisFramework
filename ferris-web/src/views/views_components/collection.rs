//! Shared chrome for the collection views (list, kanban, calendar, tree, gantt,
//! report).
//!
//! Every collection view is a *peer*: the same doctype, the same module, one
//! URL per mode. Without a shared action builder each view would publish its own
//! top bar — some with a switcher, some without — and the user could reach a
//! board with no way back to the list. This module publishes one bar:
//!
//! * the **view switcher** (only the modes the doctype declares),
//! * **refresh**, which re-fetches the view's own data,
//! * **create**, offered only when the doctype grants it.
//!
//! It is a function rather than a wrapper component because the calling view
//! owns its own state and layout; only the action registry is shared.

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_navigate, use_params_map};

use crate::WebUser;
use crate::api::{DynMeta, ViewSpec, can_create, view_modes};
use crate::components::desk::top_bar::{ButtonSpec, TopBarAction, use_top_bar_actions};
use crate::components::ui::button::ButtonVariant;
use crate::desk::session::path_segment;

/// The configured field names a view cannot resolve against the effective
/// metadata.
///
/// The write boundary refuses a setter that names an unknown field, so this only
/// happens when the field is removed afterwards (a customization deleted, a
/// compiled doctype changed under a stored setter). Saying so is not optional:
/// the renderer would otherwise fall back to another field and the user would
/// see a board grouped by something they never chose.
#[must_use]
pub fn unresolved_fields(meta: &DynMeta, configured: &[(&str, Option<&str>)]) -> Vec<String> {
    configured
        .iter()
        .filter_map(|(label, field)| {
            let field = (*field).filter(|field| !field.is_empty())?;
            (!meta.fields.iter().any(|found| found.fieldname == field))
                .then(|| format!("{label}: el campo «{field}» no existe en este DocType."))
        })
        .collect()
}

/// One banner sentence for [`unresolved_fields`], or `None` when every
/// configured field resolves.
#[must_use]
pub fn stale_settings_message(unresolved: &[String]) -> Option<String> {
    (!unresolved.is_empty()).then(|| {
        format!(
            "La configuración de esta vista apunta a campos que ya no existen. {}",
            unresolved.join(" ")
        )
    })
}

/// Publish the collection chrome for `view`.
///
/// `meta` is the effective metadata (the switch re-derives when it lands),
/// `on_reload` is the view's own refresh (a page/board/form re-fetch, never a
/// full browser reload), and `view` is the mode this page renders so the
/// switcher can name the current one.
pub fn use_collection_actions(
    meta: impl Into<Signal<Option<DynMeta>>>,
    view: &'static str,
    on_reload: Callback<()>,
) {
    let meta = meta.into();
    let params = use_params_map();
    let doctype = Signal::derive(move || params.with(|p| p.get("doctype").unwrap_or_default()));
    let module = Signal::derive(move || params.with(|p| p.get("module").unwrap_or_default()));
    let navigate_fn = use_navigate();
    let navigate: Callback<(String, NavigateOptions)> =
        Callback::new(move |(url, opts): (String, NavigateOptions)| {
            navigate_fn(&url, opts);
        });
    let top_actions = use_top_bar_actions();
    let user = use_context::<RwSignal<WebUser>>();

    Effect::new(move |_| {
        let Some(current) = meta.get() else {
            top_actions.set(Vec::new());
            return;
        };
        if current.is_single {
            top_actions.set(Vec::new());
            return;
        }
        let doctype = doctype.get();
        let module = if current.module.is_empty() {
            module.get()
        } else {
            current.module.clone()
        };
        let specs: Vec<ViewSpec> = view_modes(&current, &module, &doctype);
        let mut out: Vec<TopBarAction> = Vec::new();
        if let Some(switcher) = TopBarAction::view_switcher(&specs, view, navigate) {
            out.push(switcher);
        }
        out.push(TopBarAction::Button(ButtonSpec {
            label: String::new(),
            aria_label: Some("Recargar".to_string()),
            icon: Some("RefreshCw"),
            variant: ButtonVariant::Outline,
            disabled: false,
            on_click: on_reload,
        }));

        let roles = user
            .map(|user| user.with(|u| u.roles.clone()))
            .unwrap_or_default();
        if can_create(&current, &roles) {
            let url = format!(
                "/desk/{}/{}/new",
                path_segment(&module),
                path_segment(&doctype)
            );
            out.push(TopBarAction::Button(ButtonSpec {
                label: "Nuevo".to_string(),
                aria_label: Some("Crear documento".to_string()),
                icon: Some("Plus"),
                variant: ButtonVariant::Default,
                disabled: false,
                on_click: Callback::new(move |()| {
                    let _ = navigate.try_run((url.clone(), NavigateOptions::default()));
                }),
            }));
        }
        top_actions.set(out);
    });

    on_cleanup(move || top_actions.set(Vec::new()));
}

#[cfg(test)]
mod tests {
    use super::{stale_settings_message, unresolved_fields};
    use crate::api::{DynField, DynMeta};

    fn meta(fields: &[&str]) -> DynMeta {
        DynMeta {
            name: "Task".to_string(),
            fields: fields
                .iter()
                .map(|name| DynField {
                    fieldname: (*name).to_string(),
                    fieldtype: "Data".to_string(),
                    ..DynField::default()
                })
                .collect(),
            ..DynMeta::default()
        }
    }

    /// A configured field the metadata still declares is not a warning; one it
    /// dropped is named, with the setting that points at it.
    #[test]
    fn only_missing_configured_fields_are_reported() {
        let resolved = unresolved_fields(
            &meta(&["status", "parent_task"]),
            &[
                ("Agrupación", Some("status")),
                ("Padre", Some("parent_task")),
            ],
        );
        assert!(resolved.is_empty(), "{resolved:?}");

        let unresolved = unresolved_fields(
            &meta(&["status"]),
            &[
                ("Agrupación", Some("status")),
                ("Padre", Some("parent_task")),
            ],
        );
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].contains("parent_task"));
        assert!(unresolved[0].contains("Padre"));
    }

    /// An unset setting is not a stale one, and a list with nothing to report
    /// produces no banner at all.
    #[test]
    fn unset_settings_and_empty_reports_stay_silent() {
        assert!(unresolved_fields(&meta(&[]), &[("Padre", None), ("Fecha", Some(""))]).is_empty());
        assert_eq!(stale_settings_message(&[]), None);
        assert!(
            stale_settings_message(&["Padre: el campo «x» no existe.".to_string()])
                .is_some_and(|message| message.contains("Padre"))
        );
    }
}
