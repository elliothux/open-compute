# R1：单 daemon、多 Instance 剩余资格

状态：**pending qualification**。R1 核心实现与本地 Gate 已完成，见[实现摘要](../implemented/r1-single-daemon-instances.md)。

## 剩余输入

- Linux systemd 实机安装、非 root daemon、不同 UID peer credential 拒绝、停止与卸载保留数据。
- macOS GUI/TCC 环境和正式 system service 低端口策略。
- 两个真实 instance 在正式发行布局下并行运行、独立升级/停止、daemon crash recovery 与 orphan cleanup。
- S3-backed 多 instance 的独立 prefix、整机冷恢复、备份恢复身份去重和外置 data-dir 场景。
- 正式 package/全新主机的 setup → readiness → restart 流程。

## 完成条件

每个平台使用最终发行 artifact 和明确的 OCD scope，记录版本、目标、输入、成功退出状态和残留进程/监听检查。失败必须保留诊断，不得用开发目录或 mock workerd 代替发行资格。完成后把关键结果并入实现摘要并删除本文件。
