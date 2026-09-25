---
title: "ocd CLI"
description: "按任务查询 open-compute daemon、开发 launcher、诊断和维护命令。"
---

`ocd --help` 和各级 subcommand 的 `--help` 是参数 authority。本页只按任务分组并说明共同语义。

## 安装与运行

- `setup`、`run`
- `start`、`stop`、`restart`、`status`、`logs`
- `instances`、`instance setup|add|start|stop|restart|remove`、`purge`
- `dashboard`

`ocd run` 启动选定的 OCD 作用域：默认当前用户，或显式 `ocd run --system`。它只读取该作用域的 `ocd.toml`，不接受 `--config` 或 `--instance`。`ocd start|stop|restart|status|logs|instances` 操作该作用域唯一的 daemon 服务，并拒绝实例选择器。

`ocd caddy version|list-modules|fmt|validate|reload|status` 也只操作选定的 OCD 作用域，拒绝 `--config` 和 `--instance`；它管理的是共享 Gateway，而非某个实例的 Gateway。`reload` 通过运行中的作用域 daemon 原子应用完整 Caddy 配置。

受管 registration 只来自 `<OCD_DIR>/ocd.toml`。每个 entry 只保存 `config` 与 `autostart`；运行时不扫描 `instances/`，精确配置中的 `[data].path` 是唯一数据根权威。

实例范围命令用 `--instance` 选择已登记实例，或读取显式 `--config` 指定的精确文件；两者均未提供时，仅在所选 OCD 作用域恰好登记一个实例时自动选择。零个或多个实例须明确选择；CLI 不从 cwd、HOME/XDG 或 `/etc/open-compute` 发现配置。

例如，`ocd restart` 会重启作用域 daemon 及其全部实例，`ocd instance restart staging` 只重启一个实例。用 `ocd --instance staging dashboard` 打开该实例的 Dashboard。详见[实例](/zh/docs/ocd/instances/)、[Dashboard](/zh/docs/ocd/dashboard/)和 [Gateway](/zh/docs/gateway/)。

## 开发与部署

- `target add|list|show|test|remove`
- `wrangler [--target <name>] [--project <dir>] <wrangler-command> ...`
- `worker bundle`

`ocd wrangler` 解析项目内 Wrangler，并原样传递 Wrangler command 之后的参数。它不会下载、修复或静默替换依赖。Wrangler major 不同会警告，但不阻止执行。

target 清单位于所选 user／显式 system 作用域的 `<OCD_DIR>/targets.toml`；升级检查元数据是 `<OCD_DIR>/cache/update-check.json` 中的可丢弃缓存。
`ocd target add <名称> --api-base-url <URL> --instance-id <ID> --token-file <路径>` 登记远端 open-compute 的 InstanceId；同一个值只在线协议及 Wrangler 表面称为 `account_id`。

## 检查与诊断

- `config init|check`
- `config gateway-dns-plan|gateway-challenge-probe|gateway-dns-verify|gateway-tls-probe`
- `doctor`、`capabilities`、`support-bundle`
- `docs`、`licenses`

只读命令在 help 明确说明时支持 `--json`。JSON 有版本合同；面向人的输出不应被脚本解析。

## 维护与恢复

- `backup create|list|inspect|delete|retention-plan|restore`
- `backup cleanup-incomplete|cleanup-restore|attest-restore-smoke`
- `scheduler recover-corrupt`
- `cache clean [--instance <ID 或名称>|--all] [--dry-run]`
- `upgrade`、`uninstall [--purge --yes]`、`purge --instance <id>|--config <path>`

Backup 和 scheduler recovery 必须满足 help 与[运维指南](/zh/docs/operate/)中的 offline 或 exclusive 条件。不要直接编辑 SQLite 文件或 migration table。

`ocd cache clean` 默认只清理所选 OCD 作用域的共享可重建缓存；`--instance <ID 或名称>` 只清理一个已登记实例，`--all` 覆盖共享缓存及全部登记实例，两者互斥。`--dry-run` 只预览，不创建目录或写数据。daemon 在线时清理由 owner-only 控制 socket 执行；离线时必须取得既有 OCD 锁、实例锁并完成子进程核验／恢复，不能把 socket 不可达当作 daemon 已停止。输出区分释放或预计释放字节、跳过条目和失败；不执行 purge、临时恢复状态清理，也不删除当前嵌入的 runtime package。

`instance add --config <path>` 通过运行中的作用域 daemon 登记已初始化配置，不重写其 `[data].path`。`instance remove <ID 或名称>` 停止该实例并只移除清单项，保留配置和数据。在线修改经 owner-only 管理 socket 执行；`ocd.toml` 被外部编辑后，下一次在线写入会拒绝覆盖。`uninstall` 默认保留实例数据，只有显式 `--purge` 才会删除。`purge` 要求一个精确 selector，先打印计划，支持 `--dry-run`，且 stdin 非交互时必须传 `--yes`。

`ocd instance setup --name dev --yes` 通过运行中的 daemon 创建全新实例。`--config` 和 `--data-dir` 独立指定配置路径与显式 `[data].path`；`--autostart=false`、`--start=false` 可关闭两项默认启动选择。不带 `--yes` 时需要交互确认；已有配置或未知非空数据不会被覆盖。

`ocd wrangler` 成功后会替换 launcher process，因此最终 stdout、stderr、信号和退出码由 Wrangler 拥有。
