# W3：用户可扩展原生 Binding

状态：**implemented；最终资格检查已完成（2026-09-18）**。W3 在单机 Unix 部署中提供 operator 配置的
Extension Worker 与受监督 native Provider。扩展不是 tenant 上传的 native code，也不是第二个 workerd；可选 macOS XPC
资格不属于跨平台完成条件。

## 用户合同

主配置只登记本地名字与 extension directory：

```toml
[extensions.local-files]
path = "./extensions/files"
```

路径相对实际加载的 `ocd` config file 解析。目录包含严格的 `extension.toml`：

```toml
[worker]
main = "worker/index.js"

[native]
executable = "native/files-provider"
```

`ocd` 启动时以 no-follow 路径遍历打开 manifest、UTF-8 facade 与 executable，限制文件大小并固定已打开 executable 的
SHA-256/FD 身份。错误使启动失败；没有 cwd fallback、网络下载、动态库加载、热更新或历史实现 fallback。

Worker 继续使用 Wrangler `services + props`，没有新的公开 Binding 类型：

```json
{
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "props": { "directory": "invoices" }
    }
  ]
}
```

facade 从 `this.ctx.props` 读取不可变 Binding 参数并调用唯一新增的私有能力：

```ts
interface HostExtensionPort {
  call(method: number, payload: Uint8Array): Promise<Uint8Array>;
  stream(method: number, payload: Uint8Array): ReadableStream<Uint8Array>;
}
```

只有配置中打开的 facade 收到 `HOST`。普通 Worker 不能取得 Factory、session identity、Provider path、raw FD 或平台
credential。facade 的 `globalOutbound` 为 `null`；业务 codec、Provider 输入校验和宿主权限仍由 operator-owned 扩展负责。

## Authority 与持久化

Service descriptor 直接采用 schema 2 的 tagged target：普通 Worker 保存 stable Worker ID，本地扩展保存 lowercase slug。
V7 migration 是明确的 Day1 break：存在旧 `version_services` row 时整个 migration 原子失败并要求 clean data directory；没有
backfill、双读写或 schema 1 compatibility branch。已发布的 V1–V6 bytes 未修改。

扩展名字与所有 live Worker 名字共享 namespace。启动会拒绝已有冲突；后续 Worker 创建和 Service upload 也双向拒绝冲突。
部署固定扩展名字、entrypoint 与 canonical props，不固定扩展内容。operator 替换文件并重启后，现有部署使用新实现；移除扩展
只产生 unavailable，不会 fallback 到同名 Worker。

每个 generation 的 session identity 由私有 registry 为已验证 caller Worker/version/Binding 签发并授权，最多 1,024 个。
不同 Binding 使用不同 facade cache key 与 session；同一 extension 的 Provider process 可共享，但 props 不进入 Provider argv、env
或可变全局配置。

## 进程与通信

`ocd` 为每个 workerd generation 建立独立 broker socket，并以 inherited fd 4 交给正式 runtime。建立 session 时：

```text
workerd ── OCH1 + identity / SCM_RIGHTS ──> ocd
ocd     ── OCP1 / SCM_RIGHTS + ACK ──────> Provider
workerd <──── session Cap'n Proto ───────> Provider
```

`ocd` 只校验 identity、创建 socketpair、等待 Provider ACK 并转交 FD，不代理业务 payload。ACK 后会再次核验 session authority，
避免 generation/关闭竞争把晚到 FD 交给失效调用。ABI 不匹配、额外 FD、未知 identity、timeout 或 disconnect 均 fail closed。

direct ABI 只有 numeric unary 与 stream：request/response 各不超过 8 MiB，stream chunk 不超过 64 KiB，总读取不超过
64 MiB。既有 Service authority 提供 30 秒 root deadline；调用取消不代表 Provider 副作用回滚，也不会重放请求。

Provider 首次 session 时启动，每个扩展名字一个 process group。通用 persistent-process owner 负责 env clear、stdin（fd 0）control、
bounded logs、lease/start identity、TERM/KILL、reap 与严格 orphan recovery。crash 后当前调用失败，后续 acquire 按 200 ms–5 s
backoff 重启；连续六次失败后本次 `ocd` 生命周期保持 unavailable。Broker EOF 或 workerd generation 更换会关闭对应 session；
Provider 可供新 generation 复用，并在 `ocd` shutdown 时有界回收。

## Fork seam

fork 只增加私有 `HostExtensionFactory`/`HostExtensionPort`、broker fd 接线和 session-scoped Cap'n Proto client。Factory 仅绑定到
trusted loader/DO host system Workers；Port 只能通过 dynamic env capability table 委派一次，不能持久化或再次 RPC 转移。
Provider reference fixture 现由 open-compute 的 test-support Cargo 二进制
`crates/service/src/bin/host_extension_test_provider/`（Rust，schema 拷贝随仓库提交）承担，product Gate 通过 `CARGO_BIN_EXE`
直接定位；fork test target 保留 fork 自己的 C++ fixture。二者都不进入 release artifact。

FD passing 正式使用 `--//:io_backend=cxx`。macOS release build同时保留
`--@rules_rust//:extra_exec_rustc_flag=-Cstrip=none`，避免 exec-configuration proc-macro dylib 被破坏；最终 `workerd` 仍以
`--strip=always` 构建。formal source、四平台 binary/archive digest 与 build inputs 由
[`workerd.lock.json`](../../packages/runtime/workerd.lock.json)唯一固定。

## Cloudflare 边界与非目标

`services[].props` 到 `ctx.props` 与 RPC 调用继续遵守 Cloudflare
[Context](https://developers.cloudflare.com/workers/runtime-apis/context/)和
[Service Binding RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)语义。本机 extension
解析、Provider lifecycle 与 Host ABI 是明确的 open-compute superset，不计入 Cloudflare stable runtime-member inventory，也不
宣称 Cloudflare 托管环境存在相同能力。

Day1 不提供 extension installer、package/version registry、历史内容存储、grant/allowlist、marketplace、远程下载、热加载、
tenant-native sandbox、集群调度、Provider pool、HTTP fallback 或 XPC wrapper。operator 负责扩展兼容、升级、回滚与宿主权限。

## 验证

fork 的 Provider 回归直接执行 `OCP1` SCM_RIGHTS attach/ACK、Cap'n Proto unary/stream、path containment 与 disconnect；Rust
回归覆盖严格 manifest、generation broker frame/单 FD、session authority、V7 clean break、名称冲突与 descriptor/runtime-source
完整性。`p3-services-product` 使用同 revision 的真实 Provider fixture，验证两个 props 不同的 Binding 经正式 pinned workerd
完成 unary list 与 streamed read。最终 Rust line coverage 为 90.04%（136,270 / 151,346）；
`./test/gate.py --workspace` 单轮通过全部 52 个 target 与 1,550 个 case。

返回 [workerd 路线](../workerd/README.md) 与 [已完成实现](README.md)。
