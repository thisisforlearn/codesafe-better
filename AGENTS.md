# AGENTS.md

This repository uses **codesafe** (`codeguard`) for verification-first agentic
coding. Read **`AGENTIC.md`** for the full contract, prompt templates, JSON
recipes, and long-running-agent protocol.

## Before editing

```bash
codesafe brief . --output json     # highest-priority task with folder/file/line/symbol/snippet
codesafe test --fail-on warning .  # baseline: must be understood before you change anything
```

## Rules

- Make the **smallest scoped change** that resolves one task.
- Prefer the task's `safe_fix` when present.
- **Never** weaken, skip, delete, or rewrite tests; never hard-code test inputs;
  never mock the unit under test.
- If a test looks wrong, **stop and explain why** instead of changing it.
- Do not add dependencies or refactor unrelated code without an explicit reason.
- Preserve public APIs and on-disk formats unless the task changes them.

## Verify before saying "done"

```bash
codesafe test --fail-on warning .
codesafe verify .
codesafe brief .          # the target task must disappear
```

A task is done only when all three succeed and the diff contains only
task-relevant changes. If verification fails or a tool is missing, report the
blocker; do not claim success.

## Artifacts

- `AGENT_TASK.md`, `.codesafe/agent-brief.json` — current task packet
- `report.md`, `report.json` — full findings, AI tips, optimizations
- `.codesafe/optimizations.json`, `.codesafe/mutation-report.txt` — advisory
- `.codesafe/spec.json` — acceptance criteria and write allowlist

Generated artifacts are ignored for dirty-tree checks and are not committed by
`codesafe fix`.

## Commands cheatsheet

| Need | Command |
|---|---|
| Task packet | `codesafe brief <repo>` |
| Safe auto-fix (verified) | `codesafe fix --dry-run <repo>` → `codesafe fix <repo>` |
| Build + tests + lint | `codesafe test --fail-on warning <repo>` |
| Anti-gaming gate | `codesafe verify <repo>` |
| Optimization advice | `codesafe optimize <repo>` |
| Prove tests can fail | `codesafe mutate <repo>` |
| Coverage hotspots | `codesafe cover <repo>` |
| Full artifacts | `codesafe report <repo>` |
| Tool schema | `codesafe schema --output json` |
| LLM tool definitions | `codesafe generate-skill --output json` |

Exit codes: `0` success, `1` verification/gate failure, `2` config/usage error.
Machine output: `--output json` (one envelope) or `--output jsonl` (one line).
