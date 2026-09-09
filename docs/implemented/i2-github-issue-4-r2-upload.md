# I2：GitHub Issue #4 R2 上传调度

状态：verified，2026-09-05。范围为
[issue #4](https://github.com/elliothux/open-compute/issues/4) 的上传优化与同尺寸并发回归。

## 结果

- `UploadPart` 删除未消费的五种全文件摘要，只传路径与精确长度；S3 `Content-MD5`、Local 完整性、
  part/complete ETag、SSE-C 和完成后发布语义不变。
- 普通 PUT 的必需摘要进入有界 blocking task。任务持有 staging 文件、字节配额和 CPU permit，
  caller 取消不会提前释放正在使用的资源。
- `oc-r2-http-fields` presence mask 区分用户字段与 provider 默认 `Content-Type`；显式字段丢失、
  标记缺失或损坏继续 fail closed。
- SigV4 fixture 只保留实际使用的 SHA-256、part MD5 和 multipart ETag。
- runtime bridge 的 30 秒 header deadline、multipart 状态机、配额和未知提交恢复合同未改变。

## 验证

同一 241,910,375-byte、29 × 8 MiB parts、4 并发输入通过真实 HTTP、workerd、R2 和 SQLite/D1；
Complete 前对象不可见，完成后读回 SHA-256 为
`7fa67c90cb2e12b9700ae89db64ae62fe35879932c28b08bc05ab027fc41c1c3`。

取消、超时和摘要失败回归确认 staging、quota、CPU permit、pin 和 authority 正确释放。
Coverage 为 49 targets、1,140/1,140 cases、90.044691%；最终非插桩 Gate 为
49 targets、1,140/1,140 cases，单轮通过。

未执行新的 Cloudflare hosted differential；全球复制/placement 仍是 `OC-R2-001` excluded scope。
测试容器已精确清理，原始失败证据保留在 `.temp/issue4/failed/`。
