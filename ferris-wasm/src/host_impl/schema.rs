//! `schema` interface of [`HostApi`](crate::host::HostApi).

use super::RealHostApi;
use crate::host::{SchemaDoctypeMeta, SchemaFieldMeta};
use anyhow::Result;

impl RealHostApi {
    // -- schema -------------------------------------------------------------

    pub(super) async fn schema_get_doctype(&self, doctype: &str) -> Result<SchemaDoctypeMeta> {
        let meta = ferris_schema::get_doctype(doctype).ok_or_else(|| {
            // A missing DocType is the plugin asking about something that does
            // not exist: classified `NotFound` (404) at the boundary, not an
            // internal error.
            anyhow::anyhow!(ferris_core::Error::Schema(
                ferris_core::SchemaError::DocTypeNotFound(doctype.to_string())
            ))
        })?;
        Ok(SchemaDoctypeMeta {
            name: meta.name.clone(),
            fields: meta
                .fields
                .iter()
                .map(|field| SchemaFieldMeta {
                    name: field.fieldname.clone(),
                    fieldtype: field.fieldtype.clone(),
                    label: field.label.clone().unwrap_or_default(),
                    required: field.reqd,
                    read_only: field.read_only,
                    options: field.options.clone(),
                })
                .collect(),
            is_submittable: meta.is_submittable,
            is_child_table: meta.is_child_table,
            is_single: meta.is_single,
        })
    }

    pub(super) async fn schema_list_doctypes(&self) -> Result<Vec<String>> {
        Ok(ferris_schema::list_doctypes())
    }
}
