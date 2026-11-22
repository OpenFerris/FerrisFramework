//! `config` interface of [`HostApi`](crate::host::HostApi).

use super::RealHostApi;
use anyhow::Result;
use ferris_core::Value;
use ferris_db::Stmt;
use ferris_db::sql::{UpsertBuilder, UpsertSet};

impl RealHostApi {
    // -- config -------------------------------------------------------------

    pub(super) async fn config_get(&self, plugin: &str, key: &str) -> Result<Option<String>> {
        let row = self
            .pool
            .fetch_optional(&Stmt::new(
                format!("SELECT value FROM {CONFIG_TABLE} WHERE plugin = ? AND key = ?"),
                vec![Value::from(plugin), Value::from(key)],
            ))
            .await?;
        Ok(row.and_then(|row| row.opt_str("value")))
    }

    pub(super) async fn config_set(&self, plugin: &str, key: &str, value: &str) -> Result<()> {
        let upsert = UpsertBuilder::new(self.pool.dialect(), CONFIG_TABLE)
            .values(vec![
                ("plugin", Value::from(plugin)),
                ("key", Value::from(key)),
                ("value", Value::from(value)),
            ])
            .conflict_on(["plugin", "key"])
            .on_update("value", UpsertSet::Value(Value::from(value)))
            .build()?;
        self.pool.upsert(upsert).await?;
        Ok(())
    }

    pub(super) async fn config_list(&self, plugin: &str) -> Result<Vec<String>> {
        let rows = self
            .pool
            .fetch_all(&Stmt::new(
                format!("SELECT key FROM {CONFIG_TABLE} WHERE plugin = ? ORDER BY key ASC"),
                vec![Value::from(plugin)],
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| row.opt_str("key"))
            .collect())
    }

    pub(super) async fn config_delete(&self, plugin: &str, key: &str) -> Result<()> {
        self.pool
            .execute(&Stmt::new(
                format!("DELETE FROM {CONFIG_TABLE} WHERE plugin = ? AND key = ?"),
                vec![Value::from(plugin), Value::from(key)],
            ))
            .await?;
        Ok(())
    }
}

/// The per-app config table. The `(plugin, key)` primary key namespaces one
/// app's `config` keys off every other's.
pub(super) const CONFIG_TABLE: &str = "tabAppConfig";
