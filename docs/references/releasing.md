# 版本与发布流程

macOS 的文档解析功能完整保留，但解析子进程尚无可强制执行的内存硬上限。
0.1.0 接受该限制；CPU、输入/输出、并发和超时约束继续生效。
该进程复用同一个 `ocd`，不属于 workerd Worker isolate 的额度，也不增加 sidecar 分发文件。
宿主内存压力仍可能影响主服务，后续工作见 [macOS 内存限制 TODO](../macos-document-parser.md)。


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

不发布 Rust crate、npm package、sidecar 布局、外部 workerd、安装器或自动更新通道。
`open-compute.dev` 可以提供人类可读的下载入口，但必须链接到上述不可变 GitHub Release assets，
不能维护第二套可独立替换的二进制镜像。

构建 job 必须以 `lfs: true` 检出 `share/workerd/` 的固定依赖。setup action 从宿主二进制
离线准备正式 archive；根 build 验证三个正式目标。无需预先发布 fork archive，也不下载 stock runtime。
更新依赖须同步三个正式目标的 LFS 对象及 `packages/runtime/workerd.lock.json`；macOS Intel 的固定输入仅供手动编译，向远端推送前用
`git lfs fsck` 检查本地对象，不能只上传 pointer。生产分发仍只包含每个平台的 `ocd`。

## 两条工作流

`.github/workflows/ci.yml` 只在 `main` push 和以 `main` 为 base 的 pull request 上执行轻量检查：
显式 runtime/tooling build 与 typecheck、快速 JS/Python 测试、format、Rust 1.98 workspace/all-targets
check、metadata 和依赖边界。普通 CI 不执行完整 workspace Gate、coverage 或发行打包。
release PR 复用其 main head 已通过的 push check，不再重复执行相同检查；tag 触发的 release workflow
会校验 release merge commit 对应的 main source commit 已通过该 pre-check。各 PR 与分支使用独立
concurrency group，取消过期运行；汇总 job `ci` 是 `release` 分支的 required check。
`main` 是无分支保护的开发分支，`release` 是受保护的版本发布分支；默认分支仍为 `main`。

`.github/workflows/release.yml` 只由 `v*` tag push 触发；工作流首先拒绝不满足以下全部条件的 tag：

1. tag 是严格的 `vX.Y.Z`，且没有前导零或预发布/构建后缀；
2. tag 是 annotated tag；
3. tag、checkout 和 `GITHUB_SHA` 指向同一个 commit；
4. 该 commit 已经可从 `origin/release` 到达；
5. tag 版本等于根 `Cargo.toml` 的 `[workspace.package].version`；
6. checkout 干净；
7. release merge commit 对应的 main source commit 已通过 `main` push 的 `ci.yml` pre-check。

校验通过后，release workflow 才执行 Linux/macOS 静态检查、90% Rust 行覆盖率、完整单轮
最终 workspace Gate（coverage 成功后执行），以及 Linux 受控 egress fixture。三个原生 runner 在身份校验后立即并行使用正式
workerd lock 打包自己的 `ocd`，并以 `OPEN_COMPUTE_TEST_OCD` 跑单文件隔离、首启、重启和损坏拒绝测试。

打包可以与资格验证并行，但 `publish` 明确依赖全部静态检查、coverage、最终 Gate 和三个正式平台 assemble；
任何一项未通过均不得公开发布。失败构建保存缓存、编译耗时和标明未验收的二进制，不作为公开发行物。
缓存和任务依赖设计见 [CI 构建性能](ci-build-performance.md)。

只读 `assemble` job 只接受三个精确命名的二进制和对应 package report；它重新核对版本、revision、workerd pin、
lock SHA-256、文件大小与文件 SHA-256，然后生成 `release.json` 和 `SHA256SUMS`。工作流的默认权限
是只读，只有 `release` environment 中的最后一个 job 获得 `contents: write`。该 job 先创建 Draft
GitHub Release，上传五个公开 assets，再全部下载回来逐字节比较并执行 `sha256sum --check`；全部通过
后才把 Draft 变成正式 latest release。任一目标或回读校验失败时，不会出现部分公开 release。

CI 和 release 都使用 `bun run test:js:ci` 的平台工具/runtime 测试集合。第三方应用 qualification
独立执行，不属于 workspace Gate 或此次原生二进制发行资格。当前 `test:js` 额外包含的 vinext
冻结输入检查存在 `root lock digest drift`；旧应用报告不证明当前源码，不能因此声称当前版本通过
vinext/Next.js 端到端或 hosted Cloudflare differential。其冻结摘要和历史报告保持原样。

## 发布一个版本

`main` 是开发和版本准备的唯一来源，`release` 是唯一的长期发布分支；不要创建
`release/0.1.1` 这类按版本命名的分支。版本候选应从最新 `main` 推进到 `release`，而不是从
旧的 `release` 反向开发：

1. 在最新 `main` 修改根 `Cargo.toml` 的 workspace 版本为新的 `X.Y.Z`；
2. 运行 Cargo，让它更新 `Cargo.lock` 中所有 workspace package 的版本，不手改 lockfile；
3. 检查并上传 Git LFS 实体，不能只把 pointer 推到 Git：

   ```sh
   git lfs fsck
   git lfs push origin --all
   ```

4. 在版本候选源码冻结后，先在本地干净 checkout 执行一次 coverage preflight。必须显式准备正式
   workerd，先运行 `bun run build`，再用宿主对应的 `OPEN_COMPUTE_TEST_WORKERD` 执行
   `./test/coverage.sh --jobs 2`；90% Rust 行覆盖率和其中的单轮 workspace Gate 都必须通过。保存失败
   证据，不自动重试；这次本地 coverage 是发版前的单轮拦截，不替代 tag workflow 的独立 coverage。
5. 提交版本变更到 `main`，等待 main 的轻量 `ci` 通过。main CI 只做 build、快速 JS/Python、fmt、
   workspace check、metadata 和边界检查；clippy、no-default-features、coverage、完整 workspace
   Gate、三个正式平台打包和发布验证由 tag 触发的 release workflow 负责；
6. 以 `main` 为 head、`release` 为 base 创建并合并一个 version PR。`release` 受保护，不能直接
   推送，也不能通过按版本创建临时分支绕过 PR；
7. 确认 PR 合并产生的精确 `release` commit 已包含通过的 main pre-check，再在干净的本地 `release`
   上创建 annotated tag。release 分支不再重复运行同一套轻量 pre-check。

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

push tag 是唯一发布触发器。随后在 GitHub Actions 的 `release` workflow 中确认所有 qualification、
三个正式目标 package 和 `publish` job 成功，并在 GitHub Release 页面核对五个 assets。仓库已配置以下设置（2026-09-06 按用户要求迁移）：

- main 分支不启用分支保护；版本从 main 推进到唯一的 release 分支；release 分支要求 PR、最新 required `ci` 成功和讨论解决，禁止强推/删除；管理员同样受检查约束；
- `Release tags` ruleset 限制 `v*` tag 创建/更新/删除，仅 repository admin maintainer 可 bypass；
- `release` environment 仅允许 `v*` tag，发布入口由上述 maintainer tag 规则控制；
- 启用 GitHub immutable releases，使已发布 tag 和 assets 不能被修改或删除；
- Actions 默认 token 权限保持 read-only，由 workflow 仅给 `publish` job 提升 `contents: write`。

首次 `0.1.0` 的源码范围、验证状态和已知限制记录在[发行验收](../acceptance/first-release-0.1.0.md)。

## 安装与校验

优先使用仓库正式 [`scripts/install.sh`](../../scripts/install.sh)（审阅后）从公开 GitHub Releases
安装匹配 OS/CPU 的 `ocd` 到 `/usr/local/bin/ocd`，并写入不含 secret 的 install receipt。也可手工下载资产与
`SHA256SUMS` 后安装。例如 Linux x64 手工路径：

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
- Draft 已创建但上传/回读失败：Draft 保持非公开。确认失败证据后可删除该 Draft，再对同一、未移动的
  tag rerun failed jobs；不能覆盖已存在的 asset。
- release 已公开：视为不可变。发现缺陷时发布新的 patch 版本，例如 `v0.1.1`；不要替换二进制、移动
  tag 或删除旧版本来伪装相同版本。
- 本地与远端存在同名 tag 但指向不同对象时，先保留远端正式 tag，不要 force push 或替换远端 tag；
  只删除/重建本地副本，确认当前版本输入后使用新的 patch 版本继续发布。
- 某个平台没有成功产物：整个版本不发布，不能先公开其余三个平台。

workflow 配置存在只说明流程已定义；只有某个 tag 的 workflow 实际成功且 GitHub Release 已公开，
才能声称该版本完成跨平台发行资格。长时 soak、签名、公证或其他尚未接入 workflow 的资格继续按
active acceptance 文档记录，不能由 tag 发布成功替代。
