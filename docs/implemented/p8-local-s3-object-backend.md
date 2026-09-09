# P8：Local／S3 对象后端

状态：**verified / Implementation GO（2026-09-05）**。

## 用户结果

- `ocd` 启动时从 tagged `[storage]` 配置中互斥选择 Local 或 S3；不双写、不 fallback、不在线切换。
- Artifact、R2、Cache body、snapshot、KV/D1 backup 和 AI Search source 共用一个 backend-neutral `ObjectBackend`。
- Local 直接使用安全文件操作，不启动 HTTP/S3 server 或 rclone；S3 adapter 继续使用 AWS SDK／SigV4。
- Domain store 仍拥有 key、metadata、完整性和生命周期；backend 只拥有对象操作、条件和错误映射。
- Local 使用 fd-relative no-follow、regular-file／owner／mode／hardlink 检查、原子 envelope、fsync、有界扫描和 crash recovery。
- SSE-C 在 Local 使用 authenticated chunked encryption；明文 key、object path 和 payload 不进入日志、health 或 support bundle。
- SQLite、marker 和 snapshot 固定 object-authority fingerprint；已初始化平台更换 backend 或 authority 时 fail closed。
- 开发配置直接使用 Local，删除 rclone 编排；S3 contract Gate 仍使用测试自有 SigV4 fixture。

不支持 Local↔S3 migration、mirror、tiering 或运行时切换；需要时另行设计显式 export/import。

## 历史验证

Build、静态检查、Cloudflare R2 compatibility review、coverage 和最终单轮 workspace Gate 成功。两次 Gate 均为
49 targets／1,129 cases；coverage 为 109,286 / 121,412 Rust lines（90.0125%）。正式输入为 workerd
`v1.20260830.1`。未执行发布或 Cloudflare 账号 mutation。
