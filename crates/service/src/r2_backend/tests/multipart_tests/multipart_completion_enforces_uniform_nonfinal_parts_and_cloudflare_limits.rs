use super::*;

#[test]
fn multipart_completion_enforces_uniform_nonfinal_parts_and_cloudflare_limits() {
    let requested = [
        open_compute_artifacts::R2UploadedPart {
            part_number: 1,
            etag: "one".to_owned(),
        },
        open_compute_artifacts::R2UploadedPart {
            part_number: 2,
            etag: "two".to_owned(),
        },
        open_compute_artifacts::R2UploadedPart {
            part_number: 3,
            etag: "three".to_owned(),
        },
    ];
    let base = open_compute_artifacts::R2_MIN_MULTIPART_PART_BYTES;
    let valid = [
        R2MultipartPartRecord {
            part_number: 1,
            etag: "one".to_owned(),
            size: base,
        },
        R2MultipartPartRecord {
            part_number: 2,
            etag: "two".to_owned(),
            size: base,
        },
        R2MultipartPartRecord {
            part_number: 3,
            etag: "three".to_owned(),
            size: 1,
        },
    ];
    multipart::validate_complete_parts(&requested, &valid).unwrap();

    let mut uneven = valid.clone();
    uneven[1].size += 1;
    assert_eq!(
        multipart::validate_complete_parts(&requested, &uneven)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    let one = [requested[0].clone()];
    let oversized = [R2MultipartPartRecord {
        part_number: 1,
        etag: "one".to_owned(),
        size: open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES + 1,
    }];
    assert_eq!(
        multipart::validate_complete_parts(&one, &oversized)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );

    assert_eq!(
        multipart::validate_complete_parts(&[], &[])
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    let duplicate = [requested[0].clone(), requested[0].clone()];
    assert_eq!(
        multipart::validate_complete_parts(&duplicate, &valid)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    assert_eq!(
        multipart::validate_complete_parts(&one, &[])
            .unwrap_err()
            .code(),
        ErrorCode::R2MultipartInvalid
    );

    let count = usize::try_from(
        open_compute_artifacts::R2_MAX_MULTIPART_OBJECT_BYTES
            / open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES
            + 1,
    )
    .unwrap();
    let requested = (1..=count)
        .map(|part_number| open_compute_artifacts::R2UploadedPart {
            part_number: i32::try_from(part_number).unwrap(),
            etag: part_number.to_string(),
        })
        .collect::<Vec<_>>();
    let stored = requested
        .iter()
        .map(|part| R2MultipartPartRecord {
            part_number: part.part_number,
            etag: part.etag.clone(),
            size: open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        multipart::validate_complete_parts(&requested, &stored)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
}
