//! Python adapter: pytest, ruff, mypy, bandit, hypothesis, pytest-benchmark, cProfile.

use crate::Result;
use crate::adapters::{
    Context, FixResult, Hotspot, LanguageAdapter, require_tool, run_command, run_command_in_dir,
};
use crate::types::{BenchmarkResult, Language, Rule, RuleSafety, ToolResult};
use std::path::Path;

pub struct Python;

impl LanguageAdapter for Python {
    fn language(&self) -> Language {
        Language::Python
    }

    fn detect(&self, path: &Path) -> Result<bool> {
        Ok(has_python_config(path) || has_python_files(path))
    }

    fn build(&self, ctx: Context<'_>) -> Result<ToolResult> {
        let Some(python) = python_executable() else {
            return Ok(skipped("python3/python"));
        };
        // Python "build" = bytecode compile / syntax check of all sources.
        run_command(
            ctx.path,
            python,
            &["-m", "compileall", "-q", ctx.path.to_str().unwrap_or(".")],
            ctx.timeout_s,
        )
    }

    fn test(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if require_tool("pytest")?.skipped {
            return Ok(skipped("pytest"));
        }
        let Some(python) = python_executable() else {
            return Ok(skipped("python3/python"));
        };
        run_command(ctx.path, python, &["-m", "pytest", "-q"], ctx.timeout_s)
    }

    fn lint(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if !require_tool("ruff")?.skipped {
            return run_command(
                ctx.path,
                "ruff",
                &["check", ctx.path.to_str().unwrap_or(".")],
                ctx.timeout_s,
            );
        }
        if !require_tool("mypy")?.skipped {
            return run_command(
                ctx.path,
                "mypy",
                &[ctx.path.to_str().unwrap_or(".")],
                ctx.timeout_s,
            );
        }
        if !require_tool("bandit")?.skipped {
            return run_command(
                ctx.path,
                "bandit",
                &["-q", ctx.path.to_str().unwrap_or(".")],
                ctx.timeout_s,
            );
        }
        Ok(skipped("ruff/mypy/bandit"))
    }

    fn fuzz(&self, ctx: Context<'_>) -> Result<ToolResult> {
        // Hypothesis property-based tests are exercised by pytest.
        if require_tool("pytest")?.skipped {
            return Ok(skipped("pytest (hypothesis)"));
        }
        let Some(python) = python_executable() else {
            return Ok(skipped("python3/python"));
        };
        run_command(
            ctx.path,
            python,
            &["-m", "pytest", "-q", "-k", "fuzz or property or hypothesis"],
            ctx.timeout_s,
        )
    }

    fn bench(&self, ctx: Context<'_>) -> Result<Vec<BenchmarkResult>> {
        bench_python(ctx)
    }

    fn profile(&self, ctx: Context<'_>) -> Result<Vec<Hotspot>> {
        profile_python(ctx)
    }

    fn fix(&self, ctx: Context<'_>, rules: &[Rule]) -> Result<Vec<FixResult>> {
        fix_python(ctx, rules)
    }
}

fn skipped(tool: &str) -> ToolResult {
    ToolResult {
        tool: tool.to_string(),
        command: format!("which {tool}"),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 0,
        skipped: true,
        skip_reason: Some(format!("{tool} not found on PATH")),
    }
}

/// Resolve a usable Python interpreter.
fn python_executable() -> Option<&'static str> {
    if which::which("python3").is_ok() {
        Some("python3")
    } else if which::which("python").is_ok() {
        Some("python")
    } else {
        None
    }
}

fn has_python_files(path: &Path) -> bool {
    crate::scan::has_files_with_ext(path, &["py"])
}

fn has_python_config(path: &Path) -> bool {
    [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
        "Pipfile",
    ]
    .iter()
    .any(|name| path.join(name).is_file())
}

fn find_files_by_ext(path: &Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    crate::scan::files_with_ext(path, exts)
}

/// Benchmarks via pytest-benchmark JSON output.
fn bench_python(ctx: Context<'_>) -> Result<Vec<BenchmarkResult>> {
    let Some(python) = python_executable() else {
        return Ok(vec![]);
    };
    let tmp = std::env::temp_dir().join(format!("codeguard-bench-{}.json", std::process::id()));
    let tmp_str = tmp
        .to_str()
        .map(|s| s.to_owned())
        .unwrap_or_else(|| "/tmp/codeguard-bench.json".to_string());
    let _ = run_command(
        ctx.path,
        python,
        &["-m", "pytest", "--benchmark-json", tmp_str.as_str(), "-q"],
        ctx.timeout_s,
    )?;
    let text = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    let data: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let benchmarks: Vec<serde_json::Value> = data
        .get("benchmarks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for b in benchmarks {
        let name = b
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let stats: serde_json::Map<String, serde_json::Value> = b
            .get("stats")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let median = stats.get("median").and_then(|v| v.as_f64());
        let unit = stats
            .get("unit")
            .and_then(|v| v.as_str())
            .unwrap_or("s")
            .to_string();
        out.push(BenchmarkResult {
            name,
            before: None,
            after: median,
            unit,
            change_pct: None,
        });
    }
    Ok(out)
}

/// Profile the test suite with py-spy when available.
///
/// py-spy does not require modifying the program and works for normal Python
/// processes. Raw collapsed stacks are aggregated by leaf frame.
fn profile_python(ctx: Context<'_>) -> Result<Vec<Hotspot>> {
    if require_tool("py-spy")?.skipped {
        return Ok(vec![]);
    }
    let Some(python) = python_executable() else {
        return Ok(vec![]);
    };
    let tmp = std::env::temp_dir().join(format!(
        "codesafe-profile-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let tmp_str = tmp.to_string_lossy().to_string();
    let _ = run_command(
        ctx.path,
        "py-spy",
        &[
            "record", "-f", "raw", "-o", &tmp_str, "--", python, "-m", "pytest", "-q",
        ],
        ctx.timeout_s,
    )?;
    let text = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    Ok(crate::adapters::parse_raw_profile(&text))
}

/// Safe fixes: apply each safe Python rule's action (e.g. ruff format / ruff check --fix).
fn fix_python(ctx: Context<'_>, rules: &[Rule]) -> Result<Vec<FixResult>> {
    let mut results = Vec::new();
    if require_tool("ruff")?.skipped {
        return Ok(results);
    }
    if find_files_by_ext(ctx.path, &["py"]).is_empty() {
        return Ok(results);
    }
    for rule in rules {
        if rule.language != Language::Python || rule.safety != RuleSafety::Safe {
            continue;
        }
        let action = rule.action.trim();
        if action.is_empty() {
            continue;
        }
        if ctx.dry_run {
            results.push(FixResult {
                rule_id: rule.id.clone(),
                file: "repository".into(),
                applied: false,
                reverted: false,
                reason: "dry run, no changes applied".into(),
            });
            continue;
        }
        let res = run_command_in_dir(ctx.path, action, ctx.timeout_s)?;
        results.push(FixResult {
            rule_id: rule.id.clone(),
            file: "repository".into(),
            applied: res.success(),
            reverted: false,
            reason: if res.success() {
                "rule action applied".into()
            } else {
                format!("action failed: {}", res.stderr.trim())
            },
        });
    }
    Ok(results)
}
