//! Tests for the capability model: primordial, specific, the declared set and
//! the operator approval loaded from `grants.toml`.

use super::*;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_app(tag: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "ferris-approved-{tag}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp app dir");
    dir
}

#[test]
fn primordial_capabilities_need_no_declaration() {
    let grants = GrantSet::primordial();
    for capability in [
        Capability::Filesystem,
        Capability::Routes,
        Capability::Http,
        Capability::DocstoreRead,
        Capability::DocstoreWrite,
        Capability::Config,
        Capability::Schema,
        Capability::Scheduler,
        Capability::Notifications,
        Capability::Pubsub,
    ] {
        assert!(grants.contains(capability), "{capability}");
    }
    for capability in Capability::ALL.into_iter().filter(|c| c.specific()) {
        assert!(
            !grants.contains(capability),
            "{capability} must be declared"
        );
    }
}

#[test]
fn a_declared_specific_capability_is_granted() {
    let grants = GrantSet::for_declaration([Capability::TlsClient]);
    assert!(grants.contains(Capability::TlsClient));
    assert!(!grants.contains(Capability::Streams));
    assert!(grants.contains(Capability::Filesystem));
}

#[test]
fn capability_names_round_trip_and_reject_typos() {
    for capability in Capability::ALL {
        assert_eq!(Capability::from_name(capability.name()), Some(capability));
        assert_eq!(Capability::from_wit(capability.to_wit()), capability);
    }
    assert_eq!(Capability::from_name("htp"), None);
    assert_eq!(Capability::from_name("Blobs"), None);
}

#[test]
fn the_wire_projection_is_the_effective_set() {
    let grants = GrantSet::for_declaration([Capability::Overrides]);
    let wire = grants.to_wit().capabilities;
    assert!(wire.contains(&wit::Capability::Overrides));
    assert!(wire.contains(&wit::Capability::Filesystem));
    assert!(!wire.contains(&wit::Capability::Pki));
}

/// `ferris app install` writes the approval at the app root and as
/// `world/grants.toml`; the root wins when both exist.
#[test]
fn the_approval_prefers_the_app_root_over_the_world_fallback() {
    let app = temp_app("prefer");
    std::fs::create_dir_all(app.join("world")).expect("world");

    trust::GrantSet::new(
        vec!["process".to_string()],
        vec!["api.root.test".to_string()],
        Vec::new(),
    )
    .save(&app.join(trust::GRANTS_FILE))
    .expect("root grants");
    trust::GrantSet::new(
        vec!["tls-client".to_string()],
        vec!["api.world.test".to_string()],
        Vec::new(),
    )
    .save(&app.join("world").join(trust::GRANTS_FILE))
    .expect("world grants");

    let approved = ApprovedGrants::load(&app).expect("loaded");
    assert!(approved.allows_capability(Capability::Process));
    assert!(!approved.allows_capability(Capability::TlsClient));
    assert!(approved.allows_http_host("api.root.test"));
    assert!(!approved.allows_http_host("api.world.test"));
    assert_eq!(
        approved.loaded_from(),
        Some(app.join(trust::GRANTS_FILE).as_path())
    );

    let _ = std::fs::remove_dir_all(&app);
}

/// A world that only carries the `world/` copy (the layout
/// `ferris site install-app` copies from) is still read.
#[test]
fn the_approval_falls_back_to_the_world_copy() {
    let app = temp_app("fallback");
    std::fs::create_dir_all(app.join("world")).expect("world");
    trust::GrantSet::new(
        vec!["tls-client".to_string()],
        vec!["api.world.test".to_string()],
        Vec::new(),
    )
    .save(&app.join("world").join(trust::GRANTS_FILE))
    .expect("world grants");

    let approved = ApprovedGrants::load(&app).expect("loaded");
    assert!(approved.allows_capability(Capability::TlsClient));
    assert!(approved.allows_http_host("api.world.test"));
    assert_eq!(
        approved.loaded_from(),
        Some(app.join("world").join(trust::GRANTS_FILE).as_path())
    );

    let _ = std::fs::remove_dir_all(&app);
}

/// No file is an empty approval, never an implicit grant; the candidate list
/// is what the fail-closed message shows.
#[test]
fn a_missing_approval_grants_nothing() {
    let app = temp_app("missing");
    let approved = ApprovedGrants::load(&app).expect("missing is not a read error");
    assert!(approved.grant_set().is_none());
    assert!(approved.loaded_from().is_none());
    assert!(!approved.allows_capability(Capability::Process));
    assert!(!approved.allows_http_host("api.test"));
    let label = approved.candidates_label();
    assert!(label.contains("grants.toml"), "{label}");
    assert!(label.contains("world"), "{label}");

    let _ = std::fs::remove_dir_all(&app);
}

/// A present-but-empty file is an approval that names nothing: it is not the
/// same as a missing file (the denial can point at the file), but it grants
/// nothing either.
#[test]
fn an_empty_approval_is_not_a_missing_one() {
    let app = temp_app("empty");
    trust::GrantSet::default()
        .save(&app.join(trust::GRANTS_FILE))
        .expect("empty grants");
    let approved = ApprovedGrants::load(&app).expect("loaded");
    assert!(approved.grant_set().is_some());
    assert!(approved.loaded_from().is_some());
    assert!(!approved.allows_capability(Capability::Process));

    let _ = std::fs::remove_dir_all(&app);
}
