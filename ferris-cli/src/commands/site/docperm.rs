//! The `docperm.toml` an app package may ship.
//!
//! A WASM descriptor declares `DocType`s but has no place for access rules; the
//! host's `DocPerm` rows are the policy. Without a file to carry them, every
//! app's `DocType`s are reachable only by `System Manager` until an operator
//! creates rows by hand. This module closes that gap without changing the WIT
//! contract: `docperm.toml` lives next to `plugin.toml` in the catalogue and
//! `ferris site install-app` upserts it into **that site's** database.
//!
//! The rows are the same shape `ferris role-permissions set` writes, so the
//! operator can inspect or override them with the existing command. Applying is
//! an upsert: installing/upgrading twice is a no-op, and a role removed from
//! the file is left alone (removing a grant is an explicit operator action).

use anyhow::{Context, Result};
use ferris_auth::docperm::{DocPermRow, DocPermStore};
use ferris_core::site::SiteManager;
use serde::Deserialize;

/// The manifest file an app package may carry.
pub const DOCPERM_FILE: &str = "docperm.toml";

#[derive(Debug, Deserialize)]
struct DocPermFile {
    #[serde(default)]
    permissions: Vec<DocPermEntry>,
}

/// One `[[permissions]]` entry. Every right defaults to `false`, matching
/// `DocPermRow::default`: a row that names nothing grants nothing.
// One row mirrors `DocPermRow`, whose thirteen rights are independent booleans
// by design: a bitmask here would be a second policy vocabulary to maintain.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Deserialize)]
struct DocPermEntry {
    doctype: String,
    role: String,
    #[serde(default)]
    permlevel: u8,
    #[serde(default)]
    read: bool,
    #[serde(default)]
    write: bool,
    #[serde(default)]
    create: bool,
    #[serde(default)]
    delete: bool,
    #[serde(default)]
    submit: bool,
    #[serde(default)]
    cancel: bool,
    #[serde(default)]
    amend: bool,
    #[serde(default)]
    print: bool,
    #[serde(default)]
    email: bool,
    #[serde(default)]
    share: bool,
    #[serde(default)]
    report: bool,
    #[serde(default)]
    import: bool,
    #[serde(default)]
    export: bool,
    #[serde(default)]
    if_owner: bool,
    #[serde(default)]
    all_doctypes: bool,
}

impl DocPermEntry {
    fn into_row(self) -> DocPermRow {
        DocPermRow {
            doctype: self.doctype,
            role: self.role,
            permlevel: self.permlevel,
            read: self.read,
            write: self.write,
            create: self.create,
            delete: self.delete,
            submit: self.submit,
            cancel: self.cancel,
            amend: self.amend,
            print: self.print,
            email: self.email,
            share: self.share,
            report: self.report,
            import: self.import,
            export: self.export,
            if_owner: self.if_owner,
            apply_to_all_doctypes: self.all_doctypes,
        }
    }
}

/// Apply the catalogue package's `docperm.toml` to `site`.
///
/// Returns how many rows were written; a package without the file applies
/// nothing. The site must already exist (its database is opened here).
///
/// # Errors
///
/// A malformed file, a database that cannot be opened, or an upsert failure.
/// None is swallowed: an install that promises permissions and silently skips
/// them is worse than one that fails.
pub fn apply(catalogue: &SiteManager, site: &str, app: &str) -> Result<usize> {
    let path = catalogue
        .global_app_dir(app)
        .with_context(|| format!("app `{app}` is not in the global catalogue"))?
        .join(DOCPERM_FILE);
    if !path.is_file() {
        return Ok(0);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let file: DocPermFile = toml::from_str(&text)
        .with_context(|| format!("invalid permissions in {}", path.display()))?;

    let site_config = catalogue.get(site)?;
    let database = super::site_database(catalogue, &site_config);
    let runtime = crate::runtime::block_on_runtime(Some(2))?;
    let pool = runtime.block_on(ferris_db::create_pool(&database))?;
    let store = DocPermStore::new(pool);
    runtime.block_on(store.init())?;
    let mut written = 0;
    for entry in file.permissions {
        let row = entry.into_row();
        runtime
            .block_on(store.upsert(&row))
            .with_context(|| format!("cannot apply permissions for `{}`", row.doctype))?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::DocPermFile;

    #[test]
    fn the_rights_default_to_closed() {
        let file: DocPermFile = toml::from_str(
            r#"
[[permissions]]
doctype = "Task"
role = "All"
read = true
"#,
        )
        .expect("valid file");
        let row = file
            .permissions
            .into_iter()
            .next()
            .expect("one row")
            .into_row();
        assert_eq!(row.doctype, "Task");
        assert_eq!(row.role, "All");
        assert!(row.read);
        assert!(!row.write, "an unnamed right grants nothing");
        assert!(!row.create);
    }
}
