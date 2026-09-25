---
title: "Gateway"
description: "通过一个共享、受管的 Caddy Gateway 和显式 DNS/TLS authority 公开已登记实例。"
---

可选 Gateway 为 Worker 增加公网 HTTPS origin，同时保留每个 Worker 的 `.localhost` origin。一个有作用域的 `ocd` daemon 持有共享且固定版本的 Caddy child、challenge DNS provider、listener、配置状态，以及所有已登记实例的 route。

Gateway 配置分为两份 authority：

- `<OCD_DIR>/ocd.toml` 持有共享 listener、ingress address、可信 PROXY peer 和 operator Caddyfile。
- 每个实例的 `compute.toml` 可声明一个独占的 `base_domain`。

```toml title="ocd.toml"
[gateway]
ingress_ipv4 = ["203.0.113.10"]
https_listen = "0.0.0.0:8443"
challenge_dns_listen = "0.0.0.0:8053"
```

```toml title="compute.toml"
[public_gateway]
base_domain = "compute.example.com"
```

名为 `api` 的 Worker 会保留本机 origin，并可获得 `https://api.compute.example.com`。启用 R2 时，它使用固定的 `r2.<base_domain>` namespace。已登记实例不能声明相同、父级或子级 base domain。

## 失败边界

`ocd` 生成完整受管 Caddy 配置，先验证再原子 reload；reload 被拒绝时保留最后确认的配置。Caddy crash recovery 受监督且有界；公网 Gateway 故障不会删除本机 Worker route 或其持久化 claim。

证书与 ACME authority 位于 `<OCD_DIR>/gateway/` 下，属于共享 daemon 状态，不进入实例 snapshot。它必须与 `ocd.toml`、全局 key 及外置 operator Caddyfile 一起备份。

继续阅读 [DNS 与 TLS](/zh/docs/gateway/dns-tls/)和 [Caddy 配置](/zh/docs/gateway/caddy/)。公网 DNS 与证书 qualification 需要主机可从 Internet 接收 TCP 443 和 UDP/TCP 53。
