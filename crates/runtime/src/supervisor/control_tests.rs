use super::*;

#[test]
fn accepts_loopback_listen() {
    let mut p = ControlParser::new();
    p.push(br#"{"event":"listen","socket":"http","port":43123}"#)
        .unwrap();
    p.push(b"\n").unwrap();
    assert_eq!(p.listen().unwrap().port, 43123);
}

#[test]
fn rejects_malformed_duplicate_and_unknown_fields() {
    let mut p = ControlParser::new();
    assert!(p.push(b"not-json\n").is_err());
    let mut p = ControlParser::new();
    p.push(b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9}\n")
        .unwrap();
    assert!(
        p.push(b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9}\n")
            .is_err()
    );
    let mut p = ControlParser::new();
    assert!(
        p.push(b"{\"event\":\"listen\",\"socket\":\"https\",\"port\":9}\n")
            .is_err()
    );
    let mut p = ControlParser::new();
    assert!(
        p.push(b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9,\"address\":\"1.2.3.4\"}\n")
            .is_err()
    );
    let mut p = ControlParser::new();
    assert!(
        p.push(b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9,\"extra\":1}\n")
            .is_err()
    );
    let mut p = ControlParser::new();
    let huge = vec![b'x'; MAX_LINE + 2];
    assert!(p.push(&huge).is_err());
    let mut p = ControlParser::new();
    assert!(
        p.push(
            b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9,\"address\":\"not-an-addr\"}\n"
        )
        .is_err()
    );
    let mut p = ControlParser::new();
    assert!(
        p.push(
            b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9,\"address\":\"127.0.0.1\"}\n"
        )
        .is_err()
    );
    let mut p = ControlParser::new();
    assert!(
        p.push(
            b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9,\"address\":\"127.0.0.1:8\"}\n"
        )
        .is_err()
    );
    let mut p = ControlParser::new();
    p.push(b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9,\"address\":\"127.0.0.1:9\"}\n")
        .unwrap();
    assert_eq!(p.listen().unwrap().port, 9);
}

#[test]
fn lifetime_bound_counts_empty_lines_and_accepts_fragmented_listen() {
    let mut p = ControlParser::new();
    let chunk = b"\n";
    let mut exceeded = false;
    for _ in 0..(MAX_TOTAL + 8) {
        if p.push(chunk).is_err() {
            exceeded = true;
            break;
        }
    }
    assert!(exceeded, "empty lines must consume the lifetime bound");

    let mut p = ControlParser::new();
    let msg = br#"{"event":"listen","socket":"http","port":43123}"#;
    for piece in msg.chunks(3) {
        p.push(piece).unwrap();
    }
    p.push(b"\n").unwrap();
    assert_eq!(p.listen().unwrap().port, 43123);
}

#[test]
fn crlf_empty_event_and_port_boundaries_are_rejected_or_accepted_exactly() {
    let mut parser = ControlParser::new();
    parser.push(b"\r\n").unwrap();
    assert!(!parser.accepted());
    parser
        .push(b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":9}\r\n")
        .unwrap();
    assert!(parser.accepted());

    for line in [
        b"{\"event\":\"ready\",\"socket\":\"http\",\"port\":9}\n".as_slice(),
        b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":0}\n".as_slice(),
        b"{\"event\":\"listen\",\"socket\":\"http\",\"port\":65536}\n".as_slice(),
    ] {
        let mut parser = ControlParser::new();
        assert!(parser.push(line).is_err());
    }
}
