use super::*;
use crate::challenge_dns::{ChallengeAuthority, ChallengeDnsServer};
use hickory_proto::rr::Record;
use hickory_proto::rr::rdata::caa::KeyValue;
use hickory_proto::rr::rdata::{A, CNAME, SOA};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::watch;

fn name(value: &str) -> Name {
    dns_name(value).unwrap()
}

fn record(owner: &str, data: RData) -> Record {
    Record::from_rdata(name(owner), 60, data)
}

#[test]
fn caa_wildcard_policy_honors_issuewild_and_unknown_critical_tags() {
    let issue = CAA::new_issue(false, Some(name("letsencrypt.org")), Vec::new());
    let issuewild = CAA::new_issuewild(false, Some(name("other.example")), Vec::new());
    assert!(!caa_allows_wildcard(&[&issue, &issuewild]));
    assert!(caa_allows_wildcard(&[&issue]));
    let mut unknown = CAA::new_issue(false, None, Vec::new());
    unknown.issuer_critical = true;
    unknown.tag = "unknown".to_owned();
    assert!(!caa_allows_wildcard(&[&issue, &unknown]));

    let dns_only = CAA::new_issuewild(
        false,
        Some(name("letsencrypt.org")),
        vec![KeyValue::new("validationmethods", "http-01,dns-01")],
    );
    assert!(caa_allows_wildcard(&[&dns_only]));
    let http_only = CAA::new_issuewild(
        false,
        Some(name("letsencrypt.org")),
        vec![KeyValue::new("validationmethods", "http-01")],
    );
    assert!(!caa_allows_wildcard(&[&http_only]));
    let account_bound = CAA::new_issuewild(
        false,
        Some(name("letsencrypt.org")),
        vec![KeyValue::new(
            "accounturi",
            "https://acme-v02.api.letsencrypt.org/acme/acct/123",
        )],
    );
    assert!(!caa_allows_wildcard(&[&account_bound]));
}

#[test]
fn public_verification_rejects_local_resolvers() {
    for address in [
        "127.0.0.1:53",
        "10.0.0.1:53",
        "169.254.1.1:53",
        "[::1]:53",
        "[fd00::1]:53",
        "[::ffff:127.0.0.1]:53",
        "100.64.0.1:53",
        "0.1.2.3:53",
        "192.0.0.1:53",
        "192.88.99.1:53",
        "198.18.0.1:53",
        "240.0.0.1:53",
        "255.255.255.254:53",
        "[100::1]:53",
        "[2001:db8::1]:53",
        "[2002::1]:53",
        "[3fff::1]:53",
        "1.1.1.1:0",
    ] {
        assert!(!public_resolver(address.parse().unwrap()), "{address}");
    }
    for address in ["1.1.1.1:53", "8.8.8.8:53", "[2606:4700:4700::1111]:53"] {
        assert!(public_resolver(address.parse().unwrap()), "{address}");
    }
}

async fn fake_resolver(
    socket: UdpSocket,
    mode: Arc<AtomicU8>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut buffer = [0u8; MAX_DNS_BYTES];
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            received = socket.recv_from(&mut buffer) => {
                let Ok((size, peer)) = received else { return };
                let Ok(request) = Message::from_vec(&buffer[..size]) else { continue };
                let Some(question) = request.queries.first() else { continue };
                let owner = question.name().to_ascii().to_ascii_lowercase();
                let owner = owner.trim_end_matches('.');
                let kind = question.query_type();
                let mut response = Message::response(request.metadata.id, OpCode::Query);
                response.add_query(question.clone());
                response.metadata.recursion_available = request.metadata.recursion_desired;
                response.metadata.authoritative = !request.metadata.recursion_desired
                    && (owner != "_acme-challenge.compute.example.com"
                        || mode.load(Ordering::Relaxed) == 6)
                    && mode.load(Ordering::Relaxed) != 5;
                match (owner, kind, request.metadata.recursion_desired) {
                    ("example.com", RecordType::SOA, false) => {
                        response.add_answer(record("example.com", RData::SOA(SOA::new(
                            name("ns.example.com"), name("hostmaster.example.com"), 1, 60, 60, 60, 60,
                        ))));
                    }
                    ("compute.example.com", RecordType::SOA, _) => {
                        response.add_authority(record("example.com", RData::SOA(SOA::new(
                            name("ns.example.com"), name("hostmaster.example.com"), 1, 60, 60, 60, 60,
                        ))));
                    }
                    ("example.com", RecordType::NS, _) => {
                        response.add_answer(record("example.com", RData::NS(NS(name("ns.example.com")))));
                    }
                    ("_acme-challenge.compute.example.com", RecordType::NS, true) => {
                        response.add_answer(record(owner, RData::NS(NS(name("ns1.compute.example.com")))));
                    }
                    ("_acme-challenge.compute.example.com", RecordType::NS, false) => {
                        let target = if mode.load(Ordering::Relaxed) == 3 {
                            "other.example.com"
                        } else {
                            "ns1.compute.example.com"
                        };
                        response.add_authority(record(owner, RData::NS(NS(name(target)))));
                    }
                    ("compute.example.com", RecordType::CAA, _) => {
                        let issuer = if mode.load(Ordering::Relaxed) == 1 { "other.example" } else { "letsencrypt.org" };
                        response.add_answer(record(owner, RData::CAA(CAA::new_issuewild(
                            false, Some(name(issuer)), Vec::new(),
                        ))));
                    }
                    ("ingress.compute.example.com" | "ns1.compute.example.com" | "ns.example.com", RecordType::A, _) => {
                        response.add_answer(record(owner, RData::A(A(Ipv4Addr::LOCALHOST))));
                    }
                    (value, RecordType::A | RecordType::AAAA, true)
                        if value.starts_with('p') && value.ends_with(".compute.example.com") => {
                        if mode.load(Ordering::Relaxed) == 4 {
                            if kind == RecordType::A {
                                response.add_answer(record(value, RData::A(A(Ipv4Addr::LOCALHOST))));
                            }
                        } else {
                        let alias = if mode.load(Ordering::Relaxed) == 2 {
                            "other.example.com"
                        } else {
                            "ingress.compute.example.com"
                        };
                        let mut alias_record = record(value, RData::CNAME(CNAME(name(alias))));
                        if mode.load(Ordering::Relaxed) == 7 {
                            alias_record.dns_class = DNSClass::CH;
                        }
                        response.add_answer(alias_record);
                        }
                    }
                    _ => {}
                }
                let Ok(wire) = response.to_vec() else { continue };
                let _ = socket.send_to(&wire, peer).await;
            }
        }
    }
}

#[tokio::test]
async fn public_dns_verification_checks_recursive_parent_challenge_and_caa() {
    let resolver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver_address = resolver.local_addr().unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let (stop_resolver, resolver_shutdown) = watch::channel(false);
    let resolver_task = tokio::spawn(fake_resolver(resolver, mode.clone(), resolver_shutdown));

    let challenge = Arc::new(ChallengeAuthority::new("compute.example.com", false).unwrap());
    let dns = ChallengeDnsServer::bind("127.0.0.1:0".parse().unwrap(), challenge)
        .await
        .unwrap();
    let challenge_address = dns.local_addr();
    let (stop_challenge, challenge_shutdown) = watch::channel(false);
    let challenge_task = tokio::spawn(dns.serve(challenge_shutdown));

    let gateway = PublicGatewayConfig {
        base_domain: "compute.example.com".to_owned(),
        ingress_ipv4: vec![Ipv4Addr::LOCALHOST],
        ingress_ipv6: Vec::new(),
        https_listen: "127.0.0.1:8443".parse().unwrap(),
        challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
        proxy_protocol_from: Vec::new(),
        caddy: Vec::new(),
    };
    assert!(
        verify_public_gateway_dns(&gateway, &[resolver_address])
            .await
            .is_err(),
        "the public entry point must reject a loopback recursive resolver"
    );
    verify_with(
        &gateway,
        &[resolver_address],
        resolver_address.port(),
        challenge_address.port(),
        false,
    )
    .await
    .unwrap();
    assert!(
        verify_parent_delegation(
            resolver_address,
            resolver_address.port(),
            &name("_acme-challenge.compute.example.com"),
            &name("ns1.compute.example.com"),
            true,
        )
        .await
        .is_err(),
        "public verification queried a private parent nameserver"
    );
    for failure in [1, 2, 3, 4, 5, 6, 7] {
        mode.store(failure, Ordering::Relaxed);
        assert!(
            verify_with(
                &gateway,
                &[resolver_address],
                resolver_address.port(),
                challenge_address.port(),
                false,
            )
            .await
            .is_err(),
            "invalid DNS fixture mode {failure} was accepted"
        );
    }
    stop_resolver.send(true).unwrap();
    stop_challenge.send(true).unwrap();
    resolver_task.await.unwrap();
    challenge_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn empty_challenge_probe_plan_is_a_noop() {
    let gateway = PublicGatewayConfig {
        base_domain: "compute.example.com".to_owned(),
        ingress_ipv4: Vec::new(),
        ingress_ipv6: Vec::new(),
        https_listen: "127.0.0.1:8443".parse().unwrap(),
        challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
        proxy_protocol_from: Vec::new(),
        caddy: Vec::new(),
    };
    crate::gateway_dns_probe::probe_public_challenge_dns(&gateway)
        .await
        .unwrap();
    let nameserver = name("ns1.compute.example.com");
    assert!(
        crate::gateway_dns_probe::probe_challenge_address(
            "[::1]:1".parse().unwrap(),
            "_acme-challenge.compute.example.com",
            &nameserver,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn truncated_udp_dns_response_retries_over_tcp() {
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = tcp.local_addr().unwrap();
    let udp = UdpSocket::bind(address).await.unwrap();
    let server = tokio::spawn(async move {
        let mut buffer = [0u8; MAX_DNS_BYTES];
        let (size, peer) = udp.recv_from(&mut buffer).await.unwrap();
        let request = Message::from_vec(&buffer[..size]).unwrap();
        let mut truncated = Message::response(request.metadata.id, OpCode::Query);
        truncated.metadata.recursion_available = true;
        truncated.metadata.truncation = true;
        truncated.add_query(request.queries[0].clone());
        udp.send_to(&truncated.to_vec().unwrap(), peer)
            .await
            .unwrap();

        let (mut stream, _) = tcp.accept().await.unwrap();
        let mut length = [0u8; 2];
        stream.read_exact(&mut length).await.unwrap();
        let mut wire = vec![0; usize::from(u16::from_be_bytes(length))];
        stream.read_exact(&mut wire).await.unwrap();
        let request = Message::from_vec(&wire).unwrap();
        let mut response = Message::response(request.metadata.id, OpCode::Query);
        response.metadata.recursion_available = true;
        response.add_query(request.queries[0].clone());
        let wire = response.to_vec().unwrap();
        stream
            .write_all(&u16::try_from(wire.len()).unwrap().to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&wire).await.unwrap();
    });
    dns_query(address, &name("example.com"), RecordType::A, true)
        .await
        .unwrap();
    server.await.unwrap();
}
