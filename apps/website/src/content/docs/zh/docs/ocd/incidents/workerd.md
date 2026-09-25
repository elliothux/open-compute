---
title: "workerd 崩溃循环"
---

触发信号：restart counter 持续增长、readiness runtime unavailable、activation 或 WebSocket 大量失败。影响面是 tenant execution；ocd control plane 仍应存活。

只读诊断：

```sh
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml doctor --json
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml support-bundle --output /tmp/open-compute-support.tar
```

检查 bundle 内的 `deployment-runtime.json` 与 `workerd-last-exit.json`。后者只保留最近一次 bounded、redacted 的 stdout/stderr tail、exit code/signal、精确的已退出 startup generation、restart reason、digest 与 deployment attribution class。`deployment_quarantined` 表示一个在途 active deployment 已被识别并回退；`attribution_ambiguous` 或 `unattributed` 需要关联受影响请求，不得手改 SQLite。

允许的 mutation 是停止 service、恢复同一 release package 中的 verified workerd/runtime assets，再启动；不得 PATH 搜索、自动下载或扩大 abort allowlist。只替换并校验完整 `ocd`，不单独替换缓存中的 workerd 或 JS。

预期 supervisor bounded backoff、reap 旧 process group、旧 generation token 失效，并永久 quarantine 能精确归因的 deployment。停止条件是 digest/version 不匹配、未知 orphan identity 或 localDisk compatibility 未通过。回滚是恢复完整旧 package 加其 snapshot，而不是单换 binary。验证是 doctor full、当前 runtime Gate、DO/alarms/basic WebSocket 和无 orphan/FD 泄漏。
