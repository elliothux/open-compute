---
title: "How extension calls work"
description: "How a user Worker, facade, ocd, workerd, and native Provider communicate without ocd proxying business bytes."
---

An extension call has four participants and two transports. `ocd` authenticates a session and hands over a socket. After that, workerd talks to the Provider directly.

## Participants

| Participant | Role                                                                                                                                |
| ----------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| User Worker | Declares `services` + `props`. Calls facade RPC such as `env.FILES.list()`. Never sees `HOST`.                                      |
| Facade      | Operator JavaScript loaded from `extension.toml` `[worker].main`. Reads `this.ctx.props`. Calls `env.HOST`.                         |
| workerd     | The single supervised pinned runtime. Holds the private `HostExtensionFactory`, broker fd 4, and the session Cap'n Proto client.    |
| `ocd`       | Loads the extension at startup, issues session identity, brokers one socketpair, waits for Provider ACK, then leaves the data path. |
| Provider    | Operator native process. Accepts `OCP1` + `SCM_RIGHTS` on its stdin (fd 0), ACKs, and serves `HostExtension` on the session socket. |

There is still one workerd. The Provider is a host child of `ocd`, not a second runtime and not a tenant isolate.

## Call sequence

```text
user Worker  -- Service Binding RPC + ctx.props -->  facade (workerd isolate)
facade       -- HOST.call / HOST.stream ---------->  session Cap'n Proto
                                                     (after the socket exists)

workerd  -- OCH1 + session identity / SCM_RIGHTS -->  ocd
ocd      -- OCP1 / SCM_RIGHTS + wait for ACK ------>  Provider
ocd      -- session FD back to workerd ------------>  workerd
workerd  <--------- session Cap'n Proto ----------->  Provider
```

1. The user Worker invokes the Service Binding. Admission resolves `services[].service` as a tagged target: a Worker ID or an extension slug. `props` stay with the Binding.
2. The trusted loader instantiates the facade, injects `HOST` from `HostExtensionFactory.get(sessionIdentity)`, and passes `props` as `ctx.props`. `globalOutbound` is `null`.
3. The first `HOST` use makes workerd send `OCH1`, the session identity, and one file descriptor on the generation broker (inherited fd 4).
4. `ocd` looks up that identity in the private registry (caller Worker, version, and Binding; at most 1,024 sessions). Unknown identity fails closed.
5. `ocd` starts the Provider if needed, creates a socketpair, sends `OCP1` plus one end of the pair to the Provider control socket, and waits for ACK `0`.
6. After ACK, `ocd` checks the session is still authorized (so a generation or shutdown race cannot deliver a late FD), then returns the other end to workerd.
7. workerd and the Provider speak Cap'n Proto `HostExtension` on that socket. Unary `call` and streamed `openStream` carry operator-defined method numbers and bytes.

`ocd` does not proxy, parse, or log the business payload.

## Why two magics

| Magic  | Socket                           | Direction        | Purpose                                                  |
| ------ | -------------------------------- | ---------------- | -------------------------------------------------------- |
| `OCH1` | workerd generation broker (fd 4) | workerd → `ocd`  | Prove a validated session identity and receive a data FD |
| `OCP1` | Provider control (stdin, fd 0)   | `ocd` → Provider | Attach one session FD; Provider ACKs `0`                 |

Both transfers use `SCM_RIGHTS` with exactly one FD. Extra FDs, wrong magic, truncated identity, or a missing ACK fail closed. Formal FD passing uses workerd `--//:io_backend=cxx`.

## Session authority

The registry issues a session identity only for a verified caller Worker, version, and Binding. Different `props` produce different facade cache keys and sessions. One Provider process may serve many sessions for the same extension name.

Broker EOF or a workerd generation change closes the sessions of that generation. The Provider can be reused by the next generation. `ocd` shutdown reaps Providers with a bounded TERM/KILL. Crash of the Provider fails in-flight calls; later acquire uses 200 ms–5 s backoff and, after six consecutive failures, stays unavailable for the rest of this `ocd` life.

## What each layer must not see

| Secret or handle             | User Worker         | Facade | Provider                                     | `ocd` logs |
| ---------------------------- | ------------------- | ------ | -------------------------------------------- | ---------- |
| `HOST` / `HostExtensionPort` | no                  | yes    | n/a                                          | no         |
| Session identity             | no                  | no     | no                                           | no         |
| Provider path / raw FD       | no                  | no     | own stdin / session FD                       | no         |
| Platform tokens, S3, SQLite  | no                  | no     | no                                           | redacted   |
| `ctx.props`                  | Binding config only | yes    | only if the facade sends them in the payload | no         |

## Cloudflare boundary

`services[].props` → `ctx.props` and Service Binding RPC follow Cloudflare [Context](https://developers.cloudflare.com/workers/runtime-apis/context/) and [Service Binding RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/). Local extension resolution, Provider lifecycle, `OCH1`/`OCP1`, and `HOST` are open-compute-only. Do not treat them as hosted Cloudflare capabilities or as members of the Worker API inventory.

Limits, name sharing, and the V7 clean data-directory break are listed in [Extension API](/docs/extension/api/). A worked example is in [Implement an extension](/docs/extension/tutorial/).
