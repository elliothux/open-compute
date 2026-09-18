---
title: "Extensions"
description: "Operator-owned native extensions that user Workers call through ordinary Service Bindings."
---

An extension is operator-owned code that `ocd` loads once at startup and exposes as a Service Binding target. A user Worker still binds with Wrangler `services` and `props`. There is no new public Binding type.

An extension is not tenant-uploaded native code, not a plugin installer, and not a second workerd. Cloudflare hosted Workers do not provide this native Provider path; it is an open-compute superset on macOS and Linux.

## What you operate

Each configured name points at one local directory:

```toml
[extensions.local-files]
path = "./extensions/files"
```

The path is resolved against the loaded `ocd` config file. That directory must contain a strict `extension.toml` naming one facade Worker module and one native Provider executable. Startup opens those files without following symlinks, checks size and executable mode, and pins the opened executable by SHA-256 and file descriptor. A bad manifest, missing file, or symlink fails closed: `ocd` does not start.

## What a user Worker sees

The user Worker calls an ordinary Service Binding. The `service` value is the extension name, not a Worker ID:

```json
{
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "props": { "directory": "invoices" }
    }
  ]
}
```

`ocd` loads the facade as the Service target, injects immutable `ctx.props`, and gives **only that facade** a private `HOST` port. Ordinary Workers cannot obtain `HostExtensionFactory`, session identity, the Provider path, raw file descriptors, or platform credentials.

## Model

| Fact        | Contract                                                                                     |
| ----------- | -------------------------------------------------------------------------------------------- |
| Owner       | The operator who placed the files and listed them in config                                  |
| Load time   | `ocd` startup only; replace files and restart to pick up a new implementation                |
| Runtime     | The same supervised pinned workerd; the Provider is a separate host process                  |
| User API    | Wrangler `services` + `props` / `ctx.props`; facade-only `HOST.call` / `HOST.stream`         |
| Namespace   | Extension names share the live Worker service namespace                                      |
| Persistence | Deployments pin the extension **name**, entrypoint, and canonical props — not the file bytes |

Removing an extension does not fall back to a Worker of the same name; callers see unavailable.

## Not provided

Day1 does not install, download, version, hot-reload, grant, marketplace, pool, HTTP-fallback, or OS-sandbox extensions. `ocd` does not load dynamic libraries or tenant native code. The operator owns compatibility, host filesystem permissions, upgrade, and rollback.

Continue with:

- [Implement an extension](/docs/extension/tutorial/) — facade, Provider, config, and a user Worker
- [Extension API](/docs/extension/api/) — config, manifest, `HOST`, limits
- [How calls work](/docs/extension/architecture/) — `ocd`, workerd, Provider, and the user Worker
