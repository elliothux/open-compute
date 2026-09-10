# Wrangler 项目与部署目标

Worker 保持为标准 Wrangler 项目。项目拥有 `wrangler.jsonc`、可复现固定的 `wrangler` 依赖与 lockfile、源码、environment、本地 `.dev.vars` 和测试。`ocd` 只拥有所选 open-compute authority，并且只把短命 deployer credential 注入 Wrangler child process。

## 三种相互独立的选择

| 概念                 | 选择什么                                           | 是否进入项目                           |
| -------------------- | -------------------------------------------------- | -------------------------------------- |
| local instance       | 本机正在运行的一个 `ocd`                           | 否                                     |
| remote target        | 命名的 origin、account 与外部 token-file reference | 非 secret 的 target name 可进入 script |
| Wrangler environment | `wrangler.jsonc` 中如 `env.staging` 的配置         | 是                                     |

本机只有一个运行中的 instance 时不需要 selector：

```sh
ocd wrangler deploy --env dev
```

用全局 selector 选择精确本机 instance，或用 launcher selector 选择远程 target：

```sh
ocd --instance k7m2r wrangler deploy --env staging
ocd wrangler --target company-prod deploy --env production
```

`--target`、`--instance`、`--config` 两两互斥。本机可用 instance 为零个或多个时 fail closed；先用 `ocd instances` 明确选择。

## 注册远程 target

deployer token 必须在 repository 之外。token file 必须是绝对路径、由当前用户拥有、类型为 regular file、权限精确为 `0600`，且不能经 symlink 访问。

```sh
install -d -m 700 "$HOME/.config/open-compute"
install -m 600 /dev/null "$HOME/.config/open-compute/company-prod.token"
printf '%s\n' "$OPEN_COMPUTE_DEPLOYER_TOKEN" > "$HOME/.config/open-compute/company-prod.token"

ocd target add company-prod \
  --api-base-url https://compute.example.com/client/v4 \
  --account-id 0123456789abcdef0123456789abcdef \
  --token-file "$HOME/.config/open-compute/company-prod.token"
ocd target test company-prod
```

远程 URL 必须使用 HTTPS；只有 loopback 可使用 HTTP。registry 位于用户目录（`$XDG_CONFIG_HOME/open-compute/targets.toml`、macOS Application Support 或平台 fallback），是 owner-only、有大小上限、严格 schema 的 TOML。它只保存 token file path，不保存 token value。

```sh
ocd target list
ocd target show company-prod --json
ocd target remove company-prod
```

list、show、remove 都不打开 token file；remove 也不删除外部文件。`target test` 才是显式网络操作：它验证认证、account、capabilities 和作为认证基线的 Wrangler 精确版本。

## 项目内 Wrangler

在 `devDependencies` 中可复现地固定 Wrangler 并提交 Bun lockfile。launcher 从 `--project`（或启动 cwd）向上寻找最近的 `node_modules/.bin/wrangler`。同一认证 major 内的 minor、patch 漂移不会产生 warning；major 不同时会显示 detected 与 certified version，但仍然启动 Wrangler，最终 exit status 由 child command 决定。缺少 binary 或 version check 失败仍然直接失败；launcher 不下载也不自动修复依赖。

Wrangler command 之后的参数保持 opaque：

```sh
ocd wrangler --project /srv/workers/billing deploy --env staging --config wrangler.jsonc
ocd wrangler -- --version
```

launcher 成功后以 Wrangler 替换 `ocd` 进程，因此保留 terminal、stdout/stderr、signal 和 exit status。它不解析或改写项目配置；framework 生成的 `.wrangler/deploy/config.json` 继续使用标准 Wrangler 语义。

## 开发、发布与回退

快速循环直接用上游本地开发，不访问 `ocd`：

```sh
bun run dev                 # wrangler dev
bun run deploy:dev          # 向所选本机 instance 做真实 dev 部署
bun run deploy:staging      # 显式 remote target + env
bun run logs:production     # 通过 production target 实时 tail
```

不要把 `wrangler dev --remote` 宣称为支持；真实 integration 使用显式 dev 或 staging Deployment。

production 发布和回退只改变 active immutable Version pointer，不修改 Version bytes：

```sh
ocd wrangler --target production deploy --env production
ocd wrangler --target production deployments list --env production
ocd wrangler --target production versions list --env production
ocd wrangler --target production rollback <version-id> --env production --yes
```

mutation 中断后，应查询 Deployment 与资源状态。client disconnect 不能证明服务端已取消操作。

## 不依赖 target registry 的 CI

CI 可以直接运行项目内同一固定版本的 Wrangler。CI secret store 只保存 deployer token；API base URL 与 account ID 使用非 secret variable。

```sh
CLOUDFLARE_API_BASE_URL="$OPEN_COMPUTE_API_BASE_URL" \
CLOUDFLARE_API_TOKEN="$OPEN_COMPUTE_DEPLOYER_TOKEN" \
CLOUDFLARE_ACCOUNT_ID="$OPEN_COMPUTE_ACCOUNT_ID" \
bun run deploy:ci
```

仓库示例提供 [GitHub Actions](https://github.com/elliothux/open-compute/blob/main/examples/hello-worker/ci/github-actions.yml) 和 [GitLab CI](https://github.com/elliothux/open-compute/blob/main/examples/hello-worker/ci/gitlab-ci.yml) 模板。两者都安装 frozen lockfile、使用项目精确依赖、禁用 Wrangler telemetry/error reporting，并且只打印非 secret 的 target identity。

## 故障处理

| 失败                  | 处理                                                                                                   |
| --------------------- | ------------------------------------------------------------------------------------------------------ |
| 本机 instance 不唯一  | 运行 `ocd instances`，再传 `--instance`、`--config` 或 `--target`                                      |
| target 不存在         | 运行 `ocd target list`；target name 精确匹配                                                           |
| token file 被拒绝     | 使用当前用户拥有、绝对路径、权限精确 `0600` 的 regular file；不要用 symlink                            |
| capability probe 失败 | 运行 `ocd target test <name>`，检查网络/TLS、account 与 deployer role                                  |
| Wrangler major 不匹配 | 检查 warning 中的 detected 与 certified version；需要兼容保证时使用 certified major                    |
| Wrangler command 失败 | 保留其 exit status 与 Cloudflare-style error；修正项目或支持能力，不删除 binding、不改写 config 后重试 |

Wrangler 标准环境变量和 environment 行为仍以上游合同为准：[system environment variables](https://developers.cloudflare.com/workers/wrangler/system-environment-variables/) 与 [environments](https://developers.cloudflare.com/workers/wrangler/environments/)。
