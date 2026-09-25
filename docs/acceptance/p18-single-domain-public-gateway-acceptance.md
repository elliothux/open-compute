# P18：公网 Gateway、DNS 与 TLS 资格

状态：**pending external qualification**。本地实现与离线/fixture 验证已完成，见[实现摘要](../implemented/p18-single-domain-public-gateway.md)。

## 所需环境

- 一台可公开接收 TCP 443、UDP 53 和 TCP 53 的专用主机；仅在 operator 明确选择时使用 HTTP/80。
- 一个专用测试 base domain，可设置 ingress A/AAAA、Worker wildcard CNAME、challenge NS delegation 和 CAA。
- 当前正式发行 executable 中嵌入并校验的 Caddy bytes，不能用系统 Caddy 或开发 binary 替代。

## 验收

1. 运行 `gateway-dns-plan`，按输出配置 DNS；使用 `gateway-challenge-probe` 和 `gateway-dns-verify` 验证公共 resolver、父区委派、CAA 及 UDP/TCP challenge authority。
2. 启动 Gateway，观察 DNS-01 wildcard 证书签发；用 `gateway-tls-probe` 验证 SNI、证书链、私有 upstream marker 和 Worker HTTPS route。
3. 创建、重命名、删除公网 Worker binding，确认 local/public endpoint 独立投影且故障不会删除本机 route。
4. 覆盖 reload 失败回退、Caddy crash/restart、daemon restart、证书续期和 DNS drift；检查无 orphan child、端口和 challenge TXT。
5. 记录最终版本、域名、DNS 输入、证书 issuer/有效期、命令退出状态及清理结果。

外部 mutation 和专用主机使用仍需 operator 明确授权。完成后把关键结果并入实现摘要并删除本文件。
