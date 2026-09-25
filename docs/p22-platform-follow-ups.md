# P22：平台后续能力

状态：**planned**。本页保留尚未进入独立阶段的现有产品待办；开始实施时应冻结每项合同，若范围扩大则拆成独立编号。

## 待办

- [ ] 评估固定 workerd compatibility date 的必要性。
- [ ] 支持从显式 `.env` 输入读取 instance 配置值，不隐式发现 cwd 或用户目录。
- [ ] 支持仍在使用的 `wrangler.toml` 项目输入，并保持 Wrangler 为部署 transport authority。
- [ ] 完成 operator-facing logger 合同，包括输出、级别、敏感信息清理和持久化边界。
- [ ] 评估并实现 lazy Worker startup，同时保持 readiness、首请求失败语义和受监督进程恢复合同。

Q1 coverage 与 P21 macOS 签名已有各自的活动方案，不在这里重复跟踪。
