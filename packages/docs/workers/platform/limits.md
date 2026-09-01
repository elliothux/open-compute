# Limits

数值上限以这台机器上的 `ocd capabilities --json` 的 `limits` 为准。不要把本页或默认 TOML 抄成运行中的配额。

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。Cloudflare 托管套餐数字见 [Workers limits](https://developers.cloudflare.com/workers/platform/limits/)，不能用来推断本进程已经执行了那些 request-scoped quota。

## 与 Cloudflare 相同

产品自己的耐久限额（KV 键大小、D1 语句、R2 对象、cache 对象大小等）由平台配置冻结，并出现在 `limits` 里。Worker 编程模型仍是 isolate，不是无限主机进程。

## 故意不同：OC-WKR-LIMIT-001

pinned stock OSS workerd standalone `LimitEnforcer` 不执行 subrequest/CPU 限制。open-compute 不声称 Cloudflare request-scoped CPU、subrequest 或 simultaneous-connection quota；这不放宽 public-address 安全边界、产品专有限额、handle 清理和进程监督。

不要从其它 `limits` 字段推导这些托管配额已生效。Cache 对象默认 16 MiB / Worker 1 GiB 逻辑 body 见 `OC-CACHE-002`。
