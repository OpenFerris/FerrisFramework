//! The DocTypes the Desk exposes, for pickers and diagnostics.
//!
//! Two pages (the custom-permission matrix and the calendar) need the same
//! "which doctypes does this Desk have" list. Keeping the walk in one hook
//! means the sidebar shape is interpreted once, nested groups included, instead
//! of each page re-implementing it and drifting.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::desk::session::{self, DeskSidebarEntry};

/// A signal that fills with the sorted DocType names once `/api/desk-layout`
/// answers. It starts empty on both SSR and the first hydrate pass, so the
/// server and the browser render the same placeholder.
pub fn use_desk_doctypes() -> RwSignal<Vec<String>> {
    let doctypes = RwSignal::new(Vec::new());
    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(layout) = session::desk_layout().await {
                let mut names = Vec::new();
                for module in layout.workspaces {
                    collect(&module.items, &mut names);
                }
                names.sort();
                names.dedup();
                doctypes.set(names);
            }
        });
    });
    doctypes
}

fn collect(entries: &[DeskSidebarEntry], out: &mut Vec<String>) {
    for entry in entries {
        if let Some(doctype) = entry.doctype.as_deref().map(str::trim)
            && !doctype.is_empty()
            && !out.iter().any(|name| name == doctype)
        {
            out.push(doctype.to_string());
        }
        collect(&entry.children, out);
    }
}
