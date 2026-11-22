//! The DOM mount plumbing behind `UiSlot` and `UiHeaderSlot`: create the
//! declared custom elements inside the anchor's shadow root, adopt the
//! declaring apps' CSS, and stamp the live header props.

#[cfg(any(target_arch = "wasm32", test))]
use crate::app::Workspace;

#[cfg(target_arch = "wasm32")]
use super::assets::component_tag;
#[cfg(target_arch = "wasm32")]
use super::slots::SLOT_SHELL_HEADER;
#[cfg(target_arch = "wasm32")]
use super::{AppUi, UiExtension};

/// Mount the custom elements declared for `shell.header` into the trigger
/// anchor's shadow root, replacing whatever was mounted before (a manifest
/// refresh). Only shapes the anchor's *set* of elements: the workspace and
/// sidebar props are stamped live by [`stamp_header_props`], so a prop change
/// never rebuilds the plugins' elements.
#[cfg(target_arch = "wasm32")]
pub(super) fn mount_header_slot(
    anchor: Option<web_sys::HtmlDivElement>,
    exts: &[(String, UiExtension)],
    apps: &[AppUi],
) {
    let Some(anchor) = anchor else {
        return;
    };
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(shadow) = crate::ext::loader::shadow_root(&anchor) else {
        return;
    };
    let Some(container) = crate::ext::loader::mount_container(&shadow) else {
        return;
    };

    while let Some(child) = container.first_child() {
        let _ = container.remove_child(&child);
    }

    let mut owners: Vec<&str> = Vec::new();
    for (owner, ext) in exts {
        if !owners.contains(&owner.as_str()) {
            owners.push(owner.as_str());
        }
        let tag = component_tag(&ext.component);
        let Ok(el) = document.create_element(&tag) else {
            continue;
        };
        let _ = el.set_attribute("data-slot", SLOT_SHELL_HEADER);
        if let Some(label) = &ext.label {
            let _ = el.set_attribute("data-label", label);
        }
        let _ = el.set_attribute("data-component", &ext.component);
        let _ = container.append_child(el.as_ref());
    }

    let urls = crate::ext::loader::stylesheet_urls_for_owners(apps, &owners);
    crate::ext::loader::adopt(&shadow, &urls);
}

/// Stamp the live header props on the mounted elements: the sidebar state, the
/// active module's fields and the `data-props` JSON (the same contract slots
/// and widgets use). Runs on every workspace or sidebar change; the elements
/// themselves stay mounted, so a plugin keeps its internal state.
#[cfg(target_arch = "wasm32")]
pub(super) fn stamp_header_props(
    anchor: Option<web_sys::HtmlDivElement>,
    workspace: Option<&Workspace>,
    open: bool,
) {
    use wasm_bindgen::JsCast;

    let Some(anchor) = anchor else {
        return;
    };
    let Some(shadow) = anchor.shadow_root() else {
        return;
    };
    let Some(container) = crate::ext::loader::mount_container(&shadow) else {
        return;
    };

    let props = header_props_json(workspace, open);
    let open_attr = if open { "true" } else { "false" };
    // The container's children are exactly the mounted header elements (the
    // `<style>` fallbacks live directly under the shadow root, not here).
    let mut child = container.first_child();
    while let Some(node) = child {
        if let Some(el) = node.dyn_ref::<web_sys::Element>() {
            let _ = el.set_attribute("data-props", &props);
            let _ = el.set_attribute("data-sidebar-open", open_attr);
            match workspace {
                Some(ws) => {
                    let _ = el.set_attribute("data-module", &ws.name);
                    let _ = el.set_attribute("data-label", &ws.label);
                    match &ws.icon {
                        Some(icon) => {
                            let _ = el.set_attribute("data-icon", icon);
                        }
                        None => {
                            let _ = el.remove_attribute("data-icon");
                        }
                    }
                }
                None => {
                    let _ = el.remove_attribute("data-module");
                    let _ = el.remove_attribute("data-label");
                    let _ = el.remove_attribute("data-icon");
                }
            }
        }
        child = node.next_sibling();
    }
}

/// The `data-props` payload for the header slot: the sidebar state plus the
/// whole active workspace (name, label, module, icon and items) as JSON — the
/// app's element renders whatever it wants from the same contract slots and
/// widgets use.
#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn header_props_json(workspace: Option<&Workspace>, open: bool) -> String {
    serde_json::json!({
        "open": open,
        "workspace": workspace,
    })
    .to_string()
}

/// Resolve a slot's declared props against the row document (R-7): a value
/// written as `"$field"` projects `field` from `row` (JSON-typed, `null` when
/// the row lacks it); any other value is sent verbatim as a string. Kept out of
/// the wasm-only mount path so the projection is unit-testable — and it is
/// compiled only for that path (wasm) and for the unit tests.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn resolve_props(
    props: &[(String, String)],
    row: Option<&serde_json::Value>,
) -> Vec<(String, serde_json::Value)> {
    props
        .iter()
        .map(|(name, value)| {
            let resolved = match value.strip_prefix('$') {
                Some(field) => row
                    .and_then(|doc| doc.get(field))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                None => serde_json::Value::String(value.clone()),
            };
            (name.clone(), resolved)
        })
        .collect()
}

/// Mount the custom elements declared for a slot into the anchor's shadow root,
/// replacing whatever was mounted before (another manifest refresh, a doctype
/// change). Each owner's CSS is adopted into that root, so app A's styles cannot
/// reach app B's declaration in the same slot (F6).
#[cfg(target_arch = "wasm32")]
pub(super) fn mount_slot(
    anchor: Option<web_sys::HtmlDivElement>,
    slot: &str,
    doctype: Option<&str>,
    row: Option<&str>,
    row_data: Option<&serde_json::Value>,
    exts: &[(String, UiExtension)],
    apps: &[AppUi],
) {
    let Some(anchor) = anchor else {
        return;
    };
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(shadow) = crate::ext::loader::shadow_root(&anchor) else {
        return;
    };
    let Some(container) = crate::ext::loader::mount_container(&shadow) else {
        return;
    };

    while let Some(child) = container.first_child() {
        let _ = container.remove_child(&child);
    }

    let mut owners: Vec<&str> = Vec::new();
    for (owner, ext) in exts {
        if !owners.contains(&owner.as_str()) {
            owners.push(owner.as_str());
        }
        let tag = component_tag(&ext.component);
        let Ok(el) = document.create_element(&tag) else {
            continue;
        };
        let _ = el.set_attribute("data-slot", slot);
        if let Some(dt) = doctype {
            let _ = el.set_attribute("data-doctype", dt);
        }
        if let Some(r) = row {
            let _ = el.set_attribute("data-row", r);
        }
        if let Some(label) = &ext.label {
            let _ = el.set_attribute("data-label", label);
        }
        let _ = el.set_attribute("data-component", &ext.component);
        if !ext.props.is_empty() {
            // Same single DOM channel the widgets already read their params
            // from: JSON in, no reflection through live properties. `$field`
            // values are projected from the row document here.
            let resolved = resolve_props(&ext.props, row_data);
            let props = serde_json::to_string(&resolved).unwrap_or_else(|_| "[]".to_string());
            let _ = el.set_attribute("data-props", &props);
        }
        let _ = container.append_child(el.as_ref());
    }

    let urls = crate::ext::loader::stylesheet_urls_for_owners(apps, &owners);
    crate::ext::loader::adopt(&shadow, &urls);
}
