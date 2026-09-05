# ocd 命名契约

命名改造已完成，实际变更与验收见[完成记录](ocd-day1-rename-results.md)。

| 对象 | 唯一名称 |
| --- | --- |
| Production executable / CLI / daemon | `ocd` |
| 项目与文档 origin | `https://open-compute.dev` |
| Reverse-DNS 服务前缀 | `dev.open-compute` |
| launchd identity | `dev.open-compute.ocd` |

CLI、配置示例、服务文件、发布物和文档使用同一命名；不保留旧命令或服务名别名。
当前运行与发行命令见[单二进制指南](../references/single-binary.md)和[发布流程](../references/releasing.md)。
