---
title: "配置"
---

`wrangler@4.127.1/config-schema.json` 是项目语法 authority。`ocd wrangler` 不解析项目；config resolution 和类型生成使用项目内 Wrangler。

```json
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "app",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "workers_dev": false,
  "vars": { "LOG_LEVEL": "info" }
}
```

支持标准 `name`、`account_id`、`main`、`compatibility_date`、`compatibility_flags`、`env`、build 字段、`vars`、各产品 binding 数组、Service Bindings、Static Assets、cron triggers、Images、Workers AI、Version Metadata、cache 配置，以及仅供本地使用的 `secrets.required` 声明。通过 Wrangler schema 不代表远端能力已实现；不支持的 server 能力会在 API 或 upload validation 阶段 fail closed。

框架 adapter 保留用户的 `wrangler.jsonc`，并生成标准 `.wrangler/deploy/config.json` redirect，指向生成的 Wrangler 配置。类型生成与部署由项目内 Wrangler 拥有。

参见[绑定](/docs/zh/workers/configuration/bindings/)、[兼容日期](/docs/zh/workers/configuration/compatibility-dates/)、[兼容 flags](/docs/zh/workers/configuration/compatibility-flags/)、[Cron](/docs/zh/workers/configuration/cron-triggers/)、[变量](/docs/zh/workers/configuration/environment-variables/)和[密钥](/docs/zh/workers/configuration/secrets/)。
