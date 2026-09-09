# P12：Wrangler 项目开发与部署体验

状态：**implemented / Implementation GO（2026-09-09）**。

## 用户结果

- `ocd target add/list/show/test/remove` 管理显式远程目标。每个目标只保存规范化 API base URL、公开
  account ID 和外部 deployer token 文件引用；list/show/JSON 不读取或输出 token。
- `ocd wrangler` 按 `--target`、`--instance`、`--config` 或 P11 的本机 0/1/N 规则选择唯一执行
  目标，从项目目录向上解析最近的 `node_modules/.bin/wrangler`，并要求版本与目标 capabilities 公布的
  精确 pin 一致。
- launcher 只注入标准 Cloudflare API base、token、account 变量和关闭 telemetry/error reporting 的
  Wrangler 变量；冲突的现代与旧式 Cloudflare 凭据变量全部移除。
- Wrangler command 起的 argv 保持 opaque；launcher 不解析项目配置、改写输出、翻译错误、重试 mutation、
  使用全局 Wrangler 或触发 package manager 下载。Unix 使用 process replacement，保留 TTY、signal 和
  child exit status。
- 本机执行目标只使用已验证 instance descriptor/config 的 admin surface。远程目标要求 HTTPS，只有
  loopback 可使用 HTTP；不提供 `--insecure` 或 Cloudflare fallback。
- 项目继续使用标准 `wrangler.jsonc`、Wrangler environments 和
  `.wrangler/deploy/config.json` generated-config redirect。日常开发仍用本地 `wrangler dev`；
  真实 dev/staging/production 集成用显式 instance 或 target。
- 示例项目提供固定 Wrangler 版本、本地开发、dev/staging/production 部署、tail 和 GitHub/GitLab CI
  模板。CI 可直接使用三个标准 Cloudflare 环境变量，不必创建开发机 target。

## 持久边界

- target registry 是 bounded、owner-only 的 per-user TOML，使用 mutation lock、atomic write 和 fsync；
  未知 schema、重复 authority、symlink、错误 owner/mode、宽松目录权限及不安全 token 文件均 fail closed。
- token 文件必须是绝对路径、当前用户拥有、权限精确为 `0600`、no-follow、bounded 的普通文件。
  `target remove` 只删除 registry record，不删除外部 token。
- generation descriptor 直接包含当前 Cloudflare-compatible account ID；没有旧 descriptor 兼容读取、
  双 schema 或 backfill。
- 旧 `oc deploy` 在线 shim 已删除；`oc` 只保留离线 build/types，`ocd wrangler` 是唯一的人类
  wrapper，标准 Wrangler 环境变量仍是自动化的底层接口。

## 验证

- 固定 Wrangler `4.127.1` 的真实 Gate 覆盖三个独立项目、dev/staging 两个 environment、generated
  config、deploy、secret、KV、实时 tail、Version／单 Version 100% Deployment、项目隔离以及 daemon
  PID 不变。
- 独立 P12 进程测试覆盖 target lifecycle、URL/account/token 安全、opaque Unicode/空参数、hoisted
  executable、环境清理、版本失败、SIGTERM process replacement 与 exit code 透传。
- `bun run build`、frontend/source policy、TypeScript、Oxlint/Knip、JS tests、Rust format/clippy、
  no-default-features、Rust 1.98 MSRV、metadata 和 dependency boundaries 均通过。
- workspace line coverage 为 **90.25%**；instrumented workspace Gate 通过。
- 最终单轮 workspace Gate 通过 **51 targets / 1402 cases**：
  `.temp/gate-run/20260909T040644-423f0335/report.json`。

## 接受的限制

- 不实现 `wrangler dev --remote`、自动保存上传、managed CI、global Wrangler fallback、自动安装或
  multi-version percentage rollout。
- Wrangler environment 只提供项目级命名与资源隔离；需要独立 data-dir、listener、object authority 或
  故障域时必须使用不同 instance/target。
- 真实 systemd／launchd、跨机器公网 TLS 部署和正式 release 安装资格不由 P12 本地 Gate 代替。
