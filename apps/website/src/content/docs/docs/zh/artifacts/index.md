---
title: "Artifacts"
description: "通过 Wrangler、Cloudflare-compatible API 和 Worker binding 使用 Git-backed artifact repository。"
---

Artifacts 提供 account-scoped namespace 和 Git-backed repository。可以使用认证 Wrangler、兼容 `/client/v4` API、Git Smart HTTP 或 `artifacts` Worker binding 创建、导入、fork、读取和管理 repository 与 scoped token。

Repository metadata 以 SQLite 为 authority；bare Git 数据位于平台 data directory。Token 只在创建时返回明文，持久层保存 digest。Snapshot 和 restore 包含 repository 文件与不可变 Worker Version binding。

## 当前边界

支持 namespace/repository lifecycle、公开 HTTPS import、独立 fork、read/write token lifecycle、Git HTTP clone/fetch/push、REST object read 和固定 Worker binding surface。

ArtifactFS、event subscription、自动 build/deploy、Git LFS、SSH、private remote import 和 hosted placement 未提供。参见[产品](/docs/zh/products/)和[兼容性](/docs/zh/platform/compatibility/)。
