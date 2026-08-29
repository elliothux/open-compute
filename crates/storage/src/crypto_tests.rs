use super::*;
use crate::master_key;

#[test]
fn aead_roundtrip_nonce_context_tamper_key() {
    let key = SecretBytes::new(vec![7u8; 32]);
    let fp = master_key::fingerprint_for_test(key.expose());
    let crypto = SecretCrypto::new(&key, &fp).unwrap();
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let deployment = DeploymentId::generate();
    let pt = SecretBytes::new(b"super-secret-value".to_vec());
    let e1 = crypto
        .encrypt(&pt, account, worker, deployment, "BINDING", "revision-1")
        .unwrap();
    let e2 = crypto
        .encrypt(&pt, account, worker, deployment, "BINDING", "revision-1")
        .unwrap();
    assert_ne!(e1.nonce, e2.nonce);
    let back = crypto
        .decrypt(&e1, account, worker, deployment, "BINDING", "revision-1")
        .unwrap();
    assert_eq!(back.expose(), b"super-secret-value");
    for (bound_account, bound_worker, bound_deployment, name, revision) in [
        (
            AccountId::generate(),
            worker,
            deployment,
            "BINDING",
            "revision-1",
        ),
        (
            account,
            WorkerId::generate(),
            deployment,
            "BINDING",
            "revision-1",
        ),
        (
            account,
            worker,
            DeploymentId::generate(),
            "BINDING",
            "revision-1",
        ),
        (account, worker, deployment, "OTHER", "revision-1"),
        (account, worker, deployment, "BINDING", "revision-2"),
        (account, worker, deployment, "BINDINGrevision-", "1"),
    ] {
        assert!(
            crypto
                .decrypt(
                    &e1,
                    bound_account,
                    bound_worker,
                    bound_deployment,
                    name,
                    revision
                )
                .is_err()
        );
    }
    let mut tampered = e1.clone();
    tampered.ciphertext[0] ^= 0xff;
    assert!(
        crypto
            .decrypt(
                &tampered,
                account,
                worker,
                deployment,
                "BINDING",
                "revision-1"
            )
            .is_err()
    );
    let other_key = SecretBytes::new(vec![9u8; 32]);
    let other_fp = master_key::fingerprint_for_test(other_key.expose());
    let other = SecretCrypto::new(&other_key, &other_fp).unwrap();
    assert_eq!(
        other
            .decrypt(&e1, account, worker, deployment, "BINDING", "revision-1")
            .unwrap_err()
            .code(),
        ErrorCode::MasterKeyMismatch
    );
    let same_id_wrong_key = SecretCrypto::new(&other_key, &fp).unwrap();
    assert!(
        same_id_wrong_key
            .decrypt(&e1, account, worker, deployment, "BINDING", "revision-1")
            .is_err()
    );
    let mut wrong_nonce = e1;
    wrong_nonce.nonce[0] ^= 1;
    assert!(
        crypto
            .decrypt(
                &wrong_nonce,
                account,
                worker,
                deployment,
                "BINDING",
                "revision-1"
            )
            .is_err()
    );
}

#[test]
fn aead_rejects_empty_overlong_name_and_invalid_key_id() {
    let key = SecretBytes::new(vec![7u8; 32]);
    let fp = master_key::fingerprint_for_test(key.expose());
    let crypto = SecretCrypto::new(&key, &fp).unwrap();
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let deployment = DeploymentId::generate();
    let pt = SecretBytes::new(b"x".to_vec());
    let err = crypto
        .encrypt(&pt, account, worker, deployment, "", "revision-1")
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    let long = "a".repeat(4097);
    let err = crypto
        .encrypt(&pt, account, worker, deployment, &long, "revision-1")
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    assert!(SecretCrypto::new(&key, "deadbeef").is_err());
    assert!(SecretCrypto::new(&key, &"A".repeat(64)).is_err());
}

#[test]
fn aead_revision_and_envelope_validation_matrix() {
    let key = SecretBytes::new(vec![3_u8; 32]);
    let fingerprint = master_key::fingerprint_for_test(key.expose());
    let crypto = SecretCrypto::new(&key, &fingerprint).unwrap();
    assert_eq!(
        SecretCrypto::new(&SecretBytes::new(vec![0_u8; 31]), &fingerprint)
            .unwrap_err()
            .code(),
        ErrorCode::MasterKeyMismatch
    );
    assert_eq!(crypto.key_id(), fingerprint);
    assert_eq!(crypto.fingerprint_key_id().len(), 64);
    assert_ne!(
        crypto.fingerprint_request(b"one"),
        crypto.fingerprint_request(b"two")
    );
    assert!(format!("{crypto:?}").contains("XCHACHA20-POLY1305"));

    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let deployment = DeploymentId::generate();
    let plaintext = SecretBytes::new(b"revision-secret".to_vec());
    let envelope = crypto
        .encrypt(
            &plaintext,
            account,
            worker,
            deployment,
            "TOKEN",
            "revision-1",
        )
        .unwrap();
    assert_eq!(
        crypto
            .decrypt(
                &envelope,
                account,
                worker,
                deployment,
                "TOKEN",
                "revision-1",
            )
            .unwrap()
            .expose(),
        b"revision-secret"
    );
    assert!(
        crypto
            .decrypt(
                &envelope,
                account,
                worker,
                deployment,
                "TOKEN",
                "revision-2",
            )
            .is_err()
    );
    for revision in ["", &"r".repeat(4097)] {
        assert_eq!(
            crypto
                .encrypt(&plaintext, account, worker, deployment, "TOKEN", revision,)
                .unwrap_err()
                .code(),
            ErrorCode::SecretInvalid
        );
    }

    for mutate in [
        |value: &mut SecretEnvelope| value.version = 2,
        |value: &mut SecretEnvelope| value.algorithm = "OTHER".to_owned(),
        |value: &mut SecretEnvelope| value.nonce.clear(),
    ] {
        let mut invalid = envelope.clone();
        mutate(&mut invalid);
        assert!(
            crypto
                .decrypt(&invalid, account, worker, deployment, "TOKEN", "revision-1",)
                .is_err()
        );
    }
    let other_key = SecretBytes::new(vec![4_u8; 32]);
    let other_fingerprint = master_key::fingerprint_for_test(other_key.expose());
    let other = SecretCrypto::new(&other_key, &other_fingerprint).unwrap();
    assert_eq!(
        other
            .decrypt(
                &envelope,
                account,
                worker,
                deployment,
                "TOKEN",
                "revision-1",
            )
            .unwrap_err()
            .code(),
        ErrorCode::MasterKeyMismatch
    );
}
