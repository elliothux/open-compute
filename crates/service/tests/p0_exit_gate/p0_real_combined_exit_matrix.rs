use super::*;

pub(super) fn run() {
    std::thread::Builder::new()
        .name("p0-combined-exit".to_owned())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("P0 runtime")
                .block_on(p0_real_combined_exit_matrix_inner());
        })
        .expect("P0 Gate thread")
        .join()
        .expect("P0 Gate thread result");
}
