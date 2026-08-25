use super::*;
use open_compute_storage::{D1Meta, D1StatementResult};

#[test]
fn query_frame_round_trips_binary_values_without_json_or_base64() {
    let mut writer = Writer::new();
    writer.bytes(b"D1Q1").unwrap();
    writer.u8(4).unwrap();
    writer.u16(1).unwrap();
    writer.text("SELECT ?1, ?2, ?3, ?4, ?5").unwrap();
    writer.u16(5).unwrap();
    for value in [
        D1Value::Null,
        D1Value::Integer(42),
        D1Value::Real(1.5),
        D1Value::Text("text".to_owned()),
        D1Value::Blob(vec![0, 1, 2, 255]),
    ] {
        writer.value(&value).unwrap();
    }
    let request = decode_query(&writer.finish()).unwrap();
    assert_eq!(request.mode, D1QueryMode::Batch);
    assert_eq!(
        request.statements[0].params[4],
        D1Value::Blob(vec![0, 1, 2, 255])
    );
}

#[test]
fn malformed_trailing_and_oversized_frames_fail_closed() {
    let mut frame = b"D1E1\0\0\0\x08SELECT 1".to_vec();
    assert_eq!(decode_exec(&frame).unwrap(), "SELECT 1");
    frame.push(0);
    assert_eq!(
        decode_exec(&frame).unwrap_err().code(),
        ErrorCode::D1InternalProtocolError
    );
    assert_eq!(
        decode_exec(&vec![0; D1_MAX_FRAME_BYTES + 1])
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError,
    );
    assert_eq!(
        decode_query(&vec![0; D1_MAX_FRAME_BYTES + 1])
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError,
    );
    for frame in [
        b"bad!\x01\x00\x01".to_vec(),
        b"D1Q1\xff\x00\x01".to_vec(),
        b"D1Q1\x01\x00\x00".to_vec(),
        b"D1Q1\x01\x00\x02".to_vec(),
    ] {
        assert_eq!(
            decode_query(&frame).unwrap_err().code(),
            ErrorCode::D1InternalProtocolError
        );
    }
    let mut too_many_params = Writer::new();
    too_many_params.bytes(b"D1Q1").unwrap();
    too_many_params.u8(1).unwrap();
    too_many_params.u16(1).unwrap();
    too_many_params.text("SELECT 1").unwrap();
    too_many_params
        .u16(u16::try_from(D1_MAX_BOUND_PARAMS + 1).unwrap())
        .unwrap();
    assert_eq!(
        decode_query(&too_many_params.finish()).unwrap_err().code(),
        ErrorCode::D1LimitError
    );
    let mut nonfinite = Writer::new();
    nonfinite.bytes(b"D1Q1").unwrap();
    nonfinite.u8(1).unwrap();
    nonfinite.u16(1).unwrap();
    nonfinite.text("SELECT ?1").unwrap();
    nonfinite.u16(1).unwrap();
    nonfinite.u8(2).unwrap();
    nonfinite.f64(f64::NAN).unwrap();
    assert_eq!(
        decode_query(&nonfinite.finish()).unwrap_err().code(),
        ErrorCode::D1TypeError
    );

    assert_eq!(
        decode_exec(b"D1Q1\0\0\0\0").unwrap_err().code(),
        ErrorCode::D1InternalProtocolError
    );
    let mut oversized_text = b"D1E1".to_vec();
    oversized_text.extend_from_slice(&u32::try_from(D1_MAX_SQL_BYTES + 1).unwrap().to_be_bytes());
    assert_eq!(
        decode_exec(&oversized_text).unwrap_err().code(),
        ErrorCode::D1LimitError
    );
    let mut unknown_value = Writer::new();
    unknown_value.bytes(b"D1Q1").unwrap();
    unknown_value.u8(1).unwrap();
    unknown_value.u16(1).unwrap();
    unknown_value.text("SELECT ?1").unwrap();
    unknown_value.u16(1).unwrap();
    unknown_value.u8(255).unwrap();
    assert_eq!(
        decode_query(&unknown_value.finish()).unwrap_err().code(),
        ErrorCode::D1InternalProtocolError
    );

    let mut full_writer = Writer {
        bytes: vec![0; D1_MAX_FRAME_BYTES],
    };
    assert_eq!(
        full_writer.u8(0).unwrap_err().code(),
        ErrorCode::D1LimitError
    );
}

#[test]
fn result_frame_keeps_duplicate_columns_and_blob_bytes() {
    let result = D1StatementResult {
        columns: vec![
            "duplicate".to_owned(),
            "duplicate".to_owned(),
            "__proto__".to_owned(),
        ],
        rows: vec![vec![
            D1Value::Integer(1),
            D1Value::Integer(2),
            D1Value::Blob(vec![7, 8, 9]),
        ]],
        meta: D1Meta {
            served_by: "open-compute-local".to_owned(),
            served_by_primary: true,
            duration: 1.0,
            changes: 0,
            last_row_id: 0,
            changed_db: false,
            size_after: 4096,
            rows_read: 1,
            rows_written: 0,
        },
    };
    let frame = encode_results(&[result]).unwrap();
    assert_eq!(&frame[..4], b"D1R1");
    assert!(frame.windows(3).any(|window| window == [7, 8, 9]));
    assert!(!String::from_utf8_lossy(&frame).contains("BwgJ"));

    let invalid_row = D1StatementResult {
        columns: vec!["one".to_owned()],
        rows: vec![vec![]],
        meta: D1Meta {
            served_by: "open-compute-local".to_owned(),
            served_by_primary: true,
            duration: 0.0,
            changes: 0,
            last_row_id: 0,
            changed_db: false,
            size_after: 0,
            rows_read: 0,
            rows_written: 0,
        },
    };
    assert_eq!(
        encode_results(&[invalid_row]).unwrap_err().code(),
        ErrorCode::D1InternalProtocolError
    );
    let mut writer = Writer::new();
    assert_eq!(
        writer
            .value(&D1Value::Real(f64::INFINITY))
            .unwrap_err()
            .code(),
        ErrorCode::D1InternalProtocolError
    );
}
