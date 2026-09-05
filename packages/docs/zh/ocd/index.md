# 运维概述

`ocd` 是 open-compute 的平台进程：在单节点上监督锁定版本的 `workerd`，对外提供控制面和数据面。

## 发行物

发行物是一个匹配 OS/CPU 的 `ocd` 文件。workerd、系统 Worker、默认配置和运维手册都内嵌其中。运行时不会下载 runtime，也不要在二进制旁另行放置 workerd。

一个 `ocd` 对应一个 data-dir 和一个 workerd 子进程。不要在同一 data-dir 上启动第二个 `ocd`。

## 前置条件

- 一份绝对路径的配置文件
- 一块本机可写的 data-dir（SQLite、身份、master key、运行时解压均在其中）
- 一个选定的 object authority：默认使用本机 Local backend，也可以显式配置 S3 backend；R2、Worker bundle、Static Assets 和大对象共用这一 authority

密钥只走配置里的 `env:` / `file:` 引用，不要写进 unit、镜像或仓库。

## 注意

- 单文件发行物在首次运行时会在 data-dir 里解压并校验内嵌 runtime。
- 备份必须同时覆盖 SQLite 和所选 object authority。Local backend 的同机副本不能替代异机备份；S3 backend 也不是 R2 的时间点恢复。
- `/health/live` 只表示进程存活。`/health/ready` 是准入；不要因 readiness 失败而重启。
- 租户只能使用部署里声明的 binding，不能访问 SQLite 路径、Local object path、S3 凭据或其他租户的资源。

## 本节

- [安装与首次启动](/zh/ocd/get-started)
- [配置](/zh/ocd/configuration)
- [部署](/zh/ocd/deploy)
- [健康检查](/zh/ocd/health)
- [备份与保留](/zh/ocd/backup)
- [常用命令](/zh/ocd/cli)
- [故障手册](/zh/ocd/incidents/)
