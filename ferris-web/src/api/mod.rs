//! API: platform-aware data access for the Desk.

pub mod client;
pub mod dashboard_layout;
pub mod export;
pub mod files;
pub mod hooks;
pub mod print;
#[cfg(feature = "ssr")]
pub mod server;
pub mod store;
pub mod users;

// The wire contract and the metadata helpers are a pure leaf crate, so the
// browser and the server compile the exact same definitions. Re-exported here
// because the Desk addresses them as `api::meta` / `api::types` / `api::meta_types`.
pub use ferris_web_shared::{meta, meta_types, types};

pub use client::{
    api_cancel, api_create, api_delete, api_form_tours, api_get_raw, api_json, api_meta,
    api_post_raw, api_submit, api_update, log_client,
};
pub use export::export_csv;
pub use hooks::{
    MutationType, use_document, use_engagement, use_list_data, use_mutation,
    use_persisted_view_state,
};
pub use meta::{
    Column, ViewSpec, apply_filters, can_create, display_value, doc_label, docstatus_rank,
    filter_fields, form_fields, is_opaque_name, is_submittable, label_of, link_display,
    list_columns, matches_filter, record_name, standard_filter_fields, title_field, view_modes,
};
pub use meta_types::{DynField, DynMeta, DynPermissionRule, NamingType};
#[cfg(feature = "ssr")]
pub use server::DocStoreApi;
pub use store::DocStore;
pub use types::{
    ActivityEntry, ActivityKind, ApiResponse, DocComment, EngagementPage, FILTER_OPERATORS, Filter,
    FilterOp, LikeState, ListParams,
};

/// Run a future in the platform runtime (spawn on wasm, same executor on SSR).
pub fn drive<F>(fut: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    leptos::task::spawn_local(fut);
}
