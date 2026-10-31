use super::*;

#[test]
fn compiled_config_accessors_and_corruption_matrix() {
    fn make(dir: &Path, name: &str) -> CompiledConfig {
        CompiledConfig::from_bytes_for_test(&dir.join(name), &"ab".repeat(32), b"compiled").unwrap()
    }

    let dir = TempDir::new().unwrap();
    let valid = make(dir.path(), "valid");
    assert_eq!(valid.digest(), "ab".repeat(32));
    assert!(valid.path().is_absolute());
    assert_eq!(valid.read_bytes().unwrap(), b"compiled");
    assert!(format!("{valid:?}").contains(valid.digest()));
    assert!(valid.to_string().contains(valid.digest()));

    for (name, sidecar_bytes) in [
        ("empty", Vec::new()),
        ("oversized", vec![b'x'; 257]),
        ("non-utf8", vec![0xff, 0xfe]),
        ("wrong-digest", b"wrong\nwrong\n".to_vec()),
        (
            "wrong-content",
            format!("{}\n{}\n", "ab".repeat(32), "cd".repeat(32)).into_bytes(),
        ),
    ] {
        let compiled = make(dir.path(), name);
        let sidecar = compiled.path().with_extension("bin.digest");
        fs::write(&sidecar, sidecar_bytes).unwrap();
        assert!(compiled.open().is_err(), "corrupt sidecar accepted: {name}");
    }

    let empty = make(dir.path(), "empty-config");
    fs::write(empty.path(), b"").unwrap();
    assert!(empty.open().is_err());
}
