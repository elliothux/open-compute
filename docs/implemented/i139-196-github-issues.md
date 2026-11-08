# I139–I196：小缺陷修复、AI Search 上游跟进与 Linux glibc 基线

状态：**implemented、verified**（2026-11-08）。本批覆盖
[#196](https://github.com/elliothux/open-compute/issues/196)、
[#195](https://github.com/elliothux/open-compute/issues/195)、
[#158](https://github.com/elliothux/open-compute/issues/158)、
[#189](https://github.com/elliothux/open-compute/issues/189)、
[#192](https://github.com/elliothux/open-compute/issues/192)、
[#191](https://github.com/elliothux/open-compute/issues/191)、
[#190](https://github.com/elliothux/open-compute/issues/190)、
[#160](https://github.com/elliothux/open-compute/issues/160)、
[#139](https://github.com/elliothux/open-compute/issues/139) 与
[#194](https://github.com/elliothux/open-compute/issues/194)。当前支持面以
[Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md)和源码为准。

## 实现结果

### 数据面（e96a2d665）

- #196：D1 authorizer 放行不带值的 `PRAGMA data_version`。FTS5 写入用它判断 index 缓存是否过期。赋值形式和 `main`
  以外的 database 仍被拒绝。
- #195：删除单个模块的 8 MiB 上限。单个模块只受 32 MiB 的 Worker 总模块上限约束，与 Cloudflare 一致。multipart、
  `POST .../versions` 和离线 encoder 使用同一上限。

### 运维面（ae8406903）

- #158：全局暂停或 alarm 暂停时，周期 DO alarm 修复扫描停止。operator 手动触发的修复不受暂停影响。
- #189：Linux `ocd logs --follow` 以 `journalctl -f` 替换当前进程。macOS launchd plist 把 stdout 和 stderr 写入 scope
  根目录下的 `ocd.log`；`logs` 读取最后 100 行，`--follow` 使用 `tail -F`。
- #192：交互式 setup 只提供 local backend。S3 通过编辑 `compute.toml` 配置。

### Dashboard 与 CLI（ed4c90efe）

- #191：Dashboard 关闭时 control socket 不签发登录码，`ocd dashboard` 返回 `DASHBOARD_DISABLED`。
- #190：新增 `DELETE /operator/session`，先校验同源 CSRF，再吊销请求携带的会话。无效 token 也返回成功。Dashboard
  退出登录先尽力调用该接口，再清除本地状态。
- #160：`ocd cf` 读取 `node_modules/cf/package.json` 的 `bin`，在 cf 包目录内解析 JavaScript 入口后用 node 执行。
  版本检测和 exec 使用同一个入口。

### AI Search 上游跟进（7f701fb34、b0a7aa9ad）

- 扫描器按“方法 + 参数名归一化后的路径”匹配接口，并输出 `renamedOperations`。上游把 instance 路径参数改为
  `{namespace}`，原来被误报为 removed 的 22 个接口改为按 changed 处理。
- 固定组合刷新为 OpenAPI `1e7d44922a8a`、`cloudflare@7.3.0` 与 cf `1.0.0-beta.21`。
- 选入 instance `PATCH`（把 instance 移到另一个 namespace）和 `purge_cache`。移动由 control migration `V25` 的触发器
  保证引用一致。本地不缓存搜索结果，purge 校验 instance 后直接成功。官方 SDK 7.3.0 没有这两个方法，所以它们不进入
  SDK surface。
- 新增官方错误：429 `7119`（本地容量饱和）、502 `7120`（模型 provider 失败）、503 `7125`（暂时不可用）。429 和 503
  带 `Retry-After`。
- 10 个 Container 管理路由进入官方 OpenAPI 与 SDK，改为正式选入，继续使用 `OC-CONTAINERS-MGMT-001`。

### Linux glibc 2.34 基线（64db3358d、8fab31610、201c240f0、5df211550）

- 目标基线为 glibc 2.34，覆盖 RHEL/Rocky/Alma 9、Ubuntu 22.04 和 Debian 12。
- workerd fork R26（`8c3699444`）在 manylinux_2_34 容器中构建 Linux archive：kernel.org LLVM 22.1.8，在容器内重建的
  静态 libc++ 与 LLVM libunwind，gcc-toolset-14 提供 crt 和 libgcc。AlmaLinux 9 把 `_dl_find_object` 以 `GLIBC_2.35`
  回移，`libgcc_eh` 会调用它，所以链接时把 LLVM libunwind 放在 `libgcc_eh` 之前。移除条件记录在
  [workerd 路线](../workerd/README.md)。
- ocd 用 cargo-zigbuild 链接到 `<target>.2.34`。
- `scripts/glibc-baseline.ts` 在打包和组装发布时检查 `ocd` 与 workerd 引用的最高 `GLIBC_` 版本，并拒绝动态
  libstdc++。`release.json` 记录 `glibc`。
- `install.sh` 在下载前比较本机 glibc 与发行物基线，低于基线时明确失败。

## 验证

- 每批提交前，受影响的 Gate 均已通过：`d1`、`workers`、`scheduler`、`alarms`、`daemon`、`dashboard`、`search`、`recorded`、
  `contract`、`cf-cli`、`cloudflare-sdk`。
- 固定 R26 后（`5df211550`、`299af27f7`），以下检查通过：
  - Gate `runtime`、`contract`、`daemon`、`dashboard`、`browser`、`cf-cli`；
  - `bun run test:js`（644 项）；
  - service lib（1008 项）与 storage lib（440 项）；
  - `cargo fmt --all --check` 与 `./test/check-rust-clippy.sh`。
- workerd fork 发布 run [42421490269](https://github.com/elliothux/workerd/actions/runs/42421490269) 的四个构建和 publish
  均通过。两个 Linux workerd 引用的最高版本为 `GLIBC_2.34`，且不依赖 libstdc++。
- cargo-zigbuild 构建的 linux-arm64 ocd 内嵌 R26 workerd，最高引用 `GLIBC_2.34`，且不依赖 libstdc++。
  `test/distro-smoke/run.py` 用该 ocd 执行安装、daemon、setup 和 Worker 部署：
  - Rocky Linux 9（glibc 2.34）与 Ubuntu 22.04（glibc 2.35）返回 `distro-smoke-ok`；
  - Rocky Linux 8（glibc 2.28）被 `install.sh` 明确拒绝。
- 未执行：
  - linux-x64 ocd 的发行版冒烟（需要 CI 或模拟环境）；
  - `release-dry-run.sh` 与 CI package job；
  - Containers fixture 的 `cf build`（需要预先准备的 P23 镜像输入）。

## 已接受的限制

- 官方 SDK 7.3.0 未表达的新增字段（KV `mode` 回显、Workflows jurisdiction 冲突、Observability `$metadata` 时间字段、
  AI Search 音视频 chunk 字段）本地不产生。它们在 schema 中都是可选字段，或是本地不会出现的错误。
- cf `1.0.0-beta.21` 新增的 `access` 配置字段按 `unsupported` 失败关闭。
- macOS `ocd.log` 不轮转。日志轮转和保留属于 [P25](../p25-platform-follow-ups.md) 的 operator logger 合同。
- glibc 2.28（RHEL 8 系）不在基线内。
- 扫描本身唤醒每个活着的 Durable Object（[#157](https://github.com/elliothux/open-compute/issues/157)），需要独立的
  alarm 投影方案。

各 issue 在对应提交推送到 `main` 后关闭。
