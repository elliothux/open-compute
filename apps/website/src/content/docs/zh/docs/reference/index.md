---
title: "参考"
description: "open-compute 的兼容性、限制、配置、API、认证和 runtime 合同。"
---

Reference 用于查询稳定合同。教程与操作流程分别位于[开发应用](/zh/docs/develop/)和[运行与运维](/zh/docs/operate/)。

- [兼容性](/zh/docs/platform/compatibility/)：产品、Worker API、Wrangler 与单机拓扑
- [行为差异](/zh/docs/platform/deviations/)：相对 Cloudflare 托管平台的明确差异
- [限制](/zh/docs/platform/limits/)：配置与 release 拥有的边界；live 值使用 `ocd capabilities --json`
- [未提供能力](/zh/docs/platform/unsupported/)：被拒绝或尚未实现的能力
- [API 与产品索引](/zh/docs/platform/reference/api/)：产品文档、管理 API、SDK 与生成 surface authority
- [平台配置](/zh/docs/ocd/configuration/)：`compute.toml` / system config、secret reference、storage、runtime 和产品限制
- [扩展](/zh/docs/extension/)：通过 Service Binding 暴露的 operator 原生 Provider
- [CLI](/zh/docs/cli/)：选择、输出、联网与 mutation 语义

Cloudflare-compatible 管理 API 位于 `/client/v4`。大型 route 与 member inventory 从实现和 conformance catalog 生成，不复制到正文。
