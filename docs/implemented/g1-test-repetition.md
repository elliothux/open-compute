# G1：Gate 轮数收敛

状态：**verified（2026-08-29）**。

## 最终结果

- 开发、最终验收和发行默认都只执行所选 Gate 一轮；场景内部需要的 restart、crash 和 recovery 仍完整执行。
- `test/gate_cases.py` 是 case registry，`test/gate.py` 负责 discovery、选择、并发、失败停止和报告。
- Coverage 单独执行一次，不替代最终未插桩 Gate。
- 重复诊断、soak、load 和 fuzz 只有在明确选择时运行，不属于默认 acceptance。
- 当前规则只维护在[测试手册](../references/testing.md)，历史三轮报告不创建重复执行义务。

## 历史依据

同一 macOS arm64 输入上，23-host／63-case 完整追加轮耗时 497.65 秒；只重复 18-host／43-case 的时序集合耗时
413.81 秒，减少 16.85%。每组只有一个样本，不是稳定性能承诺。

当日 coverage 为 56,276 / 62,424 Rust lines（90.15%）。整机重启后，历史完整首轮 690/690、两个时序追加轮
各 43/43 成功。期间 `syspolicyd` 曾在程序入口前崩溃并导致两次失败；重启只恢复了该次执行能力，没有证明系统问题被修复。
