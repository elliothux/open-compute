# workerd 崩溃循环

触发信号：restart counter 持续增长、readiness runtime unavailable、activation 或 WebSocket 大量失败。影响面是 tenant execution；platformd control plane 仍应存活。

只读诊断：

```bash
/opt/open-compute/platformd --config /etc/open-compute/platform.toml capabilities --json
/opt/open-compute/platformd --config /etc/open-compute/platform.toml doctor --json
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --json
```

允许的 mutation 是停止 service、恢复同一 release package 中的 verified workerd/runtime assets，再启动；不得 PATH 搜索、自动下载或扩大 abort allowlist。预期 supervisor bounded backoff、reap 旧 process group、旧 generation token 失效。停止条件是 digest/version 不匹配、未知 orphan identity 或 localDisk compatibility 未通过。回滚是恢复完整旧 package 加其 snapshot，而不是单换 binary。验证是 doctor full、G0/P0 runtime smoke、DO/alarms/basic WebSocket 和无 orphan/FD 泄漏。
