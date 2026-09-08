# P11 运维体验正式资格

状态：active，2026-09-08。核心实现、本地 90.0047% coverage 和最终 workspace Gate 见
[P11 运维体验](../implemented/p11-ocd-operator-experience.md)。本文只保留正式 Release 与真实 OS service 证据。

## 固定输入

使用不可变 GitHub Release 的 darwin-arm64、linux-arm64、linux-x64 assets、匹配的
`scripts/install.sh`、`SHA256SUMS` 和当前 install receipt schema。真实 systemd/launchd 操作只在隔离 runner 上执行。

## 剩余 Gate

- [ ] 三个目标从公开 Release 安装，校验 SHA-256、`ocd --version` 和不含 secret 的 receipt；安装器不创建配置、data-dir、token 或 service。
- [ ] Linux systemd 与 macOS launchd 分别验证 enable/load、启动、ready、restart、stop、失败和日志路径。
- [ ] 干净主机运行 `ocd setup --yes`，在期限内 ready；失败不留下半配置，成功后 Dashboard login code 可兑换。
- [ ] 同一主机两个隔离实例并行 ready，selector、restart 和 stop 指向正确实例。

FakeServiceManager 和本地单测不替代上述证据。下载正式 Release、`sudo`、systemd/launchd 写入均需授权。
完成后把 runner、命令、结果和限制并入 P11 implemented 文档并删除本文；发现实现缺口时恢复活动方案。
