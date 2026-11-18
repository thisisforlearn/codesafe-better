//! Command-line interface for codesafe.
//!
//! Every command supports --output (human|json|jsonl) and --schema for
//! agent-facing introspection. The generate-skill subcommand emits an
//! agent skill document for LLM tool-use.

use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

use crate::aci::OutputMode;
use crate::types::{Config, Severity};

/// codesafe - test, benchmark, profile and safely auto-fix Python, Rust and C++ repositories.
///
/// Epistemic infrastructure for agentic coding: independently certifies
/// whether agent work is correct, making gaming of the feedback loop
/// structurally impossible.
#[derive(Debug, Parser)]
#[command(name = "codesafe", version, author, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to a codesafe.toml (or codeguard.toml) config file.
    /// When omitted, the repository's config is auto-discovered.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Number of parallel jobs.
    #[arg(long, global = true, short = 'j')]
    pub jobs: Option<usize>,

    /// Timeout in seconds per tool invocation.
    #[arg(long, global = true)]
    pub timeout: Option<u64>,

    /// Fail with non-zero exit code if any finding has severity at least this level.
    #[arg(long, global = true)]
    pub fail_on: Option<String>,

    /// Do not modify any files; report only.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Allow running fix on a dirty git tree.
    #[arg(long, global = true)]
    pub force: bool,

    /// Output mode: human, json, or jsonl.
    #[arg(long, global = true, default_value = "human")]
    pub output: String,
}

#[derive(Debug, Parser)]
pub enum Command {
    /// Detect languages and build systems, list available tools.
    Scan {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Build and run tests, lint, sanitizers.
    Test {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Run benchmarks, profile, rank hotspots.
    Bench {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Apply safe rule-based fixes with verification.
    Fix {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Write report.md and report.json.
    Report {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Scan, test, bench, fix, report.
    All {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Initialize a codesafe.toml config and rules/ directory for a project.
    Init {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Write an acceptance criteria spec (test-writing agent only).
    Spec {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(value_name = "DESCRIPTION")]
        description: String,
        #[arg(value_name = "ALLOWLIST", value_delimiter = ',')]
        allowlist: Vec<PathBuf>,
    },
    /// Write a RED test from a spec (test-writing agent only).
    Red {
        #[arg(value_name = "SPEC_ID")]
        spec_id: String,
        #[arg(value_name = "TEST_NAME")]
        test_name: String,
        #[arg(value_name = "TEST_FILE")]
        test_file: PathBuf,
        #[arg(value_name = "TEST_SOURCE")]
        test_source: String,
    },
    /// Run RED-first verification protocol.
    Verify {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Run GREEN verification (implementation against RED tests).
    Green {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Analyze coverage and identify hotspots needing tests.
    Cover {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "3")]
        min_complexity: u32,
        #[arg(long, default_value = "50.0")]
        max_coverage_pct: f64,
    },
    /// Generate tests targeting coverage hotspots.
    GenTest {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "10")]
        max_tests: u32,
        /// Write generated test skeletons into the repository (default: report only).
        #[arg(long)]
        write: bool,
    },
    /// Run self-healing loop: generate → review → verify → fix.
    Heal {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "5")]
        max_retries: u32,
    },
    /// Generate an agent skill document for LLM tool-use.
    #[command(hide = true)]
    GenerateSkill {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Emit a JSON schema for all commands and flags (agent introspection).
    #[command(hide = true)]
    Schema,
    /// Replay the event-sourced orchestration log.
    Replay {
        #[arg(value_name = "PATH", default_value = ".codesafe/events.jsonl")]
        path: PathBuf,
    },
    /// Run mutation testing scoped to the current git diff (Rust/Python).
    Mutate {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Soft cap used when reporting; full tool behavior is unchanged.
        #[arg(long, default_value = "200")]
        max_mutants: u32,
    },
    /// Analyze code and emit concrete optimization suggestions.
    Optimize {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Maximum number of suggestions to emit.
        #[arg(long, default_value = "60")]
        max: usize,
    },
    /// Build a complete, LLM-ready task packet (agent brief) for this repo.
    Brief {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Write `.codesafe/agent-brief.json` and `AGENT_TASK.md`.
        #[arg(long, default_value_t = true)]
        write: bool,
    },
    /// Run a command in hermetic (isolated) execution context.
    Hermetic {
        #[arg(value_name = "COMMAND")]
        command: String,
        /// Arguments passed to the command. Use `--` before them.
        #[arg(value_name = "ARGS", num_args = 0.., trailing_var_arg = true)]
        args: Vec<String>,
        /// Working directory for the command.
        #[arg(long, value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
}

/// Resolved runtime configuration: config file merged with CLI overrides.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeConfig {
    pub jobs: usize,
    pub timeout_s: Option<u64>,
    pub fail_on: Option<Severity>,
    pub dry_run: bool,
    pub force: bool,
    pub bench_runs: u32,
    pub regression_threshold_pct: f64,
    pub rules_dir: Option<String>,
    pub config_path: PathBuf,
    pub output_mode: OutputMode,
}

impl RuntimeConfig {
    pub fn from_cli(
        cli: &Cli,
        config: Config,
        config_path: PathBuf,
        output_mode: OutputMode,
    ) -> crate::Result<Self> {
        config.validate()?;
        let fail_on = match &cli.fail_on {
            Some(raw) => Some(raw.parse::<Severity>().map_err(crate::Error::Config)?),
            None => config.fail_on(),
        };
        if let Some(jobs) = cli.jobs
            && jobs == 0
        {
            return Err(crate::Error::Config("jobs must be at least 1".into()));
        }
        if let Some(timeout) = cli.timeout
            && timeout == 0
        {
            return Err(crate::Error::Config(
                "timeout must be at least 1 second".into(),
            ));
        }
        Ok(Self {
            jobs: cli.jobs.unwrap_or(config.jobs()),
            timeout_s: cli.timeout.or(config.timeout_s()),
            fail_on,
            dry_run: cli.dry_run || config.dry_run.unwrap_or(false),
            force: cli.force || config.force.unwrap_or(false),
            bench_runs: config.bench_runs(),
            regression_threshold_pct: config.regression_threshold_pct(),
            rules_dir: config.rules_dir.clone(),
            config_path,
            output_mode,
        })
    }
}

/// JSON schema for all codesafe commands (agent introspection).
pub fn schema_json() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "codesafe CLI schema",
        "description": "Agent-facing schema for codesafe commands and flags.",
        "type": "object",
        "properties": {
            "config": { "type": "string", "description": "Path to codesafe.toml config file" },
            "jobs": { "type": "integer", "minimum": 1, "maximum": 64, "description": "Number of parallel jobs" },
            "timeout": { "type": "integer", "minimum": 1, "description": "Timeout in seconds per tool invocation" },
            "fail_on": { "type": "string", "enum": ["info", "warning", "error", "critical"], "description": "Fail if any finding has severity at least this level" },
            "dry_run": { "type": "boolean", "description": "Do not modify any files; report only" },
            "force": { "type": "boolean", "description": "Allow running fix on a dirty git tree" },
            "output": { "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode" }
        },
        "commands": {
            "scan": { "description": "Detect languages and build systems, list available tools", "args": { "path": { "type": "string", "default": "." } } },
            "test": { "description": "Build and run tests, lint, sanitizers", "args": { "path": { "type": "string", "default": "." } } },
            "bench": { "description": "Run benchmarks, profile, rank hotspots", "args": { "path": { "type": "string", "default": "." } } },
            "fix": { "description": "Apply safe rule-based fixes with verification", "args": { "path": { "type": "string", "default": "." } } },
            "report": { "description": "Write report.md and report.json", "args": { "path": { "type": "string", "default": "." } } },
            "all": { "description": "Scan, test, bench, fix, report", "args": { "path": { "type": "string", "default": "." } } },
            "init": { "description": "Initialize a codesafe.toml config and rules/ directory", "args": { "path": { "type": "string", "default": "." } } },
            "schema": { "description": "Emit this JSON schema", "args": {} },
            "generate-skill": { "description": "Generate an agent skill document", "args": { "path": { "type": "string", "default": "." } } },
            "spec": { "description": "Write an acceptance criteria spec (test-writing agent only)", "args": { "id": { "type": "string" }, "description": { "type": "string" }, "allowlist": { "type": "array", "items": { "type": "string" } } } },
            "red": { "description": "Write a RED test from a spec (test-writing agent only)", "args": { "spec_id": { "type": "string" }, "test_name": { "type": "string" }, "test_file": { "type": "string" }, "test_source": { "type": "string" } } },
            "verify": { "description": "Run RED-first verification protocol", "args": { "path": { "type": "string", "default": "." } } },
            "green": { "description": "Run GREEN verification (implementation against RED tests)", "args": { "path": { "type": "string", "default": "." } } },
            "cover": { "description": "Analyze coverage and identify hotspots needing tests", "args": { "path": { "type": "string", "default": "." }, "min_complexity": { "type": "integer", "default": 5 }, "max_coverage_pct": { "type": "number", "default": 50.0 } } },
            "gen-test": { "description": "Generate tests targeting coverage hotspots", "args": { "path": { "type": "string", "default": "." }, "max_tests": { "type": "integer", "default": 10 } } },
            "heal": { "description": "Run self-healing loop: generate → review → verify → fix", "args": { "path": { "type": "string", "default": "." }, "max_retries": { "type": "integer", "default": 5 } } },
            "replay": { "description": "Replay the event-sourced orchestration log", "args": { "path": { "type": "string", "default": ".codesafe/events.jsonl" } } },
            "mutate": { "description": "Run mutation testing scoped to the current git diff (cargo-mutants / mutmut)", "args": { "path": { "type": "string", "default": "." }, "max_mutants": { "type": "integer", "default": 200 } } },
            "optimize": { "description": "Emit concrete optimization suggestions with gain, risk and verification", "args": { "path": { "type": "string", "default": "." }, "max": { "type": "integer", "default": 60 } } },
            "brief": { "description": "Build an LLM-ready task packet (folder/file/line/symbol/snippet/fix plan/definition of done)", "args": { "path": { "type": "string", "default": "." }, "write": { "type": "boolean", "default": true } } },
            "hermetic": { "description": "Run a command in hermetic (isolated) execution context", "args": { "command": { "type": "string" }, "args": { "type": "array", "items": { "type": "string" } }, "path": { "type": "string", "default": "." } } },
        },
        "output_envelope": {
            "description": "Every json/jsonl response follows this envelope: { ok, result, error, meta }",
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "result": {},
                "error": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" },
                        "category": { "type": "string" },
                        "message": { "type": "string" },
                        "suggestion": { "type": "string" },
                        "is_retryable": { "type": "boolean" },
                        "trace_id": { "type": "string" }
                    }
                },
                "meta": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "path": { "type": "string" },
                        "timestamp_ms": { "type": "integer" },
                        "trace_id": { "type": "string" },
                        "output_mode": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// Agent skill document for LLM tool-use (e.g., tooli, OpenAI function calling).
pub fn skill_document(path: &str) -> String {
    format!(
        r#"{{
  "name": "codesafe",
  "description": "Test, benchmark, profile and safely auto-fix Python/Rust/C++ repositories. Epistemic infrastructure for agentic coding.",
  "version": "0.3.0",
  "commands": [
    {{
      "name": "codesafe scan",
      "description": "Detect languages, build systems, and available tools in a repository.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe test",
      "description": "Build and run tests, lint, sanitizers across all detected languages.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "fail_on": {{ "type": "string", "enum": ["info", "warning", "error", "critical"], "description": "Fail if any finding has severity at least this level" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe bench",
      "description": "Run benchmarks and detect regressions.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe fix",
      "description": "Apply safe rule-based fixes with verification (checkpoint → apply → verify → commit/revert).",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "force": {{ "type": "boolean", "description": "Allow running fix on a dirty git tree" }},
          "dry_run": {{ "type": "boolean", "description": "Report what would change without modifying files" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe report",
      "description": "Write report.md (human) and report.json (AI-actionable tips).",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe all",
      "description": "Run scan → test → bench → fix → report in one command.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe init",
      "description": "Initialize a codesafe.toml config and rules/ directory for a new project.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Project path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe spec",
      "description": "Write an acceptance criteria spec (test-writing agent only). Coding agent never sees this.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "id": {{ "type": "string", "description": "Spec identifier" }},
          "description": {{ "type": "string", "description": "Human-readable spec description" }},
          "allowlist": {{ "type": "array", "items": {{ "type": "string" }}, "description": "Files the coding agent is allowed to modify" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": ["id", "description"],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe red",
      "description": "Write a RED test from a spec (test-writing agent only). Test must fail before implementation.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "spec_id": {{ "type": "string", "description": "Identifier of the spec this test derives from" }},
          "test_name": {{ "type": "string", "description": "Name of the test function" }},
          "test_file": {{ "type": "string", "description": "Path to the test file" }},
          "test_source": {{ "type": "string", "description": "Full source code of the test" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": ["spec_id", "test_name", "test_file", "test_source"],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe verify",
      "description": "Run RED-first verification protocol with anti-gaming checks.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe green",
      "description": "Run GREEN verification - implementation against RED tests.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe cover",
      "description": "Analyze coverage and identify hotspots needing tests.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "min_complexity": {{ "type": "integer", "description": "Minimum cyclomatic complexity (default: 5)" }},
          "max_coverage_pct": {{ "type": "number", "description": "Max coverage % to be a hotspot (default: 50.0)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe gen-test",
      "description": "Generate tests targeting coverage hotspots.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "max_tests": {{ "type": "integer", "description": "Maximum tests to generate (default: 10)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe heal",
      "description": "Run self-healing loop: generate → review → verify → fix.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "max_retries": {{ "type": "integer", "description": "Maximum retry attempts per test (default: 5)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe replay",
      "description": "Replay the event-sourced orchestration log.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Path to the event log (default: .codesafe/events.jsonl)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe hermetic",
      "description": "Run a command in hermetic (isolated) execution context.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "command": {{ "type": "string", "description": "Command to execute" }},
          "args": {{ "type": "array", "items": {{ "type": "string" }}, "description": "Command arguments" }},
          "path": {{ "type": "string", "description": "Working directory (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": ["command"],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe mutate",
      "description": "Run mutation testing scoped to the current git diff and report surviving mutants as follow-up tasks.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe optimize",
      "description": "Emit concrete optimization suggestions with location, explanation, expected gain, risk and verification command.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "max": {{ "type": "integer", "description": "Maximum suggestions (default: 60)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe brief",
      "description": "Build a complete LLM-ready task packet: folder, file, line, symbol, snippet, evidence, root-cause hypothesis, fix plan, risks and definition of done.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "write": {{ "type": "boolean", "description": "Write .codesafe/agent-brief.json and AGENT_TASK.md (default: true)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }}
  ],
  "repository": "{path}",
  "capabilities": [
    "structured_output",
    "parsed_diagnostics",
    "recovery_hints",
    "anti_gaming_verification",
    "receipt_bound_verdicts",
    "event_sourced_state",
    "hermetic_execution"
  ]
}}"#
    )
}
