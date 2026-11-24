//! Role Permissions Builder command.
//!
//! Reads and writes `DocPerm` rows in the configured database so those rules
//! are part of the boot-time policy install ([`ferris_api::authz`] merges the
//! same table at every boot). The row is persisted to the shared `tabDocPerm`
//! table; the `/api/authz/role-permissions*` endpoints live-reload a *running*
//! server, whereas a CLI edit is picked up on the server's next restart —
//! which is exactly what this file says, so nobody mistakes a CLI write for a
//! live change.

use super::db_map::core_database;
use super::*;
use crate::config::CliConfig;
use ferris_auth::docperm::{DocPermRow, DocPermStore};
use tracing::{info, warn};

pub fn run(args: super::RolePermissionsArgs, config: &CliConfig) -> Result<()> {
    let rt = crate::runtime::block_on_runtime(None)?;

    rt.block_on(async {
        let database = core_database(&config.database, config.database.name.clone())?;
        let pool = ferris_db::create_pool(&database).await?;
        let store = DocPermStore::new(pool.clone());
        store.init().await?;

        match args.command {
            super::RolePermissionsCommand::List { doctype } => {
                let rows = match &doctype {
                    Some(doctype) => store.list(doctype).await?,
                    None => store.list_all().await?,
                };
                println!("{}", serde_json::to_string_pretty(&rows)?);
            }
            super::RolePermissionsCommand::Set {
                doctype,
                role,
                permlevel,
                read,
                write,
                create,
                delete,
                submit,
                cancel,
                amend,
                print,
                email,
                share,
                report,
                import,
                export,
                if_owner,
                all_doctypes,
            } => {
                let row = DocPermRow {
                    doctype,
                    role: role.clone(),
                    permlevel,
                    read,
                    write,
                    create,
                    delete,
                    submit,
                    cancel,
                    amend,
                    print,
                    email,
                    share,
                    report,
                    import,
                    export,
                    if_owner,
                    apply_to_all_doctypes: all_doctypes,
                };
                store.upsert(&row).await?;
                if permlevel > 0 {
                    warn!(
                        "permlevel {permlevel} rows are stored but not translated by the engine \
                         (field-level checks do not exist yet)"
                    );
                }
                info!(
                    "wrote role {} on {} (permlevel {permlevel}); a running server picks this up on restart",
                    role, row.doctype
                );
            }
            super::RolePermissionsCommand::Delete { doctype, role } => {
                store.delete(&doctype, &role).await?;
                info!("deleted role {role} on {doctype}");
            }
        }

        Ok(())
    })
}
