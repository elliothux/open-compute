# Compatibility dates

每个部署的 Version 都声明标准 `compatibility_date`。它必须落在 `GET /client/v4/open-compute/capabilities` 广告的范围内；当前示例使用 **`2026-08-30`**。

```sh
curl -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  "$CLOUDFLARE_API_BASE_URL/open-compute/capabilities"
```

读取响应中的 `compatibility_date.minimum` 与 `compatibility_date.maximum`。日期选择 workerd 的可观察行为，语义见 [Cloudflare compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/)。

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 日期选择 workerd 可观察行为 | 是 | 是 |
| 每项目 `compatibility_date` | 是 | 必填，并按不可变 Version 持久化 |
| 支持范围 | Cloudflare runtime | extension capabilities 与正式 runtime pin |
