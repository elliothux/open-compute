---
title: "Not available"
description: "Cloudflare platform capabilities that open-compute does not currently provide."
---

An upstream type or cf field does not mean open-compute injects the corresponding capability. Unsupported configuration fails at admission instead of creating a placeholder binding.

## Current exclusions

- Containers and Cloudchamber
- Hyperdrive
- Analytics Engine
- Full Workers for Platforms and dispatch namespaces
- General Workers AI model inference, model catalog, and AutoRAG
- Pipelines
- Rate Limiting
- mTLS certificates
- Tail Workers, distributed trace export, and Logpush

Dynamic Worker Loader is supported with documented local limits and deviations. Full Workers for Platforms, dispatch namespaces, and the experimental `allowExperimental` and `streamingTails` controls remain outside the supported surface. AI Search and Markdown Conversion do not make unrelated Workers AI methods available.

Artifacts is a current supported product and is documented under [Artifacts](/docs/artifacts/). Browser Run is available with an explicitly configured backend and documented limits; see [Browser Run](/docs/browser-run/). Containers remain planned and are not deployable capabilities.

See [Products](/docs/products/) and [Compatibility](/docs/platform/compatibility/).
