---
title: "Implement an extension"
description: "Build a local-files extension: manifest, facade Worker, native Provider, ocd config, and a user Worker binding."
---

This walkthrough implements a `local-files` extension that lists and reads files from directories the operator placed next to the Provider. A user Worker binds it with Wrangler `services` and `props`. There is no new public Binding type.

You need a running `ocd` on macOS or Linux, a writable config file, and a native toolchain that can speak Unix SCM_RIGHTS plus Cap'n Proto. A complete working Provider ships in the open-compute repository as the test fixture [crates/service/src/bin/host_extension_test_provider/](https://github.com/elliothux/open-compute/blob/main/crates/service/src/bin/host_extension_test_provider/main.rs) — schema, Cap'n Proto bindings, and the attach loop in about 300 lines of Rust. Use it as the live reference for your own Provider; it is a `test-support` binary and is not part of the release artifact.

## 1. Create the extension directory

Place the directory next to the config that will name it. Paths inside the manifest are relative, cannot contain `.` or `..`, and are opened without following symlinks.

```text
extensions/files/
  extension.toml
  worker/index.js
  native/files-provider
```

`extension.toml` has exactly two required sections:

```toml
[worker]
main = "worker/index.js"

[native]
executable = "native/files-provider"
```

The facade must be UTF-8 JavaScript (or compiled-to-JS) source, at most 64 MiB. The executable must be a regular file, owner-executable, and not group- or world-writable, at most 256 MiB. `extension.toml` itself is at most 64 KiB. Unknown fields are rejected.

## 2. Write the facade

The facade is a Service Binding target. It reads immutable Binding parameters from `this.ctx.props` and is the only Worker that receives `env.HOST`:

```ts
interface HostExtensionPort {
  call(method: number, payload: Uint8Array): Promise<Uint8Array>;
  stream(method: number, payload: Uint8Array): ReadableStream<Uint8Array>;
}
```

A local-files facade that lists a directory with unary method `1` and reads a file with stream method `2`:

```js
import { WorkerEntrypoint } from "cloudflare:workers";

const encode = (value) => new TextEncoder().encode(value);

export default class Files extends WorkerEntrypoint {
  async list() {
    return new TextDecoder().decode(
      await this.env.HOST.call(1, encode(this.ctx.props.directory)),
    );
  }

  async read(name) {
    return new Response(
      this.env.HOST.stream(2, encode(`${this.ctx.props.directory}/${name}`)),
    ).text();
  }
}
```

The facade has `globalOutbound` set to `null`. It cannot fetch the public internet, reach platform listeners, or see S3, SQLite, loader keys, or internal tokens. Codec, path checks, and host permissions stay in this operator-owned code and in the Provider.

`HostExtensionFactory` is private to the trusted loader. Do not import it, persist `HOST`, or try to RPC-transfer the port again.

## 3. Write the native Provider

`ocd` starts one Provider process per extension name, on first session, with:

- the opened executable (identity pinned when the instance starts);
- a cleared environment and empty argv;
- working directory `<data.path>/runtime/extensions/<name>` (not the source directory);
- control socket inherited as standard input (file descriptor 0).

The control socket is only for session attach. Business bytes never go through `ocd`.

On standard input the Provider loops:

1. Read 20 bytes plus one file descriptor (`SCM_RIGHTS`).
2. Require magic `OCP2`, a 16-byte nonce, and exactly one FD.
3. Treat that FD as a Cap'n Proto two-party session.
4. Write ACK byte `0` followed by the same 16-byte nonce.

The session schema is:

```text
interface HostExtension {
  call @0 (method :UInt32, payload :Data) -> (payload :Data);
  openStream @1 (method :UInt32, payload :Data) -> (stream :HostExtensionStream);
}

interface HostExtensionStream {
  read @0 (maxBytes :UInt32) -> (payload :Data, eof :Bool);
  cancel @1 ();
}
```

Method numbers and payload codec are yours. Unary request and response are each at most 8 MiB. Each stream chunk is at most 64 KiB; a stream may not read more than 64 MiB in total.

For this tutorial, method `1` lists regular files in the relative directory named by the payload (UTF-8, no `/` prefix, no `..`) and returns a newline-separated listing. Method `2` opens that relative path as a file and streams its bytes. Open through `openat` with `O_NOFOLLOW` from the process working directory, or from another operator-chosen root the Provider itself opens. `props` never appear in argv, environment, or a mutable Provider config; the facade must send them in the payload. The linked fixture implements exactly this contract, including the `O_NOFOLLOW` path walk.

A mismatched ABI, extra file descriptor, unknown identity, attach timeout, or disconnect fails closed. Cancellation does not roll back Provider side effects and does not replay the request.

## 4. Register it in `ocd` config

```toml
[extensions.local-files]
path = "./extensions/files"
```

The name `local-files` is a lowercase ASCII slug (1–63 characters, alphanumeric start and end, hyphens allowed). It shares the Worker service namespace: startup, Worker create, and Service upload all reject a collision.

Check, then restart. Extensions do not hot-reload.

```sh
ocd --config /var/lib/open-compute/instances/default/compute.toml config check
ocd instance restart default
```

`config check` validates the TOML. Instance startup is what opens `extension.toml`, the facade, and the executable. If those files are wrong, the target instance refuses to start.

## 5. Bind from a user Worker

In the application `wrangler.jsonc`, declare a Service Binding whose `service` is the extension name:

```json
{
  "name": "billing",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "props": { "directory": "invoices" }
    }
  ]
}
```

Two Bindings to the same extension with different `props` get different facade cache keys and sessions. They may share one Provider process. Deploy pins the name, entrypoint, and canonical props; replacing extension files and restarting the instance makes existing deployments use the new implementation.

Call it like any other Service Binding RPC:

```ts
export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const listing = await env.FILES.list();
    const body = await env.FILES.read("a.txt");
    return new Response(`${listing}\n${body}`);
  },
} satisfies ExportedHandler<Env>;
```

```sh
ocd wrangler deploy
```

The user Worker never sees `HOST`. It only sees the facade's exported methods.

## 6. Confirm the boundary

- The user Worker talks to the facade over Service Binding RPC (`ctx.props` semantics match Cloudflare [Context](https://developers.cloudflare.com/workers/runtime-apis/context/) and [Service Binding RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)).
- The facade talks to the Provider over `HOST.call` / `HOST.stream`.
- `ocd` brokers one session socket and then steps out of the data path.

Details of that handshake are in [How calls work](/docs/extension/architecture/). The config and `HOST` surface are in [Extension API](/docs/extension/api/).
