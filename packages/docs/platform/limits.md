# 限制

Limits 来自**正在运行的**二进制：`ocd capabilities --json` 的 `limits`。那是配置里冻结的产品数值上限，**不含密钥**。不要把本页、默认 TOML 或 Cloudflare 托管文档抄成这台机器上的配额。

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。

## 不要从 `limits` 推导的托管配额

[`OC-WKR-LIMIT-001`](/platform/deviations#oc-wkr-limit-001)：pinned stock OSS workerd 的 standalone `LimitEnforcer` 不执行 Cloudflare 托管环境的 request-scoped CPU、subrequest 或 simultaneous-connection quota。不能从其它 `limits` 字段推导这些配额已生效。

Cloudflare 托管数字（我们**不声称**）见 [Workers platform limits](https://developers.cloudflare.com/workers/platform/limits/)。

## 缓存容量

[`OC-CACHE-002`](/platform/deviations#oc-cache-002)：运维配置的默认值是每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值仍以当前 `capabilities.limits` 为准。
