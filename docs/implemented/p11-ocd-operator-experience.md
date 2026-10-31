# P11：`ocd` 安装、实例与本机运维

状态：**implemented / Implementation GO（2026-09-08）**。

## 用户结果

- 单文件 `ocd` 同时提供前台 daemon 与本机管理 CLI，不增加 manager daemon 或 self-daemonize 路径。
- 配置按显式 `--config`、当前目录 `compute.toml` 和 system config 确定性发现；高优先级配置损坏时不 fallback。
- 配置 canonical path 派生稳定 Instance ID；registry 和 control socket 支持列出、选择、启动、停止、重启和查看本机实例。
- systemd／launchd adapter 管理后台生命周期；data-dir flock、listener bind 和 daemon authority 仍是最终冲突边界。
- `ocd setup` 安全生成配置与 secret，并在失败时 rollback；`ocd dashboard` 使用一次性 code 换取短期 browser session。
- `scripts/install.sh`、install receipt、`ocd upgrade` 和 `ocd uninstall` 组成分发生命周期；不删除 operator 的配置、secret 或数据。
- Dashboard 只显示正式版本检查结果和主机 CLI 指引；升级执行只在 `ocd upgrade`，不建立持久 update job。
- 普通管理 CLI 可异步刷新有界 release metadata cache；daemon startup、`--help`、`--version` 和 `--no-update-check` 不联网。

target registry 与安全边界见 [P12](p12-wrangler-project-workflow.md)；当前 Worker 项目、CLI 与构建入口由
[P20 cf 工作流](p20-cf-cli-migration.md) 拥有，不在 P11 复制。

## 历史验证与限制

本地 selector、registry、fake service manager、setup rollback、control socket／HTTP readiness、Dashboard login、upgrade fixture、
update-check 和 uninstall 均通过。Coverage 为 90.0047%；最终 workspace Gate 为 49 targets，报告位于
`.temp/gate-run/20260908T042240-09ec8be2/report.json`。

Windows service、自动后台更新和跨机器 registry 是非目标。
