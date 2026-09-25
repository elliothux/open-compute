# AGENTS – Website

This file applies to `apps/website/**` and supplements the repository-level `AGENTS.md`.

## Site and Content

- Preserve `https://open-compute.dev` as the canonical origin and the URL as the locale authority: English is unprefixed and Simplified Chinese uses `/zh/`.
- Keep English and Chinese documentation trees structurally identical. Add, move, or remove both locale files together; there is no implicit locale fallback.
- Keep locale routing and copy centralized in the existing i18n modules. Components must not infer locale from browser settings, cookies, or duplicate pathname parsing.
- Keep `public/llms.txt` concise and link to durable documentation. Do not place large media in `public/`; use the existing content-addressed static asset hosting contract.

## Architecture and Operations

- Keep Astro/Starlight responsible for static pages and documentation; use React islands only where client interaction is required.
- Keep secrets and GitHub API access server-side. Only `PUBLIC_` variables may be assumed safe for the browser bundle.
- Do not deploy, publish, or mutate Cloudflare resources without explicit authorization.

## Validation

- Run `bun run --filter @open-compute/website build` after website or documentation changes. This includes locale-tree checks, TypeScript, Astro build, and internal-link validation.
- Do not commit generated `dist/` output.
