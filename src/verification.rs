//! Verifiable Verification Protocol (Pillar 3).
//!
#![allow(clippy::too_many_arguments)]

//! This module implements adversarial verification gates that make gaming
//! the feedback loop structurally impossible:
//! - Asymmetric test/code separation: coding agent never sees acceptance criteria
//! - RED-first TDD: failing test must be independently verified before implementation
//! - Anti-gaming lints: deterministic checks against stub-passability, assertion gaming,
//!   scope violations, and suite-safety

use crate::types::{Language, Severity};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Acceptance criteria — only the test-writing agent sees this.
/// The coding agent only sees failure messages, never the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub file_allowlist: Vec<PathBuf>,
    pub language: Language,
    pub trace_id: String,
}

/// A test case derived from spec by the test-writing agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub id: String,
    pub spec_id: String,
    pub name: String,
    pub source: String,
    pub file: PathBuf,
    pub expected_to_fail: bool, // RED: must fail before implementation
    pub trace_id: String,
}

/// Result of running a verification gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    Fail,
}

/// Evidence bound to a specific git tree hash and oracle version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub git_tree_hash: String,
    pub oracle_version: String,
    pub verdict: Verdict,
    pub trace_id: String,
    pub timestamp_ms: u64,
}

/// Anti-gaming lint check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AntiGamingResult {
    /// No gaming detected
    Clean,
    /// Gaming detected with description
    GamingDetected {
        check: String,
        description: String,
        severity: Severity,
    },
}

/// Verification protocol state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationState {
    pub spec: Option<Spec>,
    pub red_tests: Vec<TestCase>,
    pub green_implementation_files: Vec<PathBuf>,
    pub receipts: Vec<Receipt>,
    pub current_phase: VerificationPhase,
}

/// Current phase in the RED-green cycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VerificationPhase {
    #[default]
    /// No spec loaded
    Idle,
    /// Spec loaded, waiting for RED test
    SpecLoaded,
    /// RED test written and verified failing
    RedVerified,
    /// Implementation written, waiting for GREEN verification
    GreenPending,
    /// GREEN verified - implementation passes RED tests
    GreenVerified,
}

/// Create a new spec from acceptance criteria.
pub fn create_spec(
    id: &str,
    description: &str,
    acceptance_criteria: Vec<String>,
    file_allowlist: Vec<PathBuf>,
    language: Language,
    trace_id: &str,
) -> Spec {
    Spec {
        id: id.to_string(),
        description: description.to_string(),
        acceptance_criteria,
        file_allowlist,
        language,
        trace_id: trace_id.to_string(),
    }
}

/// Write a RED test from spec. The test must be expected to fail.
pub fn write_red_test(
    spec: &Spec,
    test_name: &str,
    test_source: &str,
    test_file: PathBuf,
    trace_id: &str,
) -> TestCase {
    TestCase {
        id: format!("red-{}-{}", spec.id, test_name),
        spec_id: spec.id.clone(),
        name: test_name.to_string(),
        source: test_source.to_string(),
        file: test_file,
        expected_to_fail: true,
        trace_id: trace_id.to_string(),
    }
}

/// Anti-gaming lint 1: Stub-passability.
///
/// Rejects a RED test that mocks the unit under test. Importing the unit under
/// test is *required* for a real test, so it is explicitly allowed. Only actual
/// mocking/patching of that unit is treated as gaming.
pub fn check_stub_passability(test_source: &str, unit_under_test: &str) -> AntiGamingResult {
    let source = strip_strings_and_comments(test_source);
    let unit = unit_under_test.trim();
    let lower = source.to_lowercase();
    let unit_lower = unit.to_lowercase();

    let mut suspicious = Vec::new();
    // Rust mocking that targets the unit.
    if (lower.contains("mockall") || lower.contains("#[automock]"))
        && (unit.is_empty()
            || lower.contains(&unit_lower)
            || lower.contains("mock!")
            || lower.contains("automock"))
    {
        suspicious.push("mockall/automock");
    }
    if lower.contains("mock!") {
        suspicious.push("mock! macro");
    }
    // Python mocking that targets the unit.
    for pat in [
        "unittest.mock",
        "magicmock",
        "mock(",
        "mock.patch",
        "patch(",
        "@patch",
        "monkeypatch",
        "mocker.patch",
    ] {
        if lower.contains(pat) && (unit.is_empty() || lower.contains(&unit_lower)) {
            suspicious.push(pat);
            break;
        }
    }
    // Direct mock of a fully-qualified unit symbol.
    if !unit_lower.is_empty() {
        for pat in [
            format!("patch(\"{}\",", unit_lower),
            format!("patch('{}',", unit_lower),
            format!("setattr({}", unit_lower),
            format!("mock({}", unit_lower),
        ] {
            if lower.contains(&pat) {
                suspicious.push("direct patch of unit under test");
                break;
            }
        }
    }

    if let Some(pat) = suspicious.first() {
        return AntiGamingResult::GamingDetected {
            check: "stub-passability".to_string(),
            description: format!(
                "RED test appears to mock the unit under test ('{unit}') via '{pat}'. \
                 A RED test must exercise real code, not mocks."
            ),
            severity: Severity::Critical,
        };
    }

    // Tautological assertions make a test pass without testing behaviour.
    if let Some(desc) = tautology(&source) {
        return AntiGamingResult::GamingDetected {
            check: "tautology".to_string(),
            description: desc,
            severity: Severity::Critical,
        };
    }

    AntiGamingResult::Clean
}

fn tautology(source: &str) -> Option<String> {
    let compact: String = source
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    for pat in [
        "assert!(true)",
        "assert_eq!(1,1)",
        "assert_eq!(0,0)",
        "assert(true)",
        "assert_eq!(true,true)",
        "assertequal(1,1)",
        "asserttrue(true)",
        "assert(true)",
        "assert_true(true)",
        "expect(true).tobe(true)",
        "asserttrue",
        "assert!(true);",
    ] {
        if compact.contains(pat) {
            return Some(format!(
                "Test contains a tautological assertion ('{pat}') that always passes."
            ));
        }
    }
    if compact.contains("assert!(1==1)") || compact.contains("assert 1 == 1") {
        return Some("Test contains a tautological equality assertion.".to_string());
    }
    None
}

/// Remove string literals and comments so keyword checks do not fire on text.
fn strip_strings_and_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                let quote = c;
                out.push(' ');
                while let Some(n) = chars.next() {
                    if n == '\\' {
                        chars.next();
                    } else if n == quote {
                        break;
                    }
                }
            }
            '#' => {
                // Python comment, unless this is a Rust attribute (`#[...]` / `#![...]`).
                if matches!(chars.peek(), Some('[') | Some('!')) {
                    out.push('#');
                } else {
                    out.push(' ');
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Anti-gaming lint 1b: New skips/xfails.
///
/// Turning a failing test into a skipped test is one of the most common ways to
/// make a suite green without fixing behaviour. Adding skips is flagged; existing
/// skips are allowed so unrelated suites are not blocked.
pub fn check_test_skips(pre_red_source: &str, post_red_source: &str) -> AntiGamingResult {
    let pre = count_skips(pre_red_source);
    let post = count_skips(post_red_source);
    if post > pre {
        return AntiGamingResult::GamingDetected {
            check: "new-skip".to_string(),
            description: format!(
                "Test skips/xfails increased from {pre} to {post}. Skipping tests hides failures."
            ),
            severity: Severity::Critical,
        };
    }
    AntiGamingResult::Clean
}

fn count_skips(source: &str) -> usize {
    let cleaned = strip_strings_and_comments(source);
    let mut count = 0;
    for pat in [
        "pytest.mark.skip",
        "pytest.mark.xfail",
        "pytestmark",
        "pytest.skip(",
        "@unittest.skip",
        "unittest.skip(",
        "#[ignore]",
        "it.skip(",
        "test.skip(",
        "describe.skip(",
        "xit(",
        "xdescribe(",
        "xcontext(",
    ] {
        count += cleaned.matches(pat).count();
    }
    count
}

/// Anti-gaming lint 2: Test-integrity diff guard.
/// Compares pre-RED and post-RED test source to detect assertion gaming.
pub fn check_test_integrity(pre_red_source: &str, post_red_source: &str) -> AntiGamingResult {
    let pre_assertions = count_assertions(pre_red_source);
    let post_assertions = count_assertions(post_red_source);

    if post_assertions < pre_assertions {
        return AntiGamingResult::GamingDetected {
            check: "test-integrity".to_string(),
            description: format!(
                "Assertions reduced from {} to {} after RED. \
                 Weakening assertions is gaming.",
                pre_assertions, post_assertions
            ),
            severity: Severity::Critical,
        };
    }

    if post_red_source.trim().is_empty() && !pre_red_source.trim().is_empty() {
        return AntiGamingResult::GamingDetected {
            check: "test-integrity".to_string(),
            description: "Test body emptied after RED. This is gaming.".to_string(),
            severity: Severity::Critical,
        };
    }

    if let Some(desc) = tautology(&strip_strings_and_comments(post_red_source)) {
        return AntiGamingResult::GamingDetected {
            check: "tautology".to_string(),
            description: desc,
            severity: Severity::Critical,
        };
    }

    AntiGamingResult::Clean
}

fn count_assertions(source: &str) -> usize {
    let cleaned = strip_strings_and_comments(source);
    cleaned
        .match_indices("assert")
        .filter(|(idx, _)| {
            let after = &cleaned[idx + "assert".len()..];
            after.starts_with('!')
                || after.starts_with('_')
                || after.starts_with(' ')
                || after.starts_with('(')
        })
        .count()
        + cleaned.matches(".should").count()
        + cleaned.matches("EXPECT_").count()
        + cleaned.matches("ASSERT_").count()
        + cleaned.matches("REQUIRE(").count()
}

/// Anti-gaming lint 3: Scope-inverse.
/// Flags implementation writes outside the spec's file allowlist.
pub fn check_scope_inverse(changed_files: &[PathBuf], allowlist: &[PathBuf]) -> AntiGamingResult {
    // "." (or an empty entry) means the whole repository is in scope.
    if allowlist
        .iter()
        .any(|a| a.as_os_str().is_empty() || a == std::path::Path::new("."))
    {
        return AntiGamingResult::Clean;
    }
    for changed in changed_files {
        if !allowlist.iter().any(|a| changed.starts_with(a)) {
            return AntiGamingResult::GamingDetected {
                check: "scope-inverse".to_string(),
                description: format!(
                    "Implementation wrote to '{}' which is outside the spec's file allowlist.",
                    changed.display()
                ),
                severity: Severity::Critical,
            };
        }
    }

    AntiGamingResult::Clean
}

/// Anti-gaming lint 4: Suite-safety.
/// Flags when a scoped suite passes but the full suite fails.
/// If scoped tests pass but the full test suite fails, implementation may be
/// gaming the scoped tests.
pub fn check_suite_safety(scoped_passed: bool, full_passed: bool) -> AntiGamingResult {
    if scoped_passed && !full_passed {
        return AntiGamingResult::GamingDetected {
            check: "suite-safety".to_string(),
            description: "Scoped tests pass but full suite fails. \
             Implementation may be gaming scoped tests."
                .to_string(),
            severity: Severity::Critical,
        };
    }

    AntiGamingResult::Clean
}

/// Run all anti-gaming lints on a proposed implementation.
pub fn run_anti_gaming_lints(
    test_source: &str,
    unit_under_test: &str,
    pre_red_source: Option<&str>,
    post_red_source: &str,
    changed_files: &[PathBuf],
    allowlist: &[PathBuf],
    scoped_passed: bool,
    full_passed: bool,
) -> Vec<AntiGamingResult> {
    let mut results = Vec::new();

    // 1. Stub-passability
    results.push(check_stub_passability(test_source, unit_under_test));

    // 2. Test-integrity (if pre-RED source available)
    if let Some(pre) = pre_red_source {
        results.push(check_test_integrity(pre, post_red_source));
    }

    // 3. Scope-inverse
    results.push(check_scope_inverse(changed_files, allowlist));

    // 4. Suite-safety
    results.push(check_suite_safety(scoped_passed, full_passed));

    results
}

/// Public tautology-only check (used for repository-wide verification where
/// mocking libraries may legitimately be present).
pub fn check_tautological(source: &str) -> AntiGamingResult {
    match tautology(&strip_strings_and_comments(source)) {
        Some(description) => AntiGamingResult::GamingDetected {
            check: "tautology".to_string(),
            description,
            severity: Severity::Critical,
        },
        None => AntiGamingResult::Clean,
    }
}

/// Aggregate result of verifying a working tree.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationReport {
    pub checks: Vec<AntiGamingResult>,
    pub violations: usize,
    pub checked_files: Vec<String>,
    pub changed_files: Vec<String>,
    pub scope_ok: bool,
    pub passed: bool,
}

/// Verify the working tree: test-file integrity against HEAD, tautologies, and
/// (when a spec is supplied) write-scope. This is the real implementation behind
/// `codesafe verify`.
pub fn verify_repository(root: &std::path::Path, spec: Option<&Spec>) -> VerificationReport {
    let changed: Vec<PathBuf> = crate::git::changed_files(root)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !is_generated_path(p))
        .collect();
    let allowlist: Vec<PathBuf> = spec.map(|s| s.file_allowlist.clone()).unwrap_or_default();
    let mut checks = Vec::new();
    let mut checked_files = Vec::new();

    let scope_ok = if allowlist.is_empty() {
        true
    } else {
        matches!(
            check_scope_inverse(&changed, &allowlist),
            AntiGamingResult::Clean
        )
    };
    if !allowlist.is_empty() {
        checks.push(check_scope_inverse(&changed, &allowlist));
    }

    let root_canon = root.to_path_buf();
    for rel in &changed {
        if !is_test_file(rel) {
            continue;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        checked_files.push(rel_str.clone());
        let current = std::fs::read_to_string(root_canon.join(rel)).unwrap_or_default();
        if let Some(pre) = crate::git::file_at_revision(root, "HEAD", &rel_str).unwrap_or(None) {
            checks.push(check_test_integrity(&pre, &current));
            checks.push(check_test_skips(&pre, &current));
        } else {
            checks.push(check_tautological(&current));
        }
    }

    let violations = checks
        .iter()
        .filter(|c| matches!(c, AntiGamingResult::GamingDetected { .. }))
        .count();
    VerificationReport {
        checks,
        violations,
        checked_files,
        changed_files: changed
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
        scope_ok,
        passed: violations == 0,
    }
}

fn is_generated_path(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    text.starts_with(".codesafe/")
        || text.starts_with(".codeguard/")
        || text == "report.md"
        || text == "report.json"
        || text == "AGENT_TASK.md"
        || text.ends_with("/report.md")
        || text.ends_with("/report.json")
        || text.ends_with("/AGENT_TASK.md")
}

fn is_test_file(path: &std::path::Path) -> bool {
    if is_generated_path(path) {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !matches!(
        ext.as_str(),
        "py" | "rs" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx"
    ) {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    let in_tests_dir = path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("tests") | Some("test") | Some("__tests__")
        )
    });
    name.contains("test") || name.starts_with("spec") || in_tests_dir
}

/// Path of the persisted spec file for a repository.
pub fn spec_path(root: &std::path::Path) -> PathBuf {
    root.join(".codesafe").join("spec.json")
}

/// Persist a spec so later `verify`/`brief` calls can enforce scope.
pub fn save_spec(root: &std::path::Path, spec: &Spec) -> crate::Result<PathBuf> {
    let path = spec_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    }
    let json = serde_json::to_string_pretty(spec).map_err(crate::Error::Json)?;
    std::fs::write(&path, json).map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    Ok(path)
}

/// Load a persisted spec, if present.
pub fn load_spec(root: &std::path::Path) -> crate::Result<Option<Spec>> {
    let path = spec_path(root);
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    let spec = serde_json::from_str(&text).map_err(crate::Error::Json)?;
    Ok(Some(spec))
}

/// Create a receipt-bound verdict tied to git tree hash and oracle version.
pub fn create_receipt(
    git_tree_hash: &str,
    oracle_version: &str,
    verdict: Verdict,
    trace_id: &str,
) -> Receipt {
    Receipt {
        git_tree_hash: git_tree_hash.to_string(),
        oracle_version: oracle_version.to_string(),
        verdict,
        trace_id: trace_id.to_string(),
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_stub_passability_detects_mock() {
        let result = check_stub_passability("use mymodule::*;\nuse mockall::*;", "mymodule");
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_stub_passability_clean_for_real_test() {
        let result =
            check_stub_passability("fn test_foo() { assert_eq!(add(1, 2), 3); }", "mymodule");
        assert!(matches!(result, AntiGamingResult::Clean));
    }

    #[test]
    fn check_test_integrity_detects_weakened() {
        let result = check_test_integrity(
            "assert_eq!(foo(), 42);\nassert!(bar());",
            "assert!(foo() > 0);",
        );
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_scope_inverse_detects_outside_allowlist() {
        let result = check_scope_inverse(
            &[PathBuf::from("src/evil.rs")],
            &[PathBuf::from("src/good")],
        );
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_suite_safety_detects_scoped_pass_full_fail() {
        let result = check_suite_safety(true, false);
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_suite_safety_clean_when_both_pass() {
        let result = check_suite_safety(true, true);
        assert!(matches!(result, AntiGamingResult::Clean));
    }

    #[test]
    fn create_receipt_binds_verdict() {
        let receipt = create_receipt("abc123", "v1.0.0", Verdict::Pass, "cs-trace");
        assert_eq!(receipt.git_tree_hash, "abc123");
        assert_eq!(receipt.oracle_version, "v1.0.0");
        assert!(matches!(receipt.verdict, Verdict::Pass));
        assert_eq!(receipt.trace_id, "cs-trace");
    }

    #[test]
    fn importing_unit_under_test_is_not_gaming() {
        let src = "use mymodule::Widget;\n\n#[test]\nfn real() {\n    let w = Widget::new();\n    assert_eq!(w.size(), 3);\n}\n";
        assert!(matches!(
            check_stub_passability(src, "mymodule"),
            AntiGamingResult::Clean
        ));
    }

    #[test]
    fn mocking_unit_under_test_is_gaming() {
        let src = "#[automock]\nuse mockall::*;\n\n#[test]\nfn fake() { assert!(true); }\n";
        assert!(matches!(
            check_stub_passability(src, "mymodule"),
            AntiGamingResult::GamingDetected { .. }
        ));
    }

    #[test]
    fn tautological_assertions_are_detected() {
        let result = check_tautological("fn test() { assert!(true); }");
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
        let result = check_tautological("def test():\n    assert True\n");
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn assertions_in_comments_and_strings_are_not_counted() {
        let pre = "# assert_eq!(a, b)\nlet s = \"assert!(x)\";\n";
        let post = "let s = \"assert!(x)\";\n";
        assert!(matches!(
            check_test_integrity(pre, post),
            AntiGamingResult::Clean
        ));
    }

    #[test]
    fn new_skips_are_detected_but_existing_ones_are_allowed() {
        let pre = "#[test]\nfn t() { assert_eq!(add(1, 2), 3); }\n";
        let post = "#[ignore]\n#[test]\nfn t() { assert_eq!(add(1, 2), 3); }\n";
        assert!(matches!(
            check_test_skips(pre, post),
            AntiGamingResult::GamingDetected { .. }
        ));
        assert!(matches!(
            check_test_skips(post, post),
            AntiGamingResult::Clean
        ));
        let py = "def test_x():\n    pytest.skip('later')\n";
        assert!(matches!(
            check_test_skips("def test_x():\n    assert True\n", py),
            AntiGamingResult::GamingDetected { .. }
        ));
    }

    #[test]
    fn spec_persists_round_trip() {
        let dir = std::env::temp_dir().join(format!("codesafe-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = create_spec(
            "s1",
            "desc",
            vec!["works".into()],
            vec![PathBuf::from("src")],
            Language::Rust,
            "cs",
        );
        save_spec(&dir, &spec).unwrap();
        let loaded = load_spec(&dir).unwrap().unwrap();
        assert_eq!(loaded.id, "s1");
        assert_eq!(loaded.file_allowlist, vec![PathBuf::from("src")]);
    }

    #[test]
    fn verify_repository_without_git_is_clean() {
        let dir = std::env::temp_dir().join(format!("codesafe-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = verify_repository(&dir, None);
        assert!(report.passed);
        assert_eq!(report.violations, 0);
    }
}
