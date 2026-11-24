//! The automation DocTypes: `Automation Flow`, `Automation Trigger Queue`,
//! `Assignment Rule` and `Milestone`.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager};

/// An automation rule: a condition and the actions it runs.
#[must_use]
pub fn automation_flow_meta() -> DoctypeMeta {
    DoctypeMeta::new("Automation Flow", MODULE)
        .description("A document-event rule and the actions it runs.")
        .title_field("flow_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("flow_name")
                .label("Flow Name")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::link("document_type", "DocType")
                .label("Document Type")
                .in_list_view(),
        )
        .field(
            FieldMeta::select(
                "event",
                "after_insert\non_update\non_submit\non_cancel\nscheduled",
            )
            .label("Event")
            .default_value("after_insert")
            .in_list_view(),
        )
        .field(FieldMeta::small_text("condition").label("Condition"))
        .field(FieldMeta::code("actions").label("Actions"))
        .field(
            FieldMeta::check("run_once")
                .label("Run Once")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .permission(system_manager())
}

/// The queue of automation flows waiting to run.
#[must_use]
pub fn automation_trigger_queue_meta() -> DoctypeMeta {
    DoctypeMeta::new("Automation Trigger Queue", MODULE)
        .description("A queued automation trigger and its run state.")
        .sort("creation", "asc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::link("flow", "Automation Flow")
                .label("Flow")
                .in_list_view(),
        )
        .field(FieldMeta::link("document_type", "DocType").label("Document Type"))
        .field(
            FieldMeta::dynamic_link("document_name", "document_type")
                .label("Document Name")
                .in_list_view(),
        )
        .field(FieldMeta::data("event").label("Event"))
        .field(
            FieldMeta::select("status", "Queued\nRunning\nCompleted\nFailed")
                .label("Status")
                .default_value("Queued")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("started_at").label("Started At"))
        .field(FieldMeta::datetime("finished_at").label("Finished At"))
        .field(FieldMeta::small_text("error").label("Error"))
        .permission(system_manager())
}

/// A rule that assigns documents to a user or role.
#[must_use]
pub fn assignment_rule_meta() -> DoctypeMeta {
    DoctypeMeta::new("Assignment Rule", MODULE)
        .description("Assigns documents of a type to a user or role.")
        .title_field("rule_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("rule_name")
                .label("Rule Name")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::link("document_type", "DocType")
                .label("Document Type")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::small_text("description").label("Description"))
        .field(FieldMeta::small_text("condition").label("Condition"))
        .field(FieldMeta::link("assign_to", "User").label("Assign To"))
        .field(FieldMeta::link("assign_to_role", "Role").label("Assign To Role"))
        .field(
            FieldMeta::int("priority")
                .label("Priority")
                .default_value("0"),
        )
        .field(FieldMeta::data("due_date_based_on").label("Due Date Based On"))
        .field(
            FieldMeta::check("disabled")
                .label("Disabled")
                .default_value("0"),
        )
        .permission(system_manager())
}

/// A tracked milestone on a document's lifecycle.
#[must_use]
pub fn milestone_meta() -> DoctypeMeta {
    DoctypeMeta::new("Milestone", MODULE)
        .description("A milestone tracked against a document's lifecycle.")
        .title_field("milestone")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("milestone")
                .label("Milestone")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::link("document_type", "DocType").label("Document Type"))
        .field(
            FieldMeta::dynamic_link("document_name", "document_type")
                .label("Document Name")
                .in_list_view(),
        )
        .field(FieldMeta::data("track_field").label("Track Field"))
        .field(FieldMeta::data("value").label("Value"))
        .field(
            FieldMeta::check("achieved")
                .label("Achieved")
                .default_value("0"),
        )
        .field(FieldMeta::datetime("achieved_on").label("Achieved On"))
        .permission(system_manager())
}
