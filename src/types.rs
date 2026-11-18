//! Shared data types for codeguard.

use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::Digest;

/// Detected programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    Rust,
    Cpp,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Python => write!(f, "python"),
            Language::Rust => write!(f, "rust"),
            Language::Cpp => write!(f, "cpp"),
        }
    }
}

/// Finding severity for CI gating (`--fail-on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}

/// Parse a severity from a CLI string, case-insensitive.
impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Severity::Info),
            "warning" | "warn" => Ok(Severity::Warning),
            "error" | "err" => Ok(Severity::Error),
            "critical" | "crit" => Ok(Severity::Critical),
            other => Err(format!("unknown severity: {other}")),
        }
    }
}

/// Whether a rule may be auto-applied without review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSafety {
    Safe,
    Review,
}

impl std::fmt::Display for RuleSafety {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSafety::Safe => write!(f, "safe"),
            RuleSafety::Review => write!(f, "review"),
        }
    }
}

/// Status of a finding after a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingStatus {
    Fixed,
    Reported,
}

impl std::fmt::Display for FindingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FindingStatus::Fixed => write!(f, "fixed"),
            FindingStatus::Reported => write!(f, "reported"),
        }
    }
}

/// Outcome of running one external tool once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub skipped: bool,
    pub skip_reason: Option<String>,
}

impl ToolResult {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// True for build/test tools, where a non-zero exit means the project is
    /// broken rather than that a style rule fired.
    pub fn is_build_or_test(&self) -> bool {
        let cmd = self.command.to_lowercase();
        cmd.contains("pytest")
            || cmd.contains("cargo test")
            || cmd.contains("cargo build")
            || cmd.contains("ctest")
            || cmd.contains("make test")
            || cmd.contains("nextest")
            || cmd.contains("compileall")
            || cmd.contains("cargo clippy")
    }

    /// Severity of this result, accounting for the tool's semantics.
    pub fn severity(&self) -> Severity {
        match self.exit_code {
            Some(0) => Severity::Info,
            Some(1) if self.is_build_or_test() => Severity::Error,
            Some(1) => Severity::Warning,
            Some(_) => Severity::Error,
            None => Severity::Critical,
        }
    }
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            tool: String::new(),
            command: String::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            skipped: true,
            skip_reason: Some("tool not found".into()),
        }
    }
}

/// One detected issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub language: Language,
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub category: String,
    pub error_type: Option<String>,
    pub status: FindingStatus,
    pub description: String,
    pub fix: Option<String>,
    pub evidence: String,
    pub context_lines: Vec<String>,
    pub trace_id: Option<String>,
    pub recovery_hint: Option<RecoveryHint>,
    /// Directory containing the affected file (relative to the repo root).
    #[serde(default)]
    pub folder: String,
    /// Enclosing function/class/module symbol, when it can be located.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Numbered source snippet around the finding, ready to paste into an LLM prompt.
    #[serde(default)]
    pub snippet: String,
    /// What the affected code is responsible for.
    #[serde(default)]
    pub affected_area: Option<String>,
    /// What breaks if the finding is left unfixed.
    #[serde(default)]
    pub impact: Option<String>,
    /// Risk of fixing it (to avoid "fixing" the code into a worse state).
    #[serde(default)]
    pub fix_risk: Option<String>,
}

/// Recovery hint attached to a finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryHint {
    pub action: String,
    pub fix: Option<String>,
    pub confidence: f64,
}

impl Finding {
    pub fn new(
        id: &str,
        severity: Severity,
        language: Language,
        category: &str,
        description: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            severity,
            language,
            file: String::new(),
            line: None,
            column: None,
            category: category.to_string(),
            error_type: None,
            status: FindingStatus::Reported,
            description: description.to_string(),
            fix: None,
            evidence: String::new(),
            context_lines: Vec::new(),
            trace_id: None,
            recovery_hint: None,
            folder: String::new(),
            symbol: None,
            snippet: String::new(),
            affected_area: None,
            impact: None,
            fix_risk: None,
        }
    }
}

/// One benchmark measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
    pub unit: String,
    pub change_pct: Option<f64>,
}

/// One AI-actionable tip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTip {
    pub file: String,
    pub line: Option<u32>,
    pub concrete_change: String,
    pub why_safe: String,
    pub how_to_verify: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub affected_code: String,
    #[serde(default)]
    pub impact: String,
    #[serde(default)]
    pub risk: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub verification_command: String,
}

/// One concrete, verifiable optimization suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Optimization {
    pub id: String,
    pub title: String,
    pub category: String,
    pub language: Language,
    pub file: String,
    #[serde(default)]
    pub folder: String,
    pub line: Option<u32>,
    #[serde(default)]
    pub symbol: Option<String>,
    pub evidence: String,
    #[serde(default)]
    pub snippet: String,
    pub explanation: String,
    pub suggested_change: String,
    pub expected_gain: String,
    pub risk: String,
    pub why_safe: String,
    pub how_to_apply: String,
    pub how_to_verify: String,
    /// 0.0..1.0; higher means higher expected value.
    pub score: f64,
}

/// One editable rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub language: Language,
    pub trigger: String,
    pub action: String,
    pub safety: RuleSafety,
    pub explanation: String,
    /// Optional glob filter relative to the repository root (e.g. `src/**/*.py`).
    #[serde(default)]
    pub globs: Vec<String>,
    /// Optional list of tools that must be on PATH before the action is applied.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Optional per-rule timeout override in seconds.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Optional category used in reports (format, lint, security, ...).
    #[serde(default)]
    pub category: Option<String>,
    /// Optional severity attached to findings produced by this rule.
    #[serde(default)]
    pub severity: Option<Severity>,
}

/// Aggregated summary of a run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Summary {
    pub language: Option<Language>,
    pub tools_found: usize,
    pub tools_missing: usize,
    pub tests_passed: bool,
    pub tests_total: u64,
    pub tests_failed: u64,
    pub findings_total: usize,
    pub findings_fixed: usize,
    pub benchmarks_total: usize,
    pub benchmarks_improved: usize,
    pub benchmarks_regressed: usize,
}

/// Full report emitted as report.md and report.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Report {
    pub path: String,
    pub summary: Summary,
    pub findings: Vec<Finding>,
    pub benchmarks: Vec<BenchmarkResult>,
    pub ai_tips: Vec<AiTip>,
    pub tool_results: Vec<ToolResult>,
    #[serde(default)]
    pub optimizations: Vec<Optimization>,
    /// Structured diagnostics parsed from tool output.
    #[serde(default)]
    pub parsed_diagnostics: Vec<crate::diagnostics::ParsedDiagnostic>,
}

impl Report {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }
}

/// Key/value config loaded from `codeguard.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub jobs: Option<usize>,
    pub timeout_s: Option<u64>,
    pub fail_on: Option<String>,
    pub dry_run: Option<bool>,
    pub force: Option<bool>,
    pub bench_runs: Option<u32>,
    pub regression_threshold_pct: Option<f64>,
    pub rules_dir: Option<String>,
}

impl Config {
    pub fn jobs(&self) -> usize {
        self.jobs.unwrap_or(1)
    }
    pub fn timeout_s(&self) -> Option<u64> {
        self.timeout_s
    }
    pub fn fail_on(&self) -> Option<Severity> {
        self.fail_on
            .as_ref()
            .and_then(|s| s.parse::<Severity>().ok())
    }
    pub fn bench_runs(&self) -> u32 {
        self.bench_runs.unwrap_or(3)
    }
    pub fn regression_threshold_pct(&self) -> f64 {
        self.regression_threshold_pct.unwrap_or(5.0)
    }

    /// Validate config values. Returns an error for nonsensical settings
    /// (zero jobs, zero timeout, etc.).
    pub fn validate(&self) -> Result<()> {
        if let Some(jobs) = self.jobs {
            if jobs == 0 {
                return Err(crate::Error::Config("jobs must be at least 1".into()));
            }
            if jobs > 64 {
                return Err(crate::Error::Config("jobs must not exceed 64".into()));
            }
        }
        if let Some(timeout) = self.timeout_s
            && timeout == 0
        {
            return Err(crate::Error::Config(
                "timeout_s must be at least 1 second".into(),
            ));
        }
        if let Some(fail_on) = &self.fail_on
            && fail_on.parse::<Severity>().is_err()
        {
            return Err(crate::Error::Config(format!(
                "invalid fail_on value {fail_on:?}: expected info|warning|error|critical"
            )));
        }
        if let Some(runs) = self.bench_runs
            && runs == 0
        {
            return Err(crate::Error::Config("bench_runs must be at least 1".into()));
        }
        if let Some(threshold) = self.regression_threshold_pct
            && (!threshold.is_finite() || threshold < 0.0)
        {
            return Err(crate::Error::Config(
                "regression_threshold_pct must be a finite non-negative number".into(),
            ));
        }
        Ok(())
    }
}

pub type FileHash = String;
pub type TraceId = String;

/// Per-file content hash used for caching.
pub fn hash_file(path: &std::path::Path) -> crate::Result<FileHash> {
    let bytes =
        std::fs::read(path).map_err(|e| crate::Error::Cache(format!("reading {path:?}: {e}")))?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// Hash a directory's relevant files into a single cache key.
///
/// Files are sorted first so the hash is stable across platforms and directory
/// iteration orders. Unreadable files contribute a marker instead of silently
/// producing an identical hash to an empty file.
pub fn hash_files(files: &[std::path::PathBuf]) -> crate::Result<FileHash> {
    let mut sorted: Vec<&std::path::PathBuf> = files.iter().collect();
    sorted.sort();
    let mut hasher = sha2::Sha256::new();
    for path in sorted {
        hasher.update(path.to_string_lossy().as_bytes());
        match std::fs::read(path) {
            Ok(bytes) => {
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            Err(e) => {
                hasher.update(b"<unreadable>");
                hasher.update(e.to_string().as_bytes());
            }
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        assert!(Severity::Critical > Severity::Error);
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn severity_from_str_case_insensitive() {
        assert_eq!("CRITICAL".parse::<Severity>(), Ok(Severity::Critical));
        assert_eq!("warn".parse::<Severity>(), Ok(Severity::Warning));
        assert!("bogus".parse::<Severity>().is_err());
    }

    #[test]
    fn tool_result_success() {
        let mut r = ToolResult::default();
        assert!(!r.success());
        r.exit_code = Some(0);
        assert!(r.success());
        r.exit_code = Some(1);
        assert!(!r.success());
    }

    #[test]
    fn hash_file_is_deterministic() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let h1 = hash_file(&dir.join("a.txt")).unwrap();
        let h2 = hash_file(&dir.join("a.txt")).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("codeguard-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn build_and_test_failures_are_errors_not_warnings() {
        let mut r = ToolResult {
            tool: "python3".into(),
            command: "python3 -m pytest -q".into(),
            exit_code: Some(1),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 1,
            skipped: false,
            skip_reason: None,
        };
        assert_eq!(r.severity(), Severity::Error);
        r.command = "cargo test".into();
        assert_eq!(r.severity(), Severity::Error);
        r.command = "ruff check .".into();
        assert_eq!(r.severity(), Severity::Warning);
        r.exit_code = Some(0);
        assert_eq!(r.severity(), Severity::Info);
        r.exit_code = None;
        assert_eq!(r.severity(), Severity::Critical);
    }
}
