# P0.1：平台基础

> 状态：已实现；按既有 [P0.1 回归记录](./p0-5-r2.md)归档，本次未重跑验收。

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- 一个 `ocd` 独占 data directory，拥有 SQLite authority、master key、HTTP listeners 和受监督的 workerd child。
- 启动顺序保持配置与密钥验证、目录锁、数据库身份验证、对象后端 preflight、runtime 物化、child readiness、流量准入。失败不开放 readiness。
- 正式 workerd pin、archive/binary 摘要和静态 runtime assets 必须匹配；启动离线，内置 archive 的物化路径由 data directory 独占。
- Artifact 和 cache 入场前校验内容摘要，写入采用受控 staging、原子提交和权限／路径检查。
- Child 生命周期包含控制通道、HTTP readiness、进程组、有限日志、退避、强制终止与回收。PID 操作必须验证进程身份。
- 内部 listener 只在 loopback，generation capability 不出现在 argv、环境、日志或租户响应中。
- live 表示进程存活，ready 表示准入；后端故障不通过重启循环掩盖。

## 源码入口

- [`crates/runtime/src`](../../crates/runtime/src)
- [`crates/storage/src`](../../crates/storage/src)
- [`crates/artifacts/src`](../../crates/artifacts/src)
- [`crates/service/src/run.rs`](../../crates/service/src/run.rs)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p0-5-r2.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
