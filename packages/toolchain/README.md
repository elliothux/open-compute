# Worker TypeScript 工具链

`oc` 在开发端用 TS7 严格检查项目，再用 Rolldown 编译和打包。产物由匹配版本的
`platformd worker bundle` 编码，复用 Rust 的 canonical bundle 格式与大小限制。
部署走已有的校验、持久化与激活接口；生产 `platformd` 启动和请求路径不调用 Bun、Node 或编译器。

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

`open-compute.json` 至少需要 `name`、`main` 和 `compatibilityDate`。
`main` 与 `tsconfig`（默认 `tsconfig.json`）相对配置文件目录解析，不能逃逸到目录之外。
默认平台地址为 `http://127.0.0.1:8787`；支持 `endpoint` / `--endpoint` 覆盖。
默认账户由经过身份验证的 `GET /v1/account` 返回，也可用 `accountId` / `--account` 明确指定。

其他字段是 `compatibilityFlags`、`vars`、`secrets` 和 `bindings`。
密钥配置只能引用环境变量，例如 `"secrets": {"TOKEN": {"env": "MY_TOKEN"}}`；
只有 `run` / `deploy` 才读取其值，离线 bundle 不包含配置变量和密钥。
管理令牌从 `OPEN_COMPUTE_ADMIN_TOKEN` 读取，或通过 `--token-env` 指定另一个变量名。
不要把密钥值写入项目配置或命令参数。

ESM 静态依赖、动态 import 的 chunks 和具名导出会保留；`node:` 导入需要显式启用
`nodejs_compat`。工具链不提供 Node 运行环境、不填补未实现的产品 API，也不下载远程 import。
运行时仍按平台当前的兼容性日期、能力与资源限制校验产物。

## 检查

```sh
bun run typecheck
bun run test:js
bun run --filter @open-compute/toolchain build
```

P0.2 integration test 增加了真实 TS CLI → 管理 HTTP → SQLite/S3 → stock workerd 的用例，
包括动态 import、变量/密钥注入、更新，以及类型错误不创建新部署。该用例需要本机 Bun 与已安装
的锁定依赖；可用 `OPEN_COMPUTE_TEST_BUN` 指定 Bun 路径。开发时按 `docs/testing.md` 只跑一轮。

runtime 系统源码与工具链使用同一套锁定的 TS7 / Rolldown。类型检查和工具链测试不能替代
系统资产切换后的最终三轮 Gate。
