# Changelog

不要在本页编造日期或版本故事。以这台机器上的发行身份为准。

```sh
ocd capabilities --json
```

读 `release`：`platform_version`、`git_revision`、`workerd_version`、`workerd_lock_sha256`、schema 版本。仓库 git tag（若有）指向源码；**运行中的契约是 `capabilities.release`，不是本文件的散文。**

## 与 Cloudflare 相同

导航位置对应 [Cloudflare Workers changelog](https://developers.cloudflare.com/workers/platform/changelog/)。那是他们的托管发布说明，不是这份二进制的。

## 故意不同

open-compute 不维护一份手写日期列表。workerd / types pin 的变更是一次依赖升级，会反映在 lock 的 `effective_compatibility_date` 和 `workerd_version` 上。当前 lock 日期是 `2026-08-30`；若 JSON 里的值不同，信 JSON。
