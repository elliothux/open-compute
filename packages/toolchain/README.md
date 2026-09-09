# Worker TypeScript 工具链

`oc` 保留 open-compute 自有的离线开发职责：TypeScript 7 严格检查、Rolldown build、
Static Assets 扫描、framework output 导入、Env 类型生成和单一 Worker bundle 编码。项目语法只由仓库精确
pin 的 `wrangler@4.127.1` 解析，唯一配置文件是 `wrangler.jsonc`。

`oc` 不包含在线命令。部署由 `ocd wrangler` 选择 instance/target、注入短命凭据并直接执行项目内固定版本的上游 Wrangler：

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
```

从 Wrangler command 开始的参数原样传递；认证、multipart、
Versions、Deployments、Secrets 和资源 provisioning 均由 Wrangler 与 `/client/v4` 合同负责。

离线 build 继续使用仓库实现：

```sh
bun run oc build --config examples/hello-worker/wrangler.jsonc \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
```

输出必须是新文件，已有文件不会被覆盖。该命令不安装依赖、不下载 workerd、不访问管理 API。
Assets 会在本地校验，但不会封装成 open-compute 私有部署包；静态资源的三阶段上传由项目内固定 Wrangler 完成。assets-only 项目直接使用 `ocd wrangler deploy`。

Env 类型也由本地工具链从 Wrangler 规范化后的配置生成：

```sh
bun run oc types --config examples/hello-worker/wrangler.jsonc
```

默认写入项目目录的 `worker-configuration.d.ts`。变量使用标准 `vars`；远端密钥值不写入配置，
只可用 `secrets.required` 声明类型所需的密钥名。KV、D1、R2、Durable Objects、Queues、Workflows、
Service Bindings、Vectorize、AI Search、Images、Workers AI 和 Version Metadata 均使用 Wrangler
schema 中的标准字段。

框架 adapter 使用 Wrangler 的标准 redirect：用户项目保留 `wrangler.jsonc`，框架产出
`.wrangler/deploy/config.json`，其中 `configPath` 指向框架生成的 Wrangler JSON。toolchain 通过
Wrangler 的 environment/config resolver 读取它，再验证并导入预构建 module graph 和 assets；不维护第二套
项目 grammar。

检查：

```sh
bun run --filter @open-compute/toolchain typecheck
bun test packages/toolchain/tests/project.test.mjs \
  packages/toolchain/tests/generate-types.test.mjs \
  packages/toolchain/tests/framework-output.test.mjs
```
