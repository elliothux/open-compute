# I117、I130、I132：网络、R2 preflight 与 Cloudflare 上游刷新

当前 CLI/应用配置与构建入口由 [P20](p20-cf-cli-migration.md) 拥有：`ocd cf`、`cloudflare.config.ts`、官方 Vite 插件 v2 与 Build Output。
本文旧客户端命令、版本、fixture 与 PASS 保留为当时证据，不要求恢复旧入口，也不证明当前 cf 的资格。

状态：**implemented、verified**（2026-09-30）。本批覆盖
[#117](https://github.com/elliothux/open-compute/issues/117)、
[#130](https://github.com/elliothux/open-compute/issues/130) 与
[#132](https://github.com/elliothux/open-compute/issues/132)。当前支持面仍以
[Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md)和源码为准。

## 实现结果

- 普通 tenant outbound 统一使用 workerd 原生 `Network(allow = ["network", "local"], deny = ["unix", "unix-abstract"])`，覆盖公网、私网、loopback、link-local 与 metadata IP；Unix socket 保持不可达。open-compute 不提供实例级 CIDR/端口防火墙，operator 通过宿主防火墙、network namespace、容器或 VM 管理地址策略。
- runtime-source 等平台内部端点在读取请求体前校验 generation token 与启动代次；网络可达性不再承担鉴权。外部冲突内部 header 仍由公开入口覆盖或移除。
- R2/S3 启动 preflight 的非末尾 multipart part 使用 S3 规定的 5 MiB 下限，末尾 part 保持小体积。SSE-C multipart ETag 作为 provider opaque value 处理；完成后验证精确大小和解密字节，不再错误假设它等于明文 MD5。
- Cloudflare upstream workflow 的 step output、classification 与 Draft PR 条件恢复为同一 authority。当时的 Wrangler candidate scanner 在不执行候选包代码的前提下区分 compatible、blocked 与 breaking drift；当时固定组合为 Wrangler 4.143.0、Cloudflare SDK 7.2.0。
- CI 增加受保护的 `s3-provider-qualification` 与 `ai-search-qualification`。它们只在 trusted `main` push 或手动 dispatch 运行；PR 和普通 workspace Gate 继续使用确定性本地 fixture，不接收 provider secrets。

## 验证

- 专用 Cloudflare R2 bucket 的生产 S3 preflight Gate 通过，记录为 `.temp/gate-run/20260930T004747-87ed45bc/report.json`；资格测试创建的对象和 multipart upload 已清理。
- 本地完整 AI provider Gate 通过，记录为 `.temp/gate-run/20260930T010602-efa11527/report.json`；三条端到端流程复用生产 embedding、rerank、Chat 与 AI Search pipeline。
- `bun run build`、JavaScript tests、format、Clippy、no-default-features、Rust 1.98 MSRV、metadata、dependency boundaries 与文档检查通过。
- Rust workspace line coverage 为 90.05%。最终单轮 `./test/gate.py --workspace` 的 54 个目标全部通过，记录为 `.temp/gate-run/20260930T020551-5695c05c/report.json`。

本次只完成本地验收，没有触发 GitHub Actions；远程 qualification 留给 `main`/发布流程独立执行。
