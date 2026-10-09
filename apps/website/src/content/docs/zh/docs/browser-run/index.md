---
title: "Browser Run"
description: "由 operator 配置的浏览器 session、Quick Actions、DevTools 与 Live View。"
---

Browser Run 使用所选实例显式配置的 backend。支持固定 `@cloudflare/puppeteer` 1.4.0、`@cloudflare/playwright` 1.3.6 的已验证路径、browser session、DevTools、Live View 与九项 Quick Actions；不承诺全部 CDP 命令、客户端选项或 Cloudflare Browser API。

## 配置 backend

未配置 `[browser]` 时 Browser 不启用。下面全部容量与 deadline 字段必填，数值只是示例，不是默认值。实例没有可用 backend 时拒绝 Browser binding。

managed 模式先准备完整、已安装的 `chrome-headless-shell`：

```sh
bun scripts/prepare-browser.ts --source /abs/installed/chrome-headless-shell --dest /abs/prepared/browser
```

工具复制现有安装并提取匹配的离线 DevTools、协议和许可证，不下载浏览器。生产 `ocd` 不内嵌浏览器、不搜索 `PATH`，也不安装依赖。

加入实例 `compute.toml`：

```toml
[browser]
max_sessions = 4
max_pending_acquires = 4
acquire_timeout_ms = 10000
command_timeout_ms = 10000
max_connections = 8
max_frontend_requests = 64
max_actions = 2
max_body_bytes = 1048576
max_result_bytes = 16777216
max_download_bytes = 16777216
max_download_files = 16
max_message_bytes = 16777216
max_queued_messages = 256
max_history_entries = 1000
history_retention_ms = 86400000

[browser.backend]
kind = "managed"
executable = "/abs/prepared/browser/chrome-headless-shell"
browser_idle_timeout_ms = 1000
shutdown_grace_ms = 100
```

managed 按需启动，每 instance 一个受监督进程组，每 session 独立临时 context。保留 Chrome 原生 sandbox，没有 `--no-sandbox` fallback。临时 profile 和下载随所属 session/generation 回收，不纳入备份。宿主网络策略和卷配额由 operator 管理；不承诺浏览器被攻破后的宿主文件隔离。

external CDP 用以下内容替换整个 `[browser.backend]`，不保留 managed 字段：

```toml
[browser.backend]
kind = "cdp"
url = "http://127.0.0.1:9222"
```

external CDP 保留原生浏览器行为。operator 负责其进程、profile、网络及文件策略，并保护原始 endpoint。两个 backend 互斥。

## 声明与使用 binding

在 `cloudflare.config.ts` 声明：

```ts
import { bindings, defineConfig } from "cf/config";

export default defineConfig({
  worker: {
    name: "browser-example",
    entrypoint: "./src/index.ts",
    compatibilityDate: "2026-09-08",
    compatibilityFlags: ["nodejs_compat"],
    env: { BROWSER: bindings.browser() },
  },
});
```

沿用 [cf 工作流](/zh/docs/develop/)构建和部署。固定客户端使用声明的 `env.BROWSER`；backend endpoint、内部 token 与宿主路径不会进入 Worker。Dashboard 也支持添加此 binding。

Quick Actions 为 `content`、`screenshot`、`pdf`、`scrape`、`links`、`snapshot`、`markdown`、`json`、`accessibilityTree`。`/json` 使用 operator 配置的 generation provider；`custom_ai` 接受一至三个已配置 generation alias，按顺序尝试并共享一个总 deadline，不开放任意模型访问。

已鉴权的公开路由使用 `/client/v4/accounts/{account_id}/browser-run` 或 `/browser-rendering`。实现子集包括 Quick Actions 和 DevTools session/browser/CDP/Live View；这些路由不代表生成的 `@open-compute/sdk` facade 已开放全部上游 Browser 方法。

## 远程访问与运维

其它机器上的客户端需要在 `[browser]` 设置 `public_origin = "https://control.example.com"`。它必须是 operator 管理的精确 HTTP(S) origin，不含凭据、路径、query 或 fragment。将已鉴权的 `/client/v4/` HTTP 与 WebSocket 请求转发到 daemon control listener。HTTPS 生成 WSS URL；未设置时使用绑定的 loopback listener。请求 Host/Forwarded 不参与 public authority 选择。

session history 只保留终态记录，同时受条数与保留时间限制。原因不明的 lost session 使用 `0 / Unknown`。`/metrics` 按实例提供有界 operation、duration、in-flight 与 active-session 指标；不代表独立 browser CPU/memory。Browser tail 与独立 CPU/memory accounting 尚未提供。

## 已知限制

- Playwright 下载策略、事件与元数据可用，但 Worker 侧 `saveAs()`、`createReadStream()` 不交付 Chrome 下载文件的字节。`path()` 不授予宿主文件访问权。
- binding 的 `connectionStartTime` 返回 string，符合固定 Puppeteer 声明，与固定 Playwright 的 number 声明存在差异；公开 v4 API 使用 number。
- Browser/CDP 只声明已验证的客户端路径，不代表完整协议对齐。Containers 和 Cloudflare Sandbox 仍未提供。

参见[兼容性](/zh/docs/platform/compatibility/)与[限制](/zh/docs/platform/limits/)。接入流量前用 `ocd capabilities --json` 核对运行中的所选版本。
