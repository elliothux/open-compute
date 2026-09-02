# 常用命令

以 `ocd --help` 和当前二进制为准。`--config` 是全局选项，必须是绝对路径；不从 cwd 或 `$HOME` 搜索。下面不编造未实现的 flag。

不需要配置即可运行：`--help`、`--version`、`docs`、`licenses`、`capabilities`、`config init`、`worker bundle`。其余子命令都要 `--config`。

## `docs`

列出或打印打进二进制的运维手册。仓库目录怎么改都不改变手册名。

```sh
ocd docs
ocd docs install-and-first-start
```

手册名（无 `.md`）：`backup-and-retention`、`collect-support-bundle`、`disk-pressure`、`fresh-host-restore`、`install-and-first-start`、`master-key-loss-and-recovery`、`s3-outage`、`scheduler-recovery`、`sqlite-corruption`、`current-release-recovery`、`workerd-crash-loop`。

站点页面是给运维读的正文；`ocd docs` 输出的是内嵌 runbook。两者命令应一致。若 runbook 示例写成 `platform.toml`，仍用你的绝对 `--config` 路径。

## `licenses`

打印打进本可执行文件的许可证（Open Compute 与内嵌 Cloudflare workerd）。

```sh
ocd licenses
```

## `capabilities`

打印版本化的产品与发行契约。`--json` 输出 `schema_version`、`release`、`runtime`、`products`、`limits`。读法见[兼容性](/zh/platform/compatibility)。

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

## `config init` / `config check`

```sh
ocd config init --data-dir /var/lib/open-compute
ocd --config /etc/open-compute/config.toml config check
ocd --config /etc/open-compute/config.toml config check --json
```

`init`：`--data-dir` 绝对路径；完整 starter TOML 写到 stdout；不建文件、不写密钥。JSON check 成功形如 `{"schema_version":1,"command":"config_check","result":"ok"}`，人读输出是 `CONFIG_OK`。

## `run`

启动平台进程。首次运行在拿到锁之后生成身份、数据库和 master key，并物化内嵌 runtime。

```sh
ocd --config /etc/open-compute/config.toml run
```

## `doctor`

默认只读。`--full` 授权 S3 canary 和临时 workerd 编译/启动/停止。`--json` 输出版本化报告。详见 [健康检查](/zh/ocd/health)。

```sh
ocd --config /etc/open-compute/config.toml doctor --json
ocd --config /etc/open-compute/config.toml doctor --full --json
```

## `backup`

离线整机快照。子命令：

| 命令 | 作用 |
| --- | --- |
| `backup create --name <label>` | 创建并完整校验一份 committed snapshot |
| `backup list` | 列出本平台已认证的 committed snapshot |
| `backup inspect --snapshot <uuid> [--verify]` | 检查一份；`--verify` 校验每个对象 |
| `backup delete --snapshot <uuid>` | 删除这一份的自有对象；manifest 最后删 |
| `backup retention-plan --keep-last <n> [--max-age-seconds] [--keep-label]` | 只出计划，不删 |
| `backup cleanup-incomplete` | 清超过 grace 的不完整上传 |
| `backup restore --snapshot <uuid>` | 恢复到**空的**新 data-dir |
| `backup cleanup-restore --staging <uuid>` | 按失败 receipt 精确清 staging |
| `backup attest-restore-smoke --snapshot <uuid> --passed` | 记录产品 smoke 已通过；不能替代实际执行过的 smoke |

均需 `--config`；均可 `--json`。流程见 [备份与保留](/zh/ocd/backup) 和 [故障手册](/zh/ocd/incidents/)。

## 事故时才会用到的命令

```sh
ocd --config /etc/open-compute/config.toml support-bundle --output /var/tmp/open-compute-support-20260826.tar --json
ocd --config /etc/open-compute/config.toml scheduler recover-corrupt --backup-name scheduler-corrupt-20260826
```

`support-bundle`：`--output` 必须是绝对路径、目标不存在、不是符号链接。`scheduler recover-corrupt` 仅用于 control 可验证且不含 Queue/Cron/Workflow 状态的 alarm-only 目录。完整停止条件见 [故障手册](/zh/ocd/incidents/)。

`ocd worker bundle` 是离线开发者工具，从 stdin 读版本化 build JSON、向 stdout 写 bundle。运维装机用不到。Worker 编程面见[开始](/zh/get-started)与[兼容性](/zh/platform/compatibility)。
