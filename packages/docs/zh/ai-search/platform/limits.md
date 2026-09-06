# 限制

已声明的 Worker/API 上限包括：AI Search item 与 `toMarkdown` 单文件 ≤ 4 MiB；`toMarkdown` 批次 ≤ 16 个文件 / 32 MiB 输入；单文件 Markdown 输出 ≤ 16 MiB；多 instance 请求 ≤ 10 个 instance；自定义 metadata 字段 ≤ 5；检索结果通常 1–50；context expansion 0–3。

本机配额还约束每 instance 的 items/chunks/vectors、indexing jobs、provider 并发、请求/响应字节与流数量。Parser 子进程默认含 30s 截止与有界地址/CPU/stderr。运行中数值：

```sh
ocd capabilities --json
```

这些不是 Cloudflare AI Search 套餐配额。因不提供完整 Workers AI 推理，对应推理限额不适用。
