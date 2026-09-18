# P17：宿主子进程管理基础设施

状态：**implemented（2026-09-17）**。

P17 将宿主子进程的安全启动和回收原语收敛到 `open-compute-runtime`，并把 Xberg 文档解析 child 迁移到该 crate 的短任务接口。它不改变
[Host authority](../references/host-authority.md)，也不为尚未存在的 Gateway、Extension Provider 或 Browser child 预建 Manager。

## 当前合同

`VerifiedLaunchImage` 持有由产品 authority 预先打开并验证的 executable；通用层不搜索 `PATH`、不下载 runtime，也不重新选择版本。
`HostProcessSpec` 明确给出参数、完整环境、working directory、stdin、deadline 以及 stdout/stderr 上限。`run_host_process` 的短任务
owner 负责：

- `env_clear()` 后只注入声明的环境变量；
- 从已打开的 executable identity 启动独立 process group；
- 并行写入 stdin 并有界读取 stdout/stderr；
- deadline、取消、输出 overflow 和 owner 失败后的 TERM/KILL/reap；
- 返回脱敏、定长的 `BoundedOutput`，不公开 raw child 或可任意 signal 的 PID handle。

workerd 的 verified-fd、lease、orphan fencing、readiness 和 generation restart 仍由 `WorkerdSupervisor` 拥有。当前短任务使用
`process/execution.rs` 的 `owner_wait`；常驻 workerd 使用 `supervisor/owner.rs` 的 `ChildHandle`/`owner_loop`。两条路径共享
verified-exec、process-group 与 signal/reap 原语，不应描述为已经统一的单一 owner loop 或已交付的通用常驻 child API。

workerd 的 `LogCollector` 持续读取并保留有界日志 tail，不按生命周期累计日志量终止进程。Xberg 保留 OCDP、CPU/address-space
rlimit、一次一进程和稳定错误码，只删除产品层重复的 Tokio spawn、pipe reader、drop guard 和 kill/reap 实现。

## 容量与 ownership

当前产品只有一个正式 workerd generation 和短命 Xberg child：workerd 的单 generation 约束由 supervisor 执行；Xberg 的全局、account
和 Version semaphore 继续由 `DocumentParserBindingService` 执行。因此没有增加一个只转发现有两个上限的 speculative Process
Coordinator。后续 child 必须复用验证执行与单 child 单 owner 的基础原语，不能在产品层另写一套底层生命周期；只有出现实际跨产品
FD/child 竞争时，才在 composition root 增加共享总预算。一个可选 Caddy 不构成新增全局调度器的理由。

每个产品继续拥有自己的协议和状态机：

- workerd：control-fd listen、HTTP readiness、generation fencing、restart 与 orphan lease；
- Xberg：单个 OCDP frame、rlimit、deadline 和不重试的转换结果；
- 后续 Gateway/Provider/Browser：各自的 readiness、session 和 restart，不抽象成 `Supervisor<Policy>`。

## P18 接入边界（待实现）

[P18](../p18-single-domain-public-gateway.md) §6.3 拥有 Caddy 常驻 child 的新增合同：在现有 runtime/workerd owner 基础上提取必要的
受控启动/停止接口，保留短任务执行与常驻日志两种语义，保持 `ExecImage`/FD/staging 到回收完成；不复制第三套进程实现，
也不把 `WorkerdSupervisor` 改成万能 `Supervisor<Policy>`。

Caddy 的配置、TLS readiness 与 crash backoff 归 `GatewayManager`，证书/私钥归 Caddy；`service` 显式协调依赖和关闭顺序。
这些接口、Caddy lease recovery 与验收属于 P18 待实现范围，不计入下面的 P17 已完成验证，也不需要预建 Process Coordinator。

## 验收覆盖

runtime 定向回归覆盖显式 cwd、清空环境、stdin 完整传输、stdout 内容、stderr cap/overflow 和 overflow 后及时回收。既有 runtime suite 继续覆盖
deadline、取消、leader/descendant 回收、reader/wait failure、lease identity、PID/PGID 校验和 macOS verified-fd staging。parser 回归覆盖
spawn/input/output/timeout/exit/resource-signal 分类以及稳定公开错误映射。

实现没有新增依赖、第二套 supervisor、兼容 wrapper 或运行时下载路径。

返回[完成索引](README.md)。
