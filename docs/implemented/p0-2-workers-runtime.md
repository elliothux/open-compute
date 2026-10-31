# P0.2：Worker 装载与部署

> 状态：已实现；按既有 [P0.2 回归记录](./p0-3-resource-binding-framework.md)归档，本次未重跑验收。

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Bundle／Version 内容不可变；部署、路由切换和回滚通过 authority 指针与 pin 协调，不能改写已发布内容。
- RuntimeSource 从持久化 authority 解析模块、binding、secret 和执行身份；loader cache 只是加速层。
- 普通 Worker 由 system Loader 动态装载，缓存 key 绑定不可变部署身份；冷、热调用都必须经过可信 authority 校验。
- 上传在编译、模块和 binding 验证完成后才进入 ready；失败和响应丢失不能替换当前生效部署。
- `ocd` 是公开入口，生成可信请求身份并剥离外部伪造的内部字段。租户只获得声明的能力。
- 一般出网统一使用 workerd 地址层 `Network(allow = ["public"])`，涵盖 fetch、sockets 和 Node TCP；内部 listener 不可达。
- 请求、事件与存储引用保有对应资源 pin；删除与 GC 不得回收仍在使用的内容。
- 公开 limits／Loader 的未实现部分见 workerd 活动设计；不能从基础 Loader 可用推断完整产品支持。

## 源码入口

- [`crates/workers/src`](../../crates/workers/src)
- [`crates/service/src`](../../crates/service/src)
- [`packages/runtime/src/loader`](../../packages/runtime/src/loader)
- [`packages/runtime/src/gateway`](../../packages/runtime/src/gateway)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p0-3-resource-binding-framework.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
