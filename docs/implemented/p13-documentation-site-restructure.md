# P13：用户文档信息架构整理

状态：已实现。

日期：2026-09-12

P13 将 `https://open-compute.dev/docs/` 从按产品机械拆页的目录，整理为以用户任务为主线的双语文档站。内容优先级固定为：先安装和运行 open-compute，再开发和部署应用，然后查询产品与合同，最后理解和贡献项目。

## 当前信息架构

英文文档位于 `/docs/`，简体中文位于 `/docs/zh/`。两种语言使用相同的一级结构：

| 区域        | 用途                                                               |
| ----------- | ------------------------------------------------------------------ |
| Get started | 从正式 release 安装，到第一个 Worker 部署                          |
| Develop     | Wrangler 项目、本地开发、target、environment、bindings、部署与回滚 |
| Operate     | 单机服务、配置、健康检查、备份和故障处理                           |
| CLI         | 按任务解释当前 `ocd` 命令与共同选择规则                            |
| Products    | 当前支持的产品、最小用法和关键差异                                 |
| Reference   | compatibility、limits、API 和稳定合同                              |
| Project     | 架构、源码构建、测试、workerd 与贡献入口                           |

Sidebar 根据当前区域切换，只展示本区内容和必要的下一步。产品保留稳定入口，但通用的开发、部署、secret、日志和运维说明不在每个产品下重复。

## 已完成整理

- 重写中英文首页、Get started、Develop、Operate、CLI、Products、Reference 和 Project 入口；
- 将普通用户的黄金路径改为一行正式 release 安装、独立 `ocd setup`、项目内 Wrangler 和 `ocd wrangler`；
- 依据当前 CLI、capability catalog、release 与正式 workerd pin 更新能力说明；
- 增加 Artifacts，并准确说明 AI Search R2 source、Dynamic Workers、Browser Run 与 Containers 的边界；
- 合并产品下重复且内容很少的 Concepts、Guides、Examples、Limits 与 Deviations 页面；
- 更新根 README、网站 README、营销页导航、CTA、footer、`llms.txt`、robots 和 sitemap 合同；
- 为被合并的公开入口提供单跳 redirect，并增加双语任务型 404；
- 增加 `scripts/check-docs.ts`，在网站构建前校验双语页面、内部链接、导航、redirect 和过期命令。

## 维护规则

- `ocd --help` 和各级 `--help` 是 CLI 参数 authority；文档只维护任务分组与少量示例。
- 产品状态以 `share/cloudflare-capabilities.json`、当前源码、正式 release notes 和兼容矩阵为准。
- 用户文档不把仓库 checkout、Rust toolchain 或根 Bun workspace 作为安装前置条件。
- 应用项目命令使用同步的 npm、pnpm、bun Tabs；只有 repository contributor 流程可以依赖根 Bun workspace。
- 英中关键路径和一级页面必须成对更新；移动页面只保留必要的单跳 redirect。
- 稳定合同放在 Reference，源码构建和内部工程内容放在 Project，教程通过链接引用它们。
- 根 README 优先链接线上文档，不复制长篇产品或 CLI 参考。

## 验证

实现完成时执行：

```sh
bun run --filter @open-compute/website check:docs
bun run --filter @open-compute/website build
git diff --check
```

网站构建同时运行文档检查和 TypeScript typecheck。文档校验覆盖中英文路径对称、必需入口、内部链接、导航目标、`llms.txt`、redirect 目标和已退出的公开命令。
