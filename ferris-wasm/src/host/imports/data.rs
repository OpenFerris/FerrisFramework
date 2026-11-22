//! `data` import bridge: documents, naming series and schema
//! metadata, forwarded into [`HostApi`](crate::host::HostApi).
//!
//! Two things happen here that a plain host call would not do
//! not need:
//!
//! - the **capability gate** ([`HostState::require`](crate::host::HostState::require)):
//!   a call the app was not granted fails with `permission-denied` before the
//!   service is touched — reads need `docstore-read`, writes `docstore-write`,
//!   series allocation `docstore-write`, blob traffic `blobs` and the
//!   file-manager half (listing, signed URLs) `files`;
//! - the **shape adaptation**: `write-doc`/`apply-ops` take the typed `write`
//!   record (kind + optional name/data + disposition) and `get-docs` is
//!   positional (`none` for a missing row), so the guest needs neither a
//!   single-document `get` nor a strict batch read.

use crate::ferris::framework::data as wit_data;
use crate::ferris::framework::types::Error as WitError;
use crate::host::{
    BridgeError, Capability, DocAggregateSpec, DocOp, DocRecord, HostApi, HostState, ListDocsQuery,
    WriteDisposition,
};

use super::block_on;

impl crate::ferris::framework::data::Host for HostState {
    fn list_docs(
        &mut self,
        doctype: String,
        query: wit_data::Query,
    ) -> Result<Vec<wit_data::Doc>, WitError> {
        self.require(Capability::DocstoreRead)?;
        let query = ListDocsQuery::from_wit(query);
        block_on(async {
            self.api()
                .docstore_list(&doctype, &query)
                .await
                .map(|docs| docs.into_iter().map(DocRecord::into_wit).collect())
        })
    }

    fn get_docs(
        &mut self,
        doctype: String,
        names: Vec<String>,
    ) -> Result<Vec<Option<wit_data::Doc>>, WitError> {
        self.require(Capability::DocstoreRead)?;
        block_on(async {
            let found = self.api().docstore_get_many(&doctype, &names).await?;
            // The host service answers with the rows that exist, in `names`
            // order; the wire contract is *positional*, so a missing name
            // becomes `none` instead of shifting every later entry.
            let by_name: std::collections::HashMap<String, DocRecord> = found
                .into_iter()
                .map(|doc| (doc.name.clone(), doc))
                .collect();
            // A repeated name answers the same row every time: the map is read,
            // never drained.
            Ok(names
                .into_iter()
                .map(|name| by_name.get(&name).cloned().map(DocRecord::into_wit))
                .collect())
        })
    }

    fn write_doc(&mut self, write: wit_data::Write) -> Result<Option<wit_data::Doc>, WitError> {
        self.require(Capability::DocstoreWrite)?;
        let op = DocOp::from_wit(write);
        block_on(async {
            let api = self.api();
            match op.kind.as_str() {
                "create" => {
                    let data = require_data(&op)?;
                    Ok(Some(
                        api.docstore_create(&op.doctype, data).await?.into_wit(),
                    ))
                }
                "upsert" => {
                    let data = require_data(&op)?;
                    let stored = match op.disposition {
                        WriteDisposition::Silent => {
                            api.docstore_write_no_events(&op.doctype, data).await?
                        }
                        WriteDisposition::EmitEvents => {
                            api.docstore_upsert(&op.doctype, data).await?
                        }
                    };
                    Ok(Some(stored.into_wit()))
                }
                "update" => {
                    let name = require_name(&op)?;
                    let data = require_data(&op)?;
                    Ok(Some(
                        api.docstore_update(&op.doctype, name, data)
                            .await?
                            .into_wit(),
                    ))
                }
                "delete" => {
                    let name = require_name(&op)?;
                    api.docstore_delete(&op.doctype, name).await?;
                    Ok(None)
                }
                other => Err(BridgeError::unsupported(format!(
                    "data.write-doc with unknown op `{other}`"
                ))),
            }
        })
    }

    fn apply_ops(
        &mut self,
        ops: Vec<wit_data::Write>,
    ) -> Result<Vec<Option<wit_data::Doc>>, WitError> {
        self.require(Capability::DocstoreWrite)?;
        let ops: Vec<DocOp> = ops.into_iter().map(DocOp::from_wit).collect();
        block_on(async {
            if ops.iter().all(|op| op.kind == "create") {
                // A create-only batch is `create_bulk`: one transaction, one
                // DocType. The engine's batch primitive is upsert/update/delete,
                // and pretending a create were an upsert would turn a name
                // conflict into a silent overwrite.
                return create_batch(self.api(), &ops).await;
            }
            if ops.iter().any(|op| op.kind == "create") {
                return Err(BridgeError::unsupported(
                    "data.apply-ops mixing `create` with upsert/update/delete \
                     (send the creates as their own batch)",
                ));
            }
            let stored = self.api().docstore_apply(&ops).await?;
            Ok(align_apply_result(&ops, stored))
        })
    }

    fn count_docs(&mut self, doctype: String, query: wit_data::Query) -> Result<u64, WitError> {
        self.require(Capability::DocstoreRead)?;
        let query = ListDocsQuery::from_wit(query);
        block_on(async { self.api().docstore_count(&doctype, &query).await })
    }

    fn aggregate_docs(
        &mut self,
        doctype: String,
        query: wit_data::Query,
        group_by: Vec<String>,
        aggregates: Vec<wit_data::AggregateSpec>,
    ) -> Result<Vec<wit_data::AggregateRow>, WitError> {
        self.require(Capability::DocstoreRead)?;
        let query = ListDocsQuery::from_wit(query);
        let aggregates: Vec<DocAggregateSpec> = aggregates
            .into_iter()
            .map(DocAggregateSpec::from_wit)
            .collect();
        block_on(async {
            self.api()
                .docstore_aggregate(&doctype, &query, &group_by, &aggregates)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
        })
    }

    fn next_name(&mut self, series: String) -> Result<String, WitError> {
        // A series allocates a number; it does not read a document.
        self.require(Capability::DocstoreWrite)?;
        block_on(async { self.api().naming_next(&series).await })
    }

    fn rename_doc(
        &mut self,
        doctype: String,
        name: String,
        new_name: String,
    ) -> Result<wit_data::Doc, WitError> {
        self.require(Capability::DocstoreWrite)?;
        block_on(async {
            self.api()
                .docstore_rename(&doctype, &name, &new_name)
                .await
                .map(DocRecord::into_wit)
        })
    }

    fn set_docstatus(
        &mut self,
        doctype: String,
        name: String,
        action: wit_data::DocstatusAction,
    ) -> Result<wit_data::Doc, WitError> {
        self.require(Capability::DocstoreWrite)?;
        let action = match action {
            wit_data::DocstatusAction::Submit => "submit",
            wit_data::DocstatusAction::Cancel => "cancel",
            wit_data::DocstatusAction::Amend => "amend",
        };
        block_on(async {
            self.api()
                .docstore_set_docstatus(&doctype, &name, action)
                .await
                .map(DocRecord::into_wit)
        })
    }

    fn doctype_json(&mut self, doctype: String) -> Result<String, WitError> {
        self.require(Capability::Schema)?;
        block_on(async { self.api().docstore_doctype_json(&doctype).await })
    }

    fn field_types(&mut self) -> Result<Vec<String>, WitError> {
        self.require(Capability::Schema)?;
        block_on(async { Ok(self.api().field_types().await) })
    }

    fn get_doctype(&mut self, doctype: String) -> Result<wit_data::DoctypeMeta, WitError> {
        self.require(Capability::Schema)?;
        block_on(async {
            let meta = self.api().schema_get_doctype(&doctype).await?;
            Ok(wit_data::DoctypeMeta {
                name: meta.name,
                fields: meta.fields.into_iter().map(wit_field_meta).collect(),
                is_submittable: meta.is_submittable,
                is_child_table: meta.is_child_table,
                is_single: meta.is_single,
            })
        })
    }

    fn list_doctypes(&mut self) -> Result<Vec<String>, WitError> {
        self.require(Capability::Schema)?;
        block_on(async { self.api().schema_list_doctypes().await })
    }

    fn call_method(&mut self, name: String, params: String) -> Result<String, WitError> {
        // No capability of its own: the resolver refuses a name the calling app
        // does not override, and the ambient user is the one the outer
        // `handle-request` already runs as, so `super` cannot escalate.
        let Some(resolver) = crate::host::globals::method_resolver() else {
            return Err(WitError::NotFound(
                "this host has no API method chain".to_string(),
            ));
        };
        let app = self.plugin_name.clone();
        block_on(async move { resolver.call_next(&app, &name, &params).await })
    }
}

// -- helpers ----------------------------------------------------------------

/// The schema field projection `data.get-doctype` answers with.
fn wit_field_meta(field: crate::host::SchemaFieldMeta) -> wit_data::FieldMeta {
    wit_data::FieldMeta {
        name: field.name,
        fieldtype: field.fieldtype,
        label: field.label,
        required: field.required,
        read_only: field.read_only,
        options: field.options,
    }
}

/// The JSON body of a write, or the typed error that names the op.
fn require_data(op: &DocOp) -> Result<&str, BridgeError> {
    op.data.as_deref().ok_or_else(|| {
        BridgeError::Unsupported(format!(
            "data.write-doc `{}` on `{}` requires `data`",
            op.kind, op.doctype
        ))
    })
}

/// The row name of a write, or the typed error that names the op.
fn require_name(op: &DocOp) -> Result<&str, BridgeError> {
    if op.name.is_empty() {
        return Err(BridgeError::Unsupported(format!(
            "data.write-doc `{}` on `{}` requires `name`",
            op.kind, op.doctype
        )));
    }
    Ok(&op.name)
}

/// A create-only batch: one transaction over one `DocType`, positional result.
async fn create_batch(
    api: &std::sync::Arc<dyn HostApi>,
    ops: &[DocOp],
) -> anyhow::Result<Vec<Option<wit_data::Doc>>> {
    let Some(first) = ops.first() else {
        return Ok(Vec::new());
    };
    if ops.iter().any(|op| op.doctype != first.doctype) {
        return Err(BridgeError::unsupported(
            "data.apply-ops with creates spanning more than one DocType",
        ));
    }
    let records: Vec<String> = ops
        .iter()
        .map(|op| require_data(op).map(str::to_string))
        .collect::<Result<_, BridgeError>>()?;
    let stored = api.docstore_bulk_create(&first.doctype, &records).await?;
    Ok(stored.into_iter().map(|doc| Some(doc.into_wit())).collect())
}

/// Put an `apply` result back in op order: a `delete` contributes `none`.
pub(super) fn align_apply_result(
    ops: &[DocOp],
    stored: Vec<DocRecord>,
) -> Vec<Option<wit_data::Doc>> {
    let mut written = stored.into_iter();
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        if op.kind == "delete" {
            out.push(None);
        } else {
            out.push(written.next().map(DocRecord::into_wit));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire contract is positional: a `delete` contributes `none` instead
    /// of shifting the documents that follow it.
    #[test]
    fn an_apply_result_stays_positional_across_deletes() {
        let ops = vec![
            DocOp {
                kind: "update".into(),
                doctype: "Task".into(),
                name: "T-1".into(),
                data: Some("{}".into()),
                disposition: WriteDisposition::EmitEvents,
            },
            DocOp {
                kind: "delete".into(),
                doctype: "Task".into(),
                name: "T-2".into(),
                data: None,
                disposition: WriteDisposition::EmitEvents,
            },
            DocOp {
                kind: "upsert".into(),
                doctype: "Task".into(),
                name: String::new(),
                data: Some("{}".into()),
                disposition: WriteDisposition::EmitEvents,
            },
        ];
        let stored = vec![
            DocRecord {
                name: "T-1".into(),
                doctype: "Task".into(),
                owner: "u".into(),
                docstatus: "draft".into(),
                data: "{}".into(),
            },
            DocRecord {
                name: "T-3".into(),
                doctype: "Task".into(),
                owner: "u".into(),
                docstatus: "draft".into(),
                data: "{}".into(),
            },
        ];
        let aligned = align_apply_result(&ops, stored);
        assert_eq!(aligned.len(), 3);
        assert_eq!(
            aligned[0].as_ref().map(|doc| doc.name.as_str()),
            Some("T-1")
        );
        assert!(aligned[1].is_none(), "a delete has no document");
        assert_eq!(
            aligned[2].as_ref().map(|doc| doc.name.as_str()),
            Some("T-3")
        );
    }
}
