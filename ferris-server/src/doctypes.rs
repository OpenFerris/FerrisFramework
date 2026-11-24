//! DocTypes declared by `ferris-server` (the System section).
//!
//! The framework/system DocTypes: the health and version reports, the system
//! settings Single, the scheduler/job/worker logs and the storage reports
//!. The registry lives in `ferris-schema`; this
//! module only declares the metas and registers them, and the host calls it
//! from [`crate::catalog::register_doctype_catalog`] (§14.1).
//!
//! `Activity Log` and `Access Log` are deliberately **not** declared here: they
//! are declared by `ferris-auth` (the Users section) so the install has one
//! declaration of each (§16.2/§16.9, plan §11 #8). The System workspace may
//! still link to them; a link is not a declaration.
#![allow(clippy::doc_markdown)]

use ferris_schema::{DoctypeMeta, FieldMeta, RolePermission};

/// Desk module/section every DocType here belongs to.
pub const MODULE: &str = "System";

/// Full CRUD for the administrator role.
#[must_use]
fn system_manager() -> RolePermission {
    RolePermission::new("System Manager")
        .read()
        .write()
        .create()
        .delete()
}

/// Read + write with neither create nor delete: the policy of a Single.
#[must_use]
fn system_manager_single() -> RolePermission {
    RolePermission::new("System Manager").read().write()
}

/// Audit rows are append-only. `System Manager` may read them; no role is
/// granted write, create or delete, and the declaration is `read_only` so the
/// Desk does not offer a mutation either (SEC-07/08/09/DX-01).
#[must_use]
fn audit_read() -> RolePermission {
    RolePermission::new("System Manager").read()
}

/// Single: install-wide system defaults.
#[must_use]
pub fn system_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("System Settings", MODULE)
        .single()
        .description("Install-wide system defaults.")
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(FieldMeta::link("language", "Language").label("Language"))
        .field(FieldMeta::data("time_zone").label("Time Zone"))
        .field(FieldMeta::data("date_format").label("Date Format"))
        .field(
            FieldMeta::int("float_precision")
                .label("Float Precision")
                .default_value("2"),
        )
        .field(
            FieldMeta::check("enable_scheduler")
                .label("Enable Scheduler")
                .default_value("1"),
        )
        .field(
            FieldMeta::check("enable_telemetry")
                .label("Enable Telemetry")
                .default_value("0"),
        )
        .permission(system_manager_single())
}

/// A recorded request path and its outcome.
#[must_use]
pub fn path_log_meta() -> DoctypeMeta {
    DoctypeMeta::new("Path Log", MODULE)
        .description("A recorded request path and its outcome.")
        .sort("creation", "desc")
        .read_only()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(FieldMeta::data("method").label("Method").in_list_view())
        .field(FieldMeta::data("path").label("Path").in_list_view())
        .field(
            FieldMeta::int("status_code")
                .label("Status Code")
                .in_list_view(),
        )
        .field(FieldMeta::int("duration_ms").label("Duration (ms)"))
        .field(FieldMeta::link("user", "User").label("User"))
        .field(FieldMeta::data("ip_address").label("IP Address"))
        .permission(audit_read())
}

/// A released version of the application.
#[must_use]
pub fn version_meta() -> DoctypeMeta {
    DoctypeMeta::new("Version", MODULE)
        .description("A released version of the application.")
        .title_field("version")
        .sort("released_on", "desc")
        .field(
            FieldMeta::data("version")
                .label("Version")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::date("released_on")
                .label("Released On")
                .in_list_view(),
        )
        .field(FieldMeta::data("commit").label("Commit"))
        .field(FieldMeta::text("notes").label("Notes"))
        .permission(system_manager())
}

/// The cron definition of one schedulable job.
#[must_use]
pub fn scheduled_job_type_meta() -> DoctypeMeta {
    DoctypeMeta::new("Scheduled Job Type", MODULE)
        .description("The cron definition of one schedulable job.")
        .sort("next_execution", "asc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("cron_format")
                .label("Cron Format")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("last_execution").label("Last Execution"))
        .field(FieldMeta::datetime("next_execution").label("Next Execution"))
        .field(
            FieldMeta::check("disabled")
                .label("Disabled")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("stopped")
                .label("Stopped")
                .default_value("0"),
        )
        .permission(system_manager())
}

/// One execution of a scheduled job.
#[must_use]
pub fn scheduled_job_log_meta() -> DoctypeMeta {
    DoctypeMeta::new("Scheduled Job Log", MODULE)
        .description("One execution of a scheduled job.")
        .sort("creation", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::link("job_type", "Scheduled Job Type")
                .label("Job Type")
                .in_list_view(),
        )
        .field(
            FieldMeta::select("status", "Scheduled\nQueued\nRunning\nFinished\nFailed")
                .label("Status")
                .default_value("Scheduled")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("start_time").label("Start Time"))
        .field(FieldMeta::datetime("end_time").label("End Time"))
        .field(FieldMeta::text("output").label("Output"))
        .field(FieldMeta::text("error").label("Error"))
        .permission(system_manager())
}

/// One job in the background queue.
#[must_use]
pub fn rq_job_meta() -> DoctypeMeta {
    DoctypeMeta::new("RQ Job", MODULE)
        .description("One job in the background queue.")
        .title_field("job_id")
        .sort("creation", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("job_id")
                .label("Job ID")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("queue").label("Queue").in_list_view())
        .field(
            FieldMeta::select("status", "Queued\nStarted\nFinished\nFailed")
                .label("Status")
                .default_value("Queued")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("started_at").label("Started At"))
        .field(FieldMeta::datetime("ended_at").label("Ended At"))
        .field(FieldMeta::text("result").label("Result"))
        .field(FieldMeta::text("exc_info").label("Exception"))
        .permission(system_manager())
}

/// A worker process consuming the background queue.
#[must_use]
pub fn rq_worker_meta() -> DoctypeMeta {
    DoctypeMeta::new("RQ Worker", MODULE)
        .description("A worker process consuming the background queue.")
        .title_field("worker_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(
            FieldMeta::data("worker_name")
                .label("Worker Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("queue").label("Queue").in_list_view())
        .field(
            FieldMeta::select("status", "Idle\nBusy\nStopped")
                .label("Status")
                .default_value("Idle")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("last_heartbeat").label("Last Heartbeat"))
        .permission(system_manager())
}

/// A captured server error.
#[must_use]
pub fn error_log_meta() -> DoctypeMeta {
    DoctypeMeta::new("Error Log", MODULE)
        .description("A captured server error.")
        .sort("creation", "desc")
        .read_only()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("error_type")
                .label("Error Type")
                .in_list_view(),
        )
        .field(FieldMeta::text("error").label("Error").in_list_view())
        .field(FieldMeta::link("reference_doctype", "DocType").label("Reference DocType"))
        .field(
            FieldMeta::dynamic_link("reference_name", "reference_doctype")
                .label("Reference Document"),
        )
        .field(FieldMeta::text("traceback").label("Traceback"))
        .permission(audit_read())
}

/// A recorded API call and its outcome.
#[must_use]
pub fn api_request_log_meta() -> DoctypeMeta {
    DoctypeMeta::new("API Request Log", MODULE)
        .description("A recorded API call and its outcome.")
        .sort("creation", "desc")
        .read_only()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(FieldMeta::data("method").label("Method").in_list_view())
        .field(FieldMeta::data("url").label("URL").in_list_view())
        .field(
            FieldMeta::int("status_code")
                .label("Status Code")
                .in_list_view(),
        )
        .field(FieldMeta::int("duration_ms").label("Duration (ms)"))
        .field(FieldMeta::link("user", "User").label("User"))
        .field(FieldMeta::data("ip_address").label("IP Address"))
        .permission(audit_read())
}

/// A document opened by a user.
#[must_use]
pub fn view_log_meta() -> DoctypeMeta {
    DoctypeMeta::new("View Log", MODULE)
        .description("A document opened by a user.")
        .sort("creation", "desc")
        .read_only()
        .field(FieldMeta::link("user", "User").label("User").in_list_view())
        .field(
            FieldMeta::link("reference_doctype", "DocType")
                .label("Reference DocType")
                .in_list_view(),
        )
        .field(
            FieldMeta::dynamic_link("reference_name", "reference_doctype")
                .label("Reference Document")
                .in_list_view(),
        )
        .permission(audit_read())
}

/// Every DocType this crate declares, in registration order.
#[must_use]
pub fn doctype_metas() -> Vec<DoctypeMeta> {
    vec![
        // Direct
        system_settings_meta(),
        path_log_meta(),
        version_meta(),
        // Jobs
        scheduled_job_type_meta(),
        scheduled_job_log_meta(),
        rq_job_meta(),
        rq_worker_meta(),
        // Logs (Activity Log / Access Log live in ferris-auth; see the module docs)
        error_log_meta(),
        api_request_log_meta(),
        view_log_meta(),
        // Reports
    ]
}

/// Register this crate's DocTypes in the global schema registry.
///
/// Idempotent: re-registering overwrites the same keys, so it is safe on every
/// boot and after a hot reload. Tables themselves are created lazily by the
/// document engine the first time a DocType is touched.
///
/// The append-only guard is installed here rather than in the router because
/// every write reaches the engine through the same global hook registry, and
/// this function is the one boot step both the server and the tests run
/// (SEC-07/08/09/AUTH-17).
pub fn register_doctypes() {
    for meta in doctype_metas() {
        ferris_schema::register_doctype(meta);
    }
    crate::observability::install_append_only_guard();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::User;
    use ferris_document::engine::{DocumentEngine, EngineContext};
    use ferris_schema::get_doctype;

    async fn memory_pool() -> ferris_db::Db {
        ferris_db::create_pool(&ferris_core::DatabaseConfig {
            r#type: ferris_core::DatabaseType::SQLite,
            host: "localhost".into(),
            port: 0,
            name: ":memory:".into(),
            user: String::new(),
            password: String::new(),
            max_connections: 1,
            min_connections: 1,
            connect_timeout_secs: 5,
            idle_timeout_secs: 300,
            max_lifetime_secs: 3600,
            ssl_mode: ferris_core::SslMode::Disable,
            schema: None,
        })
        .await
        .expect("memory pool")
    }

    #[test]
    fn every_doctype_registers_with_fields() {
        register_doctypes();
        for meta in doctype_metas() {
            let got = get_doctype(&meta.name).expect("registered");
            assert_eq!(got.module, MODULE, "{}", meta.name);
            assert!(!got.fields.is_empty(), "{} needs fields", meta.name);
        }
    }

    #[test]
    fn only_system_settings_is_a_single() {
        let singles: Vec<_> = doctype_metas()
            .into_iter()
            .filter(|m| m.is_single)
            .map(|m| m.name)
            .collect();
        assert_eq!(singles, vec!["System Settings"]);
    }

    #[test]
    fn activity_and_access_logs_are_not_duplicated() {
        let names: Vec<_> = doctype_metas().into_iter().map(|m| m.name).collect();
        assert!(!names.iter().any(|n| n == "Activity Log"));
        assert!(!names.iter().any(|n| n == "Access Log"));
    }

    #[test]
    fn register_is_idempotent() {
        register_doctypes();
        register_doctypes();
        assert!(get_doctype("System Settings").is_some());
        assert!(get_doctype("Error Log").is_some());
    }

    #[test]
    fn audit_doctypes_grant_read_only() {
        let metas = [
            path_log_meta(),
            error_log_meta(),
            api_request_log_meta(),
            view_log_meta(),
        ];
        for meta in metas {
            assert!(!meta.allow_delete, "{} must not offer delete", meta.name);
            assert!(meta.read_only, "{} must be declared read-only", meta.name);
            assert!(
                meta.permissions
                    .iter()
                    .any(|rule| rule.role == "System Manager" && rule.read),
                "{} must stay readable by System Manager",
                meta.name
            );
            assert!(
                meta.permissions
                    .iter()
                    .all(|rule| !rule.write && !rule.create && !rule.delete),
                "{} must grant no write/create/delete",
                meta.name
            );
        }
    }

    /// SEC-07/08/09: `System Manager` is exactly the role the core permission
    /// engine short-circuits, so the policy has to be enforced by the engine
    /// hook. Both REST mutations go through these two calls.
    #[tokio::test]
    async fn audit_logs_reject_update_and_delete_even_for_system_manager() {
        register_doctypes();
        let pool = memory_pool().await;
        let engine = DocumentEngine::new(pool.clone());
        let anon = EngineContext::new("Path Log", "").allow_anonymous(true);
        let created = engine
            .create_owned_by(
                &anon,
                "Administrator",
                ferris_core::Value::from_json(serde_json::json!({
                    "method": "GET",
                    "path": "/api/resource/ToDo/abc",
                    "status_code": 200,
                    "duration_ms": 3,
                    "user": "Administrator",
                    "ip_address": "127.0.0.1",
                })),
            )
            .await
            .expect("audit row is writable by the internal producer");
        let name = created
            .get("name")
            .and_then(ferris_core::Value::as_str)
            .expect("name")
            .to_string();

        let mut admin = User::system_user("admin@example.com".into(), "Admin".into());
        admin.roles = vec!["System Manager".to_string()];
        let ctx = EngineContext::new("Path Log", "").with_user(&admin);

        let deleted = engine.delete(&ctx, &name).await;
        assert!(
            matches!(&deleted, Err(ferris_core::Error::Authz(_))),
            "delete must be refused, got {deleted:?}"
        );

        let updated = engine
            .update(
                &ctx,
                &name,
                ferris_core::Value::from_json(serde_json::json!({ "path": "/tampered" })),
            )
            .await;
        assert!(
            matches!(&updated, Err(ferris_core::Error::Authz(_))),
            "update must be refused, got {updated:?}"
        );
    }
}
