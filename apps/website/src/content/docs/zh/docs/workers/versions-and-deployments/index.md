---
title: "Versions and deployments"
---

一次部署：创建或复用 Worker → 编码不可变 bundle → 校验 runtime → 激活（promote）。部署状态位于本机 SQLite；`ocd` 监督当前 workerd 进程。所选 local instance 和显式 remote target 使用同一个固定上游 Wrangler wire path。

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

校验失败不会创建 deployment，也不会改变当前 active。激活前，exact Version 必须在当前 running workerd generation 中成功加载；validation 与 commit 之间 generation 改变会拒绝 deployment。deploy / rollback 只切换 active pointer，不修改已 ready Version 的 bytes。

管理 SDK 暴露 Cloudflare Beta Worker Version DELETE 路径，用于清理历史 Version。删除非 active Version 只 tombstone 该 immutable Version 并释放其 binding reference；绝不删除当前 Worker 或外部 KV、D1、R2、Queue、Workflow、Durable Object 数据。active、仍有 pin、持久引用、前缀歧义或跨 instance 的目标都会 fail closed。重复已完成的删除是安全的，但被删 Version 不再可 rollback。

固定 Wrangler 使用的 Beta Worker GET prerequisite 返回当前 Worker 身份，并明确报告 `workers.dev` 与 preview subdomain 已关闭；open-compute 不伪造 Cloudflare 托管 DNS。

每个 committed deployment 都有一份与不可变 bytes 分离的 mutable runtime assessment。能够精确归因的 unexpected workerd exit 会 quarantine 该 deployment，并原子回退到最近的旧 dispatchable deployment；并发歧义时不猜测 culprit。`GET /client/v4/open-compute/system/status` 提供 dispatchable/quarantined 数量与 `active_runtime_dispatchable`；runtime health component 不可用时后者为 false。support bundle 包含 `deployment-runtime.json`，发生 incident 后还包含 bounded、redacted 的 `workerd-last-exit.json`。

## 兼容性

| 主题                                                                                | Cloudflare                                                                                           | open-compute                               |
| ----------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| 版本不可变；发布切换 active 指针                                                    | 是，见 [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | 是                                         |
| 回滚指向旧版本，而不是改字节                                                        | 是                                                                                                   | 是                                         |
| 部署记录                                                                            | Cloudflare 全球 rollout / placement / traffic-splitting                                              | 本机 SQLite；`ocd` 监督当前 workerd 进程   |
| gradual deployments / version affinity / Cloudflare preview URL / Workers Builds CI | 是                                                                                                   | 不提供                                     |
| `ocd wrangler` local target                                                         | 不适用                                                                                               | 已验证的 local instance admin API          |
| `ocd wrangler --target`                                                             | Wrangler deploy                                                                                      | 显式 HTTPS target；只有 loopback 可用 HTTP |
