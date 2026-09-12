---
title: "参考"
description: "open-compute 的兼容性、限制、配置、API、认证和 runtime 合同。"
---

Reference 用于查询稳定合同。教程与操作流程分别位于[开发应用](/docs/zh/develop/)和[运行与运维](/docs/zh/operate/)。

- [兼容性](/docs/zh/platform/compatibility/)：产品、Worker API、Wrangler 与单机拓扑
- [行为差异](/docs/zh/platform/deviations/)：相对 Cloudflare 托管平台的明确差异
- [限制](/docs/zh/platform/limits/)：配置与 release 拥有的边界；live 值使用 `ocd capabilities --json`
- [未提供能力](/docs/zh/platform/unsupported/)：被拒绝或尚未实现的能力
- [Worker API 索引](/docs/zh/platform/reference/api/)：生成的 API member inventory
- [平台配置](/docs/zh/ocd/configuration/)：`compute.toml` / system config、secret reference、storage、runtime 和产品限制
- [CLI](/docs/zh/cli/)：选择、输出、联网与 mutation 语义

Cloudflare-compatible 管理 API 位于 `/client/v4`。大型 route 与 member inventory 从实现和 conformance catalog 生成，不复制到正文。
