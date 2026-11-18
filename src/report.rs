//! Report generation: report.md for humans, report.json for tools/AI.

use crate::{
    Result,
    aci::make_trace_id,
    diagnostics,
    types::{AiTip, Finding, FindingStatus, Language, Report, Severity},
};
use std::path::Path;

/// Write report.md and report.json into `path`.
pub fn write(path: &Path, report: &Report) -> Result<()> {
    let md = render_md(report);
    let json = serde_json::to_string_pretty(report).map_err(crate::Error::Json)?;
    std::fs::write(path.join("report.md"), md)
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    std::fs::write(path.join("report.json"), json)
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    Ok(())
}

pub fn render_md(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# codesafe report\n\n");
    out.push_str(&format!("Path: {}\n\n", report.path));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- tests passed: {}\n- findings: {}\n- benchmarks: {}\n- optimizations: {}\n- ai tips: {}\n\n",
        report.summary.tests_passed,
        report.summary.findings_total,
        report.summary.benchmarks_total,
        report.optimizations.len(),
        report.ai_tips.len()
    ));
    out.push_str("## Tool results\n\n");
    if report.tool_results.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str("| tool | status | duration_ms | skip_reason |\n");
        out.push_str("|------|--------|-------------|-------------|\n");
        for r in &report.tool_results {
            let status = if r.skipped {
                "skipped"
            } else if r.success() {
                "ok"
            } else {
                "failed"
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                r.tool,
                status,
                r.duration_ms,
                r.skip_reason.as_deref().unwrap_or("-")
            ));
        }
        out.push('\n');
    }
    out.push_str("## Findings\n\n");
    if report.findings.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str(
            "| id | severity | language | file | line | category | status | description |\n",
        );
        out.push_str(
            "|----|----------|----------|------|------|----------|--------|-------------|\n",
        );
        for f in &report.findings {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {:?} | {} | {} | {} |\n",
                f.id, f.severity, f.language, f.file, f.line, f.category, f.status, f.description
            ));
        }
        out.push('\n');
    }
    out.push_str("## Benchmarks\n\n");
    if report.benchmarks.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str("| name | before | after | unit | change_pct |\n");
        out.push_str("|------|--------|-------|------|------------|\n");
        for b in &report.benchmarks {
            out.push_str(&format!(
                "| {} | {:?} | {:?} | {} | {:?} |\n",
                b.name, b.before, b.after, b.unit, b.change_pct
            ));
        }
        out.push('\n');
    }
    out.push_str("## Optimization opportunities\n\n");
    if report.optimizations.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str("| id | category | file:line | symbol | suggested change | gain | risk |\n");
        out.push_str("|----|----------|-----------|--------|------------------|------|------|\n");
        for o in &report.optimizations {
            out.push_str(&format!(
                "| {} | {} | {}:{} | {} | {} | {} | {} |\n",
                o.id,
                o.category,
                o.file,
                o.line.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
                o.symbol.as_deref().unwrap_or("-"),
                o.suggested_change.replace('|', "\\|"),
                o.expected_gain.replace('|', "\\|"),
                o.risk
            ));
        }
        out.push('\n');
    }

    out.push_str("## AI tips\n\n");
    if report.ai_tips.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        for (i, tip) in report.ai_tips.iter().enumerate() {
            out.push_str(&format!("### Tip {}\n\n", i + 1));
            out.push_str(&format!("- file: {} (folder: {})\n", tip.file, tip.folder));
            out.push_str(&format!("- line: {:?}\n", tip.line));
            out.push_str(&format!("- symbol: {:?}\n", tip.symbol));
            out.push_str(&format!("- category: {}\n", tip.category));
            out.push_str(&format!("- change: {}\n", tip.concrete_change));
            out.push_str(&format!("- impact: {}\n", tip.impact));
            out.push_str(&format!("- risk: {}\n", tip.risk));
            out.push_str(&format!("- why safe: {}\n", tip.why_safe));
            out.push_str(&format!("- verify: {}\n", tip.how_to_verify));
            if !tip.affected_code.is_empty() {
                out.push_str("\n```\n");
                out.push_str(&tip.affected_code);
                out.push_str("```\n\n");
            } else {
                out.push('\n');
            }
        }
    }
    out
}

/// Convert a tool result into a reported finding.
/// Attempts to parse structured diagnostics from tool output first;
/// falls back to raw output if parsing fails.
pub fn finding_from_result(
    id: &str,
    language: Language,
    result: &crate::types::ToolResult,
    category: &str,
) -> Finding {
    let severity = match result.exit_code {
        Some(0) => Severity::Info,
        Some(1) => Severity::Warning,
        Some(_) => Severity::Error,
        None => Severity::Critical,
    };
    let trace_id = make_trace_id("finding", None);

    // Try to parse structured diagnostics from tool output
    let diagnostic_text = if !result.stderr.trim().is_empty() {
        result.stderr.trim().to_string()
    } else if !result.stdout.trim().is_empty() {
        result.stdout.trim().to_string()
    } else {
        "(no diagnostic output)".to_string()
    };

    // Try tool-specific parsing first
    let parsed = match result.tool.as_str() {
        "python3" => {
            let diags = diagnostics::parse_python_compiler(&result.stderr, &trace_id);
            if !diags.is_empty() {
                Some(diags[0].to_finding())
            } else {
                None
            }
        }
        "ruff" => {
            let mut diags = diagnostics::parse_ruff(&result.stderr, &trace_id);
            diags.extend(diagnostics::parse_ruff(&result.stdout, &trace_id));
            if !diags.is_empty() {
                Some(diags[0].to_finding())
            } else {
                None
            }
        }
        "pytest" => {
            let diags = diagnostics::parse_pytest(&result.stderr, &trace_id);
            if !diags.is_empty() {
                Some(diags[0].to_finding())
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(mut f) = parsed {
        f.id = id.to_string();
        f.language = language;
        f.category = category.to_string();
        return f;
    }

    // Fallback to raw output
    Finding {
        id: id.to_string(),
        severity,
        language,
        file: String::new(),
        line: None,
        column: None,
        category: category.to_string(),
        error_type: None,
        status: crate::types::FindingStatus::Reported,
        description: diagnostic_text,
        fix: None,
        evidence: if !result.stdout.trim().is_empty() {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        },
        context_lines: Vec::new(),
        trace_id: Some(trace_id),
        recovery_hint: Some(crate::types::RecoveryHint {
            action: "review the diagnostic and apply a manual fix".to_string(),
            fix: None,
            confidence: 0.3,
        }),
        folder: String::new(),
        symbol: None,
        snippet: String::new(),
        affected_area: None,
        impact: Some("the reported code path is unreliable until fixed".to_string()),
        fix_risk: Some("medium: manual change required".to_string()),
    }
}

/// Build AI-actionable tips from reported findings.
pub fn ai_tips_for(findings: &[Finding]) -> Vec<AiTip> {
    ai_tips_for_root(None, findings)
}

/// Build AI-actionable tips enriched with source context (folder, symbol,
/// affected code, impact and risk) so an agent can act without extra exploration.
pub fn ai_tips_for_root(root: Option<&Path>, findings: &[Finding]) -> Vec<AiTip> {
    findings
        .iter()
        .filter(|f| f.status != FindingStatus::Fixed)
        .map(|f| {
            let ctx = match (root, f.file.is_empty()) {
                (Some(root), false) => crate::context::analyze(root, &f.file, f.line),
                _ => crate::context::SourceContext::default(),
            };
            let file = if f.file.is_empty() { ctx.file.clone() } else { f.file.clone() };
            let line = f.line.or(ctx.line);
            let folder = if f.folder.is_empty() { ctx.folder.clone() } else { f.folder.clone() };
            let symbol = f.symbol.clone().or_else(|| ctx.symbol.clone());
            let affected_code = if !f.snippet.is_empty() {
                f.snippet.clone()
            } else if !ctx.snippet.is_empty() {
                ctx.snippet.clone()
            } else {
                f.context_lines.join("\n")
            };
            let concrete_change = f.fix.clone().unwrap_or_else(|| {
                if let Some(sym) = &symbol {
                    format!(
                        "Fix `{sym}` in {file}{}: {}. Change the smallest possible surface.",
                        line.map(|l| format!(":{l}")).unwrap_or_default(),
                        f.description
                    )
                } else {
                    format!("Fix the reported issue in {file}: {}", f.description)
                }
            });
            let why_safe = if f.fix.is_some() {
                "the suggested action is the project's own lint/formatter fix; \
                 it is output-preserving by construction and is verified by build+tests"
                    .to_string()
            } else {
                "no automatic fix is available; constrain the change to the reported symbol and re-run the full suite"
                    .to_string()
            };
            let verification_command = "codesafe test <path>".to_string();
            let how_to_verify = if file.is_empty() {
                "rerun the failing tool and `codesafe test <path>`; the finding should disappear or move to `fixed`"
                    .to_string()
            } else {
                format!(
                    "apply the change to {file}{}, then run `codesafe test <path>` and confirm tests still pass",
                    line.map(|l| format!(":{l}")).unwrap_or_default()
                )
            };
            let impact = f.impact.clone().or_else(|| ctx.affected_area.clone()).unwrap_or_else(|| {
                "behaviour in this path stays uncertified until the finding is resolved".to_string()
            });
            let risk = f.fix_risk.clone().unwrap_or_else(|| {
                if f.fix.is_some() {
                    "low: verified automatically".to_string()
                } else {
                    "medium: manual change, verify with the full test suite".to_string()
                }
            });
            AiTip {
                file,
                line,
                concrete_change,
                why_safe,
                how_to_verify,
                folder,
                symbol,
                affected_code,
                impact,
                risk,
                category: f.category.clone(),
                verification_command,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_contains_sections() {
        let report = Report::new(".");
        let md = render_md(&report);
        assert!(md.contains("# codesafe report"));
        assert!(md.contains("## Summary"));
    }
}
