# P4：Next.js / vinext 应用资格验证

状态：**Application Go，资格实现与仓库回归验收完成**，2026-09-01。固定的 vinext/Next.js
production artifact 已在 open-compute 与真实 Cloudflare Workers 上完成同源 HTTP、Chromium、
Server Action、RSC、Assets、binding、安全隔离和精确清理对照。实施与实际证据见
[P4 结果](./p4-nextjs-vinext-results.md)。

## 1. 判定语义

Wrangler 与 open-compute 使用同一份冻结的 production output tree；上传期间 module names／bytes、assets、
generated config 和 locator 保持不变。Version／deployment digest 绑定实际上传字节。
`vinext/build/production-output` 属于 qualification，跨独立构建的 `reproducible-inventory` 仅作为 toolchain 观察。

原跨构建 Hard Gate 判定已撤回；实际漂移和后续判定完整保留在[原始调查](p4-nextjs-vinext-p4-0-results.md)。
官方发布模型见 [Workers Versions & Deployments](https://developers.cloudflare.com/workers/versions-and-deployments/)。

## 2. 目标与结论边界

P4 只回答：一个固定 Next.js 16 workload 经固定 vinext production build 后，是否能以同一冻结产物在
Cloudflare Workers 与 open-compute 上得到一致的公开行为。

它不回答：

- vinext 或 Next.js 全 API、全 upstream suite 是否兼容；
- vinext dev/HMR、Node production server、OpenNext 或 Vercel runtime 是否兼容；
- 所有 KV、D1、R2、Durable Objects、Queues、Workflows、Service Binding、Cache、Images 的产品行为
  是否由这一个应用重新证明；
- 多地域 CDN、全球缓存或跨平台发行是否完成；
- open-compute 的 Platform verdict 是否能被一个第三方应用替代。

vinext 是组合 workload，不是平台规范。只有能脱离 vinext、映射到已声明 Cloudflare contract 的普通
Worker reproduction，才允许晋升为平台缺陷。

## 3. 固定输入

机器可读基线为
[`vinext.json`](../../test/conformance/applications/vinext.json)，有限 case matrix 为
[`vinext-cases.json`](../../test/conformance/applications/vinext-cases.json)。固定输入包括：

- vinext `1.0.0-beta.8`，commit `20fdac4cec59fd0f8dcdf490693e5205ccc33dff`；
- `@vinext/cloudflare` `1.0.0-beta.6`，commit `365c604032835c7a604d922f95ed833bde75d7dd`；
- Next `16.2.7`、React `19.2.7`、Vite `8.2.2`、Wrangler `4.127.1`；
- Playwright `1.62.1`、Chromium revision `1234`、browser version `151.0.7922.34`；
- compatibility date `2026-08-30` 和 `nodejs_compat`；
- 根 `bun.lock`、fixture tree、case matrix 与 browser executable SHA-256。

离线冻结检查：

```sh
bun test/conformance/applications/check-vinext.ts --list
```

该命令只读取固定输入，验证依赖、digest、case selection、Chromium executable、runner registry 和已记录
的 differential/cleanup evidence；不构建、不启动浏览器、不联网、不创建资源。

## 4. 有限选择集

本次 matrix 共 34 个候选：20 mandatory、0 optional、14 excluded。20 个 mandatory 中，5 个是
build/import/deploy/cleanup orchestration，15 个由同一个 application runner 在两个目标上逐项执行。

### 4.1 Mandatory orchestration

| case | 实际观察 |
| --- | --- |
| `vinext/build/production-output` | 正式 locator、generated Wrangler config、server module tree 与 client assets 完整 |
| `vinext/import/framework-output` | `no_bundle` 产物不重打包、不 flatten；Wrangler 与 importer 都发现 79 个相同 module names |
| `vinext/import/binding-reconciliation` | generated name/kind/class/entrypoint 与本地声明双向核对；provider resource identity 不成为本地 authority |
| `vinext/deploy/immutable-activation` | 同一冻结 artifact 在本地形成 immutable deployment，在 Cloudflare 形成一个 Worker Version 和一个 Deployment |
| `vinext/cleanup/exact-absence` | 只删除本轮精确命名/ID 的 Worker、route 和进程；两端复查 absence |

### 4.2 Mandatory runtime/application cases

```text
vinext/app/ssr-status-headers
vinext/app/streaming-ssr
vinext/app/hydration-navigation
vinext/app/rsc-flight
vinext/app/server-actions
vinext/app/routes-middleware
vinext/app/metadata-assets
vinext/pages/gssp
vinext/pages/gsp-paths
vinext/static/export-routing
vinext/static/deployment-coherence
vinext/bindings/env-context
vinext/lifecycle/cold-warm
vinext/security/server-only
vinext/security/browser-context-isolation
```

runner 位于
[`qualification.ts`](../../test/applications/vinext/tests/qualification.ts)，`--list` 与执行共享一个权威 case
registry；没有 retry、skip、fixme 或隐式安装/下载。HTTP 与 browser 断言在 Cloudflare/open-compute 上
使用同一代码，只改变 base URL，以及本地 exact-host 所需的 Host/Chromium host mapping。

### 4.3 Excluded

以下能力未被固定 representative workload 使用，因此不进入 denominator，也不能写成 P4 已验证：

- source build 跨构建 byte reproducibility；
- ISR、path/tag invalidation、KV data adapter、Workers Cache；
- `next/image` partial path；
- KV/D1/R2/DO 组合、Queue/Workflow、companion/self Service Binding；
- promotion/rollback、workerd restart、platformd restart；
- 两个 open-compute account 的产品级 tenant isolation。

这些能力继续由各自普通 Worker/product Gate 所有。P4 只额外验证了两个独立 Chromium context 的
HttpOnly Server Action cookie 不串读。

## 5. Production build 与 importer contract

P4 只调用固定 vinext Cloudflare production build。Importer 读取
`.wrangler/deploy/config.json` 的唯一 `configPath`，要求 `auxiliaryWorkers` 为空、`no_bundle: true`，并
验证 generated compatibility date/flags 与 formal runtime lock 一致。

模块收集语义与 Wrangler 对齐：

- 遍历 output tree 中所有命中 generated `rules` 或 Wrangler 默认 module extension 的 regular files；
- 不用自制 JavaScript import graph 猜测可达性；
- 保留相对 module name，不 flatten、不重新 transform；
- 排除 Assets directory；拒绝 symlink、special file、边界逃逸、过量/过大文件；
- `.js`/`.cjs` 默认 CommonJS，`.mjs` 默认 ESModule，显式 rules 可覆盖；Wasm/Text/Data 按 Wrangler
  类型导入，JSON 不被默认附加。

Generated binding reconciliation 支持平台已有的 KV、R2、D1、DO、Queue producer、Workflow、Service、
Assets、Cache、Images 和 Version Metadata 声明。对于 DO/Workflow，`className` 是必须核对的 framework
语义字段；它不替代本地 resource ID，也不发送为平台资源 authority。Cloudflare Worker/service/resource
name 或 ID 只属于 provider config，不要求等于 open-compute 的 Worker/resource identity。

固定产物的 Wrangler dry-run 与 importer inventory 均为 79 modules，排序后的 module-name inventory
SHA-256 都是 `8311c2918f094d6bbd435c9db94d4ced97d8da8c710e008d6dd925214a3e29d1`。

## 6. Runtime 与浏览器验证

open-compute 使用隔离 data-dir、测试 S3 fixture、formal pinned workerd archive/binary、fresh Worker 和
exact-host root route。固定 bundle SHA-256 为
`a68f14ec859a463bbd4f946bff33cdbf57d32a32a4ebd3efd23ed2de2a02dd14`。

Cloudflare 使用预检不存在的唯一 Worker name，只执行：

```sh
vinext-cloudflare deploy --skip-build --name <unique-run-owned-worker>
```

`--skip-build` 保证 Cloudflare 上传已经在本地冻结并由 open-compute importer 检查过的正式 output，
而不是在部署期间再次 source build。实际 Cloudflare 产生一个 Version、一个 Deployment、100% 流量；
版本与部署 ID 记录在 machine-readable manifest 和结果报告中。

两个目标各自 15/15 通过，覆盖：

- SSR body/status/headers 与 streaming fallback-before-resolved 顺序；
- Chromium hydration、client state、RSC navigation request；
- Server Action request、redirect result 和 HttpOnly cookie；
- App/Pages Route Handler、GSSP、GSP paths/404；
- metadata、public asset 和一个 HTML 引用的完整 client chunk 集合；
- `cloudflare:workers` env 访问；
- cold/warm 一致性；
- server-only canary 不进入 selected public bodies 或 client tree；
- 两个 browser contexts 的 action cookie 隔离。

## 7. 上传 body limit 的通用修复

真实 vinext Worker+Assets bundle 暴露了一个与框架无关的 control-plane bug：全局 4 KiB middleware 在
staged upload endpoint 自己的严格上限之前就拒绝 create/object/finalize request。修复只按精确 staged
deployment route shape 提高外层 hard ceiling，endpoint handler 的细分上限、认证、完整性和 staged
upload 状态机保持不变；相似路径仍为 4 KiB。

该修复没有 vinext case ID、fixture name、URL 或专用分支，并有 staged object/create、hard ceiling 和
lookalike path 回归测试。

## 8. 外部资源与清理

Cloudflare 只创建了两个全局唯一、运行前确认不存在的测试 Worker；第一个用于最小传播/部署语义探针，
第二个用于最终固定 workload。未创建 KV、R2、D1、Queue、Workflow、route 或其他账号资源，也未修改
任何已有服务。两个测试 Worker 均已删除，并通过对精确 name 查询得到 API code `10007`（不存在）。

Wrangler `delete` 在 Worker 已删除后继续尝试 account-wide legacy Workers Sites KV 检查；本机 OAuth 对
该 list 返回 `10000`，因此 CLI 进程退出 1。随后对精确 Worker 的 read-only deployment 查询返回
`10007`，证明这不是残留 Worker。没有因此扩大权限、枚举删除或触碰其他 namespace。

本地 exact-host route 与 Worker 已 tombstone；fresh platformd 进程复查 Worker list 为空；资格验证启动
的 platformd、workerd 和 S3 fixture 都已停止，监听端口为空。保留 `.temp/` 中脱敏的失败/运行证据。

## 9. 仓库验收

P4 实现冻结后完成以下验证：

| 检查 | 结果 |
| --- | --- |
| `bun run build` / `bun run typecheck` | 通过 |
| `bun run test:js` | 197/197 通过 |
| `cargo fmt --all --check` | 通过 |
| canonical Clippy | workspace/all-targets/all-features 通过，`-D warnings` |
| no-default-features、metadata、dependency boundaries | 通过 |
| Rust 1.98 all-targets/all-features check | 通过 |
| `./test/coverage.sh` | 40 targets、802/802 cases；Rust 行覆盖率 90.17% |
| `OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace` | 80 processes、894/894 case executions；完整轮一次，46 个时序 case 追加两轮；1604.22 秒 |

Coverage Gate 报告保留于 `.temp/gate-run/20260901T183941-4cc8d923/report.json`，最终三轮报告保留于
`.temp/gate-run/20260901T185216-7bb31c64/report.json`。

仓库不带 feature 的 canonical MSRV 命令仍有一个既存静态问题：`p0_2_runtime_gate` 会调用只在
`test-support` feature 下导出的 `product_promotion_for_test`。P4 没有修改该 target 或 feature contract；
同一 Rust 1.98 toolchain 的 `--all-targets --all-features` 检查和最终 workspace Gate 均通过。本项作为
仓库级已知限制记录，不改变本 P4 Application verdict。

## 10. Verdict

Application verdict 为 `go`，范围严格限定为本 manifest 的 20 个 selected mandatory cases。理由：

1. 同一冻结 artifact 通过 Wrangler/importer inventory 对齐；
2. importer、binding reconciliation 和 immutable deployment 通过；
3. open-compute 与 Cloudflare 各 15/15 runtime/application cases 通过；
4. 无未解释的 `Cloudflare PASS / open-compute FAIL`；
5. server-only 与 browser-context isolation 通过；
6. 两端精确清理已证明。

Platform verdict 保持其独立状态。不能把本结论扩写为“全部 Next.js/vinext API 兼容”“P4 exclusions 已
实现”或“跨平台/发行资格完成”。
