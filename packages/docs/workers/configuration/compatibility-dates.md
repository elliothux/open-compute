# Compatibility dates

Every deployed Version declares standard `compatibility_date`. It must fall inside the range advertised by `GET /client/v4/open-compute/capabilities`; the current example uses **`2026-08-30`**.

```sh
curl -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  "$CLOUDFLARE_API_BASE_URL/open-compute/capabilities"
```

Read `compatibility_date.minimum` and `compatibility_date.maximum`. The date selects workerd's observable behavior as documented by [Cloudflare compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/).

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Date selects workerd observable behavior | Yes | Yes |
| Per-project `compatibility_date` | Yes | Required and persisted per immutable Version |
| Supported range | Cloudflare runtime | Extension capabilities plus the formal runtime pin |
