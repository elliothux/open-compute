# P1 authority parser fuzz ownership

P1 的长时 fuzz rehearsal 使用独立的 `fuzz/` package、Rust 1.98.0、固定 seed
`6f70656e636f6d70`、64 KiB 单输入上限和受版本控制的 corpus。运行命令是：

```bash
./scripts/fuzz-p1.sh --seconds 60
```

| authority target | deterministic owner |
| --- | --- |
| canonical bundle | `fuzz/src/main.rs` + `crates/workers` property tests |
| binding descriptor | `fuzz/src/main.rs` + `crates/workers` descriptor tests |
| request metadata/header bridge | `runtime_bridge_tests.rs` |
| resource/deployment/cursor ID codec | `fuzz/src/main.rs` + typed-ID tests |
| facade RPC frame/structured value | `binding_backend_tests.rs` |
| KV cursor and metadata | `kv/engine_tests.rs` + `kv_backend_tests.rs` |
| D1 SQL authorizer and result encoder | `d1/tests.rs` + `d1_protocol_tests.rs` |
| R2/S3 object key builder | `r2/tests.rs` + artifact-store tests |
| snapshot manifest/path parser | `fuzz/src/main.rs` + snapshot/restore tests |
| migration/release metadata parser | `fuzz/src/main.rs` + migration tests |
| scheduler/DO internal envelope | `scheduler_tests.rs` + `runtime_bridge_tests.rs` + P0.8 stock-workerd Gate |

发现 panic、hang、OOM 或不变量失败时，先缩减输入，再把最小输入和确定性断言加入 owning crate 的普通
test；不得只把输入留在本机 corpus。
