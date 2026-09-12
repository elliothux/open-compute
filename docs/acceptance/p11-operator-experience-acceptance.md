# P11 运维体验正式资格

状态：active，2026-09-08。核心实现、本地 90.0047% coverage 和最终 workspace Gate 见
[P11 运维体验](../implemented/p11-ocd-operator-experience.md)。本文只保留正式 Release 与真实 OS service 证据。

## 固定输入

使用不可变 GitHub Release 的 darwin-arm64、linux-arm64、linux-x64 assets、匹配的
`scripts/install.sh`、`SHA256SUMS` 和当前 install receipt schema。真实 systemd/launchd 操作只在隔离 runner 上执行。

## 剩余 Gate

- [ ] 三个目标以普通用户从公开 Release 安装到 `$HOME/.local`，校验 SHA-256、`ocd --version`、PATH 配置和不含 secret 的 receipt；安装器不创建 config、data、token 或 service；root 安装独立验证 `/usr/local`。
- [ ] Linux systemd user/system 与 macOS LaunchAgent/LaunchDaemon 分别验证 enable/load、当前会话启动、登录或 boot 后启动、ready、restart、stop、失败和日志路径。
- [ ] 干净主机运行 `ocd setup --yes`，在期限内 ready；root 未传 `--system` 被拒绝；失败不留下半配置，成功后 Dashboard login code 可兑换。
- [ ] 同一主机两个隔离实例并行 ready，selector、restart 和 stop 指向正确实例。
- [ ] `uninstall` 对 user/system 两种 scope 停止并注销 owned instance、保留且打印本地路径；另一个 executable 的 registration 不受影响。
- [ ] `purge` 的 Local、外部 Local、S3 retained、overlap、symlink、active socket、partial failure、dry-run 与重复调用在隔离主机验证。

FakeServiceManager 和本地单测不替代上述证据。下载正式 Release、`sudo`、systemd/launchd 写入均需授权。
完成后把 runner、命令、结果和限制并入 P11 implemented 文档并删除本文；发现实现缺口时恢复活动方案。
