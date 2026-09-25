---
title: "Caddy 配置"
description: "检查固定的 Caddy runtime，并通过 operator Caddyfile 安全扩展共享 Gateway。"
---

`ocd` 内嵌并校验受支持的 Caddy build。生产环境不会搜索 `PATH`、使用系统 Caddy 或在启动时下载 Caddy。

## 受管命令

```sh
ocd caddy version
ocd caddy list-modules
ocd caddy fmt ./site.Caddyfile
ocd caddy validate
ocd caddy reload
ocd caddy status
```

这些命令选择当前 user 或显式 `--system` OCD 作用域。它们不接受 `--instance` 或 `--config`，因为一个 Caddy child 服务该作用域的完整 Gateway。`fmt` 只向 stdout 输出格式化内容，不修改源文件。`reload` 通过运行中的 daemon 验证并原子应用完整的生成配置与导入配置。

## Operator 站点

在 `ocd.toml` 中最多添加 16 个 operator-owned 标准 Caddyfile：

```toml
[[gateway.caddy]]
caddy_file = "./sites/internal.Caddyfile"
```

相对路径以 `ocd.toml` 所在目录为基准。重复文件、符号链接、无效 module 和验证失败都会被拒绝。Operator 站点不会变成 Worker route claim，也不会建立第二套 deployment mapping；其 domain、upstream、可选 TCP 80 listener 和 DNS 均由 operator 持有。

生成的平台 route 与导入文件会作为一份配置应用。失败时，`ocd` 保留最后确认的 snapshot。`ocd caddy status` 会报告 runtime pin、child state、配置摘要、DNS 状态、TLS readiness 和最近一次 reload 结果，同时不暴露 Caddy storage 或证书私钥。
