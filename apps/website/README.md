# open-compute website and documentation

The Astro site for open-compute. The marketing homepage remains a React island,
while Starlight renders the Markdown documentation under `/docs/`. Cloudflare
Pages Functions serve runtime data that must not expose deployment credentials
to the browser.

## Commands

```sh
bun run --filter @open-compute/website dev
bun run --filter @open-compute/website build
bun run --filter @open-compute/website build:functions
bun run --filter @open-compute/website preview
```

`preview` serves the production build and Pages Functions together. It loads
`VITE_GITHUB_PERSONAL_ACCESS_TOKEN` from the ignored `.env` file. Despite the
legacy `VITE_` prefix, Astro's Vite configuration exposes only `PUBLIC_`
variables to the client bundle.

English documentation lives in `src/content/docs/docs/`; Simplified Chinese
content lives in `src/content/docs/docs/zh/`. The extra `docs/` content folder is
intentional: Starlight maps it to the public `/docs/` route without moving the
marketing homepage away from `/`.

For Cloudflare Pages, configure `VITE_GITHUB_PERSONAL_ACCESS_TOKEN` as an
encrypted project secret before deploying. The `/api/github-stars` function
uses it server-side and returns only the repository star count.
