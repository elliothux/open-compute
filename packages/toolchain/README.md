# Worker TypeScript 工具链

`oc` 是单机 Cloudflare Workers Platform 的开发与部署客户端。它在开发端用 TS7 严格检查普通
Worker，再用 Rolldown 编译和打包；也可以导入已构建的框架产物，或部署 Static Assets-only
项目。产物由匹配版本的 `platformd worker bundle` 编码，复用 Rust 的 canonical bundle 格式与
大小限制。部署走统一的校验、持久化与激活接口；生产 `platformd` 启动和请求路径不调用 Bun、
Node 或编译器。

配置借用 Cloudflare/Wrangler 的常见字段和语义，但 `open-compute.json` 不是完整 `wrangler.jsonc`
兼容层。parser 只接受本文列出的已实现字段，未知字段直接拒绝；平台未广告的 API 不能靠工具链
配置变成可用。

## 本地运行

仓库使用根目录 Bun workspace 和 `bun.lock`。先安装锁定依赖，并通过 `scripts/dev.sh`
启动本地平台；该脚本需要已经准备好的 pinned workerd 与 rclone，具体前提见根 README。

从仓库根目录运行示例：

```sh
bun run oc run --config examples/hello-worker/open-compute.json \
  --platformd "$PWD/target/debug/platformd"
```

命令会检查 TS、编译、创建或复用同名 Worker、校验并激活新部署，最后打印可访问 URL。
类型检查失败时不会调用部署接口。修改源码后再次运行同一命令即可更新；当前不提供 watch/HMR。
`run` 要求已有本地平台，并不启动另一个 workerd。远端使用 `deploy`，只接受 HTTPS origin。

只生成离线产物：

```sh
bun run oc build --config examples/hello-worker/open-compute.json \
  --platformd "$PWD/target/debug/platformd" --out /absolute/new-worker.bundle
```

输出必须是新文件，已有文件不会被覆盖。`--json` 可输出公开的结果字段。
这两个命令都不会安装依赖、下载 workerd 或执行项目配置代码。

## 项目配置

`open-compute.json` 必须有 `name`、`compatibilityDate`，并选择一种内容形态：`main`、`assets`、
`main + assets`，或 `frameworkOutput`。`frameworkOutput` 不能再和显式 `main`/`assets` 组合；
Assets-only 项目不能声明 vars、secrets、产品/service bindings，也不能要求 Worker-first。
`main`、`frameworkOutput`、assets directory 与 `tsconfig`（默认 `tsconfig.json`）相对配置文件目录
解析，不能逃逸到允许的项目/产物边界之外。
默认平台地址为 `http://127.0.0.1:8787`；支持 `endpoint` / `--endpoint` 覆盖。
默认账户由经过身份验证的 `GET /v1/account` 返回，也可用 `accountId` / `--account` 明确指定。

其他字段是 `compatibilityFlags`、`vars`、`secrets`、`bindings`、`services` 和 `assets`。普通产品
binding 使用 `{type, id, permissions?}`；Service Binding 使用
`[{binding, service, entrypoint?}]`，部署时把同账户 Worker 名解析并冻结为目标 ID。Assets 支持
`directory`、`binding`、`run_worker_first`、`html_handling`、`not_found_handling` 和
`publish_source_maps`；精确支持值由 parser 与 Static Assets 文档共同约束。
密钥配置只能引用环境变量，例如 `"secrets": {"TOKEN": {"env": "MY_TOKEN"}}`；
只有 `run` / `deploy` 才读取其值，离线 bundle 不包含配置变量和密钥。
管理令牌从 `OPEN_COMPUTE_ADMIN_TOKEN` 读取，或通过 `--token-env` 指定另一个变量名。
不要把密钥值写入项目配置或命令参数。

ESM 静态依赖、动态 import 的 chunks 和具名导出会保留；`node:` 导入需要显式启用
`nodejs_compat`。工具链不提供 Node 运行环境、不填补未实现的产品 API，也不下载远程 import。
运行时仍按平台当前的兼容性日期、capability/deviation 与资源限制校验产物。当前配置没有
Cache/Images 字段；规划中的能力在实现、Gate 和 capability 广告完成前不得写进项目配置示例。

## 检查

```sh
bun run typecheck
bun run test:js
bun run --filter @open-compute/toolchain build
```

P0.2 integration test 增加了真实 TS CLI → 管理 HTTP → SQLite/S3 → stock workerd 的用例，
包括动态 import、变量/密钥注入、更新，以及类型错误不创建新部署。该用例需要本机 Bun 与已安装
的锁定依赖；可用 `OPEN_COMPUTE_TEST_BUN` 指定 Bun 路径。开发时按 `docs/references/testing.md` 只跑一轮。

runtime 系统源码与工具链使用同一套锁定的 TS7 / Rolldown。类型检查和工具链测试不能替代
系统资产切换后的最终三轮 Gate。
