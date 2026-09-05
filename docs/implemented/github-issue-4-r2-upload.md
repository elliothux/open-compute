# GitHub Issue #4：R2 上传调度与大文件验收

状态：Completed，2026-09-05。范围为 [issue #4](https://github.com/elliothux/open-compute/issues/4)
的上传优化、同尺寸并发回归、debug/release 对比和本地验收；没有修改 LynxOS 或 Cloudflare 账号上的服务。

## 修复

1. `UploadPart` 改用只有路径和精确长度的 `R2PartSource`，删除异步处理线程内那次不被下游消费的
   MD5、SHA-1、SHA-256、SHA-384、SHA-512 全文件计算，以及无消费的 part source version。
   S3 adapter 的 `Content-MD5` 和 Local backend 的内容完整性处理不变。
2. 普通 Worker/管理面 PUT 的必需摘要计算进入有界 blocking task。任务持有 staging 文件、字节配额
   和 CPU permit，取消等待不会提前释放仍被计算占用的资源。staging concern 从已有过长的
   `r2_backend.rs` 提取到 `r2_backend_staging.rs`，没有新增队列、后台上传成功语义或兼容分支。
3. Adobe S3Mock 对比暴露了同一路径的第二处故障：29 个分片及最终对象已经保存，但 provider 自动补入的
   `Content-Type: application/octet-stream` 与未指定的 HTTP metadata 不同，Complete 因此返回
   `R2_OBJECT_METADATA_INVALID`。当前 Day1 对象使用必需的 `oc-r2-http-fields` presence mask 区分
   用户声明字段与 provider 默认字段；显式字段丢失、标记缺失或损坏仍 fail closed。
4. SigV4 fixture 的 multipart complete 也删除了只取 SHA-256 却计算五种摘要的无用工作；保留实际使用的
   SHA-256、part MD5 和 multipart ETag 计算，没有弱化完整性断言。

未修改 `runtime_bridge` 的 30 秒 response-header deadline，也未改变 multipart 的持久状态机、
part/complete ETag、SSE-C、配额或 complete 后才发布对象的顺序。

## 并发上传证据

同一维护用例 `p0-5::uploads::concurrent_large_upload_keeps_runtime_responsive` 通过真实 HTTP、
stock workerd、R2 与 SQLite/D1 执行：合成输入 241,910,375 bytes，8 MiB 分片，共 29 片，4 并发。
Worker 先等待 `uploadPart()`，再写 D1；测试核对 part number/ETag/size authority、D1 总数与总字节数、
Complete 前对象不可见、Complete 后状态和流式读回 SHA-256。

| 本次单次运行 | provider | 分片上传阶段 | 最慢分片 | 轻量请求探测数 | 最慢轻量请求 |
| --- | --- | ---: | ---: | ---: | ---: |
| debug | Adobe S3Mock | 3,286 ms | 531 ms | 46 | 120 ms |
| release | Adobe S3Mock | 2,378 ms | 478 ms | 34 | 66 ms |
| 最终非插桩 Gate / debug | 仓库 SigV4 fixture | 4,447 ms | 688 ms | 69 | 89 ms |

Adobe 镜像固定为
`adobe/s3mock@sha256:65cf60155a2e235fe7d5bf6c633747d6fc7ed93f9f5a6727d86470026b83c2a2`，
与 issue 指定镜像一致；分别使用本次独立创建的 loopback-only 容器，没有下载镜像。
表中时间是分片阶段，不包含准备、Complete、读回或测试清理；是单次观察，不是吞吐 SLA 或重复压测中位数。
所有运行的读回摘要相同：
`7fa67c90cb2e12b9700ae89db64ae62fe35879932c28b08bc05ab027fc41c1c3`。

取消/超时回归另证明：未结束的分片 stream 被取消或超时后，staging 字节数归零、文件与 pin 清理、
不产生 part/object authority，上传许可可再次获取，显式 abort 进入终态。受控 blocking-pool 屏障证明
PUT checksum 等待被取消时文件、字节配额和 CPU permit 保留到计算结束；摘要失败也释放这些资源。

原始 cached debug binary 的源码身份没有独立验证，本次不能反推出旧超时只有一个原因，也没有重新运行
原始 LynxOS UI 或用户文件。证据证明的是当前生产处理链路、同尺寸输入及同版本 provider 的场景。

## 构建、静态检查与最终验收

基线 commit 为 `62cb7a0c92059b9cc7a4c956915094d671d075b1`，包含本次修改的冻结工作树：

- Gate source SHA-256：`bfbabb39134e038abd099752225fb1c37dcc77390811293ed840de294e60caa7`；
- conformance source digest：`5746485ff24af5503b10ab4fb63b293d2bd5c3829e1adde683140111c89c704f`，
  该算法额外排除 `test/conformance/baseline.json`；
- stock workerd：`v1.20260830.1`，revision `e9dda5963aba7ee4323960db795690ec78fec118`，
  effective date `2026-08-30`；使用已存在并经构建/运行时验证的 `.temp/workerd-pin/` archive 和 binary。

`bun run build`、`bun run check:generated`、`cargo fmt --all --check`、canonical workspace/all-targets/
all-features Clippy（`--keep-going -- -D warnings`）、无默认特性检查（`RUSTFLAGS='-D warnings'`）、
Rust 1.98.0 all-targets 检查、Cargo metadata 和 dependency-boundary 检查均通过。
没有修改 JS/TS 实现；未重复执行与本次修改无关的 JS suite。

- `./test/coverage.sh` 一次：49 targets、1,140/1,140 cases，914.70 s；按仓库既有排除规则，
  Rust 行覆盖率为 **90.044691%（109,606 / 121,724）**。未修改覆盖率规则或降低门槛。
- 冻结源码后的 `./test/gate.py --workspace` 一次：49 targets、1,140/1,140 cases，799.13 s；
  native inventory 已核对，无 ignored cases。没有第二次相同输入的最终 aggregate。

原始证据保留于仓库忽略目录：

- `.temp/issue4/adobe-debug-fixed.log`、`.temp/issue4/adobe-release.log`；
- `.temp/issue4/failed/adobe-debug-metadata.log` 与 `adobe-completed-object-headers.txt`；
- `.temp/issue4/` 下的 static、provider、coverage 和 final Gate 日志；
- `.temp/gate-run/20260905T134640-40d71b53/report.json`（coverage）；
- `.temp/gate-run/20260905T140249-6b780bc2/report.json`（最终 Gate）。

## cf-compatibility-check 复核

本次按 `cf-compatibility-check` 检查工作树；默认分支 merge-base 等于上述 HEAD，没有额外的 committed
branch diff。合同采用已归档的 runtime compatibility design、当前兼容矩阵和 deviation registry；
stable types 固定为 `@cloudflare/workers-types@5.20260830.1`，没有手写或修改 Cloudflare 类型。

| 改动面 | 结论 | 依据 |
| --- | --- | --- |
| `uploadPart` 返回值、part/complete ETag、SSE-C | aligned | pinned `R2MultipartUpload`/`R2UploadedPart`、typed-store 与 stock-workerd R2 Gate；只删除无消费的私有输入 |
| PUT checksum 与完成后可见性 | aligned | 原算法和 mismatch 拒绝保留；有界 blocking 调度、取消/失败回归及最终 Gate |
| HTTP metadata 缺省与显式字段 | aligned | presence-mask 全组合、缺失/损坏拒绝、Adobe 无 metadata multipart 与现有显式字段 round-trip |
| Cloudflare 托管端 differential | unverified | 本次没有部署 Cloudflare；不能把本地通过外推为新的托管端差分资格 |

当前 [Cloudflare R2 Worker API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/)
定义的 multipart/metadata/checksum surface 保持不变；
[Tokio blocking-task 生命周期](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)
要求不能把取消等待等同于任务停止。审查范围内没有剩余可操作的不兼容 finding；全球复制/placement
属于既有 `OC-R2-001` excluded self-host scope，没有新增 deviation。整个平台的独立托管资格仍按既有
acceptance 文档管理，不因本报告变成全平台 Cloudflare Go。

## Day1 与操作边界

缺少新 HTTP metadata 标记的旧开发 R2 对象/上传不会被迁移或兼容读取。没有重置已有开发数据、
修改父项目、清理用户 Rust cache 或改动现有 Cloudflare 服务。
两个专用 S3Mock 容器及其可再生成的合成数据已精确清理，并确认本次 label 下无残留容器；
日志和失败证据保留。client disconnect 仍不被承诺为 stock-workerd 的可靠取消信号，后端未知提交
结果继续使用既有持久 intent/reconcile 合同。
