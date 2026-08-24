use super::*;
use open_compute_core::Redactor;

#[test]
fn redacts_token_and_headers_and_bounds_lines() {
    let mut r = Redactor::new();
    r.register_str("sekrit-token");
    let c = LogCollector::new(r);
    c.ingest(b"token=sekrit-token\n");
    c.ingest(b"Authorization: Bearer abc\n");
    c.ingest(&vec![b'a'; MAX_LINE + 8]);
    let s = c.snapshot().as_lossy_str();
    assert!(!s.contains("sekrit-token"));
    assert!(s.contains("[REDACTED]"));
    assert!(!s.contains("Bearer abc"));
}

#[test]
fn pipe_reader_handles_interrupts_errors_and_pending_line_bounds() {
    struct InterruptedThenData {
        state: u8,
    }
    impl Read for InterruptedThenData {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.state += 1;
            match self.state {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => {
                    buf[..5].copy_from_slice(b"line\n");
                    Ok(5)
                }
                _ => Ok(0),
            }
        }
    }
    struct AlwaysFails;
    impl Read for AlwaysFails {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("failed"))
        }
    }
    let collector = LogCollector::new(Redactor::new());
    read_pipe_into(InterruptedThenData { state: 0 }, &collector).unwrap();
    assert!(collector.snapshot().as_lossy_str().contains("line"));
    assert!(read_pipe_into(AlwaysFails, &collector).is_err());

    let long_with_newline = [vec![b'x'; MAX_LINE + 1], b"\n".to_vec()].concat();
    read_pipe_into(long_with_newline.as_slice(), &collector).unwrap();
    let long_pending = vec![b'y'; MAX_LINE + 1];
    read_pipe_into(long_pending.as_slice(), &collector).unwrap();
    read_pipe_into(b"pending".as_slice(), &collector).unwrap();
    assert!(
        collector
            .snapshot()
            .as_lossy_str()
            .contains("<truncated-line>")
    );
}
