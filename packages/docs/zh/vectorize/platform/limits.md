# 限制

Worker API 上限在已声明处与 Cloudflare Vectorize 对齐：维度 32–1536（float32），向量 id 与 namespace ≤ 64 UTF-8 字节，metadata ≤ 10 KiB/向量，每索引 ≤ 10 个 metadata index，mutation 批次 ≤ 1000，`topK` ≤ 100（返回 values 或全部 metadata 时 ≤ 50）。

本机资源配额由 operator 管理，并在创建索引时冻结。内嵌默认约为 **每索引 10 万向量**（schema 上限低于 Cloudflare 托管的百万级规模），另有逻辑字节配额与有界 CPU 并发。运行中数值：

```sh
ocd capabilities --json
```

这些不是 Cloudflare 套餐配额。近似 ANN 容量（托管千万级/索引一类）不提供。
