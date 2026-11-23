//! The translation catalogues an app package ships.
//!
//! An app declares its locales in the WIT descriptor (`descriptor.locales`) and
//! ships the messages as `locales/<tag>.toml` next to `plugin.toml`. Both halves
//! are checked here, before the package is copied into the catalogue:
//!
//! * a declared tag the package does not ship (or a file nobody declared) is a
//!   packaging mistake, not something to discover at boot;
//! * an invalid tag is refused instead of becoming a directory name or a
//!   lookup key;
//! * a malformed TOML, a non-string leaf or an absurdly large file is refused
//!   with the file that carries it.
//!
//! The messages themselves are merged into the site's catalogue set at boot;
//! this module only proves they are well formed.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use ferris_core::i18n::Catalog;
use ferris_core::i18n::Locale;
use ferris_core::i18n::keys::is_valid_key;

/// Largest catalogue accepted per locale.
///
/// The whole effective catalogue travels in the bootstrap `<meta>` of every SSR
/// render, so a runaway file would cost every page. Half a megabyte is two
/// orders of magnitude above a realistic app catalogue.
const MAX_CATALOGUE_BYTES: u64 = 512 * 1024;

/// Validate the catalogues of `package` and return their tags.
///
/// `declared` is `descriptor.locales`. `None` means the caller has no descriptor
/// (a catalogue installed before the field existed): the files are still
/// validated, they are just not cross-checked against a declaration.
///
/// # Errors
///
/// A malformed tag, TOML or key, an oversized file, or a declaration that does
/// not match the files on disk.
pub fn validate(package: &Path, declared: Option<&[String]>) -> Result<Vec<String>> {
    let directory = package.join(ferris_core::site::LOCALES_DIR);
    let mut shipped: BTreeSet<String> = BTreeSet::new();

    if directory.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&directory)
            .with_context(|| format!("cannot read {}", directory.display()))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("cannot read {}", directory.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);

        for entry in entries {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let locale = Locale::parse(stem).with_context(|| {
                format!(
                    "`{}` is not a locale tag; catalogues are named `locales/<tag>.toml`",
                    path.display()
                )
            })?;

            let bytes = fs::metadata(&path)
                .with_context(|| format!("cannot stat {}", path.display()))?
                .len();
            if bytes > MAX_CATALOGUE_BYTES {
                anyhow::bail!(
                    "{} is {bytes} bytes; a catalogue may not exceed {MAX_CATALOGUE_BYTES}",
                    path.display()
                );
            }
            let text = fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let catalog = Catalog::from_toml(locale.clone(), &text)
                .with_context(|| format!("invalid catalogue at {}", path.display()))?;
            if catalog.is_empty() {
                anyhow::bail!("{} declares no messages", path.display());
            }
            for (key, _) in catalog.messages() {
                if !is_valid_key(key) {
                    anyhow::bail!("{}: `{key}` is not a valid catalogue key", path.display());
                }
            }
            shipped.insert(locale.as_str().to_string());
        }
    }

    if let Some(declared) = declared {
        let declared: BTreeSet<String> = declared
            .iter()
            .map(|tag| {
                Locale::parse(tag)
                    .map(|locale| locale.as_str().to_string())
                    .with_context(|| format!("`{tag}` is not a locale tag"))
            })
            .collect::<Result<_>>()?;
        if declared != shipped {
            anyhow::bail!(
                "descriptor.locales and locales/ disagree: declared [{}], shipped [{}]",
                join(&declared),
                join(&shipped)
            );
        }
    }

    Ok(shipped.into_iter().collect())
}

fn join(tags: &BTreeSet<String>) -> String {
    tags.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferris-locales-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("locales")).expect("the fixture directory is created");
        dir
    }

    #[test]
    fn a_matching_declaration_and_catalogue_is_accepted() {
        let package = package("ok");
        fs::write(
            package.join("locales/en.toml"),
            "\"doctype.Task.label\" = \"Task\"\n",
        )
        .expect("the fixture is written");

        let tags = validate(&package, Some(&["en".to_string()])).expect("the package is valid");
        assert_eq!(tags, vec!["en".to_string()]);
        fs::remove_dir_all(package).ok();
    }

    #[test]
    fn a_missing_catalogue_for_a_declared_locale_is_refused() {
        let package = package("missing");
        fs::write(package.join("locales/en.toml"), "\"a.b\" = \"A\"\n").expect("fixture");
        let error = validate(&package, Some(&["en".to_string(), "es".to_string()]))
            .expect_err("the declaration must be honoured");
        assert!(error.to_string().contains("disagree"));
        fs::remove_dir_all(package).ok();
    }

    #[test]
    fn an_undeclared_catalogue_is_refused() {
        let package = package("extra");
        fs::write(package.join("locales/en.toml"), "\"a.b\" = \"A\"\n").expect("fixture");
        fs::write(package.join("locales/pt.toml"), "\"a.b\" = \"A\"\n").expect("fixture");
        let error = validate(&package, Some(&["en".to_string()])).expect_err("pt was not declared");
        assert!(error.to_string().contains("disagree"));
        fs::remove_dir_all(package).ok();
    }

    #[test]
    fn an_invalid_tag_or_key_is_refused() {
        let package = package("invalid");
        fs::write(package.join("locales/es ES.toml"), "\"a.b\" = \"A\"\n").expect("fixture");
        assert!(validate(&package, None).is_err());

        fs::remove_file(package.join("locales/es ES.toml")).ok();
        fs::write(package.join("locales/es.toml"), "\"a b\\n\" = \"A\"\n").expect("fixture");
        assert!(validate(&package, None).is_err());
        fs::remove_dir_all(package).ok();
    }

    #[test]
    fn a_non_string_leaf_is_refused() {
        let package = package("trap");
        fs::write(package.join("locales/en.toml"), "\"a.b\" = 1\n").expect("fixture");
        assert!(validate(&package, None).is_err());
        fs::remove_dir_all(package).ok();
    }

    #[test]
    fn a_package_without_catalogues_and_no_declaration_is_a_no_op() {
        let package = package("empty");
        fs::remove_dir_all(package.join("locales")).ok();
        assert!(
            validate(&package, None)
                .expect("nothing to check")
                .is_empty()
        );
        assert!(validate(&package, Some(&[])).is_ok());
        fs::remove_dir_all(package).ok();
    }
}
