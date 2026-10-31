# P23：Cloudflare Containers

状态：**implemented**。本地候选验收通过；正式 tag 资格与公开发布未完成（2026-10-25，UTC）。

open-compute 通过原生 `ctx.container` 和独立运维的 Linux Docker Broker 实现固定 stable Container API。
基础批次覆盖 13 个 Docker operation family；R22 批次扩展到完整 65 个成员，包含完整快照和目录恢复。
当前运维合同见 [Containers 运维](../references/containers.md)。

## 1. 已落地范围

应用使用固定 cf、官方 Vite 插件和原版 `@cloudflare/containers@0.3.7`。
Worker 的 SQLite-backed Durable Object 通过原生 `ctx.container` 执行，不改写官方 package。
动态 attachment 只授予已声明的 Container class；旧 facet 或 generation 不授予新执行的控制权限。
code-only deploy 保留已有 execution。image 或 resource rollout 发布新的 immutable target。

基础集合为 bridge inspect、container create/inspect/start/stop/kill/wait/delete、
exec create/start/resize/inspect 和 archive PUT，共 13 个 family。
Broker 验证固定 route、query、schema、状态码、HTTP upgrade、stream framing 和 tar 路径。
未知操作和危险 HostConfig、mount、device、capability 被拒绝；不提供通用 Docker 透传。

cf 管理面已接入 application、immutable image、instance 和 rollout。
当前 API 只接受零 `rollout_active_grace_period`，rolling steps 使用同一规范化和事务路径。
不提供全球 placement、fleet autoscaling、计费或每实例独立 VM。

## 2. Authority 与恢复

SQLite 持有 image、application target、Worker Version declaration 和单调 execution lease generation。
V16 保存有序 layer diff IDs、effective resources、AEAD startup envelope 和持久 stop intent；已发布 SQL 保持原字节。
manifest、config、全部 layer digest、diff IDs、架构和压缩／展开预算在 admission 前完整校验。
application create/update 只提交已准入 target，Engine 缓存不构成 image authority。
execution start 先保留 lease 和容量，再证明 Engine 图；只有明确 404 才离线导入并重新证明。

`lost`、未知 create/start 和 provider 失效仍占容量，不自动创建 replacement。
Broker 与 daemon 重启后，cleanup-only 登记只收养冻结的 lease，不启动执行或颁发凭据。
maintenance 继续持久停止意图；确认终止、清理和迟到操作 fencing 后才释放容量。
旧 key 的停止幂等受同一事务中的完整 facet、历史 identity、stop intent 和无 replacement 条件约束。

## 3. Provider 与安全边界

Docker 与 Broker 由 operator 独立部署和管理；普通 `ocd run` 不下载、启动或 supervise 两者。
只有 Broker 持有 Engine socket，workerd 与 `ocd` 使用私有 Unix endpoint。
资源资格验证实际 CPU、内存、磁盘、PID、文件与进程限制，不把 Docker 接受字段当作有效执行证明。
Engine continuity 失效后，Broker 保持 fail-closed，不自动重连或重新授权。

sidecar profile v2 使用控制端与回调凭据。token 不进入 argv、env、公开响应或 image artifact。
application 固定移除 `NET_RAW`；网络命名空间精确绑定已验证 proxy 的真实 Docker ID。
反向连接的 idle reader 在交接前退出；关闭或取用 unused connection 立即释放池槽位。
Owner、client 与 attachment grant 分别撤销所属回调，不把 native Owner 退出当作 Engine 终止。
宿主允许的 IP 网络仍由 operator 管理；平台不承诺 provider 网络直连隔离。

## 4. 验证记录与待完成检查

- 正式 runtime 输入：R9（`v1.20260930.0-open-compute-r9.a8e3d0bd4`），之后为 R22。
- 2026-10-23 R22：未插桩最终 workspace Gate 的 63 个目标、2087 项用例全部通过；P23 三项真实产品用例通过；coverage 90.763%。
  外层运行器因依赖文件描述符检查以退出码 1 结束，独立补查确认输入摘要一致。报告：`.temp/gate-run/20261023T175731-7f3ea228/report.json`。
- 三宿主发行包、十项安装范围和 x64 压力恢复检查通过。
- 2026-10-24 正式 tag 资格失败，公开发布未执行。
- 主进程日志采集仍未实现，见 [P25](../p25-platform-follow-ups.md)。

### 4.1 CF 兼容性复核

2026-10-11，复核采用上述 source `8ec1a158b0c386aea03bfd243b37fbf9daeb7f1d4e33a21627bb6d370e1b59e3`、
正式 R9、原版 `@cloudflare/containers@0.3.7` 和 `@cloudflare/workers-types@5.20260830.1`。
语义来源为 [官方 Container API](https://developers.cloudflare.com/containers/api/durable-object-container/)和
[官方 snapshots](https://developers.cloudflare.com/containers/guides/snapshots/)，并核对固定 upstream 的原生测试。
原始复核标识为 `r9-final-runtime-cf-review-8ec1/report.md`。

已测原生 `ctx.container`、HTTP、exec、streams、取消、Owner、idle、code-only、rollout 和 restart 路径为 aligned。
本次复核未发现这些路径中的新确定性 mismatch；结论只覆盖基础 13 family 与原版 SDK HTTP 场景。
该结论不声明管理 API 全量 parity，也不将 Provider 资源上限等同于 Cloudflare plan quota。

| 未完成范围                            | 状态与边界                                                   |
| ------------------------------------- | ------------------------------------------------------------ |
| 完整 64 个稳定成员资格                | blocked；未逐成员完成验收，已测路径不自动提升 inventory 状态 |
| TCP、WebSocket、HTTPS 拦截            | unverified；HTTP Fetcher 和 exec 证明不能替代这些协议        |
| Snapshot、startup snapshot options    | blocked；稳定 API 仍为目标，不改标 experimental              |
| 原生 inactivity timeout、主进程日志   | blocked；SDK alarm 和 logs 请求意图不证明对应能力生效        |
| Stock／hosted Cloudflare differential | unverified；本轮没有该执行对照，不把证据缺口写成 mismatch    |

## 5. cf 配置、Build Output 与 upload contract

### 5.1 标准配置

```ts
import {
  bindings,
  defineConfig,
  defineContainer,
  defineWorker,
  exports,
} from "cf/config";

const apiContainer = defineContainer({
  name: "api-container",
  image: { dockerfile: "./Dockerfile" },
  maxInstances: 4,
  instanceType: "lite",
  rollout: { stepPercentage: [10, 100] },
});

const worker = defineWorker({
  name: "container-app",
  entrypoint: "./src/index.ts",
  compatibilityDate: "2026-09-07",
  workersDev: false,
  exports: {
    ApiContainer: exports.durableObject({
      storage: "sqlite",
      container: apiContainer,
    }),
  },
  env: {
    API_CONTAINER: bindings.durableObject({
      worker: "container-app",
      exportName: "ApiContainer",
    }),
  },
});

export default defineConfig({ worker, containers: [apiContainer] });
```

此例保留 CT0 当时固定 `cf/config` 的声明关系，范围为基础 13 family。
当时的完整 64 个稳定成员均为 `blocked`；当前成员范围与结果见本页的 R22 记录。
mode 求值、DO export lifecycle 与 Build Output
由 cf 和官方构建链拥有；`ocd` 验证实际上传合同，不读取 TypeScript 项目文件或沿用旧 migrations/tag 配置。

字段、默认值、互斥关系、mode 求值和错误文本以固定 cf/config schema、deploy producer 与 tests 为准。
配置使用 camelCase；v4 multipart/API wire 按各自 schema 冻结，不把配置字段拼写直接当成 wire。特别要求：

- `exports` 中的 Container DO 应是同一 Worker 的 SQLite-backed export，env binding 指向该 export；
  wire 中的 `class_name`/export 关联由 CT0 fixture 固定，不能指向另一个 script；
- application name、class name、binding name 与 account/script 唯一性逐项按 fixture 校验；
- `instanceType`（wire `instance_type`）的 named/custom 两种形状不能混用，resource 值在 admission 时归一化一次；
- `maxInstances`、`rollout` steps/grace、scheduling/affinity 中未实现的字段明确拒绝，不能静默忽略；
- `image` 是开发/部署输入，不作为 tenant runtime 可修改字段；
- tenant config 不接受 engine socket、Docker host、privileges、devices、mounts、network mode 或 provider auth；
- P23 只实现当前 cf/Build Output 的 Day1 schema；旧项目由用户显式 `cf migrate <exact-file> --bundler vite`，
  不在平台内读取旧配置或增加兼容路径。
