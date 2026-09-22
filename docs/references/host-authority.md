# Host ingress 与 hostname authority

本文持续维护 open-compute 本机和公网 HTTP ingress 共用的 hostname ownership 与解析合同。具体实施、迁移和资格分别由 R0、P18
及后续产品文档拥有。R0 本机路径已实现；P18 的双入口与 Gateway 扩展已在本地实现；公网 DNS/ACME qualification 仍需具备公网 TCP 443、UDP/TCP 53 的专用主机。

## 唯一 authority

`ocd` 是 hostname 到 product target 的唯一解析 authority：

```text
canonical authority + pathname
              |
              v
persisted hostname claim
              |
              v
typed product binding / route
              |
              v
Worker | R2 | another explicit HTTP product
```

- canonical hostname 在实例内全局 claim；claim 保存 account、product namespace、exposure、state、generation 和 lifecycle metadata；
- 一个 hostname 只归属一个 product namespace；该 product 可以继续执行自己声明的 path routing；
- target 使用带真实外键的 product-owned binding/route 表表达；禁止使用没有 referential integrity 的通用
  `target_kind + target_id`；
- 自动生成的 Worker origin 和公开 bucket origin 绑定 `/`，因此该 origin 的全部 path 归属同一 target；
- SQLite 是 authority；Gateway config、runtime memory、endpoint response 和 SDK model 都只是 projection。

R0 已建立全局 hostname claim 和 Worker typed route，默认 Worker endpoint 为
`<worker>.<account>.localhost`。后续公网 Gateway 和其他 HTTP 产品复用该 authority，不建立第二套 hostname registry、内存 route map
或 Gateway-owned resource mapping。

## Worker 双入口基数

实例可不启用 Gateway；启用时最多配置一个基础域名。每个 live tenant Worker 恰好一个 local claim，最多一个 public claim，
不支持任意多公网别名。两条 typed route 都指向同一个 Worker，沿其 `active_deployment_id` 解析当前部署，不各自存储 deployment。

P18 的追加 migration 已把 R0 的 `UNIQUE(worker_id)` 改为每 `(worker_id, exposure)` 至多一条 active route；claim/route 的 account、
namespace 和 exposure 以复合外键对齐。local 恰好一条由 Worker 创建/删除事务和 invariant 验证保证；public 的新增、替换、撤销
同事务完成，失败不丢失旧 binding。完整 schema 与迁移合同见 [P18](../p18-single-domain-public-gateway.md) §8.2。

关闭 public、更换基础域名或 Gateway 故障不改变 local claim；删除 Worker 才同时撤销两个入口。资源绑定变化仍只更新 SQLite。

## 传输路径

```text
local client  --HTTP------------------------> ocd ingress
public client --HTTPS--> Gateway/TLS child --> private ocd ingress
                                                   |
                                                   `-- same hostname authority
```

Gateway 的平台路径只拥有 TLS、ACME、固定 product namespace admission、外部 header 清洗和一个固定 `ocd` upstream。
它不接收逐 Worker/bucket route，不持有 deployment/account mapping；资源创建、删除和切换不得触发 Gateway reload。

`ocd` 从实际 listener 建立可信 ingress context。直接 listener 使用实际 scheme/client address；Gateway private listener 只接受经过固定
peer/socket boundary 的 Gateway，并使用其覆盖后的 external scheme/client metadata。外部 `Forwarded`、`X-Forwarded-*` 与平台内部
header 不能自行提升为可信 context。两条路径使用相同的 canonical Host resolver，但允许的 exposure 由实际 listener 决定：本机路径
只接收 local claim，Gateway private 路径只接收 public claim。请求不能自行声明 exposure；transport metadata 不改变持久化 ownership。

平台管理的 tenant hostname 必须在 path-based 控制面 router 之前解析。匹配后，`/health`、`/client/v4`、`/operator` 等 path 都是
tenant path；unknown、disabled、tombstoned 或 Host/SNI 不一致均 fail closed。

## Operator Caddyfile 扩展

同一个 Caddy 可通过标准 `import` 加载 operator 管理的额外站点文件；平台配置仍由 ocd 生成。一个基础域名的约束针对平台
Worker/R2 等 origin，不禁止用户在其他域名上发布非平台应用。平台 `base_domain` 及其子域树保留，不允许用户 exact/wildcard
或无 Host 限制的站点抢占；平台 unknown/disabled Host 不落入用户 fallback，不能仅靠文件顺序保证隔离。

额外站点的原始 Caddyfile 是 operator 配置来源，不复制成 Worker claim/route 或第二套 deployment mapping；其请求直接由 Caddy
处理，不经过 workerd，不作为 Worker endpoint 返回。扩展是宿主级管理员能力，不开放给 tenant/deployer，也不是不可信配置沙箱。

所有托管文件和证书状态仍在现有 data-dir；平台总入口与用户文件组合成一份完整运行配置，`ocd caddy` 通过既有实例控制通道
交给 GatewayManager 统一验证、热重载和恢复。admin API 仅在私有 Unix socket，不能让 CLI 或 Dashboard 成为第二个配置 writer。
用户文件变更可触发显式整体 reload，普通 Worker 资源绑定变化仍不重载 Caddy。完整合同见 [P18](../p18-single-domain-public-gateway.md)
§6.4、§8.4、§9.4 与 §11.2。

## Endpoint projection

Endpoint API 从 hostname authority 和各自 transport capability 独立投影 URL：本机返回 `local_origin`/`local_machine`，公网返回
`public_origin`/`public_network`。local 使用实际可达 loopback port 与 HTTP，public 使用外部 HTTPS 443，不复用本机或 Caddy 内部端口。

本机 listener 不可达时只省略 local 项，不能提前返回整个空列表；Gateway 未完成首次资格、已停用或不可服务时只省略 public 项。
已完成资格且有效证书仍能服务的 renewal degraded 不撤销既有 public endpoint；新增 public binding 仍要求 namespace active。
故障不删除持久化 binding，也不阻止 Worker deployment 建立本机可用状态。endpoint 不是另一套路由 authority。

OpenAPI、生成 SDK、CLI/Wrangler 与 Dashboard 同步消费两种 kind/scope，不把所有 route 当成 local origin。具体改动与验收由 P18 拥有。

## 实施归属

- [R0 Worker `.localhost` Origin 重构](../implemented/r0-localhost-worker-origins.md)：已落地 hostname claim、Worker typed route、Host-first
  ingress 与 endpoint projection；
- [P17 宿主子进程管理基础设施](../implemented/p17-host-process-infrastructure.md)：已有 verified-exec 与 process ownership 原语，
  常驻 Caddy 接口已由 P18 提取接入；不拥有路由；
- [P18 单域名公网网关、DNS 与 TLS](../p18-single-domain-public-gateway.md)：复用 R0 authority，增加公网 DNS、TLS、Gateway transport
  与固定双入口生命周期；本地实现已落地，真实公网 qualification 单独保留。
