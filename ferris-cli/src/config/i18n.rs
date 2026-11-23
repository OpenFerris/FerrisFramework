//! The `[i18n]` section: which locales the deployment advertises and the one
//! a new visitor gets.
//!
//! The catalogue itself lives in `ferris-core` (embedded `en.toml`/`es.toml`),
//! not here: a CLI config selects among the catalogs the binary already ships,
//! and a locale without a catalogue falls back to the default instead of
//! failing the boot. A site overrides both keys from its `site_config.json`
//! (`default_locale`, `supported_locales`) before the server settings are
//! built.

use serde::{Deserialize, Serialize};

/// The default locale when neither the config nor the site names one.
pub(super) const DEFAULT_LOCALE: &str = "en";

/// The locales the binary ships catalogues for. Kept as a list (instead of a
/// hardcoded `["en"]`) so `ferris config show` reports the same set the Desk
/// can actually negotiate.
const BUILTIN_LOCALES: [&str; 2] = ["en", "es"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct I18nConfig {
    /// Locale served when the client sends no `Accept-Language` (or none of
    /// its preferences is supported).
    #[serde(default = "default_locale")]
    pub default_locale: String,
    /// Locales advertised by `GET /api/i18n`. An empty list means the built-in
    /// set; a locale without a catalogue still negotiates but falls back to
    /// the default messages.
    #[serde(default = "default_supported")]
    pub supported: Vec<String>,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            default_locale: default_locale(),
            supported: default_supported(),
        }
    }
}

fn default_locale() -> String {
    DEFAULT_LOCALE.to_string()
}

fn default_supported() -> Vec<String> {
    BUILTIN_LOCALES
        .iter()
        .map(|tag| (*tag).to_string())
        .collect()
}

impl I18nConfig {
    /// The configured locales, with the default first and no blank or
    /// duplicated entries. An empty list falls back to the built-in set.
    #[must_use]
    pub fn normalized(&self) -> Vec<String> {
        let mut locales: Vec<String> = Vec::with_capacity(self.supported.len() + 1);
        let default = self.default_locale.trim();
        if !default.is_empty() {
            locales.push(default.to_string());
        }
        for tag in &self.supported {
            let tag = tag.trim();
            if !tag.is_empty() && !locales.iter().any(|seen| seen == tag) {
                locales.push(tag.to_string());
            }
        }
        if locales.is_empty() {
            locales.extend(BUILTIN_LOCALES.iter().map(|tag| (*tag).to_string()));
        }
        locales
    }
}

#[cfg(test)]
mod tests {
    use super::I18nConfig;

    #[test]
    fn the_default_is_english_and_advertises_the_builtin_pair() {
        let config = I18nConfig::default();
        assert_eq!(config.default_locale, "en");
        assert_eq!(config.supported, vec!["en", "es"]);
    }

    #[test]
    fn normalization_puts_the_default_first_and_drops_duplicates() {
        let config = I18nConfig {
            default_locale: "es".into(),
            supported: vec!["es".into(), " ".into(), "en".into(), "es".into()],
        };
        assert_eq!(config.normalized(), vec!["es", "en"]);
    }

    #[test]
    fn an_empty_list_falls_back_to_the_builtin_set() {
        let config = I18nConfig {
            default_locale: String::new(),
            supported: Vec::new(),
        };
        assert_eq!(config.normalized(), vec!["en", "es"]);
    }
}
