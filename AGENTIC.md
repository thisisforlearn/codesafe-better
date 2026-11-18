# AGENTIC.md — the codesafe agent contract

This file is the operating manual for coding agents using **codesafe**
(binary name: `codeguard`). It is written to be loaded into an agent's context
or referenced from `AGENTS.md`, a system prompt, a skill, or a task packet.

The goal is simple: **an agent should never guess where a bug is, never guess
whether a fix is safe, and never be able to claim success by weakening tests.**

---

## 0. The ten rules

1. **Read `codesafe brief` before editing.** It provides folder, file, line,
   symbol, snippet, evidence, root-cause hypothesis, fix plan and definition of
   done.
2. **Change the smallest possible surface.** One task per iteration.
3. **Never weaken, skip, delete, or rewrite tests to make them pass.** If a test
   is wrong, stop and explain why.
4. **Never hard-code the expected value or special-case the test input.** Tests
   verify correctness; they do not define the solution.
5. **Never mock the unit under test.** Importing it is required; mocking it is
   gaming.
6. **Prefer the safe automated fix** listed in the task. Otherwise follow the
   fix plan manually.
7. **Run the verification plan** before saying "done":
   `codesafe test --fail-on warning <repo>` and `codesafe verify <repo>`.
8. **Keep the diff scoped.** Do not refactor unrelated code or add dependencies
   without an explicit reason.
9. **Commit one logical change at a time** and record the evidence
   (commands + outcomes) in the commit body or progress notes.
10. **Non-convergence is a valid outcome.** Report blockers instead of claiming
    success.

These rules are guidance. They are enforced mechanically by `codesafe verify`,
CI, CODEOWNERS, and your repository branch protections.

---

## 1. What codesafe is

`codesafe` is a verification-first repair and optimization engine:

- It orchestrates the Python / Rust / C++ toolchains you already use.
- It emits machine-consumable findings and task packets.
- It applies only rules marked `safety = "safe"`, and keeps a fix only when
  build + tests still pass and test output is semantically unchanged.
- It detects common test-gaming patterns and blocks them.

`codesafe` never reimplements linters, compilers, or test runners. Missing tools
are skipped with a reason, never treated as success or failure.

---

## 2. Install and verify the tool

```bash
# from source
git clone https://github.com/thisisforlearn/codesafe-better.git
cd codesafe-better
cargo build --release
install -m 0755 target/release/codeguard /usr/local/bin/codeguard
ln -sf /usr/local/bin/codeguard /usr/local/bin/codesafe

# confirm
codesafe --version
codesafe schema --output json | head
```

`codesafe` and `codeguard` are the same binary. Commands below use `codesafe`;
substitute `codeguard` if that is what is on your `PATH`.

---

## 3. Output contract (why agents can rely on it)

Every command supports `--output human|json|jsonl`.

JSON mode returns exactly one envelope:

```json
{
  "ok": true,
  "result": { "command-specific": "payload" },
  "error": null,
  "meta": {
    "command": "brief",
    "path": "/repo",
    "trace_id": "cs-...",
    "timestamp_ms": 0,
    "schema_version": "1.0",
    "output_mode": "json"
  }
}
```

JSONL mode returns the same object on one line. Long-running harnesses should
use `--output jsonl` and read one line per call.

Errors:

```json
{
  "ok": false,
  "result": null,
  "error": {
    "code": "E001",
    "category": "git",
    "message": "working tree is dirty",
    "suggestion": "...",
    "retryable": true
  },
  "meta": { "...": "..." }
}
```

Exit codes:

| Code | Meaning |
|---|---|
| `0` | success / gate not triggered |
| `1` | gate failure, verification failure, surviving mutants, or reverted fix |
| `2` | configuration / usage error |

Use `codesafe schema --output json` to discover every command and flag.
Use `codesafe generate-skill --output json` to get LLM tool definitions.

---

## 4. Command selection table

| Situation | Command | Notes |
|---|---|---|
| Start a task / after every change | `codesafe brief <repo>` | Produces `AGENT_TASK.md` + `.codesafe/agent-brief.json` |
| Need only fast orientation | `codesafe scan <repo>` | Languages, build systems, tool availability |
| Need machine-readable work items | `codesafe brief <repo> --output json` | Stable task ids and priorities |
| Need optimization ideas | `codesafe optimize <repo>` | Report-only; includes gain, risk, how-to |
| Need to apply safe fixes | `codesafe fix --dry-run <repo>` then `codesafe fix <repo>` | Verified; auto-reverts on failure |
| Need to certify the change | `codesafe test --fail-on warning <repo>` | Build + tests + lint + fuzz |
| Need anti-gaming certification | `codesafe verify <repo>` | Tautologies, skips, weakened assertions, scope |
| Need to prove tests can fail | `codesafe mutate <repo>` | Diff-scoped mutation testing |
| Need test targets | `codesafe cover <repo>` / `codesafe gen-test <repo>` | Static estimate; `--write` for scaffolds |
| Need to iterate on failures | `codesafe heal <repo> --max-retries 3` | Test → safe fix → retest |
| Need a full artifact set | `codesafe report <repo>` | `report.md` + `report.json` |
| CI / batch | `codesafe all <repo> --output json` | One envelope for the whole pipeline |
| Recover/resume | `codesafe replay <repo>` | Replay the durable event log |
| TDD protocol | `codesafe spec`, `codesafe red`, `codesafe green`, `codesafe verify` | Asymmetric test-writing |

Global flags:

| Flag | Use |
|---|---|
| `--output json\|jsonl` | Machine contract |
| `--fail-on info\|warning\|error\|critical` | CI gate severity |
| `--dry-run` | Never writes, never commits |
| `--force` | Allow `fix` on a dirty tree (still checkpointed) |
| `-j, --jobs N` | Adapter parallelism |
| `--timeout S` | Per-tool timeout |
| `--config PATH` | Explicit config; otherwise auto-discovered from the repo |

---

## 5. The required agent loop

```
┌──────────────────────────────────────────────────────────────┐
│ 1. codesafe brief <repo>                                     │
│    → read the highest-priority blocking task                 │
│    → note folder, file, line, symbol, snippet, fix plan      │
└──────────────────────────────┬───────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────┐
│ 2. Inspect the real code around the snippet                  │
│    → confirm the hypothesis; do not edit blind              │
└──────────────────────────────┬───────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────┐
│ 3. Apply the smallest change                                 │
│    → prefer safe_fix if present                             │
│    → never weaken tests, never hard-code inputs             │
└──────────────────────────────┬───────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────┐
│ 4. codesafe test --fail-on warning <repo>                    │
│    → if it fails, diagnose; do not edit the test to pass    │
└──────────────────────────────┬───────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────┐
│ 5. codesafe verify <repo>                                    │
│    → zero gaming violations required                        │
└──────────────────────────────┬───────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────┐
│ 6. codesafe brief <repo>                                     │
│    → the task must disappear or decrease in priority        │
│    → if not, stop and report the blocker                    │
└──────────────────────────────┬───────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────┐
│ 7. Commit one logical change with evidence                   │
└──────────────────────────────────────────────────────────────┘
```

### Stop conditions

Stop and report when **all** are true:

- `codesafe test --fail-on warning` exits `0`
- `codesafe verify` exits `0`
- the target task is gone from the regenerated brief
- the diff contains only task-relevant changes

Stop **immediately** and report if:

- tests fail for reasons unrelated to the task
- a test appears incorrect (say so; do not rewrite it)
- the fix would require changing a public API or on-disk format outside scope
- a required tool is missing and the task cannot be verified without it

Never report "done" after exhausting a retry budget. Say what is blocking.

---

## 6. Reading a brief task

`codesafe brief --output json` returns tasks like:

```json
{
  "id": "ruff-E501",
  "priority": 1,
  "severity": "warning",
  "category": "lint",
  "language": "python",
  "folder": "src/buggy_pkg",
  "file": "src/buggy_pkg/app.py",
  "line": 25,
  "symbol": "parse_config",
  "affected_area": "function `parse_config` in src/buggy_pkg",
  "symptom": "line too long",
  "evidence": "...",
  "code_snippet": ">   25 | ...",
  "root_cause_hypothesis": "...",
  "fix_plan": ["..."],
  "safe_fix": "ruff check --fix .",
  "why_safe": "...",
  "risks": ["..."],
  "verification": ["codesafe test --fail-on warning <repo>", "codesafe verify <repo>"],
  "definition_of_done": ["..."]
}
```

Use it as follows:

- `folder` / `file` / `line` / `symbol`: where to look and what to change.
- `code_snippet`: paste into your reasoning; do not re-read the whole repo first.
- `root_cause_hypothesis`: a starting hypothesis, not a fact. Confirm it.
- `safe_fix`: if present and the tool is installed, prefer it.
- `fix_plan`: fallback manual plan.
- `risks`: things to check before keeping the change.
- `verification`: exact commands.
- `definition_of_done`: the claim you are allowed to make at the end.

If two tasks touch the same symbol, fix them together only when the combined
diff is still minimal; otherwise do one at a time.

---

## 7. Anti-gaming policy (machine-enforced)

`codesafe verify` rejects:

- tautological assertions (`assert!(true)`, `assert True`, `assert_eq!(1, 1)`)
- mocks or patches of the unit under test
- assertion removal/weakening between `HEAD` and the working tree
- newly added skips/xfails (`pytest.mark.skip`, `#[ignore]`, `it.skip`, ...)
- writes outside the spec's file allowlist
- scoped-suite pass with full-suite failure

Do **not** do any of these; they are the exact failure modes agents are known
for:

| Gaming pattern | Example | Correct action |
|---|---|---|
| Change the expected value | `expect(limit).toBe(5)` → `toBe(10)` | Fix the implementation or explain why the spec changed |
| Weaken assertions | `toEqual(invoice)` → `toBeDefined()` | Keep the assertion; fix the code |
| Skip/delete a test | new `.skip`, `#[ignore]`, deleted case | Fix the failure or report the blocker |
| Special-case test inputs | branch only for the fixture value | Implement the general rule |
| Mock the unit under test | replace `charge()` with a mock in its own test | Test the real function |
| Swallow errors | catch and return a default so the test passes | Surface the error or handle it correctly |
| Edit test config | ignore paths, lower coverage, raise timeouts | Change code, not the gate |

If the test itself is wrong, **stop and explain why**. A human (or a separate
reviewer agent) must approve test changes.

---

## 8. Prompt templates

### 8.1 System-prompt addendum (generic)

```text
You are working in a repository that uses codesafe. Before editing:
1. Run `codesafe brief <repo>` and read the highest-priority blocking task.
2. Use the task's folder/file/line/symbol/snippet to locate the code. Confirm
   the root-cause hypothesis yourself; do not edit blind.
3. Apply the smallest change that satisfies the task. Prefer `safe_fix` when
   listed. Never weaken, skip, delete, or rewrite tests; never hard-code test
   inputs; never mock the unit under test.
4. Verify with `codesafe test --fail-on warning <repo>` and
   `codesafe verify <repo>`.
5. Re-run `codesafe brief <repo>`. The task must disappear. If it does not,
   report the blocker instead of claiming success.
6. Commit one logical change and list the exact commands you ran.
Never report "done" when verification failed or was skipped.
```

### 8.2 Per-task prompt

```text
Task: <paste the task JSON or id>.
Goal: resolve exactly this task in <repo>.
Constraints:
- Change only files needed for this task.
- Do not modify or delete tests unless the task explicitly requires it.
- Do not add dependencies.
- Preserve public APIs and on-disk formats.
Process: confirm the snippet and hypothesis, apply the minimal patch, run
`codesafe fix --dry-run` if a safe fix exists, then `codesafe test --fail-on
warning` and `codesafe verify`. Finish with `codesafe brief` and a short report
containing: files changed, commands run, results, residual risks.
```

### 8.3 Code review / judge prompt

```text
Review this diff as an adversarial reviewer. You did not write it.
Check:
1. Does the diff resolve the cited task, or only hide the symptom?
2. Were tests weakened, skipped, deleted, mocked, or special-cased?
3. Are assertions still able to fail when the behaviour is wrong?
4. Is the change scoped to the cited folder/symbol?
5. Do `codesafe test --fail-on warning` and `codesafe verify` pass?
Return: APPROVE / REQUEST_CHANGES / BLOCKED, with file:line evidence.
```

### 8.4 `AGENTS.md` snippet (drop-in)

```markdown
## Verification with codesafe
- Start: `codesafe brief .` (read the top blocking task).
- Fix: smallest scoped change; prefer the task's `safe_fix`.
- Verify: `codesafe test --fail-on warning .` then `codesafe verify .`.
- Finish: re-run `codesafe brief .`; the task must disappear.
- Never weaken/skip/delete tests, hard-code test inputs, or mock the unit under
  test. If a test is wrong, stop and explain why.
Full contract: `AGENTIC.md`.
```

---

## 9. Long-running and multi-session work

For agent runs that span many turns or sessions:

1. **One task per iteration.** Do not batch unrelated findings.
2. **Commit after each verified task** with a descriptive message and the
   commands you ran. This gives a clean recovery point.
3. **Keep a progress note** in `.codesafe/progress.md` (or your own file):
   what changed, what passed, what is blocking, what to try next.
4. **Start each session** by reading `AGENTIC.md`, `codesafe brief`, and the last
   few commit messages.
5. **End each session** with a commit and a progress update, even if the task is
   incomplete.
6. **Do not leave background loops running** just to poll; each turn that stays
   alive adds context cost. Run bounded commands and stop.

Suggested cadence:

```bash
codesafe brief . --output json > /tmp/brief.json
codesafe test --fail-on warning .
codesafe verify .
codesafe mutate .          # optional, when touching auth/billing/boundaries
codesafe brief .           # confirm task disappeared
git add -A && git commit -m "fix(<scope>): <task id> — <one line>"
```

---

## 10. Safety boundaries

- `fix` refuses to run on a dirty git tree unless `--force`. Do not use
  `--force` to skip reading the diff; use it only when you understand the
  pre-existing changes.
- `fix` checkpoints before applying anything and reverts to the checkpoint if
  build or tests fail. Never bypass this by editing the same files concurrently.
- `--dry-run` never writes or commits. Use it before risky fixes.
- Rule actions are shell commands from a trusted `rules/*.toml`. Treat rules
  like CI configuration; do not run untrusted rules.
- `hermetic` clears the environment and enforces a timeout, but it is not an OS
  sandbox. Do not treat it as a security boundary.
- Never print secrets, tokens, credentials, or customer data into prompts,
  reports, issues, or commit messages.
- Generated artifacts (`report.md`, `report.json`, `AGENT_TASK.md`,
  `.codesafe/**`) are ignored for dirty-tree checks and are not committed by
  `fix`.

---

## 11. Failure recovery

| Symptom | Meaning | Action |
|---|---|---|
| `E001` dirty tree | `fix` needs a clean checkpoint | Commit/stash your work, or use `--force` deliberately |
| `fix` reports reverted | build/tests failed after the rule | Read the task's `fix_plan`; fix manually; do not force-commit |
| `test` fails on a test you touched | possible gaming or wrong expectation | Revert the test change; fix the implementation or explain |
| `verify` reports `scope-inverse` | wrote outside the spec allowlist | Narrow the diff or update the spec with a reason |
| `verify` reports `new-skip` | you hid a failure | Remove the skip; fix the cause |
| `mutate` reports survivors | tests run but do not defend behaviour | Add focused tests for the surviving behaviour |
| Tool missing | optional toolchain not installed | Use the next-best command; do not claim unverified success |
| Brief unchanged after fix | fix did not address the cause | Stop and report the blocker |

---

## 12. JSON recipes

Read the top task:

```bash
codesafe brief . --output json \
  | jq -r '.result.tasks[0] | "\(.severity) \(.file):\(.line) \(.symbol) — \(.symptom)"'
```

List only auto-fixable tasks:

```bash
codesafe brief . --output json \
  | jq -r '.result.tasks[] | select(.safe_fix != null) | "\(.id)\t\(.safe_fix)"'
```

Get optimization suggestions for one file:

```bash
codesafe optimize . --output json \
  | jq -r '.result.optimizations[] | select(.file=="src/app.py") | "\(.line)\t\(.title)\t\(.suggested_change)"'
```

Check mutation survivors:

```bash
codesafe mutate . --output json \
  | jq -r '.result | "total=\(.total) killed=\(.killed) survived=\(.survived)"'
```

---

## 13. Testing your agent setup

Do not judge an instruction file by how complete it looks. Verify it with a
small, reversible task:

1. Pick a repository and a task that crosses the important path: locate the
   module, change one behaviour, update a focused test, run verification.
2. Ask the agent to summarize the task and its plan **before editing**.
3. Inspect the diff for scope creep, test weakening, and generated files.
4. Confirm `codesafe verify` passed and the task disappeared from the brief.
5. Record whether the agent followed the contract; update `AGENTIC.md` based on
   real failures, not hypothetical ones.

Test the loading path too: some agents read `AGENTS.md`, some read
`CLAUDE.md`, some need a skill/tool definition. Keep a thin adapter file if
needed and point it at this contract.

---

## 14. Reference

- Tool definitions: `codesafe schema --output json`,
  `codesafe generate-skill --output json`
- Reports: `report.md`, `report.json`, `.codesafe/agent-brief.json`
- Artifacts: `.codesafe/optimizations.json`, `.codesafe/mutation-report.txt`,
  `.codesafe/events.jsonl`, `.codesafe/spec.json`
- Repository: https://github.com/thisisforlearn/codesafe-better
- Maintainer: **Vaibhav Agarwal** ([@thisisforlearn](https://github.com/thisisforlearn))

---

## 15. Design sources

This contract follows current agent-instruction guidance:

- **AGENTS.md** — repository instructions should be concise, versioned, and
  cover setup, commands, boundaries, and definition of done.
- **Anthropic / OpenAI prompting guidance** — be explicit, give the reason for a
  rule, separate instructions/context/examples, and state that tests verify
  correctness rather than define the solution.
- **Mutation-testing practice for agent-written code** — prove tests can fail;
  scope mutation testing to the diff; treat surviving mutants as bug reports.
- **Long-running agent harnesses** — one feature at a time, commit progress,
  keep a progress file, and verify before declaring completion.
