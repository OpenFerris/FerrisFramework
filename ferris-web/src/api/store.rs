//! Platform-aware data access for the Desk.
//!
//! - `ssr` (native, in-process): talks to the engine via
//!   [`super::server::DocStoreApi`], pre-rendering real data without self-HTTP.
//! - everything else (the browser, and any native build without `ssr`): talks
//!   to the REST API via [`super::client`].
//!
//! The split is on the **feature**, not on `target_arch`: gating on
//! `target_arch` meant a native `cargo check` of the lib had to have the whole
//! engine in the graph even when nothing asked for SSR. Views only depend on
//! this uniform facade.

use ferris_web_shared::route::path_segment;
use serde::Serialize;
use serde_json::Value;

use super::meta_types::DynMeta;
#[cfg(feature = "ssr")]
use super::server::DocStoreApi;
use super::types::{
    ActivityEntry, ApiResponse, Assignment, Attachment, CustomFieldSpec, Customization, DocComment,
    EngagementPage, ExportScope, LikeState, ListParams, Neighbors, PropertySetterSpec, Share,
};

/// Uniform document store facade.
pub enum DocStore {
    #[cfg(feature = "ssr")]
    Server(DocStoreApi),
    Client,
}

impl DocStore {
    /// Obtain the runtime facade for the current platform.
    pub fn runtime() -> Result<Self, String> {
        #[cfg(feature = "ssr")]
        {
            Ok(DocStore::Server(DocStoreApi::from_context()?))
        }
        #[cfg(not(feature = "ssr"))]
        {
            Ok(DocStore::Client)
        }
    }

    pub async fn meta(&self, doctype: &str) -> Result<Option<DynMeta>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.meta(doctype).await,
            DocStore::Client => super::client::api_meta(doctype).await,
        }
    }

    /// The guided tours declared for `doctype` (empty when none exist).
    pub async fn form_tours(&self, doctype: &str) -> Result<Vec<Value>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.form_tours(doctype).await,
            DocStore::Client => super::client::api_form_tours(doctype).await,
        }
    }

    pub async fn list(
        &self,
        doctype: &str,
        params: &ListParams,
    ) -> Result<ApiResponse<Value>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.list(doctype, params).await,
            DocStore::Client => super::client::api_list(params, doctype).await,
        }
    }

    /// Like and comment state of `names`, read in one round trip so the list
    /// does not ask per row.
    pub async fn engagement(
        &self,
        doctype: &str,
        names: &[String],
    ) -> Result<EngagementPage, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.engagement(doctype, names).await,
            DocStore::Client => super::client::api_engagement(doctype, names).await,
        }
    }

    /// Like `name`, or unlike it when already liked.
    pub async fn toggle_like(&self, doctype: &str, name: &str) -> Result<LikeState, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.toggle_like(doctype, name).await,
            DocStore::Client => super::client::api_toggle_like(doctype, name).await,
        }
    }

    /// The comments of a document, oldest first.
    pub async fn list_comments(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<DocComment>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.list_comments(doctype, name).await,
            DocStore::Client => super::client::api_list_comments(doctype, name).await,
        }
    }

    /// Post a comment, or a reply when `parent` is set.
    pub async fn add_comment(
        &self,
        doctype: &str,
        name: &str,
        body: &str,
        parent: Option<&str>,
    ) -> Result<DocComment, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.add_comment(doctype, name, body, parent).await,
            DocStore::Client => super::client::api_add_comment(doctype, name, body, parent).await,
        }
    }

    /// A document's activity timeline, oldest first.
    pub async fn activity(&self, doctype: &str, name: &str) -> Result<Vec<ActivityEntry>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.activity(doctype, name).await,
            DocStore::Client => super::client::api_activity(doctype, name).await,
        }
    }

    /// A document's assignments, oldest first.
    pub async fn list_assignments(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<Assignment>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.list_assignments(doctype, name).await,
            DocStore::Client => super::client::api_list_assignments(doctype, name).await,
        }
    }

    /// Assign `user` to a document.
    pub async fn add_assignment(
        &self,
        doctype: &str,
        name: &str,
        user: &str,
    ) -> Result<Assignment, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.add_assignment(doctype, name, user).await,
            DocStore::Client => super::client::api_add_assignment(doctype, name, user).await,
        }
    }

    /// Remove `user` from a document's assignments.
    pub async fn remove_assignment(
        &self,
        doctype: &str,
        name: &str,
        user: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.remove_assignment(doctype, name, user).await,
            DocStore::Client => super::client::api_remove_assignment(doctype, name, user).await,
        }
    }

    /// A document's attachments, oldest first.
    pub async fn list_attachments(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<Vec<Attachment>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.list_attachments(doctype, name).await,
            DocStore::Client => super::client::api_list_attachments(doctype, name).await,
        }
    }

    /// Register an already-stored file as an attachment of a document.
    pub async fn add_attachment(
        &self,
        doctype: &str,
        name: &str,
        file_name: &str,
        file_url: &str,
        file_size: u64,
    ) -> Result<Attachment, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => {
                api.add_attachment(doctype, name, file_name, file_url, file_size)
                    .await
            }
            DocStore::Client => {
                super::client::api_add_attachment(doctype, name, file_name, file_url, file_size)
                    .await
            }
        }
    }

    /// Remove the attachment `id` from a document.
    pub async fn remove_attachment(
        &self,
        doctype: &str,
        name: &str,
        id: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.remove_attachment(doctype, name, id).await,
            DocStore::Client => super::client::api_remove_attachment(doctype, name, id).await,
        }
    }

    /// A document's share grants.
    pub async fn list_shares(&self, doctype: &str, name: &str) -> Result<Vec<Share>, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.list_shares(doctype, name).await,
            DocStore::Client => super::client::api_list_shares(doctype, name).await,
        }
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
    ) -> Result<Share, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.add_share(doctype, name, user, read, write, share).await,
            DocStore::Client => {
                super::client::api_add_share(doctype, name, user, read, write, share).await
            }
        }
    }

    /// Remove `user`'s share grant on a document.
    pub async fn remove_share(&self, doctype: &str, name: &str, user: &str) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.remove_share(doctype, name, user).await,
            DocStore::Client => super::client::api_remove_share(doctype, name, user).await,
        }
    }

    /// The previous/next docname of a document in list order.
    /// Call a whitelisted server method, as a `Server Action` does.
    ///
    /// The click handler that calls this only runs in the browser, so the SSR
    /// branch is unreachable by construction; it still returns an explicit
    /// error rather than pretending an in-process registry exists (the SSR
    /// bridge owns a pool, not the server's `MethodRegistry`).
    pub async fn call_method(&self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(_) => Err(format!(
                "`{method}` is a Server Action and must run through the HTTP API, not the SSR bridge"
            )),
            DocStore::Client => super::client::api_call_method(method, params).await,
        }
    }

    /// The role × custom-permission-type matrix of one DocType.
    pub async fn permission_matrix(
        &self,
        doctype: &str,
    ) -> Result<super::types::PermissionMatrix, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.permission_matrix(doctype).await,
            DocStore::Client => super::client::api_permission_matrix(doctype).await,
        }
    }

    /// Grant or revoke one custom permission type for one role.
    pub async fn set_custom_permission(
        &self,
        doctype: &str,
        role: &str,
        perm_type: &str,
        granted: bool,
    ) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => {
                api.set_custom_permission(doctype, role, perm_type, granted)
                    .await
            }
            DocStore::Client => {
                super::client::api_set_custom_permission(doctype, role, perm_type, granted).await
            }
        }
    }

    /// The Connections panel of one document.
    ///
    /// A server that predates the endpoint answers 404; the caller treats that
    /// as "no declared connections", which is the same thing the panel would
    /// render for a DocType with no `DocType Link` rows.
    pub async fn connections(
        &self,
        doctype: &str,
        name: &str,
    ) -> Result<super::types::Connections, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.connections(doctype, name).await,
            DocStore::Client => super::client::api_connections(doctype, name).await,
        }
    }

    pub async fn neighbors(&self, doctype: &str, name: &str) -> Result<Neighbors, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.neighbors(doctype, name).await,
            DocStore::Client => super::client::api_neighbors(doctype, name).await,
        }
    }

    /// The runtime customizations of a DocType.
    pub async fn get_customization(&self, doctype: &str) -> Result<Customization, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.get_customization(doctype).await,
            DocStore::Client => super::client::api_get_customization(doctype).await,
        }
    }

    /// Add or replace a custom field.
    pub async fn put_custom_field(
        &self,
        doctype: &str,
        spec: &CustomFieldSpec,
    ) -> Result<CustomFieldSpec, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.put_custom_field(doctype, spec).await,
            DocStore::Client => super::client::api_put_custom_field(doctype, spec).await,
        }
    }

    /// Undo a custom field.
    pub async fn delete_custom_field(&self, doctype: &str, fieldname: &str) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.delete_custom_field(doctype, fieldname).await,
            DocStore::Client => super::client::api_delete_custom_field(doctype, fieldname).await,
        }
    }

    /// Add or replace a property setter.
    pub async fn put_property_setter(
        &self,
        doctype: &str,
        spec: &PropertySetterSpec,
    ) -> Result<PropertySetterSpec, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.put_property_setter(doctype, spec).await,
            DocStore::Client => super::client::api_put_property_setter(doctype, spec).await,
        }
    }

    /// Write this DocType's customization into a plugin world.
    ///
    /// `scope` restricts the file to the changes the caller selected; `None`
    /// exports the whole customization. The same scope reaches the engine on
    /// both branches, so SSR and hydrate cannot write different files.
    ///
    /// # Errors
    ///
    /// The API's message (permission, invalid app name, a scope that selects
    /// nothing, filesystem).
    pub async fn export_customization(
        &self,
        doctype: &str,
        app: &str,
        scope: Option<ExportScope>,
    ) -> Result<String, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.export_customization(doctype, app, scope.as_ref()).await,
            DocStore::Client => super::client::api_export_customization(doctype, app, scope).await,
        }
    }

    /// Undo a property setter, optionally scoped to a field.
    pub async fn delete_property_setter(
        &self,
        doctype: &str,
        fieldname: Option<&str>,
        property: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => {
                api.delete_property_setter(doctype, fieldname, property)
                    .await
            }
            DocStore::Client => {
                super::client::api_delete_property_setter(doctype, fieldname, property).await
            }
        }
    }

    pub async fn get(&self, doctype: &str, name: &str) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.get(doctype, name).await,
            DocStore::Client => super::client::api_get_raw(&resource_path(doctype, name))
                .await
                .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string())),
        }
    }

    /// Read the one document of a Single DocType.
    pub async fn get_single(&self, doctype: &str) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.get_single(doctype).await,
            DocStore::Client => super::client::api_get_single(doctype).await,
        }
    }

    /// Save the one document of a Single DocType (upsert).
    pub async fn save_single(&self, doctype: &str, body: impl Serialize) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => {
                let value = serde_json::to_value(body).map_err(|e| e.to_string())?;
                api.save_single(doctype, value).await
            }
            DocStore::Client => {
                let value = serde_json::to_value(body).map_err(|e| e.to_string())?;
                super::client::api_put_single(doctype, value).await
            }
        }
    }

    pub async fn create(&self, doctype: &str, body: impl Serialize) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => {
                let value = serde_json::to_value(body).map_err(|e| e.to_string())?;
                api.create(doctype, value).await
            }
            DocStore::Client => {
                let value = serde_json::to_value(body).map_err(|e| e.to_string())?;
                super::client::api_create(doctype, value).await
            }
        }
    }

    pub async fn update(
        &self,
        doctype: &str,
        name: &str,
        body: impl Serialize,
    ) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => {
                let value = serde_json::to_value(body).map_err(|e| e.to_string())?;
                api.update(doctype, name, value).await
            }
            DocStore::Client => {
                let value = serde_json::to_value(body).map_err(|e| e.to_string())?;
                super::client::api_update(doctype, name, value).await
            }
        }
    }

    /// Rename a document (move its primary key).
    ///
    /// # Errors
    ///
    /// A permission error, a validation error for the new name, or
    /// `AlreadyExists` when the target name is taken.
    pub async fn rename(&self, doctype: &str, name: &str, new_name: &str) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.rename(doctype, name, new_name).await,
            DocStore::Client => super::client::api_rename(doctype, name, new_name).await,
        }
    }

    pub async fn delete(&self, doctype: &str, name: &str) -> Result<(), String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.delete(doctype, name).await,
            DocStore::Client => super::client::api_delete(doctype, name).await,
        }
    }

    pub async fn submit(&self, doctype: &str, name: &str) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.submit(doctype, name).await,
            DocStore::Client => super::client::api_submit(doctype, name).await,
        }
    }

    pub async fn cancel(&self, doctype: &str, name: &str) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.cancel(doctype, name).await,
            DocStore::Client => super::client::api_cancel(doctype, name).await,
        }
    }

    pub async fn amend(&self, doctype: &str, name: &str) -> Result<Value, String> {
        match self {
            #[cfg(feature = "ssr")]
            DocStore::Server(api) => api.amend(doctype, name).await,
            DocStore::Client => super::client::api_amend(doctype, name).await,
        }
    }
}

/// The URL of one document (`GET /api/resource/{doctype}/{name}`).
///
/// Doctype and docname are data: a name may contain `/`, `?` or `#`, so both
/// segments go through [`path_segment`]. Interpolating them raw let a document
/// called `a/b` address `/api/resource/Task/a/` and made `?x` rewrite the
/// query of another endpoint. The rest of the client already encodes through
/// the same helper; this is the one call that built its path by hand.
fn resource_path(doctype: &str, name: &str) -> String {
    format!(
        "/api/resource/{}/{}",
        path_segment(doctype),
        path_segment(name)
    )
}

#[cfg(test)]
mod tests {
    use super::resource_path;

    /// A docname is data. `/` and `?` must stay inside one path segment, or
    /// `DocStore::get("Task", "a/b?x=1")` reads a different endpoint.
    #[test]
    fn resource_path_encodes_both_dynamic_segments() {
        assert_eq!(
            resource_path("Task", "TASK-0001"),
            "/api/resource/Task/TASK-0001"
        );
        assert_eq!(
            resource_path("Sales Invoice", "a/b?c#d"),
            "/api/resource/Sales%20Invoice/a%2Fb%3Fc%23d"
        );
        assert_eq!(resource_path("Task", "100%"), "/api/resource/Task/100%25");
    }

    /// The encoded form cannot grow a second segment or a query.
    #[test]
    fn encoded_names_do_not_add_path_or_query() {
        let path = resource_path("Task", "a/b?x=1");
        assert!(!path.contains('?'));
        assert_eq!(path.matches('/').count(), 4);
    }
}
