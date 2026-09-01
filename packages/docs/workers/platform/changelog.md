# Changelog

以该节点上的发行身份为准，不以本页文字为准。

```sh
ocd capabilities --json
```

读取 `release`：`platform_version`、`git_revision`、`workerd_version`、`workerd_lock_sha256`、schema 版本。仓库 git tag（若有）指向源码；**运行中的契约是 `capabilities.release`。**

导航位置对应 [Cloudflare Workers changelog](https://developers.cloudflare.com/workers/platform/changelog/)。那是托管发布说明，不是这份二进制的。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 手写日期列表 | 是 | 不提供 |
| workerd / types pin 变更 | 托管发布 | 一次依赖升级，反映在 lock 的 `effective_compatibility_date` 与 `workerd_version` |
| 当前 lock 日期 | 不适用 | `2026-08-30`；若 JSON 中的值不同，以 JSON 为准 |

