# Cloudflare 上游刷新

状态：**已实现（P16，2026-09-14）**。本页定义 OpenAPI schema、官方 TypeScript SDK 与 Wrangler 的长期更新合同。发现
（scanner：`test/upstream-review/scanner.ts`，分类 fixture：`test/upstream-review/scanner.test.mjs`）与 scheduled
workflow（`.github/workflows/cloudflare-upstream-review.yml`，周一 06:23 UTC + `workflow_dispatch`）已接入；ready 候选的
frozen-identity Draft PR 由 `draft-pr` job 通过 `test/upstream-review/apply-candidate.ts` 机械再生。首次实际扫描
（2026-09-14）判定为 `blocked`：schema HEAD 前进但官方 SDK 7.1.0 未发布对应字段（AI Search `use_ocr`、
Queue create `jurisdiction`），且 Wrangler 需要协调评审。2026-09-25 经 three-way closure 接受 OpenAPI
`425ceea95cdaa4c43dd462279e42d74fbc00441e`、SDK `7.1.0` 与 Wrangler `4.138.0`；selected operation、生成 SDK 与固定
Wrangler 证据同步更新。随后只读 scan 已发现更新的 OpenAPI `01a855ec4bd180a1173f1b4587ef0fd0ca9f55e6` 改动四个 AI Search item operation，而 stable SDK
仍未表达这些变化，因此下一候选继续 `blocked`，formal pin 不移动。当前已接受的版本与支持面仍由
[`cloudflare-openapi.lock.json`](../../openapi/upstream/cloudflare-openapi.lock.json)、
[`cloudflare-subset-manifest.json`](../../openapi/cloudflare-subset-manifest.json) 和
[Cloudflare 兼容矩阵](cloudflare-compatibility.md)共同定义。

## 目标与更新单位

上游刷新以一个 **candidate compatibility set** 为更新单位，同时复核但不要求每次都改变：

- `cloudflare/api-schemas` 的精确 commit、blob 与 schema SHA-256；
- npm stable dist-tag 指向的 `cloudflare` 版本、tarball integrity/shasum、repository revision 与 selected resource declarations；
- npm stable dist-tag 指向的 `wrangler` 版本、tarball integrity/shasum、config schema 与 CLI identity。

“最新”表示经过验证的最新相容组合，不等于三个独立 moving latest 的机械拼接。schema 先出现而已发布 SDK 尚未表达的字段不是可接受
组合；定时运行要求持续发现、分类和留证，不要求每次运行都移动正式 pin。OpenAPI dialect、Cloudflare API version、schema revision、
SDK package version 和 Wrangler package version 是不同身份，不能把其中一个的变化代替其它输入的复核。

## 调度与只读发现

`.github/workflows/cloudflare-upstream-review.yml` 每周一在非整点运行，并支持 `workflow_dispatch`。scheduled workflow 只从默认分支
执行已提交的仓库内 scanner，以当前 lock 为基线解析三个上游的 stable/HEAD 身份。

发现 job 默认只有 `contents: read`。它只能把下载内容写入 `.temp/upstream-review/`，静态检查 schema 和 npm tarball；不得运行
upstream package lifecycle script、生成代码、执行下载的 Wrangler CLI，或持有 npm、GitHub Release、environment、deployment 与 OIDC
publish credential。常规构建和 `ocd` 启动继续完全离线，不调用该 workflow 或联网解析 latest。

scanner 输出 machine-readable `upstream-review.json` 与人类可读摘要，至少包含：

- 当前和候选的 immutable identities 与获取时间；
- selected OpenAPI operation 的新增、删除和语义变化；
- official SDK resource/method/type closure 的增删改和无法唯一映射项；
- Wrangler config schema、binding 与命令面的变化；
- 受影响的 capability、deviation、fixture、实现 owner 与资格 Gate；
- `ready`、`blocked` 或 `breaking` 判定及逐项原因。

所有 identity 都未变化时，job 成功且不创建噪音通知。有变化或 scanner 失败时上传 Actions artifact，并创建或更新唯一的
`Cloudflare upstream refresh` tracking issue；不得每周创建重复 issue。Actions artifact 是原始扫描证据，issue 保存稳定摘要和当前
阻塞状态，不把大段原始输出提交到文档。

## 候选分类

| 判定       | 条件                                                                                  | 自动动作                                                   |
| ---------- | ------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `ready`    | 候选组合通过静态 three-way closure，且 selected contract 没有未解释缺口               | 更新 tracking issue，并创建或刷新一个 Draft PR             |
| `blocked`  | schema、SDK、Wrangler 或当前 `ocd` 对 selected contract 不一致                        | 只更新 tracking issue，列出阻塞 operation/field 与等待对象 |
| `breaking` | selected path/method 删除、请求/响应不兼容、安全公告要求处理，或 scanner 无法证明映射 | workflow 失败并更新高优先级 tracking issue，不改 pin       |

schema 先于已发布 official SDK 出现新字段时必须保持 `blocked`，等待 SDK stable 发布后重新扫描；不得依赖未发布 package、Git branch
构建或本地伪造的官方类型。Wrangler 可以在 schema/SDK identity 不变时单独前进，但候选仍是一个完整三方组合，并必须重新生成全部
Wrangler/product evidence。

## Draft PR 与更新范围

`ready` 候选创建或刷新一个 Draft PR。PR 固定扫描当次解析出的 exact identities，不在 PR CI 中重新解析 npm `latest` 或 schema
HEAD；上游再次变化时由下一次 scanner 明确刷新 candidate，而不是让同一 CI run 漂移输入。

升级直接修改当前 Day1 模型，同一 PR 更新所有受影响生产者和消费者：

- 根 dependency catalog 与 `bun.lock`；
- `openapi/upstream/cloudflare-openapi.lock.json` 及其完整 tarball/blob/digest identity；
- Cloudflare subset、capability/deviation projection 与 generated SDK/surface report；
- Wrangler config/CLI evidence、fixtures、implementation 和用户文档；
- 与 selected contract 变化对应的持久化、validation、wire model 和 real-process tests。

不增加双 pin、version selector、旧 schema fallback 或兼容分支。机器人不得自动 merge、tag、publish npm package 或发布 GitHub Release。
普通 reporting job 只有 `issues: write`；创建 Draft PR 的独立 job 按需增加最小 `contents: write` 与 `pull-requests: write`，不能获得
release/package/environment 权限。branch、commit 与 PR 内容必须可追溯到 report 中的 frozen identities。

## 验收顺序

每个候选先完成便宜、确定的检查，再进入完整资格：

1. pin/tarball/blob identity 与 scanner classification fixtures；
2. subset、capability、deviation 和 generated SDK byte drift；
3. selected OpenAPI operation 与 official SDK method/type 的双向 closure；
4. Wrangler config schema、CLI request trace 与 focused product tests；
5. official SDK request trace、types、package inspection 与 isolated consumers；
6. 受影响的 real-process `ocd` Gate；
7. repository static checks、coverage，以及源码冻结后一次最终 workspace Gate。

任何一步失败都保持现有正式 pin，保留失败 artifact，并把精确差异写回 tracking issue；修复后重新生成 candidate，不跳过 Gate 或把
unknown 降级成 compatible。PR 合并后关闭当前 tracking cycle，下一次扫描以已合并 lock 为新基线。

## 节奏与升级策略

- 每周发现一次，并允许 maintainer 随时手动触发；
- selected contract 的安全修复、移除、弃用或 breaking change 立即进入人工评审；
- 普通 compatible patch/minor 候选最多按月合并一次，减少重复完整 Gate 成本；
- 没有 `ready` 组合时允许 pin 保持不动，tracking issue 清楚记录等待 Cloudflare schema、SDK、Wrangler 或本仓库实现中的哪一方；
- 接受的 pin 更新进入正常版本和发布流程，不单独覆盖已发布 SDK/`ocd` 版本，也不移动既有 tag。

scanner、classification fixture 与 workflow 本身属于普通 CI 检查范围。上游布局、npm metadata 或 schema 格式变化导致 scanner 无法继续时，
必须显式失败并更新 issue，不能把“没有结果”解释为“没有更新”。
