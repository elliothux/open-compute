# open-compute website and documentation

The Astro site for `https://open-compute.dev`. The marketing homepage remains a React island, Starlight renders task-oriented documentation under `/docs/`, and a Cloudflare Worker serves the static build plus the small GitHub API proxy.

## Commands

```sh
bun run --filter @open-compute/website dev
bun run --filter @open-compute/website build
bun run --filter @open-compute/website preview
bun run --filter @open-compute/website deploy
```

`preview` serves the production build and Worker API routes together. It loads `VITE_GITHUB_PERSONAL_ACCESS_TOKEN` from the ignored `.env` file. Despite the legacy `VITE_` prefix, Astro exposes only `PUBLIC_` variables to the client bundle.

## Documentation structure

English Markdown lives in `src/content/docs/docs/`; Simplified Chinese lives in `src/content/docs/docs/zh/`. The public structure is:

- [Get started](https://open-compute.dev/docs/get-started/)
- [Develop](https://open-compute.dev/docs/develop/)
- [Operate](https://open-compute.dev/docs/operate/)
- [CLI](https://open-compute.dev/docs/cli/)
- [Products](https://open-compute.dev/docs/products/)
- [Reference](https://open-compute.dev/docs/reference/)
- [Project](https://open-compute.dev/docs/project/)

`src/docs-navigation.ts` owns the route-scoped sidebar. Product pages keep stable public slugs, while shared development and operational guidance belongs in the task areas.

## Cloudflare Workers Builds

- Root directory: `apps/website`
- Build command: `bun run build`
- Deploy command: `bun run deploy`
- Non-production deploy command: `bun run build && wrangler versions upload`

Configure `VITE_GITHUB_PERSONAL_ACCESS_TOKEN` as an encrypted Worker secret. The `/api/github-stars` route uses it server-side and returns only the repository star count.

The landing-page hero and footer videos are immutable objects in the `open-compute` R2 bucket and are served through `https://static.open-compute.dev`. Keep large video files out of `public/`; use content-addressed object names so long-lived browser and Cloudflare caches remain safe.
