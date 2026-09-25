# P18：单域名公网网关、DNS 与 TLS

状态：**implemented locally**（2026-09-22）。真实公网 DNS/ACME 资格见 [P18 验收计划](../acceptance/p18-single-domain-public-gateway-acceptance.md)。

## 用户结果

可选 Gateway 为一个 instance 配置一个 operator 控制的 `base_domain`。Worker 保留 `.localhost` 本机入口，并可获得 `<name>.<base_domain>` 公网 HTTPS 入口；后续公开产品使用固定 product namespace。公网能力尚未资格化时只省略 public endpoint，不删除本机 route 或持久化 claim。

一个 `ocd` daemon 管理共享 Caddy child、私有 admin Unix socket、固定 challenge DNS provider、配置快照和所有已登记 instance 的平台 routes。operator 可通过标准 Caddyfile `import` 追加自有站点，不需要学习另一套代理 DSL。

## Authority 与失败语义

- hostname claim、typed route、Gateway domain/namespace 和 TLS readiness 持久化在平台 authority；新增公网 binding 必须同时满足 active namespace 与当前 Caddy PID 的 TLS 资格。
- 托管目录 `gateway/{run,storage,config-state}` 和生成文件使用受限权限并拒绝符号链接。
- 配置由 `GatewayManager` 完整生成、验证和原子 reload；失败时保留最后已确认配置。Caddy crash 由统一 host-process supervision 退避恢复。
- DNS-01 provider 只服务固定 challenge zone。TXT mutation 使用精确 zone/value 和有界 cleanup，不能修改 operator 的业务 zone。
- 平台 HTTPS 请求只经 tenant-only router 和指定 Caddy PID 的私有 upstream 进入 Worker；外部请求不能注入可信 scheme 或内部 identity。

## Operator surface

- `ocd caddy version|list-modules|fmt|validate|reload|status`
- `ocd config gateway-dns-plan`
- `ocd config gateway-challenge-probe`
- `ocd config gateway-dns-verify`
- `ocd config gateway-tls-probe`

DNS verify 检查公网递归结果、父区委派、CAA 和 challenge authority；TLS probe 使用固定 Mozilla roots 并保留 SNI。`doctor` 和 `caddy status` 报告 pin、child、配置摘要、DNS、TLS 与最近 reload 状态。

## 构建与验证记录

Caddy 2.11.4 的 Go module graph、module inventory、四平台 binary 和摘要由正式 lock 固定并嵌入单一发行 executable。2026-09-22 的定向测试覆盖配置解析、route/claim 事务、DNS fixtures、provider rollback、Caddyfile 组合、热重载、快照恢复、child supervision 和 CLI。固定 Rust 1.98/Ubuntu builder 的断网 final layer 启动了真实 `ocd` 与内嵌 Caddy，并确认退出后没有 Caddy/workerd 残留。

本地验证不等于公网证书签发；当前支持声明必须继续标注验收计划中的外部边界。
