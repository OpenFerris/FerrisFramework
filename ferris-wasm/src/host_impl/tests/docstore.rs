//! `docstore`/`session` behaviour: upsert, count, aggregate and authorization.

use super::super::RealHostApi;
use super::super::http::HttpPolicy;
use super::helpers::{admin, memory_pool, plain_user, test_host};
use crate::host::{DocAggregateRow, DocAggregateSpec, DocFilter, ListDocsQuery};
use ferris_document::engine::{DocumentEngine, EngineContext, ListQuery};
use parking_lot::Mutex;
use std::collections::HashMap;

/// §C2 regression: an in-app notification sent by a plugin must land in the
/// same `Notification` doctype the Desk feed reads. The engine's `Create`
/// check refuses `User::guest()`, so the host has to persist as the system
/// service principal — the twin of the API layer's
/// `panel::push_notification`. Pre-fix the WIT bridge reported success
/// while the engine dropped every notification at the permission gate.
#[tokio::test]
async fn notify_in_app_persists_as_a_notification_doc() {
    let pool = memory_pool().await;
    let host = RealHostApi {
        engine: DocumentEngine::new(pool.clone()),
        pool: pool.clone(),
        job_queue: None,
        scheduler: Mutex::new(None),
        streams: Mutex::new(HashMap::new()),
        stream_buffers: Mutex::new(HashMap::new()),
        current_user: Mutex::new(None),
        certificate_service: Mutex::new(false),
        http: HttpPolicy::from_env(),
    };

    host.notify_in_app_async("admin@local", "Nueva alerta", "mensaje de verificacion")
        .await
        .expect("the in-app notification must persist, not be dropped");

    // The Desk feed reads it back as a `Notification` doc for the recipient.
    let engine = DocumentEngine::new(pool);
    let mut ctx = EngineContext::new("Notification", "");
    ctx.user = Some(RealHostApi::notification_service_user());
    let mut query = ListQuery::default();
    query.limit = Some(20);
    let (docs, total) = engine.list(&ctx, &query).await.expect("feed list");
    assert_eq!(
        total, 1,
        "/notify must persist exactly one Notification doc"
    );
    assert_eq!(docs.len(), 1);
    assert_eq!(
        docs[0].get("title").and_then(|v| v.as_str()),
        Some("Nueva alerta")
    );
    assert_eq!(
        docs[0].get("user").and_then(|v| v.as_str()),
        Some("admin@local")
    );
}

/// R-4: `upsert` creates on the first call and updates the same row on a
/// retry, so an ingest replay never forks a second document.
#[tokio::test]
async fn upsert_creates_then_updates_by_name() {
    let host = test_host(memory_pool().await);

    let first = host
        .docstore_upsert("Probe", r#"{"name":"P-1","title":"uno"}"#)
        .await
        .expect("first upsert creates");
    assert_eq!(first.name, "P-1");

    let second = host
        .docstore_upsert("Probe", r#"{"name":"P-1","title":"dos"}"#)
        .await
        .expect("second upsert updates");
    assert_eq!(second.name, "P-1");
    assert!(second.data.contains("dos"), "updated body: {}", second.data);

    // Still one row: the retry updated in place.
    let total = host
        .docstore_count("Probe", &ListDocsQuery::default())
        .await
        .expect("count after upsert");
    assert_eq!(total, 1, "upsert must not fork a second row");
}

/// R-5: `count` agrees with `list` for the host-side (`ne`/`between`)
/// predicates the engine cannot push down, instead of returning the
/// pre-predicate total.
#[tokio::test]
async fn count_applies_host_side_predicates() {
    let host = test_host(memory_pool().await);
    for (name, title) in [("A", "keep"), ("B", "drop"), ("C", "keep")] {
        host.docstore_create(
            "Probe",
            &format!(r#"{{"name":"{name}","title":"{title}"}}"#),
        )
        .await
        .expect("seed row");
    }

    let query = ListDocsQuery {
        filters: vec![DocFilter {
            field: "title".into(),
            op: "ne".into(),
            value: "drop".into(),
        }],
        ..Default::default()
    };
    let total = host.docstore_count("Probe", &query).await.expect("count");
    assert_eq!(total, 2, "`ne` must be applied before counting");

    // The page the same query would return has the same length as the count.
    let rows = host.docstore_list("Probe", &query).await.expect("list");
    assert_eq!(rows.len() as u64, total);
}

/// #20: `aggregate` groups and computes count/sum/avg/max over the matching
/// set; non-numeric values are skipped by the numeric aggregates, and an
/// empty `group-by` yields one grand-total row.
#[tokio::test]
async fn aggregate_groups_and_computes_over_the_match() {
    let host = test_host(memory_pool().await);
    for (name, region, amount) in [
        ("A", "north", "10"),
        ("B", "north", "20"),
        ("C", "south", "5"),
        ("D", "south", "x"), // non-numeric: skipped by sum/avg/max
    ] {
        host.docstore_create(
            "Probe",
            &format!(r#"{{"name":"{name}","region":"{region}","amount":"{amount}"}}"#),
        )
        .await
        .expect("seed row");
    }

    let specs = vec![
        DocAggregateSpec {
            func: "count".into(),
            field: String::new(),
            alias: None,
        },
        DocAggregateSpec {
            func: "sum".into(),
            field: "amount".into(),
            alias: Some("total".into()),
        },
        DocAggregateSpec {
            func: "avg".into(),
            field: "amount".into(),
            alias: None,
        },
        DocAggregateSpec {
            func: "max".into(),
            field: "amount".into(),
            alias: None,
        },
    ];
    let rows = host
        .docstore_aggregate(
            "Probe",
            &ListDocsQuery::default(),
            &["region".into()],
            &specs,
        )
        .await
        .expect("aggregate");

    // One row per region. The *order* is first-seen over the candidate scan,
    // and four rows created inside the same second share `creation`, so their
    // SQL order is not something a test may pin: look each group up instead of
    // indexing the result (the old assertion flaked exactly that way).
    assert_eq!(rows.len(), 2, "one row per region: {rows:?}");
    let by_region = |region: &str| -> &DocAggregateRow {
        rows.iter()
            .find(|row| row.group == vec![("region".to_string(), region.to_string())])
            .unwrap_or_else(|| panic!("no row for region {region}: {rows:?}"))
    };
    assert_eq!(
        by_region("north").values,
        vec![
            ("count".to_string(), "2".to_string()),
            ("total".to_string(), "30".to_string()),
            ("avg_amount".to_string(), "15".to_string()),
            ("max_amount".to_string(), "20".to_string()),
        ]
    );
    assert_eq!(
        by_region("south").values,
        vec![
            ("count".to_string(), "2".to_string()),
            ("total".to_string(), "5".to_string()),
            ("avg_amount".to_string(), "5".to_string()),
            ("max_amount".to_string(), "5".to_string()),
        ],
        "the non-numeric `x` is skipped, leaving one value of 5"
    );

    // Grand total: no `group-by` yields a single row with an empty group.
    let grand = host
        .docstore_aggregate(
            "Probe",
            &ListDocsQuery::default(),
            &[],
            &[DocAggregateSpec {
                func: "count".into(),
                field: String::new(),
                alias: None,
            }],
        )
        .await
        .expect("grand total");
    assert_eq!(grand.len(), 1);
    assert!(grand[0].group.is_empty());
    assert_eq!(
        grand[0].values,
        vec![("count".to_string(), "4".to_string())]
    );
}

/// A verified client certificate is never a global service authorization.
///
/// The boundary resolves it to a role-less `cert:<fingerprint>` principal, so
/// the engine still evaluates the ordinary `DocPerm` policy. A certificate
/// that reaches `ctx` without a bound principal is default-denied; granting a
/// device access is an explicit operator mapping, not an anonymous bypass.
#[tokio::test]
async fn certificate_service_mode_never_opts_into_anonymous_authorization() {
    let host = test_host(memory_pool().await);

    // Default: no user, no service mode → the engine stays default-deny.
    host.set_current_user(None);
    assert!(
        !host.ctx("Equipo").allow_anonymous,
        "without certificate service mode an anonymous context stays default-deny"
    );

    // Certificate marker without a bound principal: still default-deny.
    host.set_certificate_service(true);
    let cert_only = host.ctx("Equipo");
    assert!(
        !cert_only.allow_anonymous,
        "a certificate alone must not enable the anonymous service bypass"
    );
    assert!(
        cert_only.user.is_none(),
        "the boundary must bind a principal; ctx must not fabricate one"
    );

    // A real user is never downgraded to anonymous service mode.
    host.set_current_user(Some(admin()));
    let with_user = host.ctx("Equipo");
    assert!(
        with_user.user.is_some(),
        "a bound user is preserved under certificate service mode"
    );
    assert!(
        !with_user.allow_anonymous,
        "a bound caller keeps normal role checks, never the anonymous bypass"
    );

    // Leaving the mode restores default-deny for the next operation.
    host.set_current_user(None);
    host.set_certificate_service(false);
    assert!(
        !host.ctx("Equipo").allow_anonymous,
        "service mode does not leak across operations"
    );
}

/// H7/M: a row-level `session::can` check answers `false` for a row that
/// does not exist, exactly like a row the caller may not see. The guest
/// never learns whether an arbitrary id exists.
#[tokio::test]
async fn session_can_does_not_reveal_whether_a_row_exists() {
    let host = test_host(memory_pool().await);
    let created = host
        .docstore_create("Probe", r#"{"title":"present"}"#)
        .await
        .expect("create as admin");
    host.set_current_user(Some(plain_user("nobody@x")));

    let missing = host
        .session_can("Probe", "read", Some("DOES-NOT-EXIST"))
        .await
        .expect("a missing row is a verdict, not an error");
    let existing = host
        .session_can("Probe", "read", Some(&created.name))
        .await
        .expect("an unreadable row is a verdict");
    assert!(!missing, "a missing row must not be readable");
    assert!(
        !existing,
        "the plain user must be denied on the admin-owned row"
    );
    assert_eq!(
        missing, existing,
        "the two cases must be indistinguishable to the guest"
    );
}

/// A Permission Type (`approve`, `download`, …) is resolved through the same
/// effective `Custom DocPerm` rules as the Desk. An unknown-to-the-built-ins
/// action is a verdict, never the "unknown permission" error the old parser
/// returned.
#[tokio::test]
async fn a_custom_permission_name_is_a_verdict_not_an_error() {
    let host = test_host(memory_pool().await);
    host.set_current_user(Some(plain_user("nobody@x")));

    let verdict = host
        .session_can("Probe", "approve", None)
        .await
        .expect("a custom permission type is a valid question");
    assert!(!verdict, "no Custom DocPerm rule grants it");
}
