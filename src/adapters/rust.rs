//! Rust adapter: cargo test/fmt/clippy/audit, proptest, criterion, miri.

use crate::Result;
use crate::adapters::{
    Context, FixResult, Hotspot, LanguageAdapter, require_tool, run_command, run_command_in_dir,
};
use crate::types::{BenchmarkResult, Language, Rule, RuleSafety, ToolResult};
use std::path::Path;

pub struct Rust;

impl LanguageAdapter for Rust {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn detect(&self, path: &Path) -> Result<bool> {
        Ok(path.join("Cargo.toml").is_file() || has_rust_files(path))
    }

    fn build(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if require_tool("cargo")?.skipped {
            return Ok(skipped("cargo"));
        }
        run_command(ctx.path, "cargo", &["build"], ctx.timeout_s)
    }

    fn test(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if require_tool("cargo")?.skipped {
            return Ok(skipped("cargo"));
        }
        run_command(ctx.path, "cargo", &["test"], ctx.timeout_s)
    }

    fn lint(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if require_tool("cargo")?.skipped {
            return Ok(skipped("cargo"));
        }
        // Prefer clippy; fall back to fmt --check if clippy is missing.
        if !require_tool("clippy-driver")?.skipped {
            return run_command(
                ctx.path,
                "cargo",
                &["clippy", "--all-targets", "--message-format=json"],
                ctx.timeout_s,
            );
        }
        run_command(ctx.path, "cargo", &["fmt", "--check"], ctx.timeout_s)
    }

    fn fuzz(&self, ctx: Context<'_>) -> Result<ToolResult> {
        // proptest property tests run under `cargo test`; run them if proptest is a dependency.
        if require_tool("cargo")?.skipped || !has_proptest(ctx.path) {
            return Ok(skipped("cargo proptest"));
        }
        run_command(ctx.path, "cargo", &["test"], ctx.timeout_s)
    }

    fn bench(&self, ctx: Context<'_>) -> Result<Vec<BenchmarkResult>> {
        bench_rust(ctx)
    }

    fn profile(&self, ctx: Context<'_>) -> Result<Vec<Hotspot>> {
        profile_rust(ctx)
    }

    fn fix(&self, ctx: Context<'_>, rules: &[Rule]) -> Result<Vec<FixResult>> {
        fix_rust(ctx, rules)
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

fn has_rust_files(path: &Path) -> bool {
    crate::scan::has_files_with_ext(path, &["rs"])
}

fn find_files_by_ext(path: &Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    crate::scan::files_with_ext(path, exts)
}

fn has_proptest(path: &Path) -> bool {
    std::fs::read_to_string(path.join("Cargo.toml"))
        .map(|c| c.contains("proptest"))
        .unwrap_or(false)
}

/// Benchmarks via criterion (cargo bench). Parses criterion's `time:` summary lines.
fn bench_rust(ctx: Context<'_>) -> Result<Vec<BenchmarkResult>> {
    if require_tool("cargo")?.skipped {
        return Ok(vec![]);
    }
    let result = run_command(ctx.path, "cargo", &["bench"], ctx.timeout_s)?;
    let mut out = Vec::new();
    for line in result.stdout.lines() {
        if let Some(b) = parse_criterion_line(line) {
            out.push(b);
        }
    }
    Ok(out)
}

/// Criterion prints lines like:
///   `name::iterations time:   [123.45 us 124.10 us 124.80 us]`
fn parse_criterion_line(line: &str) -> Option<BenchmarkResult> {
    let (name, rest) = line.split_once(' ')?;
    if name.is_empty() || !rest.starts_with("time:") {
        return None;
    }
    let nums: Vec<f64> = rest
        .split_whitespace()
        .skip(1)
        .filter_map(|t| t.trim_end_matches(['[', ']', ',', ';']).parse::<f64>().ok())
        .collect();
    if nums.len() < 3 {
        return None;
    }
    let unit = rest
        .split_whitespace()
        .find(|t| matches!(*t, "ns" | "us" | "ms" | "s"))
        .unwrap_or("ms")
        .to_string();
    Some(BenchmarkResult {
        name: name.to_string(),
        before: None,
        after: Some(nums[1]),
        unit,
        change_pct: None,
    })
}

/// Profile via perf if available; otherwise no hotspots (clear note is reported elsewhere).
fn profile_rust(ctx: Context<'_>) -> Result<Vec<Hotspot>> {
    // Sampling the test suite gives representative hotspots. perf failures
    // (permissions/kernel settings) yield an empty list, not an error.
    crate::adapters::perf_hotspots(ctx.path, &["cargo", "test"], ctx.timeout_s)
}

/// Safe fixes: apply each safe Rust rule's action (e.g. cargo fmt / cargo clippy --fix).
fn fix_rust(ctx: Context<'_>, rules: &[Rule]) -> Result<Vec<FixResult>> {
    let mut results = Vec::new();
    if require_tool("cargo")?.skipped {
        return Ok(results);
    }
    if find_files_by_ext(ctx.path, &["rs"]).is_empty() {
        return Ok(results);
    }
    for rule in rules {
        if rule.language != Language::Rust || rule.safety != RuleSafety::Safe {
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
