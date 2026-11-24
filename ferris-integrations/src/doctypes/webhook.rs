//! The `Webhook` DocType.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager};

/// A webhook fired on a document event.
#[must_use]
pub fn webhook_meta() -> DoctypeMeta {
    DoctypeMeta::new("Webhook", MODULE)
        .description("An outbound HTTP call triggered by a document event.")
        .title_field("request_url")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("webhook_doctype")
                .label("Document Type")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::select(
                "webhook_docevent",
                "after_insert\non_update\non_submit\non_cancel\non_trash",
            )
            .label("Document Event")
            .default_value("after_insert")
            .in_list_view(),
        )
        .field(FieldMeta::small_text("condition").label("Condition"))
        .field(
            FieldMeta::url("request_url")
                .label("Request URL")
                .required(),
        )
        .field(
            FieldMeta::select("request_method", "POST\nPUT\nPATCH\nDELETE")
                .label("Request Method")
                .default_value("POST"),
        )
        .field(FieldMeta::code("request_headers").label("Request Headers"))
        .field(FieldMeta::code("request_body").label("Request Body"))
        .field(FieldMeta::secret("webhook_secret").label("Webhook Secret"))
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .permission(system_manager())
}
