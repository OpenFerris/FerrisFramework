//! DNS regressions: answers, CNAME chasing, refusals and zone limits.

use std::collections::HashSet;

use ferris_wasm::{DnsRecord, DnsRecordType, DnsZone};

use super::service::dns_host;
use super::zone::{encode_name, qualify, txt_chunks};
use super::*;

/// The set of owners in a zone (diagnostics).
fn owners(zone: &Zone) -> HashSet<String> {
    zone.records
        .iter()
        .map(|record| record.owner.clone())
        .collect()
}

fn zone() -> Zone {
    Zone::parse(&DnsZone {
        origin: "example.test".to_string(),
        address: None,
        port: None,
        records: vec![
            DnsRecord {
                name: "@".to_string(),
                record_type: DnsRecordType::A,
                ttl: 60,
                data: "192.0.2.10".to_string(),
                priority: None,
            },
            DnsRecord {
                name: "www".to_string(),
                record_type: DnsRecordType::Cname,
                ttl: 60,
                data: "example.test.".to_string(),
                priority: None,
            },
            DnsRecord {
                name: "@".to_string(),
                record_type: DnsRecordType::Mx,
                ttl: 60,
                data: "mail.example.test.".to_string(),
                priority: Some(5),
            },
        ],
        soa: Some(DnsSoa {
            primary_ns: "ns1.example.test.".to_string(),
            admin: "hostmaster.example.test.".to_string(),
            serial: 0,
            refresh: 0,
            retry: 0,
            expire: 0,
            minimum_ttl: 0,
        }),
    })
    .expect("zone")
}

fn query(name: &str, rtype: u16) -> Vec<u8> {
    let mut out = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    encode_name(&mut out, name);
    out.extend_from_slice(&rtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out
}

#[test]
fn an_a_record_is_answered() {
    let response = zone()
        .answer(&query("example.test.", TYPE_A), true)
        .expect("response");
    assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);
    let rdata = &response[response.len() - 4..];
    assert_eq!(rdata, [192, 0, 2, 10]);
}

#[test]
fn a_cname_chases_inside_the_zone() {
    let response = zone()
        .answer(&query("www.example.test.", TYPE_A), true)
        .expect("response");
    // CNAME + the apex A it points at.
    assert_eq!(u16::from_be_bytes([response[6], response[7]]), 2);
}

#[test]
fn an_unknown_name_is_nxdomain() {
    let response = zone()
        .answer(&query("nope.example.test.", TYPE_A), true)
        .expect("response");
    assert_eq!(response[3] & 0x0F, 3);
}

#[test]
fn an_outside_name_is_refused() {
    let response = zone()
        .answer(&query("elsewhere.test.", TYPE_A), true)
        .expect("response");
    assert_eq!(response[3] & 0x0F, 5);
}

#[test]
fn a_malformed_packet_is_not_a_query() {
    assert!(zone().answer(&[0, 1, 2], true).is_none());
}

#[test]
fn relative_names_belong_to_the_origin() {
    assert_eq!(qualify("example.test.", "mail"), "mail.example.test.");
    assert_eq!(qualify("example.test.", "mail.other."), "mail.other.");
}

#[test]
fn txt_is_split_at_the_255_byte_limit() {
    let long = "a".repeat(300);
    assert_eq!(txt_chunks(&long).len(), 2);
}

#[test]
fn owners_are_normalised() {
    assert!(owners(&zone()).contains("example.test."));
}

/// WASM-013: an undeclared bind address is loopback, not every interface.
#[test]
fn the_inferred_zone_bind_is_loopback() {
    assert_eq!(dns_host(None), "127.0.0.1");
    assert_eq!(dns_host(Some("   ")), "127.0.0.1");
    assert_eq!(dns_host(Some("192.0.2.53")), "192.0.2.53");
}

/// WASM-014: a zone past the record budget is refused before it is served,
/// and the TCP side has a finite connection cap.
#[test]
fn an_oversized_zone_is_refused() {
    let records: Vec<DnsRecord> = (0..=MAX_DNS_RECORDS)
        .map(|index| DnsRecord {
            name: format!("host{index}"),
            record_type: DnsRecordType::A,
            ttl: 60,
            data: "192.0.2.10".to_string(),
            priority: None,
        })
        .collect();
    let oversized = DnsZone {
        origin: "example.test".to_string(),
        address: None,
        port: None,
        records,
        soa: None,
    };
    let err = Zone::parse(&oversized)
        .err()
        .expect("a zone over the record budget is refused");
    assert!(err.to_string().contains("record budget"), "{err}");
}
