---
title: "ocd CLI"
description: "按任务查询 open-compute daemon、开发 launcher、诊断和维护命令。"
---

`ocd --help` 和各级 subcommand 的 `--help` 是参数 authority。本页只按任务分组并说明共同语义。

## 安装与运行

- `setup`、`run`
- `start`、`stop`、`restart`、`status`、`logs`、`dashboard`
- `instances`、`instance remove`

使用全局 `--config <path>` 或 `--instance <id>` 选择本机 instance。多数 online 命令可以自动选择唯一运行实例；没有或存在多个候选实例时会 fail closed。

## 开发与部署

- `target add|list|show|test|remove`
- `wrangler [--target <name>] [--project <dir>] <wrangler-command> ...`
- `worker bundle`

`ocd wrangler` 解析项目内 Wrangler，并原样传递 Wrangler command 之后的参数。它不会下载、修复或静默替换依赖。Wrangler major 不同会警告，但不阻止执行。

## 检查与诊断

- `config init|check`
- `doctor`、`capabilities`、`support-bundle`
- `docs`、`licenses`

只读命令在 help 明确说明时支持 `--json`。JSON 有版本合同；面向人的输出不应被脚本解析。

## 维护与恢复

- `backup create|list|inspect|delete|retention-plan|restore`
- `backup cleanup-incomplete|cleanup-restore|attest-restore-smoke`
- `scheduler recover-corrupt`
- `upgrade`、`uninstall`

Backup 和 scheduler recovery 必须满足 help 与[运维指南](/docs/zh/operate/)中的 offline 或 exclusive 条件。不要直接编辑 SQLite 文件或 migration table。

`ocd wrangler` 成功后会替换 launcher process，因此最终 stdout、stderr、信号和退出码由 Wrangler 拥有。
