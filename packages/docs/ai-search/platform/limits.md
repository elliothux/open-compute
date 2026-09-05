# Limits

Declared Worker/API ceilings include: AI Search item and `toMarkdown` file ≤ 4 MiB; `toMarkdown` batch ≤ 16 files / 32 MiB input; per-file Markdown output ≤ 16 MiB; multi-instance request ≤ 10 instances; custom metadata fields ≤ 5; search results typically 1–50; context expansion 0–3.

Local quotas also bound items/chunks/vectors per instance, indexing jobs, provider concurrency, request/response bytes, and stream counts. Parser child defaults include a 30s deadline and bounded address/CPU/stderr. Live numbers:

```sh
ocd capabilities --json
```

These are not Cloudflare AI Search plan quotas. Full Workers AI inference limits do not apply because that surface is not provided.
