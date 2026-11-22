//! `docstore` interface of [`HostApi`](crate::host::HostApi).

use super::RealHostApi;
use super::aggregate::{AggregateAcc, default_aggregate_alias};
use super::query::{MAX_RANGE_ROWS, cell_value};
use crate::host::{
    DocAggregateRow, DocAggregateSpec, DocOp, DocRecord, HostError, ListDocsQuery,
    WriteDisposition, classify_host_error,
};
use anyhow::Result;
use ferris_core::Value;
use ferris_document::engine::{ApplyKind, ApplyOp};
use std::collections::HashMap;

impl RealHostApi {
    /// Create-or-update by `name`, idempotent: try `create` first and, when the
    /// name already exists, update the same row with the same body. The
    /// create-first order keeps the happy path a single write; the conflict
    /// fallback is the retry an ingest would otherwise have to implement
    /// (get → create → update) in the guest. `suppress_events` is the
    /// `write-no-events` reentrancy guard.
    async fn upsert_impl(
        &self,
        doctype: &str,
        data: &str,
        suppress_events: bool,
    ) -> Result<DocRecord> {
        let value: Value = serde_json::from_str(data)?;
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut ctx = self.ctx(doctype);
        ctx.suppress_events = suppress_events;

        let stored = match self.engine.create(&ctx, value.clone()).await {
            Ok(stored) => stored,
            Err(ferris_core::Error::Document(ferris_core::DocumentError::AlreadyExists {
                ..
            })) => {
                if name.is_empty() {
                    // `create` only reports a name conflict when the body
                    // carried one; an empty name that conflicts is a host
                    // invariant break, not something to guess around.
                    anyhow::bail!("upsert conflict without a document name");
                }
                self.engine.update(&ctx, &name, value).await?
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Self::record_from_value(doctype, &stored))
    }
}

/// The JSON body of an `upsert`/`update` `docstore::apply` op. Absent or
/// malformed data is a typed error, so a batch cannot silently skip a write it
/// was asked to perform.
fn op_data(op: &DocOp) -> Result<Value> {
    let raw = op
        .data
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("`{}` op on `{}` requires `data`", op.kind, op.doctype))?;
    Ok(serde_json::from_str(raw)?)
}

impl RealHostApi {
    // -- docstore -----------------------------------------------------------

    pub(super) async fn docstore_list(
        &self,
        doctype: &str,
        query: &ListDocsQuery,
    ) -> Result<Vec<DocRecord>> {
        let (documents, _) = self.query_documents(doctype, query, true, false).await?;
        Ok(documents
            .iter()
            .map(|value| Self::doc_record(value, doctype, query.fields.as_deref()))
            .collect())
    }

    pub(super) async fn docstore_get(&self, doctype: &str, name: &str) -> Result<DocRecord> {
        let value = self.engine.get(&self.ctx(doctype), name).await?;
        let obj = value.as_object().cloned().unwrap_or_default();
        // Read-back canónico: el engine persiste el blob con claves ordenadas y
        // la comparación exacta del plugin (doc.data != data) solo es estable
        // si este re-serializado también las ordena. `serde_json::to_value`
        // deja un `Map` (ordenado) en cada nivel.
        let canonical: serde_json::Value = serde_json::to_value(&obj).unwrap_or_default();
        let data = serde_json::to_string(&canonical).unwrap_or_default();

        Ok(DocRecord {
            name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            doctype: doctype.to_string(),
            owner: obj
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            docstatus: obj
                .get("docstatus")
                .and_then(|v| v.as_str())
                .unwrap_or("draft")
                .to_string(),
            data,
        })
    }

    pub(super) async fn docstore_get_many(
        &self,
        doctype: &str,
        names: &[String],
    ) -> Result<Vec<DocRecord>> {
        // A missing name is not an error for the batch read: `get` raises
        // `not-found`, which is exactly what a partially-known id set should
        // tolerate. Any other failure propagates.
        let mut docs = Vec::with_capacity(names.len());
        for name in names {
            match self.docstore_get(doctype, name).await {
                Ok(doc) => docs.push(doc),
                Err(e) => match classify_host_error(&e) {
                    HostError::NotFound(_) => {}
                    _ => return Err(e),
                },
            }
        }
        Ok(docs)
    }

    pub(super) async fn docstore_create(&self, doctype: &str, data: &str) -> Result<DocRecord> {
        let value: Value = serde_json::from_str(data)?;
        let ctx = self.ctx(doctype);

        let stored = self.engine.create(&ctx, value).await?;
        let obj = stored.as_object().cloned().unwrap_or_default();

        Ok(DocRecord {
            name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            doctype: doctype.to_string(),
            owner: obj
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            docstatus: obj
                .get("docstatus")
                .and_then(|v| v.as_str())
                .unwrap_or("draft")
                .to_string(),
            data: serde_json::to_string(&obj).unwrap_or_default(),
        })
    }

    /// Create-or-update by `name`, idempotent: try `create` first and, when the
    /// name already exists, update the same row with the same body. The
    /// create-first order keeps the happy path a single write; the conflict
    /// fallback is the retry an ingest would otherwise have to implement
    /// (get → create → update) in the guest.
    pub(super) async fn docstore_upsert(&self, doctype: &str, data: &str) -> Result<DocRecord> {
        self.upsert_impl(doctype, data, false).await
    }

    /// `docstore::write-no-events`: the same create-then-update as `upsert`, but
    /// the engine context suppresses the plugin `doctype-event` forwarder so the
    /// guest never re-enters on the row it just wrote (reentrancy guard).
    pub(super) async fn docstore_write_no_events(
        &self,
        doctype: &str,
        data: &str,
    ) -> Result<DocRecord> {
        self.upsert_impl(doctype, data, true).await
    }

    /// Count the documents matching `query` without materialising the rows.
    /// Delegates to the same planner as [`Self::docstore_list`] but discards
    /// the page, so column-backed predicates become a single `COUNT(*)` and
    /// large telemetry windows never cross the plugin boundary.
    pub(super) async fn docstore_count(&self, doctype: &str, query: &ListDocsQuery) -> Result<u64> {
        let (_, total) = self.query_documents(doctype, query, false, false).await?;
        Ok(total)
    }

    /// Group and aggregate the documents matching `query`.
    ///
    /// The candidate set is selected through the same planner as `docstore_list`
    /// (filters + ordering), so the semantics of a filter are identical; only
    /// the page is dropped. Aggregation runs host-side over the scan, bounded by
    /// `MAX_RANGE_ROWS` exactly like the `ne`/`between` list fallback, which
    /// keeps a million-row telemetry series from materialising.
    pub(super) async fn docstore_aggregate(
        &self,
        doctype: &str,
        query: &ListDocsQuery,
        group_by: &[String],
        aggregates: &[DocAggregateSpec],
    ) -> Result<Vec<DocAggregateRow>> {
        // Validate up front: a typo must be a typed validation error, not a
        // silently empty column.
        for spec in aggregates {
            if !matches!(spec.func.as_str(), "count" | "sum" | "avg" | "min" | "max") {
                anyhow::bail!(
                    "unknown aggregate `{}` (use count/sum/avg/min/max)",
                    spec.func
                );
            }
        }

        let mut scan = query.clone();
        scan.limit = Some(MAX_RANGE_ROWS);
        scan.offset = Some(0);
        scan.fields = None;
        let (documents, _) = self.query_documents(doctype, &scan, true, true).await?;

        // Accumulate per distinct group key, preserving first-seen order so the
        // result is deterministic (and mirrors the candidate ordering).
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, (Vec<(String, String)>, Vec<AggregateAcc>)> =
            HashMap::new();
        for doc in &documents {
            let key_pairs: Vec<(String, String)> = group_by
                .iter()
                .map(|field| (field.clone(), cell_value(doc, field)))
                .collect();
            let key = key_pairs
                .iter()
                .map(|(field, value)| format!("{field}={value}"))
                .collect::<Vec<_>>()
                .join("\u{1f}");
            if !groups.contains_key(&key) {
                order.push(key.clone());
                groups.insert(
                    key.clone(),
                    (
                        key_pairs,
                        aggregates.iter().map(AggregateAcc::new).collect(),
                    ),
                );
            }
            let (_, accs) = groups.get_mut(&key).expect("group was just inserted");
            for (acc, spec) in accs.iter_mut().zip(aggregates) {
                acc.observe(spec, doc);
            }
        }

        let mut rows = Vec::with_capacity(order.len());
        for key in order {
            let (group, accs) = groups.remove(&key).expect("group was inserted in order");
            let values = aggregates
                .iter()
                .zip(accs)
                .map(|(spec, acc)| {
                    let alias = spec
                        .alias
                        .clone()
                        .unwrap_or_else(|| default_aggregate_alias(spec));
                    (alias, acc.render(&spec.func))
                })
                .collect();
            rows.push(DocAggregateRow { group, values });
        }
        Ok(rows)
    }
    pub(super) async fn docstore_update(
        &self,
        doctype: &str,
        name: &str,
        data: &str,
    ) -> Result<DocRecord> {
        let value: Value = serde_json::from_str(data)?;
        let ctx = self.ctx(doctype);

        let stored = self.engine.update(&ctx, name, value).await?;
        let obj = stored.as_object().cloned().unwrap_or_default();

        Ok(DocRecord {
            name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            doctype: doctype.to_string(),
            owner: obj
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            docstatus: obj
                .get("docstatus")
                .and_then(|v| v.as_str())
                .unwrap_or("draft")
                .to_string(),
            data: serde_json::to_string(&obj).unwrap_or_default(),
        })
    }

    pub(super) async fn docstore_delete(&self, doctype: &str, name: &str) -> Result<()> {
        let ctx = self.ctx(doctype);
        // Propagate the typed core error (`?`) so the boundary classifies a
        // missing row as `not-found` instead of flattening it to `internal`.
        self.engine.delete(&ctx, name).await?;
        Ok(())
    }

    pub(super) async fn docstore_bulk_create(
        &self,
        doctype: &str,
        records: &[String],
    ) -> Result<Vec<DocRecord>> {
        let ctx = self.ctx(doctype);
        let values = records
            .iter()
            .map(|raw| serde_json::from_str::<Value>(raw))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // One transaction for the whole batch (see `DocumentEngine::create_bulk`).
        let stored = self.engine.create_bulk(&ctx, values).await?;
        Ok(stored
            .into_iter()
            .map(|v| {
                let obj = v.as_object().cloned().unwrap_or_default();
                DocRecord {
                    name: obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    doctype: doctype.to_string(),
                    owner: obj
                        .get("owner")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    docstatus: obj
                        .get("docstatus")
                        .and_then(|v| v.as_str())
                        .unwrap_or("draft")
                        .to_string(),
                    data: serde_json::to_string(&obj).unwrap_or_default(),
                }
            })
            .collect())
    }

    /// Apply a batch of `upsert`/`update`/`delete` writes as **one**
    /// transaction (see `DocumentEngine::apply`). Each op gets a context for its
    /// own `DocType`, so a single batch can span doctypes; any failing op rolls
    /// the whole batch back. Returns the documents written by the write ops, in
    /// op order (`delete` ops contribute none).
    pub(super) async fn docstore_apply(&self, ops: &[DocOp]) -> Result<Vec<DocRecord>> {
        let mut batch = Vec::with_capacity(ops.len());
        for op in ops {
            let op_kind = match op.kind.as_str() {
                "upsert" => ApplyKind::Upsert(op_data(op)?),
                "update" => ApplyKind::Update {
                    name: op.name.clone(),
                    data: op_data(op)?,
                },
                "delete" => ApplyKind::Delete {
                    name: op.name.clone(),
                },
                other => {
                    anyhow::bail!("unknown `apply` op `{other}` (use upsert/update/delete)")
                }
            };
            let mut ctx = self.ctx(&op.doctype);
            // `write-disposition.silent` is the app's explicit opt-out of its
            // own handlers: the row is persisted and stamped, but no
            // `doctype-event` re-enters the guest for it.
            if op.disposition == WriteDisposition::Silent {
                ctx.suppress_events = true;
            }
            batch.push(ApplyOp { ctx, op: op_kind });
        }
        let stored = self.engine.apply(batch).await?;
        // `delete` ops contribute no document; walk the ops in step with the
        // returned docs so each record is labelled with its own DocType.
        let mut out = Vec::with_capacity(stored.len());
        let mut written = stored.into_iter();
        for op in ops {
            if op.kind == "delete" {
                continue;
            }
            let value = written
                .next()
                .ok_or_else(|| anyhow::anyhow!("apply returned fewer documents than writes"))?;
            out.push(Self::record_from_value(&op.doctype, &value));
        }
        Ok(out)
    }

    pub(super) async fn docstore_rename(
        &self,
        doctype: &str,
        name: &str,
        new_name: &str,
    ) -> Result<DocRecord> {
        let value = self
            .engine
            .rename(&self.ctx(doctype), name, new_name)
            .await?;
        Ok(Self::record_from_value(doctype, &value))
    }

    pub(super) async fn docstore_set_docstatus(
        &self,
        doctype: &str,
        name: &str,
        action: &str,
    ) -> Result<DocRecord> {
        let value = match action {
            "submit" => {
                self.engine
                    .set_docstatus(
                        &self.ctx(doctype),
                        name,
                        ferris_document::DocStatus::Submitted,
                    )
                    .await?
            }
            "cancel" => {
                self.engine
                    .set_docstatus(
                        &self.ctx(doctype),
                        name,
                        ferris_document::DocStatus::Cancelled,
                    )
                    .await?
            }
            "amend" => self.engine.amend(&self.ctx(doctype), name).await?,
            other => anyhow::bail!(
                "unknown docstatus action `{other}` (expected submit, cancel or amend)"
            ),
        };
        Ok(Self::record_from_value(doctype, &value))
    }

    pub(super) async fn docstore_doctype_json(&self, doctype: &str) -> Result<String> {
        // Effective metadata: the app sees the same custom fields, property
        // setters and Custom DocPerm overrides the Desk does, not the compiled
        // shape with the site's customizations missing.
        let meta = ferris_schema::effective_meta(&self.pool, doctype)
            .await?
            .ok_or_else(|| anyhow::anyhow!("DocType `{doctype}` is not registered"))?;
        Ok(ferris_schema::dyn_meta_json(&self.pool, &meta).await?)
    }

    pub(super) fn field_types() -> Vec<String> {
        let mut names: Vec<String> = ferris_schema::fieldtype::FIELDTYPES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        names.sort();
        names
    }
}
