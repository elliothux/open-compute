---
title: "Not available"
description: "Cloudflare platform capabilities that open-compute does not currently provide."
---

An upstream type or Wrangler field does not mean open-compute injects the corresponding capability. Unsupported configuration fails at admission instead of creating a placeholder binding.

## Current exclusions

- Browser Run and browser rendering
- Containers and Cloudchamber
- Hyperdrive
- Analytics Engine
- Full Workers for Platforms and dispatch namespaces
- General Workers AI model inference, model catalog, and AutoRAG
- Pipelines
- Rate Limiting
- mTLS certificates
- Tail Workers, distributed trace export, and Logpush

Dynamic Worker Loader has a bounded native surface, but the complete product remains blocked on standard CPU, memory, and subrequest limit enforcement. AI Search and Markdown Conversion do not make unrelated Workers AI methods available.

Artifacts is a current supported product and is documented under [Artifacts](/docs/artifacts/). Browser Run and Containers have design work in progress but are not deployable capabilities.

See [Products](/docs/products/) and [Compatibility](/docs/platform/compatibility/).
