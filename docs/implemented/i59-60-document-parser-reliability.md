# I59–60 parser OCR cache and terminal failure handling

Issues [#59](https://github.com/elliothux/open-compute/issues/59) and
[#60](https://github.com/elliothux/open-compute/issues/60) close two related parser-child failure paths.

- The Xberg Tesseract result cache is explicitly disabled. The parser keeps `RLIMIT_FSIZE=0`, resolves any upstream
  cache location inside its disposable 0700 working directory, and uses the instance-owned AI Search parse cache as
  the only reusable parsed-document cache.
- Every unsuccessful child is classified as spawn/stream/I/O, timeout, exit/signal, stdout bound, or stderr bound.
  Operator logs contain only that class, exit code or Unix signal, byte counts, and a digest of bounded stderr; they
  never contain stderr text, document bytes, secrets, or paths.
- Exit/signal and output-bound failures return stable `DOCUMENT_PROCESS_FAILED` and are terminal for AI Search.
  Temporary parser/provider/storage unavailability and parser timeouts remain retryable with exponential backoff,
  but stop after five durable claim attempts. Exhaustion survives restart, records the originating stable code, and
  moves the job, item generation, and item to `error`.

The parser contract digest changed with the cache policy, so existing instance parse-cache entries miss and are
recomputed under the current contract. No old parser policy, error alias, or unbounded retry path remains.

## Cloudflare compatibility review

The change preserves the pinned `Ai.toMarkdown` overloads and its per-document `format: "error"` response shape, as
documented by Cloudflare's [Workers binding contract](https://developers.cloudflare.com/workers-ai/features/markdown-conversion/usage/binding/).
It also preserves the AI Search item contract: failed asynchronous indexing ends with `status: "error"` and the
existing optional error field, matching the official [Items binding](https://developers.cloudflare.com/ai-search/api/items/workers-binding/)
and [AI Search API](https://developers.cloudflare.com/api/resources/ai_search/). Cache placement, sanitized child
diagnostics, and the five-attempt local retry budget are implementation details inside the already-declared
`OC-AI-MARKDOWN-001` and `OC-AI-SEARCH-001` single-machine deviations. They do not change the fixed type inventory,
capability catalog, compatibility date/flags, public methods, inputs, or response schemas, so no new deviation is
introduced.

## Acceptance evidence

The frozen source passed the runtime build, Rust format/Clippy, no-default-features, Rust 1.98 MSRV, metadata, and
dependency-boundary checks. The final instrumented workspace run reported 90.06% Rust line coverage. The subsequent
single uninstrumented workspace Gate passed all 52 test processes and all 1,485 discovered cases, including the P5
real-process raster/scanned-PDF OCR path under production parser limits. No runtime download, privileged fixture, or
hosted Cloudflare mutation was used.
