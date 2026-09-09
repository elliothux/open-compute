# Vinext 离线输入校验

`bun test/conformance/applications/check-vinext.ts --list` 检查当前输入，输出
`verdict: "inputs-verified"`。它核验根 lock、fixture tree、case matrix、已安装的直接依赖版本、
Playwright 内置 Chromium 元数据及 runner case registry，不构建、不启动浏览器、不创建云端资源。

[`vinext.json`](../../test/conformance/applications/vinext.json) schema 2 的顶层三个 SHA-256
属于当前离线输入。`p4Status.evidence` 中同名摘要保留历史运行的输入身份；`historicalVerdict: "go"`
仅引用 [2026-09-01 P4 实现与验证](../implemented/p4-nextjs-vinext-qualification.md)。当前离线检查不会更新
历史 Worker Version、Deployment、产物或 differential 报告，也不能证明新 runtime/lock 的云端行为。

## 2026-09-06 漂移修复

旧检查首先报 `root lock digest drift`，并遮住了另两处已经存在的漂移：

- `96d3ffb7c` 加入 Dashboard 依赖、`bfe21b7ab` 迁移 P6 消费者，改变共享根 lock；
- `80953b9e2` 删除已废弃的 `test/applications/vinext/open-compute.json`，改变 fixture tree；
- 同一提交把两个 case 的 fixture 引用改为 `wrangler.jsonc`，改变 case matrix。

当前 fixture 相比 P4 提交只删除了旧配置，应用源码和声明的固定依赖版本未变。修复更新当前三个
摘要，保留原摘要作为历史证据，并检查 fixture `node_modules` 中每个直接依赖的实际版本。
没有恢复已废弃配置，也没有将当前输入校验冒充新的 Application Go。

## 后续更新

修改共享 lock、fixture 或 case matrix 后，应审查具体差异，再更新对应当前摘要；检查继续拒绝
未审查的字节漂移。直接依赖必须使用精确版本，安装状态也必须匹配。不得自动接受摘要漂移，
或者只为获得绿色结果改写历史证据。新的应用资格结论需要针对同一冻结产物重新执行两端
HTTP/Chromium、导入、部署与精确清理，独立保存报告。

回归入口是 `node --test test/conformance/vinext-application.test.mjs`（也包含在完整
`bun run test:js` 中）。回归在 `.temp/vinext-lock-fix/` 的隔离仓库验证 lock、fixture、matrix
以及安装版本漂移均会失败，不修改真实依赖或历史产物。
