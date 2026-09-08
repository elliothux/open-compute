use super::*;

pub(super) fn run() {
    std::thread::Builder::new()
        .name("p1-snapshot-restore".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(snapshot_restore_gate());
        })
        .expect("P1 Gate thread")
        .join()
        .expect("P1 Gate thread result");
}
