---
title: "项目"
description: "open-compute 架构、源码构建、测试、workerd、安全、贡献和发布指南。"
---

open-compute 是单进程 Rust 平台。`ocd` 拥有配置、data-directory lock、SQLite、object storage、公开与管理 HTTP surface、scheduler 和一个受监督的固定 workerd child。

## 架构

底层 crates 分别拥有 core types、storage、artifacts 和 runtime supervision。Workers 拥有不可变 bundle、deployment、route 和 runtime-source snapshot。Service 是 composition root。CI 会检查依赖方向。

当前正式 runtime 是由 `packages/runtime/workerd.lock.json` 固定并校验的 `elliothux/workerd` fork，随每个 release 二进制内嵌。生产启动保持离线，不搜索 `PATH` 或下载 runtime。

## 构建与贡献

仓库开发需要固定的 Rust、Bun、TypeScript 和 Git LFS 输入。Cargo 消费 runtime assets 前必须显式构建。项目使用一轮完整 final Gate，并保持 Rust 行覆盖率至少 90%。

- [仓库架构](https://github.com/elliothux/open-compute/blob/main/AGENTS.md)
- [构建与单二进制指南](https://github.com/elliothux/open-compute/blob/main/docs/references/single-binary.md)
- [测试策略](https://github.com/elliothux/open-compute/blob/main/docs/references/testing.md)
- [workerd fork 与 pin](https://github.com/elliothux/open-compute/blob/main/docs/workerd/README.md)
- [发布流程](https://github.com/elliothux/open-compute/blob/main/docs/references/releasing.md)

用户安装和应用开发应留在对应文档区；源码构建属于贡献者流程，不是运行正式 release 的前置条件。
