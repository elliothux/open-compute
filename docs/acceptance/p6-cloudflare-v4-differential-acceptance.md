# P6 Cloudflare v4 与 Wrangler 远端差分验收

状态：blocked by credentials / product permissions，2026-09-03。P6 本地实现与固定客户端验证见
[P6 v4 管理面](../implemented/p6-cloudflare-v4-wrangler-compatibility.md)。

## 阻塞

当前环境没有已确认具备所需产品权限的 `CLOUDFLARE_API_TOKEN` 和 `CLOUDFLARE_ACCOUNT_ID`，
因此未执行会创建 Cloudflare 资源的 hosted runner。缺少凭据不证明兼容，也不能写成 PASS。

## 固定输入

Wrangler `4.127.1`、Cloudflare SDK `7.1.0`、OpenAPI revision
`b8687f42e28fbfcb296a350f7dbf16349ea900af`、workerd `v1.20260830.1` 和 compatibility date
`2026-08-30`；完整摘要见
[`cloudflare-openapi.lock.json`](../../openapi/upstream/cloudflare-openapi.lock.json)。

## 剩余 Gate

- [ ] 获得外部写入授权并通过只读 identity、inventory 和产品权限 preflight。
- [ ] 用固定 Wrangler 比较 account discovery、Worker/Version/Deployment、secret、KV、D1、R2、
  Vectorize、AI Search、Queues 和 Workflows 的 method/path/query/header/content-type/exit code。
- [ ] 用固定 SDK 比较 envelope、分页、raw bytes 和错误形状；比较 Assets multipart metadata 与三段上传。
- [ ] portable runner 在同一 revision 上比较 Workers、KV、D1、R2、DO、Queues 和 Workflows。
- [ ] 所有资源使用唯一前缀，按 ownership journal 精确删除并复查 absent；报告必须脱敏。

任一固定输入变化都需重新固定本地合同。权限不足的产品单独记录，不由其他产品替代。
完成后将结果并入 P6 implemented 文档并删除本文；runner 暴露实现缺口时恢复活动方案。
