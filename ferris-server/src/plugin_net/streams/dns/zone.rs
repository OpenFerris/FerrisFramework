//! The parsed zone and the DNS wire codec (parse, encode, answer).

use std::hash::{Hash, Hasher};
use std::net::IpAddr;

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{DnsRecordType, DnsZone};

use super::{
    CLASS_IN, DEFAULT_SOA_EXPIRE, DEFAULT_SOA_MINIMUM, DEFAULT_SOA_REFRESH, DEFAULT_SOA_RETRY,
    DEFAULT_TTL, MAX_DNS_RECORDS, MAX_UDP_ANSWER, RCODE_FORMERR, RCODE_NOERROR, RCODE_NXDOMAIN,
    RCODE_REFUSED, Record, TYPE_A, TYPE_AAAA, TYPE_ANY, TYPE_CNAME, TYPE_MX, TYPE_NS, TYPE_PTR,
    TYPE_SOA, TYPE_SRV, TYPE_TXT, Zone,
};

impl Zone {
    /// Validate and normalise a WIT zone.
    pub(super) fn parse(zone: &DnsZone) -> Result<Self> {
        if zone.records.len() > MAX_DNS_RECORDS {
            return Err(BridgeError::unsupported(format!(
                "io::dns-serve: {} records exceed the {MAX_DNS_RECORDS} record budget of one zone",
                zone.records.len()
            )));
        }
        let origin = absolute_name(&zone.origin)
            .with_context(|| format!("io::dns-serve: invalid zone origin `{}`", zone.origin))?;
        let mut records = Vec::with_capacity(zone.records.len());
        for record in &zone.records {
            // A `soa` record and the typed SOA are two ways to say the same
            // thing; the contract refuses to pick one, so the host does too.
            if matches!(record.record_type, DnsRecordType::Soa) && zone.soa.is_some() {
                return Err(BridgeError::unsupported(
                    "io::dns-serve: declare the SOA either as `soa` or as a `soa` record, not both",
                ));
            }
            let owner = if record.name.trim().is_empty() || record.name.trim() == "@" {
                origin.clone()
            } else {
                relative_name(&origin, &record.name)?
            };
            let rtype = rtype_of(record.record_type);
            records.push(Record {
                owner,
                rtype,
                ttl: if record.ttl == 0 {
                    DEFAULT_TTL
                } else {
                    record.ttl
                },
                priority: record.priority,
                data: record.data.clone(),
            });
        }
        Ok(Self {
            origin,
            records,
            soa: zone.soa.clone(),
        })
    }

    pub(super) fn same_origin(&self, other: &Self) -> bool {
        self.origin.eq_ignore_ascii_case(&other.origin)
    }

    /// The SOA this zone answers with, packing a typed declaration or the
    /// textual `soa` record into the same shape.
    fn soa_rdata(&self) -> Vec<u8> {
        let (mname, rname, serial, refresh, retry, expire, minimum) = if let Some(soa) = &self.soa {
            let serial = if soa.serial == 0 {
                self.derived_serial()
            } else {
                soa.serial
            };
            (
                absolute_name(&soa.primary_ns).unwrap_or_else(|_| "ns.invalid.".to_string()),
                absolute_name(&soa.admin).unwrap_or_else(|_| "hostmaster.invalid.".to_string()),
                serial,
                nonzero(soa.refresh, DEFAULT_SOA_REFRESH),
                nonzero(soa.retry, DEFAULT_SOA_RETRY),
                nonzero(soa.expire, DEFAULT_SOA_EXPIRE),
                nonzero(soa.minimum_ttl, DEFAULT_SOA_MINIMUM),
            )
        } else if let Some(record) = self
            .records
            .iter()
            .find(|record| record.rtype == TYPE_SOA && record.owner == self.origin)
        {
            let parts: Vec<&str> = record.data.split_whitespace().collect();
            if parts.len() != 7 {
                return Vec::new();
            }
            (
                absolute_name(parts[0]).unwrap_or_else(|_| self.origin.clone()),
                absolute_name(parts[1]).unwrap_or_else(|_| self.origin.clone()),
                parts[2]
                    .parse::<u32>()
                    .unwrap_or_else(|_| self.derived_serial()),
                parts[3].parse().unwrap_or(DEFAULT_SOA_REFRESH),
                parts[4].parse().unwrap_or(DEFAULT_SOA_RETRY),
                parts[5].parse().unwrap_or(DEFAULT_SOA_EXPIRE),
                parts[6].parse().unwrap_or(DEFAULT_SOA_MINIMUM),
            )
        } else {
            (
                self.default_ns(),
                format!("hostmaster.{}", self.origin),
                self.derived_serial(),
                DEFAULT_SOA_REFRESH,
                DEFAULT_SOA_RETRY,
                DEFAULT_SOA_EXPIRE,
                DEFAULT_SOA_MINIMUM,
            )
        };
        let mut out = Vec::new();
        encode_name(&mut out, &mname);
        encode_name(&mut out, &rname);
        for number in [serial, refresh, retry, expire, minimum] {
            out.extend_from_slice(&number.to_be_bytes());
        }
        out
    }

    fn default_ns(&self) -> String {
        self.records
            .iter()
            .find(|record| record.rtype == TYPE_NS)
            .map_or_else(
                || format!("ns1.{}", self.origin),
                |record| qualify(&self.origin, &record.data),
            )
    }

    fn derived_serial(&self) -> u32 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.origin.hash(&mut hasher);
        for record in &self.records {
            record.owner.hash(&mut hasher);
            record.rtype.hash(&mut hasher);
            record.ttl.hash(&mut hasher);
            record.data.hash(&mut hasher);
        }
        self.soa
            .as_ref()
            .map(|soa| &soa.primary_ns)
            .hash(&mut hasher);
        u32::try_from(hasher.finish() % u64::from(u32::MAX)).unwrap_or(0)
    }

    /// The RDATA bytes of one record.
    fn rdata(&self, record: &Record) -> Vec<u8> {
        let mut out = Vec::new();
        match record.rtype {
            TYPE_A | TYPE_AAAA => {
                if let Ok(ip) = record.data.trim().parse::<IpAddr>() {
                    match ip {
                        IpAddr::V4(v4) => out.extend_from_slice(&v4.octets()),
                        IpAddr::V6(v6) => out.extend_from_slice(&v6.octets()),
                    }
                }
            }
            TYPE_CNAME | TYPE_NS | TYPE_PTR => {
                encode_name(&mut out, &qualify(&self.origin, &record.data));
            }
            TYPE_MX => {
                let target = record
                    .data
                    .split_whitespace()
                    .last()
                    .unwrap_or(&record.data);
                out.extend_from_slice(&record.priority.unwrap_or(10).to_be_bytes());
                encode_name(&mut out, &qualify(&self.origin, target));
            }
            TYPE_TXT => {
                for chunk in txt_chunks(&record.data) {
                    out.push(u8::try_from(chunk.len()).unwrap_or(u8::MAX));
                    out.extend_from_slice(&chunk);
                }
            }
            TYPE_SRV => {
                let parts: Vec<&str> = record.data.split_whitespace().collect();
                if parts.len() == 3 {
                    let weight = parts[0].parse::<u16>().unwrap_or(0);
                    let port = parts[1].parse::<u16>().unwrap_or(0);
                    out.extend_from_slice(&record.priority.unwrap_or(0).to_be_bytes());
                    out.extend_from_slice(&weight.to_be_bytes());
                    out.extend_from_slice(&port.to_be_bytes());
                    encode_name(&mut out, &qualify(&self.origin, parts[2]));
                }
            }
            TYPE_SOA => out.extend_from_slice(&self.soa_rdata()),
            _ => {}
        }
        out
    }

    /// Answer one wire query.
    ///
    /// Returns `None` when the packet is not a DNS query the host answers (the
    /// socket drops it). A malformed query gets FORMERR, an out-of-zone name
    /// gets REFUSED and a missing name gets NXDOMAIN — each of them a real
    /// response, because a resolver that receives nothing just retries.
    pub(super) fn answer(&self, query: &[u8], udp: bool) -> Option<Vec<u8>> {
        if query.len() < 12 {
            return None;
        }
        let id = u16::from_be_bytes([query[0], query[1]]);
        let flags = u16::from_be_bytes([query[2], query[3]]);
        if flags & 0x8000 != 0 {
            // A response is not a query; answering one would loop.
            return None;
        }
        let qdcount = u16::from_be_bytes([query[4], query[5]]);
        if qdcount != 1 {
            return Some(header(id, flags, RCODE_FORMERR, 0, &[]));
        }
        let Ok((qname, offset)) = read_name(query, 12) else {
            return Some(header(id, flags, RCODE_FORMERR, 0, &[]));
        };
        if offset + 4 > query.len() {
            return Some(header(id, flags, RCODE_FORMERR, 0, &[]));
        }
        let qtype = u16::from_be_bytes([query[offset], query[offset + 1]]);
        let qclass = u16::from_be_bytes([query[offset + 2], query[offset + 3]]);
        let question = &query[12..offset + 4];
        let name = qname.to_ascii_lowercase();
        if !self.in_zone(&name) {
            return Some(header(id, flags, RCODE_REFUSED, 0, question));
        }
        if qclass != CLASS_IN {
            return Some(header(id, flags, RCODE_REFUSED, 0, question));
        }

        let mut answers = Vec::new();
        let known = self.records.iter().any(|record| record.owner == name) || name == self.origin;
        if !known {
            return Some(header(id, flags, RCODE_NXDOMAIN, 0, question));
        }

        if (qtype == TYPE_SOA || qtype == TYPE_ANY) && name == self.origin {
            answers.push(self.soa_owner_answer());
        }
        for record in &self.records {
            if record.owner != name {
                continue;
            }
            if record.rtype == TYPE_SOA {
                // The SOA is answered through the typed path so there is one
                // encoding, not two.
                continue;
            }
            if qtype == TYPE_ANY || qtype == record.rtype {
                push_record(&mut answers, record, self);
            }
        }
        // A CNAME is authoritative for every other type: return it and chase
        // the target inside the zone. A target outside the zone stops there,
        // which is exactly what a resolver expects.
        if qtype != TYPE_CNAME
            && let Some(cname) = self
                .records
                .iter()
                .find(|record| record.owner == name && record.rtype == TYPE_CNAME)
        {
            push_record(&mut answers, cname, self);
            let target = qualify(&self.origin, &cname.data);
            for record in &self.records {
                if record.owner == target && (record.rtype == qtype || qtype == TYPE_ANY) {
                    push_record(&mut answers, record, self);
                }
            }
        }

        let answer_count = u16::try_from(answers.len()).unwrap_or(u16::MAX);
        let mut body = Vec::new();
        for answer in &answers {
            body.extend_from_slice(answer);
        }
        let mut response = header(id, flags, RCODE_NOERROR, answer_count, question);
        response.extend_from_slice(&body);
        if udp && response.len() > MAX_UDP_ANSWER {
            // Too large for UDP: the header carries TC and the client retries
            // over TCP, where the answer is sent whole.
            return Some(header_with_tc(id, flags, RCODE_NOERROR, 0, question));
        }
        Some(response)
    }

    fn soa_owner_answer(&self) -> Vec<u8> {
        let mut out = Vec::new();
        encode_name(&mut out, &self.origin);
        out.extend_from_slice(&TYPE_SOA.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&DEFAULT_TTL.to_be_bytes());
        let rdata = self.soa_rdata();
        out.extend_from_slice(&u16::try_from(rdata.len()).unwrap_or(u16::MAX).to_be_bytes());
        out.extend_from_slice(&rdata);
        out
    }

    fn in_zone(&self, name: &str) -> bool {
        let name = name.trim_end_matches('.');
        let origin = self.origin.trim_end_matches('.');
        name == origin || name.ends_with(&format!(".{origin}"))
    }
}

const fn nonzero(value: u32, default: u32) -> u32 {
    if value == 0 { default } else { value }
}

fn push_record(out: &mut Vec<Vec<u8>>, record: &Record, zone: &Zone) {
    let mut entry = Vec::new();
    encode_name(&mut entry, &record.owner);
    entry.extend_from_slice(&record.rtype.to_be_bytes());
    entry.extend_from_slice(&CLASS_IN.to_be_bytes());
    entry.extend_from_slice(&record.ttl.to_be_bytes());
    let rdata = zone.rdata(record);
    entry.extend_from_slice(&u16::try_from(rdata.len()).unwrap_or(0).to_be_bytes());
    entry.extend_from_slice(&rdata);
    out.push(entry);
}

const fn rtype_of(record_type: DnsRecordType) -> u16 {
    match record_type {
        DnsRecordType::A => TYPE_A,
        DnsRecordType::Aaaa => TYPE_AAAA,
        DnsRecordType::Cname => TYPE_CNAME,
        DnsRecordType::Mx => TYPE_MX,
        DnsRecordType::Txt => TYPE_TXT,
        DnsRecordType::Srv => TYPE_SRV,
        DnsRecordType::Ptr => TYPE_PTR,
        DnsRecordType::Ns => TYPE_NS,
        DnsRecordType::Soa => TYPE_SOA,
    }
}

/// A DNS response header plus the question section.
fn header(id: u16, query_flags: u16, rcode: u16, answers: u16, question: &[u8]) -> Vec<u8> {
    let flags = 0x8000 | 0x0400 | (query_flags & 0x0100) | (rcode & 0x000F);
    let mut out = Vec::with_capacity(12 + question.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&answers.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(question);
    out
}

fn header_with_tc(id: u16, query_flags: u16, rcode: u16, answers: u16, question: &[u8]) -> Vec<u8> {
    let mut out = header(id, query_flags, rcode, answers, question);
    let flags = u16::from_be_bytes([out[2], out[3]]) | 0x0200;
    out[2..4].copy_from_slice(&flags.to_be_bytes());
    out
}

pub(super) fn encode_name(out: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(u8::try_from(len).unwrap_or(63));
        out.extend_from_slice(&bytes[..len]);
    }
    out.push(0);
}

/// Read a (possibly compressed) QNAME from `at`; returns the name and the
/// offset just past it. Compression pointers are followed once (a query that
/// points at itself is rejected, not looped).
fn read_name(message: &[u8], at: usize) -> Result<(String, usize), ()> {
    let mut labels = Vec::new();
    let mut offset = at;
    let mut end = None;
    let mut jumps = 0;
    loop {
        let Some(&len) = message.get(offset) else {
            return Err(());
        };
        if len & 0xC0 == 0xC0 {
            let Some(&low) = message.get(offset + 1) else {
                return Err(());
            };
            let pointer = (usize::from(len & 0x3F) << 8) | usize::from(low);
            if end.is_none() {
                end = Some(offset + 2);
            }
            jumps += 1;
            if jumps > 8 || pointer >= message.len() {
                return Err(());
            }
            offset = pointer;
            continue;
        }
        if len == 0 {
            let finish = end.unwrap_or(offset + 1);
            let mut name = labels.join(".");
            if !name.is_empty() {
                name.push('.');
            }
            return Ok((name, finish));
        }
        let len = usize::from(len);
        if len > 63 {
            return Err(());
        }
        let start = offset + 1;
        let Some(label) = message.get(start..start + len) else {
            return Err(());
        };
        labels.push(String::from_utf8_lossy(label).into_owned());
        offset = start + len;
    }
}

/// A lower-case absolute name with exactly one trailing dot.
fn absolute_name(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("an empty DNS name");
    }
    let name = if raw.ends_with('.') {
        raw.to_string()
    } else {
        format!("{raw}.")
    };
    validate_name(&name)?;
    Ok(name.to_ascii_lowercase())
}

fn validate_name(name: &str) -> Result<()> {
    if name.len() > 254 {
        anyhow::bail!("a DNS name may not exceed 254 bytes");
    }
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            anyhow::bail!("`{name}` has an empty or over-long label");
        }
        if label
            .chars()
            .any(|ch| ch.is_whitespace() || ch == '/' || ch == '\\')
        {
            anyhow::bail!("`{name}` contains a character DNS does not allow in a label");
        }
    }
    Ok(())
}

/// A name relative to the zone origin (`www` -> `www.<origin>.`).
fn relative_name(origin: &str, raw: &str) -> Result<String> {
    let name = if raw.trim().ends_with('.') {
        absolute_name(raw)?
    } else {
        absolute_name(&format!("{raw}.{origin}"))?
    };
    Ok(name)
}

/// Qualify an RDATA target: absolute names stay, relative ones belong to the
/// zone. An unparseable target is returned unchanged so the record still
/// answers (the resolver sees an unusable RDATA, not a crashed server).
pub(super) fn qualify(origin: &str, raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return origin.to_string();
    }
    if raw.ends_with('.') {
        return raw.to_ascii_lowercase();
    }
    format!("{}.{}", raw.to_ascii_lowercase(), origin)
}

/// Split a TXT value into character-strings of at most 255 bytes.
pub(super) fn txt_chunks(raw: &str) -> Vec<Vec<u8>> {
    let stripped = raw.trim();
    let stripped = stripped
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(stripped);
    if stripped.is_empty() {
        return vec![Vec::new()];
    }
    stripped
        .as_bytes()
        .chunks(255)
        .map(<[u8]>::to_vec)
        .collect()
}
