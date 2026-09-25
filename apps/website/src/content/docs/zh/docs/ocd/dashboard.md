---
title: "Dashboard"
description: "启用 operator Dashboard，使用一次性登录，并在已登记实例之间切换。"
---

Dashboard 随 `ocd` 一起交付，并使用与 Wrangler、官方 SDK 相同的 `/client/v4` API。在需要提供 operator UI 的每个实例配置中启用：

```toml
[dashboard]
enabled = true
```

`ocd setup` 默认会为第一个实例启用 Dashboard，不需要单独运行 Dashboard 进程或安装 package。

## 打开 Dashboard

作用域内只有一个已登记实例时：

```sh
ocd dashboard
```

存在多个实例时，选择负责签发登录的 ready 实例：

```sh
ocd --instance staging dashboard
```

使用 `--no-open` 只打印 URL，或使用 `--json` 获得带版本的机器输出。URL 包含短期、一次性的 login code，不会暴露长期 admin token；交换成功后，浏览器取得有界 operator session。

## 切换实例

账户切换器列出同一所选 daemon 作用域中的已登记实例。在 Dashboard 模型中，一个 account 就是一个 open-compute instance：其 InstanceId、data directory、SQLite authority、object authority、凭据、资源与 workerd 均与其他 entry 隔离。

切换账户只会更改当前选择的 API authority，不会合并实例间的资源、凭据或缓存。实例生命周期操作使用 `ocd instance start|stop|restart <id-or-name>`。

operator surface 只接受受支持的 loopback operator host，并始终要求 daemon admin authority 或有效 browser session。继续阅读[实例](/zh/docs/ocd/instances/)与[健康检查](/zh/docs/ocd/health/)。
