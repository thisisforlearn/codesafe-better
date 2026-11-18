//! Per-language adapters.
//!
//! Each language implements `LanguageAdapter`, orchestrating existing tools
//! (pytest, cargo, clang-tidy, ...) instead of reimplementing them.

use crate::Result;
use crate::types::{BenchmarkResult, Language, Rule, ToolResult};
use std::path::Path;

pub mod cpp;
pub mod python;
pub mod rust;

/// A ranked profiler hotspot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Hotspot {
    pub function: String,
    pub file: String,
    pub line: Option<u32>,
    pub percent_runtime: f64,
    pub samples: u64,
}

/// Result of applying one fix candidate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FixResult {
    pub rule_id: String,
    pub file: String,
    pub applied: bool,
    pub reverted: bool,
    pub reason: String,
}

/// Context passed to every adapter invocation.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub path: &'a std::path::Path,
    pub jobs: usize,
    pub timeout_s: Option<u64>,
    pub dry_run: bool,
    pub force: bool,
}

/// Every language plugin implements this trait.
pub trait LanguageAdapter: Send + Sync {
    fn language(&self) -> Language;
    /// True if this adapter applies to the given directory.
    fn detect(&self, path: &std::path::Path) -> Result<bool>;
    /// Build the project (compile). Returns skipped if no build system.
    fn build(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run the test suite.
    fn test(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run the linter.
    fn lint(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run a fuzzer briefly.
    fn fuzz(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run benchmarks and return measurements.
    fn bench(&self, ctx: Context<'_>) -> Result<Vec<BenchmarkResult>>;
    /// Profile and return ranked hotspots.
    fn profile(&self, ctx: Context<'_>) -> Result<Vec<Hotspot>>;
    /// Apply safe rules (filtered by the caller) and report what was applied.
    fn fix(&self, _ctx: Context<'_>, _rules: &[Rule]) -> Result<Vec<FixResult>> {
        Ok(vec![])
    }
}

/// Convenience aliases for collections used by the CLI.
pub type AdapterList = Vec<Box<dyn LanguageAdapter>>;
pub type ToolResults = Vec<ToolResult>;

/// Run a command, capturing output and timing. Never panics on failure;
/// a non-zero exit is reported in ToolResult. A timeout kills the process.
pub fn run_command(
    cwd: &Path,
    program: &str,
    args: &[&str],
    timeout_s: Option<u64>,
) -> Result<ToolResult> {
    let start = std::time::Instant::now();
    let mut cmd = std::process::Command::new(program);
    cmd.current_dir(cwd);
    // Cargo writes its target dir next to the manifest by default; keep it per-repo
    // (and off any noexec mount) unless the caller already set CARGO_TARGET_DIR.
    if program == "cargo" && std::env::var("CARGO_TARGET_DIR").is_err() {
        let target_dir = cwd.join(".codesafe").join("target");
        let _ = std::fs::create_dir_all(&target_dir);
        cmd.env("CARGO_TARGET_DIR", target_dir);
    }
    cmd.args(args);
    // Capture stdout/stderr so timeout kills still yield partial output.
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| crate::Error::CommandFailed {
        command: program.to_string(),
        exit: None,
        stderr: e.to_string(),
    })?;

    let output = match timeout_s {
        Some(secs) => {
            let deadline = start + std::time::Duration::from_secs(secs);
            loop {
                match child.try_wait().map_err(|e| crate::Error::CommandFailed {
                    command: program.to_string(),
                    exit: None,
                    stderr: e.to_string(),
                })? {
                    Some(_status) => {
                        break child.wait_with_output().map_err(|e| {
                            crate::Error::CommandFailed {
                                command: program.to_string(),
                                exit: None,
                                stderr: e.to_string(),
                            }
                        })?;
                    }
                    None => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            let output = child.wait_with_output().map_err(|e| {
                                crate::Error::CommandFailed {
                                    command: program.to_string(),
                                    exit: None,
                                    stderr: e.to_string(),
                                }
                            })?;
                            let elapsed = start.elapsed().as_millis() as u64;
                            return Ok(ToolResult {
                                tool: program.to_string(),
                                command: format!("{} {}", program, args.join(" ")),
                                exit_code: output.status.code(),
                                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                                stderr: format!(
                                    "timed out after {secs}s\n{}",
                                    String::from_utf8_lossy(&output.stderr)
                                ),
                                duration_ms: elapsed,
                                skipped: false,
                                skip_reason: None,
                            });
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
        None => child
            .wait_with_output()
            .map_err(|e| crate::Error::CommandFailed {
                command: program.to_string(),
                exit: None,
                stderr: e.to_string(),
            })?,
    };

    let duration = start.elapsed().as_millis() as u64;
    let exit_code = output.status.code();
    Ok(ToolResult {
        tool: program.to_string(),
        command: format!("{} {}", program, args.join(" ")),
        exit_code,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        duration_ms: duration,
        skipped: false,
        skip_reason: None,
    })
}

/// Run a command in `path` with `sh -c`, capturing output. Never panics on failure;
/// a timeout kills the process.
pub fn run_command_in_dir(
    path: &Path,
    command: &str,
    timeout_s: Option<u64>,
) -> Result<ToolResult> {
    let start = std::time::Instant::now();
    #[cfg(unix)]
    let mut cmd = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };
    cmd.current_dir(path);
    // Keep cargo build artifacts out of the repository (and out of fix commits),
    // and off noexec mounts, exactly like `run_command` does.
    if command.contains("cargo") && std::env::var("CARGO_TARGET_DIR").is_err() {
        let target_dir = path.join(".codesafe").join("target");
        let _ = std::fs::create_dir_all(&target_dir);
        cmd.env("CARGO_TARGET_DIR", target_dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| crate::Error::CommandFailed {
        command: command.to_string(),
        exit: None,
        stderr: e.to_string(),
    })?;

    let output = match timeout_s {
        Some(secs) => {
            let deadline = start + std::time::Duration::from_secs(secs);
            loop {
                match child.try_wait().map_err(|e| crate::Error::CommandFailed {
                    command: command.to_string(),
                    exit: None,
                    stderr: e.to_string(),
                })? {
                    Some(_status) => {
                        break child.wait_with_output().map_err(|e| {
                            crate::Error::CommandFailed {
                                command: command.to_string(),
                                exit: None,
                                stderr: e.to_string(),
                            }
                        })?;
                    }
                    None => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            break child.wait_with_output().map_err(|e| {
                                crate::Error::CommandFailed {
                                    command: command.to_string(),
                                    exit: None,
                                    stderr: e.to_string(),
                                }
                            })?;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
        None => child
            .wait_with_output()
            .map_err(|e| crate::Error::CommandFailed {
                command: command.to_string(),
                exit: None,
                stderr: e.to_string(),
            })?,
    };

    let duration = start.elapsed().as_millis() as u64;
    Ok(ToolResult {
        tool: "rule-action".into(),
        command: command.to_string(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        duration_ms: duration,
        skipped: false,
        skip_reason: None,
    })
}

/// Collect hotspots by sampling a test command with `perf`.
///
/// Best-effort: perf frequently needs privileges or kernel settings. Any failure
/// yields an empty hotspot list instead of an error, and the raw `perf.data` file
/// is removed afterwards so the workspace stays clean.
pub fn perf_hotspots(
    path: &Path,
    test_args: &[&str],
    timeout_s: Option<u64>,
) -> Result<Vec<Hotspot>> {
    if which::which("perf").is_err() {
        return Ok(vec![]);
    }
    let mut record_args = vec!["record", "-g", "--"];
    record_args.extend_from_slice(test_args);
    let record = run_command(path, "perf", &record_args, timeout_s)?;
    if !record.success() {
        let _ = std::fs::remove_file(path.join("perf.data"));
        return Ok(vec![]);
    }
    let report = run_command(
        path,
        "perf",
        &[
            "report",
            "--stdio",
            "--percent-limit",
            "1",
            "--sort",
            "symbol",
        ],
        timeout_s,
    )?;
    let _ = std::fs::remove_file(path.join("perf.data"));
    Ok(parse_perf_report(&report.stdout))
}

/// Parse `perf report --stdio --sort symbol` into ranked hotspots.
pub fn parse_perf_report(text: &str) -> Vec<Hotspot> {
    let mut totals: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let mut samples: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(first) = trimmed.split_whitespace().next() else {
            continue;
        };
        let Some(pct_str) = first.strip_suffix('%') else {
            continue;
        };
        let Ok(pct) = pct_str.parse::<f64>() else {
            continue;
        };
        if pct <= 0.0 {
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        let symbol = tokens
            .iter()
            .rev()
            .find(|t| !t.starts_with('[') && !t.starts_with('(') && **t != "|")
            .copied()
            .unwrap_or("unknown");
        if symbol == "symbol" || symbol == "Command" {
            continue;
        }
        // Strip offsets and argument lists from symbol names.
        let symbol = symbol
            .split('+')
            .next()
            .unwrap_or(symbol)
            .split('(')
            .next()
            .unwrap_or(symbol)
            .to_string();
        *totals.entry(symbol.clone()).or_insert(0.0) += pct;
        *samples.entry(symbol).or_insert(0) += 1;
    }
    let mut hotspots: Vec<Hotspot> = totals
        .into_iter()
        .map(|(function, percent)| Hotspot {
            samples: samples.get(&function).copied().unwrap_or(0),
            function,
            file: String::new(),
            line: None,
            percent_runtime: percent.min(100.0),
        })
        .collect();
    hotspots.sort_by(|a, b| {
        b.percent_runtime
            .partial_cmp(&a.percent_runtime)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hotspots
}

/// Parse py-spy `--format raw` (collapsed stacks) into hotspots by leaf frame.
pub fn parse_raw_profile(text: &str) -> Vec<Hotspot> {
    let mut counts: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut total = 0u64;
    for line in text.lines() {
        let mut parts = line.rsplitn(2, ' ');
        let Some(count) = parts.next().and_then(|c| c.parse::<u64>().ok()) else {
            continue;
        };
        let Some(stack) = parts.next() else {
            continue;
        };
        let leaf = stack.rsplit(';').next().unwrap_or(stack).to_string();
        let leaf = leaf.split('(').next().unwrap_or(&leaf).trim().to_string();
        if leaf.is_empty() {
            continue;
        }
        *counts.entry(leaf).or_insert(0) += count;
        total += count;
    }
    let mut hotspots: Vec<Hotspot> = counts
        .into_iter()
        .map(|(function, count)| Hotspot {
            percent_runtime: if total == 0 {
                0.0
            } else {
                count as f64 / total as f64 * 100.0
            },
            samples: count,
            function,
            file: String::new(),
            line: None,
        })
        .collect();
    hotspots.sort_by(|a, b| {
        b.percent_runtime
            .partial_cmp(&a.percent_runtime)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hotspots
}

/// Check a single tool is available on PATH; return a skipped ToolResult if not.
pub fn require_tool(tool: &str) -> Result<ToolResult> {
    match which::which(tool) {
        Ok(_) => Ok(ToolResult {
            tool: tool.to_string(),
            command: format!("which {tool}"),
            exit_code: Some(0),
            stdout: format!("{tool} found"),
            stderr: String::new(),
            duration_ms: 0,
            skipped: false,
            skip_reason: None,
        }),
        Err(_) => Ok(ToolResult {
            tool: tool.to_string(),
            command: format!("which {tool}"),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            skipped: true,
            skip_reason: Some(format!("{tool} not found on PATH")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_reports_non_zero_exit() {
        let res = run_command(Path::new("."), "sh", &["-c", "exit 3"], None).unwrap();
        assert_eq!(res.exit_code, Some(3));
        assert!(!res.success());
    }

    #[test]
    fn parses_perf_report() {
        let text = "  42.10%  fixture  [.] slow_sum\n  12.34%  fixture  [.] fast_sum\n  5.00%  [kernel]  [k] do_syscall_64\n";
        let hotspots = parse_perf_report(text);
        assert_eq!(hotspots[0].function, "slow_sum");
        assert!(hotspots[0].percent_runtime > 40.0);
        assert!(hotspots.iter().any(|h| h.function == "fast_sum"));
    }

    #[test]
    fn parses_raw_pyspy_profile() {
        let text = "app.py:main;app.py:slow_sum 30\napp.py:main;app.py:fast_sum 70\n";
        let hotspots = parse_raw_profile(text);
        assert_eq!(hotspots[0].function, "app.py:fast_sum");
        assert!(hotspots[0].percent_runtime > 60.0);
    }

    #[test]
    fn missing_tool_is_skipped_not_error() {
        let res = require_tool("definitely-not-a-real-codeguard-tool-xyz").unwrap();
        assert!(res.skipped);
        assert!(res.skip_reason.is_some());
        assert_eq!(res.exit_code, None);
    }
}
