//! Test Generation & Self-Healing Loop (Pillar 4).
//!
//! Coverage-guided test generation targets hotspots (complex methods
//! with zero coverage). A self-healing loop with bounded retry budget
//! iterates: generate → review → verify → fix until green or budget
//! exhausted. Non-convergence is a valid outcome (non-zero exit,
//! named blockers).

use crate::types::Language;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Coverage data for a single function/method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageData {
    pub function: String,
    pub file: PathBuf,
    pub line: Option<u32>,
    pub complexity: u32, // cyclomatic complexity
    pub hits: u64,       // number of times executed during tests
    pub total_lines: u32,
    pub covered_lines: u32,
    /// True when this data came from static analysis (no coverage tool ran).
    #[serde(default)]
    pub estimated: bool,
}

/// A hotspot: complex method with insufficient test coverage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hotspot {
    pub function: String,
    pub file: PathBuf,
    pub line: Option<u32>,
    pub complexity: u32,
    pub coverage_pct: f64,
    pub priority: f64, // 0.0..1.0, higher = more urgent
}

/// A generated test case targeting a hotspot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedTest {
    pub id: String,
    pub hotspot: Hotspot,
    pub source: String,
    pub test_file: PathBuf,
    pub status: TestStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// Status of a generated test in the self-healing loop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TestStatus {
    Generated,
    Reviewed,
    Verified,
    FixNeeded,
    Failed,
    Exhausted,
}

/// Result of the self-healing loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealingResult {
    pub tests_generated: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub tests_exhausted: usize,
    pub total_attempts: u32,
    pub converged: bool,
    pub blocking_tests: Vec<String>,
    pub trace_id: String,
}

/// Static, dependency-free coverage estimator.
///
/// True line coverage needs a coverage tool. As a portable fallback this walks
/// every source file, extracts function bodies, computes a branch-based
/// complexity, and marks a function as covered only when its name appears in a
/// test file. Results are explicitly marked `estimated = true` so callers never
/// present them as measured coverage.
pub fn static_coverage(root: &std::path::Path) -> Vec<CoverageData> {
    let all = crate::scan::all_files(root);
    let mut test_corpus = String::new();
    for file in &all {
        let rel = crate::scan::relative_to(root, file);
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        let in_tests = rel
            .split('/')
            .any(|seg| matches!(seg, "tests" | "test" | "__tests__" | "spec" | "specs"));
        if (name.contains("test") || name.contains("spec") || in_tests)
            && crate::scan::language_of_path(file).is_some()
        {
            if let Ok(text) = std::fs::read_to_string(file) {
                test_corpus.push_str(&text.to_lowercase());
                test_corpus.push('\n');
            }
        }
    }

    let mut out = Vec::new();
    for file in &all {
        let Some(language) = crate::scan::language_of_path(file) else {
            continue;
        };
        let rel = crate::scan::relative_to(root, file);
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if name.contains("test") || name.contains("spec") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (start, func_name, end) in extract_functions(&lines, language) {
            let body = &lines[start..end.min(lines.len())];
            let complexity = 1 + branch_count(body, language);
            let referenced = test_corpus.contains(&func_name.to_lowercase());
            let total_lines = body.len() as u32;
            out.push(CoverageData {
                function: func_name,
                file: PathBuf::from(&rel),
                line: Some((start + 1) as u32),
                complexity,
                hits: if referenced { 1 } else { 0 },
                total_lines,
                covered_lines: if referenced { total_lines } else { 0 },
                estimated: true,
            });
        }
    }
    out
}

/// Extract `(start_line_index, name, end_line_index_exclusive)` for functions.
fn extract_functions(lines: &[&str], language: Language) -> Vec<(usize, String, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let t = raw.trim_start();
        if t.is_empty() || t.starts_with('#') || t.starts_with("//") || t.starts_with("/*") {
            i += 1;
            continue;
        }
        let candidate = match language {
            Language::Python => {
                let rest = t
                    .strip_prefix("async def ")
                    .or_else(|| t.strip_prefix("def "));
                rest.map(|r| {
                    let name: String = r
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    (name, indent_width(raw), false)
                })
            }
            Language::Rust => t.find("fn ").map(|pos| {
                let r = &t[pos + 3..];
                let name: String = r
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                (name, indent_width(raw), true)
            }),
            Language::Cpp => {
                if t.starts_with('#')
                    || t.starts_with("if ")
                    || t.starts_with("for ")
                    || t.starts_with("while ")
                    || t.starts_with("switch ")
                    || t.starts_with("return ")
                {
                    None
                } else if t.contains('(') && !t.ends_with(';') {
                    match t.find('(') {
                        Some(open) => {
                            let name = t[..open]
                                .rsplit(|c: char| c.is_whitespace() || c == '*' || c == '&')
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            Some((name, indent_width(raw), true))
                        }
                        None => None,
                    }
                } else {
                    None
                }
            }
        };
        let Some((name, base_indent, braced)) = candidate else {
            i += 1;
            continue;
        };
        if name.is_empty() || matches!(name.as_str(), "main" | "new" | "drop" | "fmt" | "default") {
            i += 1;
            continue;
        }
        let end = if braced {
            brace_block_end(lines, i)
        } else {
            python_block_end(lines, i, base_indent)
        };
        out.push((i, name, end.max(i + 1)));
        i = end.max(i + 1);
    }
    out
}

fn brace_block_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    let mut seen = false;
    for (idx, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
                seen = true;
            } else if ch == '}' {
                depth -= 1;
                if seen && depth <= 0 {
                    return idx + 1;
                }
            }
        }
        if idx - start > 500 {
            return idx + 1;
        }
    }
    lines.len()
}

fn python_block_end(lines: &[&str], start: usize, base_indent: i32) -> usize {
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if indent_width(line) <= base_indent {
            return idx;
        }
    }
    lines.len()
}

fn indent_width(line: &str) -> i32 {
    let mut n = 0;
    for c in line.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 4,
            _ => break,
        }
    }
    n
}

fn branch_count(body: &[&str], language: Language) -> u32 {
    let mut count = 0;
    for line in body {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('#') {
            continue;
        }
        let tokens: &[&str] = match language {
            Language::Python => &[
                "if ", "elif ", "for ", "while ", "except", " and ", " or ", "match ",
            ],
            Language::Rust => &[
                "if ", "else if", "for ", "while ", "match ", "&&", "||", "?",
            ],
            Language::Cpp => &[
                "if ", "else if", "for ", "while ", "case ", "catch", "&&", "||", "?",
            ],
        };
        for token in tokens {
            if t.contains(token) {
                count += 1;
            }
        }
    }
    count
}

/// Identify hotspots from coverage data: complex methods with low coverage.
pub fn identify_hotspots(
    coverage: &[CoverageData],
    min_complexity: u32,
    max_coverage_pct: f64,
) -> Vec<Hotspot> {
    let mut hotspots: Vec<Hotspot> = coverage
        .iter()
        .filter(|c| c.complexity >= min_complexity)
        .filter(|c| {
            let cov = if c.total_lines > 0 {
                c.covered_lines as f64 / c.total_lines as f64 * 100.0
            } else {
                0.0
            };
            cov < max_coverage_pct
        })
        .map(|c| {
            let coverage_pct = if c.total_lines > 0 {
                c.covered_lines as f64 / c.total_lines as f64 * 100.0
            } else {
                0.0
            };
            // Priority: high complexity + low coverage = high priority
            let priority = (c.complexity as f64 / 10.0).min(1.0) * (1.0 - coverage_pct / 100.0);
            Hotspot {
                function: c.function.clone(),
                file: c.file.clone(),
                line: c.line,
                complexity: c.complexity,
                coverage_pct,
                priority,
            }
        })
        .collect();

    // Sort by priority descending
    hotspots.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hotspots
}

/// Generate a test targeting a hotspot.
pub fn generate_test_for_hotspot(
    hotspot: &Hotspot,
    language: Language,
    trace_id: &str,
) -> GeneratedTest {
    // Skeletons are intentionally free of tautological assertions. They are
    // scaffolds for a human/agent to fill in, never passing tests by default.
    let test_source = match language {
        Language::Python => format!(
            "# Generated by codesafe for hotspot `{}` at {}:{:?} (complexity {}).\n# TODO: replace this scaffold with real cases.\n# Suggested shape:\n#   from <module> import {}\n#   def test_{}_handles_boundaries():\n#       ...\n",
            hotspot.function,
            hotspot.file.display(),
            hotspot.line,
            hotspot.complexity,
            hotspot.function,
            hotspot.function
        ),
        Language::Rust => format!(
            "// Generated by codesafe for hotspot `{}` at {}:{:?} (complexity {}).\n// TODO: add focused cases that exercise the real implementation.\n// Suggested shape:\n//   #[test]\n//   fn {}_handles_boundaries() {{ /* ... */ }}\n",
            hotspot.function,
            hotspot.file.display(),
            hotspot.line,
            hotspot.complexity,
            hotspot.function
        ),
        Language::Cpp => format!(
            "// Generated by codesafe for hotspot `{}` at {}:{:?} (complexity {}).\n// TODO: add focused cases that exercise the real implementation.\n// Suggested shape:\n//   TEST({}Test, HandlesBoundaries) {{ /* ... */ }}\n",
            hotspot.function,
            hotspot.file.display(),
            hotspot.line,
            hotspot.complexity,
            hotspot.function
        ),
    };

    let stem = hotspot
        .file
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let test_file = match language {
        Language::Python => std::path::PathBuf::from(format!("tests/test_{stem}_generated.py")),
        Language::Rust => std::path::PathBuf::from(format!("tests/{stem}_generated.rs")),
        Language::Cpp => std::path::PathBuf::from(format!("tests/{stem}_generated.cpp")),
    };

    GeneratedTest {
        id: format!("gen-{}-{}", trace_id, hotspot.function),
        hotspot: hotspot.clone(),
        source: test_source,
        test_file,
        status: TestStatus::Generated,
        attempts: 0,
        last_error: None,
    }
}

/// Self-healing loop with bounded retry budget.
/// Pipeline: generate → review → verify → fix.
/// Stops when converged (all tests pass) or budget exhausted.
pub fn self_healing_loop(
    mut tests: Vec<GeneratedTest>,
    max_retries: u32,
    verify_fn: impl Fn(&GeneratedTest) -> bool,
    fix_fn: impl Fn(&mut GeneratedTest) -> bool,
    trace_id: &str,
) -> HealingResult {
    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut tests_exhausted = 0;
    let mut total_attempts = 0u32;
    let mut blocking_tests: Vec<String> = Vec::new();

    for test in tests.iter_mut() {
        let mut attempts = 0;
        let mut converged = false;

        while attempts < max_retries && !converged {
            attempts += 1;
            total_attempts += 1;
            test.attempts = attempts;

            // Review: check if test looks reasonable
            test.status = TestStatus::Reviewed;

            // Verify: run the test
            if verify_fn(test) {
                test.status = TestStatus::Verified;
                tests_passed += 1;
                converged = true;
            } else {
                // Fix: try to fix the test
                if fix_fn(test) {
                    test.status = TestStatus::FixNeeded;
                    continue;
                } else {
                    test.status = TestStatus::Failed;
                    tests_failed += 1;
                    blocking_tests.push(test.id.clone());
                    break;
                }
            }
        }

        if !converged && test.status != TestStatus::Failed {
            test.status = TestStatus::Exhausted;
            tests_exhausted += 1;
            blocking_tests.push(test.id.clone());
        }
    }

    let converged = tests_failed == 0 && tests_exhausted == 0;

    HealingResult {
        tests_generated: tests.len(),
        tests_passed,
        tests_failed,
        tests_exhausted,
        total_attempts,
        converged,
        blocking_tests,
        trace_id: trace_id.to_string(),
    }
}

/// Compute coverage percentage from hits data.
pub fn compute_coverage_pct(hits: u64, total_lines: u32) -> f64 {
    if total_lines == 0 {
        0.0
    } else {
        (hits as f64 / total_lines as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_hotspots_returns_high_complexity_low_coverage() {
        let coverage = vec![
            CoverageData {
                function: "simple".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(10),
                complexity: 1,
                hits: 100,
                total_lines: 10,
                covered_lines: 10,
                estimated: false,
            },
            CoverageData {
                function: "complex".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(50),
                complexity: 15,
                hits: 0,
                total_lines: 20,
                covered_lines: 0,
                estimated: false,
            },
        ];
        let hotspots = identify_hotspots(&coverage, 5, 50.0);
        assert_eq!(hotspots.len(), 1);
        assert_eq!(hotspots[0].function, "complex");
        assert!(hotspots[0].priority > 0.5);
    }

    #[test]
    fn generate_test_for_hotspot_python() {
        let hotspot = Hotspot {
            function: "foo".to_string(),
            file: PathBuf::from("src/bar.py"),
            line: Some(42),
            complexity: 8,
            coverage_pct: 0.0,
            priority: 0.9,
        };
        let test = generate_test_for_hotspot(&hotspot, Language::Python, "cs-trace");
        assert!(test.source.contains("Generated by codesafe"));
        assert!(test.source.contains("foo"));
        assert!(test.test_file.starts_with("tests"));
        assert_eq!(test.status, TestStatus::Generated);
        assert_eq!(test.attempts, 0);
    }

    #[test]
    fn self_healing_loop_converges_when_all_pass() {
        let tests = vec![GeneratedTest {
            id: "t1".to_string(),
            hotspot: Hotspot {
                function: "f".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(1),
                complexity: 1,
                coverage_pct: 0.0,
                priority: 0.5,
            },
            source: "test".to_string(),
            test_file: PathBuf::from("test_lib.rs"),
            status: TestStatus::Generated,
            attempts: 0,
            last_error: None,
        }];

        let result = self_healing_loop(
            tests,
            3,
            |_| true, // all pass
            |_| false,
            "cs-trace",
        );

        assert!(result.converged);
        assert_eq!(result.tests_passed, 1);
        assert_eq!(result.tests_failed, 0);
        assert_eq!(result.blocking_tests.len(), 0);
    }

    #[test]
    fn self_healing_loop_exhausts_budget() {
        let tests = vec![GeneratedTest {
            id: "t1".to_string(),
            hotspot: Hotspot {
                function: "f".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(1),
                complexity: 1,
                coverage_pct: 0.0,
                priority: 0.5,
            },
            source: "test".to_string(),
            test_file: PathBuf::from("test_lib.rs"),
            status: TestStatus::Generated,
            attempts: 0,
            last_error: None,
        }];

        let result = self_healing_loop(
            tests,
            2,
            |_| false, // always fails
            |_| false, // cannot fix
            "cs-trace",
        );

        assert!(!result.converged);
        assert_eq!(result.tests_failed, 1);
        assert_eq!(result.total_attempts, 1);
        assert_eq!(result.blocking_tests.len(), 1);
    }

    #[test]
    fn static_coverage_finds_untested_complex_function() {
        let dir = std::env::temp_dir().join(format!("codesafe-static-cov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(
            dir.join("pkg/calc.py"),
            "def untested(x):\n    if x:\n        return 1\n    return 0\n\ndef covered(x):\n    return x\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tests/test_calc.py"),
            "from pkg.calc import covered\n\ndef test_covered():\n    assert covered(1) == 1\n",
        )
        .unwrap();
        let coverage = static_coverage(&dir);
        assert!(
            coverage
                .iter()
                .any(|c| c.function == "untested" && c.covered_lines == 0)
        );
        assert!(
            coverage
                .iter()
                .any(|c| c.function == "covered" && c.covered_lines > 0)
        );
        assert!(coverage.iter().all(|c| c.estimated));
        let hotspots = identify_hotspots(&coverage, 1, 50.0);
        assert!(hotspots.iter().any(|h| h.function == "untested"));
    }
}
