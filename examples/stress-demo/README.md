# stress-demo

Full-stack Worker used by compose P0 stress harnesses.

```bash
cd examples/stress-demo
bun install
bun run build
# Deploy against a running compose stack:
export CLOUDFLARE_API_BASE_URL=http://127.0.0.1:8787/client/v4
export CLOUDFLARE_ACCOUNT_ID=<from /client/v4/accounts>
export CLOUDFLARE_API_TOKEN=<deployer token from examples/container/.env>
bun run deploy:local
```

Smoke endpoints: `/api/health`, `/ping`, `/api/mixed-tx`, `/api/kv`, `/api/d1`, `/api/r2`, `/spin`.
