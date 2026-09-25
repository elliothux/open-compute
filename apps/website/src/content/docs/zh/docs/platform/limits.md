---
title: "限制"
---

operator 可配置的产品容量上限来自**运行中的**二进制：`ocd capabilities --json` 的 `limits`。该字段是配置中冻结的产品数值上限，**不含密钥**。Worker Standard request/isolate 限制在下文单独固定。

```sh
ocd --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。

## Worker Standard 资源限制

正式固定的 open-compute `workerd` fork 原生执行以下 request/isolate 限制：

| 限制                              |                       Standard 数值 |
| --------------------------------- | ----------------------------------: |
| 每次 invocation CPU               | 默认 30,000 ms；可配置至 300,000 ms |
| 每次 invocation subrequests       |    默认 10,000；可配置至 10,000,000 |
| isolate 内存                      |                             128 MiB |
| startup CPU                       |                            1,000 ms |
| 等待响应头的 outbound connections |                每次 invocation 6 个 |

可配置的两个维度使用普通 Wrangler schema：

```jsonc
{
  "limits": {
    "cpu_ms": 60000,
    "subrequests": 20000,
  },
}
```

省略维度使用 Standard 默认值。未知字段、camelCase、零、负数、小数和超上限值会被拒绝。Script Settings `GET` 返回 effective limits；multipart `PATCH` 创建新的 immutable Version，并可单独更新任一维度。

CPU/内存终止使用 Cloudflare `1102`，未捕获的 subrequest 超限异常使用 `1101`，startup-limit 上传验证使用 `10021`。剩余 hosted-only 差异见 [Workers platform limits](https://developers.cloudflare.com/workers/platform/limits/)和[行为差异](/zh/docs/platform/deviations/)。

## 缓存容量

默认每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值仍以当前 `capabilities.limits` 为准。
