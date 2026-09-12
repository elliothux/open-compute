---
title: "Secrets"
---

使用项目内精确固定版本的 Wrangler 管理 Worker secret。secret value 由 Wrangler 从 stdin 读取，不能出现在 `wrangler.jsonc`、package script、命令参数、target record 或日志中。

```sh
ocd wrangler --target staging secret put API_TOKEN --env staging
ocd wrangler --target staging secret list --env staging
ocd wrangler --target staging secret delete API_TOKEN --env staging
ocd wrangler --target staging secret bulk ./secrets.json --env staging
```

请选择 deployer target。target 的 deployer token 只授权管理请求，不会暴露给 Worker。`ocd` 从 owner-only 的外部 token file 读取该凭据，并且只把它放入短命 Wrangler child environment。

secret mutation 遵守 immutable Version 模型：open-compute 加密 value，并在官方语义要求时创建新 Version 和 100% Deployment。list/get response 只暴露 name 与 type，永不返回 plaintext。rollback 只改变 active Version pointer，因此恢复该 Version 的 secret binding，不改写它。

不提供 Cloudflare Secrets Store 和 Dashboard secret 管理。target 设置、CI 处理与失败恢复见[开发应用](/docs/zh/develop/)。
