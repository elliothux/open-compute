# open-compute website and documentation

The Astro site for open-compute. The marketing homepage remains a React island,
while Starlight renders the Markdown documentation under `/docs/`. A Cloudflare
Worker serves the static Astro build and handles runtime API routes without
exposing deployment credentials to the browser.

## Commands

```sh
bun run --filter @open-compute/website dev
bun run --filter @open-compute/website build
bun run --filter @open-compute/website preview
bun run --filter @open-compute/website deploy
```

`preview` serves the production build and Worker API routes together. It loads
`VITE_GITHUB_PERSONAL_ACCESS_TOKEN` from the ignored `.env` file. Despite the
legacy `VITE_` prefix, Astro's Vite configuration exposes only `PUBLIC_`
variables to the client bundle.

English documentation lives in `src/content/docs/docs/`; Simplified Chinese
content lives in `src/content/docs/docs/zh/`. The extra `docs/` content folder is
intentional: Starlight maps it to the public `/docs/` route without moving the
marketing homepage away from `/`.

For Cloudflare Workers Builds, use these settings:

- Root directory: `apps/website`
- Build command: `bun run build`
- Deploy command: `bunx wrangler deploy`
- Non-production deploy command: `bunx wrangler versions upload`

Configure `VITE_GITHUB_PERSONAL_ACCESS_TOKEN` as an encrypted Worker secret.
The `/api/github-stars` route uses it server-side and returns only the
repository star count.
