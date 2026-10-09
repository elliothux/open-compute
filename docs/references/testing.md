# Gate 验证节奏

开发、审查、修复和最终验收都只跑所选目标一轮。源码冻结后先完成静态检查与 coverage，
最后执行一次完整 workspace Gate。每个选定 case 的顶层测试进程只执行一次；场景内部定义的
多次重启、崩溃点和恢复步骤继续完整执行。默认失败停止；开发 `--keep-going` 收集全部所选目标的失败，不自动重试；coverage 不代替未插桩的进程验收。

该入口默认验收本地平台 contract/product，不以第三方框架测试定义支持面。P3.4 的 typed target
与 Cargo target 使用同一 discovery、精确 case、环境 allowlist、超时、清理和报告协议。
应用 qualification 和真实 Cloudflare differential 必须显式选择、独立报告，不能暗中加入普通
`--workspace`。
`bun run test:js` 包含 [Vinext 离线输入校验](vinext-input-validation.md)，通过该检查不代表重新完成云端应用资格测试。

## 显式准备输入

P0.1 进程验证需要 `lsof`（Linux 镜像安装同名包；macOS 使用系统版本）。
调度器在编译和 runtime 校验前检查该工具，缺失即失败。

```sh
git lfs pull --include="share/pyodide/**,share/tessdata/**,share/xberg-tesseract-cache/**"
bun install --frozen-lockfile --ignore-scripts
runtime_inputs=$(mktemp -d "$PWD/.temp/runtime-inputs.XXXXXX")
eval "$(bun scripts/prepare-workerd.ts --dest "$runtime_inputs/workerd" --download)"
eval "$(bun scripts/prepare-caddy.ts --dest "$runtime_inputs/caddy" --download)"
bun run build
bun run check:generated
bun run test:js
export RUSTFLAGS='-D warnings'
# 仅 p5-search：本机 OpenAI-compatible embedding fixture
export OPEN_COMPUTE_TEST_EMBEDDING_API_KEY=fixture-secret
export OPEN_COMPUTE_TEST_EMBEDDING_BASE_URL=http://127.0.0.1:8080/v1
```

唯一正式 pin 是 `packages/runtime/workerd.lock.json`。`packages/runtime/dist/` 不提交 Git；
每个 CI job 及干净检出都必须先构建。Rust 校验 archive、binary、生成模块、完整清单及
源码/工具配置摘要，拒绝过期资产。生产启动离线，不依赖 JS 工具链。
准备工具只有显式 `--download` 才读取 lock 指定的 GitHub Release，也可用 `--archive /abs/pinned.gz` 提供同一 pin；
目标目录必须不存在。workerd 与 Caddy 均在 Cargo 前验证摘要和身份，生产启动仍离线。
P22 的 CDP 和 managed fixture 都使用显式准备的完整 `chrome-headless-shell` 安装：

macOS coverage 和最终 workspace CI 显式启用 setup action 的 `browser-fixture` 输入，
从 `test/browser-fixture.lock.json` 固定的官方 Chrome for Testing URL 下载 headless-shell，
先校验 archive SHA-256，再调用同一 preparation 工具。此步骤不调用 Playwright install。
同一 setup action 也支持已固定的 Linux x64 archive；ARM64 宿主 Docker 若缺少 x64 execution support，
会在 native shell 阶段失败，不能以解压或摘要校验代替 Linux x64 runtime 验收。

```sh
bun scripts/prepare-browser.ts --source /abs/installed/chrome-headless-shell --dest /abs/new/browser-fixture
export OPEN_COMPUTE_TEST_BROWSER=/abs/new/browser-fixture/chrome-headless-shell
```

Dashboard 的 Browser Run 绑定验收也复用该显式 headless-shell 安装：先启动已配置 Browser Run 的隔离 ocd，
设置 `OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL` 为其 `/operator/` 地址，再运行
`bun run --filter @open-compute/dashboard test:e2e worker-browser-binding.spec.ts`。
不要同时设置 `OPEN_COMPUTE_DASHBOARD_E2E_BROWSER_CHANNEL`；此用例要求 `OPEN_COMPUTE_TEST_BROWSER`，不执行 Playwright browser install。
Docker 中的 SQLite 测试数据使用 Linux 原生 volume；日志、截图和失败 traces 保存在 `.temp/`，再次运行前保留失败 evidence。

工具不下载输入，复制安装并提取同一 executable 的离线 DevTools、协议与许可证；runtime 验证资源摘要及 native identity。
安装目录缺少 `browser-devtools.json.gz` 即失败，不从旧缓存或网络补齐。Linux fixture 可以在 Docker 中准备、验证，
使用非 root 和固定启动参数保留 Chrome native sandbox；记录容器权限、mount 和网络条件，不使用 `--no-sandbox`。
验收 open-compute 的启动策略、CDP 文件能力与生命周期；Chrome 内部 namespace/seccomp 实现按用户要求不单独验证。
managed 的文件安全验收覆盖 CDP 文件能力限制、平台所属临时目录和网页间接访问；不要求额外 bubblewrap、
Seatbelt 或外层 sandbox，不承诺 Chrome 主进程被攻破后的 OS 文件隔离。

Browser Run sustained qualification 单独运行，不加入日常 workspace Gate：在隔离、本机 loopback 的实例上，
配置至少 2 个 concurrent actions，提供该实例的 API token 和 account ID，执行
`bun test/conformance/applications/browser-soak.ts http://127.0.0.1:8787/client/v4 <account-id> 600000`。
token 仅从 `OPEN_COMPUTE_BROWSER_SOAK_TOKEN` 读取；默认 10 分钟，允许 30 秒至 1 小时。
它通过固定 SDK 并发渲染 HTML、周期性截图、检查已完成调用的 session 收敛；关闭重试，失败即停止并保留日志。
工具拒绝外部 origin，不能用来隐式向 Cloudflare 创建会话。资格检查同时记录本机进程/目录回收与 SQLite 历史预算，
这些检查应由拥有隔离实例生命周期的 fixture 完成，不能把 HTTP 成功数量作为无进程/存储泄漏的证明。

W3 Provider fixture 不属于发行物；它是本仓库 `test-support` feature 下的 Cargo 测试二进制
`crates/service/src/bin/host_extension_test_provider/`（schema 拷贝、Cap'n Proto 绑定与 `OCP2` attach 循环），
随测试目标一起由 cargo 构建，`p3-services-product` 通过 `CARGO_BIN_EXE` 直接定位，无需外部 fixture、环境变量或 Bazel。

## 一个调度入口

调度器仅依赖 Python 3.11+ 标准库；CI 显式选择 Python 3.12。
macOS 本地运行会自动经 `caffeinate -is` 重新执行，避免 suspend/resume 打断真实 workerd、
loopback transport 和有界计时；Linux/CI 行为不变。

```sh
./test/gate.py p0-2
./test/gate.py p0-2 p2-3 --jobs 2
./test/gate.py workflow --list
./test/gate.py p3-contract
./test/gate.py p5-search
# 受保护 CI environment 中的真实 provider 资格测试；不会加入 workspace：
BAILIAN_API_HOST=workspace.cn-beijing.maas.aliyuncs.com \
BAILIAN_API_KEY=... DEEPSEEK_API_KEY=... COHERE_API_KEY=... \
  ./test/gate.py p5-ai-provider-qualification --jobs 1
# 显式 hosted S3 资格测试；使用专用空闲 bucket 和独立前缀，不会加入 workspace：
OPEN_COMPUTE_TEST_R2_S3_MUTATION_ACK=s3-provider-qualification \
  ./test/gate.py s3-provider-qualification --jobs 1
# 外部写入；仅在明确授权、预置账号与 cf OAuth/token credential 后选择：
./test/gate.py p3-cf-diff
# 在同一个冻结报告中合成本地 contract 与 remote qualification：
./test/gate.py p3 p3-cf-diff
./test/gate.py all --jobs 2
./test/gate.py --workspace --final --jobs 2
```

开发验证按受影响目标运行 `./test/gate.py <targets> --keep-going`，一次收集全部目标的失败；
调度仍遵守独占屏障，每个目标只执行一次，失败目标不自动重试，最终退出码保持失败。
批量修复后只重跑失败及直接受影响目标，不能因小修改重新启动 workspace。
完整未插桩 workspace 执行必须显式带 `--final`；coverage.sh 的插桩执行与 `--list` 不受此限制。
`--final` 只接受未插桩单轮，并拒绝与保留报告中相同 source digest、verified inputs 的重复完整验收，
无论上次通过还是失败。生产源字节与覆盖率规则未变时复用已核对的 coverage 证据。

仓库验收使用默认单轮，不设置 `OPEN_COMPUTE_GATE_ROUNDS`。runner 为读取历史报告和显式重复诊断仍识别
`1` 或 `3`，但三轮不再是开发、最终或发行验收要求；只有用户明确要求单独的重复诊断时才可设置为 `3`。
`--list` 输出本轮目标、注册用例及计划进程数，不构建、不执行；workspace 中未列入产品 Gate 的宿主用例会在构建后发现，
计划中的 `null` 不代表跳过。`inventory_verified` 只有核对原生 discovery 后才为真。
workspace 的 service library 仍只构建一个 test binary；调度器对同一原生 inventory 做互斥分区：普通 case
进入现有并行槽，`test/gate_cases.py` 登记的真实进程/lifecycle case 保持独占。两个逻辑目标的 discovery
必须完全相同，分区必须无遗漏、无重叠，且每个 case 仍只执行一次，否则在任何产品 case 启动前失败。
P0.5 的 241 MB 并发上传/回读矩阵保持独占，避免 coverage 下与 service library 争用时截断 HTTP stream。
workspace 顺序先做 CLI、single-binary 和独占的 p5-search，再进入普通并行段；这不增加成功路径工作量，
但会在约五分钟内暴露已知的打包/runtime/search 阻塞，而不是等到长尾独占段。
旧 `test-p*.sh` 递归入口已删除，
仅保留需要授权和清理的 Linux egress wrapper。

| 选择                                                       | 实际目标                                                                                                                                                                                                 |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `p0-1` … `p0-8`、`p0-exit`                                 | 对应 service 集成测试；P0.1 本体只有一轮                                                                                                                                                                 |
| `p1-conformance`、`p1-security`、`p1-crash`、`p1-snapshot` | 对应当前 P1 集成测试；恢复 staging 拒绝矩阵归 `p1-snapshot`                                                                                                                                              |
| `p1-8`                                                     | P0.7 WebSocket/hibernation 与 P1 capability；保留该别名用于当前产品回归，不恢复旧 No-Go 实现                                                                                                             |
| `p2-1`、`p2-2`、`p2-exit`                                  | 对应 scheduler、queue producer、产品链集成测试                                                                                                                                                           |
| `p2-3`                                                     | P0.2 同一 Worker/Queue/Cron 矩阵，不再重复调度                                                                                                                                                           |
| `workflow-runtime`                                         | 当前 Workflow 的真实 runtime、suspension、timeout、parallel 与 native output gate                                                                                                                        |
| `workflow-recovery`                                        | 当前 Workflow 的 snapshot、进程恢复、transport fault 与产品 binding 路径                                                                                                                                 |
| `workflow-product`                                         | 当前 durable execution 与大结果/批次边界                                                                                                                                                                 |
| `workflow`                                                 | 上述三个 Workflow 目标；`p2` 也包含它们                                                                                                                                                                  |
| `p21-python-main`                                          | 单个普通 Python 主链场景：静态官方 cf 上传、真实 daemon/SQLite/SigV4、prepare/restore、KV/D1/R2、重启、rollback 与损坏拒绝；纳入 all、p3 和 workspace，输入缺失即失败                                    |
| `p21-python-frameworks`                                    | 三个普通 daemon 场景：Django、Flask、FastAPI 的真实 cf 静态包、HTTP/body/stream、prepare/restore、密文校验、重启、rollback、失败拒绝与日志脱敏，Flask SDK stream 仍有已知阻塞                            |
| `p21-python-services`                                      | 单个普通 daemon Python/JavaScript Service 对照：默认/命名 fetch 与 RPC、callback、目标切换、prepare/restore、重启、rollback 与被引用/已删除目标拒绝                                                      |
| `p21-python-queues`                                        | 单个普通 daemon Python/JavaScript Queue 对照：四种消息类型、send/sendBatch、消费 metadata、ack/retry/DLQ、暂停重启、rollback、公开 consumer 配置和引用删除拒绝                                           |
| `p21-python-durable-objects`                               | 单个普通 daemon Python/JavaScript DO 对照：ID/fetch/RPC、SQL/KV、actor replacement、持久 alarm、重启、rollback 和退休 namespace 拒绝                                                                     |
| `p21-python-workflows`                                     | 单个普通 daemon Python/JavaScript Workflow 对照：step/retry/error、batch、pause/event、SQLite replay、版本保留、重启、rollback 与 terminate                                                              |
| `p21-python-runtime`                                       | 单个普通 daemon Python SDK/raw FFI/JavaScript 对照：共享 KV/D1/R2、转换/bytes/crypto/callback、stdlib、waitUntil、HTTP/TCP、临时文件与重启/rollback                                                      |
| `p3-contract`                                              | baseline/catalog、capability/type/config/deviation/case/source 双射；无 workerd、网络或外部 mutation                                                                                                     |
| `p3-assets`、`p3-services`、`p3-cache-images`              | 对应静态资产、Service binding（含事件源与 SIGKILL cleanup）、Cache/Images 真实 runtime 产品矩阵                                                                                                          |
| `p3-isolation`、`p3-recovery`                              | 两账户 fail-closed 与 P3 产品隔离；进程/快照/跨产品 crash recovery 与清理                                                                                                                                |
| `p3`                                                       | `p3-contract`、P0/P1/P2/Workflow 与全部 P3/L6 本地目标，不含外部 differential                                                                                                                            |
| `p3-cf-diff`                                               | 显式真实 Cloudflare portable differential；不属于 `all` 或 `--workspace`                                                                                                                                 |
| `p5-search`                                                | 本机有界 provider fixtures + pinned workerd 的 AI Search upload/index、vector/keyword/hybrid retrieval、boosting、两个 rerank 协议、Search/Chat/SSE/restart 和失败矩阵；不读取真实 credential 或访问公网 |
| `p5-ai-provider-qualification`                             | 显式 live Gate：百炼 embedding、Cohere/百炼 rerank 与 DeepSeek rewrite/Chat 的三条生产链；只接收四个受保护 provider 变量，记录去敏协议证据，不属于 `all` 或 `--workspace`                                |
| `p5`                                                       | 上述本地与 live 两个目标；仅在已授权且四个 provider 变量齐全时显式选择                                                                                                                                   |
| `s3-provider-qualification`                                | 显式 hosted S3 Gate：以 production S3 client 运行 object authority 与完整 R2 preflight（含严格 multipart），使用每轮唯一前缀并复查清理；不属于 `all` 或 `--workspace`                                    |
| `runtime`、`single-binary`                                 | supervisor、单文件离线首启/重启/损坏路径，以及单 daemon 双实例实进程隔离                                                                                                                                 |
| `p0`、`p1`、`p2`、`all`                                    | 对应集合；多个选择取并集，每个选定目标与 case 执行一次                                                                                                                                                   |

`p3-cf-diff` 每个 fixture 只使用随机 `oc-p34-*` Worker 名；Cloudflare 使用 workers.dev endpoint，
open-compute 使用 test-support 数据面。两个 provider 都通过固定 cf 与官方 v4 API 创建同前缀且
唯一的 KV namespace、D1 database、R2 bucket、Queue、Worker-owned Durable Object namespace 或
Workflow；open-compute 只通过 `CLOUDFLARE_API_BASE_URL` 选择本地 v4 origin。runner 在 mutation 前验证
目标账号和每个同名资源不存在，只对只读状态做有界重试，绝不重试写操作。cleanup 禁止扩大到非本轮
资源，按精确 Worker、binding resource ID/name 删除并再次枚举确认 absent。任何清理失败都使 Gate 失败
并保留 inventory，不得扩大到账号级批量删除或触碰其它服务。

`p5-ai-provider-qualification` job 受 `ai-search-qualification` CI environment 的 `main` branch policy 保护，只在 trusted
`main` push 或手动 dispatch 执行，并读取现有 repository secrets。固定 manifest 是
[`test/ai-provider-qualification.manifest.json`](../../test/ai-provider-qualification.manifest.json)；model、protocol、endpoint suffix
和 response schema digest 不从环境动态发现。runner 只透传 `BAILIAN_API_HOST`、`BAILIAN_API_KEY`、
`DEEPSEEK_API_KEY`、`COHERE_API_KEY`，三条 case 都经生产 AI Search service、SQLite、Vectorize 与 pinned workerd
完成索引、Search 和 Chat；429、timeout 或 contract drift 直接失败且不重试。普通 PR 与 workspace Gate 始终只跑本地
`p5-search`，因此不会接触 credential 或公网。

### Python Workers

P21 的七个 exclusive target 共九项 native case，均使用正式 pin 的 workerd、真实 daemon、SQLite 和 SigV4 fixture。Gate 先核对原生 inventory 与 registry，再串行执行每项一次；输入缺失、digest 不匹配或 ignored case 都失败。维护源码与原始 cf multipart 位于 `test/applications/python-*` 和 `test/fixtures/python-*`。每次读取核对 main/package/data 与全部 19 项官方 SDK 字节。Gate 复用静态输入，不运行 PyWrangler/Wrangler、不构建依赖、不下载 runtime。

开发侧 Python 构建临时由 `scripts/build-python.ts` 调用用户安装的 PyWrangler，优先项目 `.venv/bin/pywrangler`，再查 PATH；未安装报错，不安装或校验版本。依赖、构建解释器须显式准备，cf `--prebuilt` 负责上传。桥接只负责 Python build，移除条件见 [P25 活动 TODO](../p25-platform-follow-ups.md)。

| 目标                 | 单个 case 拥有的验证范围                                                                                                                                                                                                                                         |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Main                 | 官方 Python entrypoint、三类 package 与 package data；Python/JavaScript 共享 KV/D1/R2、outbound/body stream、secret identity、prepare/restore、promotion/restart/rollback；六类应用 import/package 拒绝、密文截断与同大小损坏后失败关闭和 fresh-process 恢复     |
| Frameworks（3 case） | Django、普通 Flask、FastAPI HTTP/template/HEAD/query/status；Django/FastAPI streaming；各自两次不可变 Version、secret、promotion/rollback、密文损坏拒绝、重启恢复、失败不切换 active 和退出清理                                                                  |
| Services             | 双向默认/命名 fetch 与 RPC、clone/callback、业务异常脱敏；双方两个 Version、目标切换、restart/rollback、引用删除 409、显式 force 删除后精确 503/SERVICE_TARGET_NOT_READY                                                                                         |
| Queues               | 共享持久 Queue 的 json/text/bytes/V8 Date 与 50,000 中文字符；Python 和 JavaScript consumer、retry/ack/DLQ、暂停帧跨重启、两个 Version/secret、rollback、被引用删除在重启前后均拒绝且后续投递正常                                                                |
| DO                   | 两个语言各自 self-owned namespace 的 native ID/name、fetch/RPC、SQL/KV、abort actor replacement、alarm、WebSocket 消息/attachment/close、两个 Version/secret、promotion/rollback、SIGKILL orphan recovery、Python class 退休与独立 JavaScript namespace 连续运行 |
| Workflows            | 两个语言各自 self-owned Flow、共享 KV effects；step retry/NonRetryableError/catch、batch、pause/event/restart/committed-step replay、旧实例固定原版本、公开 PUT 重绑 definition 后 rollback 新实例、terminate 持久性                                             |
| Runtime              | 官方 SDK/raw FFI/JavaScript 的转换、bytes/crypto/callback destroy、stdlib、waitUntil、HTTP/TCP、临时文件和 observability；共享 KV/D1/R2、Cache/Assets/Images/Markdown/Vectorize、声明 AI Search/Artifacts subset，重启与 rollback                                |

Runtime 的同一个 case 还部署未修改的 Pyodide `requests` 2.33.1 与 `httpx` 0.28.1 wheel（实际 wheel 来源与 digest 由 `pylock.toml` 和 cf capture 固定）。同步 requests、同步/异步 httpx 验证 Unicode body/header、HTTP status exception、完整分块流、真实延迟响应超时、连接拒绝与后续调用恢复；异步 httpx task cancellation 先用服务端 barrier 确认请求已接收。`subrequests=2` 的独立不可变 Version 必须只向 fixture 发出两次请求，第三次被 native quota 拒绝，下一 invocation 仍可访问。初始部署、fresh daemon restart、promotion/rollback 后复验客户端路径。requests/urllib3 的超时对外是 `ConnectionError`，测试必须同时确认其异常链包含真实 `_TimeoutError`，并证明延迟响应尚未完成，不能把任意连接失败当超时。取消后等待受控服务的 pending operation 归零并释放 listener；不把 task 取消写成底层 Fetch 立即 abort，也不依赖 client disconnect 取消。

同一 Runtime case 使用 JavaScript parent、公开 `open-compute:worker-loader` forwarding helper、最小 `child.py` 和 `globalOutbound=null` 建立 Dynamic 基线。每个阶段分别调用两次 `loadWorker()` 和两次相同 key 的 `getWorker()`，记录 factory 次数、错误及 wall time：普通 Python preparation 前的 fresh process、普通 preparation 后、fresh daemon restart 后。子 Worker 不注入普通 prepared artifact，不提高 1,000 ms startup CPU 限额；没有成功初始化就不存在 warmed successful child。wall time 不冒充启动 CPU 测量；基线记录位于 [P21](../implemented/p21-python-workers.md)，Dynamic 实现仍属于 [#126](https://github.com/elliothux/open-compute/issues/126)。

Cache API 的 default/named 命名空间各自保存 Worker 级共享可变状态；第二个 Version 写入的默认缓存在 rollback 和 fresh restart 后仍可读取，命名缓存与其他 Worker 不受影响。自动 Workers Cache 的默认版本隔离是另一个合同。

公开上传当前拒绝 cross-Script DO/Workflow binding。其用例比较 self-owned 对象，不冒充 JavaScript 直接绑定同一个 Python DO/Flow。Workflow definition 的 current Version 独立于 HTTP deployment；rollback 后必须通过公开 PUT 重绑后再创建第一版实例。Queue consumer 通过公开 API 配置，不据此声明独立 API attachment 自动随 promotion 切换。Queue 的 `force` 只授权 backlog purge，不绕过 live referrer 保护。

2026-10-04 用户暂缓 [workers-py#287](https://github.com/cloudflare/workers-py/issues/287)：Flask request/app-context streaming 留为已知上游问题，SDK 不改，原 route、捕获和失败证据保留。三个 framework case 均执行，无 ignored case；Flask 普通 HTTP/template 与部署恢复继续必选，上游修复后恢复流式 body/context/cancellation 验证。Dynamic Python 的 fresh isolate 仍按 `OC-WKR-LIMIT-001` 单列限制，普通 prepared deployment 不为它提供支持证明。

这些 case 验证已列出的实际路径，不能推广为全部上游 overload、PITR、hibernation eviction、live WebSocket 跨进程死亡、托管模型、全球调度或 hosted Cloudflare differential。权限、stale fencing、crash 和持久产品语义仍由各产品现有 Gate 共同拥有；扩展只按声明 subset 资格，不新增 Python facade。

正式 Pyodide 输入允许导入 `fcntl`、`termios`、`pty`、`tty`，这不证明宿主 POSIX 调用可用；线程启动拒绝，`multiprocessing` 只验证导入。标准库缺失列表按当前正式 pin 的实际行为固定，无历史列表兼容分支。

实际 ordinary run 标识、workspace coverage 和最终单轮 Gate 写入精简 P21 实现记录；编译、capture、source review 和一次性 native probe 不冒充完整 daemon case 通过。失败诊断和 exact coverage inputs 一律保留于 `.temp/`。

service library 的 `fresh_native_preparation_publishes_once_and_reaps_after_import_rejection` 是 preparation 所属回归：使用 cf Main 与正式 runtime 验证独立受监督 child、双重 readiness、加密 publication、重复 restore、import error 和 lease 清理。runtime 的 `isolated_python_preparation_owner_lifecycle` 与 `isolated_python_preparation_owner_rejection_matrix` 另外验证 child 取消/退出/orphan recovery 和输入拒绝，使用 verified supervisor fixture。它们不替代普通 deployment Gate。

## 单轮覆盖原则

`runtime` 另外拥有 `isolated_python_preparation_owner_lifecycle` 与
`isolated_python_preparation_owner_rejection_matrix` 两个进程组件场景：它们验证 prepare child 的共享 owner、双重 readiness、
取消/退出/lease recovery、输入拒绝及主进程隔离，使用既有 verified supervisor fixture，不替代正式 Python deployment Gate。
两个场景只在 runtime 目标登记一次。

P0.5 的 `uploads::concurrent_large_upload_keeps_runtime_responsive` 使用真实 HTTP、stock workerd、
SQLite/D1 和 SigV4 S3 fixture，上传 241,910,375 bytes（8 MiB 分片、4 并发），核对分片 authority、
D1 记录、complete 前不可见、流式读回 SHA-256 和并发轻量请求延迟。它属于普通 Gate 的固定回归，
不是吞吐 SLA 或重复压测。单元测试另覆盖 staging 取消/超时和 blocking checksum 的资源生命周期。

普通 Gate 继续使用自有 SigV4 fixture，负责确定性的权限、签名、故障、恢复和清理矩阵；真实 hosted provider
不能替代这些可控故障断言。`s3-provider-qualification` 另用 production S3 client 对专用 Cloudflare R2 bucket
执行 object authority 与 R2 startup preflight，包含非最终 part 的 5 MiB 下限、SSE-C、条件写、range、分页和
multi-delete。该目标只接受六个 `OPEN_COMPUTE_TEST_R2_S3_*` 变量，要求 HTTPS endpoint 与精确 mutation ack，
每轮使用唯一 prefix，删除 authority marker 并复查两个 prefix 均为空；报告不记录 endpoint、credential、object key
或 provider body。GitHub Actions 只在 trusted `main` push 或 `main` 手动 dispatch 使用现有 repository secrets，
普通 PR、`all` 和 `--workspace` 都不运行它。

唯一 case inventory 是 [`test/gate_cases.py`](../../test/gate_cases.py)，按完整 Rust 测试名登记。
调度器将 `--list` 的实际用例集合与该表逐项比对：新增、删除、改名、重复登记、空 Gate 或非预期
discovery 都在业务用例启动前失败。每个选定 case 用 `--exact` 执行一次，并在所属宿主内串行运行。
退出成功但通过数不足、用例被忽略或全部被过滤掉也不能通过验收；允许 Cargo metadata 声明且
discovery 确认为零用例的 workspace binary harness。

单轮仍执行完整固定矩阵、全部故障点和场景内部明确建模的多次重启或 SIGKILL，不删断言、不用 mock
代替真实路径。并发、取消、deadline 和恢复风险必须通过明确状态屏障、有界条件等待、确定性 fault point
及场景内恢复断言覆盖，不能依赖增加顶层轮数来碰撞竞态。需要额外置信度时，load、soak、fuzz 或重复
诊断作为独立活动显式执行和报告，不属于日常或最终 Gate 的完成条件。

调度器一次 `mbx test --no-run --all-features`，根据 Cargo JSON 的精确 executable 路径运行测试，
不搜索可能过期的哈希文件；typed target 则校验 tracked source/executable identity，并通过自身 JSON
discovery 枚举精确 case。执行阶段不再调用 Cargo，也不重建 JS。正确 keyed 的 `target/`、
`node_modules/`、正式 immutable 输入可复用，不清缓存、不下载。库单测、类型检查与 coverage
由下面的完整检查负责，不能塞回 Gate 循环。load/soak/fuzz 和正式打包保留独立显式入口。

构建后先对每个原生测试宿主执行一次有界 `--list`（最多 600 秒），全部成功后才进入用例阶段。
这会执行测试框架的发现功能，但不执行用例、不启动 workerd、不创建产品状态；其耗时及
`preparation_processes` 单列。它隔离冷宿主加载/系统可执行文件评估与业务超时。真实运行时验证、
readiness、事务、崩溃与恢复的超时和 fresh-process 要求均不改变。

## 多核与隔离

`--jobs N` 只并发经过隔离审查的**测试进程**；默认 `min(4, CPU 数)`，进程内保留 `--test-threads=1`。
业务数据库、generation token、临时目录、S3 fixture/prefix 和端口均由各目标独立拥有；
每个目标提供独立 `TMPDIR`，端口由系统分配。P0.1 的全局 staging 断言、supervisor、
single-binary 与 `workflow-product` 使用独占屏障。该目标同时覆盖 16 路 batch 并发、单 step 最大结果和
大结果 replay；三者组合时必须保持在 Standard 128 MiB heap 内。隔离整个目标以避免叠加 CPU、内存和
内核缓冲区压力，不减少内部并发、放宽断言超时或将失败改为重试。不能把全局状态测试直接改成多线程。
临时目录使用较短的 `.temp/gate-tmp/<随机名>/`，避免报告目录中的长目标名称超过 Unix socket
路径长度上限；退出后非空目录移入该目标的诊断目录并拒绝覆盖，成功返回但留有文件也算失败。
默认并行失败后不再提交目标，已开始的目标完成自己的清理；`--keep-going` 继续提交剩余目标，独占屏障和失败退出码保持不变。

串行/并行对比必须使用相同源码、输入、目标及构建配置，分别记录编译和执行耗时，不能用
串行冷编译与并行热缓存混算提速。基准对比是明确的测量活动，不冒充最终验收。
并发结果仍受 CPU/内存、磁盘和系统负载影响，不承诺固定提速比例。

开发/test profile 仅对 `sha2` 和 `miniz_oxide` 依赖使用 `opt-level=3`，降低每个独立数据目录
首次物化时的完整解压/摘要校验成本。workspace 源码仍未优化，debug 断言、溢出检查和 release
配置不变；不缓存或绕过完整性检查。实测及其测量口径见 [Runtime 与测试布局](../implemented/p2-7-runtime-and-test-layout.md)。

`--workspace` 通过 Cargo metadata 枚举全部启用的 test harness，使用
`mbx test --workspace --all-targets --all-features --no-run` 一次构建，再逐一执行；
拒绝缺少或未计划的 executable。它与普通 Cargo workspace 测试的目标集合相同，保留 package
工作目录（supervisor 的相对诊断输出使用本轮目标目录，避免清空环境的夹具写入源码树）；
不接受同时指定 Gate 名称。最终验收执行一个完整 round，覆盖全部 workspace 宿主；库、普通集成和
零用例 binary harness 都不被机械重复。它替代同一冻结输入上先跑 workspace、再跑重复 aggregate 的调度。
core/storage/artifacts/workers/service 五个库的故障钩子均
为进程私有，staging/数据库归属独立 TMPDIR，已经并发验证。runtime 库的 1–2 秒进程故障
窗口在并行实测中失败，因此保留独占，不放宽时限。CLI 先独占执行，使实际 ocd 的首次
加载不与定时 runtime probe 重叠；随后立即独占验证 single-binary，在长并行区前暴露正式单文件启动失败。
新增未审查目标保守独占。无需安装额外 Rust 测试运行器。

## 完整检查与最终验收

验证磁盘预算与证据生命周期见 `AGENTS.md` 的 Validation Disk Budget：任务可丢弃数据默认
不超过 32 GiB，共享 mbx 使用既有 8 GiB 总预算。长构建前和每个阶段结束后检查宿主空闲空间、
Docker volumes/images 与任务目录；复用构建目录和当前源码快照，禁止每次修复复制整套输入。
失败应及时查看，记录结论后删除不再需要的运行目录、fixture 数据、可执行文件副本与 profiles；
未定位问题只保留仍用于诊断的必要文件。成功验收保留输入标识、命令、计数、结论和 coverage
摘要，不永久保存全部成功/失败运行目录。coverage 原始对象在报告验证及 LLVM 差异调查完成前
保留，结束后清理；全局 Docker prune 会伤及其他任务，禁止使用。
阶段结束使用现有 `./scripts/clean-mbx-cache.sh` 回收共享 Rust 缓存；活跃 Docker volumes
在准备与运行期间保持引用，结束后删除本任务不再需要的容器/卷。

Rust 构建直接调用全局安装的 `mbx`，工具缺失即失败；仓库不保留逐项目版本文件、包装入口或缓存配置。项目和 worktree 共用 mbx 的全局用户缓存（macOS 默认 `~/Library/Caches/mbx/`），`mbx cache dir` 显示实际对象目录。CI 和容器安装 mbx 1.22.0。

全局设置保留 `build.rs` 校验、`CARGO_TARGET_DIR` 和 coverage 插桩参数；对象与内部状态总预算 8 GiB、最低空闲目标 4 GiB。关闭 build.rs 执行缓存、target views 和 hardlink 恢复，不自动回收普通 target。首次安装后运行：

```sh
mbx settings set build_script_execution false
mbx settings set target.views false
mbx settings set restore_hardlink false
mbx settings set gc.auto true
mbx settings set gc.max_total_size 8GiB
mbx settings set gc.min_free_size 4GiB
```

`mbx setup` 安装全局 Cargo shim，按其输出配置 PATH 后，其他项目的普通 Cargo 命令也经过 mbx。CI 使用 OS `/tmp` 放置临时 session，避免 Unix socket 路径限制。清理脚本调用 mbx 自带 GC，共享对象被回收后会在后续编译时重建；日志、失败证据、普通 target 和运行时输入不在清理范围内。

```sh
./scripts/clean-mbx-cache.sh --dry-run
./scripts/clean-mbx-cache.sh
./scripts/clean-mbx-cache.sh --max-size 2GiB
```

```sh
mbx fmt --all --check
./test/check-rust-clippy.sh
RUSTFLAGS='-D warnings' mbx check --workspace --no-default-features
mbx +1.98.0 check --workspace --all-targets
mbx metadata --no-deps --format-version 1
./test/check-boundaries.sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s test -p 'test_gate.py'
./test/check-production.py
./test/coverage.sh
# 最后一次调度同时完成普通 workspace 测试与产品验收。
./test/gate.py --workspace --final
```

production 检查只构建无 test-support 的普通开发二进制并扫描测试标记，不调用发行包装。
coverage 为当前诊断和验收保留独立输入；报告验证及 LLVM 差异调查完成后，删除已复盘的
profile、object 和旧报告副本，只保留有效摘要。Rust 行覆盖率门槛仍为 **90.00%**。
第三方 Cargo registry/git 依赖按实际 `CARGO_HOME` 排除，包含其绝对路径和 symlink 解析路径；
Docker 自定义依赖目录不应计入 workspace 分母。生产源码的排除规则不变。
APFS 使用 clone，Linux 使用可用的 filesystem reflink 保存独立 object；不支持 clone 时复制字节。
缓存替换不能改变留存 object，现有留存目录不能被覆盖。
coverage 只运行一轮；调度器拒绝把已知 coverage 插桩环境用于最终未插桩 Gate。
coverage 使用 cargo-llvm-cov `show-env --sh` 的外部运行器接口和相同 workspace 调度器，
构建缓存继续放在 `target/llvm-cov-target/`。每轮在 `.temp/coverage/run-*/` 保留独立
profile 池、merged profdata 和精确 object 副本，拒绝覆写已存在 object 目录；缓存重建不会
修改证据副本，也不把先前轮次计数混入本轮。报告先写入本轮目录；90% 门槛通过后，
保留旧标准报告副本，再发布到文档约定的 `target/llvm-cov/`。失败不会覆写先前标准报告。
supervisor 夹具的环境
由生产 spawn 清空，其默认 profile 留在本轮 `.temp/gate-run/` 诊断目录；不向生产环境透传变量，
夹具仍按原有测试源码规则排除，生产模块的插桩与计数不变。
`crates/search/examples/exact_search_benchmark.rs` 是单独留存报告的发行 benchmark，不计入
production Rust 行覆盖率；不得把生产模块移入 `examples/` 规避门槛。
`./test/coverage.sh --jobs 1` 可串行诊断，不清理普通 `target/debug/` 缓存。
真实运行时测试缺少 workerd 即失败，不允许静默 `return` 成为通过证据。

本地开发 smoke 使用 `./scripts/dev-test.sh smoke`：脚本先显式构建资产与 `test-support` 二进制，
在 `.temp/r1/` 的短 OCD 测试作用域启动空清单 daemon，再通过 `instance setup` 创建
`<OCD_DIR>/instances/dev/` 中显式 `[data].path` 的实例。它不修改正式用户 OCD_DIR；端口占用时
可用 `OPEN_COMPUTE_DEV_PORT` 和独立的 `OPEN_COMPUTE_DEV_OCD_ROOT` 选择新的测试作用域，
不覆盖已有清单。`./scripts/dev.sh` 复用该流程，但将持久开发状态放在 `.data/`。

### 按输入选择测试

SDK 消费者通过 package exports 解析 `packages/sdk/dist/` 的声明与代码。修改 SDK 生成面、
合入 SDK authority 更新或从没有 `dist/` 的 checkout 开始时，先运行
`bun run --filter @open-compute/sdk build`，再执行消费者的 typecheck/build。
SDK 的 `typecheck` 验证生成源码与严格类型，不刷新 `dist/`；旧构建产物不能证明当前 API。

修改文档、release notes 或只读脚本时不需要 Rust Gate；CI 对纯文档变更只运行 `docs-checks`。纯 SDK
、dashboard、website 或 toolchain 变更只运行对应的 JavaScript typecheck/test/build/pack 检查；混合
变更按更宽的 scope 处理。修改 SDK 生成面时运行 SDK typecheck/test/pack；普通 Rust 代码按受影响
crate 与 Gate 做一次 focused check，源码冻结后再做一次完整 workspace。修改
`packages/runtime/workerd.lock.json`、`packages/runtime/caddy.lock.json`、对应 submodule、runtime loader、
Cap'n Proto 或 Cloudflare compatibility baseline 时，真实 workerd 行为可能变化，必须重建 runtime
并至少执行 `p3-contract`、受影响的 P0/P1/P2/Workflow/P3 targets、coverage 和三平台 package；
正式 tag 仍由 release workflow 执行完整矩阵。只更新 fixture 或文档不能把旧 workerd 结果冒充新 pin
的资格。

## 日志与保留证据

调度报告位于 `.temp/gate-run/<run-id>/report.json`，包括源码/输入/测试可执行文件摘要、
revision、工具链、目标集合、轮数、并发度、一次构建耗时、每目标耗时、总墙钟和子进程 CPU 时间。
另列分轮策略、每轮精确用例、已验证 inventory、计划用例数与实际通过数；不能只用目标个数
推断覆盖完整。P3 运行另写 `contract-report.json`，列出 baseline/catalog digest、逐 contract
状态/case/deviation、L0–L3/L6 结果、Cloudflare qualification 与 Platform verdict。显式
`p3-cf-diff` 写去 credential 的 `diff-report.json` 和 cleanup inventory；未选择应用时不伪造
`application-report.json`。旧三轮历史记录保留原样，只说明当时实际执行情况，不构成当前或未来要求。
源码冻结包含所有仓库代码、配置、测试及 `docs/references/`（内嵌 runbooks 与 conformance
输入），不包含没有代码消费者的 `docs/` 规划和 `docs/implemented/` 历史记录；并行编写规划
不会中止测试。报告声明此范围；代码、维护参考文档、正式 pin 或资产变化仍会失败。
报告另列测试宿主准备阶段和实际准备进程数，不把发现进程算成已执行的业务用例。
`test_processes_executed` 统计顶层测试进程，场景内 restart/crash 的子进程仍由各测试拥有。
失败整次保留在 `failed/<run-id>/`；已运行、失败和未运行目标不混淆。终端不回显失败测试的
原始输出；日志供本地诊断，生成报告不包含 secret 或环境变量转储。

所有仓库内临时文件及失败现场留在 `.temp/<purpose>/`；Rust 产物仍在 `target/`，依赖在
`node_modules/`，业务持久状态在 `.data/`。不删除历史 `.temp/` 证据。

POC 一次性上游探测已退役；删除分类与保留回归见 [迁移记录](../implemented/p2-7-runtime-and-test-layout.md)。
`docs/implemented/g0-results.md` 保留原始字节，`D-abort` 仍是已接受限制：不能把客户端断开
当作保证取消执行。产品 Gate 保留上传取消、流中断、事务/响应失败及 crash/recovery 断言。

Linux 受控 egress 仍需显式 `OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1`，执行
`./test/test-p0-2-egress-linux.sh`；它变更 loopback 与 `/etc/hosts`，
不能在未授权宿主上运行。正式发行文件需另行授权构建，再以 `OPEN_COMPUTE_TEST_OCD`
传给 `single-binary`，本地未包装的二进制测试不能声称正式发布已通过。

Native 与 typed Gate 进程统一关闭 Bun runtime transpiler cache 和 Node compile cache，避免 cf/SDK 子进程在隔离 TMPDIR 留下缓存。临时文件残留仍判失败并保留证据，不设置缓存 allowlist。

## Installation and upgrade release qualification

PR CI executes `service_manager::tests`, including isolated umasks 000, 002,
022 and 077, unsafe existing directories, and symlink refusal. These fast
permission checks supplement the workspace Gate; they cannot prove an operating
system service was actually installed.

Publication and release recovery additionally require all five `install-lifecycle` jobs. It reuses
verified candidate binaries on Ubuntu 24.04 and Debian 13 (native x64 and ARM64),
and macOS ARM64 (`macos-15`). Linux runs a new ordinary login in a disposable
privileged container with real systemd and private writable cgroups. macOS uses
the clean hosted runner's existing GUI login for real launchd. Each OS tests both
user and system scopes: the actual installer, first setup under umask 002,
service ownership and permissions, health, restart, stop/start, no surviving
workerd children, and persisted KV values read by a deployed Worker.

The upgrade phase selects the latest official stable release strictly older than
the candidate, downloads its real binary and checksum metadata, initializes real
data, and calls the same production `run_upgrade` implementation as `ocd upgrade`.
The test-support-only driver changes the release transport to a loopback mirror
because an unpublished candidate cannot be downloaded from GitHub. It does not
provide a product download override or an alternative upgrade implementation.
Successful upgrade must replace the binary and receipt, restart the actual service,
retain keys/configuration, and preserve KV/Worker behavior across another restart.
Every SQL migration present in the previous official tag must remain byte-identical.

Success is mandatory by default. A deliberately incompatible pre-1.0 release must
explicitly set both `UPGRADE_REJECT_CODE` and `UPGRADE_REJECT_REASON` in the workflow,
explain the break in its release notes, and prove rejection preserves the old
binary, receipt, keys, configuration, running service, and readable data. Arbitrary
upgrade failures never count as a passing qualification.

Run input checks without installing anything:

```sh
python3 -B -m unittest discover -s test/install-lifecycle
```

`test/install-lifecycle/prepare.py --help` prepares verified inputs;
`test/install-lifecycle/run.py --help` describes the real OS scenario. Running the
scenario requires `OPEN_COMPUTE_INSTALL_TEST_DISPOSABLE=1`, an ordinary login,
passwordless scoped sudo, and completely empty product binary/data/service paths.
It stops the service and removes only the fixture paths it created. Never acknowledge a developer
host as disposable. Evidence under `.temp/install-lifecycle/evidence/` records
steps and exit codes without command output, database contents, or credentials;
failed runs remain under `failed/`. No additional workspace Gate is scheduled
per distribution. These Linux container checks do not qualify host reboot behavior
or alternative init systems, and do not qualify the product uninstall/purge commands. Alpine/musl, Windows, and macOS x64 are outside the
formal release target set.

The first real Debian qualification also found an independent cleanup limitation:
`ocd uninstall --purge --yes` can fail with `PATH_INVALID: purge tree contains an
ambiguous filesystem entry` after `doctor --full` materializes hardlinked OCR/tessdata aliases.
This remains fail-closed and retains data; it needs a separately scoped product
fix and real OS uninstall regression. Installation/upgrade fixture teardown uses
its own proven ownership rather than treating product purge as qualification.

Local qualification at workspace version 0.2.3 selected official v0.2.2. Its
unchanged setup-generated AI configuration contains `max_embedding_request_bytes`
and `max_embedding_response_bytes`, whereas the current model uses
`max_provider_request_bytes` and `max_provider_response_bytes`. The candidate
preflight rejects that default configuration (`CONFIG_PARSE_FAILED`, surfaced by
upgrade as `INSTANCE_REGISTRY_INVALID`). The local refusal check is explicitly labelled;
it does not certify a successful default v0.2.2 upgrade. The release workflow
retains mandatory success by default and chooses the newest official version
below the candidate; a subsequent version must qualify against official v0.2.3.
No historical config shim or automatic data conversion is introduced.
