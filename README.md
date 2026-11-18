# codesafe

**The verification-first repair and optimization engine for agentic coding.**

One static binary that orchestrates the Python, Rust and C++ toolchains you
already use — and turns their output into **machine-actionable repair packets**
for coding agents: exact folder, file, line, enclosing symbol, numbered snippet,
root-cause hypothesis, safe fix, risks, and a definition of done that cannot be
gamed.

`codeguard` is the binary name (for compatibility); `codesafe` is the same tool.

[![CI](https://github.com/thisisforlearn/codesafe-better/actions/workflows/ci.yml/badge.svg)](https://github.com/thisisforlearn/codesafe-better/actions/workflows/ci.yml)

---

## Why codesafe

Most "AI fixers" ask a model to guess where a bug lives. codesafe removes the
guesswork:

| Problem | codesafe answer |
|---|---|
| The agent does not know which folder/file/line to edit | `brief` emits folder, file, line, symbol, numbered snippet, imports and module summary |
| The agent does not know what the code is responsible for | enclosing block + `affected_area` + impact/risk fields |
| The agent "fixes" by weakening tests | anti-gaming checks: tautologies, mock-of-unit, assertion removal, scope violations, scoped-pass/full-fail |
| A fix silently breaks the repo | every safe fix is followed by build + tests + output comparison; failure auto-reverts |
| A failed fix loses uncommitted work | git checkpoints return the new commit hash; non-git repos get full workspace snapshots |
| "Optimize this" produces vague advice | `optimize` emits evidence, exact change, expected gain, risk and a verification command |
| Agents guess at CLI output | every command has a stable `{ ok, result, error, meta }` JSON envelope |

**Design principles**

- **No orchestration of unknown tools.** Missing tools are skipped with a reason,
  never a crash.
- **Safe by default.** Only `safety = "safe"` rules are applied automatically;
  `review` rules are guidance.
- **Verified, one-fix-per-commit.** A change is kept only when the build and the
  test suite still pass and their output is semantically unchanged.
- **Deterministic output.** Sorted file walks, stable hashes, sorted tasks and
  suggestions.
- **Agent-first contracts.** JSON schema + JSONL envelopes + a generated skill
  document with tool definitions.

---

## Install

```bash
# from source (recommended while the project is young)
git clone https://github.com/thisisforlearn/codesafe-better.git
cd codesafe-better
cargo build --release
install -m 0755 target/release/codeguard /usr/local/bin/codeguard
ln -sf /usr/local/bin/codeguard /usr/local/bin/codesafe

# or with cargo
cargo install --git https://github.com/thisisforlearn/codesafe-better codeguard
```

Prebuilt assets are attached to GitHub Releases by
`.github/workflows/release.yml`. You can also use `install.sh`:

```bash
curl -fsSL https://raw.githubusercontent.com/thisisforlearn/codesafe-better/main/install.sh | bash
```

Requirements: Rust 1.85+ to build. Python/Rust/C++ tools are optional; each is
detected and skipped when absent.

---

## 60-second tour

```bash
# 1. See what is in the repo
codesafe scan .

# 2. Build a complete repair packet for an LLM/agent
codesafe brief .            # writes AGENT_TASK.md + .codesafe/agent-brief.json

# 3. See optimization opportunities with gain/risk/how-to
codesafe optimize .

# 4. Apply only safe, verified fixes (dry-run first)
codesafe fix --dry-run .
codesafe fix .

# 5. Certify the result
codesafe test --fail-on warning .
codesafe verify .
```

A single task in `AGENT_TASK.md` looks like:

```markdown
### 1. [error] line too long (`lint`)

- location: `src/buggy_pkg/app.py` line 25 (folder `src/buggy_pkg`)
- symbol: `parse_config`
- affected area: function `parse_config` in src/buggy_pkg
- root cause hypothesis: a lint rule identifies a risky or non-idiomatic pattern at this location
- fix plan:
  - Open src/buggy_pkg/app.py:25 (folder `src/buggy_pkg`).
  - Locate the enclosing `parse_config` and confirm the reported behaviour there.
  - Apply the safe automated fix: `ruff check --fix .`.
  - Check call sites/imports of the changed symbol for contract changes.
  - Run the verification commands and keep the change only if all pass.
- verification:
  - `codesafe test --fail-on warning <repo>`
  - `codesafe verify <repo>`
- definition of done:
  - The finding `ruff-E501` no longer appears in `codesafe report`.
  - The full test suite and build pass.
  - No tests were weakened or removed.
  - The diff contains only changes relevant to this task.

```text
>    22 |     handle = open(path)  # no context manager
    23 |     for line in handle.read().splitlines():
    24 |         if "=" not in line:
>   25 |             continue
```
```

---

## Commands

| Command | What it does |
|---|---|
| `scan <path>` | Detect languages, build systems, source/config counts, available tools |
| `test <path>` | Build + test + lint + fuzz across Python/Rust/C++ in parallel |
| `bench <path>` | Median-of-N benchmarks, cache, regression detection, profiler hotspots |
| `fix <path>` | Verified safe-fix loop (built-in pack fallback; project rules override) |
| `report <path>` | Write `report.md`, `report.json` (findings, AI tips, optimizations, parsed diagnostics) |
| `all <path>` | scan → test → bench → fix → report (one JSON envelope in machine mode) |
| `brief <path>` | LLM-ready task packet + `AGENT_TASK.md` + `.codesafe/agent-brief.json` |
| `mutate <path>` | Mutation testing scoped to the current diff (`cargo-mutants` / `mutmut`); survivors are follow-up tasks |
| `optimize <path>` | Ranked optimization suggestions with evidence, gain, risk and verification |
| `cover <path>` | Static coverage estimate + complexity hotspots (no coverage tool required) |
| `gen-test <path>` | Generate tests for hotspots; `--write` creates skeletons |
| `heal <path>` | Bounded self-healing loop: test → safe fix → retest |
| `spec` / `red` / `green` / `verify` | Asymmetric TDD protocol with anti-gaming gates |
| `replay` | Replay the durable JSONL event log |
| `hermetic` | Run a command with a cleared environment and enforced timeout |
| `init <path>` | Scaffold `codesafe.toml` + a comprehensive `rules/` pack |
| `schema` | JSON schema of every command for agent tool-use |
| `generate-skill` | A skill/tool-definition document (JSON) for LLM integration |

### Global options

| Option | Description |
|---|---|
| `--config <PATH>` | Explicit config (missing file is an error). Auto-discovers repo config when omitted |
| `-j, --jobs <N>` | Jobs for language adapters (default 1) |
| `--timeout <S>` | Per-tool timeout in seconds |
| `--fail-on <LEVEL>` | Gate on severity: `info`, `warning`, `error`, `critical` |
| `--dry-run` | Report only; never writes, never commits |
| `--force` | Allow `fix` on a dirty git tree (changes are still checkpointed) |
| `--output <MODE>` | `human` (default), `json`, or `jsonl` |

**Exit codes:** `0` success, `1` gate/finding failure, `2` configuration/usage error.

---

## The agent workflow

```
          ┌──────────────┐
          │ codesafe brief│  folder+file+line+symbol+snippet+plan+DoD
          └──────┬───────┘
                 │
        ┌────────▼────────┐
        │  agent applies   │  smallest diff, no test weakening
        └────────┬────────┘
                 │
      ┌──────────▼───────────┐
      │ codesafe fix --dry-run│  see safe automated options
      └──────────┬───────────┘
                 │
        ┌────────▼────────┐
        │  codesafe test   │  build+test+lint+fuzz
        └────────┬────────┘
                 │
        ┌────────▼────────┐
        │ codesafe verify  │  anti-gaming gates
        └────────┬────────┘
                 │
        ┌────────▼────────┐
        │ codesafe brief   │  regenerate; task must disappear
        └─────────────────┘
```

Each command emits a stable envelope:

```json
{
  "ok": true,
  "result": { "...": "command-specific" },
  "error": null,
  "meta": {
    "command": "brief",
    "path": "/repo",
    "trace_id": "codesafe-...",
    "duration_ms": 42,
    "schema_version": "1.0",
    "output_mode": "json"
  }
}
```

JSONL mode emits one line per call, which is what long-running agent harnesses
want. `generate-skill` emits a tool-definition document so an LLM can call
codesafe without prompt-guessing.

---

## Safe fix loop (how a fix cannot make the repo worse)

For every `safety = "safe"` rule:

1. **Checkpoint** — git commit (returns the *new* hash, so uncommitted work is
   preserved on revert) or a full non-git workspace snapshot.
2. **Apply** the rule action (formatter / linter safe autofix).
3. **Build** the project.
4. **Run the test suite.**
5. **Compare normalized test output** with the pre-fix baseline (timing noise is
   removed). Different output means the fix changed behaviour → revert.
6. **Commit one fix** if, and only if, everything above passed. Otherwise reset
   to the checkpoint.

If the same rule produces no changes, it is reported as a no-op and no test run
is spent. Missing tools are skipped before the action runs.

---

## Built-in rules

`codesafe fix` works without any setup. When no `rules/` directory exists the
built-in pack is used; project rules always take precedence.

| Language | Safe (auto-applied) | Review (guidance) |
|---|---|---|
| Python | ruff format, ruff import sort, ruff pyupgrade, ruff safe fixes, black, isort, autoflake | ruff unsafe fixes, mypy, bandit, vulture, pytest, radon |
| Rust | cargo fmt, clippy `--fix`, cargo-sort | clippy pedantic, cargo-audit, cargo-deny, cargo-udeps, Miri, nextest |
| C/C++ | clang-format (parallel), cmake-format | clang-tidy, cppcheck, include-what-you-use, valgrind, sanitizer builds |
| Mutation | — | cargo-mutants (Rust), mutmut (Python): diff-scoped survivor reports |

Custom rule file example (`rules/python.toml`):

```toml
[[rules]]
id = "py-my-format"
language = "python"
trigger = "ruff"                       # substring, or "tool:code" (e.g. "ruff:E501")
action = "ruff format ."
safety = "safe"                        # safe | review
explanation = "Format Python sources."
category = "format"                    # optional
globs = ["**/*.py"]                    # optional; empty = all files
requires = ["ruff"]                    # optional; skip if tool absent
timeout_s = 120                        # optional per-rule timeout
severity = "warning"                   # optional finding severity
```

Rule actions are shell commands from a **trusted** rules file. Treat rules like
CI configuration: review them before running on an untrusted checkout.

---

## Optimization advisor

`codesafe optimize` is report-only and deterministic. It flags real patterns with
the exact change, expected gain, risk and verification:

- **Python:** `range(len(...))`, manual accumulation loops, nested loops,
  `open()` without `with`, bare/swallowed exceptions, `eval`/`exec`, wildcard
  imports, mutable defaults, `iterrows()`, blocking `sleep`, `deepcopy`,
  `requests` without a session, `sorted()[0]`, `== None`, `len()==0`.
- **Rust:** unnecessary `clone()`, `unwrap`/`expect` outside tests, `unsafe`
  blocks, `Vec::new` + push without `with_capacity`, per-iteration `format!`,
  `println!` in loops, index loops, intermediate `collect()`, `panic!`,
  `thread::sleep`.
- **C/C++:** `std::endl` in output loops, `using namespace std`, raw `new`,
  `malloc/free`, `push_back` without `reserve`, `strlen` in loop conditions,
  `<bits/stdc++.h>`, pass-by-value containers, index loops, nested loops.

Every suggestion includes `folder`, `file`, `line`, `symbol`, `snippet`,
`explanation`, `suggested_change`, `expected_gain`, `risk`, `why_safe`,
`how_to_apply` and `how_to_verify`.

---

## Verification protocol (anti-gaming)

`codesafe verify` inspects the working tree and reports:

- tautological assertions (`assert!(true)`, `assert True`, `assert_eq!(1,1)`)
- mocks/patching that target the unit under test (importing the unit is allowed)
- assertion removal/weakening between `HEAD` and the working tree
- write-scope violations against `.codesafe/spec.json`
- scoped-suite pass with full-suite failure

`codesafe spec` writes acceptance criteria and a file allowlist. `codesafe red`
writes a RED test and refuses mock-of-unit or tautological tests. `codesafe green`
runs the GREEN verification.

---

## Configuration

Auto-discovery order: `--config` → `<repo>/codesafe.toml` →
`<repo>/codeguard.toml` → current directory → defaults.

```toml
jobs = 4
timeout_s = 300
bench_runs = 5
regression_threshold_pct = 5.0
fail_on = "error"
dry_run = false
force = false
rules_dir = "rules"
```

Invalid values are errors, not silent defaults. `codesafe init` writes this file
(existing configs are never overwritten).

---

## Reports and artifacts

| Path | Contents |
|---|---|
| `report.md` | Human summary: tools, findings, optimizations, AI tips with code snippets |
| `report.json` | Full machine report including parsed structured diagnostics |
| `AGENT_TASK.md` | LLM-ready task list with definition of done |
| `.codesafe/agent-brief.json` | Complete task packet |
| `.codesafe/optimizations.json` | Ranked optimization suggestions |
| `.codesafe/spec.json` | Acceptance criteria + write allowlist |
| `.codesafe/events.jsonl` | Durable event-sourced orchestration log |
| `.codesafe/cache/index.json` | Benchmark cache (safe keys, no path traversal) |
| `.codesafe/target/` | Cargo target dir for repositories on noexec mounts |

Generated artifacts never make the tree look dirty, so repeated `codesafe fix`
runs work without manual cleanup.

---

## Quality and performance

- **140 automated tests** (unit + CLI integration + fixture analysis + safety).
- CI runs build, tests and CLI smoke tests on Linux, macOS and Windows, plus
  `cargo fmt --check` and `cargo clippy -D warnings`.
- Language adapters run in parallel; clang-format checks are parallel per file.
- Language detection short-circuits on the first matching source file.
- File walks are sorted and share one ignore list; config discovery is cached by
  path resolution.
- Benchmark results are cached by a deterministic source hash; a cached run only
  tests/lints when needed.
- No `unwrap()` in non-test production paths.
- `mutate` turns surviving mutants into concrete follow-up test tasks; `verify`
  also rejects newly added skips/xfails and weakened assertions.

---

## Platform notes

| Platform | Status |
|---|---|
| Linux x86_64/aarch64 | supported |
| macOS Intel/Apple Silicon | supported |
| Windows x86_64 | supported (`cmd /C` shell execution, Windows path parsing) |
| Termux | supported (`CARGO_TARGET_DIR` handled) |

`hermetic` clears the environment and enforces a timeout. Full network/FS
sandboxing is not portable; do not treat it as a security boundary.

---

## Contributing

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run -- --output json brief fixtures/python/buggy_pkg
```

Repository: <https://github.com/thisisforlearn/codesafe-better>

## Author

**Vaibhav Agarwal** ([@thisisforlearn](https://github.com/thisisforlearn))

## License

GPL-3.0-only — see [LICENSE](LICENSE).
