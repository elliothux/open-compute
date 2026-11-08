# P24：外部反向代理下的 Worker 自定义域名

状态：**implemented locally**（2026-11-08）。关联需求：[#148](https://github.com/elliothux/open-compute/issues/148)。

## 用户结果

operator 用自己的 Nginx、Caddy 或 Traefik 终止 TLS，把业务流量回源到 daemon 的 tenant-only `[external_ingress]`
listener；不需要启用受管 Gateway。deployer 通过 OCD vendor `public-origin` API 把一个 operator 已授权的 exact
hostname origin 绑定到 Worker，变更立即生效，不重启 daemon、不 reload 代理。用户说明见
[外部反向代理](../../apps/website/src/content/docs/docs/gateway/external-proxy.md)，当前 hostname 合同见
[Host authority](../references/host-authority.md)。

## 关键决策

- 配置与 SQLite 各拥有一个事实：`compute.toml` 的 `[external_ingress].hostnames` 授权 hostname 给 instance；SQLite 的
  claim 与 typed route 记录 hostname 绑定哪个 Worker。daemon 只投影 `hostname → instance`。
- 每个 Worker 仍是恰好一个 local route、至多一个 public route。public route 的 `transport` 为 `managed_gateway` 或
  `external_proxy`，external route 另存 `external_scheme` 与 `external_port`；DDL 只允许这三种组合。
- 控制面复用 `public-origin`，输入改为按 `transport` 区分的 tagged union，旧的隐式 `{name}` 输入删除。响应包含
  `id`、`transport`、`origin`、只读的 `ingress_available` 与 `created_on`；endpoint 项增加 `transport`。
- external listener 不挂任何平台 path。daemon 依次校验 TCP peer、origin-form request target、唯一 canonical Host，
  再进入唯一授权的 instance；instance 内按绑定的公网端口校验 authority，三条入口共用同一份 route resolver 与 dispatch。
- Worker 的 `request.url` 来自绑定 origin 加原始 path/query。runtime bridge 用一个经过校验的 `TrustedOrigin`
  替代原来只表示 HTTPS 的标记，Gateway 与 external 入口共用。
- external listener 绑定或服务失败只使 external origin 不可用，local 与 managed 入口继续服务。

## Schema

追加的 control migration `V26__external_proxy_worker_origins` 重建 `worker_host_routes`，为既有 route 按 exposure 写入
`local_http` 或 `managed_gateway`。原来挂在 claim 上的 managed namespace 守卫改到 route 插入，由 transport 选择：
managed route 必须落在 active base domain 的单层 label，external route 不能落在 base domain 及其子域树。
base-domain 变更守卫与 Gateway 停用检查只看 managed route。

## 已接受的限制

- 不采信 `Forwarded`、`X-Forwarded-*`、`X-Real-IP` 和 `CF-Connecting-IP`；Worker 看到的客户端 IP 是代理 peer。
- origin 中的 `https` 是 operator 的声明，OCD 不验证外部 TLS、证书或 DNS。
- exact hostname 授权的变更需要受控重启 daemon；第一次使用新域名需要 operator 先改 instance 配置。
- 不实现 Cloudflare Custom Domains API、通配符、多别名、代理配置下发和多级代理链。

## 验证记录

2026-11-08 在 macOS arm64 上用正式 workerd/Caddy 构建输入执行并通过：

- `open-compute-core`（124）、`open-compute-storage`（441）、`open-compute-service`（1010，含 Browser Run）库测试。
- Gate `daemon`、`workers`、`single-binary`、`contract`。`single-binary` 新增的
  `external_proxy_origins_route_exact_hostnames_without_gateway` 用真实 daemon、两个 instance、无 Gateway，覆盖重复授权
  拒绝、权限、端口与 Host 校验、控制面 path 归 Worker、转发头清理、经 Caddy 测试 CA 校验的 HTTPS 回源、停止 instance
  返回 503、撤销后 404、apex origin，以及有 binding 时拒绝删除授权。
- `cargo fmt --check`、`./test/check-rust-clippy.sh`、Dashboard/SDK typecheck、test、build、lint、SDK
  `check:generated` 与 `p6-contract` 测试。
