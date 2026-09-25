# I102：Dynamic Worker 扩展产品资格

状态：**pending qualification**。首批 KV、D1、R2、Queue producer 与 ordinary values 已完成，见[实现摘要](../implemented/i102-dynamic-worker-binding-forwarding.md)。

## 剩余矩阵

按公开支持计划逐项验证 Images、Vectorize、AI Search、Artifacts、Durable Object namespace、Workflow binding、Service/native `ServiceStub`、Assets 和 AI。每项必须使用正式 pinned workerd 与真实产品后端，对照直接部署和动态加载的成功/拒绝行为；已装配源码不是支持证据。

覆盖至少包括：binding 改名/重绑、warm/cold cache、父请求结束、来源部署撤销、Worker 删除、并发部署、取消、restart、流背压、伪造根对象、保留模块名和内部 transport 不可见。具名 entrypoint、动态注册类、Alarm、Cron、Queue consumer 与可恢复 Workflow 定义不因根 binding 转发自动进入范围。

发布新的 facade 类型或 capability 前记录冷启动、热请求和流式内存数据；没有实测不承诺零拷贝或无额外开销。完成一项后更新机器可读支持面和实现摘要；全部完成后删除本文件。
