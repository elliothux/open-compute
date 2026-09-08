use super::*;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

fn scratch_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // Unique top-level directory: shared parents under TMPDIR fail Gate cleanup.
    let dir = std::env::temp_dir().join(format!(
        "open-compute-instance-id-{}-{}",
        Uuid::now_v7().as_hyphenated(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn same_canonical_path_yields_same_id() {
    let dir = scratch_dir();
    let config = dir.join("compute.toml");
    fs::write(&config, "x = 1\n").unwrap();
    let canonical = dir.canonicalize().unwrap().join("compute.toml");
    let a = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let b = InstanceId::from_canonical_config_path(&canonical).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), INSTANCE_ID_MIN_LEN);
    assert_eq!(a.as_str().len(), INSTANCE_ID_MIN_LEN);
    parse_short_id(a.as_str()).unwrap();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn relative_and_parent_symlink_paths_share_id_after_canonicalize() {
    let dir = scratch_dir();
    let real = dir.join("real");
    fs::create_dir(&real).unwrap();
    let config = real.join("compute.toml");
    fs::write(&config, "x = 1\n").unwrap();
    let link_parent = dir.join("via-link");
    symlink(&real, &link_parent).unwrap();

    let via_link = link_parent.join("compute.toml");
    let parent = via_link.parent().unwrap().canonicalize().unwrap();
    let canonical = parent.join("compute.toml");
    let direct = real.canonicalize().unwrap().join("compute.toml");
    assert_eq!(canonical, direct);
    let a = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let b = InstanceId::from_canonical_config_path(&direct).unwrap();
    assert_eq!(a, b);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn moved_path_yields_new_id() {
    let dir = scratch_dir();
    let a_path = dir.canonicalize().unwrap().join("a.toml");
    let b_path = dir.canonicalize().unwrap().join("b.toml");
    fs::write(&a_path, "x = 1\n").unwrap();
    fs::write(&b_path, "x = 1\n").unwrap();
    let a = InstanceId::from_canonical_config_path(&a_path).unwrap();
    let b = InstanceId::from_canonical_config_path(&b_path).unwrap();
    assert_ne!(a.as_str(), b.as_str());
    assert_ne!(a.digest(), b.digest());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn content_change_does_not_change_id() {
    let dir = scratch_dir();
    let path = dir.canonicalize().unwrap().join("compute.toml");
    fs::write(&path, "x = 1\n").unwrap();
    let before = InstanceId::from_canonical_config_path(&path).unwrap();
    fs::write(&path, "x = 2\n").unwrap();
    let after = InstanceId::from_canonical_config_path(&path).unwrap();
    assert_eq!(before, after);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn extend_one_keeps_digest_and_lengthens_prefix() {
    let dir = scratch_dir();
    let path = dir.canonicalize().unwrap().join("compute.toml");
    fs::write(&path, "x = 1\n").unwrap();
    let base = InstanceId::from_canonical_config_path(&path).unwrap();
    let extended = base.extend_one().unwrap();
    assert_eq!(extended.digest(), base.digest());
    assert_eq!(extended.len(), base.len() + 1);
    assert!(extended.as_str().starts_with(base.as_str()));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn from_short_and_digest_rejects_mismatch() {
    let dir = scratch_dir();
    let path = dir.canonicalize().unwrap().join("compute.toml");
    fs::write(&path, "x = 1\n").unwrap();
    let id = InstanceId::from_canonical_config_path(&path).unwrap();
    let err = InstanceId::from_short_and_digest("00000", *id.digest()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceIdInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn selector_parses_exact_crockford_and_rejects_invalid() {
    let ok: InstanceSelector = "k7m2r".parse().unwrap();
    assert_eq!(ok.as_str(), "k7m2r");
    assert!("K7M2R".parse::<InstanceSelector>().is_err());
    assert!("ilou".parse::<InstanceSelector>().is_err());
    assert!("abcd".parse::<InstanceSelector>().is_err());
    assert!("not-ok".parse::<InstanceSelector>().is_err());
}

#[test]
fn relative_path_is_rejected() {
    let err = InstanceId::from_canonical_config_path(Path::new("compute.toml")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}

#[test]
fn from_digest_rejects_out_of_range_length() {
    let digest = [0u8; 32];
    assert_eq!(
        InstanceId::from_digest(digest, INSTANCE_ID_MIN_LEN - 1)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceIdInvalid
    );
    assert_eq!(
        InstanceId::from_digest(digest, INSTANCE_ID_MAX_LEN + 1)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceIdInvalid
    );
    let max = InstanceId::from_digest(digest, INSTANCE_ID_MAX_LEN).unwrap();
    assert_eq!(max.len(), INSTANCE_ID_MAX_LEN);
    assert!(!max.is_empty());
    assert!(max.extend_one().is_err());
}

#[test]
fn display_debug_and_serde_round_trips() {
    let dir = scratch_dir();
    let path = dir.canonicalize().unwrap().join("compute.toml");
    fs::write(&path, "x = 1\n").unwrap();
    let id = InstanceId::from_canonical_config_path(&path).unwrap();
    assert_eq!(format!("{id}"), id.as_str());
    assert!(format!("{id:?}").contains(id.as_str()));
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{}\"", id.as_str()));

    let selector: InstanceSelector = id.as_str().parse().unwrap();
    assert_eq!(format!("{selector}"), id.as_str());
    let selector_json = serde_json::to_string(&selector).unwrap();
    assert_eq!(selector_json, format!("\"{}\"", id.as_str()));
    let decoded: InstanceSelector = serde_json::from_str(&selector_json).unwrap();
    assert_eq!(decoded.as_str(), id.as_str());
    assert!(serde_json::from_str::<InstanceSelector>("\"abcd\"").is_err());
    let _ = fs::remove_dir_all(dir);
}
