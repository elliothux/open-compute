//! Minimal authoritative DNS state for the delegated ACME challenge names.

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{NS, SOA, TXT};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};
use open_compute_core::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket, UnixStream};
use tokio::sync::{Semaphore, watch};
use uuid::Uuid;

const MAX_QUERY_BYTES: usize = 1232;
const MAX_RECORDS: usize = 32;
const TTL_SECONDS: u32 = 60;
const MAX_PROVIDER_BYTES: usize = 1024;

struct ChallengeRecord {
    zone: String,
    value: String,
    expires_at: Instant,
}

/// In-memory TXT authority. A restart intentionally discards all pending challenges.
pub(crate) struct ChallengeAuthority {
    zones: Vec<String>,
    nameserver: Name,
    mailbox: Name,
    records: Mutex<BTreeMap<String, ChallengeRecord>>,
}

impl ChallengeAuthority {
    pub(crate) fn new(base_domain: &str, r2_enabled: bool) -> Result<Self, PlatformError> {
        let zones = [
            format!("_acme-challenge.{base_domain}"),
            format!("_acme-challenge.r2.{base_domain}"),
        ]
        .into_iter()
        .take(if r2_enabled { 2 } else { 1 })
        .collect();
        let nameserver =
            Name::from_ascii(format!("ns1.{base_domain}.")).map_err(|_| invalid_challenge())?;
        let mailbox = Name::from_ascii(format!("hostmaster.{base_domain}."))
            .map_err(|_| invalid_challenge())?;
        Ok(Self {
            zones,
            nameserver,
            mailbox,
            records: Mutex::new(BTreeMap::new()),
        })
    }

    pub(crate) fn append(&self, zone: &str, value: &str) -> Result<String, PlatformError> {
        if !self.valid_record(zone, value) {
            return Err(invalid_challenge());
        }
        let mut records = self.records.lock().map_err(|_| invalid_challenge())?;
        records.retain(|_, record| record.expires_at > Instant::now());
        if records.len() >= MAX_RECORDS {
            return Err(invalid_challenge());
        }
        let id = Uuid::now_v7().to_string();
        records.insert(
            id.clone(),
            ChallengeRecord {
                zone: zone.to_owned(),
                value: value.to_owned(),
                expires_at: Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(id)
    }

    pub(crate) fn delete(&self, id: &str) -> Result<bool, PlatformError> {
        let mut records = self.records.lock().map_err(|_| invalid_challenge())?;
        records.retain(|_, record| record.expires_at > Instant::now());
        Ok(records.remove(id).is_some())
    }

    pub(crate) fn delete_exact(&self, zone: &str, value: &str) -> Result<bool, PlatformError> {
        if !self.valid_record(zone, value) {
            return Err(invalid_challenge());
        }
        let mut records = self.records.lock().map_err(|_| invalid_challenge())?;
        records.retain(|_, record| record.expires_at > Instant::now());
        let ids: Vec<_> = records
            .iter()
            .filter(|(_, record)| record.zone == zone && record.value == value)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            records.remove(id);
        }
        Ok(!ids.is_empty())
    }

    fn valid_record(&self, zone: &str, value: &str) -> bool {
        self.zones.iter().any(|allowed| allowed == zone)
            && !value.is_empty()
            && value.len() <= 255
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }

    pub(crate) fn answer(&self, wire: &[u8], udp: bool) -> Option<Vec<u8>> {
        if wire.len() > MAX_QUERY_BYTES {
            return None;
        }
        let request = Message::from_vec(wire).ok()?;
        let mut response = Message::response(request.metadata.id, request.metadata.op_code);
        response.metadata.recursion_desired = request.metadata.recursion_desired;
        let Some(query) = request.queries.first() else {
            response.metadata.response_code = ResponseCode::FormErr;
            return response.to_vec().ok();
        };
        response.add_query(query.clone());
        if request.metadata.message_type != MessageType::Query
            || request.metadata.op_code != OpCode::Query
            || request.queries.len() != 1
            || query.query_class() != DNSClass::IN
            || !request.answers.is_empty()
            || !request.authorities.is_empty()
        {
            response.metadata.response_code = ResponseCode::Refused;
            return response.to_vec().ok();
        }
        let name = query
            .name()
            .to_ascii()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if !self.zones.iter().any(|zone| zone == &name) {
            response.metadata.response_code = ResponseCode::Refused;
            return response.to_vec().ok();
        }
        response.metadata.authoritative = true;
        let owner = query.name().clone();
        match query.query_type() {
            RecordType::TXT => {
                let records = self.records.lock().ok()?;
                for record in records
                    .values()
                    .filter(|record| record.zone == name && record.expires_at > Instant::now())
                {
                    response.add_answer(Record::from_rdata(
                        owner.clone(),
                        TTL_SECONDS,
                        RData::TXT(TXT::new(vec![record.value.clone()])),
                    ));
                }
                if response.answers.is_empty() {
                    response.add_authority(self.soa(owner));
                }
            }
            RecordType::NS => {
                response.add_answer(Record::from_rdata(
                    owner,
                    TTL_SECONDS,
                    RData::NS(NS(self.nameserver.clone())),
                ));
            }
            RecordType::SOA => {
                response.add_answer(self.soa(owner));
            }
            RecordType::AXFR | RecordType::IXFR | RecordType::ANY => {
                response.metadata.response_code = ResponseCode::Refused;
            }
            _ => {
                response.add_authority(self.soa(owner));
            }
        }
        let encoded = response.to_vec().ok()?;
        if udp && encoded.len() > 512 {
            response.answers.clear();
            response.authorities.clear();
            response.metadata.truncation = true;
            response.to_vec().ok()
        } else {
            Some(encoded)
        }
    }

    fn soa(&self, owner: Name) -> Record {
        Record::from_rdata(
            owner,
            TTL_SECONDS,
            RData::SOA(SOA::new(
                self.nameserver.clone(),
                self.mailbox.clone(),
                1,
                300,
                60,
                3600,
                TTL_SECONDS,
            )),
        )
    }
}

fn invalid_challenge() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "invalid delegated ACME challenge")
}

/// UDP and TCP DNS sockets for one fixed challenge authority.
pub(crate) struct ChallengeDnsServer {
    udp: UdpSocket,
    tcp: TcpListener,
    authority: Arc<ChallengeAuthority>,
}

impl ChallengeDnsServer {
    pub(crate) async fn bind(
        address: SocketAddr,
        authority: Arc<ChallengeAuthority>,
    ) -> Result<Self, PlatformError> {
        let udp = UdpSocket::bind(address)
            .await
            .map_err(|_| invalid_challenge())?;
        let actual = SocketAddr::new(
            address.ip(),
            udp.local_addr().map_err(|_| invalid_challenge())?.port(),
        );
        let tcp = TcpListener::bind(actual)
            .await
            .map_err(|_| invalid_challenge())?;
        Ok(Self {
            udp,
            tcp,
            authority,
        })
    }

    #[cfg(test)]
    pub(crate) fn local_addr(&self) -> SocketAddr {
        self.udp.local_addr().expect("bound challenge DNS socket")
    }

    pub(crate) async fn serve(self, shutdown: watch::Receiver<bool>) -> Result<(), PlatformError> {
        tokio::try_join!(
            serve_udp(self.udp, self.authority.clone(), shutdown.clone()),
            serve_tcp(self.tcp, self.authority, shutdown),
        )?;
        Ok(())
    }
}

async fn serve_udp(
    socket: UdpSocket,
    authority: Arc<ChallengeAuthority>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), PlatformError> {
    let mut buffer = [0u8; MAX_QUERY_BYTES];
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            received = socket.recv_from(&mut buffer) => {
                let (size, peer) = received.map_err(|_| invalid_challenge())?;
                if let Some(response) = authority.answer(&buffer[..size], true) {
                    socket.send_to(&response, peer).await.map_err(|_| invalid_challenge())?;
                }
            }
        }
    }
}

async fn serve_tcp(
    listener: TcpListener,
    authority: Arc<ChallengeAuthority>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), PlatformError> {
    let capacity = Arc::new(Semaphore::new(64));
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|_| invalid_challenge())?;
                if let Ok(permit) = capacity.clone().try_acquire_owned() {
                    let authority = authority.clone();
                    clients.spawn(async move {
                        let _permit = permit;
                        serve_tcp_client(stream, authority).await;
                    });
                }
            }
            Some(_) = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}

async fn serve_tcp_client(mut stream: TcpStream, authority: Arc<ChallengeAuthority>) {
    loop {
        let mut length = [0u8; 2];
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut length)).await,
            Ok(Ok(_))
        ) {
            return;
        }
        let size = usize::from(u16::from_be_bytes(length));
        if size == 0 || size > MAX_QUERY_BYTES {
            return;
        }
        let mut query = vec![0u8; size];
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut query)).await,
            Ok(Ok(_))
        ) {
            return;
        }
        let Some(response) = authority.answer(&query, false) else {
            return;
        };
        let Ok(length) = u16::try_from(response.len()) else {
            return;
        };
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), async {
                stream.write_all(&length.to_be_bytes()).await?;
                stream.write_all(&response).await
            })
            .await,
            Ok(Ok(()))
        ) {
            return;
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ProviderRequest {
    Append { zone: String, value: String },
    Delete { id: String },
    DeleteExact { zone: String, value: String },
}

#[derive(Serialize)]
struct ProviderResponse {
    id: Option<String>,
    deleted: bool,
    error: Option<&'static str>,
}

/// Private provider endpoint; only the current Caddy child PID can mutate TXT state.
pub(crate) struct ChallengeProviderServer {
    socket: crate::http::PrivateUnixListener,
    authority: Arc<ChallengeAuthority>,
    caddy_pid: Arc<AtomicI32>,
}

impl ChallengeProviderServer {
    pub(crate) fn bind(
        path: std::path::PathBuf,
        authority: Arc<ChallengeAuthority>,
        caddy_pid: Arc<AtomicI32>,
    ) -> Result<Self, PlatformError> {
        Ok(Self {
            socket: crate::http::PrivateUnixListener::bind(path)?,
            authority,
            caddy_pid,
        })
    }

    pub(crate) async fn serve(
        mut self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        let listener = self.socket.take_listener()?;
        let capacity = Arc::new(Semaphore::new(32));
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|_| invalid_challenge())?;
                    if let Ok(permit) = capacity.clone().try_acquire_owned() {
                        let authority = self.authority.clone();
                        let caddy_pid = self.caddy_pid.clone();
                        clients.spawn(async move {
                            let _permit = permit;
                            serve_provider_client(stream, authority, caddy_pid).await;
                        });
                    }
                }
                Some(_) = clients.join_next(), if !clients.is_empty() => {}
            }
        }
    }
}

async fn serve_provider_client(
    mut stream: UnixStream,
    authority: Arc<ChallengeAuthority>,
    caddy_pid: Arc<AtomicI32>,
) {
    let current_pid = caddy_pid.load(Ordering::Acquire);
    let Ok(credentials) = stream.peer_cred() else {
        return;
    };
    if current_pid <= 0
        || credentials.pid() != Some(current_pid)
        || credentials.uid() != rustix::process::getuid().as_raw()
    {
        return;
    }
    let mut length = [0u8; 2];
    if !matches!(
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut length)).await,
        Ok(Ok(_))
    ) {
        return;
    }
    let size = usize::from(u16::from_be_bytes(length));
    if size == 0 || size > MAX_PROVIDER_BYTES {
        return;
    }
    let mut body = vec![0u8; size];
    if !matches!(
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut body)).await,
        Ok(Ok(_))
    ) {
        return;
    }
    if caddy_pid.load(Ordering::Acquire) != current_pid {
        return;
    }
    let response = match serde_json::from_slice::<ProviderRequest>(&body) {
        Ok(ProviderRequest::Append { zone, value }) => match authority.append(&zone, &value) {
            Ok(id) => ProviderResponse {
                id: Some(id),
                deleted: false,
                error: None,
            },
            Err(_) => ProviderResponse {
                id: None,
                deleted: false,
                error: Some("invalid"),
            },
        },
        Ok(ProviderRequest::Delete { id }) => match authority.delete(&id) {
            Ok(deleted) => ProviderResponse {
                id: None,
                deleted,
                error: None,
            },
            Err(_) => ProviderResponse {
                id: None,
                deleted: false,
                error: Some("invalid"),
            },
        },
        Ok(ProviderRequest::DeleteExact { zone, value }) => {
            match authority.delete_exact(&zone, &value) {
                Ok(deleted) => ProviderResponse {
                    id: None,
                    deleted,
                    error: None,
                },
                Err(_) => ProviderResponse {
                    id: None,
                    deleted: false,
                    error: Some("invalid"),
                },
            }
        }
        Err(_) => ProviderResponse {
            id: None,
            deleted: false,
            error: Some("invalid"),
        },
    };
    let Ok(encoded) = serde_json::to_vec(&response) else {
        return;
    };
    let Ok(length) = u16::try_from(encoded.len()) else {
        return;
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_all(&length.to_be_bytes()).await?;
        stream.write_all(&encoded).await
    })
    .await;
}

#[cfg(test)]
#[path = "challenge_dns_tests.rs"]
mod tests;
