# I139–I196：小缺陷修复、AI Search 上游跟进与 Linux glibc 基线

状态：**planned**。本批覆盖 8 个小缺陷和 2 个专项：
[#196](https://github.com/elliothux/open-compute/issues/196)、
[#195](https://github.com/elliothux/open-compute/issues/195)、
[#158](https://github.com/elliothux/open-compute/issues/158)、
[#189](https://github.com/elliothux/open-compute/issues/189)、
[#192](https://github.com/elliothux/open-compute/issues/192)、
[#191](https://github.com/elliothux/open-compute/issues/191)、
[#190](https://github.com/elliothux/open-compute/issues/190)、
[#160](https://github.com/elliothux/open-compute/issues/160)，以及
[#139](https://github.com/elliothux/open-compute/issues/139) AI Search 上游跟进和
[#194](https://github.com/elliothux/open-compute/issues/194) Linux glibc 基线。

## 顺序

小缺陷分三批提交，每批一个 commit，按表中顺序实施。专项在小缺陷之后开始，两个专项互不依赖。

| 批次               | Issue            | 改动                                  | 受影响的测试                                                |
| ------------------ | ---------------- | ------------------------------------- | ----------------------------------------------------------- |
| A 数据面           | #196、#195       | D1 authorizer；Worker bundle 结构限制 | storage、workers 单元测试；Gate `d1`、`workers`             |
| B 运维面           | #158、#189、#192 | 调度器暂停；`ocd logs`；交互式 setup  | service 单元测试；Gate `scheduler`、`alarms`、`daemon`      |
| C Dashboard 与 CLI | #191、#190、#160 | 登录码；会话吊销；cf launcher         | service 单元测试；Dashboard test 与 build；Gate `dashboard` |
| 专项 1             | #139             | AI Search 上游刷新                    | upstream-review fixtures；Gate `search`、`contract`         |
| 专项 2             | #194             | Linux 构建基线                        | release package Gate；目标发行版冒烟                        |

## 批次 A：数据面

### #196 D1 FTS5 写入被 authorizer 拒绝

FTS5 写入时会执行 `PRAGMA main.data_version`，用来判断 index 缓存是否过期（bundled SQLite 的 `fts5IndexDataVersion`）。
`crates/storage/src/d1/hardening.rs` 的 `authorize_pragma` 只放行 introspection pragma 和 `user_version`/`application_id`，所以 INSERT 和 MATCH 返回 `D1_AUTHORIZER_DENIED`。

- 放行不带值的 `data_version`。它是只读计数器，`authorize` 已经拒绝 `main` 以外的 database。
- 在 `authorizer_blocks_cross_database_internal_and_connection_state_sql` 测试旁增加 FTS5 用例：建表、INSERT、MATCH 返回行、`DELETE`。
- 同一用例断言赋值形式的 pragma 仍被拒绝。

### #195 单个模块上限 8 MiB

`crates/workers/src/bundle.rs` 的 `BundleLimits::DEFAULT` 把 `max_module_bytes` 设为 8 MiB，比 32 MiB 的 `max_total_module_bytes` 更严。
Cloudflare 只限制整个 Worker 的大小，不限制单个模块，所以单文件 bundle 在 8–32 MiB 之间时无法上传。

- 删除 `max_module_bytes`，单个模块只受 `max_total_module_bytes` 约束。调用点在 `bundle.rs`、`worker_cli.rs` 和 `workers_http/v4/multipart.rs`。
- multipart 的单 part 上限改用总模块上限。`MAX_BODY_BYTES` 已经按总模块字节计算，不变。
- 更新 [Cloudflare 兼容矩阵](references/cloudflare-compatibility.md)中的结构限制和 `OC-WKR-LIMIT-001`。
- 用一个 9 MiB 单模块 bundle 做测试，覆盖 multipart PUT、`POST .../versions` 和离线 encoder。

## 批次 B：运维面

### #158 暂停调度器后 DO alarm 修复扫描继续运行

`crates/service/src/scheduler/runner.rs` 的修复分支在调用 `repair_once()` 前不检查暂停状态。
全局暂停和 alarm 暂停都应停止周期扫描。operator 通过 `POST .../scheduler/repair` 手动触发的修复不受暂停影响。

- 周期扫描前检查 `is_paused()` 和 `alarm_paused`。
- 测试：暂停后推进到修复截止时间，断言 transport 没有收到修复调用；恢复后扫描继续。
- 扫描本身会唤醒每个活着的 Durable Object，这是 [#157](https://github.com/elliothux/open-compute/issues/157) 内存增长的根因。#157 需要独立的 alarm 投影方案，不在本批范围内。

### #189 `ocd logs` 不完整

Linux 的 `logs --follow` 直接返回 `PLATFORM_UNAVAILABLE`。macOS 的 launchd plist 没有设置输出文件，`logs` 无内容可读。

- Linux：`--follow` 以 `journalctl [--user] -u <unit> -f --no-pager` 替换当前进程，继承 stdio。非 follow 路径不变。
- macOS：plist 写入 `StandardOutPath` 和 `StandardErrorPath`，指向 instance 状态目录下的固定日志文件。`logs` 读取最后 100 行，`--follow` 持续读取追加内容。
- 日志文件的轮转和保留属于 [P25](p25-platform-follow-ups.md) 的 operator logger 合同。本批只接通输出路径。
- 测试：生成的 plist 含两个路径；`logs` 读取固定文件；Linux follow 的命令参数。

### #192 交互式 setup 选择 s3 后报错

`crates/service/src/setup.rs` 的提示允许输入 `s3`，随后报 `CONFIG_INVALID`。

- 交互式 setup 只提供 local backend，提示文字不再列出 s3。
- `ocd setup` 文档写明 S3 通过编辑 `compute.toml` 配置，并链接现有 S3 配置说明。
- 交互式收集 S3 参数是后续需求，不在本批范围内。

## 批次 C：Dashboard 与 CLI

### #191 Dashboard 关闭时 control socket 仍签发登录码

`crates/service/src/instance_control.rs` 处理 `DashboardLoginCode` 时不检查 `dashboard.enabled`。签出的登录码无法使用，因为会话接口已经拒绝。

- Dashboard 关闭时返回失败响应，`ocd dashboard` 输出 Dashboard 已关闭和启用方法。
- 测试：关闭时没有签发登录码，开启时行为不变。

### #190 退出登录不吊销服务端会话

Dashboard 退出登录只清除 `sessionStorage`，服务端会话在 TTL 内仍然有效。这一项防的是复制或泄漏的会话 token 在用户退出后继续使用。

- 新增 `DELETE /operator/session`。它和现有会话接口一样先校验 `same_origin_csrf`，再对请求携带的会话 token 调用 `DashboardAuth::revoke_session`。无效或已过期的 token 也返回成功，不泄漏会话状态。
- Dashboard 的退出操作先尽力调用该接口，再清除本地状态。
- 测试：吊销后同一 token 访问被拒；缺少同源头时返回 CSRF 拒绝。

### #160 `ocd cf` 把 shell shim 当作 JavaScript 执行

`crates/service/src/cf_launcher.rs` 把 `node_modules/.bin/cf` 直接交给 node 执行。部分包管理器生成的是 POSIX shell shim。

- 改为读取 `node_modules/cf/package.json` 的 `bin` 声明，解析出 cf 包内的 JavaScript 入口，再用 node 执行。
- 入口必须位于 cf 包目录内，规范化后不得越界，与现有 project-local 校验一致。
- 版本检测和 exec 使用同一个入口。测试覆盖 symlink、shell shim 和越界的 `bin`。

## 专项 1：AI Search 上游跟进（#139）

刷新扫描显示为 breaking，原因是路径参数改名，并不是接口被删除。上游把 `/ai-search/namespaces/{name}/instances/**` 的路径参数改成了 `{namespace}`。
`test/upstream-review/scanner.ts` 按路径原文匹配已选接口，所以 22 个已选接口被报告为 removed。只要存在 removed 接口，扫描器就不再计算 changed 接口。

本地对比基线 OpenAPI `780de88d0324` 和候选 OpenAPI `1e7d44922a8a`，展开 `$ref` 后，这 22 个接口都有真实的合同变化：

- 实例 create、update、get 的请求体和响应体新增约 30 个字段。
- 错误响应新增 400、402、409、429、502、503，部分带 `Retry-After`。
- 列表接口的查询参数变化，包括 items 过滤枚举和分页上下限。
- 实例的 `PATCH` 和 `purge_cache` 在两个快照中都存在，但当前子集没有选入。

实施步骤：

1. 扫描器按“方法 + 参数名归一化后的路径”匹配接口，并输出参数改名。增加参数改名的 fixture，覆盖 ready、blocked、breaking 三种分类。
2. 重新扫描，得到 changed 接口和 SDK、cf 的闭包结论。按 [Cloudflare 上游刷新](references/cloudflare-upstream-refresh.md)的规则，官方 SDK 7.3.0 未表达的字段保持 blocked。
3. 子集和 manifest 采用新的路径模板。服务端路由按参数位置匹配，不受参数名影响。
4. 逐个实现新增的请求字段和错误响应。无法在本地实现的字段记录为偏差，不静默忽略。
5. 决定是否选入 `PATCH` 实例和 `purge_cache`。选入时按当前 Cloudflare 行为实现。
6. 刷新 OpenAPI lock、SDK 生成面和兼容矩阵，关闭 #139。cf `1.0.0-beta.21` 的版本变化同时经过 cf 选定面复核。

## 专项 2：Linux glibc 基线（#194）

当前 Linux 版本要求 glibc 2.38 以上。Rocky Linux 9 和 RHEL 9 只有 glibc 2.34，安装脚本会在版本检查时失败。
workerd fork 在 Ubuntu 24.04 runner 上用 Bazel `release_linux` 和 LLVM 22 构建，ocd 也在 Ubuntu 24.04 上编译。
报错同时包含 `GLIBC_2.38` 和 `GLIBCXX_3.4.31`，说明 C 库和动态链接的 libstdc++ 都需要处理。

需要先确定目标基线：

| 基线       | 覆盖                                       | 代价                                 |
| ---------- | ------------------------------------------ | ------------------------------------ |
| glibc 2.34 | RHEL/Rocky/Alma 9、Ubuntu 22.04、Debian 12 | 构建容器需要提供 2.34 sysroot        |
| glibc 2.28 | 另含 RHEL 8 系和 Debian 10                 | 工具链更旧，workerd 构建需要更多适配 |

贡献者 fork 的 `upgrade/upstream-r22-glibc228-*` 分支已在 AlmaLinux 8 容器中用 manylinux GCC 工具链加 Clang 构建并验证了 glibc 2.28 的 workerd 和 ocd，并改用了系统 malloc。实施前复核这条路径和改用系统 malloc 的原因。

实施步骤：

1. 在 workerd fork 的发布流程中，使用目标基线的构建容器或 sysroot 构建 Linux x64 和 arm64 archive。Bazel 配置的改动按 [workerd 路线](workerd/README.md)记录移除条件。
2. ocd 在同一基线上编译，静态链接 C++ 运行时，消除对系统 libstdc++ 的版本要求。
3. 发布流程增加符号检查：`ocd` 和 workerd 引用的最高 `GLIBC_` 版本不得超过目标基线，超过时构建失败。
4. 在目标发行版容器中执行安装和 daemon 冒烟测试。
5. `install.sh` 在下载前比较本机 glibc 和发行物基线，低于基线时输出明确的错误。
6. 更新 `packages/runtime/workerd.lock.json` 的正式 pin，以及[单二进制分发](references/single-binary.md)和安装文档中的平台要求。

更换正式 runtime pin 需要先取得维护者批准。新 pin 只写入 lock 文件，不提交二进制。

## 完成后

本页精简后移入 `docs/implemented/`，只保留目标、关键决策、最终结果和已接受的限制。
各 issue 在对应的提交合入 `main` 后关闭，评论中写明 commit 和执行过的检查。
