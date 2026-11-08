---
title: "External reverse proxy"
description: "Serve a Worker at an operator-authorized exact hostname behind your own Nginx, Caddy, or Traefik, with no managed Gateway."
---

An existing reverse proxy can serve a Worker at your own domain. The proxy owns TLS, certificates, and forwarding; `ocd` owns which hostname belongs to which Worker. The managed Gateway is not required.

```text
Client --HTTPS--> your proxy --HTTP, Host kept--> ocd external ingress --> Worker
```

## Configure

The operator enables one tenant-only listener in `ocd.toml` and authorizes exact hostnames in each instance's `compute.toml`. Restart the daemon to apply both.

```toml title="ocd.toml"
[external_ingress]
bind = "127.0.0.1:8788"
trusted_proxies = ["127.0.0.1/32"]
```

```toml title="compute.toml"
[external_ingress]
hostnames = ["example.com", "app.example.com"]
```

- `bind` has no default port. It must not share a port with `public_bind`, `admin_bind`, or the Gateway HTTPS listener.
- `trusted_proxies` lists the TCP peers that may connect, as IP networks such as `10.0.0.5/32`. Write IPv4 peers as IPv4; `/0` is rejected. The allowlist is not authentication, so keep the listener on loopback or a protected private network.
- `hostnames` are lowercase exact DNS names (A-labels for IDN). `example.com` does not authorize `app.example.com`. One hostname belongs to at most one registered instance, stopped instances included, and must not be inside any instance's `base_domain`.
- A hostname with an active binding cannot be removed from `hostnames`. Delete the binding first.

## Bind a Worker

A deployer with write access binds one authorized origin to a deployed Worker. Binding changes take effect immediately: no restart, no proxy change.

```sh
curl --fail-with-body -X PUT \
  "http://127.0.0.1:8787/client/v4/accounts/${ACCOUNT_ID}/open-compute/workers/worker-a/public-origin" \
  -H "Authorization: Bearer ${OCD_TOKEN}" -H 'Content-Type: application/json' \
  --data '{"transport":"external_proxy","origin":"https://app.example.com"}'
```

The origin is `http(s)://hostname[:port]` with no path, query, or credentials. The public port is part of the origin: `https://app.example.com:8443` accepts only `Host: app.example.com:8443`. Each Worker has at most one public origin; `PUT` with `{"transport":"managed_gateway","name":"app"}` switches it to the Gateway, and `DELETE` removes it. `GET public-origin` returns the binding and `ingress_available`; `GET endpoints` lists the external URL only while the listener serves.

Check from the same host:

```sh
curl 'http://127.0.0.1:8788/orders?id=1' -H 'Host: app.example.com'
```

The Worker sees `https://app.example.com/orders?id=1`. Every path, including `/health/live` and `/client/v4/...`, belongs to the Worker.

## Proxy examples

Forward the original `Host` and the full path. Nginx (put `map` in the `http` context):

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

Caddy keeps `Host` by default:

```caddyfile
app.example.com {
    tls /etc/caddy/certs/app.fullchain.pem /etc/caddy/certs/app.key
    reverse_proxy http://127.0.0.1:8788
}
```

## Limits

- `ocd` does not see the client TLS connection. `https` in the origin is the operator's statement; the proxy must serve only that origin's traffic to the listener.
- `Forwarded`, `X-Forwarded-*`, `X-Real-IP`, and `CF-Connecting-IP` are removed. Workers see the proxy address as `CF-Connecting-IP`, so client IP rate limits and audit do not work in this mode.
- Hostnames are exact: no wildcards, aliases, or redirects. This is not the Cloudflare Custom Domains API.

| Response | Cause                                                                     |
| -------- | ------------------------------------------------------------------------- |
| 403      | The TCP peer is not in `trusted_proxies`                                  |
| 400      | Missing, repeated, or non-canonical `Host`, or an absolute request target |
| 404      | Unknown or unbound hostname, or a port that differs from the origin       |
| 503      | The authorized instance is stopped                                        |
