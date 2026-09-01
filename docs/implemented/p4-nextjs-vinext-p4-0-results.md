# P4.0 Next.js / vinext build reproducibility 调查

状态：**调查完成；原 No-Go 已撤回**，2026-09-01。本报告保留当时发现的 source-build
不可复现事实和 digest，但原方案把它误设为 Runtime/Deployment Hard Gate。后续核对 Cloudflare 的
Worker Version/Deployment 语义后，该观察已重分类为非阻断 `toolchain-only` deviation；最终资格结果见
[P4 结果](./p4-nextjs-vinext-results.md)。

## 结论

原 P4.0 在 `vinext/build/reproducible-inventory` Hard Gate 停止。两次从同一源码路径、同一 root lock、
同一环境、清空 `dist/` 与 `.wrangler/` 后执行的正式 `vinext build`，产生了不同的 server module
名称和 bytes。`generateBuildId` 与 `deploymentId` 均已固定；正式 Wrangler config 和 locator 在两次
构建中逐字节相同，因此差异不是 open-compute output locator 或 importer 造成的。

该停止规则不符合 Cloudflare 实际语义：Cloudflare 为每次上传的实际 artifact 创建不可变 Worker
Version，再由 Deployment 激活；它不要求两个独立 source build 逐字节相同。因此后续执行冻结其中一份
正式 output，用 `--skip-build` 同时供 Cloudflare 与 open-compute 消费。仅本次调查本身没有创建或修改
Cloudflare 资源。

当时记录的 `no_go` / `upstream-application` 只作为被更正的历史判定保留，不再是当前 Application
verdict。Platform impact 始终为 `none`。

## 固定输入

| 输入 | 身份 |
| --- | --- |
| vinext | `1.0.0-beta.8`，tag commit `20fdac4cec59fd0f8dcdf490693e5205ccc33dff`，npm integrity `sha512-JXiyi0V13PkcIHWUiql3wO/UyNlI1R325RBwBx4CIDa8dd4lgnI90LN5bD2V1J6T+EU8eYCYnKDmtxWjRdL0cg==` |
| `@vinext/cloudflare` | `1.0.0-beta.6`，tag commit `365c604032835c7a604d922f95ed833bde75d7dd`，npm integrity `sha512-C0DWjPz+5YypjJNyYKrro9fv4neKGd1U1EQ7gvD+dxshFmU51/BGZKWd62yFJ+yCxf75LLNOAqFiIvzgm36zqw==` |
| Next / React | Next `16.2.7`；React、React DOM、RSC webpack `19.2.7` |
| Vite / Cloudflare | Vite `8.2.2`；Cloudflare Vite plugin `1.54.2`；Wrangler `4.127.1` |
| runtime contract | compatibility date `2026-08-30`，flag `nodejs_compat`，与 formal workerd lock 一致 |
| root lock | SHA-256 `1486f18448f7220d640cbd1050d11922e49c89e390daea4dd37ea9128b5128b9` |
| fixture tree | SHA-256 `a8ece60ec00d15500c06b750cb1827e7b082d9e99dac8d34d33940aa64ab80df` |
| case matrix | 33 candidates：31 mandatory、1 optional-partial、1 excluded；SHA-256 `8f1f8fbbe4231803266407ab5efc7063ea91ccbc5982a8ca515703ab09f142a1` |
| Chromium | Playwright `1.62.1`，revision `1234`，Chrome for Testing `151.0.7922.34`，executable SHA-256 `a596b1cfc6353e987fcec8d71a23a28cd6a9e7a6b4e20b908e4c4fcffe51158e` |
| Platform report | `platformVerdict=incomplete`，contract report SHA-256 `f5838726aa3d662c2b566b909c551327465d78fc32af28921628079c7d0e89e6`；对应 Gate report SHA-256 `fdc66f70f925afbb6eedab8222c52f7abc5ea7f8b288d86eeb3292ca65d049fb` |

机器可读输入位于
[`vinext.json`](../../test/conformance/applications/vinext.json)和
[`vinext-cases.json`](../../test/conformance/applications/vinext-cases.json)。离线检查命令：

```sh
bun test/conformance/applications/check-vinext.ts --list
```

当时的离线冻结逻辑验证 root lock、fixture tree、case matrix、精确依赖、case/contract 映射、固定
Chromium 和这份 reproducibility evidence。当前同路径 checker 已更新为 Cloudflare-aligned selection
与 `verdict=go`，仍不会构建、启动浏览器、联网或创建资源。

## 实际执行

固定 application 的 compatibility check 完成：12 项 project/import/config observation 全部报告
supported；这只说明静态 compatibility scan 通过，不覆盖 production artifact 可复现性。

在同一绝对源码路径顺序执行两次：

```sh
cd test/applications/vinext
bun run check
bun run typecheck
bun run build
# 保存 inventory，清空正式输出后再次执行同一 build
bun run build
```

两次 build 都退出 `0`，各产生 91 个正式 output/locator 文件。比较结果：

- 86 个相同相对路径中，80 个 bytes 相同、6 个 bytes 不同；
- 两端各有 5 个仅由 content hash 不同而改名的 server chunks；
- client output、固定 `BUILD_ID`、generated Wrangler config 与 locator 保持一致；
- 第一份 inventory SHA-256 为 `931ebcce2f04fef00877aeaabb28819cc18fe1ead90e9fdcdd15baa3a88d55b1`；
- 第二份 inventory SHA-256 为 `90c794b77d3e14d39600b254567c2ff7bb9e535845f86b8fb0320874e909e110`；
- 两次 generated Wrangler config SHA-256 都是 `bd0bab5e74e874697e8e5c56d47227e115a78e813327494bb1758e04249cc83b`；
- 两次 locator SHA-256 都是 `3bb45bee9081f928ffba85d40a8dcf66a59f80b03f5ce36ee5d35c2a0ed92e3c`。

失败证据保留在
`.temp/p4-final-lock-repro.NORFk3/`：包含两次 build log、两份完整 SHA-256 inventory 和第一份正式 output；
第二份正式 output 保留在 fixture 的 ignored `dist/`、`.wrangler/` 中。证据没有账号 credential、cookie、
admin token 或 Cloudflare resource identity。

## 根因边界

固定 tag 的 vinext production build 会生成每次构建不同的 preview credentials，并在未预置内部值时
生成 shared revalidation secret；这些值被编译进 server output，继而改变 chunk bytes、content-hash
文件名和引用它们的 manifest/entry。固定 Next.js `generateBuildId` 只稳定 `BUILD_ID`；固定
`deploymentId` 还能稳定 RSC compatibility identity，但不能稳定 preview/revalidation build credentials。

这些差异不是可从 inventory 中简单删除的时间戳或报告元数据：它们位于实际执行的 server modules 中，
并传播到 content-addressed module names。P4 不通过 patch `node_modules`、劫持随机源、重写构建产物或
把随机 server bytes 归一化成同一个 deployment digest 来绕过 Hard Gate。

对应上游实现证据：

- [`createPreviewBuildCredentials()`](https://github.com/cloudflare/vinext/blob/20fdac4cec59fd0f8dcdf490693e5205ccc33dff/packages/vinext/src/build/preview-credentials.ts)
  使用加密随机值生成 preview ID、签名 key 和加密 key；
- [`vinext build` CLI](https://github.com/cloudflare/vinext/blob/20fdac4cec59fd0f8dcdf490693e5205ccc33dff/packages/vinext/src/cli.ts)
  在未设置内部 shared revalidation secret 时生成随机 secret；
- [`createRscCompatibilityId()`](https://github.com/cloudflare/vinext/blob/20fdac4cec59fd0f8dcdf490693e5205ccc33dff/packages/vinext/src/config/next-config.ts)
  可以由 `deploymentId` 固定，本 fixture 已这样处理。

## 当时已完成的 P4.0 准备工作

- 根 Bun workspace 已加入固定 qualification fixture，只有一个 `bun.lock`；generated `dist/`、`.next/`
  和 `.wrangler/` 明确保持 untracked；
- 已建立 33 项有限 Cloudflare-alignment matrix 和 offline checker；
- 已勘察正式 generated Wrangler config、locator、server/client output 布局；在本次调查结束时，importer
  仍按原设计拒绝 generated binding declarations，通用 reconciliation 尚未实现。后续 P4 已完成该实现。

这些准备工作不等于 P4.1 验收：没有 application deployment、HTTP/browser execution、bindings/cache
组合、生命周期、双账户或 Cloudflare differential 证据。

## 更正后的处理

不再等待 deterministic build credential，也不要求新版 vinext 跨 build 可复现。每轮资格验证只需：

1. 固定 source、lock、工具链和一次正式 production build；
2. 冻结该 output tree，并核对 Wrangler/importer 对同一 tree 的 module inventory；
3. 让两端通过 `--skip-build` / framework importer 消费这同一份 immutable artifact；
4. 分别记录 Cloudflare Worker Version/Deployment 和 open-compute deployment digest；
5. 保留跨 source-build drift 作为上游供应链观察，不修改或归一化实际 server bytes。
