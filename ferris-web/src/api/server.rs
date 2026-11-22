//! In-process engine bridge used during SSR so the Desk pre-renders real data
//! without self-HTTP calls. Not available on hydrate (see [`super::store`]).
//!
//! This is the only reason `ferris-web` depends on `ferris-core`, `ferris-db`,
//! `ferris-api` and `ferris-document`, and it is behind the `ssr` feature: a
//! plain `cargo check` and the wasm32 hydrate build never compile this file.
//!
//! The types crossing this boundary are the wire contract from
//! `ferris_schema::contract`, the same ones the REST client deserializes, so
//! SSR and hydrate cannot disagree about what a response looks like.

use leptos::prelude::RwSignal;
use serde_json::Value;

use super::meta_types::DynMeta;
use super::types::{ApiResponse, EngagementPage, LikeState, ListParams};

/// Shared in-process store.
#[derive(Clone)]
pub struct DocStoreApi {
    pool: ferris_db::Db,
    /// Identity of the render, scoped as the engine's ambient user.
    ///
    /// `ferris_api::DocStore` authorises through
    /// `ferris_document::current_user()`, not through a parameter, and the SSR
    /// response streams: a layer around the request future stops covering the
    /// render once the body is being polled. Capturing the identity here (from
    /// the same context `provide_desk_context` fills) keeps every engine call
    /// authorised, whenever it runs. `None` for a guest, which the engine denies
    /// as it would for an anonymous caller.
    ///
    /// Boxed because a `User` is large and `DocStore` already has a data-less
    /// `Client` variant; inlining it made the enum's two variants differ by
    /// hundreds of bytes on every value a view holds.
    user: Option<Box<ferris_auth::User>>,
}

impl DocStoreApi {
    /// Create from the pool provided in the Desk context (SSR only).
    pub fn from_context() -> Result<Self, String> {
        let pool = leptos::prelude::expect_context::<ferris_db::Db>();
        // The guest is what `provide_desk_context` supplies when the request had
        // no session; scoping it would pass a "user" with no roles to the
        // engine, so it is normalized to no ambient user (fails closed).
        let user = leptos::prelude::use_context::<ferris_auth::User>()
            .filter(|user| user.user_type != ferris_auth::UserType::Guest)
            .map(Box::new);
        Ok(Self { pool, user })
    }

    /// Run an engine call under the render's identity.
    async fn scoped<T, F>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        ferris_document::with_user(self.user.as_deref().cloned(), fut).await
    }

    /// The REST route's `Write` gate, authorised as the render's user.
    async fn ensure_writable(&self, doctype: &str, name: &str) -> Result<(), String> {
        self.scoped(ferris_api::ensure_writable(&self.pool, doctype, name))
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn meta(&self, doctype: &str) -> Result<Option<DynMeta>, String> {
        // The same resolver the REST handler uses, so SSR carries the stored
        // Actions/Links/Permission Types instead of only the registry mirror.
        ferris_api::doc_meta::effective_dyn_meta(&self.pool, doctype)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn form_tours(&self, doctype: &str) -> Result<Vec<Value>, String> {
        // Same reader the REST endpoint uses, so SSR and hydrate show the same
        // tours instead of the SSR pass silently rendering none.
        let tours = ferris_api::form_tours::tours(&self.pool, doctype)
            .await
            .map_err(|e| e.to_string())?;
        tours
            .into_iter()
            .map(|tour| serde_json::to_value(tour).map_err(|e| e.to_string()))
            .collect()
    }

    pub async fn list(
        &self,
        doctype: &str,
        params: &ListParams,
    ) -> Result<ApiResponse<Value>, String> {
        // Same struct the REST handler deserializes: the Desk already built a
        // canonical `ListRequest`, so there is nothing to re-map.
        let page_size = params.limit_page_length.max(1);
        let user = self.user();
        let resp = self
            .scoped(ferris_api::DocStore::list_requested(
                &self.pool,
                doctype,
                params,
                user.as_deref(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        let data = resp
            .data
            .into_iter()
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .collect();
        Ok(ApiResponse {
            data,
            total: resp.total,
            page: (params.limit_start / page_size) + 1,
            page_size: params.limit_page_length,
        })
    }

    /// Identity of the rendered user, for the predicates a document cannot
    /// answer on its own. Absent outside a Desk render, where there is no
    /// request to attribute.
    fn user(&self) -> Option<String> {
        use leptos::prelude::GetUntracked;
        leptos::prelude::use_context::<RwSignal<crate::web_user::WebUser>>()
            .map(|user| user.get_untracked().id)
    }

    /// The rendered user with roles, for the gates that need them (the engine's
    /// `check_permission` reads roles, not just an identity). Absent outside a
    /// Desk render.
    fn web_user(&self) -> Option<ferris_auth::User> {
        use leptos::prelude::GetUntracked;
        let web =
            leptos::prelude::use_context::<RwSignal<crate::web_user::WebUser>>()?.get_untracked();
        let mut user = ferris_auth::User::system_user(web.id.clone(), web.full_name.clone());
        user.id = web.id;
        user.roles = web.roles;
        Some(user)
    }

    pub async fn engagement(
        &self,
        doctype: &str,
        names: &[String],
    ) -> Result<EngagementPage, String> {
        let user = self.user().unwrap_or_default();
        // `read_page` already returns the contract type the client deserializes.
        ferris_api::engagement::read_page(
            &self.pool,
            doctype,
            &user,
            // Explicit hasher type: `read_page` is generic over the hasher to
            // keep `restrict` hasher-agnostic, so the `collect`
            // target can no longer be inferred from the parameter alone.
            names
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<String>>(),
        )
        .await
        .map_err(|e| e.to_string())
    }

    pub async fn toggle_like(&self, doctype: &str, name: &str) -> Result<LikeState, String> {
        // Mirror the REST route's `Read` gate (same as the other single-document
        // engagement methods).
        self.get(doctype, name).await?;
        let user = self.user().unwrap_or_default();
        ferris_document::LikeStore::new(self.pool.clone())
            .toggle(doctype, name, &user)
            .await
            .map_err(|e| e.to_string())
    }

    /// The comments of a document, oldest first (SSR reads the engine store
    /// in-process, exactly like the REST handler does).
    pub async fn list_comments(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<super::types::DocComment>, String> {
        // Mirror the REST route's `Read` gate so SSR and hydrate agree on who
        // may see a document's discussion.
        self.get(doctype, name).await?;
        ferris_document::CommentStore::new(self.pool.clone())
            .list(doctype, name)
            .await
            .map_err(|e| e.to_string())
    }

    /// Post a comment, or a reply when `parent` is set.
    pub async fn add_comment(
        &self,
        doctype: &str,
        name: &str,
        body: &str,
        parent: Option<&str>,
    ) -> Result<super::types::DocComment, String> {
        self.get(doctype, name).await?;
        let user = self.user().unwrap_or_default();
        ferris_document::CommentStore::new(self.pool.clone())
            .add(doctype, name, &user, body, parent)
            .await
            .map_err(|e| e.to_string())
    }

    /// A document's activity timeline, oldest first (SSR composes it in-process).
    pub async fn activity(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<super::types::ActivityEntry>, String> {
        // The REST route gates on `Read`; mirror that here so SSR and hydrate
        // cannot disagree about who may see a document's history.
        self.get(doctype, name).await?;
        ferris_document::ActivityStore::new(self.pool.clone())
            .feed(doctype, name)
            .await
            .map_err(|e| e.to_string())
    }

    /// A document's assignments, oldest first (SSR reads the store in-process).
    pub async fn list_assignments(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<super::types::Assignment>, String> {
        // Mirror the REST route's `Read` gate.
        self.get(doctype, name).await?;
        ferris_document::AssignmentStore::new(self.pool.clone())
            .list(doctype, name)
            .await
            .map_err(|e| e.to_string())
    }

    /// Assign `user` to a document.
    pub async fn add_assignment(
        &self,
        doctype: &str,
        name: &str,
        user: &str,
    ) -> Result<super::types::Assignment, String> {
        // Mutating the side table is a write on the document; the REST route
        // requires `Write`, so SSR must too.
        self.ensure_writable(doctype, name).await?;
        let actor = self.user().unwrap_or_default();
        ferris_document::AssignmentStore::new(self.pool.clone())
            .assign(doctype, name, user, &actor)
            .await
            .map_err(|e| e.to_string())
    }

    /// Remove `user` from a document's assignments.
    pub async fn remove_assignment(
        &self,
        doctype: &str,
        name: &str,
        user: &str,
    ) -> Result<(), String> {
        self.ensure_writable(doctype, name).await?;
        ferris_document::AssignmentStore::new(self.pool.clone())
            .unassign(doctype, name, user)
            .await
            .map_err(|e| e.to_string())
    }

    /// A document's attachments, oldest first.
    pub async fn list_attachments(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<super::types::Attachment>, String> {
        self.get(doctype, name).await?;
        ferris_document::AttachmentStore::new(self.pool.clone())
            .list(doctype, name)
            .await
            .map_err(|e| e.to_string())
    }

    /// Register an already-stored file as an attachment of a document.
    pub async fn add_attachment(
        &self,
        doctype: &str,
        name: &str,
        file_name: &str,
        file_url: &str,
        file_size: u64,
    ) -> Result<super::types::Attachment, String> {
        self.ensure_writable(doctype, name).await?;
        let actor = self.user().unwrap_or_default();
        ferris_document::AttachmentStore::new(self.pool.clone())
            .add(doctype, name, &actor, file_name, file_url, file_size)
            .await
            .map_err(|e| e.to_string())
    }

    /// Remove the attachment `id` from a document.
    pub async fn remove_attachment(
        &self,
        doctype: &str,
        name: &str,
        id: &str,
    ) -> Result<(), String> {
        self.ensure_writable(doctype, name).await?;
        ferris_document::AttachmentStore::new(self.pool.clone())
            .remove(doctype, name, id)
            .await
            .map_err(|e| e.to_string())
    }

    /// A document's share grants.
    pub async fn list_shares(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<super::types::Share>, String> {
        self.get(doctype, name).await?;
        let manager = self.share_manager().await?;
        let shares = manager
            .get_shares(doctype, name)
            .await
            .map_err(|e| e.to_string())?;
        shares.into_iter().map(to_wire_share).collect()
    }

    /// Share a document with `user`.
    pub async fn add_share(
        &self,
        doctype: &str,
        name: &str,
        user: &str,
        read: bool,
        write: bool,
        share: bool,
    ) -> Result<super::types::Share, String> {
        self.ensure_writable(doctype, name).await?;
        let mut permissions = Vec::new();
        if read {
            permissions.push(ferris_document::SharePermission::Read);
        }
        if write {
            permissions.push(ferris_document::SharePermission::Write);
        }
        if share {
            permissions.push(ferris_document::SharePermission::Share);
        }
        if permissions.is_empty() {
            permissions.push(ferris_document::SharePermission::Read);
        }
        let actor = self.user().unwrap_or_default();
        let manager = self.share_manager().await?;
        let grant = manager
            .share(doctype, name, user, permissions, &actor)
            .await
            .map_err(|e| e.to_string())?;
        to_wire_share(grant)
    }

    /// Remove `user`'s share grant on a document.
    pub async fn remove_share(&self, doctype: &str, name: &str, user: &str) -> Result<(), String> {
        self.ensure_writable(doctype, name).await?;
        let manager = self.share_manager().await?;
        manager
            .unshare(doctype, name, user)
            .await
            .map_err(|e| e.to_string())
    }

    /// The previous/next docname of a document in list order (SSR resolves it
    /// through the engine's list path, so permissions filter it).
    pub async fn neighbors(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<super::types::Neighbors, String> {
        self.scoped(ferris_api::DocStore::neighbors(&self.pool, doctype, name))
            .await
            .map_err(|e| e.to_string())
    }

    /// The role × custom-permission-type matrix of one DocType.
    pub async fn permission_matrix(
        &self,
        doctype: &str,
    ) -> Result<super::types::PermissionMatrix, String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::permission_admin::matrix(&self.pool, &user, doctype)
            .await
            .map_err(|e| e.to_string())
    }

    /// Grant/revoke is an interactive write: it runs through the REST API where
    /// the operator policy path is available for the immediate policy rebuild.
    /// The SSR bridge (a click handler never runs there) refuses explicitly
    /// rather than writing a row whose policy effect would be delayed.
    pub async fn set_custom_permission(
        &self,
        _doctype: &str,
        _role: &str,
        _perm_type: &str,
        _granted: bool,
    ) -> Result<(), String> {
        Err("custom permissions are changed through the HTTP API, not the SSR bridge".to_string())
    }

    /// The Connections panel of one document.
    ///
    /// Reading the document first mirrors the REST route's `Read` gate, so SSR
    /// cannot show connections for a document the caller may not open.
    pub async fn connections(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<super::types::Connections, String> {
        self.get(doctype, name).await?;
        self.scoped(ferris_api::doc_meta::connections(&self.pool, doctype, name))
            .await
            .map_err(|e| e.to_string())
    }

    /// The runtime customizations of a DocType.
    pub async fn get_customization(
        &self,
        doctype: &str,
    ) -> Result<super::types::Customization, String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::read_customization(&self.pool, &user, doctype)
            .await
            .map_err(|e| e.to_string())
    }

    /// Add or replace a custom field.
    pub async fn put_custom_field(
        &self,
        doctype: &str,
        spec: &super::types::CustomFieldSpec,
    ) -> Result<super::types::CustomFieldSpec, String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::save_custom_field(&self.pool, &user, doctype, spec)
            .await
            .map_err(|e| e.to_string())
    }

    /// Undo a custom field.
    pub async fn delete_custom_field(&self, doctype: &str, fieldname: &str) -> Result<(), String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::remove_custom_field(&self.pool, &user, doctype, fieldname)
            .await
            .map_err(|e| e.to_string())
    }

    /// Add or replace a property setter.
    pub async fn put_property_setter(
        &self,
        doctype: &str,
        spec: &super::types::PropertySetterSpec,
    ) -> Result<super::types::PropertySetterSpec, String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::save_property_setter(&self.pool, &user, doctype, spec)
            .await
            .map_err(|e| e.to_string())
    }

    /// Write this DocType's customization into a plugin world.
    ///
    /// `scope` narrows the file to the changes the caller selected; `None`
    /// exports the whole customization. Both go through the same
    /// `ferris_api::export_customization` the REST handler calls, so SSR cannot
    /// write a file the HTTP path would refuse.
    pub async fn export_customization(
        &self,
        doctype: &str,
        app: &str,
        scope: Option<&super::types::ExportScope>,
    ) -> Result<String, String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::export_customization(&self.pool, &user, doctype, app, scope)
            .await
            .map_err(|e| e.to_string())
    }

    /// Undo a property setter, optionally scoped to a field.
    pub async fn delete_property_setter(
        &self,
        doctype: &str,
        fieldname: Option<&str>,
        property: &str,
    ) -> Result<(), String> {
        let user = self.web_user().ok_or("no rendering user")?;
        ferris_api::remove_property_setter(&self.pool, &user, doctype, fieldname, property)
            .await
            .map_err(|e| e.to_string())
    }

    /// The share manager for this request, with its table created if needed.
    async fn share_manager(&self) -> Result<ferris_document::ShareManager, String> {
        let manager = ferris_document::ShareManager::with_db(self.pool.clone());
        manager.init().await.map_err(|e| e.to_string())?;
        Ok(manager)
    }

    pub async fn get(&self, doctype: &str, name: &str) -> Result<Value, String> {
        self.scoped(ferris_api::DocStore::get(&self.pool, doctype, name))
            .await
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    }

    pub async fn get_single(&self, doctype: &str) -> Result<Value, String> {
        self.scoped(ferris_api::DocStore::get_single(&self.pool, doctype))
            .await
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    }

    pub async fn save_single(&self, doctype: &str, body: Value) -> Result<Value, String> {
        let core: ferris_core::Value = serde_json::from_value(body).map_err(|e| e.to_string())?;
        self.scoped(ferris_api::DocStore::save_single(&self.pool, doctype, core))
            .await
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    }

    pub async fn create(&self, doctype: &str, body: Value) -> Result<Value, String> {
        let core: ferris_core::Value = serde_json::from_value(body).map_err(|e| e.to_string())?;
        self.scoped(ferris_api::DocStore::create(&self.pool, doctype, core))
            .await
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    }

    pub async fn update(&self, doctype: &str, name: &str, body: Value) -> Result<Value, String> {
        let core: ferris_core::Value = serde_json::from_value(body).map_err(|e| e.to_string())?;
        self.scoped(ferris_api::DocStore::update(
            &self.pool, doctype, name, core,
        ))
        .await
        .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
        .map_err(|e| e.to_string())
    }

    /// Rename a document (move its primary key).
    pub async fn rename(&self, doctype: &str, name: &str, new_name: &str) -> Result<Value, String> {
        self.scoped(ferris_api::DocStore::rename(
            &self.pool, doctype, name, new_name,
        ))
        .await
        .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
        .map_err(|e| e.to_string())
    }

    pub async fn delete(&self, doctype: &str, name: &str) -> Result<(), String> {
        self.scoped(ferris_api::DocStore::delete(&self.pool, doctype, name))
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn submit(&self, doctype: &str, name: &str) -> Result<Value, String> {
        self.scoped(ferris_api::DocStore::set_docstatus(
            &self.pool,
            doctype,
            name,
            ferris_document::DocStatus::Submitted,
        ))
        .await
        .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
        .map_err(|e| e.to_string())
    }

    pub async fn cancel(&self, doctype: &str, name: &str) -> Result<Value, String> {
        self.scoped(ferris_api::DocStore::set_docstatus(
            &self.pool,
            doctype,
            name,
            ferris_document::DocStatus::Cancelled,
        ))
        .await
        .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
        .map_err(|e| e.to_string())
    }

    pub async fn amend(&self, doctype: &str, name: &str) -> Result<Value, String> {
        self.scoped(ferris_api::DocStore::amend(&self.pool, doctype, name))
            .await
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    }
}

/// Map the engine's `chrono`-bearing `DocShare` to the string-timestamped wire
/// [`Share`](super::types::Share).
///
/// A serde round trip does the work: `DocShare` serializes to RFC 3339 strings
/// and the wire `Share` deserializes the same spelling, so the two stay in sync
/// without a hand-written field-by-field copy that could silently drop one.
fn to_wire_share(share: ferris_document::DocShare) -> Result<super::types::Share, String> {
    serde_json::to_value(share)
        .map_err(|e| e.to_string())
        .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
}
