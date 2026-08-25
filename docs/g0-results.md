# G0 results

- Generated: 2026-08-25T11:33:51.258Z
- Hostname: Elliots-MacBook-Pro.local
- OS: darwin 25.6.0 arm64
- Node: v24.10.0
- Command: `./poc/g0 test all`
- Rounds: exactly 3 sequential fresh-process rounds; this run completed 3 of 3

## Pinned workerd

- Release: `v1.20260823.1`
- Version: `1.20260823.1`
- Version output: `workerd 2026-08-23`
- Artifact URL: https://github.com/cloudflare/workerd/releases/download/v1.20260823.1/workerd-darwin-arm64.gz
- Archive SHA256: `4386bf8bd6f94eed6704a7b6cdd5301daef8ada25c60f7b82e7e74d155d3beeb`
- Binary SHA256: `8c6562229cc652bcb8d926f5cd3a80e4947d723567588635fbaaebca9fdd7577`
- Compatibility date: `2026-08-22`
- Process flags: `--experimental`
- Compatibility flags: `nodejs_compat`, `rpc`, `enable_ctx_exports`, `experimental`
- Release URL: https://github.com/cloudflare/workerd/releases/tag/v1.20260823.1

## Rounds

### Round 1

- Status: ran
- Recovery seed: 1194329607 (`0x47300607`)
- Replay: `G0_RECOVERY_SEED=1194329607 ./poc/g0 test recovery`

| suite | exit | passed | failed | not-run |
| --- | --- | --- | --- | --- |
| bootstrap | 0 | 16 | 0 | 0 |
| loader | 1 | 24 | 1 | 0 |
| binding | 0 | 13 | 0 | 0 |
| durable-object | 0 | 14 | 0 | 0 |
| recovery | 0 | 17 | 0 | 0 |

### Round 2

- Status: ran
- Recovery seed: 1194329608 (`0x47300608`)
- Replay: `G0_RECOVERY_SEED=1194329608 ./poc/g0 test recovery`

| suite | exit | passed | failed | not-run |
| --- | --- | --- | --- | --- |
| bootstrap | 0 | 16 | 0 | 0 |
| loader | 1 | 24 | 1 | 0 |
| binding | 0 | 13 | 0 | 0 |
| durable-object | 0 | 14 | 0 | 0 |
| recovery | 0 | 17 | 0 | 0 |

### Round 3

- Status: ran
- Recovery seed: 1194329609 (`0x47300609`)
- Replay: `G0_RECOVERY_SEED=1194329609 ./poc/g0 test recovery`

| suite | exit | passed | failed | not-run |
| --- | --- | --- | --- | --- |
| bootstrap | 0 | 16 | 0 | 0 |
| loader | 1 | 24 | 1 | 0 |
| binding | 0 | 13 | 0 | 0 |
| durable-object | 0 | 14 | 0 | 0 |
| recovery | 0 | 17 | 0 | 0 |

## Hard matrix

| ID | case | mapped names | R1 | R2 | R3 | final |
| --- | --- | --- | --- | --- | --- | --- |
| L01 | cold load A | L01-cold-load-a | passed | passed | passed | passed |
| L02 | warm A | L02-warm-a | passed | passed | passed | passed |
| L03 | coexist A/B | L03-coexist-a-b | passed | passed | passed | passed |
| L04 | promote A to B | L04-promote-a-to-b | passed | passed | passed | passed |
| L05 | rollback B to A | L05-rollback-b-to-a | passed | passed | passed | passed |
| L06 | invalid bundle | L06-invalid-bundle | passed | passed | passed | passed |
| L07 | cold concurrency | L07-cold-concurrency | passed | passed | passed | passed |
| L08 | outbound denied | L08-outbound-denied | passed | passed | passed | passed |
| B01 | resource isolation | B01-resource-isolation | passed | passed | passed | passed |
| B02 | forged scope | B02-forged-scope | passed | passed | passed | passed |
| B03 | safe error | B03-safe-error | passed | passed | passed | passed |
| D01 | facet fetch | D01-facet-fetch | passed | passed | passed | passed |
| D02 | facet RPC | D02-facet-rpc | passed | passed | passed | passed |
| D03 | object isolation | D03-object-isolation | passed | passed | passed | passed |
| D04 | storage isolation | D04-storage-isolation | passed | passed | passed | passed |
| D05 | transaction rollback | D05-transaction-rollback | passed | passed | passed | passed |
| D06 | process restart | D06-process-restart, D06-object-2-survives, D06-supervisor-and-facet-recover, D-failAfterWrite-does-not-corrupt-other, F9-concurrent-sigkill, no-leaked-workerd-child | passed | passed | passed | passed |
| D07 | code promotion | D07-code-promotion | passed | passed | passed | passed |
| D08 | rollback | D08-rollback | passed | passed | passed | passed |
| D09 | explicit delete | D09-explicit-delete | passed | passed | passed | passed |
| R01 | repeated suite | all hard IDs L01-L08, B01-B03, D01-D09 across 3 rounds | passed | passed | passed | passed |

## Fault evidence

Whitelisted recovery fields only. Missing cases are `not-run`.

| round | case | status | classification | cycles | pendingAtKill | window |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | D-crash-loop-seeded | passed | result-unknown | 0:not-applied, 1:not-applied, 2:applied | n/a | n/a |
| 1 | F6-transaction-before-commit | passed | not-applied | n/a | n/a | n/a |
| 1 | F7-write-confirmed-response-failure | passed | applied | n/a | n/a | n/a |
| 1 | F8-idle-sigkill | passed | applied | n/a | n/a | n/a |
| 1 | F9-concurrent-sigkill | passed | applied | n/a | 1 | n/a |
| 1 | F10-promote-without-abort | passed | applied | n/a | n/a | abortIssued=false; oldCodeVersion=A; newExecutionTarget=B; observedCodeVersion=A; storageValue=3 |
| 1 | F11-abort-before-get | passed | applied | n/a | n/a | abortIssued=true; nextGetIssued=false; oldCodeVersion=A; newExecutionTarget=B; observedCodeVersion=null; lastKnownStorageValue=3; note=codeVersion after abort is not observed until the next get |
| 2 | D-crash-loop-seeded | passed | result-unknown | 0:not-applied, 1:applied, 2:not-applied | n/a | n/a |
| 2 | F6-transaction-before-commit | passed | not-applied | n/a | n/a | n/a |
| 2 | F7-write-confirmed-response-failure | passed | applied | n/a | n/a | n/a |
| 2 | F8-idle-sigkill | passed | applied | n/a | n/a | n/a |
| 2 | F9-concurrent-sigkill | passed | applied | n/a | 1 | n/a |
| 2 | F10-promote-without-abort | passed | applied | n/a | n/a | abortIssued=false; oldCodeVersion=A; newExecutionTarget=B; observedCodeVersion=A; storageValue=3 |
| 2 | F11-abort-before-get | passed | applied | n/a | n/a | abortIssued=true; nextGetIssued=false; oldCodeVersion=A; newExecutionTarget=B; observedCodeVersion=null; lastKnownStorageValue=3; note=codeVersion after abort is not observed until the next get |
| 3 | D-crash-loop-seeded | passed | result-unknown | 0:applied, 1:applied, 2:not-applied | n/a | n/a |
| 3 | F6-transaction-before-commit | passed | not-applied | n/a | n/a | n/a |
| 3 | F7-write-confirmed-response-failure | passed | applied | n/a | n/a | n/a |
| 3 | F8-idle-sigkill | passed | applied | n/a | n/a | n/a |
| 3 | F9-concurrent-sigkill | passed | applied | n/a | 1 | n/a |
| 3 | F10-promote-without-abort | passed | applied | n/a | n/a | abortIssued=false; oldCodeVersion=A; newExecutionTarget=B; observedCodeVersion=A; storageValue=3 |
| 3 | F11-abort-before-get | passed | applied | n/a | n/a | abortIssued=true; nextGetIssued=false; oldCodeVersion=A; newExecutionTarget=B; observedCodeVersion=null; lastKnownStorageValue=3; note=codeVersion after abort is not observed until the next get |

## Conditional evidence (D-abort)

Parsed `abortEvents` counts only; raw error text is omitted.

| round | status | abortEvents |
| --- | --- | --- |
| 1 | failed | 0 -> 0 |
| 2 | failed | 0 -> 0 |
| 3 | failed | 0 -> 0 |

## Accepted limitations / risk register

- Client disconnect does not abort the loaded worker `request.signal` on this pinned stock workerd (`D-abort`).
- `localDisk` is experimental; it is version-bound to the pinned workerd release and needs forward-only upgrade planning.
- An in-flight write may be `result-unknown`; this suite does not claim exactly-once.
- No alarm, WebSocket hibernation, Durable Object migration, or cross-node relocation validation.

## Hard Go conditions

| # | condition | evidence | result |
| --- | --- | --- | --- |
| 1 | Stock, pinned workerd, no source patch | bootstrap pin/checksum/config cases; official artifact URL from poc/workerd.lock; harness starts that binary | Met |
| 2 | One workerd process hosts the required static host services | bootstrap health, default/named entrypoints, internal paths not public | Met |
| 3 | workerLoader loads, caches, and isolates immutable A/B keys | L01 cold load A, L02 warm A, L03 coexist A/B | Met |
| 4 | Promotion/rollback do not overwrite bundles or invalidate cache | L04 promote A to B, L05 rollback B to A | Met |
| 5 | Loaded Worker can access only binding-scoped capability | B01 resource isolation, B02 forged scope, B03 safe error | Met |
| 6 | Dynamic DO class executes fetch, RPC, and SQLite through native facets | D01 facet fetch, D02 facet RPC, D05 transaction rollback | Met |
| 7 | Supervisor and facet storage are isolated | D04 storage isolation | Met |
| 8 | Confirmed DO writes survive SIGKILL/restart | D06 process restart and mapped recovery cases | Met |
| 9 | abort() changes code and keeps storage; only delete() drops storage | D07 code promotion, D08 rollback, D09 explicit delete | Met |
| 10 | Suite repeats unattended for three rounds | R01: hard matrix passed in all 3 sequential fresh-process rounds | Met |

## Hard No-Go conditions

| condition | evaluation |
| --- | --- |
| Core path requires fork/patch workerd | Not observed. This run used the pinned official artifact and self-owned config. |
| Loader key cannot keep immutable A/B loaded together | Not observed. L03 coexisted A and B. |
| Loaded Worker must hold a generic backend credential/Fetcher | Not observed. B01/B02 kept access binding-scoped. |
| Tenant can change props or choose another resource | Not observed. B02 rejected forged scope. |
| Dynamic DO can only be simulated with an ordinary adapter | Not observed. D01/D02/D05 used native facets. |
| Facet storage identity must include deployment ID | Not observed. D07/D08 kept storage identity across abort/get. |
| Code promotion only works by deleting SQLite | Not observed. abort/get preserved storage; only D09 delete reset it. |
| Normal restart loses confirmed DO writes | Not observed. D06 recovered confirmed writes after SIGKILL. |
| A malformed bundle/facet stably corrupts other tenant data | Not observed. L06 failed closed without taking A/B down. |
| localDisk version/recovery risk cannot be controlled by pin and release migration | Not observed as a Hard No-Go. localDisk remains experimental and is version-pinned with forward-only upgrade planning. |

## Verdict

**Conditional Go** (exit 0).

All hard matrix IDs L01-L08, B01-B03, D01-D09, and R01 passed in all 3 rounds. The only allowlisted non-pass is loader `D-abort`, with parsed abortEvents equal before and after in each completed round. That client-disconnect limitation is accepted and does not flip the run to No-Go.

This used pinned stock Cloudflare workerd, self-owned config, native workerLoader/JSRPC/facets/localDisk, no workerd patch, no Miniflare API/mock workerd, and no direct SQLite/WAL inspection.

