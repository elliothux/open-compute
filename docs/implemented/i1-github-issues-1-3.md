# I1：GitHub Issues #1–#3

状态：verified，2026-09-05。

| Issue | 结果 |
| --- | --- |
| [#1](https://github.com/elliothux/open-compute/issues/1) | 4 KiB 控制面声明长度限制不再错误覆盖 tenant ingress；tenant body 仍由 workerd 配置预算流式限制。 |
| [#2](https://github.com/elliothux/open-compute/issues/2) | Assets bulk route 使用 64 MiB wire cap，同时保留 50 MiB base64 payload 和 25 MiB 单文件限制。 |
| [#3](https://github.com/elliothux/open-compute/issues/3) | Cron activation generation 从全部持久记录递增，全部 tombstone 后不再复用旧 generation。 |

回归覆盖 tenant fallback 的 Content-Length/chunked 边界、无 Content-Length 的 Assets 超限恢复，以及
Cron 数据库完全重开后的 generation、retry 和 reconcile identity。未新增兼容分支或数据回填。

build、generated、fmt、Clippy、no-default-features、Rust 1.98、metadata 和 dependency boundaries 通过。
Coverage 为 49 targets、1,131/1,131 cases、90.014165%；最终非插桩 Gate 为
49 targets、1,131/1,131 cases，单轮通过。真实 Cloudflare differential 未运行。
