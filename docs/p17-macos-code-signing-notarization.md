# P17：macOS Developer ID 签名与 Apple 公证

状态：Day 1 发行合同与 CI 方案完成；待配置 Apple/GitHub 凭据、实施和真实 tag 验收。

P17 为正式 `darwin-arm64` 单文件 `ocd` 接入 Apple Developer ID 签名与 Notary Service 公证。目标是让从
GitHub Releases 下载的官方 macOS 产物通过 Gatekeeper，同时保持现有单 executable、不可变 release、固定 workerd、
发布前完整资格和最小 CI 权限模型。

## 1. 范围与结论

P17 Day 1 目标：

- 用有效的 `Developer ID Application` identity 签署最终 `ocd` Mach-O；
- 启用 Hardened Runtime 和 Apple secure timestamp，不申请非必要 entitlement；
- 把签名后的精确二进制装入一次性 ZIP，使用 `notarytool` 提交 Apple 公证；
- 只有 `Accepted` 的提交才能进入 release manifest、checksum 和 publish；
- 对签名后的精确公开字节重新执行单二进制验证和 Gatekeeper assessment；
- 由最终二进制的 Developer ID signature、Apple ticket 和内部 signing report 共同记录可审计身份，不记录 credential；
- 把 P12 私钥和 Notary API 私钥限制在受保护的 macOS signing job；
- 保持三个公开 executable、`release.json` 和 `SHA256SUMS` 共五个 asset，不发布临时 ZIP、P12、P8 或
  signing report；
- 保持 production startup 离线，不在启动、安装或升级路径联系 Apple Notary Service。

结论：**签名和公证属于 macOS 正式发行资格，不属于普通 build、PR CI 或运行时能力。** 当前
[`release.yml`](../.github/workflows/release.yml) 应从：

```text
package -> assemble -> publish
```

收敛为：

```text
package darwin candidate
  -> sign-macos (Developer ID -> notarize -> verify -> final report)
  -> assemble signed darwin + unchanged linux artifacts
  -> publish
```

Linux package、coverage、macOS workspace Gate 和 Linux egress qualification 继续并行。签名 job 可以等待三个 package
matrix 完成，但 publish 仍必须等待全部既有资格，不以 Apple `Accepted` 替代产品 Gate。

## 2. Apple 合同与固定工具

实施依据：

- [Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates)；
- [Signing Mac Software with Developer ID](https://developer.apple.com/developer-id/)；
- [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)；
- [Customizing the notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)；
- [TN3147: Migrating to the latest notarization tool](https://developer.apple.com/documentation/technotes/tn3147-migrating-to-the-latest-notarization-tool)；
- tag workflow 固定的 `macos-15` runner、其 Xcode Command Line Tools 中的 `codesign`、`security`、`ditto`、
  `notarytool`、`stapler` 和 `spctl`。

P17 只使用 `notarytool`，不保留已退役的 `altool` 路径。实现必须在 job 开始时记录无 secret 的 macOS build、
Xcode、`codesign` 和 `notarytool` 版本；工具缺失或输出合同无法解析时 fail closed。

`Developer ID Application` certificate 必须由当前 Apple Developer team 创建。CI 需要包含 private key 的 P12；单独
下载的 CER 不能完成签名。P17 不创建 `Developer ID Installer` certificate，因为当前产品不发布 PKG。

## 3. 凭据与 GitHub authority

### 3.1 `apple-signing` Environment

新建独立 GitHub Environment `apple-signing`，与只负责最终 GitHub Release 写入的现有 `release` Environment 分开：

- deployment branch/tag policy 只允许 `v*` tag；
- 建议配置 required reviewer，审批时核对 tag、release commit 和已通过的 main pre-check；
- signing job 只授予 `contents: read` 与 `actions: read`；
- signing job 不获得 `contents: write`，publish job 不获得 Apple credential；
- PR、main push、手动普通 workflow 和 Linux job 都不能读取该 Environment 的 secret；
- 不执行第三方 signing action；所有能影响 candidate/final bytes 或接触 Apple credential 的外部 Actions 在完整 release
  workflow 中固定到 review 过的完整 commit SHA，不能只用可移动的 major tag。

Environment secrets：

| 名称                                   | 内容                                                                    |
| -------------------------------------- | ----------------------------------------------------------------------- |
| `APPLE_SIGNING_CERTIFICATE_P12_BASE64` | Developer ID Application certificate 与 private key 的 P12，base64 编码 |
| `APPLE_SIGNING_CERTIFICATE_PASSWORD`   | P12 的强随机 export password                                            |
| `APPLE_NOTARY_API_KEY_P8_BASE64`       | App Store Connect API private key，base64 编码                          |

Environment variables：

| 名称                               | 内容                                                     |
| ---------------------------------- | -------------------------------------------------------- |
| `APPLE_DEVELOPER_TEAM_ID`          | 预期的 Developer Team ID                                 |
| `APPLE_SIGNING_IDENTITY`           | 精确的 `Developer ID Application: ... (TEAMID)` identity |
| `APPLE_SIGNING_CERTIFICATE_SHA256` | 允许用于本次 release 的 certificate fingerprint          |
| `APPLE_NOTARY_KEY_ID`              | App Store Connect API Key ID                             |
| `APPLE_NOTARY_KEY_KIND`            | 精确为 `team` 或 `individual`                            |
| `APPLE_NOTARY_ISSUER_ID`           | Team API Key 的 Issuer UUID；Individual API Key 留空     |

P17 选择 App Store Connect API Key，不实现 Apple ID + app-specific password 的第二套 CI 路径。Team API Key 按 Apple
合同要求 `APPLE_NOTARY_ISSUER_ID` 非空并传 `--issuer`；Individual API Key 必须不配置 issuer 并省略 `--issuer`。
`APPLE_NOTARY_KEY_KIND` 让两种官方形状显式、可校验，不能靠 issuer 缺失猜测。其他 required value 缺失、空值、混合模式或
格式错误一律在上传前拒绝。这是当前 Apple authentication contract，不是旧 open-compute 兼容分支。

P12/P8 原始文件不提交到 Git、不进入 Actions cache/artifact、不写日志。GitHub secret 只在 signing step 映射到环境；脚本
先设 `umask 077`，再解码到 `$RUNNER_TEMP` 中的精确文件。禁止 `set -x`、shell trace 和把 secret 放入 argv 之外的诊断输出。

### 3.2 临时 Keychain

每个 job 创建一个随机名、随机密码的临时 keychain：

1. `security create-keychain`、限定 unlock timeout 并解锁；
2. 用精确 P12 password 导入 certificate/private key，只授权 `/usr/bin/codesign`；
3. 设置 `apple-tool:,apple:,codesign:` partition list；
4. 枚举 code-signing identities，要求精确 identity 唯一存在；
5. 校验 certificate 类型、Team ID、fingerprint、有效期和 private-key presence；
6. job `trap` 无条件删除临时 keychain、P12、P8 和一次性 ZIP。

不能导入 runner login keychain，不能使用 `codesign --sign -`、ad-hoc fallback 或模糊名称选择。certificate 轮换直接替换
Environment secret/variable；不在 workflow 中同时尝试新旧 certificate。既有 release 的签名身份由其公开 bytes 和
`release.json` 保留，不需要兼容签名路径。

## 4. 最终产物与签名顺序

当前 [`package-release.ts`](../scripts/package-release.ts) 从冻结源码和正式 workerd archive 构建、复制、运行产品身份检查，
并为未签名 candidate 生成 package report。P17 保留这一步作为 candidate provenance，但它不再是公开产物。

`sign-macos` 必须按以下顺序处理：

1. 只下载 `package` job 产生的精确 `darwin-arm64` candidate 与 report；
2. 重新核对 tag、Git revision、workspace version、workerd release、lock digest、candidate size/SHA-256；
3. 使用 `codesign --force --options runtime --timestamp --sign <exact identity>` 签最终 `ocd`；
4. 使用 `codesign --verify --strict --verbose=4` 验证结构与签名；
5. 从签名结果读取 Team ID、Authority、CDHash、runtime flag 和 timestamp，逐项与固定配置核对；
6. 用 `/usr/bin/ditto -c -k --keepParent` 创建只包含该二进制的一次性 ZIP；
7. 用 `notarytool submit` 上传，立即保留 submission ID，并在 bounded deadline 内等待最终状态；
8. 只接受 `Accepted`，随后用 `spctl --assess --type execute` 验证 Gatekeeper；
9. 对签名后的二进制运行 `OPEN_COMPUTE_TEST_OCD=... ./test/gate.py single-binary --jobs 1`；
10. 生成只描述签名后最终 bytes 的 macOS package report，上传为独立 final artifact；
11. 删除临时 ZIP、credential files 与 keychain。

禁止 `codesign --deep`、`notarytool --force`、关闭 timestamp、放宽 Hardened Runtime、加入
`com.apple.security.get-task-allow`、`disable-library-validation` 或 unsigned executable memory 等 entitlement 来绕过失败。
`ocd` 当前不需要 entitlement；若 Hardened Runtime 暴露真实不兼容，先修复代码或把精确 capability 与风险写入 P17 review，
不能加一组宽泛 entitlement。

签名是公开二进制的最后一次 byte mutation。签名后不能重新 strip、改 mode 以外的内容、嵌入 metadata 或改写 Mach-O。
`release.json`、`SHA256SUMS` 和 final package report 全部基于签名后的 bytes 生成。

## 5. 公证、ticket 与单文件发行

Apple Notary Service 不接受裸 executable 作为提交容器，因此 CI 创建一次性 ZIP。ZIP 只承载签名后的单个 `ocd`，公证完成
后删除，不进入公开 release。

当前公开资产仍是无 bundle 的裸 Mach-O。`stapler` 支持 UDIF、code-signed executable bundle 和 signed flat installer
package，不能把 ticket staple 到当前裸 executable 或一次性 ZIP。因此 P17 的 Gatekeeper 验证使用 Apple 在线 ticket；
默认 GitHub 安装流程本来就需要联网下载 release。`ocd` 启动后仍不联系 Apple，production offline contract 不变。

完全离线的首次 Gatekeeper admission 不属于 P17 当前范围。若将来要求在从未访问 Apple 的隔离 Mac 上完成首次安装，应另行
评估发布 stapled PKG/DMG；这会改变“五个 asset、无 installer”的正式发行合同，不能在 P17 中偷偷增加第六个 artifact。

公证脚本必须：

- 在首次提交响应中持久化 submission ID 到 `.temp/release-signing/`；
- 对 `In Progress` 做有界查询，不做无限轮询或自动新建重复 submission；
- 对 `Invalid` 下载 log、净化本地绝对路径后作为失败 evidence 上传；
- timeout、Apple unavailable、认证失败、无法取得 log 或状态未知时使 job 失败并阻止 assemble；
- 不把一次历史 `Accepted` 当作另一个 SHA-256、tag 或 revision 的资格。

相同 tag/revision 的 runner 或 Apple 瞬时失败遵循现有 release rerun policy。自动 retry 不允许再次提交；maintainer 必须先用
已记录 submission ID 确认原提交状态，再决定继续查询或重新执行 signing job。

## 6. 内嵌 workerd 边界

P17 第一阶段只签最终公开 `ocd`，不在 release CI 中临时重签 `share/workerd/darwin-arm64/workerd`：

- workerd 是正式 lock 固定、gzip 内嵌的数据；CI 重签会改变 binary digest、archive digest、Git LFS bytes 和 build identity；
- 外层 Developer ID signature 覆盖 `ocd` Mach-O 中的内嵌 archive bytes；
- 首次物化继续按正式 lock 校验 archive/binary digest，物化文件不是独立下载的公开 release asset；
- 当前 workerd 的 ad-hoc/linker signature 不能在文档或 manifest 中宣称为 Developer ID signature。

若 Apple 对精确 `ocd` submission 拒绝内嵌 executable，或真实 Gatekeeper/launchd 验收证明物化 workerd 需要 Developer ID，必须
停止发布并执行一次协调的 workerd fork release/pin 更新：在 fork 构建阶段签 workerd，更新三个正式目标输入、lock、LFS、
摘要与跨平台 Gate。不得在 P17 job 中重签后跳过 formal pin。

## 7. Release manifest 与流水线所有权

公开 `release.json` 的现有 schema 已能用 target、size 和 SHA-256 唯一指向签名后的 macOS bytes，无需加入可从 Mach-O
signature 读取的重复字段，也无需为 P17 修改 public schema。Apple signing metadata 只进入新的 current final package
report；旧 candidate report 不被 `assemble` 接受，不保留 schema 双读或 fallback。macOS final report 增加：

```json
{
  "target": "darwin-arm64",
  "sha256": "...",
  "bytes": 123,
  "appleSigning": {
    "teamId": "...",
    "certificateSha256": "...",
    "cdhash": "...",
    "hardenedRuntime": true,
    "secureTimestamp": true,
    "notarizationStatus": "Accepted",
    "notarizationSubmissionId": "..."
  }
}
```

Linux final report 不伪造空 Apple 字段；report schema 用按 target 判定的 tagged shape，`darwin-arm64` 缺少完整 signing
record 时 assemble 拒绝。Notary API Key ID、Issuer ID、Apple ID、certificate subject email、绝对路径和 keychain 信息不进入
final report 或 public manifest。公开 `release.json` 继续只记录最终文件身份；Notary submission evidence 留在受限 CI artifact，
Developer ID identity 可从 exact binary 验证。

工作流 artifact ownership：

| Artifact                        | Producer     | Consumer             | 是否公开                 |
| ------------------------------- | ------------ | -------------------- | ------------------------ |
| macOS unsigned candidate/report | `package`    | `sign-macos`         | 否                       |
| macOS signed final/report       | `sign-macos` | `assemble`           | 是，只有 executable 公开 |
| Linux final/report              | `package`    | `assemble`           | 是，只有 executable 公开 |
| signing/notary evidence         | `sign-macos` | maintainer diagnosis | 否                       |
| `release.json` / `SHA256SUMS`   | `assemble`   | `publish`            | 是                       |

`assemble` 不再用会同时匹配 candidate/final 的宽泛 artifact glob；它显式下载一个 signed darwin artifact 和两个 Linux final
artifact，并继续拒绝多余、缺失、同名或 digest 不符的输入。`publish` 仍只在五个 public assets 上传后逐字节回读验证，再把
Draft 变为 latest release。

## 8. 验收

### 8.1 无 secret 的本地与 PR 检查

- signing metadata/schema parser、Team/identity/fingerprint 检查和 notary status parser 的 focused tests；
- fake command fixture 覆盖 Accepted、Invalid、timeout、malformed output、错误 certificate、缺少 runtime/timestamp；
- `assemble-release` 拒绝 unsigned macOS report、candidate/final 混用、错误 CDHash、错误 digest 和多余 artifact；
- workflow policy 检查 signing job 仅由 tag release 触发、只读权限、受 `apple-signing` Environment 保护；
- credential 名称和值不出现在 report、manifest、logs、cache key 或 uploaded public asset；
- `bun run test:js:ci`、workflow/static checks 与既有 release script tests 通过。

### 8.2 真实 Apple qualification

首个 P17 tag 必须在 `macos-15` runner 上取得并保留以下证据：

- 精确 certificate Team ID/fingerprint，Hardened Runtime 与 secure timestamp 验证通过；
- Notary submission 对精确 ZIP 返回 `Accepted`；
- `codesign --verify --strict` 与 `spctl --assess --type execute` 成功；
- 签名后的单二进制 Gate 通过，包括离线物化固定 workerd、启动、ready、restart、stop 与损坏拒绝；
- 文档解析 self-spawn、launchd setup/service lifecycle 和普通 CLI smoke 在签名产物上通过；
- 临时副本翻转一个受签名覆盖的 byte 后 `codesign`/`spctl` 拒绝，原 final artifact 未被修改；
- Draft 上传后回读 bytes、`release.json` 和 `SHA256SUMS` 与 signing job final bytes 完全一致；
- job 结束后没有残留 keychain、P12、P8、notary ZIP、workerd/parser process 或 listener；
- GitHub Actions artifact/log 搜索不含 P12/P8 内容、password 或其他 credential。

验收必须使用一个未公开的新 patch tag。不得移动或覆盖已经公开的 tag/release 来试验签名。Apple `Accepted` 只证明提交给
Notary Service 的具体 bytes；只有其余 release qualification 和最终 Draft 回读也通过，才能宣称该版本完成 macOS 签名发行。

## 9. 文档与发布迁移

实现时同步：

- 更新 [`references/releasing.md`](references/releasing.md) 的 workflow、credential、失败与 release 操作说明；
- 更新 [`references/single-binary.md`](references/single-binary.md) 的 macOS Gatekeeper、在线 ticket 与离线首次安装边界；
- 从新版本 release notes 删除“Code signing and macOS notarization are not included”，改为记录实际 Team/signing/notary 资格；
- 保留 0.1.2–0.1.4 release notes 的历史未签名事实，不回写旧版本；
- 更新内部 package report、assemble tests、下载校验与文档示例；公开 release manifest schema 保持不变；
- P17 完成后删除凭据配置过程和实施步骤，只把当前发行合同与实际证据精简移入 `docs/implemented/`。

## 10. 非目标

- Mac App Store 分发、sandbox profile、provisioning profile 或 App Store receipt；
- PKG、DMG、`.app` bundle、stapled offline installer 或自动更新 framework；
- macOS Intel 官方 artifact；
- 在 PR、main CI、本地普通 build 或 Linux release 中使用 Apple private key；
- 运行时、公网安装脚本或 `ocd upgrade` 访问 Notary API；
- 用 Apple 签名替代现有 SHA-256、workerd formal pin、release immutability 或产品 Gate；
- 为旧的未签名 release 增加下载 fallback、重签副本或移动 tag。
