# open-compute website and documentation

The Astro site for `https://open-compute.dev`. The localized marketing homepage remains a React island, Starlight renders task-oriented documentation, and a Cloudflare Worker serves the static build plus the small GitHub API proxy.

## Commands

```sh
bun run --filter @open-compute/website dev
bun run --filter @open-compute/website build
bun run --filter @open-compute/website preview
bun run --filter @open-compute/website deploy
```

`preview` serves the production build and Worker API routes together. It loads `VITE_GITHUB_PERSONAL_ACCESS_TOKEN` from the ignored `.env` file. Despite the legacy `VITE_` prefix, Astro exposes only `PUBLIC_` variables to the client bundle.

## Documentation structure

The URL is the only locale authority. English is the unprefixed default; Simplified Chinese uses the leading `/zh/` segment across both the homepage and documentation:

| Content       | English  | Simplified Chinese |
| ------------- | -------- | ------------------ |
| Homepage      | `/`      | `/zh/`             |
| Documentation | `/docs/` | `/zh/docs/`        |

`src/i18n/config.ts` owns the locale registry and path helpers. `src/i18n/home.ts` owns the complete, type-checked homepage copy for each locale. Pages select a locale on the server and pass only that locale's messages into the React app; components do not infer locale from the browser, cookies, or duplicated pathname rules.

English Markdown lives in `src/content/docs/docs/`; Simplified Chinese lives in `src/content/docs/zh/docs/`. Both languages have the same relative document tree:

- [Get started](https://open-compute.dev/docs/get-started/)
- [Develop](https://open-compute.dev/docs/develop/)
- [Operate](https://open-compute.dev/docs/operate/)
- [CLI](https://open-compute.dev/docs/cli/)
- [Products](https://open-compute.dev/docs/products/)
- [Reference](https://open-compute.dev/docs/reference/)
- [Project](https://open-compute.dev/docs/project/)

The corresponding Chinese routes begin with `https://open-compute.dev/zh/docs/`. `src/docs-topics.ts` owns the topic-scoped sidebar for both locales. Missing homepage strings fail TypeScript checking, and `scripts/check-docs.ts` requires a matching Chinese and English source file for every documentation route. There is no implicit content fallback or retained `/docs/zh/` route.

`public/llms.txt` is the small, hand-maintained entry point for coding agents; detailed content stays in the documentation pages linked from it.

## Cloudflare Workers Builds

- Root directory: `apps/website`
- Build command: `bun run build`
- Deploy command: `bun run deploy`
- Non-production deploy command: `bun run build && wrangler versions upload`

Configure `VITE_GITHUB_PERSONAL_ACCESS_TOKEN` as an encrypted Worker secret. The `/api/github-stars` route uses it server-side and returns only the repository star count.

The landing-page hero and footer videos are immutable objects in the `open-compute` R2 bucket and are served through `https://static.open-compute.dev`. Keep large video files out of `public/`; use content-addressed object names so long-lived browser and Cloudflare caches remain safe.
