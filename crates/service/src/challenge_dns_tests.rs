use super::*;
use crate::gateway_dns_probe::check_challenge_response;
use hickory_proto::op::Query;

fn query(name: &str, kind: RecordType) -> Vec<u8> {
    let mut message = Message::new(7, MessageType::Query, OpCode::Query);
    message.add_query(Query::query(Name::from_ascii(name).unwrap(), kind));
    message.to_vec().unwrap()
}

#[test]
fn challenge_authority_only_answers_delegated_names() {
    let authority = ChallengeAuthority::new("compute.example.com", false).unwrap();
    let zone = "_acme-challenge.compute.example.com";
    assert!(
        authority
            .append("_acme-challenge.other.example.com", "token")
            .is_err()
    );
    assert!(authority.append(zone, "bad token").is_err());
    let id = authority.append(zone, "valid-token_1").unwrap();
    authority.append(zone, "valid-token_1").unwrap();
    let response = Message::from_vec(
        &authority
            .answer(&query(zone, RecordType::TXT), true)
            .unwrap(),
    )
    .unwrap();
    assert!(response.metadata.authoritative);
    assert_eq!(response.metadata.id, 7);
    assert_eq!(response.answers.len(), 2);
    assert_eq!(
        response.answers[0].data,
        RData::TXT(TXT::new(vec!["valid-token_1".into()]))
    );
    let refused = Message::from_vec(
        &authority
            .answer(
                &query("_acme-challenge.r2.compute.example.com", RecordType::TXT),
                true,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(refused.metadata.response_code, ResponseCode::Refused);
    assert!(authority.delete(&id).unwrap());
    assert!(!authority.delete(&id).unwrap());
    assert!(authority.delete_exact(zone, "valid-token_1").unwrap());
    assert!(!authority.delete_exact(zone, "valid-token_1").unwrap());
    let expired = authority.append(zone, "expired-token").unwrap();
    authority
        .records
        .lock()
        .unwrap()
        .get_mut(&expired)
        .unwrap()
        .expires_at = Instant::now() - Duration::from_secs(1);
    assert!(!authority.delete(&expired).unwrap());
    let empty = Message::from_vec(
        &authority
            .answer(&query(zone, RecordType::TXT), true)
            .unwrap(),
    )
    .unwrap();
    assert!(empty.answers.is_empty());
    assert_eq!(empty.authorities.len(), 1);

    let full = ChallengeAuthority::new("compute.example.com", false).unwrap();
    for index in 0..MAX_RECORDS {
        full.append(zone, &format!("token_{index:02}")).unwrap();
    }
    assert!(full.append(zone, "overflow").is_err());
    let truncated =
        Message::from_vec(&full.answer(&query(zone, RecordType::TXT), true).unwrap()).unwrap();
    assert!(truncated.metadata.truncation);
    assert!(truncated.answers.is_empty());
    assert!(full.delete_exact(zone, "bad token").is_err());
    assert!(full.answer(&vec![0; MAX_QUERY_BYTES + 1], true).is_none());

    let mut empty_query = Message::new(8, MessageType::Query, OpCode::Query);
    let form_error =
        Message::from_vec(&full.answer(&empty_query.to_vec().unwrap(), true).unwrap()).unwrap();
    assert_eq!(form_error.metadata.response_code, ResponseCode::FormErr);
    empty_query.metadata.message_type = MessageType::Response;
    empty_query.add_query(Query::query(
        Name::from_ascii(zone).unwrap(),
        RecordType::TXT,
    ));
    let refused =
        Message::from_vec(&full.answer(&empty_query.to_vec().unwrap(), true).unwrap()).unwrap();
    assert_eq!(refused.metadata.response_code, ResponseCode::Refused);
}

#[test]
fn challenge_probe_rejects_wrong_nameserver_and_non_authoritative_response() {
    let authority = ChallengeAuthority::new("compute.example.com", false).unwrap();
    let wire = query("_acme-challenge.compute.example.com.", RecordType::NS);
    let request = Message::from_vec(&wire).unwrap();
    let answer = authority.answer(&wire, true).unwrap();
    let zone = request.queries[0].name();
    assert!(
        check_challenge_response(
            &answer,
            &request,
            zone,
            RecordType::NS,
            &Name::from_ascii("ns1.other.example.com.").unwrap(),
        )
        .is_err()
    );
    let mut response = Message::from_vec(&answer).unwrap();
    response.metadata.authoritative = false;
    assert!(
        check_challenge_response(
            &response.to_vec().unwrap(),
            &request,
            zone,
            RecordType::NS,
            &Name::from_ascii("ns1.compute.example.com.").unwrap(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn provider_and_dns_sockets_publish_then_remove_one_txt() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Arc::new(ChallengeAuthority::new("compute.example.com", false).unwrap());
    let dns = ChallengeDnsServer::bind("127.0.0.1:0".parse().unwrap(), authority.clone())
        .await
        .unwrap();
    let dns_addr = dns.udp.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let dns_task = tokio::spawn(dns.serve(shutdown_rx.clone()));
    let zone = "_acme-challenge.compute.example.com";
    crate::gateway_dns_probe::probe_challenge_address(
        dns_addr,
        zone,
        &Name::from_ascii("ns1.compute.example.com.").unwrap(),
    )
    .await
    .unwrap();
    let socket = directory.path().join("provider.sock");
    let pid = Arc::new(AtomicI32::new(i32::try_from(std::process::id()).unwrap()));
    let provider =
        ChallengeProviderServer::bind(socket.clone(), authority.clone(), pid.clone()).unwrap();
    let provider_task = tokio::spawn(provider.serve(shutdown_rx));

    async fn provider_call(path: &std::path::Path, body: serde_json::Value) -> serde_json::Value {
        let mut stream = UnixStream::connect(path).await.unwrap();
        let payload = serde_json::to_vec(&body).unwrap();
        stream
            .write_all(&u16::try_from(payload.len()).unwrap().to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&payload).await.unwrap();
        let mut length = [0u8; 2];
        stream.read_exact(&mut length).await.unwrap();
        let mut response = vec![0; usize::from(u16::from_be_bytes(length))];
        stream.read_exact(&mut response).await.unwrap();
        serde_json::from_slice(&response).unwrap()
    }

    let added = provider_call(
        &socket,
        serde_json::json!({"action":"append","zone":zone,"value":"token_123"}),
    )
    .await;
    let id = added["id"].as_str().unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp.send_to(&query(zone, RecordType::TXT), dns_addr)
        .await
        .unwrap();
    let mut buffer = [0u8; 1232];
    let (size, _) = tokio::time::timeout(Duration::from_secs(2), udp.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let answer = Message::from_vec(&buffer[..size]).unwrap();
    assert_eq!(answer.answers.len(), 1);

    let mut tcp = TcpStream::connect(dns_addr).await.unwrap();
    let wire = query(zone, RecordType::TXT);
    tcp.write_all(&u16::try_from(wire.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    tcp.write_all(&wire).await.unwrap();
    let mut length = [0u8; 2];
    tcp.read_exact(&mut length).await.unwrap();
    let mut body = vec![0; usize::from(u16::from_be_bytes(length))];
    tcp.read_exact(&mut body).await.unwrap();
    assert_eq!(Message::from_vec(&body).unwrap().answers.len(), 1);
    let next = query(zone, RecordType::SOA);
    tcp.write_all(&u16::try_from(next.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    tcp.write_all(&next).await.unwrap();
    tcp.read_exact(&mut length).await.unwrap();
    let mut body = vec![0; usize::from(u16::from_be_bytes(length))];
    tcp.read_exact(&mut body).await.unwrap();
    let answer = Message::from_vec(&body).unwrap();
    assert_eq!(answer.answers.len(), 1);
    assert!(matches!(answer.answers[0].data, RData::SOA(_)));

    let removed = provider_call(&socket, serde_json::json!({"action":"delete","id":id})).await;
    assert_eq!(removed["deleted"], true);
    let missing = provider_call(&socket, serde_json::json!({"action":"delete","id":id})).await;
    assert_eq!(missing["deleted"], false);
    udp.send_to(&query(zone, RecordType::TXT), dns_addr)
        .await
        .unwrap();
    let (size, _) = tokio::time::timeout(Duration::from_secs(2), udp.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert!(
        Message::from_vec(&buffer[..size])
            .unwrap()
            .answers
            .is_empty()
    );
    provider_call(
        &socket,
        serde_json::json!({"action":"append","zone":zone,"value":"token_456"}),
    )
    .await;
    let removed = provider_call(
        &socket,
        serde_json::json!({"action":"delete_exact","zone":zone,"value":"token_456"}),
    )
    .await;
    assert!(removed["error"].is_null());
    assert_eq!(removed["deleted"], true);
    let missing = provider_call(
        &socket,
        serde_json::json!({"action":"delete_exact","zone":zone,"value":"token_456"}),
    )
    .await;
    assert_eq!(missing["deleted"], false);
    let invalid = provider_call(
        &socket,
        serde_json::json!({"action":"append","zone":zone,"value":"bad token"}),
    )
    .await;
    assert_eq!(invalid["error"], "invalid");
    let invalid = provider_call(
        &socket,
        serde_json::json!({"action":"delete_exact","zone":zone,"value":"bad token"}),
    )
    .await;
    assert_eq!(invalid["error"], "invalid");
    let invalid = provider_call(&socket, serde_json::json!({"unknown":true})).await;
    assert_eq!(invalid["error"], "invalid");
    assert!(
        Message::from_vec(
            &authority
                .answer(&query(zone, RecordType::TXT), false)
                .unwrap()
        )
        .unwrap()
        .answers
        .is_empty()
    );
    let mut empty_request = UnixStream::connect(&socket).await.unwrap();
    empty_request.write_all(&[0, 0]).await.unwrap();
    assert_eq!(empty_request.read(&mut length).await.unwrap(), 0);
    let mut in_flight = UnixStream::connect(&socket).await.unwrap();
    let payload = serde_json::to_vec(
        &serde_json::json!({"action":"append","zone":zone,"value":"revoked-token"}),
    )
    .unwrap();
    in_flight
        .write_all(&u16::try_from(payload.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    tokio::task::yield_now().await;
    pid.store(0, Ordering::Release);
    let _ = in_flight.write_all(&payload).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), in_flight.read(&mut length))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(
        Message::from_vec(
            &authority
                .answer(&query(zone, RecordType::TXT), false)
                .unwrap()
        )
        .unwrap()
        .answers
        .is_empty()
    );
    let mut denied = UnixStream::connect(&socket).await.unwrap();
    denied.write_all(&[0, 2, b'{', b'}']).await.unwrap();
    assert_eq!(denied.read(&mut length).await.unwrap(), 0);

    shutdown_tx.send(true).unwrap();
    dns_task.await.unwrap().unwrap();
    provider_task.await.unwrap().unwrap();
    assert!(!socket.exists());
}
