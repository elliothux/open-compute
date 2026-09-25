---
title: "open-compute documentation"
description: "Install, develop for, and operate the open-compute single-node Cloudflare Workers-compatible platform."
---

open-compute runs supported Cloudflare Workers applications on one machine you control. One scoped `ocd` daemon owns the shared control plane and Gateway; each registered instance owns its data directory, SQLite and object authority, credentials, and supervised pinned workerd runtime.

## Start here

| I want to…                                         | Go to                             |
| -------------------------------------------------- | --------------------------------- |
| Install open-compute and deploy my first Worker    | [Get started](/docs/get-started/) |
| Develop, deploy, debug, and roll back applications | [Develop](/docs/develop/)         |
| Configure and operate an open-compute host         | [Operate](/docs/operate/)         |
| Register and manage isolated accounts              | [Instances](/docs/ocd/instances/) |
| Publish instances through the shared Gateway       | [Gateway](/docs/gateway/)         |
| Look up an `ocd` command                           | [CLI](/docs/cli/)                 |
| Add an operator-owned native extension             | [Extension](/docs/extension/)     |
| See supported bindings and platform products       | [Products](/docs/products/)       |
| Check compatibility, limits, and API contracts     | [Reference](/docs/reference/)     |

## Know the boundary

open-compute is designed for self-hosted, single-machine deployments. It is not Cloudflare's global edge and does not claim multi-region replication, Anycast, hosted billing, or managed fleet behavior. Supported APIs keep their Cloudflare programming model; documented topology differences remain explicit.

Contributors can start with [Project architecture and development](/docs/project/).
