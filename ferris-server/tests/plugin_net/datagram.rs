//! UDP and authoritative DNS: the datagrams that cross the guest boundary.
//!
//! `probe-ok` echoes `handle-datagram`, and the zone test drives a real UDP
//! query against the host's listener. No external network: everything is
//! loopback.

use std::time::Duration;

use ferris_wasm::{
    DnsRecord, DnsRecordType, DnsSoa, DnsZone, RawSpec, RawTransport, StreamService,
    UdpListenerSpec,
};
use tokio::net::UdpSocket;
use tokio::time::timeout;

use super::support;

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;

#[tokio::test]
async fn a_udp_datagram_reaches_the_guest_and_comes_back() {
    let Some(streams) = support::transport().await else {
        return;
    };
    let id = streams
        .udp_listen(
            "probe",
            &UdpListenerSpec {
                address: Some("127.0.0.1".to_string()),
                port: Some(0),
                broadcast: false,
                max_datagram: None,
            },
        )
        .await
        .expect("the UDP listener binds");
    let info = streams.describe(&id).expect("the listener is registered");
    assert_eq!(info.protocol.as_deref(), Some("udp"));
    let port = info.port.expect("an inferred port is reported");

    let client = UdpSocket::bind("127.0.0.1:0").await.expect("client socket");
    client
        .send_to(b"ping", ("127.0.0.1", port))
        .await
        .expect("datagram sent");
    let mut buffer = [0_u8; 64];
    let (size, _) = timeout(Duration::from_secs(5), client.recv_from(&mut buffer))
        .await
        .expect("the guest answers before the deadline")
        .expect("the reply arrives");
    assert_eq!(&buffer[..size], b"ping");

    streams.udp_close("probe", &id).await.expect("closed");
    assert!(streams.describe(&id).is_none());
}

#[tokio::test]
async fn an_authoritative_zone_answers_over_udp() {
    let Some(streams) = support::transport().await else {
        return;
    };
    let id = streams
        .dns_serve(
            "probe",
            &DnsZone {
                origin: "example.test".to_string(),
                address: Some("127.0.0.1".to_string()),
                port: Some(0),
                records: vec![DnsRecord {
                    name: "@".to_string(),
                    record_type: DnsRecordType::A,
                    ttl: 60,
                    data: "192.0.2.10".to_string(),
                    priority: None,
                }],
                soa: Some(DnsSoa {
                    primary_ns: "ns1.example.test.".to_string(),
                    admin: "hostmaster.example.test.".to_string(),
                    serial: 0,
                    refresh: 0,
                    retry: 0,
                    expire: 0,
                    minimum_ttl: 0,
                }),
            },
        )
        .await
        .expect("the zone binds");
    let info = streams.describe(&id).expect("the listener is registered");
    assert_eq!(info.protocol.as_deref(), Some("dns"));
    let port = info.port.expect("the effective port is reported");

    let client = UdpSocket::bind("127.0.0.1:0").await.expect("client socket");
    client
        .send_to(&query("example.test.", TYPE_A), ("127.0.0.1", port))
        .await
        .expect("query sent");
    let mut buffer = [0_u8; 512];
    let (size, _) = timeout(Duration::from_secs(5), client.recv_from(&mut buffer))
        .await
        .expect("the host answers before the deadline")
        .expect("the answer arrives");
    let response = &buffer[..size];
    assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);
    assert_eq!(&response[size - 4..], &[192, 0, 2, 10]);

    // A replacement zone is visible to the next query, atomically.
    let mut updated = ferris_wasm::DnsZone {
        origin: "example.test".to_string(),
        address: None,
        port: None,
        records: vec![DnsRecord {
            name: "@".to_string(),
            record_type: DnsRecordType::A,
            ttl: 60,
            data: "192.0.2.11".to_string(),
            priority: None,
        }],
        soa: None,
    };
    updated.origin = "example.test".to_string();
    streams
        .dns_update("probe", &id, &updated)
        .await
        .expect("the zone is replaced");
    client
        .send_to(&query("example.test.", TYPE_A), ("127.0.0.1", port))
        .await
        .expect("query sent");
    let (size, _) = timeout(Duration::from_secs(5), client.recv_from(&mut buffer))
        .await
        .expect("the host answers before the deadline")
        .expect("the answer arrives");
    assert_eq!(&buffer[size - 4..size], &[192, 0, 2, 11]);

    streams.dns_stop("probe", &id).await.expect("stopped");
    assert!(streams.describe(&id).is_none());
}

#[tokio::test]
async fn a_raw_socket_is_opened_or_refused_with_the_privilege_named() {
    let Some(streams) = support::transport().await else {
        return;
    };
    match streams
        .raw_open(
            "probe",
            &RawSpec {
                transport: RawTransport::Icmp,
                address: Some("127.0.0.1".to_string()),
                port: None,
                ip_protocol: None,
            },
        )
        .await
    {
        Ok(id) => {
            let info = streams.describe(&id).expect("registered");
            assert_eq!(info.protocol.as_deref(), Some("icmp"));
            streams.raw_close("probe", &id).await.expect("closed");
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("CAP_NET_RAW") || text.contains("permitted"),
                "a raw socket must fail with the missing privilege named: {text}"
            );
        }
    }
}

fn query(name: &str, rtype: u16) -> Vec<u8> {
    let mut out = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    for label in name.trim_end_matches('.').split('.') {
        out.push(u8::try_from(label.len()).expect("label"));
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&rtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out
}
