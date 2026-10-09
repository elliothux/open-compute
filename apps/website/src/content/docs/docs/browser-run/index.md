---
title: "Browser Run"
description: "Operator-configured browser sessions, Quick Actions, DevTools and Live View."
---

Browser Run uses an explicitly configured browser backend for the selected instance. It supports the tested paths of `@cloudflare/puppeteer` 1.4.0 and `@cloudflare/playwright` 1.3.6, browser sessions, DevTools, Live View and nine Quick Actions. This does not claim support for every CDP command, client option or Cloudflare Browser API.

## Configure a backend

Browser support is disabled when `[browser]` is absent. Every capacity and deadline field below is required; these are example values, not defaults. Browser bindings are rejected without a usable backend.

For managed mode, prepare a complete, already installed `chrome-headless-shell` installation:

```sh
bun scripts/prepare-browser.ts --source /abs/installed/chrome-headless-shell --dest /abs/prepared/browser
```

The preparation tool copies the installed browser and extracts its matching offline DevTools, protocol and licenses. It does not download a browser. Production `ocd` neither bundles a browser nor searches `PATH` or installs dependencies.

Add this to the instance's `compute.toml`:

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

Managed mode starts one supervised browser process group per instance on demand, with an isolated temporary context per session. It keeps Chrome's native sandbox and has no `--no-sandbox` fallback. Temporary profiles and downloads are removed with their owning session/generation and are not backed up. Host networking and volume quotas remain operator responsibilities; browser compromise is not claimed to provide host filesystem isolation.

For external CDP, replace the entire `[browser.backend]` section with:

```toml
[browser.backend]
kind = "cdp"
url = "http://127.0.0.1:9222"
```

External CDP retains native browser behavior. The operator owns its process, profile, network and filesystem policy and must protect the original endpoint. The two backends are mutually exclusive.

## Bind and use the browser

Declare the binding in `cloudflare.config.ts`:

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

Build and deploy through the existing [cf workflow](/docs/develop/). Use the declared `env.BROWSER` with the fixed clients; the backend endpoint, internal tokens and host paths are not exposed to Workers. Dashboard also supports adding this binding.

Quick Actions are `content`, `screenshot`, `pdf`, `scrape`, `links`, `snapshot`, `markdown`, `json` and `accessibilityTree`. `/json` uses the operator's configured generation provider. Its `custom_ai` option accepts one to three configured generation aliases, tries them in order and shares one overall deadline; it does not enable arbitrary model access.

Authenticated public routes use `/client/v4/accounts/{account_id}/browser-run` or `/browser-rendering`. The implemented subset includes Quick Actions and DevTools session/browser/CDP/Live View paths. These routes do not imply that the generated `@open-compute/sdk` facade exposes every upstream Browser method.

## Remote access and operations

For clients on another machine, set `public_origin = "https://control.example.com"` in `[browser]`. It must be an exact operator-owned HTTP(S) origin without credentials, path, query or fragment. Forward authenticated `/client/v4/` HTTP and WebSocket requests to the daemon's control listener. HTTPS produces WSS URLs; absent this setting, URLs use the bound loopback listener. Request Host/Forwarded headers do not select the public authority.

Session history stores only terminal records and is bounded by both entry count and retention. Lost sessions with no known reason use `0 / Unknown`. `/metrics` exposes bounded operation, duration, in-flight and active-session measurements by instance; these do not measure independent browser CPU or memory. Optional Browser tail and separate CPU/memory accounting are not provided.

## Known limits

- Playwright download policy, events and metadata are supported, but Worker-side `saveAs()` and `createReadStream()` do not deliver Chrome's downloaded file bytes. `path()` does not grant access to the host file.
- Binding `connectionStartTime` is a string, matching the fixed Puppeteer declaration and differing from the fixed Playwright number declaration. The public v4 API uses a number.
- Browser/CDP support is scoped to verified client paths, not complete protocol parity. Containers and Cloudflare Sandbox remain unavailable.

See [Compatibility](/docs/platform/compatibility/) and [Limits](/docs/platform/limits/). Inspect the selected running binary with `ocd capabilities --json` before admitting traffic.
