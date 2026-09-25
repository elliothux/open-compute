# R1：单 OCD daemon、多 Instance 与单一身份重构

状态：**implemented**（2026-09-24）。剩余跨平台与真实网络资格见 [R1 验收计划](../acceptance/r1-single-daemon-instances-acceptance.md)。

## 用户结果

- 一个 `ocd` daemon 持有所选 OCD 作用域的共享 listener、Gateway、作用域锁和 instance registry。
- 每个已登记 instance 分别拥有显式 config、data dir、InstanceId、SQLite authority、object authority、凭据、缓存和受监督的 workerd/Provider 子进程。
- Cloudflare `/client/v4` 协议继续使用 `account_id` wire 名称；open-compute 私有模型、CLI、Dashboard 和内部 transport 使用 `instance_id`／`instanceId`。
- `ocd instance setup|add|start|stop|restart|remove` 管理登记项；`remove` 保留配置和数据。运行时不扫描未登记配置，也不从 cwd、HOME/XDG 或 `/etc/open-compute` 猜测实例。

## Authority 与生命周期

`<OCD_DIR>/ocd.toml` 保存 daemon 作用域的共享配置，并显式登记每个 instance 的 config 路径和 autostart intent；registry entry 不复制 identity 或 data path。配置中的 `[data].path` 是该 instance 唯一数据根 authority；数据根不能重叠或落入共享 cache/run 目录。共享控制 socket 位于受限的 `<OCD_DIR>/run/`，在线 mutation 通过 owner-only socket 串行化，外部修改 manifest 后拒绝覆盖。面向用户的职责总览见[架构与职责边界](https://open-compute.dev/docs/ocd/architecture/)。

daemon 可以同时启动多个 instance。Host、凭据和可信 InstanceId 路由共同隔离管理请求、Worker ingress、资源、调度、observability 和运行时 transport。停止或崩溃一个 instance 不影响其他 instance；daemon 恢复时只启动 autostart instance，并核验、回收各自旧子进程。

平台 SQLite authority 已收敛到唯一 `instance_identity`，私有存储和 AAD 不再保留账户别名。旧私有 identity/schema/envelope 不是兼容输入，遇到错属、混合或损坏状态时失败关闭，不改写原数据。已发布 migration bytes 未修改。

## 安全边界

- 外部请求不能通过伪造 instance header 改变可信路由；实例凭据只能发现和操作所属 instance。
- Provider session、workerd loader、DO、Queue、Cache、Images、AI 和 observability transport 都核验 InstanceId 与 generation。
- 每个 instance 的 secrets、data root、S3 prefix、runtime leases 和资源 ID 独立；共享 daemon 级并发/指标额度只在声明为共享的地方共享。
- setup、restore、cache cleanup 和 system service 操作复用相同作用域锁、路径 containment、owner/mode 与 no-follow 检查。

## 验证记录

2026-09-24 的本地冻结验证完成 `bun run build`、Rust format/Clippy、no-default-features、Rust 1.98 MSRV、metadata、依赖边界、coverage 和单轮 workspace Gate。正式 pinned workerd 下，instrumented 与最终非插桩 Gate 均为 53/53 targets、1677/1677 cases；记录分别位于 `.temp/gate-run/20260924T223403-deb8b3e0/` 和 `.temp/gate-run/20260924T232214-ccd9dcbc/`。macOS system launchd 隔离验收验证了非 root daemon、权限、控制 socket、停止和清理。

历史 PASS 只证明上述输入。当前行为以源码、[CLI 文档](https://open-compute.dev/docs/cli/)和维护中的兼容合同为准。
