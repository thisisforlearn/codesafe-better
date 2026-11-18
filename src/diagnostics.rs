//! Structured diagnostics parser.
//!
//! Parses raw tool output (compiler errors, linter output, test failures)
//! into structured, localized findings with recovery hints. Never truncates
//! diagnostics; uses trace_id for full-context lookup.

use crate::types::Severity;
use serde::{Deserialize, Serialize};

/// A single parsed diagnostic from any tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDiagnostic {
    pub trace_id: String,
    pub tool: String,
    pub severity: Severity,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub error_type: String,
    pub message: String,
    pub context_lines: Vec<String>,
    pub suggestion: Option<String>,
    pub confidence: f64,
    pub is_retryable: bool,
}

impl ParsedDiagnostic {
    /// Convert to a `Finding` with recovery hint attached.
    pub fn to_finding(&self) -> crate::types::Finding {
        let confidence = self.confidence;
        let mut finding = crate::types::Finding::new(
            &format!("{}-{}", self.tool, self.trace_id),
            self.severity,
            match self.tool.as_str() {
                "python3" => crate::types::Language::Python,
                "ruff" => crate::types::Language::Python,
                "pytest" => crate::types::Language::Python,
                "cargo" => crate::types::Language::Rust,
                "rustc" => crate::types::Language::Rust,
                "clang++" | "g++" | "clang" | "gcc" => crate::types::Language::Cpp,
                _ => crate::types::Language::Python,
            },
            &self.error_type,
            &self.message,
        );
        finding.file = self.file.clone().unwrap_or_default();
        finding.line = self.line;
        finding.column = self.column;
        finding.error_type = Some(self.error_type.clone());
        finding.context_lines = self.context_lines.clone();
        finding.trace_id = Some(self.trace_id.clone());
        finding.recovery_hint = Some(crate::types::RecoveryHint {
            action: self.suggestion.clone().unwrap_or_default(),
            fix: self.suggestion.clone(),
            confidence,
        });
        finding
    }
}

/// Parse Python compiler output (SyntaxError, etc.) into structured diagnostics.
pub fn parse_python_compiler(stderr: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Python SyntaxError pattern:
    //   File "/path/to/file.py", line 6, in <module>
    //     print("\nChoose a
    //           ^
    // SyntaxError: unterminated string literal (detected at line 6)
    let lines: Vec<&str> = stderr.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("File \"") && trimmed.contains(", line ") {
            // Extract file and line
            let file = extract_python_file(trimmed);
            let line_num = extract_python_line(trimmed);

            // Collect context lines (the offending code + caret)
            let mut context = Vec::new();
            if i + 1 < lines.len() {
                context.push(lines[i + 1].to_string());
            }
            if i + 2 < lines.len() {
                context.push(lines[i + 2].to_string());
            }

            // Look for the actual error message on subsequent lines
            let mut error_type = "SyntaxError".to_string();
            let mut message = String::new();
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.starts_with("SyntaxError:")
                    || l.starts_with("IndentationError:")
                    || l.starts_with("TabError:")
                {
                    error_type = l.split(':').next().unwrap_or("Error").to_string();
                    message = l[l.find(':').map(|p| p + 2).unwrap_or(0)..]
                        .trim()
                        .to_string();
                    break;
                }
                j += 1;
            }

            if message.is_empty() && i + 3 < lines.len() {
                message = lines[i + 3].trim().to_string();
            }

            let file_display = file.as_deref().unwrap_or("<unknown>").to_string();
            let line_display = line_num.map_or("?".to_string(), |n| n.to_string());
            let error_type_clone = error_type.clone();

            diagnostics.push(ParsedDiagnostic {
                trace_id: trace_id.to_string(),
                tool: "python3".to_string(),
                severity: Severity::Error,
                file,
                line: line_num,
                column: None,
                error_type,
                message,
                context_lines: context,
                suggestion: Some(format!(
                    "fix the {error_type_clone} at {file_display}:{line_display}"
                )),
                confidence: 0.95,
                is_retryable: false,
            });

            i = j + 1;
        } else {
            i += 1;
        }
    }

    diagnostics
}

/// Parse ruff output into structured diagnostics.
pub fn parse_ruff(output: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Ruff output format:
    // /path/to/file.py:10:5: E501 Line too long (88 > 88 characters)
    // /path/to/file.py:15:1: F401 'os' imported but unused
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        // Match: file:line:col: CODE message
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() >= 4 {
            let file = parts[0].trim().to_string();
            let line_num: Option<u32> = parts[1].trim().parse().ok();
            let col: Option<u32> = parts[2].trim().parse().ok();
            let rest = parts[3].trim();

            // Extract rule code (e.g., E501, F401)
            let rule_end = rest.find(' ').unwrap_or(rest.len());
            let error_type = rest[..rule_end].to_string();
            let message = rest[rule_end..].trim().to_string();
            let file_clone = file.clone();

            diagnostics.push(ParsedDiagnostic {
                trace_id: trace_id.to_string(),
                tool: "ruff".to_string(),
                severity: Severity::Warning,
                file: Some(file),
                line: line_num,
                column: col,
                error_type,
                message,
                context_lines: vec![line.to_string()],
                suggestion: Some(format!("run `ruff check --fix {file_clone}`")),
                confidence: 0.9,
                is_retryable: true,
            });
        }
    }

    diagnostics
}

/// Parse pytest output into structured diagnostics.
pub fn parse_pytest(output: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Pytest output format:
    //   /path/to/test_file.py::test_function FAILED
    //   assert expected == actual
    //   E   AssertionError: ...
    for line in output.lines() {
        if line.contains("FAILED") || line.contains("ERROR") {
            // Extract test node id
            let parts: Vec<&str> = line.split("::").collect();
            if parts.len() >= 2 {
                let file = parts[0].trim().to_string();
                let test_name = parts[1].split_whitespace().next().unwrap_or("").to_string();
                let file_clone = file.clone();

                diagnostics.push(ParsedDiagnostic {
                    trace_id: trace_id.to_string(),
                    tool: "pytest".to_string(),
                    severity: Severity::Error,
                    file: Some(file),
                    line: None,
                    column: None,
                    error_type: if line.contains("ERROR") {
                        "CollectionError".to_string()
                    } else {
                        "TestFailed".to_string()
                    },
                    message: format!("test {test_name} failed"),
                    context_lines: vec![line.to_string()],
                    suggestion: Some(format!("inspect the failure in {file_clone}::{test_name}")),
                    confidence: 0.85,
                    is_retryable: true,
                });
            }
        }
    }

    diagnostics
}

/// Parse generic compiler output (gcc, clang, rustc) into structured diagnostics.
pub fn parse_generic_compiler(output: &str, tool: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Generic format: file:line:col: error: message
    //                file:line: warning: message
    //                C:\path\file.cpp:12: error: message
    for line in output.lines() {
        let trimmed_line = line.trim_start();
        if trimmed_line.starts_with('{') || trimmed_line.contains("\"reason\":\"compiler-message\"")
        {
            // Structured cargo JSON has its own parser; its escaped text must
            // not be mistaken for a plain compiler line.
            continue;
        }
        let lower = line.to_lowercase();
        if !(lower.contains("error:")
            || lower.contains("warning:")
            || lower.contains("fatal error:"))
        {
            continue;
        }
        let Some((file, line_num, col, rest)) = split_location(line) else {
            continue;
        };
        if !looks_like_source(&file) {
            continue;
        }
        let severity = if lower.contains("error") {
            Severity::Error
        } else {
            Severity::Warning
        };
        let error_type = rest
            .split_once(':')
            .map(|(kind, _)| kind.trim())
            .unwrap_or("diagnostic")
            .to_string();
        let message = rest
            .split_once(':')
            .map(|(_, msg)| msg.trim())
            .unwrap_or(rest)
            .to_string();
        diagnostics.push(ParsedDiagnostic {
            trace_id: trace_id.to_string(),
            tool: tool.to_string(),
            severity,
            file: Some(file),
            line: line_num,
            column: col,
            error_type,
            message,
            context_lines: vec![line.to_string()],
            suggestion: None,
            confidence: 0.8,
            is_retryable: false,
        });
    }

    diagnostics
}

/// Parse mypy output (`file.py:10: error: message [code]`).
pub fn parse_mypy(stdout: &str, stderr: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut out = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        if !line.contains(": error:") && !line.contains(": warning:") && !line.contains(": note:") {
            continue;
        }
        let Some((file, line_num, col, rest)) = split_location(line) else {
            continue;
        };
        if !file.ends_with(".py") {
            continue;
        }
        let (severity, rest) = if let Some(r) = rest.strip_prefix("error:") {
            (Severity::Error, r.trim())
        } else if let Some(r) = rest.strip_prefix("warning:") {
            (Severity::Warning, r.trim())
        } else if let Some(r) = rest.strip_prefix("note:") {
            (Severity::Info, r.trim())
        } else {
            (Severity::Warning, rest.trim())
        };
        let code = rest
            .rsplit_once('[')
            .and_then(|(_, tail)| tail.strip_suffix(']'))
            .unwrap_or("mypy");
        out.push(ParsedDiagnostic {
            trace_id: trace_id.to_string(),
            tool: "mypy".to_string(),
            severity,
            file: Some(file.clone()),
            line: line_num,
            column: col,
            error_type: code.to_string(),
            message: rest
                .rsplit_once(" [")
                .map(|(m, _)| m)
                .unwrap_or(rest)
                .trim()
                .to_string(),
            context_lines: vec![line.to_string()],
            suggestion: Some(format!("fix the type error in {file} and re-run mypy")),
            confidence: 0.92,
            is_retryable: true,
        });
    }
    out
}

/// Parse bandit output (`>> Issue: [B602:subprocess_popen_with_shell_equals_true] ...`).
pub fn parse_bandit(stdout: &str, stderr: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut out = Vec::new();
    let text = format!("{stdout}\n{stderr}");
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.trim().strip_prefix(">> Issue:") {
            let (message, code) = parse_bandit_issue(rest);
            let mut file = None;
            let mut line_num = None;
            for follow in lines.iter().skip(i + 1).take(8) {
                let f = follow.trim();
                if let Some(v) = f.strip_prefix("Location:") {
                    if let Some((fpath, lnum, _, _)) = split_location(v.trim()) {
                        file = Some(fpath);
                        line_num = lnum;
                    }
                }
            }
            out.push(ParsedDiagnostic {
                trace_id: trace_id.to_string(),
                tool: "bandit".to_string(),
                severity: Severity::Error,
                file,
                line: line_num,
                column: None,
                error_type: code,
                message,
                context_lines: vec![line.to_string()],
                suggestion: Some(
                    "review the security finding and fix the unsafe call, not the test".to_string(),
                ),
                confidence: 0.85,
                is_retryable: false,
            });
        }
        i += 1;
    }
    out
}

fn parse_bandit_issue(rest: &str) -> (String, String) {
    // [B602:subprocess_popen_with_shell_equals_true] subprocess call with shell=True
    if let Some(start) = rest.find('[') {
        if let Some(end_rel) = rest[start..].find(']') {
            let code = &rest[start + 1..start + end_rel];
            let code = code
                .split(':')
                .next()
                .unwrap_or("bandit")
                .trim()
                .to_string();
            let message = rest[start + end_rel + 1..].trim().to_string();
            return (message, code);
        }
    }
    (rest.trim().to_string(), "bandit".to_string())
}

/// Parse cargo/rustc JSON diagnostics emitted with `--message-format=json`.
pub fn parse_cargo_json(output: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut out = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let Some(diag) = value.get("diagnostic").or_else(|| value.get("message")) else {
            continue;
        };
        if diag.get("spans").is_none() && diag.get("level").is_none() {
            continue;
        }
        let severity = match diag
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("error")
        {
            "error" | "error: internal compiler error" => Severity::Error,
            "warning" => Severity::Warning,
            _ => Severity::Info,
        };
        let message = diag
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("cargo diagnostic")
            .to_string();
        let code = diag
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|v| v.as_str())
            .unwrap_or("rustc")
            .to_string();
        let mut file = None;
        let mut line_num = None;
        let mut column = None;
        if let Some(spans) = diag.get("spans").and_then(|v| v.as_array()) {
            if let Some(span) = spans
                .iter()
                .find(|s| {
                    s.get("is_primary")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                })
                .or_else(|| spans.first())
            {
                file = span
                    .get("file_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                line_num = span
                    .get("line_start")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                column = span
                    .get("column_start")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
            }
        }
        out.push(ParsedDiagnostic {
            trace_id: trace_id.to_string(),
            tool: "cargo".to_string(),
            severity,
            file,
            line: line_num,
            column,
            error_type: code,
            message,
            context_lines: vec![line.to_string()],
            suggestion: Some("apply the compiler suggestion, then rerun cargo test".to_string()),
            confidence: 0.95,
            is_retryable: true,
        });
    }
    out
}

/// Split `file:line[:col]: rest` while tolerating Windows drive letters.
fn split_location(line: &str) -> Option<(String, Option<u32>, Option<u32>, &str)> {
    let mut positions = Vec::new();
    for (idx, _) in line.match_indices(':') {
        positions.push(idx);
    }
    for (n, &pos) in positions.iter().enumerate() {
        let after = &line[pos + 1..];
        let token: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if token.is_empty() {
            continue;
        }
        let line_num: u32 = token.parse().ok()?;
        let mut consumed = token.len();
        let mut col = None;
        let remainder = &after[consumed..];
        if let Some(rest) = remainder.strip_prefix(':') {
            let col_token: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !col_token.is_empty() {
                col = col_token.parse().ok();
                consumed += 1 + col_token.len();
            }
        }
        let file = line[..pos].trim().to_string();
        let rest = after[consumed..].trim_start_matches(':').trim();
        if !file.is_empty() {
            let _ = n;
            return Some((file, Some(line_num), col, rest));
        }
    }
    None
}

fn looks_like_source(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "py" | "rs" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx"
    )
}

/// Attach recovery hints to parsed diagnostics using heuristics.
pub fn attach_recovery_hints(diagnostics: &mut [ParsedDiagnostic]) {
    for d in diagnostics.iter_mut() {
        if d.suggestion.is_none() {
            d.suggestion = Some(match d.tool.as_str() {
                "python3" => format!(
                    "review the {} at {}:{:?} and correct the source",
                    d.error_type,
                    d.file.as_deref().unwrap_or("<unknown>"),
                    d.line
                ),
                "ruff" => format!(
                    "run `ruff check --fix {}`",
                    d.file.as_deref().unwrap_or(".")
                ),
                "pytest" => format!(
                    "run `pytest {} -x` to see the full failure trace",
                    d.file.as_deref().unwrap_or(".")
                ),
                _ => "review the diagnostic and apply a manual fix".to_string(),
            });
        }
    }

    // Set confidence based on diagnostic quality (file+line precision)
    for d in diagnostics.iter_mut() {
        if d.file.is_some() && d.line.is_some() {
            d.confidence = d.confidence.max(0.9);
        } else if d.file.is_some() {
            d.confidence = d.confidence.max(0.6);
        } else {
            d.confidence = d.confidence.max(0.3);
        }
    }
}

fn extract_python_file(line: &str) -> Option<String> {
    // File "/path/to/file.py", line 6
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn extract_python_line(line: &str) -> Option<u32> {
    // , line 6,
    let marker = ", line ";
    let start = line.find(marker)? + marker.len();
    let end = line[start..].find(',')? + start;
    line[start..end].trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_python_syntax_error() {
        let stderr = r#"
  File "/home/user/test.py", line 6, in <module>
    print("\nChoose a
          ^
SyntaxError: unterminated string literal (detected at line 6)
"#;
        let diags = parse_python_compiler(stderr, "cs-test-1");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].tool, "python3");
        assert_eq!(diags[0].error_type, "SyntaxError");
        assert!(diags[0].message.contains("unterminated string literal"));
        assert_eq!(diags[0].file.as_deref(), Some("/home/user/test.py"));
        assert_eq!(diags[0].line, Some(6));
        assert!(!diags[0].context_lines.is_empty());
        assert_eq!(diags[0].trace_id, "cs-test-1");
    }

    #[test]
    fn parse_ruff_output() {
        let output = r#"
/home/user/src/main.py:10:5: E501 Line too long (88 > 88 characters)
/home/user/src/main.py:15:1: F401 'os' imported but unused
"#;
        let diags = parse_ruff(output, "cs-test-2");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].error_type, "E501");
        assert_eq!(diags[0].line, Some(10));
        assert_eq!(diags[0].column, Some(5));
        assert_eq!(diags[1].error_type, "F401");
        assert_eq!(diags[1].line, Some(15));
    }

    #[test]
    fn parse_pytest_failures() {
        let output = r#"
/home/user/test_foo.py::test_bar FAILED
/home/user/test_foo.py::test_baz ERROR
"#;
        let diags = parse_pytest(output, "cs-test-3");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].error_type, "TestFailed");
        assert!(diags[0].file.as_deref().unwrap().contains("test_foo.py"));
        assert_eq!(diags[1].error_type, "CollectionError");
    }

    #[test]
    fn attach_recovery_hints_fills_missing() {
        let mut diags = vec![ParsedDiagnostic {
            trace_id: "cs-test".to_string(),
            tool: "ruff".to_string(),
            severity: Severity::Warning,
            file: Some("src/main.py".to_string()),
            line: Some(10),
            column: Some(5),
            error_type: "E501".to_string(),
            message: "Line too long".to_string(),
            context_lines: vec![],
            suggestion: None,
            confidence: 0.5,
            is_retryable: true,
        }];
        attach_recovery_hints(&mut diags);
        assert!(diags[0].suggestion.is_some());
        assert!(
            diags[0]
                .suggestion
                .as_deref()
                .unwrap()
                .contains("ruff check --fix")
        );
    }

    #[test]
    fn parse_generic_handles_windows_paths() {
        let diags = parse_generic_compiler(
            r"C:\repo\src\main.cpp:12:3: error: expected ';'",
            "clang++",
            "cs-win",
        );
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file.as_deref(), Some(r"C:\repo\src\main.cpp"));
        assert_eq!(diags[0].line, Some(12));
        assert_eq!(diags[0].column, Some(3));
        assert_eq!(diags[0].error_type, "error");
        assert!(diags[0].message.contains("expected"));
    }

    #[test]
    fn parse_mypy_extracts_code() {
        let out = "src/app.py:7: error: Incompatible types in assignment [assignment]\n";
        let diags = parse_mypy(out, "", "cs-mypy");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].error_type, "assignment");
        assert_eq!(diags[0].line, Some(7));
        assert!(diags[0].message.contains("Incompatible types"));
    }

    #[test]
    fn parse_bandit_extracts_location() {
        let out = ">> Issue: [B602:subprocess_popen_with_shell_equals_true] shell=True\n   Location: src/run.py:9:4\n";
        let diags = parse_bandit(out, "", "cs-bandit");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].error_type, "B602");
        assert_eq!(diags[0].file.as_deref(), Some("src/run.py"));
        assert_eq!(diags[0].line, Some(9));
    }

    #[test]
    fn parse_cargo_json_extracts_span() {
        let line = r#"{"reason":"compiler-message","message":{"rendered":"...","message":"unused variable: `x`","code":{"code":"unused_variables"},"level":"warning","spans":[{"file_name":"src/main.rs","line_start":4,"column_start":9,"is_primary":true}]}}"#;
        let diags = parse_cargo_json(line, "cs-cargo");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].error_type, "unused_variables");
        assert_eq!(diags[0].file.as_deref(), Some("src/main.rs"));
        assert_eq!(diags[0].line, Some(4));
        assert_eq!(diags[0].column, Some(9));
    }
}
