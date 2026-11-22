//! Engine hook bridge: one global hook per (doctype, lifecycle point) that
//! forwards the event to the plugin store.

use std::collections::HashMap;
use std::sync::Arc;

use ferris_auth::User;
use ferris_core::Value;
use ferris_core::hooks::{DocumentHook, HookContext, HookPoint, HookRegistry};

use super::registry::PluginStore;

/// Map a guest rejection back to a framework error, preserving the typed kind
/// so a `before-*` validation the plugin raises reaches the caller as the HTTP
/// status it asked for (422 validation, 403 permission, …) instead of a blanket
/// 500. Traps and host failures stay internal.
fn plugin_error_to_core(err: &crate::WasmError) -> ferris_core::Error {
    use ferris_core::{AuthzError, ValidationError};
    match err {
        crate::WasmError::PluginError { kind, message, .. } => match kind {
            crate::PluginErrorKind::NotFound => {
                ferris_core::Error::Document(ferris_core::DocumentError::NotFound {
                    doctype: "plugin".to_string(),
                    name: message.clone(),
                })
            }
            crate::PluginErrorKind::PermissionDenied => {
                ferris_core::Error::Authz(AuthzError::PermissionDenied {
                    action: "doctype-event".to_string(),
                    resource: message.clone(),
                })
            }
            crate::PluginErrorKind::ValidationFailed => {
                ferris_core::Error::Validation(ValidationError::Constraint(message.clone()))
            }
            crate::PluginErrorKind::Conflict => {
                ferris_core::Error::Document(ferris_core::DocumentError::AlreadyExists {
                    doctype: "plugin".to_string(),
                    name: message.clone(),
                })
            }
            crate::PluginErrorKind::RateLimited(seconds) => {
                ferris_core::Error::Validation(ValidationError::Invalid(format!(
                    "rate limited, retry after {seconds}s: {message}"
                )))
            }
            crate::PluginErrorKind::Timeout => ferris_core::Error::Validation(
                ValidationError::Invalid(format!("plugin timed out: {message}")),
            ),
            crate::PluginErrorKind::Internal => {
                ferris_core::Error::Job(ferris_core::JobError::Failed(message.clone()))
            }
        },
        other => ferris_core::Error::Job(ferris_core::JobError::Failed(other.to_string())),
    }
}

/// Lifecycle points forwarded to the plugin, mapped to the kebab-case event
/// names the WIT world documents. Insert emits `before-insert` + `created`
/// (never `after-save`); update emits `before-save` + `after-save`. The
/// `before-*` entries are delivered synchronously so the guest's `error`
/// aborts the write; the rest observe it after the fact.
pub(super) const EVENT_POINTS: [(HookPoint, &str); 19] = [
    (HookPoint::BeforeNaming, "before-naming"),
    (HookPoint::BeforeValidate, "before-validate"),
    (HookPoint::Validate, "validate"),
    (HookPoint::BeforeInsert, "before-insert"),
    (HookPoint::AfterInsert, "created"),
    (HookPoint::BeforeSave, "before-save"),
    (HookPoint::AfterSave, "after-save"),
    (HookPoint::OnUpdate, "on-update"),
    (HookPoint::BeforeSubmit, "before-submit"),
    (HookPoint::OnSubmit, "submitted"),
    (HookPoint::BeforeCancel, "before-cancel"),
    (HookPoint::OnCancel, "cancelled"),
    (
        HookPoint::BeforeUpdateAfterSubmit,
        "before-update-after-submit",
    ),
    (HookPoint::OnUpdateAfterSubmit, "on-update-after-submit"),
    (HookPoint::OnChange, "on-change"),
    (HookPoint::BeforeDelete, "before-delete"),
    (HookPoint::OnTrash, "trashed"),
    (HookPoint::BeforeRename, "before-rename"),
    (HookPoint::AfterRename, "after-rename"),
];

/// Whether a guest error on this point must abort the write.
///
/// The name prefix is not enough: `validate` runs before the row is written and
/// is just as abortive as `before-*`. Every pre-write point is listed here, and
/// the plugin dispatch uses this one predicate instead of re-deriving it from
/// the event string.
pub(super) const fn aborts_on_error(point: HookPoint) -> bool {
    matches!(
        point,
        HookPoint::BeforeNaming
            | HookPoint::BeforeValidate
            | HookPoint::Validate
            | HookPoint::BeforeInsert
            | HookPoint::BeforeSave
            | HookPoint::BeforeSubmit
            | HookPoint::BeforeCancel
            | HookPoint::BeforeUpdateAfterSubmit
            | HookPoint::BeforeDelete
            | HookPoint::BeforeRename
    )
}

/// Owner tag for the plugin-forwarding hooks. `wire_engine_events` runs again on
/// every hot reload, so these entries must replace their previous self instead of
/// stacking: otherwise each reload adds a copy and every document event fires
/// once more per reload.
const PLUGIN_HOOK_TAG: &str = "ferris-plugin";

/// Register one global hook forwarding `doctype`/`point` to the plugin store.
pub(super) fn register_event_hook(
    store: Arc<PluginStore>,
    doctype: &str,
    point: HookPoint,
    event: &'static str,
) {
    let event = event.to_string();
    let pre = aborts_on_error(point);
    HookRegistry::replace_global(
        PLUGIN_HOOK_TAG,
        doctype.to_string(),
        point,
        Box::new(move |ctx: &mut HookContext, doc: &mut dyn DocumentHook| {
            // A `write-no-events` write persists the row but must not re-enter
            // the guest: the `before-*` handler would abort the write and the
            // `after-*` one would loop forever on the plugin's own row. The
            // writer marked the hook context; other global hooks are untouched.
            if ctx.suppress_events() {
                return Box::pin(async { Ok(()) });
            }
            let store = store.clone();
            let event = event.clone();
            let doctype = doc.doctype().to_string();
            let name = doc.name().to_string();
            let fields: HashMap<String, Value> = doc.get_fields().clone();
            // Resolve the operation's identity: prefer the full ambient user
            // (scoped by `ferris_document::with_user`); otherwise rebuild a
            // minimal principal from the engine's hook context. None → the
            // plugin runs anonymous (fail-closed).
            let user = ferris_document::current_user().or_else(|| {
                let id = ctx.user.trim();
                if id.is_empty() || id == "Administrator" {
                    None
                } else {
                    let mut u = User::guest();
                    u.id = id.to_string();
                    u.username = Some(id.to_string());
                    u.email = format!("{id}@local");
                    Some(u)
                }
            });
            Box::pin(async move {
                // `data` debe ser la forma canónica (claves ordenadas) que el
                // engine persiste y que `docstore::get` devuelve; si no, la
                // comparación exacta del plugin `test` ("stored data differs")
                // falla aunque el contenido sea idéntico, porque el orden de
                // iteración de un `HashMap` no es determinista entre
                // instancias. `serde_json::to_value` deja un `Map` ordenado.
                let canonical: serde_json::Value =
                    serde_json::to_value(Value::Object(fields)).unwrap_or_default();
                let data = serde_json::to_string(&canonical).unwrap_or_else(|_| "{}".to_string());
                // The plugin call is blocking; the engine may be driving us on
                // a tokio worker, so park the current thread out of the
                // scheduler while `Handle::current().block_on` inside the wasm
                // host imports keeps working.
                tokio::task::block_in_place(move || {
                    if pre {
                        store
                            .on_doctype_event_pre(&event, &doctype, &name, &data, user)
                            .map_err(|e| plugin_error_to_core(&e))
                    } else {
                        store
                            .on_doctype_event(&event, &doctype, &name, &data, user)
                            .map_err(|e| {
                                ferris_core::Error::Job(ferris_core::JobError::Failed(
                                    e.to_string(),
                                ))
                            })
                    }
                })?;
                Ok(())
            })
        }),
        0,
        None,
    );
}
