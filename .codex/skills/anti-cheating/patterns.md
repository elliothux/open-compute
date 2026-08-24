# Anti-Cheating Pattern Catalog

Use this file during cross-search and candidate classification in [SKILL.md](SKILL.md). A match is not automatic guilt—apply the four decision tests.

## Contents

1. Prompt and skill smells
2. Tool-description smells
3. Production-code smells
4. Cross-surface duplication
5. Repository grep recipes
6. Allowed exceptions
7. Rewrite guidance

## 1. Prompt & Skill Smell Patterns

### Scenario playbook in global prompt

**Smell:** Long imperative blocks in `packages/prompt/src/system.ts` or `assistant-system.ts` describing one app category's UI.

**Examples of suspicious vocabulary:**

- Media/player: `pause control`, `liked/heart`, `lyrics view`, `playlist`, `media keys`, `Space` for play/pause
- One vendor mail UI flow duplicated from `macos-mail` skill
- Chat composer / Mail reply micro-steps that belong in a domain skill
- "After OCR verification satisfies X, call workflow:save" for one task family

**Usually OK in domain skill; suspicious in global prompt.**

### Test mirroring

**Smell:** New prompt line appears after an e2e spec with the same user goal.

**How to detect:**

```bash
# List distinctive user prompts in e2e
rg -n "prompt:|submitPrompt|user.*message|打开|播放|calculate|TextEdit" e2e --glob '*.spec.ts'

# For each phrase, search production surfaces
rg -n "PHRASE" packages apps/desktop/resources/skills --glob '!e2e/**'
```

### Response shaping for qualitative specs

**Smell:** Prompt tells the model exactly what to answer, matching an e2e regex.

```bash
rg -n "expect\\(.*responseText|toMatch\\(" e2e --glob '*.spec.ts'
```

If production prompt adds matching keywords ("mention 网易云", "say 播放"), treat as Blocker.

### Over-specific retrieval policy

**Smell:** Hardcoded counts/windows that look tuned to one debugging trace:

- "at least three progressive time-window attempts"
- exact ladders like `12-24h → 2d → 3d → 5d → 1w`

Not automatically forbidden—flag when the same change set includes a failing recall/reply e2e.

### Illustrative examples vs rules

**OK:** `system-context` skill examples (`auth error`, `Q4 budget`) in example sections.

**Not OK:** Same strings used as mandatory success signals or search anchors in global prompts.

## 2. Tool Description Smell Patterns

Search tool `description` and Zod `.describe()` strings:

```bash
rg -n 'description:|\.describe\(' packages/agent-tools --glob '*.ts'
```

**Forbidden:**

- Named customer domains/pages: `use on github.com/issues`, `for Notion databases page`
- Exact button/link labels from a test recording
- "Always do X after Y" sequences copied from one workflow trace

**Allowed:**

- Schema semantics (`browserSessionId must come from open/select_tab`)
- Generic surface rules (`stable selector such as data-testid, name, aria-label`)
- Connector/API contracts

## 3. Production Code Smell Patterns

### String literals matching tests

```bash
# Set BASE to the merge base of HEAD and its upstream.
git diff --name-only "$BASE"...HEAD
git diff --name-only HEAD
git ls-files --others --exclude-standard
```

Filter the combined file set to production paths, then inspect matching literals manually. Do not use only one of these commands; each covers a different scope.

### Branching on scenario identity

Look for new conditionals on:

- `appName`, `bundleId`, `domain`, `title`, `url`, `windowTitle`
- tool argument string equality against literals
- verify substring lists arrays of expected page text

**Allowed contract switches:**

- `packages/browser/src/targets.ts`
- `packages/service/src/desktop-context.ts` app context readers
- permission / platform paths in `packages/macos/**`

### Workflow/trace special cases

Review `packages/service/src/workflow/trace-cleanup.ts` and `draft.ts` for:

- rules that keep/drop steps based on one action string from a spec
- parameterization that only makes sense for one demo form/app

### Devtools / demo copy leaking into runtime

```bash
rg -n "Demo |demo playlist|fixture|smoke test" apps/app-ui packages --glob '!e2e/**'
```

## 4. Cross-Surface Duplication Matrix

Flag when the **same scenario rule** appears in multiple layers:

| Layer A          | Layer B           | Risk                   |
| ---------------- | ----------------- | ---------------------- |
| `e2e` prompt     | global prompt     | Blocker                |
| global prompt    | domain skill      | Major (pick one owner) |
| domain skill     | tool description  | Minor–Major            |
| tool description | production branch | Blocker                |

## 5. Grep Recipes (repo-wide)

```bash
# Media/player scenario vocabulary in production surfaces
rg -n "playback|playlist|lyrics|heart state|media keys|pause control" \
  packages/prompt apps/desktop/resources/skills packages/agent-tools \
  --glob '!e2e/**'

# workflow:save mandates tied to task types
rg -n "workflow:save" packages/prompt apps/desktop/resources/skills \
  --glob '!e2e/**'

# DOM/runtime ref policy contradictions
rg -n "observe refs|DOM refs|@e|runtime ref" packages/prompt

# Hardcoded app names in changed workflow/browser code
rg -n "Mail|Calendar|Notes|Reminders|TextEdit|Calculator|网易云|NetEase|Spotify" \
  packages/agent-tools packages/service --glob '!e2e/**' --glob '!**/connectors/**' --glob '!**/macos/tools/**'

# e2e qualitative assertions
rg -n "toMatch\\(|responseText" e2e --glob '*.spec.ts'
```

## 6. Allowed Exceptions Checklist

Before filing Major/Blocker, confirm the hunk is **not**:

- [ ] A connector or macOS domain tool/skill scoped to that domain
- [ ] A platform/browser bundle-id contract in approved files
- [ ] Product IM/onboarding copy describing real settings paths
- [ ] i18n user-facing strings
- [ ] JSON schema examples using `example.com` in `.describe()` only
- [ ] officecli skill CLI examples (skill doc, not runtime)

## 7. Rewrite Guidance

| Violation                                | Preferred fix                                            |
| ---------------------------------------- | -------------------------------------------------------- |
| Global music/page playbook               | Delete or compress to generic verify-stop rule in prompt |
| Mail micro-flow in `assistant-system.ts` | Keep in `macos-mail/SKILL.md` only                       |
| Tool desc with page label                | Use semantic target fields / role/name/text              |
| Production `if (domain === ...)`         | Schema-driven verify + user/runtime inputs               |
| Prompt mirrors e2e regex                 | Remove prompt shaping; keep assertion in e2e only        |
| Mandatory `workflow:save` for one task   | Rely on `workflow` skill save gate                       |

Generic replacement templates:

**Verification (good):**

> After a UI action, verify success from returned tool state, URL/title change, extracted text, or explicit verify fields. Stop further UI steps once the user's goal is satisfied.

**Workflow save (good):**

> Call `workflow:save` only when the workflow skill save gate and deduplication rules pass.

**Surface routing (good):**

> Use integrated tools for structured app/domain operations; use `browser_use` / `computer_use` only when integrated tools cannot complete the action from current state.
