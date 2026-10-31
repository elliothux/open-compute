# 文档规则与参考资料

## 文档位置

| 位置                             | 内容                             |
| -------------------------------- | -------------------------------- |
| `docs/*.md`、`docs/workerd/`     | 待实施的方案                     |
| `docs/blocked/`                  | 被外部条件阻塞的方案             |
| `docs/releases/`                 | 版本说明                         |
| `docs/references/`               | 当前的接口、测试、发布和运维资料 |
| `docs/security.md`               | 威胁模型                         |
| `apps/website/src/content/docs/` | 用户文档，中英两份结构一致       |

方案完成后，把当前合同写入参考文档或用户文档，然后删除方案文件。历史留在 Git、PR 和 issue 中。
不建 qualification、acceptance 或测试计划文档。移动或删除文档时同步所有入链，不留 redirect 或 stub。

新文档用领域名命名，例如 `external-proxy-origins.md`，不加阶段编号。

## 写作规则

1. 第一句写结论，一段不超过三句。
2. 只写现在是什么。历史写进 commit 和 issue。
3. 不写“不做什么”，除非读者很可能误解。
4. 不写免责声明。只报告实际执行并通过的检查。
5. 一个概念一个词。authority、qualification、evidence 这类词必须先定义，否则换成具体的词。
6. 脚本行为写在脚本的注释和 `--help` 里。文档只写何时用、怎么调。
7. 每个文件有预算：`AGENTS.md` ≤ 3 KB，单篇参考文档 ≤ 10 KB。超了先删，再考虑拆。

`AGENTS.md` 只写全局规则，目录里的 `AGENTS.md` 只写局部规则，操作细节放在参考文档里。

## 检查

文档改动运行 `git diff --check` 和 `bun scripts/check-docs.ts`，并核对改动的链接和命令。`check-docs.ts`
只检查网站文档和 README 中的站内链接。根 `AGENTS.md`、`docs/references/**` 和 `.agents/skills/**` 属于源码摘要输入，
改动后按[测试说明](testing.md#按输入选择测试)同步 `sourceDigest`。

## 参考资料

| 文档                                                  | 用途                                       |
| ----------------------------------------------------- | ------------------------------------------ |
| [测试](testing.md)                                    | Gate、静态检查和 coverage                  |
| [单二进制分发](single-binary.md)                      | 构建输入、离线启动和发行合同               |
| [发布流程](releasing.md)                              | 版本、tag、release workflow 和校验         |
| [CI 构建性能](ci-build-performance.md)                | CI 拓扑、并行和缓存                        |
| [Cloudflare 兼容矩阵](cloudflare-compatibility.md)    | 当前支持面和偏差                           |
| [Cloudflare 上游刷新](cloudflare-upstream-refresh.md) | OpenAPI、官方 SDK 与 cf 的定期刷新         |
| [能力偏差](p1-deviations.md)                          | 偏差 ID 与边界                             |
| [Host authority](host-authority.md)                   | hostname 归属、解析和 endpoint 投影        |
| [Containers 运维](containers.md)                      | Broker、provider 和 execution 恢复         |
| [workerd fork](workerd.md)                            | fork 身份、定制能力、正式 pin 与二进制下载 |
| [workerd 上游](workerd-upstream.md)                   | 上游能力与 fork 升级重点                   |
| [Vinext 输入](vinext-input-validation.md)             | 固定输入与离线校验                         |
| [Fuzz 所有权](p1-fuzz-ownership.md)                   | 输入 corpus 与回归归属                     |

运维手册在 [`runbooks/`](runbooks/)，编译时嵌入 `ocd`。改路径或文件名时同步 `crates/service/src/resources.rs` 和测试。
