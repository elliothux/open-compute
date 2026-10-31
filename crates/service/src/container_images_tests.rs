use super::*;
use crate::runtime_feature_fixture::RuntimeFeatureFixture;
use bytes::Bytes;
use serde_json::json;

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn token(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(hash(bytes)))
}
fn limits() -> ContainerImageLimits {
    ContainerImageLimits {
        json_bytes: 8192,
        compressed_bytes: 8192,
        unpacked_bytes: 8192,
        layers: 8,
        entries: 8,
    }
}

async fn graph(fixture: &RuntimeFeatureFixture) -> (Vec<u8>, Vec<ArtifactRef>) {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(4);
    header.set_mode(0o755);
    builder
        .append_data(&mut header, "app", b"main".as_slice())
        .unwrap();
    let layer = builder.into_inner().unwrap();
    let config = serde_json::to_vec(&json!({"os":"linux","architecture":"arm64","rootfs":{"type":"layers","diff_ids":[token(&layer)]}})).unwrap();
    let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":token(&config),"size":config.len()},"layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":token(&layer),"size":layer.len()}]})).unwrap();
    let mut refs = Vec::new();
    for bytes in [config, layer] {
        let digest = hex::encode(hash(&bytes));
        let size = bytes.len() as u64;
        refs.push(
            fixture
                .artifacts
                .put_verified(
                    futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from(bytes)) }),
                    &digest,
                    size,
                )
                .await
                .unwrap(),
        );
    }
    (manifest, refs)
}

#[tokio::test]
async fn complete_image_admission_publishes_one_archive_and_commits_its_gc_reference() {
    let fixture = RuntimeFeatureFixture::create(open_compute_workers::VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".to_owned(),
        ..Default::default()
    })
    .await;
    let (manifest, blobs) = graph(&fixture).await;
    let expected = hash(&manifest);
    let metadata = admit_image(
        &fixture.storage,
        &fixture.artifacts,
        &fixture.artifact_cache,
        ContainerImageUpload {
            manifest: &manifest,
            expected,
            architecture: "arm64",
            blobs: blobs.clone(),
            limits: limits(),
        },
    )
    .await
    .unwrap();
    let repo = ContainerAuthority::new(fixture.storage.db());
    assert_eq!(repo.image(fixture.account, &expected).unwrap(), metadata);
    let artifact = ArtifactRef::new(
        1,
        &hex::encode(metadata.artifact_sha256),
        metadata.artifact_size,
    )
    .unwrap();
    let pin = fixture
        .artifact_cache
        .acquire(&fixture.artifacts, &artifact)
        .await
        .unwrap();
    let image = open_compute_artifacts::verify_container_image_archive(
        pin.file(),
        &artifact,
        expected,
        "arm64",
        limits(),
    )
    .unwrap();
    assert_eq!(image.config().sha256, metadata.config_sha256);
    assert_eq!(image.diff_ids(), metadata.diff_ids);
    assert!(
        open_compute_storage::worker_repository::WorkerRepository::new(fixture.storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(metadata.artifact_sha256, metadata.artifact_size))
    );
    assert_eq!(
        std::fs::read_dir(fixture.storage.data_dir().prepare_tmp_dir().unwrap())
            .unwrap()
            .count(),
        0
    );
    let repeated = admit_image(
        &fixture.storage,
        &fixture.artifacts,
        &fixture.artifact_cache,
        ContainerImageUpload {
            manifest: &manifest,
            expected,
            architecture: "arm64",
            blobs,
            limits: limits(),
        },
    )
    .await
    .unwrap();
    assert_eq!(repeated.artifact_sha256, metadata.artifact_sha256);
    assert_eq!(repeated.artifact_size, metadata.artifact_size);
    assert_eq!(repeated.diff_ids, metadata.diff_ids);
}

#[tokio::test]
async fn incomplete_or_unreferenced_graphs_leave_no_image_authority_or_spool() {
    let fixture = RuntimeFeatureFixture::create(open_compute_workers::VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".to_owned(),
        ..Default::default()
    })
    .await;
    let (manifest, original) = graph(&fixture).await;
    let expected = hash(&manifest);
    let extra_bytes = Bytes::from_static(b"distinct valid blob outside the manifest graph");
    let extra_digest = hex::encode(hash(&extra_bytes));
    let extra_size = extra_bytes.len() as u64;
    let extra = fixture
        .artifacts
        .put_verified(
            futures::stream::once(async { Ok::<_, std::io::Error>(extra_bytes) }),
            &extra_digest,
            extra_size,
        )
        .await
        .unwrap();
    for mutation in 0..7 {
        let mut blobs = original.clone();
        let mut wanted = expected;
        let mut budget = limits();
        match mutation {
            0 => {
                blobs.pop();
            }
            1 => blobs.push(blobs[0].clone()),
            2 => wanted = [0; 32],
            3 => blobs[0] = ArtifactRef::new(1, &"a".repeat(64), 2).unwrap(),
            4 => {
                blobs.extend(std::iter::repeat_n(blobs[0].clone(), 8));
            }
            5 => blobs.push(extra.clone()),
            _ => budget.json_bytes = (manifest.len() - 1) as u64,
        }
        let error = admit_image(
            &fixture.storage,
            &fixture.artifacts,
            &fixture.artifact_cache,
            ContainerImageUpload {
                manifest: &manifest,
                expected: wanted,
                architecture: "arm64",
                blobs,
                limits: budget,
            },
        )
        .await
        .unwrap_err();
        if mutation == 6 {
            assert_eq!(error.code(), ErrorCode::ObjectStorageIntegrityError);
        }
        assert!(
            ContainerAuthority::new(fixture.storage.db())
                .image(fixture.account, &expected)
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(fixture.storage.data_dir().prepare_tmp_dir().unwrap())
                .unwrap()
                .count(),
            0
        );
    }
    let recovered = admit_image(
        &fixture.storage,
        &fixture.artifacts,
        &fixture.artifact_cache,
        ContainerImageUpload {
            manifest: &manifest,
            expected,
            architecture: "arm64",
            blobs: original,
            limits: limits(),
        },
    )
    .await
    .unwrap();
    assert_eq!(recovered.manifest_sha256, expected);
    assert_eq!(
        ContainerAuthority::new(fixture.storage.db())
            .image(fixture.account, &expected)
            .unwrap(),
        recovered
    );
}
