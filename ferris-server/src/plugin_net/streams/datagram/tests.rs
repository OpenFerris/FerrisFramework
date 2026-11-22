//! Datagram regressions: peer parsing, loopback default, budget and egress.

use std::net::IpAddr;

use super::super::raw::bind_authority;
use super::parse::{raw_destination, resolve_peer};
use super::*;

#[test]
fn a_bare_peer_needs_a_port() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    assert!(runtime.block_on(resolve_peer("127.0.0.1")).is_err());
    assert!(runtime.block_on(resolve_peer("127.0.0.1:53")).is_ok());
}

#[test]
fn a_raw_destination_accepts_an_address_or_an_endpoint() {
    assert_eq!(
        raw_destination("192.0.2.1").expect("bare"),
        "192.0.2.1".parse::<IpAddr>().expect("ip")
    );
    assert_eq!(
        raw_destination("192.0.2.1:99").expect("endpoint"),
        "192.0.2.1".parse::<IpAddr>().expect("ip")
    );
    assert!(raw_destination("example.test").is_err());
}

/// WASM-013/014: a datagram socket the app did not address binds loopback,
/// not every interface.
#[test]
fn the_inferred_datagram_bind_is_loopback() {
    assert_eq!(DEFAULT_DATAGRAM_HOST, "127.0.0.1");
    assert_eq!(
        bind_authority(None, None),
        format!("{DEFAULT_DATAGRAM_HOST}:0")
    );
}

/// The datagram budget counts what the process already holds and refuses the
/// socket past it; UDP, raw and DNS share the same table.
#[tokio::test]
async fn the_datagram_budget_refuses_the_socket_past_the_cap() {
    let table = DatagramTable::default();
    let socket = Arc::new(
        UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("a loopback socket binds"),
    );
    let info = datagram_info("udp", "127.0.0.1:1".to_string(), None, 1);
    for index in 0..2 {
        table.admit("io::udp-listen", 2).expect("under the cap");
        table.insert(
            format!("udp-{index}"),
            DatagramEntry {
                app: "app".to_string(),
                kind: DatagramKind::Udp {
                    socket: Arc::clone(&socket),
                    connected: false,
                    max: 1,
                },
                tasks: Vec::new(),
                info: info.clone(),
            },
        );
    }
    let err = table
        .admit("io::udp-listen", 2)
        .expect_err("the third socket is over the cap");
    assert!(err.to_string().contains("datagram sockets"), "{err}");
}

/// WASM-005: a private datagram/raw target is refused with the same policy
/// vocabulary the byte streams use, and the grant is what lifts it.
#[test]
fn a_private_datagram_target_needs_the_grant() {
    let denied = crate::plugin_net::egress::EgressPolicy::from_env(false, false);
    assert!(!denied.allows_address("127.0.0.1".parse().expect("ip")));
    assert!(!denied.allows_address("192.168.0.10".parse().expect("ip")));
    let granted = crate::plugin_net::egress::EgressPolicy::from_env(true, true);
    assert!(granted.allows_address("127.0.0.1".parse().expect("ip")));
}
