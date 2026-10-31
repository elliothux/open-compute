# P12：项目 target 管理与历史 Wrangler 验收

状态：implemented；原 CLI/项目配置实现已由 [P20 Cloudflare CLI 单轨迁移](p20-cf-cli-migration.md) 替代。

## 当前职责与持久边界

- `ocd target add/list/show/test/remove` 管理显式远程目标。目标只保存规范化 API base URL、公开 account ID 和外部
  deployer token 文件引用；list/show/JSON 不读取或输出 token。
- target registry 是 bounded、owner-only 的 per-user TOML，使用 mutation lock、atomic write 和 fsync；未知 schema、
  重复 authority、symlink、错误 owner/mode、宽松目录权限及不安全 token 文件均 fail closed。
- token 文件必须是绝对路径、当前用户拥有、权限精确为 `0600`、no-follow、bounded 的普通文件；`target remove`
  只删除 registry record，不删除外部 token。
- 本机目标使用已验证 instance descriptor/config 的 admin surface；远程目标要求 HTTPS，只有 loopback 可使用 HTTP，
  不提供 `--insecure` 或 Cloudflare fallback。独立 data-dir、listener、object authority 或故障域须使用不同 instance/target。

当前用户入口为 `ocd cf`，项目配置、mode、官方 Vite 构建、Build Output、CLI 进程透传与版本诊断统一见 P20。
原 `ocd wrangler`、旧配置读取与 generated-config redirect 已移除，不保留 launcher alias 或客户端品牌分支。
终端流式 tail 尚无等价 cf 入口，当前使用 Dashboard Live Tail。

## 历史验证（2026-09-09）

- 认证基准 Wrangler `4.127.1` 的真实 Gate 覆盖三个独立项目、dev/staging 两个 environment、generated
  config、deploy、secret、KV、实时 tail、Version／单 Version 100% Deployment、项目隔离以及 daemon
  PID 不变。
- 独立 P12 进程测试覆盖 target lifecycle、URL/account/token 安全、opaque Unicode/空参数、hoisted
  executable、环境清理、版本失败、SIGTERM process replacement 与 exit code 透传。
- `bun run build`、frontend/source policy、TypeScript、Oxlint/Knip、JS tests、Rust format/clippy、
  no-default-features、Rust 1.98 MSRV、metadata 和 dependency boundaries 均通过。
- workspace line coverage 为 **90.25%**；instrumented workspace Gate 通过。
- 最终单轮 workspace Gate 通过 **51 targets / 1402 cases**：
  `.temp/gate-run/20260909T040644-423f0335/report.json`。

## 验收适用范围

上节保留原客户端版本、case 数与报告身份，不表示当前 cf 执行了旧 Wrangler Gate。P20 记录新的真实 cf 与进程测试验收；
当前 Gate target/case 以 [测试规则](../references/testing.md) 和 `test/gate_cases.py` 为准。
真实 systemd／launchd、跨机器公网 TLS 部署和正式 release 安装资格不由历史 P12 本地 Gate 代替。
