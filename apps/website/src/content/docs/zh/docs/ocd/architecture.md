---
title: "open-compute 的组成与边界"
description: "了解 ocd daemon、实例、配置文件、数据目录、Gateway 与扩展之间的职责边界。"
---

open-compute 将主机级服务与它管理的应用和数据分开。一个 `ocd` 进程可以服务多个相互隔离的实例；每个实例都有自己的配置和数据目录。

```text
主机
└── ocd daemon（所选 user 或 system 作用域中的一个进程）
    ├── ocd.toml                 共享 daemon 设置和实例 registry
    ├── Gateway                  共享 listener 与 TLS 状态
    └── 已登记的实例
        ├── 实例 A
        │   ├── compute.toml     此实例的设置
        │   ├── 数据目录         此实例的平台与产品数据
        │   └── 扩展             为此实例启动的可信本地程序
        └── 实例 B                独立的配置、数据和资源
```

## 各部分的职责

| 部分                 | 用途                                                                                                                                                                                 | 配置或状态位置                                                                                            |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `ocd` daemon         | 运行共享 HTTP listener、管理实例生命周期，并监督每个实例的 runtime。每个所选 user 或 system 作用域运行一个 daemon。                                                                  | `ocd` 可执行文件和所选 OCD 目录（`<OCD_DIR>`）。                                                          |
| `<OCD_DIR>/ocd.toml` | 配置 daemon 级设置，并显式登记归属该 daemon 的实例。每条 registry 记录指向一份 `compute.toml`，并设置 `autostart`。它不是实例的产品配置。                                            | `<OCD_DIR>/ocd.toml`。                                                                                    |
| 实例                 | 一个隔离的 open-compute account：拥有自己的稳定 ID、API 资源、凭据和 runtime，并独立启停。Cloudflare-compatible API 将此身份称为 `account_id`；CLI 和 Dashboard 称为 `instance_id`。 | 由一份已登记的 `compute.toml` 及其配置的数据目录定义。                                                    |
| `compute.toml`       | 配置一个实例，包括数据位置、存储、产品、Dashboard、公网域名和扩展。                                                                                                                  | `ocd.toml` 登记的确切路径；文件可以位于 `<OCD_DIR>` 内或外。                                              |
| 实例数据目录         | 保存该实例的 SQLite authority、身份、本地对象（选择 Local storage 时）及实例专属 runtime 状态。                                                                                      | 由该实例 `compute.toml` 中的 `[data].path` 指定，与配置文件位置相互独立。                                 |
| Gateway              | 提供共享公网入口和 TLS 处理，再将请求路由到声明该域名的实例。daemon 级 Gateway listener 和 TLS 状态由作用域共享；每个实例声明自己的 `base_domain`。                                  | 共享设置和 TLS 持久状态属于 daemon 作用域；域名声明位于对应实例的 `compute.toml`。                        |
| Native extension     | 为一个实例增加由 operator 提供的可信本地功能。它不是单独的 daemon，也不会由系统自动安装。                                                                                            | 在该实例的 `compute.toml` 中通过 `[extensions.<name>]` 声明；扩展源码和可执行文件由 operator 在本地管理。 |

共享且已验证的 runtime package 只缓存一份，位于 `<OCD_DIR>/cache/packages/`；实例专属的 runtime 配置和状态仍归各自实例所有。master key 文件由 `data.master_key_file` 指定，可以位于实例数据目录之外；若在目录外，备份时需单独包含该文件。

## 操作影响范围

- 重启 `ocd` 会重启共享 daemon 及其管理的实例。
- 启动、停止或重启某个实例只影响该实例；其数据和资源与其他实例隔离。
- 从 registry 移除实例不会删除它的 `compute.toml` 或数据目录。
- 实例备份只覆盖该实例，不包含 daemon registry、共享 Gateway 状态或其他实例；共享状态需要单独备份。

继续阅读：[实例管理](/zh/docs/ocd/instances/)、[配置](/zh/docs/ocd/configuration/)、[Gateway](/zh/docs/gateway/)和 [Native extensions](/zh/docs/extension/)。
