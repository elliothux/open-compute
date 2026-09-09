use super::*;

mod cancelled_and_timed_out_part_streams_release_admission_without_publishing;

fn part_frame(
    key: &str,
    upload_id: &str,
    part_number: i32,
    bytes: &[u8],
    ssec_key: Option<&str>,
) -> Body {
    let header = serde_json::to_vec(&serde_json::json!({
        "key": key,
        "uploadId": upload_id,
        "partNumber": part_number,
        "ssecKey": ssec_key,
    }))
    .unwrap();
    let mut frame = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(&header);
    frame.extend_from_slice(bytes);
    Body::from(frame)
}

mod private_protocol_covers_checksum_ssec_storage_class_multipart_and_start_after;

mod multipart_completion_enforces_uniform_nonfinal_parts_and_cloudflare_limits;

mod startup_reconciles_committed_completion_and_provider_backed_initiating;

mod multipart_create_response_loss_is_durable_and_restart_cleanup_is_scoped;

mod startup_reconciliation_pairs_every_unknown_create_without_guessing_provider_identity;
