# 威胁模型

open-compute 只防三类对手。新增防御代码时，PR 要写明它对付下表的哪一类；写不出来就不加。

| 对手           | 例子                                                                     |
| -------------- | ------------------------------------------------------------------------ |
| 网络客户端     | 能访问对外端口的任何人。防火墙放行业务端口，也就放行了同端口的管理 API。 |
| 上传的代码和包 | 部署者失误，或 CI token 泄露后上传的 bundle、asset 和配置。              |
| 其他 Worker    | 一个 Worker 试图读取另一个 Worker 的数据、secret 或平台内部服务。        |

同机的其他进程、能修改本机文件的人、被替换的 `ocd` 二进制不在模型内：他们已经控制主机。

## 必须保留的防御

| 防御                                                      | 对手                |
| --------------------------------------------------------- | ------------------- |
| 管理 API 鉴权，只在一处实现；平台监听端口不按来源地址放行 | 网络客户端          |
| 请求体大小和并发上限，只在一处配置                        | 网络客户端          |
| 上传 bundle 和 asset 的路径与大小校验                     | 上传内容            |
| secret 不出现在 GET API、日志、错误、argv 和 artifact 中  | 其他 Worker、误操作 |
| Worker 只能看到声明过的 binding，由 workerd 配置保证      | 其他 Worker         |
| secret 加密存储，因为备份和快照可能落到 S3                | 备份泄露            |
| 错误只在对外 HTTP 边界脱敏一次；内部日志保留原始错误      | 网络客户端          |

## 由主机负责

Worker 出站网络不做过滤，由主机防火墙、network namespace、容器或 VM 负责。Unix socket 一律禁止。

## 内部通道

`ocd` 与 workerd 的 HTTP 和 RPC 通道使用 `<OCD_DIR>/run/<InstanceId>/` 下的 Unix socket，目录模式为 0700。
Tenant Worker 的 Network 配置禁止 `unix` 和 `unix-abstract`。内部通道不使用 generation credential。
原生扩展的 fd 4 HostExtension session broker（Cap'n Proto）保留。
