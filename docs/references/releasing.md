# 版本与发布流程

macOS 的文档解析功能完整保留，但解析子进程尚无可强制执行的内存硬上限。
0.1.0 接受该限制；CPU、输入/输出、并发和超时约束继续生效。
该进程复用同一个 `ocd`，不属于 workerd Worker isolate 的额度，也不增加 sidecar 分发文件。
宿主内存压力仍可能影响主服务，后续工作见 [macOS 内存限制 TODO](../p5-8-macos-document-parser.md)。

open-compute 只发布标准稳定版本和三个正式平台的原生单文件 `ocd`。版本使用不带预发布或构建后缀的
SemVer：Cargo 版本写作 `X.Y.Z`，Git tag 写作 `vX.Y.Z`。不使用 `alpha`、`beta`、`rc`、
`alpha.1` 或浮动的 nightly 版本。

GitHub Releases 是公开二进制的唯一权威来源。每个 release 固定包含：

- `ocd-vX.Y.Z-darwin-arm64`；
- `ocd-vX.Y.Z-linux-arm64`；
- `ocd-vX.Y.Z-linux-x64`；
- `release.json`：版本、Git revision、正式 workerd pin/lock 摘要和逐目标文件身份；
- `SHA256SUMS`：三个二进制与 `release.json` 的 SHA-256。

Windows 和 macOS Intel 不提供官方二进制、CI package 或 GitHub Release asset。需要在目标机器上使用自己的
Rust/Bun/Bazel 工具链，从源码手动编译，并显式提供与正式 lock 匹配的 workerd 输入；该路径不属于正式发布资格。

不发布 Rust crate、sidecar 布局、外部 workerd、安装器或自动更新通道。npm 发布仅限
`@open-compute/sdk` 一个 public scoped package，且只使用 `NPM_ACCESS_TOKEN` secret
作为 registry 凭据；该 token 存放于 GitHub Actions secrets，不进入仓库、镜像或本机文件。
npm provenance（OIDC trusted publishing）在当前 token 流程下不可用，这是接受的限制；
切换到 trusted publishing 是后续工作。
`open-compute.dev` 可以提供人类可读的下载入口，但必须链接到上述不可变 GitHub Release assets，
不能维护第二套可独立替换的二进制镜像。

构建 job 必须以 `lfs: true` 检出 `share/workerd/` 的固定依赖。setup action 从宿主二进制
离线准备正式 archive；根 build 验证三个正式目标。无需预先发布 fork archive，也不下载 stock runtime。
更新依赖须同步三个正式目标的 LFS 对象及 `packages/runtime/workerd.lock.json`；macOS Intel 的固定输入仅供手动编译，向远端推送前用
`git lfs fsck` 检查本地对象，不能只上传 pointer。生产分发仍只包含每个平台的 `ocd`。

## 三条工作流

`.github/workflows/ci.yml` 只在 `main` push 和以 `main` 为 base 的 pull request 上执行静态资格：
所有变更先在同一个只读 `failfast` job 中完成路径分类、`sourceDigest` 和 release-tool contracts；通过后再按路径选择
runtime/tooling build 与 typecheck、快速 JS/Python 测试、format、clippy、no-default-features、
Rust 1.98 workspace/all-targets check、production hygiene、metadata 和依赖边界。普通 CI 不执行完整
workspace Gate、coverage 或发行打包。
full scope 将 core checks、Clippy 与 production executable hygiene 分成三个并行 matrix leg；每个 Cargo leg
都先执行完整 `bun run build`，汇总 `ci` 只有在三者全部成功后才通过。
只修改 release/recovery/dry-run workflow、release assembler/test 和随附文档时使用 `release-tooling`
scope，只执行 TypeScript、format、文档与 release contract；修改 `ci.yml`、共享 setup action、Rust/runtime
或未明确归属的路径仍执行 full scope。baseline 只有 `sourceDigest` 字段变化时是随附身份更新，不会单独扩大
owning change 的 scope；若它作为修复提交单独 push，分类器会回溯到上一次 baseline revision，并对期间所有
owning files 重新分类。baseline 任何其他字段变化或无法证明来源时仍 fail closed 到 full scope。文档与 frontend
混合时执行对应 frontend build 和文档检查，不为此启动 Rust。
release PR 复用其 main head 已通过的 push check，不再重复执行相同检查；tag 触发的 release workflow
会校验 release merge commit 对应的 main source commit 已通过该 pre-check。各 PR 与分支使用独立
concurrency group，取消过期运行；汇总 job `ci` 是 `release` 分支的 required check。
`main` 是无分支保护的开发分支，`release` 是受保护的版本发布分支；默认分支仍为 `main`。

`.github/workflows/release.yml` 只由 `v*` tag push 触发；工作流首先拒绝不满足以下全部条件的 tag：

1. tag 是严格的 `vX.Y.Z`，且没有前导零或预发布/构建后缀；
2. tag 是 annotated tag；
3. tag、checkout 和 `GITHUB_SHA` 指向同一个 commit；
4. 该 commit 精确等于 tag workflow 拉取到的当前 `origin/release` HEAD；较早的 `main` 或 `release` ancestor 都会被拒绝；
5. tag 版本等于根 `Cargo.toml` 的 `[workspace.package].version`；
6. checkout 干净；
7. release merge commit 对应的 main source commit 已通过 `main` push 的 `ci.yml` pre-check。
8. `docs/releases/X.Y.Z.md` 存在且是已提交的普通文件，至少包含 1000 bytes，并完整包含
   What's new、Workerd、Fixed、Before you upgrade、Install or upgrade、Downloads、Security、Known limitations、
   Verification 和 Thanks 章节（从 P16 起 `## SDK` 也是必填章节：记录 `@open-compute/sdk`
   package/version、official SDK/OpenAPI pins、surface 变化、安装命令与 npm version link）；
   不得保留 TODO、TBD 或 PLACEHOLDER。Workerd 与 Thanks 的内容要求见下文。

上述规则由唯一的 `failfast` job 持有，所有 coverage、integration、package 和 SDK package job 都只在它
成功后启动。该 job 绑定 `release` environment，并只做秒级检查：tag/release/source identity、版本与干净
checkout、main CI 证据、release notes 章节和占位符、Workerd 章节的精确 gitlink 或
`submodule revision is unchanged` 句子、Thanks、`sourceDigest`、release-tool 单元契约、
`NPM_ACCESS_TOKEN`/`npm whoami` 以及目标 npm 版本尚未发布。SDK report 由 packaging 与 assembler 共用同一个
parser，producer 在写报告前即调用 consumer parser；schema 漂移不再等到三平台产物完成后才发现。
`failfast` 不拉取 LFS、不构建 Rust/workerd、不跑 coverage 或完整 Gate。

校验通过后，release workflow 并行执行 90% Rust 行覆盖率、macOS 上完整单轮最终 workspace Gate、
Linux 上仅 `p0-2` 受控 egress fixture，以及三个正式平台打包。Linux egress 不再夹带第二轮
`--workspace`；静态资格直接复用 release source commit 已通过的 main `ci`，不在 tag workflow 重跑。
三个原生 runner 使用正式 workerd lock 打包自己的 `ocd`，并以 `OPEN_COMPUTE_TEST_OCD` 跑单文件隔离、
首启、重启和损坏拒绝测试。Linux ARM64 package 还对 production binary 执行精确的 embedded Dashboard
smoke：未认证 shell、admin session、account discovery、顶层 reload 和真实 Worker 创建/删除。完整 Dashboard
E2E 依赖专用多实例与可选产品 fixture，不在干净的 production package scope 中冒充发行资格。

`publish` 明确依赖 main 静态资格、coverage、macOS 最终 Gate、Linux egress 和三个正式平台 assemble；
任何一项未通过均不得公开发布。构建保存编译耗时和标明未验收的二进制；普通 Rust target cache
不保存失败半成品，package 的 bounded sccache 只作为编译加速，不作为测试通过证据或可信发行物。
缓存和任务依赖设计见 [CI 构建性能](ci-build-performance.md)。

npm 发布以 `npm publish` 成功退出为完成信号，成功后不轮询 registry，也不等待 eventual-consistency read-back。
只有 `npm publish` 返回失败时才做一次只读身份查询：若同版本已经存在且 tarball shasum/integrity 与已验证
artifact 完全一致，则把它视为此前调用结果未知但发布已完成；否则立即失败并交给 `release-recovery`，不自动重发。

只读 `assemble` job 只接受三个精确命名的二进制和对应 package report；它重新核对版本、revision、workerd pin、
lock SHA-256、文件大小与文件 SHA-256，然后生成 `release.json` 和 `SHA256SUMS`。工作流的默认权限
是只读，只有 `release` environment 中的最后一个 job 获得 `contents: write`。该 job 只使用随 tag 提交并通过上述结构
校验的版本说明，不使用 GitHub 自动生成的 PR 标题列表。它先创建 Draft
GitHub Release，上传五个公开 assets，再全部下载回来逐字节比较并执行 `sha256sum --check`；全部通过
后才把 Draft 变成正式 latest release。任一目标或回读校验失败时，不会出现部分公开 release。

`.github/workflows/release-dry-run.yml` 是只读发布预检入口。手动指定 `ref` 和 `target` 后，它构建并验证
SDK、原生包及 `single-binary` Gate；构建前同样运行 source/release-tool `failfast`，`target=all` 另外验证
三平台 artifact 组装。该 workflow 不创建
GitHub Release、不发布 npm、不创建或移动 tag，也不替代正式 tag workflow 的 coverage、workspace Gate、
受控 egress 和公开资产回读。
单目标输入只创建对应平台 runner；package 在 cache 可用时只读恢复 main 的 default Rust target cache 来复用
`single-binary` 测试 harness 依赖（没有对应平台 cache 时正常冷编译），release profile 编译继续使用独立 bounded sccache，且产物仍从当前源码
按正式 release profile 完整构建和验证。

CI 和 release 都使用 `bun run test:js:ci` 的平台工具/runtime 测试集合。第三方应用 qualification
独立执行，不属于 workspace Gate 或此次原生二进制发行资格。当前 `test:js` 额外包含的 vinext
冻结输入检查存在 `root lock digest drift`；旧应用报告不证明当前源码，不能因此声称当前版本通过
vinext/Next.js 端到端或 hosted Cloudflare differential。其冻结摘要和历史报告保持原样。

## 发布一个版本

`main` 是开发和版本准备的唯一来源，`release` 是唯一的长期发布分支；不要创建
`release/0.1.1` 这类按版本命名的分支。版本候选应从最新 `main` 推进到 `release`，而不是从
旧的 `release` 反向开发：

1. 在最新 `main` 修改根 `Cargo.toml` 的 workspace 版本为新的 `X.Y.Z`，并在同一个 version PR 中把
   `packages/sdk/package.json` 的 `version` 改为同一 `X.Y.Z`（`@open-compute/sdk` 与 `ocd` 共享同一
   stable 版本，没有独立 SDK tag；release workflow 的 `failfast` job 会拒绝 SDK 与 workspace 版本不一致
   的 tag）；
2. 运行 Cargo，让它更新 `Cargo.lock` 中所有 workspace package 的版本，不手改 lockfile；
3. 检查并上传 Git LFS 实体，不能只把 pointer 推到 Git：

   ```sh
   git lfs fsck
   git lfs push origin --all
   ```

4. 在版本候选源码冻结后，先刷新 `test/conformance/baseline.json` 的 `sourceDigest`：用
   `bun -e 'import { sourceIdentity } from "./test/conformance/checks/context.ts"; console.log(sourceIdentity())'`
   计算当前值并写回 baseline，然后执行一次 `./test/gate.py p3-contract` 确认匹配。它是源码内容摘要，
   不是 Git commit ID；凡影响摘要范围的源码、测试、工具链、manifest 或 `docs/references/**` 变更，都要一起更新。
   随后在本地干净 checkout 完成发布预检。必须显式准备正式 workerd，先运行 `bun run build` 和静态检查，
   再用宿主对应的 `OPEN_COMPUTE_TEST_WORKERD` 依次执行一次
   `./test/coverage.sh --jobs 2` 与一次 `./test/gate.py --workspace --jobs 2`。coverage 的插桩 Gate 和最终
   未插桩 Gate 各有不同验收职责；除此之外不再运行重复 aggregate。90% Rust 行覆盖率和最终 Gate
   必须通过后才能 push/tag。保存失败证据，不自动重试；本地预检用于尽早拦截，不替代 tag workflow
   的独立 runner 资格。
5. 新建 `docs/releases/X.Y.Z.md` 并加入 `docs/releases/README.md`。该文件是 GitHub Release 的正文片段，
   不写 `# open-compute X.Y.Z` 或其他一级标题；workflow 的 `--title "open-compute X.Y.Z"` 是唯一页面标题。
   写法参考成熟自托管项目的 operator-first release notes：开头用一段话说明这版解决什么问题、适合谁；随后按 What's new 和 Fixed 归纳用户可感知的变化；
   Before you upgrade 必须明确数据/配置兼容性、是否需要停机或人工动作，即使答案是“无”；Install or upgrade 给出可直接执行的
   版本固定命令；Downloads 列出支持平台和精确资产名；Security 明确安全公告或“无已知公告”；Known limitations 只列会影响部署决策的
   现实边界；Verification 只能陈述这个 revision 实际完成的资格。最后附完整 diff 链接，PR/commit 列表只能作为补充，不能替代上述内容。

   `## Workerd` 必须单独成章，不能只在父仓库摘要里写“升级了 runtime”。比较上一个正式 tag 与本版本的
   `third_party/workerd` gitlink：若 revision 变化，写出旧 revision、新 revision 和正式 pin，并列出这段
   submodule range 上的每一个提交（至少 12 位 hash 和提交说明）；内容按运维可观察的行为写，不要只贴父仓库
   pin 字符串。若 gitlink 未变化，本章必须写明 submodule revision 未变化，并引用当前正式 pin。`--version`
   日期字符串不是 pin 身份；日期未变时要明确说出来，避免运维把未变化的版本输出当成升级失败。

   `## Thanks` 必须单独成章。从上一个正式 tag 的提交时间起，到本版本为止，每一个以 completed 关闭、且提出者
   不是仓库 owner、也不是 bot 的 issue，都要在本章点名提出者的 GitHub login、链接该 issue，并明确致谢。
   维护者自己提出的 issue 写进 Fixed 或 What's new 即可，不要感谢自己。若这段时间没有外部提出者的已完成 issue，
   本章必须写精确句子 `No external issue reports were closed in this release.`，不能省略章节或留空。

6. 提交版本变更与 release notes 到 `main`，等待 main 的静态 `ci` 通过。main CI 先完成 `sourceDigest` 与
   release-tool contract fail-fast，再完成 build、快速
   JS/Python、fmt、clippy、no-default-features、Rust 1.98 workspace check、production hygiene、metadata
   和边界检查；coverage、完整 workspace Gate、Linux egress、三个正式平台打包和发布验证由 tag
   触发的 release workflow 负责；
7. 以 `main` 为 head、`release` 为 base 创建并合并一个 version PR。`release` 受保护，不能直接
   推送，也不能通过按版本创建临时分支绕过 PR；
8. 确认 PR 合并产生的精确 `release` commit 已包含通过的 main pre-check，再在干净的本地 `release`
   上创建 annotated tag。tag workflow 只重验秒级 release identity/environment contracts，不重复 Clippy、
   MSRV 或其他 main 静态检查。

不要让 GitHub Actions 自动决定版本、修改文件、创建 tag 或把任意 branch HEAD 发布出去。版本是一次
需要 review 的源码变更，tag 是 maintainer 对已经合入 `release` 的精确 commit 做出的发布决定。

合并并确认 main pre-check 已通过后，由 maintainer 在干净的最新 `release` 上创建 annotated tag：

```sh
git switch release
git pull --ff-only origin release
test -z "$(git status --porcelain --untracked-files=all)"
git tag -a vX.Y.Z -m "open-compute vX.Y.Z"
git push origin vX.Y.Z
```

每个 Gate job 都先显式执行 `bun run build` 和 `cargo fetch --locked`；打包脚本独立从源码构建。
最终 Gate 不设置三轮诊断变量，遵循[单轮测试政策](testing.md)。
共享 setup 将 Cargo registry/git 下载与编译产物分开缓存：下载缓存允许 `Cargo.lock` 变化时按 OS 回退，
coverage 保留独立 instrumented target cache；package 只使用 bounded sccache，不重复保存 Cargo target。
release 的 coverage、最终 Gate、Linux egress 与 package 只依赖身份校验并同时启动，发布墙钟由最慢路径
决定，不再把这些长任务串行相加。

push tag 是唯一发布触发器。随后在 GitHub Actions 的 `release` workflow 中确认所有 qualification、
三个正式目标 package 和 `publish` job 成功，并在 GitHub Release 页面核对五个 assets。仓库已配置以下设置（2026-09-06 按用户要求迁移）：

- main 分支不启用分支保护；版本从 main 推进到唯一的 release 分支；release 分支要求 PR、最新 required `ci` 成功和讨论解决，禁止强推/删除；管理员同样受检查约束；
- `Release tags` ruleset 限制 `v*` tag 创建/更新/删除，仅 repository admin maintainer 可 bypass；
- `release` environment 仅允许 `v*` tag，发布入口由上述 maintainer tag 规则控制；
- 启用 GitHub immutable releases，使已发布 tag 和 assets 不能被修改或删除；
- Actions 默认 token 权限保持 read-only，由 workflow 仅给 `publish` job 提升 `contents: write`。

已发布版本的说明与 GitHub Release 入口见 [Release notes](../releases/README.md)。

## 安装与校验

优先下载并审阅仓库正式 [`scripts/install.sh`](../../scripts/install.sh)，再以普通用户执行 `sh install.sh`。脚本从公开
GitHub Releases 安装匹配 OS/CPU 的 `ocd` 到 `$HOME/.local/bin/ocd`，并在同一 prefix 写入不含 secret 的 receipt；
root 调用才默认使用 `/usr/local`。脚本在联网前检查两个目标目录，显式 prefix/destination/receipt override 始终优先。
也可手工下载资产与 `SHA256SUMS` 后安装。例如 Linux x64 system-wide 手工路径：

```sh
curl -fLO https://github.com/elliothux/open-compute/releases/download/v0.1.0/ocd-v0.1.0-linux-x64
curl -fLO https://github.com/elliothux/open-compute/releases/download/v0.1.0/SHA256SUMS
grep '  ocd-v0.1.0-linux-x64$' SHA256SUMS | sha256sum --check
sudo install -m 0755 ocd-v0.1.0-linux-x64 /usr/local/bin/ocd
/usr/local/bin/ocd --version
```

macOS 使用 `shasum -a 256 -c` 校验筛选后的对应行。校验后仍应按
[单二进制分发与部署](single-binary.md)与[安装与首次启动](runbooks/install-and-first-start.md)完成配置、
`config check` 和首次启动。运维命令面见
[P11 实现](../implemented/p11-ocd-operator-experience.md)；三目标正式安装冒烟资格见
[P11 验收计划](../acceptance/p11-operator-experience-acceptance.md)。

## 失败、重跑与修复版本

- qualification 或 package 暴露源码/产物缺陷：若该 tag 尚未创建公开 GitHub Release，maintainer
  明确授权后可以在保留失败 run 证据的前提下，用修复后的同版本 release commit 替换该 tag；
  已公开的 Release 和 tag 仍不可移动，后续修复必须走新的 patch version PR。
- runner、网络或 GitHub 服务的瞬时失败：输入未变化时可以对同一 tag rerun failed jobs；不得借重跑替换
  tag、源码或任何 package 输入。
- Draft 已创建但上传/回读失败：Draft 保持非公开。publish 与
  [`release-recovery`](../../.github/workflows/release-recovery.yml) 都先确认已有 Draft 是否仍为同一
  tag，再下载并逐字节校验，不覆盖已有 asset；qualification 成功而 assemble/publish 失败时，使用
  recovery 的 `tag + source_run_id` 复用成功 artifact，不重跑测试。
- release 已公开：视为不可变。发现缺陷时发布新的 patch 版本，例如 `v0.1.1`；不要替换二进制、移动
  tag 或删除旧版本来伪装相同版本。
- 本地与远端存在同名 tag 但指向不同对象时，先保留远端正式 tag，不要 force push 或替换远端 tag；
  只删除/重建本地副本，确认当前版本输入后使用新的 patch 版本继续发布。
- 某个平台没有成功产物：整个版本不发布，不能先公开其余三个平台。

workflow 配置存在只说明流程已定义；只有某个 tag 的 workflow 实际成功且 GitHub Release 已公开，
才能声称该版本完成跨平台发行资格。长时 soak、签名、公证或其他尚未接入 workflow 的资格继续按
active acceptance 文档记录，不能由 tag 发布成功替代。
