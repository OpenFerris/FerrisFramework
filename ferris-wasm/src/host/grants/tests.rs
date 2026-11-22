//! Tests for the simplified capability model: primordial, specific and the
//! declared set.

use super::*;

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
