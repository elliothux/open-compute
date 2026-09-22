# P18：单域名公网网关、DNS 与 TLS

状态：**implemented locally; public qualification pending**（2026-09-22）。Day 1 单基础域名、Worker 固定双入口、DNS-01 wildcard TLS、标准多 Caddyfile、单 data-dir 与
`ocd caddy` 运维入口的设计已同步。V8 双入口 schema、可信 exposure 参数的 resolver、按服务能力独立投影的本机/公网 endpoint、公网绑定事务及其控制面 GET/PUT/DELETE、OpenAPI/SDK 合同与 Worker 详情页双入口控制及
独立 tenant-only gateway router、只接收指定 Caddy PID 的私有 Unix upstream、固定 challenge zone 的 UDP/TCP DNS 与私有 provider protocol 已实现并通过定向测试；公网请求送入 Worker 时使用可信 HTTPS scheme。endpoint 投影已把 child PID 鉴权与该 PID 的 TLS 资格化分开，防止仅因 Caddy 启动就提前公布公网 URL。公网网关配置的静态解析、DNS record plan、域名/路径校验和托管 Caddyfile 投影也已落地；启用 Gateway 时 daemon 会创建 mode `0700` 的 `gateway/{run,storage,config-state}`，原子写入 mode `0600` 的 `managed.caddyfile` 与入口 `Caddyfile`，socket 与投影共享相同绝对路径。重启时会拒绝这些受管目录中的符号链接。
Caddy provider 已固定 Go module graph，并在固定摘要的 Docker builder 中通过 provider 单测、定制 binary 编译、module inventory 和官方 adapter 的组合配置校验。四个平台的 Caddy 2.11.4 binary、正式 lock 和 Git LFS 路径已加入单一内嵌 runtime payload；daemon 通过 `PersistentHostProcess` 前台启动并监督 Caddy，使用独立 lease、PID 准入、TLS probe 资格化、退避重启和有界关闭。批量 TXT append 中途失败时，已追加记录改用独立的有界 cleanup context 回滚。CI full scope 已加入同样的 Docker 构建；当前源码已在固定摘要的 Rust 1.98.0/Ubuntu 24.04 Docker 镜像中编译并以断网 final layer 启动真实 `ocd` 和内嵌 Caddy，验证私有 admin socket、child argv 与退出后无 Caddy/workerd 泄漏。整份配置热重载、已确认 JSON 快照恢复与 `ocd caddy` 六个受限运维命令已实现；真实公网 DNS/ACME 验收仍待具备公网 TCP 443、UDP/TCP 53 的专用主机后完成。daemon 的独占 storage bootstrap 已接入 gateway domain/Worker namespace provisioning，配置变更在启动时按 SQLite 约束校验；新增或改名公网绑定同时要求持久化 namespace active 且当前 Caddy PID 已完成 TLS 资格化，删除绑定在网关故障时仍可操作。V8 schema 还以 deferred foreign key 保证 route/claim state 在事务提交时一致，禁止墓碑化 claim/route 复活，并在 authority 层拒绝非法或保留的公网首段；重启遇到非法持久化 base domain 时拒绝该能力，不静默改写。公网绑定替换按路由的 `claim_id` 撤销旧 hostname claim，不假设两个独立主键相同；定向持久化测试覆盖主键不同的有效 schema 状态。
回滚还会按精确 zone/value 清理本次请求中结果不明的全部同值 TXT，避免 daemon 已写入而响应丢失及随后重试时遗留重复 token；清理仍是有界的 best effort，authority 的记录另有过期时间。
启用 Gateway 时，provider socket 现在位于已验证的私有 `gateway/run/`；该目录仅在需要时创建，拒绝符号链接，生成的 Caddyfile 与 daemon 使用同一路径。
同域名重启若发现必需的 Worker namespace authority 缺失，启动会拒绝该损坏状态，不会自动补建。移除 Gateway 配置时若仍有活动公网绑定，启动拒绝；绑定已撤销时保留历史 authority 并标记 disabled，再次启用须重新完成 namespace 资格化。
`ocd config gateway-dns-plan` 已提供静态 Worker DNS 记录和 TCP/UDP 端口计划；`ocd config gateway-challenge-probe` 可从配置的公网地址只读探测 UDP/TCP 53 上的 Worker challenge SOA/NS 权威应答。`ocd config gateway-dns-verify` 已把公网递归解析、父区权威委派、CAA 与 challenge DNS 检查接为一个只读命令，并通过本地 DNS fixture 的成功和错误路径。`ocd config gateway-tls-probe` 已使用构建固定的 Mozilla 根证书集、保留 probe SNI 和私有 upstream 标记验证 HTTPS。daemon 启动时会执行一次有界、只读的 DNS drift 检查；`ocd caddy status` 与 full doctor 报告 child、配置摘要、DNS、TLS 和 pin 状态。真实公网 DNS/ACME 验收仍待公网入口。
2026-09-22 从 operator 已登录的 Cloudflare DNS 页面只读核对：`open-compute.dev` zone 当前只有 apex Worker route 和 `static.open-compute.dev` R2 记录，`gateway-test.open-compute.dev` 尚未占用。尚未创建测试 DNS 记录；在真实 Caddy child、TLS 和公网 challenge DNS 可用前，DNS 修改不能构成 Gateway 验收。
本文不将设计合同或命令示例视为已实现能力。

P18 为一个 self-hosted open-compute 实例接入一个 operator 控制的专用基础域名，并为 Worker、R2 以及以后明确支持
公网访问的产品生成稳定 HTTPS URL。设计聚焦单个专用 `base_domain` 和固定产品 namespace。operator 一次性手工配置业务
wildcard DNS 和 ACME challenge DNS；之后创建、删除资源和证书续期均由 open-compute 自动完成。

P18 遵循 [Host authority](references/host-authority.md)，复用先行的
[R0 Worker `.localhost` Origin 重构](implemented/r0-localhost-worker-origins.md)建立的 Host-first ingress、hostname claim、typed product route
和 origin endpoint API；Caddy lifecycle 复用 [P17 宿主子进程管理基础设施](implemented/p17-host-process-infrastructure.md)。R0 本机 origin
不依赖 P18；P18 增加可选的公网 DNS、HTTPS、public-name 生命周期与 operator Caddyfile 扩展。

部署范围是单机、同一组织管理的 SMB。管理员可用标准 Caddyfile 发布非 open-compute 应用；不再包装一套通用反向代理 DSL，
也不要求接管整个 Caddy 配置。平台托管配置和用户文件通过原生 `import` 组合，共用一个 Caddy child。

## 1. 范围与结论

P18 Day 1 固定以下合同：

- 公网 Gateway 可选；未启用时无需 `base_domain`，启用时每个实例只配置一个 `base_domain`；
- 每个 live tenant Worker 保留一个 `local` origin，最多再绑定一个 `public` origin；平台资源不支持多个公网别名或多基础域名；
- `base_domain` 可以是专用 registrable apex，例如 `ocd.com`，也可以是已有业务 zone 下专门划出的固定子域，例如
  `compute.example.com`；
- Worker 使用 `<name>.<base_domain>`，其他公开产品使用 `<name>.<product>.<base_domain>`；
- account 仍是持久化、授权和隔离边界，公网 URL 使用独立的产品 namespace；
- 每个启用的产品 namespace 只有一条 wildcard DNS 和一张 wildcard certificate；
- 创建、部署、重命名或删除单个资源只更新 SQLite hostname binding，并复用现有 DNS、证书和 Caddy 配置；
- wildcard certificate 通过 ACME DNS-01 申请和续期，namespace 内全部资源共享该证书；
- `ocd` 内嵌并监督一个正式 pin 的定制 Caddy child；Caddy 是唯一 TLS/certificate owner；
- 用户继续管理普通 DNS zone，open-compute 为被委派的 `_acme-challenge` 名称提供最小权威 DNS；
- 同一套运行时同时支持 ocd 直接接收公网流量，以及现有四层代理按 SNI passthrough 到 ocd，并共享同一证书状态机；
- `ocd` 是平台 hostname、account、产品资源和 deployment 的唯一路由 authority；用户站点不得侵占平台 namespace；
- operator 可配置零到多个 `[[public_gateway.caddy]]`，每项 `caddy_file` 相对 ocd 配置目录解析；对用户只提供标准 Caddyfile；
- 平台文件和用户站点由一个总入口 `import`，官方 adapter 生成的 JSON 仅作为内部校验、加载和恢复产物；
- 所有托管配置、Caddy storage、配置快照和 socket 使用现有 data-dir，不新增独立 Caddy home；
- `ocd caddy` 复用既有实例选择与控制通道，GatewayManager 是唯一配置应用者；admin API 只监听私有 Unix socket；
- 正式 release 仍只有一个 native `ocd` 文件。Caddy 像 workerd 一样作为经过校验的内嵌 payload 离线物化。

核心结构：

```text
operator-owned DNS zone
  ingress.<base_domain>               A/AAAA -> public gateway address
  *.<base_domain>                     CNAME  -> ingress.<base_domain>
  *.r2.<base_domain>                  CNAME  -> ingress.<base_domain>
  ns1.<base_domain>                   A/AAAA -> public challenge DNS address
  _acme-challenge.<base_domain>       NS     -> ns1.<base_domain>
  _acme-challenge.r2.<base_domain>    NS     -> ns1.<base_domain>
                  |
                  +-- TCP 443 ------------------------------+
                  |   direct or existing L4/SNI passthrough |
                  |                                         v
                  |                                 pinned Caddy child
                  |                                 - wildcard TLS
                  |                                 - ACME renewal
                  |                                 - reverse proxy
                  |                                         |
                  |                                         v
                  |                                 private ocd listener
                  |                                 exact Host -> target
                  |
                  +-- UDP/TCP 53 -> ocd challenge DNS
                                      - authoritative only
                                      - SOA/NS/TXT only
                                      - no recursion
```

Day 1 平台公网合同使用 TCP 443 提供 HTTP/1.1 与 HTTP/2，并使用 UDP/TCP 53 提供 challenge DNS；平台 DNS-01 不要求 TCP 80。
托管配置关闭自动 HTTP 重定向，避免仅启用平台 Gateway 就额外占用 80。用户站点显式 HTTP 或自动证书流程可能还需要 80，
必须在端口计划中显示并由 operator 保证可达；不自动改变防火墙、NAT 或权限。

## 2. 设计依据

Cloudflare 的 [`workers.dev`](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/) 展示了资源创建后立即获得
稳定 URL 的产品体验。P18 在 operator 提供的专用 namespace 上实现相同的日常资源工作流。

[Let's Encrypt challenge 文档](https://letsencrypt.org/docs/challenge-types/)明确：wildcard certificate 必须使用 DNS-01；
DNS-01 查询可以通过 `NS` 或 `CNAME` 委派给独立 challenge DNS。P18 采用 `NS` 直接委派固定 challenge 名称，使证书自动化
保持 provider-neutral。

[Caddy Automatic HTTPS](https://caddyserver.com/docs/automatic-https) 已负责 ACME account、证书签发、持久化和后台续期；
[Caddy wildcard 配置](https://caddyserver.com/docs/caddyfile/patterns#wildcard-certificates)要求 DNS challenge provider。
P18 使用标准 Caddyfile、官方 adapter 和编译期自有 provider，由 Caddy 统一管理 ACME/certificate lifecycle。

[Caddyfile import](https://caddyserver.com/docs/caddyfile/directives/import) 是解析前的文件展开，不是多实例加载或隔离机制。
[全局选项](https://caddyserver.com/docs/caddyfile/options)要求最终配置最多一个且位于最前的全局块，并支持私有 Unix admin socket
与 `persist_config off`。[官方 CLI](https://caddyserver.com/docs/command-line)负责格式化、适配和验证；
[配置加载 API](https://caddyserver.com/docs/api)负责整份配置替换，不逐文件追加。

[`caddy-dns/cloudflare`](https://github.com/caddy-dns/cloudflare/blob/master/cloudflare.go) 提供了合适的薄适配结构参考：注册
`dns.providers.*` Caddy module、实现 `Provision`，并通过 libdns record append/delete 接入 Caddy 的 DNS-01 生命周期。
P18 沿用这一模块边界，在 provider 内连接 ocd 的本地 challenge authority。

## 3. 单基础域名与 URL

### 3.1 接受的域名

以下两种配置等价：

```text
dedicated apex:       ocd.com
dedicated subdomain:  compute.example.com
```

`base_domain` 必须：

- 是 canonical ASCII hostname；输入去除末尾 dot、转小写并按 IDNA 转成 A-label；
- 不是 URL、IP literal、wildcard、localhost、public suffix 或带 port/path/query/fragment 的值；
- 至少包含一个 registrable domain；
- 是 operator 明确划给当前 open-compute 实例的独占 namespace。

一个实例最多配置一个平台 `base_domain`。此限制不禁止用户 Caddyfile 托管其他域名，但额外域名的 DNS、证书与应用可用性由
operator 负责，不进入平台 public-origin 自动管理合同。域名替换 workflow 先停用当前公网 namespace 和 binding，再为新域名完成 DNS、证书和
binding onboarding；本机 claim、Worker 和当前 deployment 不变。不要求新旧基础域名同时服务，也不增加双域名迁移系统。

### 3.2 固定 namespace

| 产品               | URL                              | DNS/certificate namespace |
| ------------------ | -------------------------------- | ------------------------- |
| Worker             | `<public-name>.<base_domain>`    | `*.<base_domain>`         |
| R2 public bucket   | `<public-name>.r2.<base_domain>` | `*.r2.<base_domain>`      |
| KV public endpoint | `<public-name>.kv.<base_domain>` | `*.kv.<base_domain>`      |

KV 行固定未来的命名方法。新增公开产品在 HTTP contract 完成后加入平台固定枚举。

`public-name` 是独立持久化的 DNS label：格式为 1–63 个小写 ASCII 字母、数字或中间位置的 `-`；在所属 namespace 内唯一，
完整 hostname 在实例内全局唯一。资源 display name 与公网 URL 分别管理。

公网 URL 不包含 account。binding 保存 `account_id` 和 target identity，控制面按 account 授权，data plane 只按 canonical
hostname 读取已经持久化的 target。Worker 根 namespace 至少保留 `ingress`、`ns1`、`r2`、`kv`、`api`、`admin`、
`health` 和 `operator`。

### 3.3 Worker 固定双入口

同一个 Worker 的两个入口为：

```text
local_origin  -> http://<worker-name>.<account-id>.localhost:<local-port>/
public_origin -> https://<public-name>.<base_domain>/
```

`local` claim 随 Worker 原子创建；`public` claim 默认不存在，仅在 namespace 就绪后显式启用。两个 claim 指向同一个 Worker，
请求沿该 Worker 的 `active_deployment_id` 解析当前部署；不复制 Worker、deployment 或运行时，也不把本机请求重定向到公网。
`public-name` 独立于 Worker display name，修改它只替换唯一的 public binding。

`.localhost` 只服务访问者本机，不能作为其他员工访问共享服务器的地址；远程访问使用绑定域名。本机 endpoint 的 port 来自实际
可达的 loopback listener，公网 URL 固定使用 HTTPS 的外部 443，不暴露 Caddy 的内部 8443 或本机端口。

Gateway 未配置、DNS/ACME 失败、Caddy crash、关闭公网访问或更换基础域名，都不得删除或停用本机 claim。两个入口的持久化约束、
可信 ingress 和 API 投影分别见 §8，资源生命周期见 §9.3。

## 4. 用户手工 DNS setup

operator 保留普通 DNS zone 的管理权。setup 根据 `base_domain`、启用的产品和 operator 显式输入的公网 IPv4/IPv6 输出一份
固定 record plan；用户在现有 DNS provider 中手工创建，`ocd` 以只读方式验证结果。

以 `base_domain=ocd.com`、启用 Worker 和 R2 为例：

```dns
; 业务流量
ingress.ocd.com.                 A       203.0.113.10
*.ocd.com.                       CNAME   ingress.ocd.com.
*.r2.ocd.com.                    CNAME   ingress.ocd.com.

; ACME challenge DNS
ns1.ocd.com.                     A       203.0.113.10
_acme-challenge.ocd.com.         NS      ns1.ocd.com.
_acme-challenge.r2.ocd.com.      NS      ns1.ocd.com.
```

有公网 IPv6 时为 `ingress` 和 `ns1` 增加 AAAA。KV 只有在公开产品合同实现后才增加：

```dns
*.kv.ocd.com.                    CNAME   ingress.ocd.com.
_acme-challenge.kv.ocd.com.      NS      ns1.ocd.com.
```

每个 certificate namespace 配置独立 challenge delegation：`*.ocd.com` 的 TXT 查询发生在
`_acme-challenge.ocd.com`，`*.r2.ocd.com` 的查询发生在 `_acme-challenge.r2.ocd.com`。

`ingress` 和 `ns1` 使用显式记录并优先于 `*.ocd.com`。Day 1 公网 endpoint 全部位于 wildcard namespace 下。

setup verification 必须从公共递归 resolver 和目标权威链验证：

- `ingress` 与每个业务 wildcard 最终解析到 operator 声明的地址；
- challenge 名称确实通过 NS 委派到 `ns1`；
- `ns1` 的 A/AAAA 可从公网解析；
- UDP/TCP 53 都能取得该 challenge zone 的 SOA/NS 响应；
- CAA 授权配置的 ACME CA；
- 公共递归 resolver 与权威查询结果共同构成公网成功证据。

当前只读 verify 默认使用 1.1.1.1 和 8.8.8.8 两个公共递归 resolver；`--resolver 1.1.1.1:53` 可明确选择网络可达的公共递归服务，重复传入时要求每个都通过。随机 Worker 子域名必须返回指向 `ingress` 的 CNAME；仅有相同终点 IP 的 A/AAAA 不算符合记录计划。它还直接查询父区全部返回的 NS 地址：每台服务器须对父区 SOA 给出权威回答，再以 Authority section 的非权威 referral 返回 challenge NS。referral 不带 AA 是 [RFC 9499 的正常委派语义](https://www.rfc-editor.org/rfc/rfc9499.html)。CAA 按 [RFC 8659](https://www.rfc-editor.org/rfc/rfc8659.html) 向根方向查找首个 RRset，wildcard 上的 `issuewild` 优先于 `issue`；无 CAA 限制允许签发。[Let's Encrypt 的 `validationmethods`](https://letsencrypt.org/docs/caa/) 参数明确包含 `dns-01` 时允许，只有 `http-01` 时拒绝。带未知 critical tag、CNAME alias、尚未与实际 ACME 账户核对的 `accounturi` 或其它未验证 issuer 参数时保守失败。当前证书 CA 固定为 Let's Encrypt。DNS 读检查不写 operator zone。

显式 `--resolver`、配置的公网入口以及从父区 NS 解析得到的权威服务器地址，若是 loopback、私网、链路本地、未指定、组播或文档地址，均在查询前被拒绝；还保守拒绝 [IANA IPv4](https://www.iana.org/assignments/iana-ipv4-special-registry/) 与 [IPv6](https://www.iana.org/assignments/iana-ipv6-special-registry/) 的常见特殊用途网段（包括 benchmark、保留地址和过时转换前缀）。少数特殊网段内可全局到达的例外也会被拒绝；使用普通公网递归 resolver。地址筛选不代替真实公网可达性验证。

普通 daemon startup 以只读方式检查 DNS。DNS drift 进入 gateway health 和 doctor；operator 修改记录后显式重新验证。

## 5. Wildcard TLS 与自有 Caddy DNS provider

### 5.1 固定证书

每个启用的 namespace 使用独立 wildcard certificate：

```text
workers -> *.<base_domain>
r2      -> *.r2.<base_domain>
kv      -> *.kv.<base_domain>
```

独立 certificate 将 challenge、renewal 和 health 状态限制在对应产品 namespace。
新增资源复用 namespace wildcard certificate。Caddy 在 namespace onboarding 时申请证书，并在到期前通过相同 DNS-01 路径
自动续期。

### 5.2 `dns.providers.opencompute`

正式 Caddy 编译一个项目自有的 `dns.providers.opencompute` module。它沿用 `caddy-dns/cloudflare` 的 module wiring，并把
libdns record 操作连接到 ocd 本地 challenge authority。

module 按 [Caddy module contract](https://caddyserver.com/docs/extending-caddy) 实现 `CaddyModule`、`Provision` 和最小 libdns
record interface，并按 [Caddyfile 扩展合同](https://caddyserver.com/docs/extending-caddy/caddyfile)实现 `UnmarshalCaddyfile`。
provider 参数使用正式固定语法，经官方 adapter 转换；不手写第二套 Caddyfile 解析器。`opencompute` provider 只绑定平台 wildcard
站点的 TLS policy，不设置为所有用户站点的全局 DNS provider；额外站点不能通过它为任意域名签发证书。

provider 只实现 Caddy/CertMagic DNS-01 所需的最小 libdns append/delete contract：

1. Caddy 生成 `_acme-challenge` TXT token；
2. provider 通过 ocd 创建的私有 Unix socket 把 record name、value 和有界 TTL 交给父进程；
3. `ocd` 校验名称严格等于当前 active/provisioning namespace 的 challenge apex，拒绝任意 zone、record type 和超长 value；
4. `ocd` challenge DNS 在内存中发布 TXT，并向 provider 返回 opaque record ID；
5. Caddy 完成 propagation check 和 ACME validation；
6. provider cleanup 优先按 opaque record ID 删除 TXT；上层丢失 libdns 的 provider-specific ID 时按精确 zone/value 删除一条；未知、重复或过期 cleanup 幂等处理，并向 libdns 只返回实际删掉的记录。

Unix socket 位于 `0700` data-dir 私有目录，socket/config path 不是 capability；文件权限和 peer process identity 限制写入者。
provider update path 只存在于本机私有 Unix socket。平台生成配置、argv、环境变量、日志和 status 不包含 challenge token；
provider 的 socket I/O 使用有界 deadline，并在调用上下文取消后中断等待；
用户配置及内部展开快照可能含用户自己的凭据，按 §6.4 保护，不输出其正文。challenge token 作为短期可重建状态保存在内存中，
进程恢复后由 Caddy 重新发起 order。

### 5.3 challenge 权威 DNS

`ocd` 只服务 setup 中明确委派的 challenge zones，例如：

```text
_acme-challenge.ocd.com
_acme-challenge.r2.ocd.com
```

它回答必要的 SOA、NS 和 TXT，支持 UDP/TCP DNS；按 [RFC 7766](https://www.rfc-editor.org/rfc/rfc7766.html) 的连接复用建议，同一 TCP 连接可连续发送多条查询，空闲 5 秒后关闭。设置有界 TTL、报文和并发限制。zone/name/type 使用固定 allowlist，其他查询
返回权威 negative response 或 `REFUSED`。

公网 53 可以直接绑定，也可以由 NAT/四层代理把 UDP/TCP 53 转发到非特权内部端口。支持的部署环境保证公网 UDP/TCP 53
能够到达该 responder。

### 5.4 Caddy certificate ownership

Caddy 独占 ACME account、private key、certificate renewal 和持久化 storage：

- storage 位于 ocd data-dir 的私有 gateway 子目录，目录 `0700`、secret files 至少 `0600`；
- Caddy storage 是需要保护和备份的 secret state；support bundle 只输出摘要和健康状态；
- 丢失 storage 不触发自动“修复”，因为批量重签可能命中 CA rate limit；
- `ocd` 不解析、导出、复制或改写 Caddy private key；
- certificate readiness 同时使用 child liveness 与真实 TLS handshake 检查 SNI、chain、SAN 和有效期；
- renewal failure 进入 gateway health，不触发 `ocd`、workerd 或 Caddy 的无界 restart loop；尚有效证书继续服务。

平台证书 policy 固定一个 ACME CA endpoint。测试使用 staging 或仓库 fixture CA，生产 qualification 使用正式 endpoint；用户额外站点可按原生 TLS 指令配置，不继承平台专用 DNS-01 provider。

## 6. Pinned Caddy 与单二进制分发

P18 沿用 workerd 的供应链模型。定制 binary 在 release build 阶段按
[Caddy 官方 custom build 方式](https://caddyserver.com/docs/build)生成，但正式输入全部由仓库 lock 固定。

### 6.1 正式 pin

实现时新增一个权威 Caddy lock，记录：

- Caddy release/tag 与 source revision；
- `dns.providers.opencompute` source revision；
- Go toolchain、完整 module graph/`go.sum` 输入；
- 每个正式 target 的 binary size、SHA-256、`caddy version`、`caddy build-info`；
- `caddy list-modules` 的精确允许集合，包含自有 provider 和正式配置需要的标准 module；
- Caddyfile adapter/module 语法、内部 JSON contract/version、平台 ACME CA、HTTP protocol 和 process flags。

正式 target 与 `ocd` release matrix 一致。每个平台的定制 Caddy binary 通过 Git LFS 保存在 `share/caddy/`，构建先验证 lock、
目标、摘要、version、build info 和 module inventory，再生成确定性压缩输入并嵌入对应平台的 `ocd`。

升级 Caddy 或任一 Go module 是显式 coordinated dependency update：更新 lock/LFS bytes/licenses，重新执行 DNS-01、TLS、supervisor、
restart 和 packaged-offline qualification。运行时始终使用内嵌并完成摘要验证的 Caddy payload。

### 6.2 离线物化与监督

取得 data-dir 排他锁后，`ocd` 把内嵌 Caddy 原子物化到按 payload digest 定位的私有 runtime 目录，逐次启动前验证 bytes、版本和
module inventory。已有文件损坏时拒绝启动，不静默覆盖后继续。

```text
ocd（唯一分发文件）
  ├─ data/runtime/packages/<payload-sha256>/workerd
  ├─ data/runtime/packages/<payload-sha256>/caddy
  ├─ workerd child
  ├─ pinned Caddy child
  └─ ocd
       ├─ SQLite/hostname authority
       ├─ public data-plane upstream
       └─ challenge DNS UDP/TCP
```

P18 复用 P17 已有的 verified executable、process-group 与停止/回收原语。现有 `PersistentHostProcess` 已提供常驻 child、
租约恢复、日志 drain、有界关闭、不轮询的退出通知和脱敏退出结果；Caddy manager 已直接复用它，没有把短任务 `run_host_process` 当作 Caddy supervisor。
`GatewayManager` 拥有平台 Caddyfile 生成、
文件组合、官方适配/验证、配置快照与热重载、storage 路径/权限配置、TLS readiness、restart/backoff 和 gateway health；
ACME account、证书及私钥仍由 Caddy 独占。admin API 固定在 data-dir 私有 Unix socket，不监听默认 TCP 2019 或公网地址。
完整候选配置验证后由 GatewayManager 通过该 socket 加载；首次启动及 crash recovery 使用同一已验证快照和受控 child 路径。

Caddy 和自有 module 的许可证及 notices 必须进入 `ocd licenses`。P18 实现完成时同步更新
[`single-binary.md`](references/single-binary.md) 的内嵌内容、物化布局、构建输入和正式单文件测试。

### 6.3 常驻 child 复用边界

P17 的短任务路径为 `run_host_process -> run_image -> owner_wait`；workerd 常驻路径为 `spawn_child -> ChildHandle -> owner_loop`。
现有 `PersistentHostProcess` 已在 host extension broker 中复用 `ChildHandle`，保留独立常驻 lease 和受控关闭。P18 直接复用该入口，
不在 Gateway 中复制 spawn/pipe/signal/wait 实现，也不把产品状态机抽象成 `Supervisor<Policy>`。

通用层提供短任务 `run_host_process` 和常驻 `PersistentHostProcess::spawn` 两种入口。后者返回受控 handle；其 PID 只供私有
Unix peer-credential 准入使用，signal/wait/reap 仍由 handle 独占。通用层只管理 OS 进程，不理解 Worker deployment、OCDP、Caddy JSON、TLS 或 ACME：

- 验证后的 executable、显式 argv/environment/cwd/stdio 与必要 FD mapping；Caddy 无控制 FD 时使用 null stdin；`env_clear()`，不搜索 PATH 或下载程序；
- 一个 child 只有一个 signal/wait/reap owner，独立 process group；handle 提供存活/退出通知和完成结果，不公开可任意操作的 raw child/PID；
- handle/owner 保持 verified `ExecImage`、FD 与平台 staging 有效，直到 child 和其受管后代完成回收；
- 短任务捕获有界结果，deadline/取消/协议输出 overflow 后停止；Caddy 常驻日志持续 drain、脱敏并保留有界 tail，
  丢弃过旧内容，不因生命周期累计日志超过 capture cap 而退出；
- 常驻 child 没有短任务式总寿命 deadline；启动、探测和 TERM/KILL/reap 各有独立有界等待，Drop 不放弃回收 owner。

`caddy version`、module inventory、`fmt`、`adapt` 和 `validate` 使用短任务接口；正式服务使用前台 `caddy run` 和常驻接口。
workerd 保留 `WorkerdSupervisor` 的 control-fd、readiness 与 deployment/generation 策略；Xberg 保留单次 OCDP、rlimit 和不自动重试；
Caddy 的 TLS probe、配置切换与有界 crash backoff 仅由 `GatewayManager` 决定。通用层不再自动重启一遍。

Caddy 使用独立的 child lease/generation，复用并按 executable identity 扩展既有 orphan fencing 原语；重启前先确认旧 child 已回收，
不得仅凭进程名称或裸 PID 发信号。`ocd` crash/restart 后先恢复 ownership，再重新接管 listener/storage，避免两个 Caddy 并存。

Day 1 保持一个 workerd、一个可选 Caddy 和 Xberg 现有并发限制，不新增全局 Process Coordinator、动态调度或预测性总预算。
`service` composition root 显式协调顺序：先准备后端、private ingress 和 challenge/provider socket，再启动 Caddy；完整关闭时先停止
公网 admission，让 Caddy 在限定时间内 drain 并 TERM/KILL/reap，期间保留 upstream/workerd，之后再关闭其依赖。
只停用 Gateway 时不停止 workerd 或本机 listener。整个 Gateway 停止或重启也影响用户扩展站点，operator surface 必须显示该影响；
不能把关闭一个 Worker 的 public binding 实现为停止共享 Caddy。续期失败不作为进程重启信号，尚有效证书继续服务。

### 6.4 现有单 data-dir 与配置归属

所有 ocd 托管文件使用既有 `data_dir`，不新增 `caddy_home` 或第二个数据目录，不默认写入 `/etc/caddy` 或调用者 Home。
以下布局的受管目录、入口、managed 文件和同 payload Caddy 已实现；配置快照仍待实现。不移动现有业务数据：

```text
<data_dir>/
  control.sqlite
  runtime/packages/<payload-sha256>/caddy
  gateway/
    Caddyfile                 # ocd 生成的总入口
    managed.caddyfile         # ocd 生成的全局选项及平台站点
    config-state/             # 已展开的候选/生效/上一份有效配置及有界元数据
    storage/                  # Caddy 独占的证书、私钥、ACME state，不是缓存
    run/                      # 私有 admin/provider socket、child lease 等短期文件
    sites/                    # 可选的 operator 源文件；不由 ocd 覆盖或清理
```

最终绝对路径从已验证的 data-dir 派生。总入口与 managed 文件是可重建产物，不接受手工覆盖作为配置来源；用户修改 `sites/` 或
显式外部文件，再通过 §11.2 验证和重载。不自动将外部 Caddyfile 复制成另一份编辑来源，不建立文件同步器。

`storage`、配置快照及其目录使用私有权限（目录 `0700`，敏感文件 `0600`），写入复用已有 atomic-write/fsync/path 校验。
生成的全局块明确设置 `storage file_system` 到 `gateway/storage/`、私有 admin socket 和 `persist_config off`；Caddy 默认 autosave
不与 GatewayManager 建立第二套恢复来源。HOME/XDG 等必要路径与受管临时输出显式约束在 data-dir，不能继承调用者环境后泄漏
到平台默认目录；既有 verified-exec 所需 OS staging 仍遵循 P17，不作为另一个持久数据目录。Unix socket 路径超出平台上限时
报错并要求调整 data-dir，不悄悄改放到另一目录。

“一个数据目录”指所有平台自有状态。用户引用的外部 Caddyfile、嵌套 import、静态文件、证书或其他应用文件仍是外部输入；
用户也可以自行将它们放入 `gateway/sites/`。data-dir 备份不自动包含外部依赖，展开配置快照也不包含引用的全部文件字节。
备份/恢复明确区分：SQLite authority 与 Caddy secret storage/生效配置状态需要一致保护；受管 binary 和总入口可重建；socket、
临时 challenge TXT 不进 snapshot；用户源文件不可被缓存清理或恢复流程覆盖。

内部展开配置可能包含用户凭据，禁止通过日志、status、metrics、support bundle 或默认 CLI 输出暴露；不得把配置正文放 argv/env。
配置快照只服务加载与恢复，不变成用户必须编辑的 JSON 或第二套 Worker 路由 authority。

## 7. 两种网络拓扑，一套运行合同

两种方式都由同一个 Caddy child 管理证书并终止 TLS，差异只在公网 TCP 443 如何到达 Caddy listen address。

### 7.1 ocd 完整托管

```text
Internet TCP 443       -> Caddy :443（或 NAT -> private :8443）
Internet UDP/TCP 53    -> ocd DNS :53（或 NAT -> private :8053）
Caddy                  -> private ocd HTTP listener
```

operator 可以授予低端口 bind capability，也可以在路由器/宿主防火墙做端口映射；`ocd` 不请求 root、不自动修改 firewall、
NAT、UPnP 或系统 capability。

### 7.2 现有代理 passthrough

```text
Internet TCP 443
        -> existing L4 proxy: inspect SNI only
        -> raw TCP/TLS passthrough to Caddy private :8443
Internet UDP/TCP 53
        -> direct/NAT/L4 forwarding to ocd DNS private :8053
```

前置代理把 ocd namespace 的原始 TLS bytes 透传到 Caddy。客户端握手最终发生在 Caddy，因此看到的就是 Caddy 申请和续期的
证书；certificate lifecycle 全部留在 ocd/Caddy 内部。

passthrough rule 只匹配严格单层的 `*.<base_domain>`、`*.r2.<base_domain>` 等 namespace；未匹配 SNI 保持现有站点行为。
Nginx 需要 [`stream` + `ssl_preread`](https://nginx.org/en/docs/stream/ngx_stream_ssl_preread_module.html)，Traefik 可使用
[TCP router `tls.passthrough=true`](https://doc.traefik.io/traefik/reference/routing-configuration/tcp/tls/)。P18 提供这两种 reference
snippet，其他四层代理遵循相同 SNI passthrough contract。

以下示例假定前置代理与 Caddy 同机，Caddy 监听 `127.0.0.1:8443`，并且只启用 Worker namespace；将 Nginx 的 `9443` 替换为现有站点的 TLS upstream。Nginx 的默认分支保留原有站点，不把未知 SNI 送往平台：

```nginx
stream {
    map $ssl_preread_server_name $tls_upstream {
        ~^[^.]+\.compute\.example\.com$ 127.0.0.1:8443;
        default 127.0.0.1:9443;
    }
    server {
        listen 443;
        proxy_pass $tls_upstream;
        ssl_preread on;
    }
}
```

Traefik v3 的 `HostSNI` 单层 wildcard 已精确匹配一个 label；现有站点由其他 router 继续处理：

```yaml
tcp:
  routers:
    opencompute-workers:
      entryPoints: [websecure]
      rule: "HostSNI(`*.compute.example.com`)"
      service: opencompute
      tls:
        passthrough: true
  services:
    opencompute:
      loadBalancer:
        servers:
          - address: "127.0.0.1:8443"
```

启用 R2 等独立 namespace 后，再为 `*.r2.compute.example.com` 添加明确规则；代理位于另一主机时，upstream 使用指向 Gateway 的私有可达地址。生产命令必须从已验证配置输出对应域名和端口，不能把这里的示例值当作发现结果。[Nginx 官方示例](https://nginx.org/en/docs/stream/ngx_stream_ssl_preread_module.html)使用 `map` 选择 upstream；[Traefik 官方规则](https://doc.traefik.io/traefik/reference/routing-configuration/tcp/routing/rules-and-priority/)规定单层 wildcard 与 `HostSNI` 的匹配范围。

passthrough topology 可选用 PROXY protocol v2 保留真实 client IP；Caddy 从显式 allowlist 的 proxy 地址接收该 metadata。
平台 hostname authorization 始终使用 SNI、Host 和 SQLite binding。

用户扩展站点与平台共享同一个 Caddy listener，不再加第二个网关进程。在 passthrough 拓扑中，额外 hostname 的 DNS、前置 SNI
规则及 HTTP/ACME 所需端口由 operator 同步配置；现有 snippet 只承诺平台 namespace。加载文件成功不等于外部路径已经可达。

## 8. Caddy projection 与 `ocd` Host authority

hostname claim、typed product route、可信 ingress context 和 endpoint projection 的共享合同由
[Host authority](references/host-authority.md)拥有，R0 已实现本机入口。P18 在同一 authority 上扩展双入口约束，不建立第二套
hostname registry。V8 schema、public resolver、endpoint 投影、绑定控制面、Caddy child supervision、TLS 资格化、完整配置热重载与已确认快照恢复已经实现；真实公网验收仍待完成。

### 8.1 Caddy projection

平台 `managed.caddyfile` 是从 SQLite/config authority 生成的可重建 projection，平台部分包含：

- 固定 HTTPS listener 和关闭 HTTP/3 的 protocol 设置；
- 每个 active product namespace 的严格 wildcard SNI/Host matcher；
- 每个 namespace 的独立 ACME DNS-01 automation policy；
- `dns.providers.opencompute` 本地 Unix socket 配置；
- 一个指向 private ocd data-plane listener 的 upstream；
- Host preservation、forwarded header overwrite、timeouts 和 streaming/WebSocket 所需 transport 设置；
- data-dir 内的 Caddy storage、私有 Unix admin socket、关闭默认 config autosave 和 secret-free 平台日志。

平台 projection 不包含 resource/account/deployment ID、DNS provider credential、ACME token 或 tenant 提交的配置。普通 resource binding
变化不重载 Caddy；domain/namespace/listener intent 变化或 operator 显式重载用户文件时，才组合并加载完整配置。
用户 Caddyfile 是 §8.4 定义的独立 operator 输入，可有额外 upstream；不把它们塞入平台 Worker claim/route，也不让其接管平台流量。配置加载现已要求显式文件存在、可读、为普通文件，最多 16 项、单项最多 1 MiB、合计最多 4 MiB，并按 canonical path 拒绝重复；嵌套 import 的最终展开与站点冲突仍须由正式 Caddy adapter 验证。

公网 hostname 使用 R0 建立的实例级 claim authority。P18 必须保证：

- public-name claim 和 product-owned typed binding 在同一 SQLite transaction 中创建或切换；
- product repository 验证 target 存在、属于 account 且允许 public access；
- Worker、R2/KV 等产品通过各自带外键的 binding/route 引用共享 claim，不使用通用 dangling target ID；
- resource binding 变化只修改 SQLite authority，不进入 Caddy config，也不依赖进程内 map。

P18 tenant hostname 的所有 path 都进入对应 product ingress。`/health/*`、Artifacts 和控制面 path 只在明确的平台 listener/host
可达；tenant 可以合法拥有这些 path。unknown、保留但未激活、disabled 或 tombstoned hostname 返回稳定 404。

平台反代路径必须移除外部传入的 `Forwarded`、`X-Forwarded-*`、`CF-Connecting-IP` 和 open-compute internal headers，再生成受信的
scheme/host/client metadata。`ocd` 只信任 Caddy 的精确 private peer，并重新校验 canonical Host；Caddy 不获得 admin/deployer
token、SQLite、workerd internal endpoint 或 tenant identity。

### 8.2 R0 到双入口的 schema 合同

当前 V6 将 claim 限定为 `namespace=worker`、`exposure=local` 和 `.localhost`，且 `active_worker_host_routes` 在 `worker_id` 上
唯一。P18 必须追加 migration，不能只增加 public claim 后继续沿用该唯一索引，也不能修改已发布 V6 的字节。

保留 `hostname_claims` 和 `worker_host_routes` 作为唯一 authority，约束如下：

- active canonical hostname 继续全实例唯一，单个 claim 只绑定一个 typed target；
- claim 的 `exposure` 扩展为 `local | public`；local 仍严格使用 R0 hostname 规则，public 必须属于当前基础域名的固定产品
  namespace。只允许已实现产品枚举，不接受任意字符串 namespace；
- Worker route 携带 `namespace=worker` 和 `exposure`，以
  `(claim_id, account_id, namespace, exposure)` 复合外键引用 claim 的对应唯一键，避免跨 account、产品或入口类型错配；
- 保留 `(worker_id, account_id)` 到 Worker 的真实外键，以及一个 claim 至多一条 Worker route 的约束；
- 用 `(worker_id, exposure)` 的 active 唯一索引替代旧的 `UNIQUE(worker_id)`，只允许每种入口一条，不开放任意多域名。

核心索引为：

```sql
CREATE UNIQUE INDEX active_worker_origin
ON worker_host_routes(worker_id, exposure)
WHERE state = 'active';
```

唯一索引保证“至多一个”；每个 live tenant Worker“恰好一个 local”由 Worker create/delete 事务与持久化 invariant 检查保证。
public claim/route 的 enable、replace、disable 在同一事务内完成，claim 与 typed route 状态一致；替换先撤销旧 public binding 再创建新
binding，冲突时整个事务回滚，原 binding 保持有效。关闭 public 的代码不得误更新 local 行。

migration 保留已有 local claim/route identity、生命周期和外键关系，验证已有 local 约束再切换 schema；不一致则事务失败，不静默
补造路由或重置数据。同步更新 schema/checksum/invariant wiring 和所有 producer/consumer，不保留旧新 schema 双读写。

### 8.3 同一 resolver 与独立 endpoint 投影

本机 listener 和 Caddy private listener 调用同一个 canonical Host resolver，但准入的 exposure 来自实际 listener 的可信 ingress
context：本机路径只接收 local claim，Caddy 路径只接收 public claim。不能由外部 header 选择 exposure，也不能把当前查询中的
`c.exposure = 'local'` 简单删除后允许任意入口穿透。Host-first dispatch 和平台 path 隔离继续生效。

resolver 在一个读取快照中沿 claim、typed Worker route、Worker 的 `active_deployment_id` 冻结当前 deployment/version。
两条 origin 不各自保存 active deployment；一次部署切换影响后续两个入口的请求，已 pin 的在途请求按原有生命周期完成。

endpoint API 保留现有 `id/kind/url/scope/created_on` 结构，按 route exposure 独立投影：

| exposure | kind            | scope            | URL 来源                                                   |
| -------- | --------------- | ---------------- | ---------------------------------------------------------- |
| local    | `local_origin`  | `local_machine`  | persisted hostname + 实际可达 loopback port，HTTP          |
| public   | `public_origin` | `public_network` | persisted hostname + 已验证的 Gateway HTTPS 能力，外部 443 |

没有本机 listener 时只省略 local 项，不能提前返回整个空列表；Gateway 未完成首次资格、已停用或不可服务时只省略 public 项，
不影响 local 项。短暂故障不删除持久化 binding；public namespace 已完成资格且尚可用有效证书服务时，单纯 renewal health degraded
不撤销现有 public endpoint。新 public binding 的 admission 仍要求 namespace active。
私有 upstream/provider 使用当前 Caddy PID 鉴权；公网 endpoint 还要求完成 TLS 资格化的 PID 与当前 child PID 一致。child 切换或资格撤销时立即隐藏 endpoint，不能把进程存活当成证书就绪。
独立的 `GET .../public-origin` 控制面查询返回已保存的 public name/URL 或 `null`，不以 Caddy 当前服务能力过滤，供故障时查看和撤销；
`GET .../endpoints` 继续只表示当前可服务入口。

实现时同步 `crates/storage/src/workers/deployments.rs` 的 local-only resolver/route metadata、
`crates/service/src/workers_http.rs` 的 Host/port/ingress 校验、`crates/service/src/cloudflare_v4/vendor.rs` 的 endpoint 投影，
以及 OpenAPI、生成 SDK、CLI/Wrangler 和 Dashboard consumer。禁止把所有 route 都格式化为 `http://hostname:<local-port>/`。

### 8.4 标准多 Caddyfile 与组合边界

用户只写标准 Caddyfile，不提供用户 JSON 配置入口，也不新增反代字段 DSL。`[[public_gateway.caddy]]` 是有序文件列表，
只包含必填 `caddy_file`；零项保持纯平台托管。每项为明确文件而非 TOML 层 glob，文件不存在、不可读、路径非法或重复解析到同一
文件时整体拒绝，不悄悄跳过。文件列表、输入与展开结果均设有界大小/数量限制，超限报错而非截断。文件是否位于 data-dir 不改变其 operator ownership。

路径合同：

| 位置                               | 基准                                                                            |
| ---------------------------------- | ------------------------------------------------------------------------------- |
| TOML 中 `caddy_file`               | 被选中的 canonical ocd 配置文件所在目录；绝对路径保持绝对路径                   |
| Caddyfile 内嵌套 `import`          | Caddy 原生规则，相对于写着 import 的文件；允许原生 snippet/glob                 |
| Caddy 指令按工作目录解析的文件路径 | adapter、validate、run 共用 ocd 配置目录作为 cwd，不跟随 shell 或总入口所在目录 |

不承诺所有指令的相对路径都相对于用户文件；例如 `root ./public` 使用 Caddy 工作目录。静态文件、手工证书等建议用绝对路径，
具体指令遵循原生语义，不由 ocd 重写。嵌套 glob 的空匹配沿用 Caddy 行为，不等同于缺失一个显式 `caddy_file`。

ocd 在 `gateway/Caddyfile` 生成唯一总入口：先 import `managed.caddyfile`，再按 TOML 顺序 import 用户文件。全部使用安全引用的
绝对路径；示意中的位置均由实际 config/data-dir 派生：

```caddyfile
# Generated by ocd. Do not edit.
import "/var/lib/open-compute/gateway/managed.caddyfile"
import "/etc/open-compute/caddy/git.caddyfile"
import "/etc/open-compute/caddy/monitor.caddyfile"
```

用户文件可含多个站点、snippet 和嵌套 import；站点必须使用显式大括号。最终配置只有一个位于最前的全局块，由 managed 文件
提供。用户不能另带全局块、修改 admin/storage/进程管理，或覆盖平台生成文件；不做任意 JSON 深合并、覆盖模式或多个 Caddy
分别装载。文件顺序只用于确定性组合，不能作为 hostname 隔离手段。

Day 1 为平台保留 `base_domain` 及其子域树。用户站点使用该范围以外的明确 hostname；不允许重复/覆盖平台地址、能匹配平台树的
wildcard，或无 Host 限制的 catch-all 站点。使用官方解析/适配后的结构检查站点地址及最终平台不变量，不能只靠正则扫描源文本、
import 顺序或 Caddy 的语法校验；无法证明地址不冲突时给出明确错误。平台 unknown/disabled hostname 仍 fail closed，不落入用户
fallback。平台内部 upstream、admin/provider socket 和控制面不能通过用户站点公开。

边界内的路由、反代、rewrite、header、静态文件和站点 TLS 使用正式 binary 已有的原生模块，不逐项包装配置字段，也不运行时
下载插件。用户可托管额外域名，证书可使用其站点级原生配置；不继承平台专用 DNS-01 provider，不宣称额外域名受平台 wildcard
覆盖。额外站点不经过 workerd、不作为 Worker endpoint 返回，其后端应用鉴权由 operator 负责。

这是受信 operator 扩展，不是隔离不可信 Caddy 指令的沙箱。有权配置者等价于有权管理这些宿主服务；tenant、普通 deployer token
和 Worker 不可修改文件列表、托管文件或 admin socket。配置中用户自身的敏感信息仍按 §6.4 保护。

## 9. 生命周期

### 9.1 Domain onboarding

一次 setup 按以下顺序执行：

1. 校验 `base_domain`、公网 ingress/DNS address 和本地 listen address；
2. 输出完整业务 wildcard 与 challenge NS record plan，不执行外部写入；
3. operator 手工配置记录，并建立 TCP 443、UDP/TCP 53 的直达或转发；
4. `ocd` 以 provisioning 状态启动最小 challenge DNS；
5. 显式 verify 从公网 DNS 路径检查 A/AAAA、CNAME、NS、SOA、UDP/TCP 53 和 CAA；
6. 组合完整 Caddyfile，使用正式 adapter/validate 固定候选快照，启动 pinned Caddy；
7. Caddy 通过自有 provider 发布 TXT，取得每个 active namespace 的 wildcard certificate；
8. 对保留 probe hostname 执行真实 TLS handshake；全部成功后将 gateway/namespace 标记 active。

失败保存精确阶段和 sanitized error，显式 retry 从最近安全阶段继续；ACME retry 使用有界退避和 order budget。

### 9.2 Product namespace

Worker namespace 是基础能力；R2 等产品只有在 HTTP contract 实现后显式启用。enable 验证对应业务 wildcard 与 challenge NS，
更新 Caddy projection、取得证书并完成 TLS probe，之后才允许资源声明 public binding。

disable 只在不存在 active/pending binding 时允许；先禁止新 claim，再移除 Caddy policy/router。P18 不自动删除用户手工 DNS，
也不删除历史 certificate/private key 来假装清理完成。

### 9.3 Resource binding

namespace active 后，资源公网 enable/replace 只有一个 SQLite transaction：校验 account、产品能力、`public-name` 和保留名称，
声明或替换唯一 public binding，返回稳定 HTTPS URL。撤销 public binding 不依赖 namespace 健康或外部网络；这些资源操作均不访问
DNS、ACME 或 Caddy，不重载 Caddy。

| 操作                       | local origin           | public origin                                                |
| -------------------------- | ---------------------- | ------------------------------------------------------------ |
| 创建 Worker                | 原子建立               | 默认不存在                                                   |
| 启用公网访问               | 不变                   | 创建唯一 binding                                             |
| 部署/回滚                  | 沿 Worker 当前部署解析 | 沿同一个 Worker 当前部署解析                                 |
| 修改 public-name           | 不变                   | 原子替换，失败保留旧 binding                                 |
| 关闭公网访问               | 不变                   | claim/route 同时失效                                         |
| 删除 Worker                | claim/route 失效       | claim/route 同时失效                                         |
| Gateway 故障或更换基础域名 | 不变                   | 故障时保留 binding；换域名时停用旧 binding 并重新 onboarding |

Worker deploy 的 `workers_dev`/subdomain intent 可以调用同一 authority，但 URL 不包含 Cloudflare account subdomain；兼容文档
必须记录这一 hostname-shape deviation。

### 9.4 整体配置应用、热重载与恢复

启动、domain/namespace 更新和 `ocd caddy reload` 使用同一个 GatewayManager 配置流程；配置应用串行化，不引入额外 manager。
显式 reload 重新读取所选 ocd 配置中的 Gateway intent/文件列表及用户文件；不借机应用其他 runtime 配置变更，基础域名改变仍走
§9.1，不绕过 onboarding。第一版无自动文件监听，不轮询嵌套 import，也不启动 `caddy run --watch`。

1. 使用现有实例身份和 data-dir ownership，生成候选平台文件及完整 import 入口；不覆盖用户源文件或当前成功状态。
2. 使用正式 pinned Caddy 的官方 `caddyfile` adapter 展开、适配一次，检查平台边界并验证这份展开结果。所有工具共享确定的 cwd
   与显式环境，不继承发起重载的 shell。验证不加载到运行实例，不以候选验证触发 ACME order；自定义模块的文件访问不能被称为沙箱。
3. 在 `gateway/config-state/` 保存内部候选字节、digest、pin、cwd 和平台 workflow generation。实际 `/load` 使用已验证的同一份
   字节，不再读取 import 源文件，避免“验证一份、加载另一份”。候选文件含密时仍为私有文件。
4. 通过固定 Unix admin socket 对运行 Caddy 应用整份配置，不能逐个 reload 用户文件，也不能允许 CLI 自选 `--address` 或用
   `--config` 替换整个托管入口。首次启动通过受控 `caddy run` 加载同一快照，不生成第二个 Caddy。
5. 加载成功并完成必要的平台入口检查后，原子记录新的生效配置和上一份有效配置，才确认操作成功。已就绪 namespace 检查不得
   回退；新 namespace 的首次证书 readiness 继续按 §9.1 处于 provisioning，不把尚未完成的 ACME 当作配置加载失败。配置接受、
   平台 TLS readiness 与用户额外站点业务可用性分别报告；不等所有用户站点业务探测成功才允许平台服务。

Caddy `/load` 拒绝候选时，保留旧运行配置和旧生效指针。加载后平台检查失败则有界回退上一份适用快照，并报告原失败；回退失败
进入 degraded，保留证据，不重启 workerd 或无限重复加载。HTTP/2、WebSocket 和长流的 reload 行为依正式 pin 验收，不笼统承诺
所有连接绝对无中断。reload、child restart 和 namespace 更新不得并发抢写配置。

恢复保存的是已展开的实际配置，不只是几行 import。Caddy 默认 autosave 关闭；GatewayManager 保存有限的 current/previous 与候选
状态，并关联已有 SQLite workflow generation，不另建自由编辑的配置数据库。若在加载成功但生效指针提交前 crash，按最后持久
确认的配置恢复，未确认候选不自动成为 authority。

child crash 或 daemon restart 先按 P17/P18 回收旧进程，再恢复与当前 pin、cwd、平台 generation 一致的已确认快照；不因用户随后
改坏了源文件就自动丢掉正在使用的配置，也不默默激活尚未 reload 的改动。pin 或平台 intent 已变化时必须从当前输入重新生成并
验证，旧快照不得恢复已停用 namespace 或绕过新安全配置；不能恢复时只让 Gateway degraded，本机入口及独立管理通道仍可用。
首次没有成功快照而配置错误时关闭公网 admission 并报错，不静默忽略坏用户文件。外部证书/静态文件被移走等运行依赖失效仍需
operator 修复；展开快照不是这些资源的备份。

## 10. 状态、持久化与失败语义

domain/namespace 只保留完成恢复所需的状态：

```text
provisioning
active
degraded
disabling
```

`control.sqlite` 保存 base domain、enabled namespace、workflow generation/阶段、公网 claim 的附加状态、projection digest 和最近成功
TLS qualification metadata；hostname ownership 和 typed target relation 复用 R0 authority。ACME account、certificate 和 private key
由 Caddy storage 独占。

Caddy storage 是 certificate/ACME secret authority；平台 Caddyfile 从 authority 生成，用户 Caddyfile 是 operator 输入；内部展开
配置和有限的加载检查点仅由 GatewayManager 管理。内存 challenge TXT 是短期状态；用户 DNS zone 是手工配置的外部 authority。
配置与运行状态的 owner、恢复及保密边界见 §6.4、§9.4。

失败语义：

- DNS 未配置/错误：签发前失败，显示缺失或冲突 record，不修改外部 DNS；
- UDP/TCP 53 不可达：namespace 保持 provisioning/degraded，等待端口和 delegation 恢复；
- TXT propagation timeout：清理本次 token，有界退避，显式 retry；
- ACME issuance failure：保留 Caddy storage/account，binding admission 关闭；
- certificate renewal failure：继续服务尚有效证书并报警，不重启 workerd；
- 用户文件缺失、配置冲突、adapter/validation/load failure：保留上一份适用的有效配置/child，报告文件位置与脱敏阶段，不漏掉失败文件后继续；
- Caddy crash：按 supervisor backoff 重启，反复失败后 gateway degraded，不拖垮控制面和内部 runtime；
- ocd restart：从 SQLite 和 Caddy storage 恢复，丢弃陈旧 challenge token；
- 外部 DNS drift：doctor 报告，不在 startup/background loop 自动修改；
- Caddy storage 损坏：fail closed 并保留证据，不自动删除后批量重签。

公网 DNS/ACME 状态进入独立、secret-free gateway health component；`/health/ready` 继续表达 ocd/workerd admission state。
Gateway 故障不关闭本机 deployment admission，也不删除 local/public binding。持久化 ownership、是否允许新 binding 与当前 transport
可服务能力分别判断；renewal degraded 但有效证书仍可服务时，保留现有 public origin，见 §8.3。

## 11. 配置与 operator experience

### 11.1 配置与用户文件

以下配置和命令已经实现；四个只读 `ocd config gateway-*` 命令在表前单独列出。配置保存静态 operator intent；已有 `data_dir` 仍由现行 ocd 配置决定，不在
`public_gateway` 中重复声明。示意如下：

```toml
[public_gateway]
base_domain = "ocd.com"
ingress_ipv4 = ["203.0.113.10"]
ingress_ipv6 = []
https_listen = "0.0.0.0:8443"
challenge_dns_listen = "0.0.0.0:8053"
proxy_protocol_from = []

[[public_gateway.caddy]]
caddy_file = "./caddy/git.caddyfile"

[[public_gateway.caddy]]
caddy_file = "./caddy/monitor.caddyfile"
```

两种拓扑使用同一 Caddy 和 DNS listener。operator 通过 listen address 与外部端口转发配置网络路径，平台证书和 DNS-01 配置保持
一致。例如上述配置保存在 `/etc/open-compute/compute.toml`，第一项解析到 `/etc/open-compute/caddy/git.caddyfile`；不会相对于
`gateway/Caddyfile` 或启动 shell 解析。用户文件可以是普通站点：

```caddyfile
git.example.com {
    reverse_proxy 127.0.0.1:3000
}
```

该域名在示例平台 `ocd.com` 之外；用户负责 DNS、证书条件、后端和权限。ocd 不改写该文件，也不为它另存一套 hostname/upstream
TOML 字段。移除文件列表中的项并成功整体 reload 后才撤销对应站点；不自动删除源文件、应用数据或历史证书。

API/CLI/dashboard 必须提供：

- 生成无 mutation 的 DNS/port plan；
- 验证 DNS、端口、Caddy pin、ACME 和 TLS 的分阶段状态；
- 显式 retry/reconcile onboarding；
- enable/disable 固定 product namespace；
- 为资源 enable/disable/update public name；
- 分别展示本机 URL 与唯一公网 HTTPS URL；公网开关和 public-name 修改不影响本机入口；
- 输出 Nginx stream 与 Traefik TCP passthrough reference snippet；
- 生成 secret-free gateway doctor/support report。

正常资源页面不要求用户理解 ACME。只有首次 setup、namespace 非 active 或 renewal health 异常时才显示 DNS/port operator action。

当前可用的静态投影为 `ocd --config ./compute.toml config gateway-dns-plan [--json]`。它只读取配置中的 Worker namespace、
ingress IP 和 listen address，不查询公网 DNS，也不把记录计划误报成已完成验证。
`ocd --config ./compute.toml config gateway-challenge-probe [--json]` 对每个声明的公网 IP 查询 UDP/TCP 53，要求 Worker challenge zone 的 SOA 与 NS 都是权威应答，且指向预期的 `ns1`。此命令不验证 DNS 委派、递归解析或 CAA，不能单独作为公网 onboarding 的通过证据。
`ocd --config ./compute.toml config gateway-dns-verify [--json]` 还验证两个公共递归 resolver 上的 ingress、随机 wildcard 名称、`ns1`、challenge NS 和 wildcard CAA，从父区 NS 直接验证委派，再检查公网 challenge DNS 的 UDP/TCP 53。它只证明执行时的 DNS 条件；TLS、ACME order、Caddy child 与 daemon health 仍分别验收。
wildcard 检查要求 IN 类 CNAME 严格指向 `ingress.<base_domain>`，不把其他 DNS class 的同名记录当作业务别名。
`ocd --config ./compute.toml config gateway-tls-probe [--json]` 使用构建固定的 Mozilla 根证书集连接配置的 HTTPS listener，以保留且租户不可声明的 `probe.<base_domain>` 作为 SNI/Host，校验证书链、域名、有效期和私有 upstream 响应。该命令只报告即时探测结果；daemon 仍须在 child supervision 中用同一检查资格化当前 Caddy PID，才能公布公网 endpoint。

### 11.2 `ocd caddy` 命令与实例 ownership

`ocd caddy` 是正式内嵌 Caddy 的唯一产品命令入口；用户不需安装 Caddy、寻找 digest binary 或知道 admin socket。
复用 [P11](implemented/p11-ocd-operator-experience.md) 的 `--config`、`--instance` 与本机实例选择规则，不另建 Caddy registry、
远程 target 或 manager daemon。它不同于 [P12 Wrangler launcher](implemented/p12-wrangler-project-workflow.md) 的 opaque argv
透传：Caddy 是受管服务，配置 mutation 和 child lifecycle 必须留在 GatewayManager。

| 命令                     | Day 1 行为                                                                                         |
| ------------------------ | -------------------------------------------------------------------------------------------------- |
| `ocd caddy version`      | 输出正式内嵌 Caddy 版本与 pin，不启动网关、不联网下载                                              |
| `ocd caddy list-modules` | 用正式 pinned binary 查看实际模块集合，和 lock inventory 一致，不安装模块                          |
| `ocd caddy fmt <file>`   | 原生格式化用户文件到 stdout，保留原文件；不提供对托管文件的覆写路径                                |
| `ocd caddy validate`     | 校验平台和全部用户文件的完整候选，不改变线上配置；输出阶段与脱敏诊断，不输出展开正文               |
| `ocd caddy reload`       | 向所选在线实例请求 §9.4 的整体配置应用；daemon 不在线时明确失败，不自行启动 Caddy                  |
| `ocd caddy status`       | 从 GatewayManager 查看 child、当前配置 digest、最近 reload 结果、平台 TLS/renewal 健康和未生效错误 |

```sh
ocd --config ./compute.toml caddy validate
ocd --config ./compute.toml caddy reload
ocd --config ./compute.toml caddy status
ocd caddy fmt ./caddy/git.caddyfile
```

CLI 中显式 `--config` 选择 ocd 配置，不透传成 Caddy 的配置替换参数；`fmt <file>` 是用户显式文件路径，相对 CLI 调用目录解析，
与 TOML 文件列表的路径规则不同。工具包装复用 pinned Caddy 原生能力和有界进程执行，不重写 parser/formatter。`fmt` 内容是用户
显式请求的输出，仍不得被复制进服务日志或 support bundle。第一版不提供用户 JSON/`adapt` 输出入口或任意参数透传。

在线 validate/reload/status 通过既有实例控制通道进入 daemon，由持有 data-dir 锁的 owner 读写托管文件与运行必要短任务；不能
让 CLI 再取一遍同一排他锁或直接写 live storage。需 materialize binary 的本机工具也遵循该 ownership：在线由 daemon 协调，离线
选定实例并取得排他锁后离线物化；不下载、不搜索 PATH，不在 Home 下另建工具缓存。纯帮助/可由正式 manifest 得到的版本信息
不物化、不启动 child。离线 validate 不启动 listener 或签发证书，验证所需前置缺失时给出明确错误，不假报完整验证通过。

接口只对实例 operator 开放；不接受普通 deployer token。stdout/stderr、退出状态及阶段错误有明确映射；mutation 结果超时不
盲目重试，通过 status 检查已确认配置。控制通道/本机 CLI 不依赖当前被编辑的公网 Caddy，网关配置错误后仍可修复。

不透传 `run`、`start`、`stop`、`upgrade`、`add-package`、`remove-package`，也不允许自选 admin address、临时覆盖托管配置、
`--watch` 或单文件 reload。后续若需要 restart，仍是 GatewayManager 操作，不开启第二套 supervisor；binary 升级随 ocd 正式 pin。

## 12. 安全边界

- challenge NS delegation等价于授权 ocd 为对应 wildcard 申请证书，setup 必须明确展示该权限；
- 权威 DNS 禁止 recursion、任意 update、zone transfer 和非 challenge zone；
- Caddy provider 只能连接 data-dir 内的私有 Unix socket，只能 append/delete TXT；
- challenge record name 使用 structured DNS name 和固定 zone allowlist 精确匹配；
- Caddy 的生成 config、argv 和环境不包含 SQLite、master key、S3、admin API token 或 workerd internal capability；
- Caddy 与 ocd 当前以同一 OS identity 运行，因此 Caddy 属于受 pin 和 supply-chain Gate 约束的 trusted computing base；
- certificate/private key、ACME account 和 challenge token不进入 SQLite GET API、日志、metrics、support bundle、argv 或环境变量；
- 平台 upstream 固定为 private ocd listener；用户站点的额外 upstream 仅来自受信 operator Caddyfile，不进入 tenant outbound 能力；
- 原生用户文件不是安全沙箱；只允许 operator 管理，平台 namespace、全局设置、admin/provider socket 和内部控制面不得被扩展覆盖或公开；
- 私有 Unix admin API 由 GatewayManager 单写，CLI 不能绕过实例权限或 data-dir ownership；
- 用户配置、内部展开快照及可能包含的用户凭据不进入普通 GET API、日志、metrics 或 support bundle；
- Host/SNI 不一致、unknown namespace/binding、disabled target 和 corrupt persisted state全部 fail closed；
- direct/passthrough 都由 Caddy 终止 TLS，并经过相同 hostname authority；
- PROXY protocol peer 使用精确 allowlist；
- Caddy 和 Go dependency pin、module inventory、embedded bytes 与运行时 executable identity 都是 release/Gate 输入。

## 13. 实施顺序

1. **双入口 authority**：按 §8.2 追加 migration，建立 singleton domain/namespace workflow 与每 Worker 一 local、至多一 public 的约束；
   同步 resolver、route metadata、endpoint schema/consumer，公网未就绪时保持 local-only 可用。
2. **常驻 child 复用**：按 §6.3 从现有 runtime/workerd owner 提取必要接口，保留短任务行为；验证日志 tail、执行文件存活期、
   TERM/KILL/reap 与独立 lease recovery，不新增全局 Coordinator。
3. **Caddy supply chain**：冻结 Caddy/Go/module pin，建立正式 target 的 lock、LFS bytes、licenses 与离线物化验证。
4. **Provider 与 Challenge DNS**：实现 `dns.providers.opencompute` 的 Caddyfile/libdns 接口、私有 Unix-socket protocol 和固定 zone responder。
5. **GatewayManager**：按 §6.4、§8.4、§9.4 接入单 data-dir、平台/用户 Caddyfile 组合、官方适配、私有 admin 热重载和快照恢复；
   保留 TLS readiness、有界 crash backoff 与 composition-root 启停协调，不复制第二套配置 writer。
6. **Public ingress**：接通 Caddy trusted-ingress boundary、public Worker URL 与 passthrough PROXY protocol allowlist；完成双入口联测。
7. **R2 public bucket**：R2 HTTP/access contract 冻结后启用 `r2` namespace。
8. **Operator surface**：实现 `[[public_gateway.caddy]]` 与 `ocd caddy` 六个命令、现有实例通道和权限，更新双入口 UI、DNS/端口计划、
   gateway doctor、备份限制、Nginx/Traefik snippet 与真实 onboarding qualification。

每一步同步更新当前 Day 1 producer、consumer、schema、fixtures 和文档。已发布 database migration bytes 保持不变，schema 变化
通过新 migration 追加。

## 14. 验收

### 14.1 常规测试

- base-domain canonicalization、IDNA、public-suffix 和 malicious suffix；
- public-name、保留名称、跨 account/global conflict 和 namespace 隔离；
- 同一 Worker 同时拥有一个 local 和一个 public；第二个同 exposure binding 即使使用不同 hostname 也原子拒绝；
- claim/route 复合外键拒绝 account、namespace、exposure 错配；追加 migration 保留 local identity，损坏状态原子拒绝；
- public enable/replace/disable、失败回滚、换基础域名、Worker delete 与 restart 后的双入口生命周期；
- 两种 listener 的 exposure 隔离、同一 active deployment 解析，以及 endpoint 的 local-only/public-only/both/none 投影；
- 固定 DNS plan 对 apex/subdomain base 的正确展开；
- challenge zone exact allowlist、SOA/NS/TXT、UDP/TCP、negative response、无 recursion/AXFR/update；
- provider append/delete、opaque ID、重复 cleanup、非法 name/type/value 和 crash token loss；
- Caddy lock、target、checksum、version、build-info、module allowlist、损坏 payload 拒绝；
- 短任务 stdout/stderr overflow 及时回收的既有行为不变；常驻 Caddy 累计日志超过短任务 capture cap 后继续运行、内存有界；
- 常驻 handle 保持 executable/staging 存活，取消/Drop、reader/owner failure 和 TERM/KILL/reap 不泄漏 child；
- Caddy crash backoff、独立 lease/orphan recovery、旧 generation 回收后才重启，以及先 drain 网关再关闭 upstream 的顺序；
- 标准 Caddyfile 平台投影、0/1/N 文件组合、官方 adapter、全局块唯一性和 deterministic 完整候选；
- config-relative 路径、不同启动 cwd、含空格路径、嵌套 import/snippet/glob、缺失/重复文件和未知模块的真实 pinned 解析；
- 平台 provider 的 Caddyfile 语法和 namespace 作用域；用户额外域名不误用平台 DNS-01 或 wildcard certificate；
- 用户 exact/wildcard/catch-all 站点与平台 namespace 冲突拒绝；unknown/disabled 平台 Host 不落入用户 fallback；
- 完整配置一次适配、同字节验证/加载、并发 reload 串行化；坏文件/拒绝加载时保持旧运行配置；
- 加载后失败回退、应用和指针提交间 crash、源文件后来损坏、pin/generation 改变时的恢复与拒绝陈旧快照；
- private admin socket、无 TCP 2019、Caddy autosave 关闭、秘密快照与所有受管持久输出都在既有 data-dir；
- `ocd caddy` 六命令、实例选择/权限、fmt 不覆盖、离线/在线锁与物化行为、不绕过 GatewayManager 的 mutation；
- 禁止的原生命令/参数和未知命令明确失败，不启动第二个 Caddy、不查 PATH、不下载插件；
- Caddy storage permissions、redaction、backup contract 和损坏 fail-closed；
- Host-first dispatch、tenant path 不落入平台 handler、spoofed forwarded header stripping；
- direct 与 PROXY-protocol allowlisted passthrough client identity；
- namespace active 后资源创建/删除只更新 hostname authority，DNS/Caddy projection 保持稳定；
- workflow checkpoint crash/restart 不创建重复 ACME loop；
- single-binary isolated startup 只加载内嵌 pinned Caddy child；平台 ACME 只在显式 onboarding/renewal，用户站点证书仅由已接受的
  operator 配置触发；fmt/validate 不启动签发；备份覆盖 secret storage/配置状态并明确外部依赖不自动包含。

### 14.2 真实 ingress qualification

首次正式启用必须使用专用测试 domain、正式 pin 的 packaged `ocd` 和受控 ACME staging/production：

- 对 dedicated apex 与 dedicated subdomain 各验证一次完整 record plan；
- UDP/TCP 53 公网权威查询、NS delegation 与临时 TXT propagation 正常；
- Worker 与 R2 分别取得正确 wildcard certificate，错误层级 hostname 不被覆盖；
- Caddy restart、ocd restart、token cleanup、ACME/DNS 暂时失败和 storage reuse 不导致无界重签；
- localhost 与绑定域名同时访问同一 Worker；关闭公网或 Caddy crash 时 localhost 继续工作，恢复后复用原 binding；
- 部署/回滚后两个入口解析同一当前部署，public-name 修改失败不丢旧 URL；有效证书下 renewal degraded 不撤销现有公网入口；
- 完整托管与现有代理 SNI passthrough 都由同一 Caddy certificate 完成真实 TLS handshake；
- HTTP/1.1、HTTP/2、streaming 和 WebSocket 正常，协议广告与 Day 1 h1/h2 合同一致；
- 两个 account 争用同一 hostname 时只有一个成功；
- 平台与两个用户 Caddyfile 共存，增删用户文件经整体 reload 后不丢平台路由；用户后端失败不触发 workerd/Caddy 重启；
- 热重载保持同一 Caddy child，验证 HTTP/2、WebSocket、流式响应的真实变化；失败回退和 restart 使用适用快照；
- 443-only 平台不额外绑定 80；用户站点 HTTP/证书所需 80 与 passthrough 额外 SNI 规则在端口计划和真实部署中验证；
- 平台生成 config、argv、环境和 provider protocol 不包含 SQLite 内容、master key、S3/admin token、workerd internal capability
  或 DNS provider credential；用户自身的敏感配置不出现在 status/日志/support bundle；
- 测试结束没有遗留 listener、challenge TXT、临时 secret、未记录 process 或测试 resource。

真实 DNS/ACME qualification 使用固定 Caddy bytes、专用 domain 和公网 UDP/TCP 53，并作为独立受控外部验收记录。

## 15. 文档与兼容声明

实施时同步：

- 对齐 [Host authority](references/host-authority.md)、[R0](implemented/r0-localhost-worker-origins.md) 与
  [P17](implemented/p17-host-process-infrastructure.md) 的已实现/待实现边界；
- 更新 endpoint OpenAPI、生成 SDK、CLI/Wrangler 和 Dashboard，区分 `local_origin`/`public_origin`，不手改生成文件；
- 更新 [`references/cloudflare-compatibility.md`](references/cloudflare-compatibility.md)，记录 Workers/R2 public URL 的精确
  single-domain deviation；
- 更新 [`references/single-binary.md`](references/single-binary.md)，加入 pinned Caddy build/payload/process、既有 data-dir 下的
  Caddyfile/storage/config-state/socket 归属，不宣称外部用户文件自动包含在备份中；
- 对齐 P11 的实例控制与 P12 的 launcher 边界，更新 `ocd caddy` 帮助、CLI/API 合同和权限测试，不改写生成文件；
- 更新 install/runbook，列出固定 DNS records、TCP 443、UDP/TCP 53、用户站点的可选 80/额外 SNI、直接/NAT/passthrough 和
  PROXY protocol，以及 Caddyfile 全局块/路径/原生模块限制和六个拟议命令；
- 更新 backup/restore，保护 SQLite authority、Caddy ACME storage 和配置快照，用户源文件不被覆盖；外部依赖单列，socket/challenge token 不进 snapshot；
- 更新 doctor/support bundle，输出 secret-free DNS delegation、Caddy pin、namespace、certificate 和 renewal 状态；
- 更新 capability manifest、Wrangler fixtures 和 dashboard 文案，统一使用 open-compute public URL 术语。
