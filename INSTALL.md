# codesafe — cross-platform install & usage guide

## Install (one-liner)

### Linux / macOS / Termux (bash)
```bash
curl -fsSL https://raw.githubusercontent.com/thisisforlearn/codesafe-better/main/install.sh | bash
```

### Windows (PowerShell)
```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force
irm https://raw.githubusercontent.com/thisisforlearn/codesafe-better/main/install.ps1 | iex
```

## What gets installed

- `codeguard` — the main binary (use this command)
- `codesafe` — symlink/alias to `codeguard` (same binary, friendlier name)

Both are identical — use whichever name you prefer.

## Quick start

```bash
# Scan a project for languages & tools
codeguard scan .

# Run tests, lint, sanitizers (JSON output for agents)
codeguard --output json test .

# Full pipeline: scan → test → bench → fix → report
codeguard all .

# Write human + AI-readable report
codeguard report .
```

## Commands

| Command | Description |
|---------|-------------|
| `scan` | Detect languages, build systems, available tools |
| `test` | Build & run tests, lint, sanitizers |
| `bench` | Run benchmarks, detect regressions |
| `fix` | Apply safe rule-based fixes with verification |
| `report` | Write `report.md` (human) + `report.json` (AI) |
| `all` | Scan → test → bench → fix → report |
| `init` | Initialize `codesafe.toml` config & a comprehensive `rules/` pack |
| `brief` | Build an LLM-ready task packet (`AGENT_TASK.md` + JSON) |
| `optimize` | Ranked optimization suggestions with gain/risk/verification |
| `mutate` | Diff-scoped mutation testing (cargo-mutants / mutmut) |
| `cover` | Static coverage estimate and complexity hotspots |
| `gen-test` | Generate tests for hotspots (`--write` to create files) |
| `heal` | Bounded test → safe-fix → retest loop |
| `verify` | Anti-gaming verification of the working tree |
| `replay` | Replay the durable event log |
| `schema` | JSON schema of every command |
| `generate-skill` | Agent tool-definition document |
| `spec` | Write acceptance criteria spec (test-writing agent only) |
| `red` | Write a RED test from a spec |
| `verify` | Run RED-first verification with anti-gaming checks |
| `green` | Run GREEN verification (implementation vs RED tests) |
| `cover` | Identify coverage hotspots needing tests |
| `gen-test` | Generate tests for coverage hotspots |
| `heal` | Self-healing loop: generate → review → verify → fix |
| `replay` | Replay event-sourced orchestration log |
| `hermetic` | Run a command in isolated execution context |

## Output modes

- `--output human` (default) — tables and prose for humans
- `--output json` — structured envelope `{ok, result, error, meta}` for agents
- `--output jsonl` — one JSON object per line for streaming

## Examples

```bash
# JSON output for AI agents / CI
codeguard --output json test ~/my-project

# JSONL streaming
codeguard --output jsonl test ~/my-project

# Gated run — fail if any finding is at least "error" severity
codeguard --fail_on error test .

# Dry run — report what would change without modifying files
codeguard --dry_run fix .
```

## Platform support

| Platform | Install method | Binary |
|----------|---------------|--------|
| Linux (x86_64, aarch64) | `install.sh` | `codeguard` |
| macOS (x86_64, arm64) | `install.sh` | `codeguard` |
| Termux (Android, aarch64) | `install.sh` | `codeguard` |
| Windows (x86_64) | `install.ps1` | `codeguard.exe` |

## Build from source

```bash
git clone https://github.com/thisisforlearn/codesafe-better.git
cd codesafe
cargo build --release
cp target/release/codeguard ~/bin/
```

## Repository

https://github.com/thisisforlearn/codesafe-better
