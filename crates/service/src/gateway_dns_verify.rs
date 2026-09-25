//! Read-only recursive and authoritative DNS checks for the public gateway.

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::{CAA, NS};
use hickory_proto::rr::{DNSClass, Name, RData, RecordType};
use open_compute_core::{ErrorCode, PlatformError, PublicGatewayConfig};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

const MAX_DNS_BYTES: usize = 4096;
const DNS_TIMEOUT: Duration = Duration::from_secs(3);
const ACME_ISSUER: &str = "letsencrypt.org.";
const PUBLIC_RESOLVERS: [SocketAddr; 2] = [
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53),
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),
];

/// Verify public recursive records, the parent authority's delegation, and challenge reachability.
pub(crate) async fn verify_public_gateway_dns(
    gateway: &PublicGatewayConfig,
    resolvers: &[SocketAddr],
) -> Result<(), PlatformError> {
    if resolvers.iter().any(|resolver| !public_resolver(*resolver)) {
        return Err(dns_failed());
    }
    let resolvers = if resolvers.is_empty() {
        &PUBLIC_RESOLVERS[..]
    } else {
        resolvers
    };
    verify_with(gateway, resolvers, 53, 53, true).await
}

fn public_resolver(resolver: SocketAddr) -> bool {
    if resolver.port() == 0 {
        return false;
    }
    match resolver.ip() {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            // Conservatively reject IANA special-use blocks, including globally reachable
            // exceptions, when the operator selects a public recursive resolver.
            (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
        }
    }
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_broadcast()
        && !ip.is_multicast()
        && !ip.is_documentation()
        && octets[0] != 0
        && !(octets[0] == 100 && (64..=127).contains(&octets[1]))
        && !(octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        && !(octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
        && !(octets[0] == 198 && (18..=19).contains(&octets[1]))
        && octets[0] < 240
}

async fn verify_with(
    gateway: &PublicGatewayConfig,
    resolvers: &[SocketAddr],
    authority_port: u16,
    challenge_port: u16,
    public_addresses_only: bool,
) -> Result<(), PlatformError> {
    gateway.validate()?;
    if resolvers.is_empty() {
        return Err(dns_failed());
    }
    let base = &gateway.base_domain;
    let ingress = dns_name(&format!("ingress.{base}"))?;
    let nameserver = dns_name(&format!("ns1.{base}"))?;
    let challenge = dns_name(&format!("_acme-challenge.{base}"))?;
    let expected: BTreeSet<IpAddr> = gateway
        .shared
        .ingress_ipv4
        .iter()
        .copied()
        .map(IpAddr::V4)
        .chain(gateway.shared.ingress_ipv6.iter().copied().map(IpAddr::V6))
        .collect();
    if public_addresses_only
        && expected
            .iter()
            .any(|address| !public_resolver(SocketAddr::new(*address, challenge_port)))
    {
        return Err(dns_failed());
    }
    let probe = dns_name(&format!("p{:08x}.{base}", rand::random::<u32>()))?;
    for &resolver in resolvers {
        for owner in [&ingress, &nameserver] {
            let found = resolved_addresses(resolver, owner, None).await?;
            if found != expected {
                return Err(dns_failed());
            }
        }
        let found = resolved_addresses(resolver, &probe, Some(&ingress)).await?;
        if found != expected {
            return Err(dns_failed());
        }
        let delegation = dns_query(resolver, &challenge, RecordType::NS, true).await?;
        if ns_targets(&delegation.answers, &challenge) != BTreeSet::from([nameserver.clone()]) {
            return Err(dns_failed());
        }
        verify_caa(resolver, dns_name(base)?).await?;
    }
    verify_parent_delegation(
        resolvers[0],
        authority_port,
        &challenge,
        &nameserver,
        public_addresses_only,
    )
    .await?;
    for address in expected {
        super::gateway_dns_probe::probe_challenge_address(
            SocketAddr::new(address, challenge_port),
            &format!("_acme-challenge.{base}"),
            &nameserver,
        )
        .await?;
    }
    Ok(())
}

async fn resolved_addresses(
    resolver: SocketAddr,
    owner: &Name,
    alias_target: Option<&Name>,
) -> Result<BTreeSet<IpAddr>, PlatformError> {
    let mut addresses = BTreeSet::new();
    for kind in [RecordType::A, RecordType::AAAA] {
        let response = dns_query(resolver, owner, kind, true).await?;
        let mut target = owner.clone();
        for record in &response.answers {
            if record.name == *owner
                && record.dns_class == DNSClass::IN
                && let RData::CNAME(alias) = &record.data
            {
                if Some(&alias.0) != alias_target {
                    return Err(dns_failed());
                }
                target = alias.0.clone();
            }
        }
        if alias_target.is_some() && target == *owner {
            return Err(dns_failed());
        }
        let followup = if target != *owner {
            Some(dns_query(resolver, &target, kind, true).await?)
        } else {
            None
        };
        for record in response
            .answers
            .iter()
            .chain(followup.iter().flat_map(|message| &message.answers))
        {
            if record.name == target && record.dns_class == DNSClass::IN {
                match &record.data {
                    RData::A(address) => {
                        addresses.insert(IpAddr::V4(address.0));
                    }
                    RData::AAAA(address) => {
                        addresses.insert(IpAddr::V6(address.0));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(addresses)
}

async fn verify_parent_delegation(
    resolver: SocketAddr,
    authority_port: u16,
    challenge: &Name,
    nameserver: &Name,
    public_addresses_only: bool,
) -> Result<(), PlatformError> {
    let base = challenge.base_name();
    let soa = dns_query(resolver, &base, RecordType::SOA, true).await?;
    let zone = soa
        .answers
        .iter()
        .chain(&soa.authorities)
        .filter(|record| matches!(record.data, RData::SOA(_)) && record.name.zone_of(&base))
        .map(|record| &record.name)
        .max_by_key(|name| name.num_labels())
        .ok_or_else(dns_failed)?;
    let ns = dns_query(resolver, zone, RecordType::NS, true).await?;
    let authorities = ns_targets(&ns.answers, zone);
    if authorities.is_empty() || authorities.len() > 16 {
        return Err(dns_failed());
    }
    for authority in authorities {
        let addresses = resolved_addresses(resolver, &authority, None).await?;
        if addresses.is_empty() || addresses.len() > 16 {
            return Err(dns_failed());
        }
        for address in addresses {
            let server = SocketAddr::new(address, authority_port);
            if public_addresses_only && !public_resolver(server) {
                return Err(dns_failed());
            }
            let parent = dns_query(server, zone, RecordType::SOA, false).await?;
            if !parent.metadata.authoritative
                || !parent.answers.iter().any(|record| {
                    record.name == *zone
                        && record.dns_class == DNSClass::IN
                        && matches!(record.data, RData::SOA(_))
                })
            {
                return Err(dns_failed());
            }
            let response = dns_query(server, challenge, RecordType::NS, false).await?;
            if response.metadata.authoritative {
                return Err(dns_failed());
            }
            let delegates = ns_targets(&response.authorities, challenge);
            if delegates != BTreeSet::from([nameserver.clone()]) {
                return Err(dns_failed());
            }
        }
    }
    Ok(())
}

fn ns_targets(records: &[hickory_proto::rr::Record], owner: &Name) -> BTreeSet<Name> {
    records
        .iter()
        .filter(|record| record.name == *owner && record.dns_class == DNSClass::IN)
        .filter_map(|record| match &record.data {
            RData::NS(NS(name)) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

async fn verify_caa(resolver: SocketAddr, mut owner: Name) -> Result<(), PlatformError> {
    while !owner.is_root() {
        let response = dns_query(resolver, &owner, RecordType::CAA, true).await?;
        if response
            .answers
            .iter()
            .any(|record| matches!(record.data, RData::CNAME(_)))
        {
            return Err(dns_failed());
        }
        let records: Vec<&CAA> = response
            .answers
            .iter()
            .filter(|record| record.name == owner)
            .filter_map(|record| match &record.data {
                RData::CAA(caa) => Some(caa),
                _ => None,
            })
            .collect();
        if !records.is_empty() {
            return if caa_allows_wildcard(&records) {
                Ok(())
            } else {
                Err(dns_failed())
            };
        }
        owner = owner.base_name();
    }
    Ok(())
}

fn caa_allows_wildcard(records: &[&CAA]) -> bool {
    if records.iter().any(|record| {
        record.issuer_critical
            && !["issue", "issuewild", "iodef"].contains(&record.tag.to_ascii_lowercase().as_str())
    }) {
        return false;
    }
    let selected = if records
        .iter()
        .any(|record| record.tag.eq_ignore_ascii_case("issuewild"))
    {
        "issuewild"
    } else {
        "issue"
    };
    let restrictions: Vec<_> = records
        .iter()
        .filter(|record| record.tag.eq_ignore_ascii_case(selected))
        .collect();
    restrictions.is_empty()
        || restrictions.iter().any(|record| {
            matches!(record.value_as_issue(), Ok((Some(name), options))
            if name.to_ascii().eq_ignore_ascii_case(ACME_ISSUER)
                && options.iter().all(|option| {
                    option.key().eq_ignore_ascii_case("validationmethods")
                        && option.value().split(',').any(|method| method == "dns-01")
                }))
        })
}

async fn dns_query(
    server: SocketAddr,
    owner: &Name,
    kind: RecordType,
    recursive: bool,
) -> Result<Message, PlatformError> {
    let mut query = Message::new(rand::random(), MessageType::Query, OpCode::Query);
    query.metadata.recursion_desired = recursive;
    query.add_query(Query::query(owner.clone(), kind));
    let wire = query.to_vec().map_err(|_| dns_failed())?;
    let bind = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).await.map_err(|_| dns_failed())?;
    socket.connect(server).await.map_err(|_| dns_failed())?;
    let mut buffer = [0u8; MAX_DNS_BYTES];
    let size = tokio::time::timeout(DNS_TIMEOUT, async {
        socket.send(&wire).await?;
        socket.recv(&mut buffer).await
    })
    .await
    .map_err(|_| dns_failed())?
    .map_err(|_| dns_failed())?;
    let mut response = Message::from_vec(&buffer[..size]).map_err(|_| dns_failed())?;
    if response.metadata.truncation {
        response = dns_query_tcp(server, &wire).await?;
    }
    if response.metadata.message_type != MessageType::Response
        || response.metadata.id != query.metadata.id
        || response.metadata.response_code != ResponseCode::NoError
        || response.metadata.truncation
        || response.queries != query.queries
        || (recursive && !response.metadata.recursion_available)
    {
        return Err(dns_failed());
    }
    Ok(response)
}

async fn dns_query_tcp(server: SocketAddr, wire: &[u8]) -> Result<Message, PlatformError> {
    let mut stream = tokio::time::timeout(DNS_TIMEOUT, TcpStream::connect(server))
        .await
        .map_err(|_| dns_failed())?
        .map_err(|_| dns_failed())?;
    tokio::time::timeout(DNS_TIMEOUT, async {
        stream
            .write_all(
                &u16::try_from(wire.len())
                    .map_err(|_| dns_failed())?
                    .to_be_bytes(),
            )
            .await
            .map_err(|_| dns_failed())?;
        stream.write_all(wire).await.map_err(|_| dns_failed())?;
        let mut length = [0u8; 2];
        stream
            .read_exact(&mut length)
            .await
            .map_err(|_| dns_failed())?;
        let size = usize::from(u16::from_be_bytes(length));
        if size == 0 || size > MAX_DNS_BYTES {
            return Err(dns_failed());
        }
        let mut buffer = vec![0u8; size];
        stream
            .read_exact(&mut buffer)
            .await
            .map_err(|_| dns_failed())?;
        Message::from_vec(&buffer).map_err(|_| dns_failed())
    })
    .await
    .map_err(|_| dns_failed())?
}

fn dns_name(value: &str) -> Result<Name, PlatformError> {
    Name::from_ascii(format!("{}.", value.trim_end_matches('.'))).map_err(|_| dns_failed())
}

fn dns_failed() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "public gateway DNS verification failed",
    )
}

#[cfg(test)]
#[path = "gateway_dns_verify_tests.rs"]
mod tests;
