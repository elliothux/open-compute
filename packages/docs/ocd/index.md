# 运维概述

open-compute 给你一台机器上的 Workers 平台：一个 `ocd` 进程，带着一份已 pin 的 `workerd`，对外提供控制面和数据面。

它兼容的是 Workers 编程模型里已经声明的那部分（Worker、KV、R2、D1、Durable Objects、Queues、Cron、Workflows、Static Assets、Service Binding、Cache、Images）。兼容不是「名字一样就一样」。全球边缘、跨地域复制、多副本高可用、计费和完整 Cloudflare 管理面都不在范围内。具体开了什么、故意做成什么样，以这台机器上的 `ocd capabilities --json` 为准。

## 你拿到的东西

发行物只有一个文件：匹配 OS/CPU 的 `ocd`。workerd、系统 Worker、默认配置和运维手册都打在里面。运行时不会再下载 runtime，也不要在旁边再放一份 workerd。

进程边界是固定的：一个 `ocd` 管一个 data-dir，再管一个 workerd 子进程。不要在同一 data-dir 上起第二个 `ocd`。

## 你还要自己准备

- 一份绝对路径的配置文件
- 一块本机可写的 data-dir（SQLite、身份、master key、运行时解压都在这）
- 一个 S3 兼容存储，用来放 R2、Worker bundle、Static Assets 和大对象

密钥只走配置里的 `env:` / `file:` 引用，不要写进 unit、镜像或仓库。

## 不要假定的事

- 单文件不等于「磁盘上永远只有这一个文件」。首次运行会在 data-dir 里解压并校验内嵌 runtime。
- 备份能覆盖本机 SQLite 权威数据；R2 仍绑在你配置的那个 bucket 上，不是对象存储的 PITR。
- `/health/live` 只表示进程活着。`/health/ready` 是准入，失败时不要拿它当重启依据。
- 租户只能碰到部署里声明过的 binding，拿不到 SQLite 路径、S3 凭据或别人的资源。

## 本节

- [安装与首次启动](/ocd/get-started)
- [配置](/ocd/configuration)
- [部署](/ocd/deploy)
- [健康检查](/ocd/health)
- [备份与保留](/ocd/backup)
- [常用命令](/ocd/cli)
- [故障手册](/ocd/incidents/)
