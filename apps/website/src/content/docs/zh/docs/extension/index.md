---
title: "扩展"
description: "Operator 拥有的原生扩展；用户 Worker 通过普通 Service Binding 调用。"
---

扩展是 operator 放在本机、由目标实例在启动时加载一次、并以 Service Binding 目标暴露的代码。用户 Worker 仍用 Wrangler `services` 和 `props` 绑定。没有新的公开 Binding 类型。

扩展不是 tenant 上传的 native code，不是安装器或插件市场，也不是第二个 workerd。Cloudflare 托管 Workers 没有这条原生 Provider 路径；这是 open-compute 在 macOS 与 Linux 上的明确超集。

## 运维侧

每个配置名字指向一个本地目录：

```toml
[extensions.local-files]
path = "./extensions/files"
```

路径相对该实例实际加载的 `compute.toml` 解析。目录必须包含严格的 `extension.toml`，分别命名一个 facade Worker 模块和一个 native Provider 可执行文件。实例启动时以 no-follow 打开这些文件，检查大小和可执行权限，并固定已打开 executable 的 SHA-256 与 FD。错误的 manifest、缺失文件或符号链接都会 fail closed：目标实例不会启动。

## 用户 Worker 看到什么

用户 Worker 调用的是普通 Service Binding。`service` 是扩展名字，不是 Worker ID：

```json
{
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "props": { "directory": "invoices" }
    }
  ]
}
```

`ocd` 把 facade 作为 Service 目标加载，注入不可变的 `ctx.props`，并且**只把**私有 `HOST` 端口交给这个 facade。普通 Worker 拿不到 `HostExtensionFactory`、session identity、Provider 路径、原始 FD 或平台凭据。

## 模型

| 事实     | 合同                                                                                    |
| -------- | --------------------------------------------------------------------------------------- |
| 所有者   | 放置文件并写入配置的 operator                                                           |
| 加载时机 | 仅实例启动；替换文件后重启该实例才使用新实现                                            |
| Runtime  | 该实例受监督的 pinned workerd；Provider 是另外的宿主进程                                |
| 用户 API | Wrangler `services` + `props` / `ctx.props`；仅 facade 拥有 `HOST.call` / `HOST.stream` |
| 名字空间 | 扩展名与 live Worker 的 service 名字共用 namespace                                      |
| 持久化   | 部署固定扩展**名字**、entrypoint 和 canonical props，不固定文件字节                     |

移除扩展不会 fallback 到同名 Worker；调用方看到 unavailable。

## 不提供

Day1 不负责安装、下载、版本管理、热更新、授权、marketplace、进程池、HTTP fallback 或 OS sandbox。`ocd` 不加载动态库，也不执行 tenant native code。兼容性、宿主文件系统权限、升级和回滚由 operator 负责。

继续阅读：

- [实现一个扩展](/zh/docs/extension/tutorial/)：facade、Provider、配置和用户 Worker
- [扩展 API](/zh/docs/extension/api/)：配置、manifest、`HOST`、限制
- [调用如何发生](/zh/docs/extension/architecture/)：`ocd`、workerd、Provider 与用户 Worker
