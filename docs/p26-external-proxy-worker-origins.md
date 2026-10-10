# P26：外部反向代理下的 Worker 自定义域名

状态：**planned / 待实施**。

关联需求：[#148：Support custom-domain HTTP Host routing behind external reverse proxies without managed Gateway][issue148]。

设计基线：`main@c62b05bc765789ef14106337fb2ccf0895be1525`。本文描述目标合同，不代表当前版本已经提供这些配置、请求字段或能力。本文仅设计 Worker HTTP origin，不改动 Worker 的部署、执行和存储模型。

## 1. 决策摘要

**域名绑定由 OCD 管理；外部反向代理只负责 TLS、证书和 HTTP 转发。使用自定义域名不要求启用 managed Gateway。**

复用现有 hostname claims、Worker typed routes、部署解析和 endpoint projection，不建立新的 `domain → Worker` registry，不在外部网关生成逐 Worker 配置。[R1] [R2]

| 事项 | 本方案决定 |
| --- | --- |
| Worker 入口基数 | 保持恰好一个 local origin、最多一个 public origin |
| 公网入口的传输方式 | `managed_gateway` 或 `external_proxy`，同一 Worker 二选一 |
| 外部回源 | daemon 拥有一个可选、专用、tenant-only HTTP listener |
| 多实例归属 | operator 在 instance 配置中授权 exact hostname；SQLite 中登记 hostname 对应的 Worker |
| 对外 URL | external binding 显式声明完整 origin，包含 scheme 和可选公网端口 |
| HTTPS 与证书 | external 模式完全由 operator 的代理负责，OCD 不参与 DNS-01、ACME 或私钥管理 |
| 转发头 | 首版不采信客户端 IP、Host、scheme 的转发头，不实现多级可信代理链 |
| API | 改造现有 OCD `public-origin` API，不另造 Custom Domain CRUD 系统 |
| 生命周期 | 公网 binding 变更不重新部署 Worker、不重启 workerd、不 reload Caddy |

现有“单公网入口”限制继续成立，是本期的范围选择，而非旧实现的兼容包袱。同一 instance 内，不同 Worker 可以分别使用 managed 和 external 两种传输；同一 Worker 不同时拥有两种公网入口。不增加多别名、主域名选择和自动重定向规则。

## 2. 需求与现状

### 2.1 用户需要的结果

用户已经有 Nginx、Caddy、Traefik 等 HTTPS 入口，可能已经配置好通配符证书。希望将 `example.com` 或 `app.example.com` 显式绑定到一个 Worker，然后统一回源到 OCD：

```text
Client: https://app.example.com/orders?id=1
       |
       v
Operator reverse proxy
  TLS / certificates / forwarding
  preserve Host: app.example.com
       |
       v
OCD external HTTP ingress
  exact hostname -> instance
  hostname claim -> Worker route -> active deployment
       |
       v
Worker sees https://app.example.com/orders?id=1
```

代理无需知道 Worker 名称、instance ID、deployment ID 或内部 `.localhost` origin。代理仍需要管理其对外服务的域名、证书以及统一 upstream；“不需要 Worker 映射”不等于“不需要任何域名配置”。[issue148]

### 2.2 目前缺口

当前实现已经有统一 Host 路由，但 public origin 的登记、流量入口和展示仍依赖 managed Gateway：[R1] [R3] [R4]

| 层次 | 当前行为 | 本方案改动 |
| --- | --- | --- |
| 写入 API | `set_worker_public_origin()` 无条件检查 `public_gateway_serving()` | 按 transport 分别校验，不让 external binding 依赖 Gateway |
| 域名生成 | 公网入口围绕 `name + base_domain` 的托管模式设计 | external 直接使用授权的完整 hostname |
| Worker dispatch | local/public exposure 区分两种 listener，公网端口按 443 校验 | exposure 之外再校验实际 ingress transport，并使用 binding 的对外端口 |
| 跨 instance 分发 | `.localhost` 编码 instance，Gateway 按 instance 的 base domain 选择目标 | external 按 operator 授权的 exact hostname 选择 instance |
| Endpoints | public 项依赖 Gateway serving，URL 固定为 HTTPS | 根据各自 transport capability 投影，external 使用显式 origin |

当前是一个 daemon 管理共享 listener/Gateway，instance 各自拥有 SQLite authority 与显式数据目录。设计必须处理多实例，而不能只修改单 instance 的 HTTP handler。[R5] [R6] [R7]

## 3. 目标与不做的事

本期支持 apex domain、普通子域名、HTTP/HTTPS 对外 origin、非默认公网端口、完整 path/query、Static Assets、流式响应和已有的 WebSocket dispatch。所有业务流量继续走既有 Worker 执行路径。

本期不实现：通配符 hostname 路由、path pattern/优先级路由、多公网别名、外部代理配置下发、外部证书探测/续签、域名所有权自动验证、跨主机分布式路由 authority、外部管理面发布、PROXY protocol、HTTP/2 到 OCD 的新接入合同，以及多级代理真实客户端 IP 还原。

外部代理对客户端使用何种 HTTP 版本不由 OCD 限定；本期验收的 proxy → OCD 路径是 HTTP/1.1。代理可以复用已有通配符证书，但 OCD 的 Worker 路由仍然是 exact hostname。

Cloudflare Custom Domains 包含其平台侧域名、DNS 和证书行为；本方案仅实现自托管的 Worker origin 绑定语义，**不据此宣称 Cloudflare Custom Domains API 已兼容**。[U1]

## 4. 配置所有权与多实例隔离

### 4.1 daemon 配置：传输边界

以下为目标 `ocd.toml` 配置片段，其余现有配置保持原样：

```toml
[external_ingress]
bind = "127.0.0.1:8788"
trusted_proxies = ["127.0.0.1/32"]
```

`external_ingress` 缺省即不启用。存在该表时，`bind` 与非空 `trusted_proxies` 必须显式提供；`8788` 只是示例端口，不是新的隐式默认。每个 daemon 最多一个 external listener，不增加 listener/provider 数组或动态 transport 插件。

`trusted_proxies` 是 TCP peer 的接入白名单，而不是“信任所有转发头”的开关。只接受 IP/CIDR，不通过 DNS 解析代理身份，不隐式信任全部 loopback 或私网地址，不允许 `/0`。地址、端口和 CIDR 使用统一配置校验；IPv4-mapped IPv6 与 IPv4 的比较在 peer 边界规范化，不能形成白名单绕过。

默认推荐同机 loopback 回源。容器或独立代理需要 operator 显式配置可达的 bind 与精确 peer 网段，并使用防火墙或可信私网保护回源链路。IP 白名单不是密码学身份认证，也不能区分同一个受信任主机上的不同进程。不要将该明文 listener 暴露到不可信公网。

新增 listener 必须与已有 public/admin listener 做绑定冲突检查。绑定失败不能换一个随机端口、降级挂到管理面或悄悄改用 managed Gateway。该可选能力应报告不可用；已工作的本机入口与其他 transport 不应因其故障被撤销。

### 4.2 instance 配置：可使用哪些域名

在该 instance 的 `compute.toml` 中增加：

```toml
[external_ingress]
hostnames = ["example.com", "app.example.com"]
```

该表只授权 exact hostname 给当前 instance。它不指定 Worker、deployment、scheme 或端口，也不创建任何可执行路由。例如，`example.com` 不隐含授权 `app.example.com`；两者需要分别列出。

**配置与 SQLite 各自只拥有一个事实：**

```text
compute.toml: app.example.com is allowed for this instance
SQLite:      app.example.com is bound to Worker A
```

同一个域名在配置中获准使用后，绑定、替换 Worker 或撤销 binding 均通过现有控制面 API 完成。外部代理无需相应修改，OCD 也无需为这些 binding 变更 reload 配置。

本期采用 exact hostname 授权，避免引入通配符授权优先级、跨 SQLite 抢占协议或新的 daemon 级 Worker 域名数据库。其明确代价是：**第一次使用一个未获授权的新域名，需要 operator 先修改 instance 配置，而不是 deployer 在 Dashboard 任意声明。**

### 4.3 全 daemon 的注册校验

复用现有 instance registration 的配置快照与冲突校验流程，覆盖所有已登记实例，包括停止或未 autostart 的实例。[R5] [R6]

- 同一个 canonical external hostname 不得被两个已登记 instance 授权。停止 instance 不释放授权。
- external hostname 不得落入任何 instance 的 managed `base_domain` 及其子域树；这是命名空间所有权限制，与 Gateway 此刻是否运行无关。
- `localhost`、`.localhost` 子域、IP literal、已知平台保留 hostname 不能作为 external hostname。
- 请求路由只使用已验证的配置快照，不在每个请求中重新读取 TOML，也不在多个 instance 中遍历猜测 Worker。

为避免悬挂绑定，配置校验必须拒绝从某 instance 的授权列表中移除仍有 active external binding 的 hostname。operator 应先撤销 binding，再移除或重新分配授权；检查使用该 instance 的持久化 authority，不能因它停止而跳过。整项 external listener 关闭不属于撤销域名授权，允许保留既有 binding。

注销 instance 按现有显式注销语义撤销其 namespace reservation；之后重新登记时重新做完整冲突校验，不能因为数据库曾经使用过某域名就自动抢回。

本期不增加新的配置热加载协议。新增或修改 external listener、peer 白名单、hostname 授权统一通过受控 daemon 重启应用；启动前校验配置。现有在线注册流程若读取这些字段，也必须使用同一校验规则，不能绕过它。binding 的普通增删改没有这一重启要求。

### 4.4 数据目录不变

binding、claim 与 audit 仍在 instance 的 `[data].path` 所拥有的 SQLite 中。不得在 `<OCD_DIR>` 新建自定义域名数据库或 Worker 映射文件。

共享 listener 配置仍属于 `ocd.toml`。external 模式不创建证书、ACME 或 Gateway 状态目录。显式数据目录合同、scope lock 和 instance data-dir lock 不变。[R6] [R7] [R8]

## 5. 唯一 hostname authority 与数据模型

### 5.1 复用现有表与生命周期

仍由 `hostname_claims` 声明 hostname 所有权，由 `worker_host_routes` 通过真实外键关联 Worker。`namespace = worker`、claim/route 的 exposure 对齐、live/tombstoned 生命周期和 audit 沿用当前模型。[R1]

不增加没有外键约束的 `target_kind + target_id` 结构，也不复制一份 domain→Worker 映射到 daemon、Caddy 或 workerd。

在 Worker typed route 上增加 transport 与 external origin 的必要元数据。目标语义如下；最终 DDL 必须表达相同约束，而不是靠读取时补默认值修复非法记录：

| 字段 | 语义与约束 |
| --- | --- |
| `exposure` | 仍为 `local` 或 `public`，不是传输方式 |
| `transport` | `local_http`、`managed_gateway`、`external_proxy` 三选一 |
| `external_scheme` | 仅 external route 有值，必须为 `http` 或 `https` |
| `external_port` | 仅 external route 有值，保存规范化后的实际公网端口，范围 1–65535 |
| hostname | 只从关联 claim 获取，不在 route/endpoint 中另存副本 |
| Worker target | 继续引用 Worker，沿其 active deployment 解析，不复制 deployment ID |

合法组合只有：

```text
local  + local_http       + no external metadata
public + managed_gateway + no external metadata
public + external_proxy  + external_scheme + external_port
```

local URL 由实际本机 listener 投影；managed URL 按托管合同投影为 HTTPS；external URL 由 claim hostname 与 external metadata 投影。完整 URL 是派生值，不另存第二份可漂移的 hostname 或 URL 字符串。

### 5.2 不变量

每个 live tenant Worker 恰好一个 local route、至多一个 public route。保留每 `(worker_id, exposure)` 至多一个 active route 的数据库约束；不能把它改成每 transport 一条，否则会无意放开多个公网入口。[R1]

同一 instance 内，同一个 hostname 只能有一个 active claim，不能因 scheme、端口或 transport 不同而分配给不同 Worker。也就是说，`https://app.example.com:443` 与 `http://app.example.com:8080` 不能分别指向两个 Worker。

只有 tenant Worker 可以拥有用户 public origin；平台系统 Worker、Dashboard 和其他 product 不能通过此接口获得 binding。

### 5.3 SharedRoutes 只投影 instance 归属

daemon 的 external 分发只从已验证配置得到 `hostname → instance`。该 projection 不保存 Worker ID、deployment ID 或 binding 状态；进入选定 instance 后必须查询 SQLite authority。

因此，移除 Worker binding 后，即使 hostname 仍在授权列表中、daemon 的配置 projection 未变化，请求也只能得到未绑定响应，不能落到其他 Worker。实例停止后其授权继续保留，但 runtime route lease 被撤销，不能换投相邻实例。

## 6. Public Origin 控制面 API

### 6.1 保留一套资源接口

继续使用现有 OCD vendor 路径：[R3]

```text
GET    /client/v4/accounts/{account_id}/open-compute/workers/{script_name}/public-origin
PUT    /client/v4/accounts/{account_id}/open-compute/workers/{script_name}/public-origin
DELETE /client/v4/accounts/{account_id}/open-compute/workers/{script_name}/public-origin

GET    /client/v4/accounts/{account_id}/open-compute/workers/{script_name}/endpoints
```

`PUT` 的输入改为显式 tagged union，拒绝未知字段及不属于所选 transport 的字段。managed 输入：

```json
{
  "transport": "managed_gateway",
  "name": "app"
}
```

external 输入：

```json
{
  "transport": "external_proxy",
  "origin": "https://app.example.com"
}
```

同一 `PUT` 负责首次创建、替换公网域名以及切换 transport。`DELETE` 只删除 public binding，不影响 local origin。无 binding 时 `GET` 和 `DELETE` 的 result 都为 `null`。

采用 Day1 合同，删除旧的隐式 `{name}` 输入，不保留 alias、双重解析或按旧 SDK 版本走分支。GET/PUT 使用同一绑定响应 schema，避免继续分别返回不同形状的资源。

示例 external 绑定响应中的 `result`：

```json
{
  "id": "<route-id>",
  "transport": "external_proxy",
  "origin": "https://app.example.com",
  "ingress_available": true,
  "created_on": "<timestamp>"
}
```

managed 响应另外包含 `name`。`ingress_available` 是当前 transport 的只读 projection，不持久化，不接受客户端写入；它不证明外部 DNS、证书、代理或应用响应正常。完整响应继续使用现有 v4 envelope 与 request ID。

### 6.2 校验与权限

GET 沿用 Read 权限，PUT/DELETE 沿用 ProductWrite。deployer 只能绑定 operator 已授权给所选 instance 的 hostname；API 不能编辑授权列表或替换目标 instance。没有 operator 授权时，即使调用者能够部署 Worker，也不能抢占自定义域名。[R3]

external PUT 检查授权、origin、Worker ownership、claim 冲突和存储健康，但**不调用 `public_gateway_serving()`，也不要求外部代理已经启动**。允许先登记 binding，再接通传输；此时 `ingress_available = false`，Endpoints 不投影可用公网入口。

managed PUT 的 base-domain、namespace 和 TLS 资格检查保留在 managed 专属路径中。本期不扩大到重写 P18 的资格化流程；删除共同入口上的无条件 Gateway 检查，不等于取消 managed 模式自己的要求。[R2] [R3]

### 6.3 Origin 输入合同

external origin 必须是一个完整 HTTP(S) origin：

```text
https://example.com
https://app.example.com:8443
http://app.example.com:8080
```

仅允许空 path 或 `/`；禁止用户名、密码、query、fragment、wildcard、IP literal、空 hostname 和端口 0。入口始终绑定 origin 根路径，不能把 `https://example.com/app` 当作 origin。

host 使用现有严格 canonical hostname 合同：小写 ASCII DNS 名，不带尾点；国际化域名使用 A-label/Punycode。持久化 hostname、配置授权与 ingress Host 必须通过同一规则，不在读路径中猜测、修复或建立大小写/尾点 alias。[R4]

默认端口规范化为 HTTP 80、HTTPS 443；输出 origin 不带默认端口和尾 `/`。非默认端口必须保留。输入的空 path 与 `/` 都代表同一 origin，不形成两份记录。

### 6.4 原子性与错误语义

在一次 SQLite 事务中完成冲突检查、旧 route/claim 撤销、新 route/claim 建立和 audit。校验全部通过之前不删除旧 binding；唯一约束是并发请求的最后保护，不能依赖“先查询没占用”。相同 normalized binding 的重复 PUT 为幂等操作，保留原 ID 和创建时间。

域名已绑定到其他 Worker 时返回冲突，不做隐式转移。跨 Worker/instance 迁移由 operator 显式撤销旧绑定并重新登记；本期不设计跨库原子转移协议。

| 情况 | HTTP 语义 |
| --- | --- |
| 无有效认证 | 沿用现有 401 行为 |
| 角色不足或 hostname 不在本 instance 授权中 | 403 |
| origin 非法、字段组合错误 | 400 |
| tenant Worker 不存在 | 404 |
| hostname 被其他 claim 占用 | 409 |
| 存储或所需 managed capability 不可用 | 503 |
| 删除已经不存在的 public binding | 200，result 为 null |

实现时同步现有 ErrorCode/v4 映射、OpenAPI 与生成客户端，不杜撰 Cloudflare Zone/DNS 错误码。

## 7. External ingress 与统一分发

### 7.1 独立的 tenant-only listener

external listener 不复用 `merged_router`，也不把请求 fallback 到 `admin_router` 或普通 listener 的管理路由。它只有业务 ingress，没有内建的 `/health`、`/metrics`、`/operator`、`/client/v4` 或 Gateway probe path。

**已绑定 hostname 的每一个 path 都属于 Worker。** 例如 `/health/live`、`/operator/session/exchange` 与 `/__open_compute_gateway_probe__` 必须进入该 Worker，而不是 OCD 自身的 handler。这一约束在 daemon 的最外层成立，不能只在选定 instance 后才做 Host-first。

普通 listener 仍只接收 local origins；managed private listener 仍保持其 Caddy peer/PID/socket 边界。新增 external transport 不得打开这两条已有入口的 exposure 隔离。[R1] [R2] [R4] [R5]

### 7.2 一次请求的解析顺序

```text
1. 执行现有连接、header 和请求大小限制；读取真实 socket peer。
2. peer 必须匹配 external_ingress.trusted_proxies。
3. 严格解析唯一的 Host authority，获得 canonical hostname 与可选端口。
4. 从已验证配置选择唯一 instance；不依据 path、Bearer 或转发头选择 instance。
5. 在该 instance 查询 active claim + Worker route；要求 public + external_proxy。
6. 使用绑定 origin 校验对外 authority，建立可信的有效请求 origin。
7. 沿现有 active_deployment_id、generation、pin 与 dispatch 路径执行 Worker。
```

hostname 授权不等于 binding 存在。任何步骤不能回退成“这个 instance 的第一个 Worker”，也不能在未匹配时转到控制面。

route resolver 的输入由单独 exposure 升级为明确的 ingress transport 合同。入口类型由 listener 和受验证的 transport context 决定，不能由请求 header 自行声明。三个入口共用一份实际 Worker route resolution 与 dispatch 实现。

### 7.3 公网端口与回源端口分离

external ingress 不再使用监听端口验证业务 `Host` 的端口。先按 hostname 找到 binding，再按该 binding 的公网 origin 验证 authority。

例如 listener 是 `127.0.0.1:8788`，binding 为 `https://app.example.com:8443`，则必须接受 `Host: app.example.com:8443`，并拒绝 `Host: app.example.com:8788`。在非默认公网端口场景，省略 Host 端口同样不匹配；默认 HTTPS origin 则允许省略端口或显式 `:443`。

缺失、重复或格式非法的 Host 返回 400。authority 与 binding 不匹配、unknown/unbound hostname、wrong transport、disabled/tombstoned binding 返回不泄露目标详情的 404。已授权 instance 停止或运行时不可用返回 503；不得路由到其他 instance。peer 不受信任时返回 403，并且不进入 tenant 或控制面逻辑。

Host 是唯一外部选路输入；不从 `Forwarded` 或 `X-Forwarded-Host` 补救缺失 Host，也不允许 absolute-form request-target 覆盖 Host。本期 proxy 回源采用 origin-form request-target；拒绝其他不在该接入合同中的形式。

## 8. Worker 可见 URL 与代理信任边界

### 8.1 有效 origin 来自 binding，不来自 HTTP 回源连接

external 请求对 Worker 呈现的 URL 为：

```text
bound external origin + original path-and-query
```

对于 origin `https://app.example.com` 与回源请求 `/a%2Fb?x=%2F`，Worker 应看到 `https://app.example.com/a%2Fb?x=%2F`，而不是 `http://127.0.0.1:8788/...` 或 `.localhost` URL。

实现复用现有 runtime bridge 的可信 origin 机制，将其表达为能够携带 scheme/authority 的经过验证的请求上下文，而不是继续叠加多个特殊 header。上下文只能由已通过 peer、hostname、route 和 transport 校验的服务端路径构造；workerd 协议和执行器没有新增的公网路由 authority。[R4]

保留原始 path/query 与编码，不做前缀剥离，不使用可能把 `//host/path` 解释为另一个 authority 的 URL join。规范化后的外部 authority 同时用于 Worker 的 `request.url` 和 Host；不能把内部回源端口暴露成业务 origin。

Static Assets、相对重定向和绝对 URL 构造均应消费同一个有效 URL。保留业务 `Authorization`、`Cookie` 与其他普通 header，不把业务 Bearer 当成 OCD 管理凭据。OCD 不额外改写 Worker 返回的 `Location`、`Set-Cookie` 或 HTML。

### 8.2 不推断外部 TLS

binding 声明 `https` 代表 operator 承诺对外使用 HTTPS，**不是 OCD 测量到了客户端 TLS**。HTTPS listener、证书正确性、Host/SNI 一致性以及 HTTP→HTTPS 重定向由外部代理负责；明文 HTTP 回源不具备验证原始 TLS/SNI 的信息。

因此，代理必须只把与已登记 origin 对应的流量送入该接入点。OCD 不依据 `X-Forwarded-Proto` 动态切换 origin，也不因用户传了该 header 就升级信任或执行重定向。

### 8.3 首版不还原真实客户端 IP

external 模式不采信 `Forwarded`、`X-Forwarded-*`、`X-Real-IP`、`CF-Connecting-IP` 和平台保留的内部身份 header。进入 runtime 前清理这些来源；`CF-Connecting-IP` 只由实际 socket peer 重新生成，不保留客户端传入值。

这意味着 Worker 在 external 模式下看到的是代理 IP，不能据此完成终端用户 IP 限流或审计。该限制必须出现在用户文档和 Dashboard 的接入提示中，不能宣称与 managed Gateway 的客户端 IP 行为完全相同。managed Gateway 既有的、由受控 Caddy 写入的 IP 路径不受影响。[R4]

这是本期明确的复杂度边界：#148 要求在支持转发头时划清信任边界，并未要求实现代理链。以后确有需求时另行定义一种明确的客户端 IP 输入及覆盖规则，不在本方案中预埋多种 parser、自动 fallback 或任意 header 配置。

## 9. 生命周期、重启与故障

| 事件 | 目标行为 |
| --- | --- |
| 新建 external binding | 只写 authority；不创建证书，不 reload 任何网关 |
| 修改同一 Worker 的 external origin | 一次事务替换；失败保留旧入口 |
| external ↔ managed 切换 | 校验目标 transport 的要求后原子替换唯一 public route |
| Worker 发布或回滚 | binding 不变，继续沿 active deployment 解析 |
| 删除 public binding | 仅撤销该 public claim/route；local origin 保留 |
| 删除 Worker | 复用现有删除事务，同时撤销其 local/public 入口及引用 |
| 停止 instance | 撤销 runtime route lease；域名授权和持久化 binding 保留 |
| 关闭 external listener | 不删除 binding，不影响 local/managed；external endpoint 不再投影 |
| managed Gateway 故障或失去资格 | 不影响 external binding 或 external listener |
| 外部代理/证书/DNS 故障 | OCD 不删除 binding，不伪造探测成功；故障由外部代理负责 |
| daemon 重启 | 从配置重新验证 namespace，从 SQLite 恢复绑定，无 route 文件回填 |

绑定删除/替换事务提交后，新进入解析的请求不得继续命中旧 binding。已经完成 admission、持有部署 pin 的请求允许按现有语义结束；已建立的 WebSocket 不因域名解绑而强制迁移或断开。Worker 删除的 drain/fencing 继续遵循既有合同，而不为域名另做一个连接生命周期系统。

读请求取得 route snapshot 与 deployment pin 时继续使用现有并发保护。transport 与 origin 元数据必须来自同一已验证 snapshot，不能分别读取后拼出“旧 Worker + 新 origin”的组合。

## 10. Endpoint projection 与 Dashboard

继续使用 `local_origin` / `public_origin` 两种 endpoint kind 和现有 scope，新增显式 transport 区分。external endpoint 的示例 `result` 项：

```json
{
  "id": "<route-id>",
  "kind": "public_origin",
  "transport": "external_proxy",
  "url": "https://app.example.com/",
  "scope": "public_network",
  "created_on": "<timestamp>"
}
```

external endpoint 的投影条件是：binding active、当前 instance 具有对应 hostname 授权、external listener 已绑定且能够服务该 instance。不要引用 `public_gateway_serving()`。这只表明 OCD 侧 ingress capability 可用，不保证公网可达、外部 TLS 有效或 Worker 应用返回成功。

`GET public-origin` 始终展示仍存在的绑定；`GET endpoints` 只投影当前可用的 transport。因此 listener 关闭时 Dashboard 仍能看到已配置域名，但不可伪装成当前可用入口。只省略对应 transport 项，不能因为某一公网能力失效而清空 local endpoint。[R1] [R3]

Dashboard 的 Endpoints、Domains and routes、Visit 必须共用生成 SDK 的模型：

- 显示“External proxy / operator-managed TLS”，不显示伪造的 Certificate active 或 Gateway Ready。
- managed 输入展示 name；external 输入展示完整 origin，并说明必须先由 operator 授权 hostname。
- Visit 优先使用可用 public endpoint，否则使用 local endpoint；不要显示内部 upstream 地址。
- 只读角色可查看；编辑和撤销遵循既有 ProductWrite 权限。不可用状态不隐藏删除入口。

不持久化新的 `pending/validating/provisioning/active` 域名状态机，也不在后台主动探测任意用户提交的 origin。这样既不制造外部 TLS 的虚假承诺，也不引入 SSRF 探测面。

## 11. 部署示例

以下 API 和配置是**本方案实施后的目标使用方式**。现有版本不能通过复制这些新增字段直接获得该能力。

### 11.1 配置与绑定

operator 在 `ocd.toml` 开启 external listener，在目标 `compute.toml` 授权 hostname，按受控流程重启 daemon。保留原有 `[data].path`、认证与其他配置，不需要任何 `[gateway]` 或 `[public_gateway]` 配置。

使用有 ProductWrite 权限的实例凭据绑定已部署 Worker：

```sh
curl --fail-with-body \
  -X PUT \
  "http://127.0.0.1:8787/client/v4/accounts/${ACCOUNT_ID}/open-compute/workers/worker-a/public-origin" \
  -H "Authorization: Bearer ${OCD_TOKEN}" \
  -H 'Content-Type: application/json' \
  --data '{"transport":"external_proxy","origin":"https://app.example.com"}'
```

示例中的 `8787` 是既有管理入口示例；已配置独立 admin listener 时使用实际管理地址。业务请求必须回源到新的 external listener，不能把管理入口直接暴露为统一公网 upstream。

同机验证：

```sh
curl --fail-with-body \
  'http://127.0.0.1:8788/orders?id=1' \
  -H 'Host: app.example.com'
```

测试 Worker 应回显 `https://app.example.com/orders?id=1`。这个请求没有使用 TLS，但 peer 和已登记 origin 让它具备受控回源语义；它不是公网 HTTPS 端到端验收。

### 11.2 Nginx 使用已有证书

放在已有 Nginx 配置中，证书路径替换为 operator 的真实路径；`map` 位于 `http` context。示例特意使用 `$http_host` 保留非默认公网端口，不把 Host 改成 Worker 的内部名字。[U2] [U3]

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    listen 443 ssl;
    server_name app.example.com;

    ssl_certificate     /etc/nginx/certs/app.fullchain.pem;
    ssl_certificate_key /etc/nginx/certs/app.key;

    location / {
        proxy_pass http://127.0.0.1:8788;
        proxy_http_version 1.1;
        proxy_set_header Host $http_host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_buffering off;
        proxy_request_buffering off;
    }
}
```

按业务需要在代理上设置请求大小、超时、未匹配站点拒绝和 HTTP→HTTPS 重定向。不要使用携带额外 URI 前缀的 `proxy_pass` 来挂载 Worker；整个 origin 的 path 都应原样到达 OCD。

### 11.3 Caddy 使用已有证书

```caddyfile
app.example.com {
    tls /etc/caddy/certs/app.fullchain.pem /etc/caddy/certs/app.key
    reverse_proxy http://127.0.0.1:8788
}
```

Caddy 的 HTTP upstream 默认保留 Host；本方案不要求它额外传递可信 scheme/IP header。这里是 operator 自己的 Caddy，而不是要求 OCD 启动 managed Caddy。[U4]

## 12. 验收合同

验收复用现有配置、存储、HTTP 和真实 daemon 测试设施，不为本功能创建新的 POC 框架或并行 Gate 系统。下面是本期必须覆盖的行为，不代表已经测试通过。

| 场景 | 必须验证的结果 |
| --- | --- |
| 无 Gateway 的最小场景 | external listener + exact hostname + 已部署 Worker 能处理根路径和普通业务路径 |
| apex domain | `example.com` 可直接绑定，不要求额外子域前缀 |
| Origin 与编码 | HTTPS、HTTP、非默认公网端口、原始 path/query 正确到达 Worker |
| 控制面隔离 | `/health/live`、`/metrics`、`/client/v4/accounts`、`/operator/session/exchange`、Gateway probe path 均属于已绑定 Worker |
| Host 非法或未知 | 缺失/重复 Host 受拒绝；unknown/unbound/wrong-port/wrong-transport 不落到其他 Worker 或管理面 |
| Peer 边界 | 未授权 peer 无法访问；IPv4-mapped IPv6、空白名单和 `/0` 不绕过配置合同 |
| 转发头伪造 | Forwarded、X-Forwarded-Host/Proto/For、X-Real-IP、CF-Connecting-IP 和内部 header 不能改变选路或可信上下文 |
| 业务认证 | 普通 Authorization/Cookie 原样交给 Worker，不参与 daemon 选 instance |
| 权限 | Read-only 不能修改；deployer 不能绑定未授权 hostname 或跨 instance 目标 |
| 并发绑定 | 两个 Worker 并发争用同一 hostname 只能有一个成功；旧绑定不会因失败替换而丢失 |
| 幂等与撤销 | 同样 PUT 不生成重复记录；重复 DELETE 成功；撤销后新请求不再命中旧 Worker |
| 多实例 | 重复授权在启动/注册前报错；停止实例仍保留授权，不发生顺序选路 |
| 授权变更 | 删除仍有 active binding 的授权项被拒绝；清理后允许显式重新分配 |
| 生命周期 | 发布、回滚、Worker 删除、instance stop/start、daemon restart 均遵守 authority/pin 合同 |
| Transport 故障隔离 | managed 故障不影响 external；external 关闭不影响 local/managed，绑定仍可查询/删除 |
| Dashboard/SDK | 外部 URL、transport、可用性、只读权限与 Visit 一致，不显示内部端口或伪造 TLS 状态 |
| 业务能力 | Static Assets 根路径、资源 URL、流式请求/响应、WebSocket 使用既有执行链 |
| 网关独立性 | external binding 增删改不创建 Caddy/challenge 子进程；有 managed Gateway 时也不触发其 reload |

真实端到端验收至少启动一次 OCD 与一个外部代理：代理持有测试证书，客户端验证证书后通过 HTTPS 访问，代理保留 Host 以 HTTP 回源。使用测试 CA 明确信任测试证书，不以 `curl -k` 作为 TLS 验收通过条件。不需要真实公网 DNS、ACME 或生产证书私钥。

验收还应明确确认：external 模式的 Worker 客户端 IP 为回源 peer，而非测试客户端；这是已声明的限制，不可用伪造 header 隐藏。

## 13. 实施顺序与完成条件

先完成配置作用域、exact hostname 冲突与 transport 数据约束；再把 ingress origin/port 校验接入现有统一 resolver；最后同步控制面和所有消费者。传输与 API 实现应保持薄，不把存储事务或部署生命周期搬入 HTTP handler。

OpenAPI、生成 SDK、Dashboard、错误映射、相关 CLI 帮助/接入示例以及测试必须在同一功能改动中对齐。CLI 继续以 `ocd cf` 负责现有部署路径，自定义 binding 通过上述管理 API/SDK 操作；本期不另造一套代理配置 DSL 或重做部署 CLI。

文档落地时更新 hostname authority 合同、Routing/Gateway 用户说明、兼容矩阵和偏差清单，明确区分“支持 external exact-host origin”与“未实现 Cloudflare Custom Domains API”。不因本方案支持 hostname 绑定，就解除应用配置里未支持的 Cloudflare zone Routes 字段限制。

本设计期间不将当前用户文档改成“已经支持”。实际完成并通过相关验收后，再把本文件按仓库生命周期移入 `implemented/`，更新活动索引；只记录真实执行过的验证结果。[R9]

## 14. Day1 与 schema 发布边界

目标只保留一套当前数据模型、一个 public-origin API 合同和一份 Worker resolver。不得保留旧输入兼容 alias、按版本分支、双写域名 registry，或“读不到新字段就假装 external/managed”的运行时补救。

已经随正式版本发布的 migration 文件、顺序和字节不可修改；schema 变更必须追加 migration。追加 migration 必须在数据库事务内建立目标约束，旧 authority 不满足目标合同且没有直接、安全的结构变更路径时应显式拒绝升级，而不是自动补造授权、猜测 origin 或清空数据。

本期不承诺旧 persisted format 兼容；需要 operator 处理的 pre-1.0 breaking change 必须在实施 PR/release notes 中说明。不得把 Day1 当成覆盖已发布 migration、自动 reset 用户数据或取消完整性检查的理由。[R8]

## 15. 本期边界的理由

**为什么不只去掉 Gateway 检查？** 因为完整 hostname 的输入、跨 instance 选择、transport admission、公网端口、可信 origin 与 endpoint projection 都有独立合同，只放开写入会留下无法访问或错误暴露的路由。[R3] [R4] [R5]

**为什么增加专用 HTTP listener？** 因为现有普通 listener 承担本机/控制面职责，直接让它接收 external public claims 会扩大暴露面。专用 tenant-only 接入点复用 resolver，但不复用管理面权限分发；一个额外可选 listener 比在管理面上逐 path 打补丁更直接。

**为什么配置中还要有域名列表？** 因为 daemon 共享入口，而 authority 按 instance 分开。配置只做 namespace delegation 和权限边界，不复制 Worker 路由。这允许停止实例保留域名、启动前发现冲突，并避免每次请求扫描多个数据库或实现跨库域名抢占事务。

**为什么首版不支持一个 Worker 多个域名？** #148 的验收只需要一个显式公网 origin。保持单一公网入口不需要新增 alias 集合、primary domain、重定向策略或多个公网 URL 的偏好规则。多域名需求另行设计，不预埋相关状态。

**为什么不直接实现 Cloudflare Custom Domains API？** 因为本需求不要求 OCD 管理 Cloudflare Zone、DNS 或证书。复用已有 OCD vendor API 可以准确表达“外部 TLS + 显式 origin”，而不是伪造一个表面兼容、实际缺少官方生命周期的接口。[U1]

**为什么不支持转发头？** 显式 origin 已解决本 issue 的域名、scheme 与 Dashboard 问题。保留代理 IP 的限制是清晰可验证的；隐式信任 X-Forwarded-* 则会改变实际安全边界。首版选择前者。

## 16. 依据与参考

以下仓库依据固定到本设计基线。正文中的 `[R#]` 指当前实现或仓库合同，`[U#]` 指外部官方资料；没有引文标记的规范性决定是本方案提出的目标设计。

- [issue148] 原始需求与最小验收场景。
- [R1] `docs/references/host-authority.md`：统一 hostname authority、固定双入口、transport 与 endpoint 合同。
- [R2] `docs/implemented/p18-single-domain-public-gateway.md`：managed Gateway 的资格化、共享 Caddy 与私有 upstream。
- [R3] `crates/service/src/cloudflare_v4/vendor/worker_origins.rs`：当前 public-origin API 与 endpoint projection。
- [R4] `crates/service/src/workers_http.rs`：exposure dispatch、可信 HTTPS origin、Host/port 校验与部署 pin。
- [R5] `crates/service/src/http/shared.rs`：共享 daemon 的 instance 选择与 runtime route lease。
- [R6] `crates/service/src/instance_registry.rs`：显式 instance registration、scope 与独立数据目录。
- [R7] `crates/core/src/config.rs`：daemon listener 和 instance 配置的分工。
- [R8] `AGENTS.md`：Day1、单机 SMB 复杂度预算与已发布 migration 不可变规则。
- [R9] `docs/README.md`：活动设计编号与文档生命周期。
- [U1] Cloudflare Workers Custom Domains 官方文档。
- [U2] Nginx HTTP proxy module 官方文档：Host 转发与 proxy 配置。
- [U3] Nginx WebSocket proxying 官方文档。
- [U4] Caddy reverse_proxy 官方文档：HTTP upstream、Host 与 header 行为。

[issue148]: https://github.com/elliothux/open-compute/issues/148
[R1]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/docs/references/host-authority.md
[R2]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/docs/implemented/p18-single-domain-public-gateway.md
[R3]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/crates/service/src/cloudflare_v4/vendor/worker_origins.rs
[R4]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/crates/service/src/workers_http.rs
[R5]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/crates/service/src/http/shared.rs
[R6]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/crates/service/src/instance_registry.rs
[R7]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/crates/core/src/config.rs
[R8]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/AGENTS.md
[R9]: https://github.com/elliothux/open-compute/blob/c62b05bc765789ef14106337fb2ccf0895be1525/docs/README.md
[U1]: https://developers.cloudflare.com/workers/configuration/routing/custom-domains/
[U2]: https://nginx.org/en/docs/http/ngx_http_proxy_module.html
[U3]: https://nginx.org/en/docs/http/websocket.html
[U4]: https://caddyserver.com/docs/caddyfile/directives/reverse_proxy
