---
title: "Containers"
description: "配置独立运维的 Broker，启用容器执行、快照与目录恢复。"
---

Containers 使用独立运维的 Broker 与通过资格检查的 rootful Linux Docker Engine。daemon 在 SQLite 中保存 application target 与 execution lease。Broker 拥有 Engine 访问权限。daemon 启动与 tenant 请求均不安装工具、拉取镜像或构建应用。

运行时实现 Workers types `5.20260930.2` 的固定 65 项 Container API。API 包含进程生命周期、exec stream、HTTP 与 WebSocket、named images、inactivity policy、容器快照及目录恢复。运行时兼容性与管理 API 范围分别登记。

## 准备 Provider

1. 准备 rootful Linux Docker Engine，使用 API 1.42 或更高版本、cgroup v2 与 seccomp。
2. 在启用 `pquota` 的 XFS 上配置 overlay2，使可写层磁盘上限生效。
3. 准备固定 sidecar image、不可变应用镜像与已验证的只读 exec helper volume。
4. 创建私有 Broker 配置，填写 Engine socket、daemon UID、持久 `state_dir`、spool 与资源上限。
5. 在 operator 的服务监督器下运行 `ocd container-broker --broker-config /absolute/path/broker.toml`。
6. 核对 Broker 资格结果，取得成功的 Provider 合同摘要。

Broker 命令拒绝 `--system`、`--config` 和 `--instance`。作用域 daemon 不监督 Broker。`qualified = false` 的诊断合同不准入 Container mutation。持久 Broker state 不是可删除的 cache。

如果资格检查返回错误，Broker 退出，不提供合同。sidecar 资格失败事件记录固定阶段和是否超过整体期限。事件不含凭据或上游异常。

[仓库运维指南](https://github.com/elliothux/open-compute/blob/main/docs/references/containers.md)包含完整 Broker 配置、helper 供应与资格检查。已安装 Docker CLI 或 Engine 接受配置，均不足以证明资格通过。

## 实例配置

把以下两节写入所选实例的 `compute.toml`。用该 Provider 成功资格检查的摘要替换占位值。省略这两节可保持 Containers 关闭。

```toml
[containers]
broker_socket = "/var/lib/open-compute/run/container-broker.sock"
provider_contract_sha256 = "<qualified-contract-sha256>"
max_instances = 4
max_snapshots = 32
max_snapshot_bytes = 8589934592

[containers.resources]
cpuNanos = 1000000000
memoryBytes = 1073741824
diskBytes = 4000000000
pids = 128
openFiles = 1024
```

socket 应为绝对私有 Unix socket 路径。实例上限不能超过已通过资格检查的 Provider 上限。Provider 缺失、合同摘要不符或原生协议不匹配时，平台拒绝 Container admission。其他 Worker 的入口保留原配置。

## 部署与检查

应用使用固定的项目本地 cf、官方 Cloudflare Vite 插件与 `@cloudflare/containers`。在应用配置中声明 Container class 与镜像。上传前先构建应用；daemon 验证上传模块和不可变镜像引用。

```sh
ocd cf deploy --mode production
ocd --instance dev capabilities --json
```

code-only 部署保留当前 execution。镜像或资源变化会创建新的不可变 target。替换流程等待旧 execution 的正向清理证明。结果未知时，平台保留 execution lease 并继续占用容量。

## 管理边界

已实现的 `/client/v4/accounts/{account_id}/containers` 子集提供以下操作：

- application 列表、创建、读取、修改、删除，以及 `/me` 的账号上限。
- 固定 cf 部署流程所需的镜像准备与 registry credential。
- rollout 列表、创建与读取。
- `instances-v2` 实例列表与单个实例的状态读取。

路由权威是 `openapi/cloudflare-subset-manifest.json` 的 `clientObservedOperations`。`ocd capabilities --json` 投影该清单。这些 cf 观察路由不属于固定上游 OpenAPI 子集。固定官方 SDK 未提供对应方法，因此生成的 `@open-compute/sdk` facade 不暴露 Container 管理方法；生成的 surface report 记录排除项。

平台只提供 immediate rollout 和已实现的 rolling steps。`rollout_active_grace_period` 应为零。rollout 运行期间，新 target 发布返回 HTTP 409；物理配置未变时可复用 target。application 的 `observability.logs` 只保存配置意图，不采集主进程 stdout 或 stderr。Dashboard 未提供独立的 Container 管理页。

## 快照与已接受的差异

容器快照保留文件系统数据，排除易失的 `/run` 数据和注入的 helper mount。恢复会创建新的 execution。目录恢复在主进程启动前验证 archive 并还原路径。本地 `snapshotDirectory()` 创建入口仍为实验功能；九个 stable Directory 字段已有真实本地恢复路径。

两项快照行为采用已接受的上游方向：`snapshotContainer()` 允许省略 options；恢复后的容器返回空的 `inspect().image`。平台不生成 Cloudflare 托管 registry 引用，也不声明已知托管 image-size 公式。[0.4.0 说明](https://github.com/elliothux/open-compute/blob/main/docs/releases/0.4.0.md)记录上游来源、候选检查及尚待执行的正式发布检查。

operator 负责宿主网络过滤。平台不承诺每个 Container 独占虚拟机、多区域 placement，或 Worker 与实例 IP 网络隔离。Engine socket、私有 callback 与管理 API 分别验证权限。
