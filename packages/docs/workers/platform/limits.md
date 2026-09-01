# Limits

数值上限以该节点上 `ocd capabilities --json` 的 `limits` 为准。

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。Cloudflare 托管套餐数字见 [Workers limits](https://developers.cloudflare.com/workers/platform/limits/)，不能用来推断本进程已执行那些请求级配额。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 编程模型为 isolate，不是无限主机进程 | 是 | 是 |
| 产品耐久限额（KV 键大小、D1 语句、R2 对象、cache 对象大小等） | 套餐 / 产品配额 | 由平台配置冻结，并出现在 `limits` 中 |
| 请求级 CPU / subrequest / simultaneous-connection 配额 | 是 | stock OSS workerd `LimitEnforcer` 不执行；本平台不提供这些托管配额。这不放宽 public-address 安全边界、产品专有限额、handle 清理和进程监督 |
| Cache 对象默认 | Cloudflare 产品配额 | 16 MiB / Worker 1 GiB 逻辑 body；见 [Workers Cache](/workers/cache/) |

