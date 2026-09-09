use super::*;

#[test]
fn p1_snapshot_layout_rejects_malformed_bounds_and_remote_corruption() {
    std::thread::Builder::new()
        .name("p1-snapshot-invalid".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(p1_snapshot_layout_invalid_gate());
        })
        .unwrap()
        .join()
        .unwrap();
}
