# R3：workerd 原生 compatibility authority 与能力发现

状态：**implemented（2026-09-30）**。R3 把 tenant Worker 的 compatibility date／flags authority 还给正式 pinned workerd，并让 `ocd` 从同一二进制公开可查询的
兼容能力。它替代当前“全平台只接受一个日期和一个手写 flag allowlist”的模型，不实现 open-compute 自有兼容规则。

## 用户结果

- 普通 tenant Worker 和 Dynamic Worker 使用同一合同：每个不可变 Version 保存自己提交的 `compatibility_date` 与
  `compatibility_flags`，只要当前正式 pinned workerd 接受即可部署。
- 升级 workerd 不改写旧 Version；旧 Version 继续按其原日期运行，新 Version 可以使用新二进制支持的日期和 flags。
- `ocd capabilities`、management capability API、SDK 和 Dashboard 从同一份由 pinned workerd 生成的 catalog 展示最大日期、日期规则和
  全部 flag 选项，不再维护第二份列表。
- compatibility date 只选择同一个 workerd binary 内的运行时行为，不选择旧 binary、旧 open-compute schema、旧 artifact 或旧持久化格式。

## 1. Authority 与边界

正式 pin 的 workerd 是唯一 admission authority。tenant metadata 原样进入 workerd 的
`CompatibilityDateValidation::CODE_VERSION`：workerd 校验日期格式、binary maximum、UTC future date、未知／重复／冲突 flag、日期默认值、
enable／disable override 和 experimental 条件。`ocd` 不复制这些规则，不按日期分支业务代码，也不把网页文档或手写常量作为接受依据。

当前 fork 已有全部原生输入：

- `src/workerd/io/maximum-compatibility-date.txt` 是该 binary 的最大日期；
- `src/workerd/io/compatibility-date.capnp` 的 annotations 定义 enable flag、disable flag、default-on date、all-dates、experimental、
  implication 和 Python snapshot release；
- server config 与 Worker Loader 都调用同一 `compileCompatibilityFlags()`，以 `CODE_VERSION` 验证 tenant code。

R3 不给日期增加最小值、枚举或 open-compute allowlist。官方 Workers upload API 省略 `compatibility_date` 时使用 oldest date `2021-11-02`；transport 只在这个有官方来源的 boundary case 物化该值，随后与显式日期走同一 workerd admission，不把它扩成内部默认或旧实现兼容分支。日期不是离散选项；公开的是 workerd 的最大日期以及“同时不得晚于当前 UTC 日期”
规则。`compatibility_flags` 也不经过平台过滤、别名或重写，包括当前 workerd 在 `--experimental` 下接受的 experimental flags；能力输出必须
准确标出 experimental 属性，使 operator 看见真实合同，但不改变它。

平台 system Workers 继续使用正式 runtime lock 中的平台日期和 system flags，因为它们是 `ocd` 自有 runtime assets，不是 tenant
metadata。system 设置不能覆盖、合并或隐式注入 tenant Version。现有平台 `--experimental` 进程 flag 继续服务正式 fork 所需能力；R3 不以
此为理由建立 tenant allowlist。

## 3. workerd 原生 compatibility catalog

在授权 fork 的 `third_party/workerd/` 增加一个只读 JSON introspection 子命令。它直接使用编入 binary 的 maximum-date bytes 和 Cap’n Proto
schema reflection，按 schema field ordinal 输出确定性 catalog；不能用 Rust／TypeScript regex 解析 `.capnp`，也不能从 Cloudflare 网页抓取
选项。

catalog 至少包含：

```json
{
  "schemaVersion": 1,
  "validation": "code_version",
  "binaryMaximumDate": "2026-09-25",
  "futureDatesAllowed": false,
  "features": [
    {
      "enableFlag": "formdata_parser_supports_files",
      "disableFlag": "formdata_parser_converts_files_to_strings",
      "defaultOnDate": "2021-11-03",
      "enabledForAllDates": false,
      "experimental": false,
      "pythonSnapshotRelease": false
    }
  ]
}
```

示例值只说明 shape，不能成为手写 baseline。无对应 annotation 的字段省略 optional JSON member；同时输出 enable 与 disable 名，因为两者都是
合法用户输入。具有 input flag 但没有 default-on date、具有 all-dates、implication、experimental 或 Python snapshot annotation 的字段必须由
reflection 准确投影。排序、UTF-8、重复 key 和 JSON formatting 固定，保证同一 binary 在四个正式目标生成逐字节相同 catalog。

该子命令只描述 binary 已编译能力，不加载配置、不启动 listener、不访问网络、不读取 tenant 数据。它的退出码、stdout-only JSON、空 stderr
和 schema version 进入 workerd fork tests；未知参数和无法反射的 annotation fail closed。

## 4. 构建、pin 与单二进制

`bun run build` 已经验证 formal lock 指定的 GitHub Release workerd archive／binary digest。验证完成后，构建流程对该 exact binary 调用 compatibility
introspection，规范化并保存 catalog 到 `.temp/workerd-build/`，然后才允许 Cargo 消费 runtime assets。不得从 `PATH` 查找另一份 workerd，也
不得在 Cargo build、`ocd` startup 或 capability 请求时下载／探测 runtime。

`packages/runtime/workerd.lock.json` 随协调 pin 增加 catalog schema 与 SHA-256，并把当前只表达单一 tenant 日期的字段改成精确 ownership：

- platform/system Worker 的固定 compatibility date／flags；
- binary maximum compatibility date；
- compatibility catalog schema／digest。

构建拒绝 binary 输出、maximum date、catalog digest、runtime lock 或目标平台之间的任何不一致。catalog bytes 作为生成 runtime asset 保持
untracked，并与其它 runtime assets 一起嵌入唯一 `ocd` executable；release identity 与 support bundle 只公开摘要和非敏感 catalog，不保留
另一个 sidecar。

## 5. `ocd`、API、SDK 与 Dashboard 输出

复用现有 `ocd capabilities`，不新增功能重叠的 `ocd compatibility` 命令。versioned JSON 在 `runtime.compatibility` 返回嵌入 catalog、
catalog SHA-256、正式 workerd identity 与 system Worker date／flags；human output 至少显示 binary maximum、`CODE_VERSION`、feature／input-flag
数量及 experimental 状态，并提示用 `--json` 查看完整选项。

`/open-compute/capabilities` 使用同一 core type 和嵌入 bytes，删除当前手写的单值 `compatibility_date.minimum/maximum` 与
`compatibility_flags`。`@open-compute/sdk` 直接生成新结构，Dashboard 创建 Worker 时展示 date rule、binary maximum 和从 catalog 得到的 flags；
最终提交仍由 workerd validation 决定，UI catalog 不能变成第三套 admission authority。

建议的 machine shape 为：

```json
{
  "runtime": {
    "compatibility": {
      "validation": "workerd_code_version",
      "binary_maximum_date": "2026-09-25",
      "future_dates_allowed": false,
      "experimental_enabled": true,
      "features": [],
      "catalog_sha256": "..."
    },
    "system_workers": {
      "compatibility_date": "2026-09-08",
      "compatibility_flags": ["experimental", "service_binding_extra_handlers"]
    }
  }
}
```

最终字段命名在实现时由一个 Rust serde type 同时拥有 CLI 与 API projection；不在 SDK、Dashboard 或 TypeScript runtime 重声明等价结构。
capability response 是发现信息，不是接受承诺：实际 candidate 仍必须经过同一 pinned workerd，catalog 与 binary 不一致时平台启动和部署均
fail closed。

## 9. 实施证据（2026-09-30）

- 正式 pin `e3bdb07f52affc6a618f02ed2b731a581b0b2f69` 基于 upstream `cb26acc62e64f64487a78e3f73d0a08e9926c690`；随后开发 checkout 与 fork `main` 已同步到 upstream `d99bc6b777e35d72d71c2f1fe2fd1db53284528a` 的 `6c3222ee561b42747bc80753f808d89dd212cd4a`，完整保留 W1/W2/W3/I102。按 pin 升级规则，submodule 前进不自动改写正式 runtime pin。
- [四平台正式构建 run 36668461686](https://github.com/elliothux/workerd/actions/runs/36668461686) 全部通过；四个 binary 的 catalog 逐字节一致，SHA-256 为 `a6a00dc87f3e246dbc29ee32a5ab06db2867ad165798b3f71b131cfc5c1281b4`，binary maximum date 为 `2026-10-07`。
- fork 的 compatibility-date、standalone limits、Rust CLI、Tokio/KJ link 与三目标 dependency-graph 检查通过；根仓库 `bun run build` 对四个平台 binary/archive、catalog、source identity 与 runtime lock 完成校验。
- ordinary Version 与 Dynamic Worker 的日期、enable/disable/experimental flags 及拒绝矩阵由正式 pinned workerd 实际执行；coverage 为 **90.08%**，source freeze 后最终 workspace Gate 54/54 通过。
- 后续 submodule／fork 同步只前进开发源码，不改正式 pin、四平台 binary、根仓库生产源码或验收结论；同步后的 reference/source identity 静态检查通过，未重复运行 workspace Gate。
