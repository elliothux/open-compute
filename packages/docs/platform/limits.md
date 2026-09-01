# 限制

数字上限来自**运行中的**二进制：`ocd capabilities --json` 的 `limits`。该字段是配置中冻结的产品数值上限，**不含密钥**。

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。

## 运行时未执行的托管配额

锁定版本的开源 `workerd` 独立进程不执行 Cloudflare 托管环境的 request-scoped CPU、subrequest 或 simultaneous-connection 配额。`LimitEnforcer` 的 subrequest 记账是不计数，`getLimitsExceeded()` 始终报告未超限。不要从其它 `limits` 字段推断这些配额已生效。

Cloudflare 托管数字见 [Workers platform limits](https://developers.cloudflare.com/workers/platform/limits/)。行为说明见[行为差异](/platform/deviations)。

## 缓存容量

默认每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值仍以当前 `capabilities.limits` 为准。
