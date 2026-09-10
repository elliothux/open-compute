# Wrangler projects and deployment targets

Keep a Worker as a standard Wrangler project. The project owns `wrangler.jsonc`, its reproducibly pinned `wrangler` dependency and lockfile, source, environments, local `.dev.vars`, and tests. `ocd` owns the selected open-compute authority and injects its short-lived deployer credential only into the Wrangler child process.

## Three separate selectors

| Concept              | Selects                                                    | Stored in the project?                            |
| -------------------- | ---------------------------------------------------------- | ------------------------------------------------- |
| Local instance       | A running `ocd` on this machine                            | No                                                |
| Remote target        | A named origin, account, and external token-file reference | The non-secret target name may be used in scripts |
| Wrangler environment | A section such as `env.staging` in `wrangler.jsonc`        | Yes                                               |

With one running local instance, no selector is needed:

```sh
ocd wrangler deploy --env dev
```

Choose an exact local instance with the global selector, or choose a remote target with the Wrangler launcher selector:

```sh
ocd --instance k7m2r wrangler deploy --env staging
ocd wrangler --target company-prod deploy --env production
```

`--target`, `--instance`, and `--config` are mutually exclusive. Zero or multiple eligible local instances fail closed. Use `ocd instances` to choose one.

## Register a remote target

Create the deployer token outside the repository. It must be an absolute, current-user-owned, regular file with mode `0600` and no symlink traversal.

```sh
install -d -m 700 "$HOME/.config/open-compute"
install -m 600 /dev/null "$HOME/.config/open-compute/company-prod.token"
printf '%s\n' "$OPEN_COMPUTE_DEPLOYER_TOKEN" > "$HOME/.config/open-compute/company-prod.token"

ocd target add company-prod \
  --api-base-url https://compute.example.com/client/v4 \
  --account-id 0123456789abcdef0123456789abcdef \
  --token-file "$HOME/.config/open-compute/company-prod.token"
ocd target test company-prod
```

Remote URLs require HTTPS. Plain HTTP is accepted only for loopback. The registry is user-local (`$XDG_CONFIG_HOME/open-compute/targets.toml`, macOS Application Support, or the platform fallback), owner-only, bounded, strict-schema TOML. It stores only the token file path, never the token value.

```sh
ocd target list
ocd target show company-prod --json
ocd target remove company-prod
```

List, show, and remove do not open the token file. Remove leaves that external file in place. `target test` is the explicit network operation: it verifies authentication, the account, capabilities, and the exact Wrangler version used as the certified baseline.

## Project-local Wrangler

Pin Wrangler reproducibly in `devDependencies` and commit the Bun lockfile. The launcher walks upward from `--project` (or the startup directory) and executes the nearest `node_modules/.bin/wrangler`. It accepts minor and patch drift within the target's certified major version without a warning. A different major version emits a warning with the detected and certified versions but still launches Wrangler; the child command owns the final exit status. The launcher still refuses a missing binary or a failed version check, and it never downloads or repairs a dependency.

Everything after the Wrangler command is opaque:

```sh
ocd wrangler --project /srv/workers/billing deploy --env staging --config wrangler.jsonc
ocd wrangler -- --version
```

The successful launcher replaces the `ocd` process, preserving Wrangler's terminal, stdout/stderr, signals, and exit status. It does not parse or rewrite project configuration. Framework-generated `.wrangler/deploy/config.json` therefore keeps the standard Wrangler meaning.

## Development, release, and rollback

Use upstream local development for the fast loop; it does not access `ocd`:

```sh
bun run dev                 # wrangler dev
bun run deploy:dev          # real dev deployment on the selected local instance
bun run deploy:staging      # explicit remote target + env
bun run logs:production     # realtime tail through the production target
```

Do not treat `wrangler dev --remote` as supported. Validate integrations with an explicit dev or staging deployment instead.

Production release and rollback change the active immutable Version pointer; they do not mutate Version bytes:

```sh
ocd wrangler --target production deploy --env production
ocd wrangler --target production deployments list --env production
ocd wrangler --target production versions list --env production
ocd wrangler --target production rollback <version-id> --env production --yes
```

After an interrupted mutation, query deployments and resource state. A client disconnect does not prove the server cancelled the operation.

## CI without a target registry

CI can invoke the same pinned project-local Wrangler directly. Store only the deployer token in the CI secret store; keep the API base URL and account ID as non-secret variables.

```sh
CLOUDFLARE_API_BASE_URL="$OPEN_COMPUTE_API_BASE_URL" \
CLOUDFLARE_API_TOKEN="$OPEN_COMPUTE_DEPLOYER_TOKEN" \
CLOUDFLARE_ACCOUNT_ID="$OPEN_COMPUTE_ACCOUNT_ID" \
bun run deploy:ci
```

The repository example includes [GitHub Actions](https://github.com/elliothux/open-compute/blob/main/examples/hello-worker/ci/github-actions.yml) and [GitLab CI](https://github.com/elliothux/open-compute/blob/main/examples/hello-worker/ci/gitlab-ci.yml) templates. Both install the frozen lockfile, use the exact project dependency, disable Wrangler telemetry/error reporting, and print only non-secret target identity.

## Troubleshooting

| Failure                   | Action                                                                                                                                                   |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| No unique local instance  | Run `ocd instances`, then pass `--instance`, `--config`, or `--target`                                                                                   |
| Target not found          | Run `ocd target list`; target names are exact                                                                                                            |
| Token file rejected       | Use an absolute regular file owned by the current user with exact mode `0600`; do not use a symlink                                                      |
| Capability probe fails    | Run `ocd target test <name>` and verify network/TLS, account, and deployer role                                                                          |
| Wrangler major mismatch   | Review the detected and certified versions in the warning; use the certified major when compatibility matters                                           |
| Wrangler command fails    | Keep its exit status and Cloudflare-style error; fix the project or supported capability rather than removing bindings or retrying with rewritten config |

Wrangler's standard environment variable and environment behavior remain upstream contracts: [system environment variables](https://developers.cloudflare.com/workers/wrangler/system-environment-variables/) and [environments](https://developers.cloudflare.com/workers/wrangler/environments/).
