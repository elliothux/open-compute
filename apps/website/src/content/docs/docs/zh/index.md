---
title: "open-compute 文档"
description: "安装、开发和运维单机 Cloudflare Workers 兼容平台 open-compute。"
---

open-compute 在你控制的一台机器上运行受支持的 Cloudflare Workers 应用。一个 `ocd` 二进制文件拥有控制面、数据面、SQLite 状态、对象存储和受监督的固定 workerd runtime。

## 从这里开始

| 我想……                                | 前往                              |
| ------------------------------------- | --------------------------------- |
| 安装 open-compute 并部署第一个 Worker | [快速开始](/docs/zh/get-started/) |
| 开发、部署、调试和回滚应用            | [开发应用](/docs/zh/develop/)     |
| 配置和运行 open-compute 主机          | [运行与运维](/docs/zh/operate/)   |
| 查询 `ocd` 命令                       | [CLI](/docs/zh/cli/)              |
| 查看支持的 binding 和平台产品         | [产品](/docs/zh/products/)        |
| 查询兼容性、限制和 API 合同           | [参考](/docs/zh/reference/)       |

## 先了解边界

open-compute 面向自托管单机部署。它不是 Cloudflare 全球边缘，不提供多区域复制、Anycast、托管账单或 fleet 管理。受支持 API 保持 Cloudflare 编程模型，单机拓扑差异会明确记录。

贡献者从[项目架构与开发](/docs/zh/project/)开始。
