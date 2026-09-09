# P2.6：单二进制分发

状态：implemented；darwin-arm64 本机验收完成，2026-08-28。正式发布、签名和其他平台资格不在本记录内。
持续构建与部署合同见[单二进制参考](../references/single-binary.md)。

## 结果与边界

- 一个原生 `ocd` 内嵌目标 workerd archive、formal lock、Cap'n Proto 模板、system Workers、
  默认配置、许可证和运维手册。生产启动不依赖 Bun、Node、Rust、源码 checkout 或 `PATH`。
- 编译和启动均校验目标、摘要、大小、manifest、version 和 compatibility input。
  runtime 在 data-dir 排他锁下以私有 staging、fsync 和原子发布物化；缓存损坏直接拒绝，不下载或自修复。
- 旧 `runtime.binary`、`runtime.lock_file`、`runtime.assets_dir` 和外部 runtime 分发路径已删除。
- workerd 仍是由 `ocd` 完整监督的子进程；单文件分发不改变 readiness、重启、孤儿回收和身份验证边界。
- 配置、业务数据、master key、凭据和 S3 服务由部署者提供，不进入发行物。

## 本机证据

目标为 `aarch64-apple-darwin`。本地 `platformd 0.1.0` 为 49,175,504 bytes，
SHA-256 `40d27fbadbdb6429edc41b4fc6ac51121a340c3e52a9c987bb98159651f6cf92`；
其 `git_revision=unknown`，不能作为正式发行身份。内嵌 workerd 为 `v1.20260826.1`。

fmt、diff、Clippy、no-default-features、Rust 1.98、metadata、dependency boundaries、typecheck、
generated 和 44/44 JS tests 通过。Rust 行覆盖率为 56,308/62,424（90.202486%）。
release 单文件隔离测试 2/2 通过，覆盖空环境首启、锁、重启、孤儿恢复和损坏缓存拒绝；
内嵌 11 份手册和两个 license 与源文件逐字节一致。

较早运行暴露冷启动观察窗口和 macOS XProtect 延迟，未通过停用安全机制或放宽生产超时规避。
原始失败证据保留在 `.temp/`。Linux、macOS x64、容器、Linux 特权 egress、Developer ID 签名、
公证、正式 package、上传和部署均未验证。
