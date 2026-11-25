//! Labels for the `DocType` builder.
//!
//! The Desk's catalogues live in Core; a key this screen introduces still
//! renders readable text until it is translated, and switches over
//! automatically once the catalogue carries it.

use crate::i18n::I18n;

/// Translate `key`, falling back to English while a catalogue entry is absent.
pub(super) fn text(i18n: I18n, key: &str, fallback: &str) -> String {
    text_with(i18n, key, fallback, &[])
}

/// [`text`] with `{name}` placeholders for the fallback string.
pub(super) fn text_with(i18n: I18n, key: &str, fallback: &str, args: &[(&str, &str)]) -> String {
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
