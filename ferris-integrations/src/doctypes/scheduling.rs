//! The scheduled-integration DocTypes: `Auto Repeat`, `Auto Email Report` and
//! `Reminder`.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager};

/// A recurring duplicate of a document.
#[must_use]
pub fn auto_repeat_meta() -> DoctypeMeta {
    DoctypeMeta::new("Auto Repeat", MODULE)
        .description("Periodically creates a fresh copy of a document.")
        .title_field("reference_document")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("reference_doctype")
                .label("Reference DocType")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::data("reference_document")
                .label("Reference Document")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::select("frequency", "Daily\nWeekly\nMonthly\nQuarterly\nYearly")
                .label("Frequency")
                .default_value("Monthly"),
        )
        .field(FieldMeta::date("start_date").label("Start Date"))
        .field(FieldMeta::date("end_date").label("End Date"))
        .field(FieldMeta::date("next_schedule_date").label("Next Schedule Date"))
        .field(FieldMeta::int("repeat_on_day").label("Repeat On Day"))
        .field(
            FieldMeta::check("disabled")
                .label("Disabled")
                .default_value("0"),
        )
        .permission(system_manager())
}

/// A report emailed on a schedule.
#[must_use]
pub fn auto_email_report_meta() -> DoctypeMeta {
    DoctypeMeta::new("Auto Email Report", MODULE)
        .description("Emails a report on a recurring schedule.")
        .title_field("report")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("report")
                .label("Report")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::link("user", "User").label("User"))
        .field(FieldMeta::data("email_to").label("Email To").in_list_view())
        .field(
            FieldMeta::select("frequency", "Daily\nWeekly\nMonthly")
                .label("Frequency")
                .default_value("Weekly"),
        )
        .field(FieldMeta::code("filters").label("Filters"))
        .field(FieldMeta::small_text("description").label("Description"))
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .field(FieldMeta::datetime("last_sent_on").label("Last Sent On"))
        .permission(system_manager())
}

/// A scheduled reminder about a document.
#[must_use]
pub fn reminder_meta() -> DoctypeMeta {
    DoctypeMeta::new("Reminder", MODULE)
        .description("A scheduled reminder about a document.")
        .title_field("reminder")
        .sort("remind_at", "asc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("reminder")
                .label("Reminder")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("document_type").label("Document Type"))
        .field(
            FieldMeta::data("document_name")
                .label("Document Name")
                .in_list_view(),
        )
        .field(
            FieldMeta::datetime("remind_at")
                .label("Remind At")
                .in_list_view(),
        )
        .field(
            FieldMeta::select("notify", "Email\nNotification\nBoth")
                .label("Notify")
                .default_value("Notification"),
        )
        .field(FieldMeta::link("user", "User").label("User"))
        .field(FieldMeta::small_text("description").label("Description"))
        .field(FieldMeta::check("sent").label("Sent").default_value("0"))
        .permission(system_manager())
}
