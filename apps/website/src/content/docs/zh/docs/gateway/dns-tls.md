---
title: "DNS 与 TLS"
description: "规划 DNS record、验证委托的 ACME challenge，并探测 Gateway 证书与 Worker route。"
---

Gateway DNS 变更由 operator 持有。`ocd` 输出并验证确定性的计划，但不会修改业务 DNS zone。

## 所需公网可达性

- 将公网 TCP 443 转发到 `[gateway].https_listen`。
- 将公网 UDP 53 与 TCP 53 转发到 `[gateway].challenge_dns_listen`。
- 受管平台不要求 TCP 80；导入的 operator Caddyfile 可以单独使用它。

`ingress_ipv4` 和 `ingress_ipv6` 是 DNS plan 中公布的公网地址，不是 listener bind address。

## 配置与验证

选择需要检查 `[public_gateway].base_domain` 的实例：

```sh
ocd --instance production config gateway-dns-plan
```

按输出在 DNS operator 处创建 ingress A/AAAA、Worker wildcard CNAME、challenge NS delegation 和所需 CAA record，然后执行：

```sh
ocd --instance production config gateway-challenge-probe
ocd --instance production config gateway-dns-verify
ocd caddy validate
ocd caddy reload
ocd caddy status
ocd --instance production config gateway-tls-probe
```

`gateway-challenge-probe` 检查配置的 challenge authority 是否能通过 UDP 与 TCP 直接访问。`gateway-dns-verify` 检查公网递归结果、父区委派、CAA 与 challenge authority。`gateway-tls-probe` 保留 SNI，并以固定 trust root 验证受管证书链、私有 upstream marker 和 Worker HTTPS route。

除 `ocd caddy reload` 会原子应用完整且已验证的 Gateway 配置外，这些 probe 都是只读操作。公网 DNS propagation、防火墙变更和专用主机 qualification 仍由 operator 负责。
