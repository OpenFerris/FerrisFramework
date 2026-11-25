//! Labels for the permission matrix.
//!
//! The Desk's catalogues live in Core; a key this screen introduces still
//! renders readable text until it is translated, and switches over
//! automatically once the catalogue carries it.

use crate::api::authz::Right;
use crate::i18n::I18n;

/// Translate `key`, falling back to English while a catalogue entry is absent.
///
/// The Desk's catalogues live in Core; a key this screen introduces still
/// renders readable text until it is translated, and switches over
/// automatically once the catalogue carries it.
pub(super) fn text(i18n: I18n, key: &str, fallback: &str) -> String {
    translate(i18n, key, fallback, &[])
}

fn translate(i18n: I18n, key: &str, fallback: &str, args: &[(&str, &str)]) -> String {
    match i18n.try_with_value(|catalogue| catalogue.t_with(key, args)) {
        Some(message) if message != key => message,
        _ => {
            let mut out = fallback.to_string();
            for (name, value) in args {
                out = out.replace(&format!("{{{name}}}"), value);
            }
            out
        }
    }
}

pub(super) fn right_label(i18n: I18n, right: Right) -> String {
    let (key, fallback) = match right {
        Right::Read => ("views.docperm.right.read", "Read"),
        Right::Write => ("views.docperm.right.write", "Write"),
        Right::Create => ("views.docperm.right.create", "Create"),
        Right::Delete => ("views.docperm.right.delete", "Delete"),
        Right::Submit => ("views.docperm.right.submit", "Submit"),
        Right::Cancel => ("views.docperm.right.cancel", "Cancel"),
        Right::Amend => ("views.docperm.right.amend", "Amend"),
        Right::Print => ("views.docperm.right.print", "Print"),
        Right::Email => ("views.docperm.right.email", "Email"),
        Right::Share => ("views.docperm.right.share", "Share"),
        Right::Report => ("views.docperm.right.report", "Report"),
        Right::Import => ("views.docperm.right.import", "Import"),
        Right::Export => ("views.docperm.right.export", "Export"),
    };
    text(i18n, key, fallback)
}

#[allow(clippy::literal_string_with_formatting_args)]
pub(super) fn field_level_note(i18n: I18n, count: usize) -> String {
    let base = text(
        i18n,
        "views.docperm.permlevel_note",
        "This matrix edits permission level 0. Field-level rules (permission level > 0) are stored but the engine reports them as untranslated today, so they are not shown here.",
    );
    if count == 0 {
        return base;
    }
    let count = count.to_string();
    let extra = translate(
        i18n,
        "views.docperm.permlevel_hidden",
        "{count} stored rule(s) at permission level > 0 are hidden.",
        &[("count", &count)],
    );
    format!("{base} {extra}")
}
