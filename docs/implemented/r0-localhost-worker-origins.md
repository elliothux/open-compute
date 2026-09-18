# R0：Worker `.localhost` Origin 重构

状态：**implemented（2026-09-17）**。GitHub `#90` 对应的本机 Worker ingress 已从 tenant path 改为 exact-host origin。

R0 是 [Host authority](../references/host-authority.md) 的首个实现批次。它不依赖 P18 Gateway、DNS 或 TLS，也不修改 workerd。

## 当前 Day 1 合同

每个 tenant Worker 创建时原子取得：

```text
http://<worker-name>.<account-id>.localhost:<public-port>/
```

- hostname 为 lowercase canonical ASCII，由现有 Worker name 与 canonical account UUID 组成；
- `.localhost` claim 在实例内唯一，typed Worker route 固定拥有 `/`；
- port 来自实际 bound public listener；非 loopback bind 不发布不可达的 local endpoint；
- Worker、Static Assets、redirect、SPA 和 `fetch()` 看到的 pathname 从 `/` 开始；
- tenant hostname 在 `/health`、`/client/v4` 等平台 path 之前解析；
- unknown、uppercase、尾随 dot、IP literal、错误 port、disabled 或 tombstoned authority 均 fail closed；
- 旧 tenant path 不再解析，也没有 alias、redirect、dual read/write 或版本分支。

`.localhost` 是 [RFC 6761 special-use namespace](https://www.rfc-editor.org/rfc/rfc6761.html#section-6.3)。本机 HTTP 可被
[Secure Contexts](https://www.w3.org/TR/secure-contexts/#localhost) 规则视为 potentially trustworthy，但不是生产 HTTPS，
也不提供远程访问；公网 hostname、DNS 和证书属于 P18。

Cloudflare [`workers.dev`](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/) 使用
`<worker>.<account-subdomain>.workers.dev`。本实现保留 Worker/account 的 host identity 形状，但将托管域替换为
本机 `.localhost`，因此是明确的本地部署投影，不冒充 Cloudflare 的公共 `workers.dev` 服务。

## Persistence 与迁移

control migration V6 新增两个 authoritative 表：

```text
hostname_claims
  id, hostname_ascii, account_id, namespace=worker, exposure=local,
  state, generation, timestamps

worker_host_routes
  id, claim_id, account_id, worker_id, path_prefix=/, entrypoint,
  state, generation, timestamps
```

V6 先验证每个 tenant Worker 恰有与 lifecycle 一致的平台 route，再把 route identity 和时间戳迁入 claim/typed route，最后删除
`worker_routes`。不一致数据使迁移事务失败；已发布 V1–V5 bytes 未修改。当前 Worker create/delete/list/resolve 只读写新 authority。

Worker create 在一个 SQLite transaction 中创建 Worker、observability defaults、hostname claim 和 typed route。delete 同时 tombstone typed
route 与 claim。请求解析 exact active claim 后冻结 route generation、deployment 和 Version；SQLite 仍是唯一 authority，内存和 endpoint
response 只是 projection。

## Ingress 与 endpoint API

public 和 merged router 都先检查 platform-managed `.localhost` authority。命中该 namespace 后不会回退控制面或默认 handler；原始 method、
query、Host、body 和从 `/` 开始的 pathname 直接交给现有 workerd transport。

vendor endpoint API 现在只返回当前 shape：

```json
{
  "id": "<route-id>",
  "kind": "local_origin",
  "url": "http://app.<account-id>.localhost:8787/",
  "scope": "local_machine",
  "created_on": "2026-09-17T00:00:00Z"
}
```

没有可达 loopback public listener 时返回空列表。旧 `path` 字段已从 OpenAPI、生成 SDK 和 Dashboard consumer 删除。

## P18 双入口扩展边界（待实现）

[P18](../p18-single-domain-public-gateway.md) §8–9 在本 authority 上增加“每 Worker 一个 local、最多一个 public”合同；每实例最多
配置一个基础域名。当前 V6 的 local-only CHECK 与 `UNIQUE(worker_id)` 尚不支持双入口，需要追加 migration，不能改已发布 V6。

扩展必须同步 local-only resolver、route metadata、endpoint API、OpenAPI/生成 SDK 和 consumer，不能只放开数据库唯一索引。
未来 local/public endpoint 分别按实际 transport capability 投影；无本机 listener 不得让已可用的 public endpoint 一起消失。
两个入口绑定同一个 Worker/当前部署；关闭公网、换域名或 Gateway 故障不撤销 local claim。此节是 P18 设计引用，不改变上述 R0
当前实现和下面的已完成验收声明。

## 验收覆盖

回归覆盖 canonical host/port 拒绝、Host-first 平台 path 遮蔽、endpoint 空/非空投影、Worker create/list/resolve/delete、V5 数据迁移、缺失
route 的原子拒绝，以及 real-process Gate 使用 canonical local hostname 调用 Worker。Static Assets 和 workerd transport 继续复用既有真实
runtime 覆盖；不增加 mount-prefix rewrite。

Cloudflare 兼容结论记录在 [Cloudflare API 兼容性](../references/cloudflare-compatibility.md) 和
[P1 差异登记](../references/p1-deviations.md)。

返回[文档索引](README.md)。
