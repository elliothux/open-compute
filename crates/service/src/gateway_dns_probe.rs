//! Read-only public challenge DNS reachability probe.

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::NS;
use hickory_proto::rr::{DNSClass, Name, RData, RecordType};
use open_compute_core::{ErrorCode, PlatformError, PublicGatewayConfig};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

const MAX_QUERY_BYTES: usize = 1232;

/// Probe the configured public DNS addresses, including external UDP/TCP 53 forwarding.
pub(crate) async fn probe_public_challenge_dns(
    gateway: &PublicGatewayConfig,
) -> Result<(), PlatformError> {
    let zone = format!("_acme-challenge.{}.", gateway.base_domain);
    let nameserver = Name::from_ascii(format!("ns1.{}.", gateway.base_domain))
        .map_err(|_| challenge_probe_failed())?;
    for address in gateway
        .ingress_ipv4
        .iter()
        .copied()
        .map(IpAddr::V4)
        .chain(gateway.ingress_ipv6.iter().copied().map(IpAddr::V6))
    {
        probe_challenge_address(SocketAddr::new(address, 53), &zone, &nameserver).await?;
    }
    Ok(())
}

pub(crate) async fn probe_challenge_address(
    address: SocketAddr,
    zone: &str,
    nameserver: &Name,
) -> Result<(), PlatformError> {
    let bind = match address {
        SocketAddr::V4(_) => "0.0.0.0:0",
        SocketAddr::V6(_) => "[::]:0",
    };
    let udp = UdpSocket::bind(bind)
        .await
        .map_err(|_| challenge_probe_failed())?;
    udp.connect(address)
        .await
        .map_err(|_| challenge_probe_failed())?;
    let mut tcp = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(address))
        .await
        .map_err(|_| challenge_probe_failed())?
        .map_err(|_| challenge_probe_failed())?;
    for kind in [RecordType::SOA, RecordType::NS] {
        let name = Name::from_ascii(format!("{}.", zone.trim_end_matches('.')))
            .map_err(|_| challenge_probe_failed())?;
        let mut query = Message::new(rand::random(), MessageType::Query, OpCode::Query);
        query.add_query(Query::query(name.clone(), kind));
        let wire = query.to_vec().map_err(|_| challenge_probe_failed())?;
        let mut buffer = [0u8; MAX_QUERY_BYTES];
        let udp_size = tokio::time::timeout(Duration::from_secs(3), async {
            udp.send(&wire).await?;
            udp.recv(&mut buffer).await
        })
        .await
        .map_err(|_| challenge_probe_failed())?
        .map_err(|_| challenge_probe_failed())?;
        check_challenge_response(&buffer[..udp_size], &query, &name, kind, nameserver)?;
        let mut length = [0u8; 2];
        tokio::time::timeout(Duration::from_secs(3), async {
            tcp.write_all(
                &u16::try_from(wire.len())
                    .map_err(|_| challenge_probe_failed())?
                    .to_be_bytes(),
            )
            .await
            .map_err(|_| challenge_probe_failed())?;
            tcp.write_all(&wire)
                .await
                .map_err(|_| challenge_probe_failed())?;
            tcp.read_exact(&mut length)
                .await
                .map_err(|_| challenge_probe_failed())?;
            let size = usize::from(u16::from_be_bytes(length));
            if size == 0 || size > MAX_QUERY_BYTES {
                return Err(challenge_probe_failed());
            }
            tcp.read_exact(&mut buffer[..size])
                .await
                .map_err(|_| challenge_probe_failed())?;
            check_challenge_response(&buffer[..size], &query, &name, kind, nameserver)
        })
        .await
        .map_err(|_| challenge_probe_failed())??;
    }
    Ok(())
}

pub(crate) fn check_challenge_response(
    wire: &[u8],
    query: &Message,
    zone: &Name,
    kind: RecordType,
    nameserver: &Name,
) -> Result<(), PlatformError> {
    let response = Message::from_vec(wire).map_err(|_| challenge_probe_failed())?;
    let valid = response.metadata.message_type == MessageType::Response
        && response.metadata.id == query.metadata.id
        && response.metadata.response_code == ResponseCode::NoError
        && response.metadata.authoritative
        && !response.metadata.truncation
        && response.queries == query.queries
        && response.answers.iter().any(|record| {
            record.name == *zone
                && record.dns_class == DNSClass::IN
                && match (&record.data, kind) {
                    (RData::NS(NS(target)), RecordType::NS) => target == nameserver,
                    (RData::SOA(soa), RecordType::SOA) => soa.mname == *nameserver,
                    _ => false,
                }
        });
    if valid {
        Ok(())
    } else {
        Err(challenge_probe_failed())
    }
}

fn challenge_probe_failed() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "public challenge DNS UDP/TCP 53 probe failed",
    )
}
