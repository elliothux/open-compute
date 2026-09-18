---
title: "Extension API"
description: "Operator config, extension.toml, Wrangler services and props, and the facade-only HOST port."
---

This page is the operator and Worker contract for local native extensions. It is not a Cloudflare hosted Workers API.

## `[extensions.<name>]`

```toml
[extensions.local-files]
path = "./extensions/files"
```

| Field              | Meaning                                                                                                                                                |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| table key `<name>` | Service name the user Worker puts in `services[].service`. Lowercase ASCII slug, 1–63 characters, starts and ends with alphanumeric, hyphens allowed.  |
| `path`             | Directory containing `extension.toml`. Relative paths resolve against the directory of the loaded config file. Must be absolute after that resolution. |

Unknown fields are rejected. Names share the live Worker service namespace. Startup refuses a configured extension that collides with an existing Worker; later Worker create and Service upload refuse a Worker that collides with an extension.

macOS and Linux only. There is no Windows Provider path in this release.

## `extension.toml`

```toml
[worker]
main = "worker/index.js"

[native]
executable = "native/files-provider"
```

| Section    | Field        | Meaning                                  |
| ---------- | ------------ | ---------------------------------------- |
| `[worker]` | `main`       | Relative path to the UTF-8 facade module |
| `[native]` | `executable` | Relative path to the Provider binary     |

Paths must be relative with only normal components (no `.`, `..`, absolute, or symlink). `ocd` opens the directory, manifest, facade, and executable with `NOFOLLOW`. Limits: manifest 64 KiB, facade 64 MiB, executable 256 MiB. The executable must be a regular file, have the owner-execute bit, and must not be group- or world-writable.

There is no cwd fallback, network download, dynamic-library load, version field, or hot update. Errors fail startup.

## Wrangler `services` + `props`

User Workers keep the standard Service Binding fields. Do not invent a Wrangler `extensions` array or a new `type`.

```json
{
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "entrypoint": "default",
      "props": { "directory": "invoices" }
    }
  ]
}
```

| Field        | Meaning                                                    |
| ------------ | ---------------------------------------------------------- |
| `binding`    | Name on the user Worker `env`                              |
| `service`    | Extension slug or Worker name in the shared namespace      |
| `entrypoint` | Optional facade entrypoint                                 |
| `props`      | Immutable JSON object the facade reads as `this.ctx.props` |

`props` are Binding parameters, not Provider configuration. They never enter Provider argv, environment, or a mutable global. The facade copies what the Provider needs into the `HOST` payload.

A deployment stores schema 2 tagged Service descriptors: ordinary Workers keep a stable Worker ID; local extensions keep the lowercase slug. Deployments pin name, entrypoint, and canonical props, not extension file bytes.

Schema V7 is a Day1 break: if the data directory still has old `version_services` rows, migration fails atomically and requires a clean data directory. There is no backfill.

## Facade `HOST`

Only the configured facade receives `env.HOST`. The TypeScript shape is:

```ts
interface HostExtensionPort {
  call(method: number, payload: Uint8Array): Promise<Uint8Array>;
  stream(method: number, payload: Uint8Array): ReadableStream<Uint8Array>;
}
```

| Member                    | Use                                     |
| ------------------------- | --------------------------------------- |
| `call(method, payload)`   | Unary request and response              |
| `stream(method, payload)` | Unary request, streamed response chunks |

`method` is an operator-chosen `UInt32`. Payloads are opaque bytes. Unary request and response are each at most 8 MiB. Stream chunks are at most 64 KiB; total stream read is at most 64 MiB. The existing Service authority supplies a 30 second root deadline.

The Cap'n Proto schema on the session socket is `HostExtension` / `HostExtensionStream` in the pinned workerd (`call` and `openStream`). `HostExtensionFactory.get(sessionIdentity)` exists only on the trusted loader and Durable Object host system Workers. The port is delegated once through the dynamic env capability table and cannot be persisted or RPC-transferred again.

## Provider process

| Topic             | Contract                                                                                                                                         |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Start             | First session for that extension name; one process group per name                                                                                |
| Identity          | Executable bytes and FD pinned at `ocd` startup                                                                                                  |
| Environment       | Cleared; argv empty; control socket is standard input (fd 0)                                                                                     |
| Working directory | `<data.path>/runtime/extensions/<name>`                                                                                                          |
| Attach            | Magic `OCP1`, exactly one `SCM_RIGHTS` FD, ACK byte `0`                                                                                          |
| Crash             | In-flight calls fail; later acquire retries with 200 ms–5 s backoff; six consecutive failures keep the extension unavailable for this `ocd` life |
| Shutdown          | Broker EOF or workerd generation change closes sessions; Providers may be reused by a new generation and are reaped on `ocd` shutdown            |

At most 1,024 live sessions. Each Binding (name + props + caller) gets its own session identity.

## Fail closed

`ocd` does not install, download, version, grant, hot-reload, sandbox, or HTTP-fallback an extension. Ordinary Workers cannot obtain `HOST`. A removed extension is unavailable; it does not resolve to a Worker of the same name. ABI mismatch, extra FDs, unknown identity, timeout, and disconnect are errors.

See [Implement an extension](/docs/extension/tutorial/) and [How calls work](/docs/extension/architecture/).
