# P11 ocd 运维体验：正式 runner 资格验收

日期：2026-09-08。状态：核心实现已完成 review 修订并通过本地最终冻结；本文件只跟踪仍需隔离 CI / 正式 release /
特权 OS service 证据的资格项，不能代替已通过的本地最终 Gate。

实现合同与本地证据见
[P11 ocd 安装、实例与本机运维体验](../implemented/p11-ocd-operator-experience.md)。
本文不重新打开已完成的 Day1 源码路径；若验收暴露新的实现缺口，应在 `docs/` 根目录恢复明确设计，
而不是把实现工作隐藏在本计划中。

## 固定输入

- 正式 GitHub Releases 上已发布、不可变的三个目标资产：`darwin-arm64`、`linux-arm64`、`linux-x64`
  （外加发布流程要求的校验与 `release.json` / `SHA256SUMS`）。
- 匹配资产的 `scripts/install.sh` 与 install receipt schema（与当前实现一致）。
- 隔离 Linux systemd runner 与 macOS launchd runner；允许显式授权的 `sudo` / 服务账户写入，
  不得在开发机上隐式修改真实启动项。
- 普通 workspace Gate / coverage 继续使用 FakeServiceManager 与 fake root；它们不代替本计划。

## 待收集证据

### A. 正式 release 安装冒烟（原 DoD §15.1）

- [ ] 三个正式目标各从公开 Release 下载，经 `SHA256SUMS` 校验后由 `install.sh`（或审阅后等价步骤）
      安装到约定全局 PATH（默认 `/usr/local/bin/ocd`）。
- [ ] 安装后 `ocd --version` 与 release identity 一致；`$PREFIX/share/open-compute/install-receipt.json`
      存在且不含 secret。
- [ ] 安装脚本不创建配置、data-dir、token 或 OS service。

### B. 真实 OS service（原 DoD §15.3 与矩阵特权行）

- [ ] Linux：systemd unit 真实 enable、开机启动、stop、restart、failure 路径与日志路径可读；
      non-root `User=`、文件 ownership、orphan / stale unit 行为符合实现合同。
- [ ] macOS：launchd（Login/LaunchAgents 或约定 scope）真实 load、登录启动、stop、restart、
      KeepAlive 与日志路径可读。
- [ ] 两台隔离 runner 上各至少跑通一次 `start` → `status` ready → `restart` → `stop` → `logs`。

### C. 全新主机 `setup` 到达 readiness（原 DoD §15.4 的真实进程部分）

实现已在 `setup`/`start`/`restart` 后调用 bounded `wait_until_instance_ready`（默认 60s；fake 用 ready stub）。
本项仍需真实 daemon 进程证据：

- [ ] 干净主机（或干净临时 root）上 `ocd setup --yes`（或等价显式 config）生成安全配置、注册 service、
      启动并在约定时间内达到 control-plane `ready`（及 `/health/ready`）。
- [ ] 中途失败（端口占用、权限、已有配置）不留下半配置；成功路径可 `ocd dashboard` 兑换一次性登录。

### D. 双实例真实并行（矩阵特权行）

- [ ] 同一主机两个真实实例使用不同 data-dir / listener / service id，可并行 ready，并可分别 restart/stop。
- [ ] 无 selector 时副作用命令拒绝并打印完整实例 ID；显式 `--instance` / `--config` 指向正确目标。

### E. 与发行流程的交叉

- [ ] 首次发行或后续 patch 的公开 Release 一旦可用，用该不可变输入重跑 A；不要用开发树二进制冒充正式安装证据。
- [ ] 本计划完成后，将实际命令、runner、报告路径与限制一并移入 `docs/implemented/`，并更新验收索引。

## 明确不由本计划替代的已有证据

- FakeServiceManager / unit·plist 渲染 / registry·socket·selector / setup 失败语义 /
  `wait_until_instance_ready`（含 timeout）/ upgrade fixture / update-check / Dashboard login code
  单测与 HTTP 兑换：见归档实现文档；当前源码已通过 90.0047% coverage 与单轮 workspace Gate。
- 自动后台更新、Windows 服务、跨机器 registry：仍是产品非目标，不纳入本资格。

## 授权提醒

网络下载正式 Release、`sudo`、写入 `/etc`、systemd、launchd 或修改真实用户启动项，均需按
[AGENTS.md](../../AGENTS.md) Operating Contract 获得明确授权后再执行。缺少条件时保持本文件为活动验收，
不得用本地 fake Gate 勾选上述复选框。
