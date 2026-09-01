# 版本与发布流程

open-compute 只发布标准稳定版本和四个平台的原生单文件 `ocd`。版本使用不带预发布或构建后缀的
SemVer：Cargo 版本写作 `X.Y.Z`，Git tag 写作 `vX.Y.Z`。不使用 `alpha`、`beta`、`rc`、
`alpha.1` 或浮动的 nightly 版本。

GitHub Releases 是公开二进制的唯一权威来源。每个 release 固定包含：

- `ocd-vX.Y.Z-darwin-arm64`；
- `ocd-vX.Y.Z-darwin-x64`；
- `ocd-vX.Y.Z-linux-arm64`；
- `ocd-vX.Y.Z-linux-x64`；
- `release.json`：版本、Git revision、正式 workerd pin/lock 摘要和逐目标文件身份；
- `SHA256SUMS`：四个二进制与 `release.json` 的 SHA-256。

不发布 Rust crate、npm package、sidecar 布局、外部 workerd、安装器或自动更新通道。
`open-compute.dev` 可以提供人类可读的下载入口，但必须链接到上述不可变 GitHub Release assets，
不能维护第二套可独立替换的二进制镜像。

## 两条工作流

`.github/workflows/ci.yml` 在分支 push 和 pull request 上运行。它负责 MSRV、TypeScript/生成资产、
JS/Python 测试、format、Clippy、no-default-features、metadata、production hygiene、依赖边界，以及
Ubuntu 上一个完整 workspace Gate round。普通 CI 不执行 release packaging，不创建 GitHub Release，
也不上传公开二进制。分支保护只需要把汇总 job `ci` 设为 required check。

`.github/workflows/release.yml` 只由 `v*` tag push 触发；工作流首先拒绝不满足以下全部条件的 tag：

1. tag 是严格的 `vX.Y.Z`，且没有前导零或预发布/构建后缀；
2. tag 是 annotated tag；
3. tag、checkout 和 `GITHUB_SHA` 指向同一个 commit；
4. 该 commit 已经可从 `origin/main` 到达；
5. tag 版本等于根 `Cargo.toml` 的 `[workspace.package].version`；
6. checkout 干净。

校验通过后，release workflow 才执行 Linux/macOS 静态检查、90% Rust 行覆盖率、完整一次加时序用例
两次的最终 Gate，以及 Linux 受控 egress fixture。所有资格校验成功后，四个原生 runner 分别使用正式
workerd lock 打包自己的 `ocd`，并以 `OPEN_COMPUTE_TEST_OCD` 跑单文件隔离、首启、重启和损坏拒绝测试。

聚合 job 只接受四个精确命名的二进制和对应 package report；它重新核对版本、revision、workerd pin、
lock SHA-256、文件大小与文件 SHA-256，然后生成 `release.json` 和 `SHA256SUMS`。发布 job 的默认权限
是只读，只有 `release` environment 中的最后一个 job 获得 `contents: write`。该 job 先创建 Draft
GitHub Release，上传六个公开 assets，再全部下载回来逐字节比较并执行 `sha256sum --check`；全部通过
后才把 Draft 变成正式 latest release。任一目标或回读校验失败时，不会出现部分公开 release。

## 发布一个版本

先提交一个普通 version PR：

1. 从最新 `main` 建分支，将根 `Cargo.toml` 的 workspace 版本改为新的 `X.Y.Z`；
2. 让 Cargo 正常更新并提交 `Cargo.lock` 中所有 workspace package 的版本，不手改 lockfile；
3. 确保 PR 描述完整列出用户可见变化、Cloudflare compatibility 变化、workerd pin 变化和已知限制；
4. 等待 required `ci` 通过并完成 review，然后合并到 `main`。

不要让 GitHub Actions 自动决定版本、修改文件、创建 tag 或把任意 branch HEAD 发布出去。版本是一次
需要 review 的源码变更，tag 是 maintainer 对已经合入 `main` 的精确 commit 做出的发布决定。

合并并确认 `main` CI 通过后，由 maintainer 在干净的最新 `main` 上创建 annotated tag：

```sh
git switch main
git pull --ff-only origin main
test -z "$(git status --porcelain --untracked-files=all)"
git tag -a v0.1.0 -m "open-compute v0.1.0"
git push origin v0.1.0
```

push tag 是唯一发布触发器。随后在 GitHub Actions 的 `release` workflow 中确认所有 qualification、
四目标 package 和 `publish` job 成功，并在 GitHub Release 页面核对六个 assets。仓库设置建议同时使用：

- ruleset 限制 `v*` tag 只能由 release maintainer 创建；
- `release` environment 限制可批准/运行发布的 maintainer；
- 启用 GitHub immutable releases，使已发布 tag 和 assets 不能被修改或删除；
- Actions 默认 token 权限保持 read-only，由 workflow 仅给 `publish` job 提升 `contents: write`。

## 安装与校验

下载与宿主匹配的文件和 `SHA256SUMS`。例如 Linux x64：

```sh
curl -fLO https://github.com/elliothux/open-compute/releases/download/v0.1.0/ocd-v0.1.0-linux-x64
curl -fLO https://github.com/elliothux/open-compute/releases/download/v0.1.0/SHA256SUMS
grep '  ocd-v0.1.0-linux-x64$' SHA256SUMS | sha256sum --check
sudo install -m 0755 ocd-v0.1.0-linux-x64 /opt/open-compute/ocd
/opt/open-compute/ocd --version
```

macOS 使用 `shasum -a 256 -c` 校验筛选后的对应行。校验后仍应按
[单二进制分发与部署](single-binary.md)完成配置、`config check` 和首次启动。

## 失败、重跑与修复版本

- qualification 或 package 暴露源码/产物缺陷：修复源码并走新的 patch version PR；不要移动已经推送的 tag。
- runner、网络或 GitHub 服务的瞬时失败：输入未变化时可以对同一 tag rerun failed jobs；不得借重跑替换
  tag、源码或任何 package 输入。
- Draft 已创建但上传/回读失败：Draft 保持非公开。确认失败证据后可删除该 Draft，再对同一、未移动的
  tag rerun failed jobs；不能覆盖已存在的 asset。
- release 已公开：视为不可变。发现缺陷时发布新的 patch 版本，例如 `v0.1.1`；不要替换二进制、移动
  tag 或删除旧版本来伪装相同版本。
- 某个平台没有成功产物：整个版本不发布，不能先公开其余三个平台。

workflow 配置存在只说明流程已定义；只有某个 tag 的 workflow 实际成功且 GitHub Release 已公开，
才能声称该版本完成跨平台发行资格。长时 soak、签名、公证或其他尚未接入 workflow 的资格继续按
active acceptance 文档记录，不能由 tag 发布成功替代。
