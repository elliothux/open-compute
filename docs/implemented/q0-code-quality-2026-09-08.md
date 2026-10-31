# Q0：代码质量边界收敛

状态：**implemented（2026-09-09）**。

## 最终结果

- Rust 源码按实际领域拆分，production 文件／函数与 test 文件／函数分别执行
  800／300 和 2000／800 行上限；局部绕过 `clippy::too_many_lines` 被拒绝。
- crate dependency direction 由统一 boundary check 覆盖，`search`、`images` 和
  `document-parser` 等低层 crate 不再依赖上层 storage、runtime、workers 或 service。
- 当时的 Dashboard 维护源码改为 lowercase kebab-case；应用级客户端状态迁入 Jotai，
  当时已有日期路径收敛到 date-fns 和 clock/date adapter。后续功能仍受现行 source policy 与 review 约束。
- Prettier、Oxlint、Knip、TypeScript、Dashboard unit/build 与 source policy 使用根级版本和命令，
  不保留 package-local lint／format authority。
- `test/fuzz` 收入根 Cargo workspace，共享 lockfile 与 dependency policy；专项识别的退役 package、tracked bytecode、
  无消费者 API／组件和重复 helper 已删除，不保留旧路径 alias。
- runtime、storage、artifacts、workers 和 service 只保留当前 Day 1 ownership；拆分没有建立第二套协议、
  composition root 或兼容层。

当前规则由根 [`AGENTS.md`](../../AGENTS.md)、[`test/check-source-policy.sh`](../../test/check-source-policy.sh)、
[`test/check-boundaries.sh`](../../test/check-boundaries.sh) 和根 [`package.json`](../../package.json) 持有；
本文件只记录当时的质量专项结果。

## 历史证据

实现提交为 `35f926e49522078f9731ebb1b9ae40a3bd69196b`（`refactor code quality boundaries`）。
后续 0.1.6 冻结候选记录了 repository build、format／static checks、Rust 1.98、dependency boundary、
source policy、至少 90% Rust 行覆盖及一次完整 workspace Gate 全部通过；详见
[0.1.6 release notes](../releases/0.1.6.md)。该结果只证明当时输入，当前工作树仍以现行 Gate 为准。
