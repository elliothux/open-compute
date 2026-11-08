---
title: "外部反向代理"
description: "在你自己的 Nginx、Caddy 或 Traefik 后面，用 operator 授权的 exact hostname 提供 Worker，不需要受管 Gateway。"
---

已有的反向代理可以用你自己的域名提供 Worker。代理负责 TLS、证书和转发；`ocd` 负责 hostname 归属哪个 Worker。不需要启用受管 Gateway。

```text
Client --HTTPS--> 你的代理 --HTTP，保留 Host--> ocd external ingress --> Worker
```

## 配置

operator 在 `ocd.toml` 开启一个只服务 tenant 的 listener，并在各实例的 `compute.toml` 授权 exact hostname。两处改动都通过重启 daemon 生效。

```toml title="ocd.toml"
[external_ingress]
bind = "127.0.0.1:8788"
trusted_proxies = ["127.0.0.1/32"]
```

```toml title="compute.toml"
[external_ingress]
hostnames = ["example.com", "app.example.com"]
```

- `bind` 没有默认端口，不能与 `public_bind`、`admin_bind` 或 Gateway HTTPS listener 共用端口。
- `trusted_proxies` 列出允许连接的 TCP peer，写成 IP 网段，例如 `10.0.0.5/32`。IPv4 peer 写成 IPv4；拒绝 `/0`。白名单不是身份认证，listener 应留在 loopback 或受保护的私网。
- `hostnames` 是小写 exact DNS 名（IDN 用 A-label）。`example.com` 不授权 `app.example.com`。一个 hostname 最多属于一个已登记实例（包括已停止的实例），且不能落在任何实例的 `base_domain` 内。
- 有 active binding 的 hostname 不能从 `hostnames` 中删除，先删除 binding。

## 绑定 Worker

有写权限的 deployer 把一个已授权的 origin 绑定到已部署的 Worker。binding 立即生效，不需要重启，也不需要改代理。

```sh
curl --fail-with-body -X PUT \
  "http://127.0.0.1:8787/client/v4/accounts/${ACCOUNT_ID}/open-compute/workers/worker-a/public-origin" \
  -H "Authorization: Bearer ${OCD_TOKEN}" -H 'Content-Type: application/json' \
  --data '{"transport":"external_proxy","origin":"https://app.example.com"}'
```

origin 写成 `http(s)://hostname[:port]`，不带 path、query 或凭据。公网端口属于 origin：`https://app.example.com:8443` 只接受 `Host: app.example.com:8443`。每个 Worker 最多一个公网 origin；用 `{"transport":"managed_gateway","name":"app"}` 调 `PUT` 可切换到 Gateway，`DELETE` 删除它。`GET public-origin` 返回 binding 与 `ingress_available`；`GET endpoints` 只在 listener 服务时列出外部 URL。

在同机检查：

```sh
curl 'http://127.0.0.1:8788/orders?id=1' -H 'Host: app.example.com'
```

Worker 看到 `https://app.example.com/orders?id=1`。包括 `/health/live` 和 `/client/v4/...` 在内的每个 path 都属于 Worker。

## 代理示例

转发原始 `Host` 和完整 path。Nginx（`map` 放在 `http` context）：

```nginx
map $http_upgrade $connection_upgrade { default upgrade; '' close; }
server {
    listen 443 ssl;
    server_name app.example.com;
    ssl_certificate     /etc/nginx/certs/app.fullchain.pem;
    ssl_certificate_key /etc/nginx/certs/app.key;
    location / {
        proxy_pass http://127.0.0.1:8788;
        proxy_http_version 1.1;
        proxy_set_header Host $http_host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_buffering off;
        proxy_request_buffering off;
    }
}
```

Caddy 默认保留 `Host`：

```caddyfile
app.example.com {
    tls /etc/caddy/certs/app.fullchain.pem /etc/caddy/certs/app.key
    reverse_proxy http://127.0.0.1:8788
}
```

## 限制

- `ocd` 看不到客户端 TLS 连接。origin 中的 `https` 是 operator 的声明；代理只能把该 origin 的流量送到 listener。
- `Forwarded`、`X-Forwarded-*`、`X-Real-IP` 和 `CF-Connecting-IP` 会被删除。Worker 看到的 `CF-Connecting-IP` 是代理地址，因此此模式下不能按客户端 IP 限流或审计。
- hostname 只做 exact 匹配：没有通配符、别名或重定向。这不是 Cloudflare Custom Domains API。

| 响应 | 原因                                                    |
| ---- | ------------------------------------------------------- |
| 403  | TCP peer 不在 `trusted_proxies` 中                      |
| 400  | `Host` 缺失、重复或不规范，或 request target 是绝对形式 |
| 404  | hostname 未知或未绑定，或端口与 origin 不一致           |
| 503  | 已授权的实例已停止                                      |
