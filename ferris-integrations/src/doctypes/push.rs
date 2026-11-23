//! The `Push Notification` DocType.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager};

/// A push notification queued for delivery to a device.
#[must_use]
pub fn push_notification_meta() -> DoctypeMeta {
    DoctypeMeta::new("Push Notification", MODULE)
        .description("A push notification queued for one user.")
        .title_field("title")
        .sort("creation", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("title")
                .label("Title")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::text("body").label("Body"))
        .field(FieldMeta::link("user", "User").label("User"))
        .field(FieldMeta::data("document_type").label("Document Type"))
        .field(FieldMeta::data("document_name").label("Document Name"))
        .field(
            FieldMeta::select("status", "Queued\nSent\nFailed")
                .label("Status")
                .default_value("Queued")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("sent_at").label("Sent At"))
        .field(FieldMeta::small_text("error").label("Error"))
        .field(
            FieldMeta::int("attempts")
                .label("Attempts")
                .default_value("0")
                .description("Delivery attempts so far; the sweep stops at the retry budget."),
        )
        .field(
            FieldMeta::datetime("next_attempt_at")
                .label("Next Attempt At")
                .description(
                    "Backoff schedule for a Failed row; empty means due now or exhausted.",
                ),
        )
        .permission(system_manager())
}
