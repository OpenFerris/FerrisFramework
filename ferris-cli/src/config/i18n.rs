//! The `[i18n]` section: which locales the deployment advertises and the one
//! a new visitor gets.
//!
//! The catalogue itself lives in `ferris-core` (embedded `en.toml`/`es.toml`),
//! not here: a CLI config selects among the catalogs the binary already ships,
//! and a locale without a catalogue falls back to the default instead of
//! failing the boot. A site overrides both keys from its `site_config.json`
//! (`default_locale`, `supported_locales`) before the server settings are
//! built.

use std::path::Path;

use ferris_core::i18n::keys::is_valid_key;
use ferris_core::i18n::{Catalog, I18n, Locale};
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

/// Largest catalogue read at boot, matching the installer's cap.
const MAX_CATALOGUE_BYTES: u64 = 512 * 1024;

/// Merge every installed app's `locales/<tag>.toml` into `i18n`.
///
/// `apps_root` holds one directory per app (the global `apps/` catalogue, or a
/// site's `plugins/` after `site install-app` copied the messages there). A file
/// is skipped — with a warning naming it — when it is oversized, has an invalid
/// tag or key, or is not valid TOML: one broken translation must not stop a site
/// from booting, and the installer already refused it at install time.
///
/// App catalogues layer over the embedded ones, so an app may translate its own
/// DocType labels and `app.<app>.*` strings without touching the binary.
#[must_use]
pub fn merge_installed_app_catalogues(mut i18n: I18n, root: &Path) -> I18n {
    let mut merged = 0_usize;

    // Two layouts, one rule. A catalogue root (`apps/`, or a site's `plugins/`)
    // holds one directory per app; a standalone project keeps its catalogues in
    // `<project>/locales` with no app directory in between.
    let (next, count) = merge_directory(i18n, &root.join(ferris_core::site::LOCALES_DIR));
    i18n = next;
    merged += count;

    if let Ok(entries) = std::fs::read_dir(root) {
        let mut apps: Vec<_> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .collect();
        apps.sort_by_key(std::fs::DirEntry::file_name);
        for app in apps {
            let (next, count) = merge_directory(
                i18n,
                &app.path().join(ferris_core::site::LOCALES_DIR),
            );
            i18n = next;
            merged += count;
        }
    }

    if merged > 0 {
        tracing::info!(
            catalogues = merged,
            root = %root.display(),
            "merged app translation catalogues"
        );
    }
    i18n
}

/// Merge every `*.toml` of one `locales/` directory; returns the instance and
/// how many catalogues it read.
fn merge_directory(mut i18n: I18n, directory: &Path) -> (I18n, usize) {
    let Ok(files) = std::fs::read_dir(directory) else {
        return (i18n, 0);
    };
    let mut files: Vec<_> = files
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect();
    files.sort_by_key(std::fs::DirEntry::file_name);

    let mut merged = 0;
    for file in files {
        let path = file.path();
        match read_catalogue(&path) {
            Some(catalog) => {
                i18n = i18n.with_catalog(catalog);
                merged += 1;
            }
            None => tracing::warn!(
                path = %path.display(),
                "skipping app catalogue: invalid or oversized"
            ),
        }
    }
    (i18n, merged)
}

fn read_catalogue(path: &Path) -> Option<Catalog> {
    let stem = path.file_stem().and_then(|stem| stem.to_str())?;
    let locale = Locale::parse(stem)?;
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_CATALOGUE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let catalog = Catalog::from_toml(locale, &text).ok()?;
    if catalog.is_empty() || catalog.messages().any(|(key, _)| !is_valid_key(key)) {
        return None;
    }
    Some(catalog)
}

#[cfg(test)]
mod tests {
    use super::{I18nConfig, merge_installed_app_catalogues};
    use ferris_core::i18n::{I18n, Locale};
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ferris-i18n-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("shop").join("locales")).expect("the fixture is created");
        root
    }

    /// An app's catalogue layers over the embedded ones and advertises its
    /// locale, so a site can offer a language the binary does not ship.
    #[test]
    fn installed_app_catalogues_are_merged_and_their_locales_advertised() {
        let root = temp_root("merge");
        std::fs::write(
            root.join("shop").join("locales").join("pt.toml"),
            "\"app.shop.cart.empty\" = \"O seu carrinho está vazio\"\n",
        )
        .expect("the catalogue is written");

        let i18n = merge_installed_app_catalogues(
            I18n::new(Locale::parse("en").expect("en parses"), Vec::new()),
            &root,
        );
        let portuguese = Locale::parse("pt").expect("pt parses");
        assert!(i18n.supported().contains(&portuguese));
        assert_eq!(
            i18n.translate(&portuguese, "app.shop.cart.empty"),
            "O seu carrinho está vazio"
        );
        assert_eq!(
            i18n.translate(&portuguese, "workspace.users.label"),
            "Users",
            "a locale without its own embedded catalogue keeps the English fallback"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// A standalone project keeps `locales/` next to `plugin.toml`, with no app
    /// directory in between.
    #[test]
    fn a_standalone_project_catalogue_is_read_from_its_own_directory() {
        let root = std::env::temp_dir().join(format!(
            "ferris-i18n-standalone-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("locales")).expect("the fixture is created");
        std::fs::write(
            root.join("locales").join("es.toml"),
            "\"app.solo.greeting\" = \"Hola\"\n",
        )
        .expect("the catalogue is written");

        let i18n = merge_installed_app_catalogues(
            I18n::new(Locale::parse("en").expect("en parses"), Vec::new()),
            &root,
        );
        let spanish = Locale::parse("es").expect("es parses");
        assert_eq!(i18n.translate(&spanish, "app.solo.greeting"), "Hola");
        std::fs::remove_dir_all(root).ok();
    }

    /// One malformed file must not stop a boot, and must not half-merge.
    #[test]
    fn a_malformed_catalogue_is_skipped() {
        let root = temp_root("broken");
        std::fs::write(
            root.join("shop").join("locales").join("es.toml"),
            "not toml",
        )
        .expect("the fixture is written");

        let i18n = merge_installed_app_catalogues(
            I18n::new(Locale::parse("es").expect("es parses"), Vec::new()),
            &root,
        );
        let spanish = Locale::parse("es").expect("es parses");
        assert_eq!(
            i18n.translate(&spanish, "workspace.users.label"),
            "Usuarios",
            "the embedded catalogue is untouched"
        );
        std::fs::remove_dir_all(root).ok();
    }

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
