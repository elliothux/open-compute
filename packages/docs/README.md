# open-compute 开发者文档

VitePress 站点：开发者产品文档，加上运维 `ocd` 章节（`/ocd`）。中文是默认语言（`/`，`zh-CN`），英文在 `/en/`（`en-US`）。导航里有语言切换。
正式站点 origin 是 [https://open-compute.dev](https://open-compute.dev)，本地开发服务器不改变该公开域名契约。

从仓库根目录：

```sh
bun run docs:dev
bun run docs:build
bun run docs:preview
```

也可以在本目录运行 `bun run dev` / `build` / `preview`。

生成输出在 `.vitepress/dist/`，缓存在 `.vitepress/cache/`，二者都不提交。

Cloudflare Workers Builds（项目名 `open-compute-docs`）根目录设为 `packages/docs`：

```
Build command: bun run build
Deploy command: bunx wrangler deploy
```

静态资源目录写在 `wrangler.jsonc` 的 `assets.directory`（`.vitepress/dist`）。不要让 Wrangler 在 CI 里走交互式自动配置。
