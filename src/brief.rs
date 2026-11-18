//! Agent brief: a complete, LLM-ready task packet for a repository.
//!
//! A brief turns raw findings into actionable tasks. Each task carries the
//! repository folder, file, line, enclosing symbol, numbered code snippet,
//! evidence, a deterministic root-cause hypothesis, a step-by-step fix plan,
//! the safe automated option (when one exists), risks, and an explicit
//! definition of done with executable verification commands.

use crate::types::{Finding, FindingStatus, Language, Optimization, Severity};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const BRIEF_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    pub id: String,
    pub priority: usize,
    pub severity: Severity,
    pub category: String,
    pub language: Language,
    pub folder: String,
    pub file: String,
    pub line: Option<u32>,
    pub symbol: Option<String>,
    pub affected_area: Option<String>,
    pub symptom: String,
    pub evidence: String,
    pub code_snippet: String,
    pub root_cause_hypothesis: String,
    pub fix_plan: Vec<String>,
    pub safe_fix: Option<String>,
    pub why_safe: String,
    pub risks: Vec<String>,
    pub verification: Vec<String>,
    pub definition_of_done: Vec<String>,
    pub related_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefSummary {
    pub tasks: usize,
    pub blocking: usize,
    pub auto_fixable: usize,
    pub optimizations: usize,
    pub tests_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBrief {
    pub schema_version: String,
    pub generated_at_ms: u64,
    pub trace_id: String,
    pub repository: String,
    pub languages: Vec<String>,
    pub summary: BriefSummary,
    pub tasks: Vec<AgentTask>,
    pub optimizations: Vec<Optimization>,
    pub next_actions: Vec<String>,
    pub verification_plan: Vec<String>,
    pub policy: Vec<String>,
}

impl AgentBrief {
    /// Render a compact markdown brief for pasting into an agent prompt.
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# AGENT_TASK\n\n");
        out.push_str(&format!(
            "Repository: `{}`\n\nLanguages: {}\n\nSchema: {}\n\n",
            self.repository,
            if self.languages.is_empty() {
                "(none detected)".to_string()
            } else {
                self.languages.join(", ")
            },
            self.schema_version
        ));
        out.push_str("## Summary\n\n");
        out.push_str(&format!(
            "- tasks: {}\n- blocking: {}\n- auto-fixable: {}\n- optimizations: {}\n- tests passed: {}\n\n",
            self.summary.tasks,
            self.summary.blocking,
            self.summary.auto_fixable,
            self.summary.optimizations,
            self.summary.tests_passed
        ));
        out.push_str("## Policy\n\n");
        for p in &self.policy {
            out.push_str(&format!("- {p}\n"));
        }
        out.push('\n');
        out.push_str("## Tasks\n\n");
        if self.tasks.is_empty() {
            out.push_str("No outstanding tasks.\n\n");
        }
        for task in &self.tasks {
            out.push_str(&format!(
                "### {}. [{}] {} (`{}`)\n\n",
                task.priority, task.severity, task.symptom, task.category
            ));
            out.push_str(&format!(
                "- location: `{}` {} (folder `{}`)\n",
                task.file,
                task.line.map(|l| format!("line {l}")).unwrap_or_default(),
                task.folder
            ));
            if let Some(sym) = &task.symbol {
                out.push_str(&format!("- symbol: `{sym}`\n"));
            }
            if let Some(area) = &task.affected_area {
                out.push_str(&format!("- affected area: {area}\n"));
            }
            out.push_str(&format!(
                "- root cause hypothesis: {}\n",
                task.root_cause_hypothesis
            ));
            if let Some(fix) = &task.safe_fix {
                out.push_str(&format!("- safe automated fix: `{fix}`\n"));
            }
            out.push_str("- fix plan:\n");
            for step in &task.fix_plan {
                out.push_str(&format!("  - {step}\n"));
            }
            out.push_str("- verification:\n");
            for step in &task.verification {
                out.push_str(&format!("  - {step}\n"));
            }
            out.push_str("- definition of done:\n");
            for step in &task.definition_of_done {
                out.push_str(&format!("  - {step}\n"));
            }
            if !task.risks.is_empty() {
                out.push_str("- risks:\n");
                for r in &task.risks {
                    out.push_str(&format!("  - {r}\n"));
                }
            }
            if !task.code_snippet.is_empty() {
                out.push_str("\n```\n");
                out.push_str(&task.code_snippet);
                out.push_str("```\n");
            }
            out.push('\n');
        }
        out.push_str("## Optimizations\n\n");
        if self.optimizations.is_empty() {
            out.push_str("(none)\n\n");
        } else {
            for (i, o) in self.optimizations.iter().take(20).enumerate() {
                out.push_str(&format!(
                    "{}. **{}** — `{}:{}` ({})\n   - change: {}\n   - gain: {} | risk: {}\n   - verify: {}\n\n",
                    i + 1,
                    o.title,
                    o.file,
                    o.line.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
                    o.category,
                    o.suggested_change,
                    o.expected_gain,
                    o.risk,
                    o.how_to_verify
                ));
            }
        }
        out.push_str("## Next actions\n\n");
        for action in &self.next_actions {
            out.push_str(&format!("- `{action}`\n"));
        }
        out.push_str("\n## Verification plan\n\n");
        for step in &self.verification_plan {
            out.push_str(&format!("- `{step}`\n"));
        }
        out
    }
}

/// Build a brief from enriched findings plus optimization suggestions.
pub fn build(
    repository: impl Into<String>,
    languages: Vec<Language>,
    tests_passed: bool,
    findings: &[Finding],
    optimizations: Vec<Optimization>,
    trace_id: &str,
) -> AgentBrief {
    let mut tasks: Vec<AgentTask> = findings
        .iter()
        .filter(|f| f.status != FindingStatus::Fixed)
        .map(task_from_finding)
        .collect();

    tasks.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    // Keep the packet usable in a single LLM context window. The full report
    // still contains every finding.
    tasks.truncate(200);
    for (idx, task) in tasks.iter_mut().enumerate() {
        task.priority = idx + 1;
    }

    let blocking = tasks
        .iter()
        .filter(|t| t.severity >= Severity::Error)
        .count();
    let auto_fixable = tasks.iter().filter(|t| t.safe_fix.is_some()).count();
    let summary = BriefSummary {
        tasks: tasks.len(),
        blocking,
        auto_fixable,
        optimizations: optimizations.len(),
        tests_passed,
    };

    let mut next_actions = Vec::new();
    if auto_fixable > 0 {
        next_actions.push("codesafe fix --dry-run <repo>".to_string());
        next_actions.push("codesafe fix <repo>".to_string());
    }
    if !tasks.is_empty() {
        next_actions.push("codesafe verify <repo>".to_string());
    }
    next_actions.push("codesafe test --fail-on warning <repo>".to_string());
    next_actions.push("codesafe optimize <repo>".to_string());

    AgentBrief {
        schema_version: BRIEF_SCHEMA_VERSION.to_string(),
        generated_at_ms: now_ms(),
        trace_id: trace_id.to_string(),
        repository: repository.into(),
        languages: languages.iter().map(|l| l.to_string()).collect(),
        summary,
        tasks,
        optimizations,
        next_actions,
        verification_plan: vec![
            "codesafe test --fail-on warning <repo>".to_string(),
            "codesafe verify <repo>".to_string(),
            "codesafe fix --dry-run <repo>".to_string(),
            "codesafe brief <repo>  # regenerate after changes".to_string(),
        ],
        policy: vec![
            "Make the smallest diff that resolves the reported task.".to_string(),
            "Never weaken, skip, or delete tests to make the suite pass.".to_string(),
            "Do not edit files outside the task's folder unless the task requires it; explain why in the change.".to_string(),
            "Preserve public APIs and on-disk formats unless the task explicitly changes them.".to_string(),
            "Run the verification plan before declaring the task done.".to_string(),
            "Prefer the safe automated fix when one is listed; otherwise follow the fix plan.".to_string(),
        ],
    }
}

fn task_from_finding(f: &Finding) -> AgentTask {
    let mut fix_plan = Vec::new();
    let file = if f.file.is_empty() {
        "<unknown file>"
    } else {
        &f.file
    };
    let location = format!(
        "{file}{}",
        f.line.map(|l| format!(":{l}")).unwrap_or_default()
    );
    fix_plan.push(format!(
        "Open {location} (folder `{}`).",
        if f.folder.is_empty() { "." } else { &f.folder }
    ));
    if let Some(sym) = &f.symbol {
        fix_plan.push(format!(
            "Locate the enclosing `{sym}` and confirm the reported behaviour there."
        ));
    }
    if let Some(fix) = &f.fix {
        fix_plan.push(format!("Apply the safe automated fix: `{fix}`."));
    } else {
        fix_plan.push(format!("Apply the minimal change for: {}", f.description));
    }
    fix_plan
        .push("Check call sites/imports of the changed symbol for contract changes.".to_string());
    fix_plan
        .push("Run the verification commands and keep the change only if all pass.".to_string());

    let mut verification = vec![
        "codesafe test --fail-on warning <repo>".to_string(),
        "codesafe verify <repo>".to_string(),
    ];
    if let Some(tool) = &f.error_type {
        verification.push(format!(
            "rerun the failing tool for `{tool}` and confirm it is gone"
        ));
    }

    let definition_of_done = vec![
        format!(
            "The finding `{}` no longer appears in `codesafe report`.",
            f.id
        ),
        "The full test suite and build pass.".to_string(),
        "No tests were weakened or removed.".to_string(),
        "The diff contains only changes relevant to this task.".to_string(),
    ];

    let risks = vec![
        f.fix_risk.clone().unwrap_or_else(|| {
            "medium: manual change; verify with the full suite before keeping it".to_string()
        }),
        "Changing a shared symbol can affect callers; search for references first.".to_string(),
    ];

    let mut related_files = Vec::new();
    if !f.folder.is_empty() {
        related_files.push(f.folder.clone());
    }
    related_files.push(file.to_string());

    let why_safe = if f.fix.is_some() {
        "the automated action is a formatter/safe lint fix that is accepted only after build+tests pass".to_string()
    } else {
        "the plan constrains changes to the reported symbol and requires full-suite verification"
            .to_string()
    };

    AgentTask {
        id: f.id.clone(),
        priority: 0,
        severity: f.severity,
        category: f.category.clone(),
        language: f.language,
        folder: if f.folder.is_empty() {
            ".".to_string()
        } else {
            f.folder.clone()
        },
        file: file.to_string(),
        line: f.line,
        symbol: f.symbol.clone(),
        affected_area: f.affected_area.clone(),
        symptom: f.description.clone(),
        evidence: f.evidence.clone(),
        code_snippet: if f.snippet.is_empty() {
            f.context_lines.join("\n")
        } else {
            f.snippet.clone()
        },
        root_cause_hypothesis: root_cause(f),
        fix_plan,
        safe_fix: f.fix.clone(),
        why_safe,
        risks,
        verification,
        definition_of_done,
        related_files,
    }
}

fn root_cause(f: &Finding) -> String {
    let et = f.error_type.as_deref().unwrap_or("").to_lowercase();
    let cat = f.category.to_lowercase();
    if et.contains("syntax") || et.contains("indentation") {
        "the source does not parse at the reported location; a string, bracket, or indentation is unbalanced".to_string()
    } else if et.contains("import") || et.contains("module") || et.contains("unresolved") {
        "an import/module path is wrong or a dependency is missing".to_string()
    } else if et.contains("assert") || et.contains("testfailed") || et.contains("test") {
        "the implementation does not satisfy the asserted behaviour, or the test encodes an incorrect expectation".to_string()
    } else if et.contains("type") || cat.contains("type") {
        "the value produced at this location does not match the declared/expected type".to_string()
    } else if et.contains("borrow") || et.contains("lifetime") {
        "ownership or lifetime rules are violated in this function".to_string()
    } else if et.contains("unused") {
        "dead code or an unused binding remains after a refactor".to_string()
    } else if cat.contains("format") {
        "the file does not match the project's formatter configuration; behaviour is unaffected"
            .to_string()
    } else if cat.contains("security") {
        "unsafe input handling in this code path can turn data into code or leak privileges"
            .to_string()
    } else if cat.contains("lint") {
        "a lint rule identifies a risky or non-idiomatic pattern at this location".to_string()
    } else if cat.contains("build") {
        "the build fails because this translation unit does not compile".to_string()
    } else {
        "the reported diagnostic points at incorrect or unverified behaviour in this code path"
            .to_string()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write `.codesafe/agent-brief.json` and `AGENT_TASK.md`.
pub fn write(root: &Path, brief: &AgentBrief) -> crate::Result<()> {
    let dir = root.join(".codesafe");
    std::fs::create_dir_all(&dir).map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    let json = serde_json::to_string_pretty(brief).map_err(crate::Error::Json)?;
    std::fs::write(dir.join("agent-brief.json"), json)
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    std::fs::write(root.join("AGENT_TASK.md"), brief.render_markdown())
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FindingStatus;

    fn finding(id: &str, severity: Severity, file: &str, line: u32) -> Finding {
        let mut f = Finding::new(id, severity, Language::Rust, "lint", "unused variable");
        f.file = file.into();
        f.line = Some(line);
        f.folder = "src".into();
        f.symbol = Some("work".into());
        f.snippet = "> 10 | let x = 1;".into();
        f.error_type = Some("unused_variables".into());
        f.fix = Some("cargo clippy --fix".into());
        f
    }

    #[test]
    fn brief_sorts_by_severity_and_assigns_priorities() {
        let findings = vec![
            finding("w", Severity::Warning, "src/b.rs", 2),
            finding("e", Severity::Error, "src/a.rs", 9),
        ];
        let brief = build("repo", vec![Language::Rust], false, &findings, vec![], "cs");
        assert_eq!(brief.tasks[0].severity, Severity::Error);
        assert_eq!(brief.tasks[0].priority, 1);
        assert_eq!(brief.tasks[1].priority, 2);
        assert_eq!(brief.summary.blocking, 1);
        assert_eq!(brief.summary.auto_fixable, 2);
    }

    #[test]
    fn brief_includes_fix_plan_and_done_criteria() {
        let findings = vec![finding("x", Severity::Error, "src/a.rs", 9)];
        let brief = build("repo", vec![Language::Rust], false, &findings, vec![], "cs");
        let task = &brief.tasks[0];
        assert!(
            task.fix_plan
                .iter()
                .any(|s| s.contains("cargo clippy --fix"))
        );
        assert!(task.definition_of_done.iter().any(|s| s.contains("tests")));
        assert!(task.code_snippet.contains("let x"));
        assert!(
            task.root_cause_hypothesis.contains("dead code")
                || !task.root_cause_hypothesis.is_empty()
        );
    }

    #[test]
    fn markdown_has_required_sections() {
        let brief = build(".", vec![Language::Python], true, &[], vec![], "cs");
        let md = brief.render_markdown();
        assert!(md.contains("# AGENT_TASK"));
        assert!(md.contains("## Policy"));
        assert!(md.contains("## Verification plan"));
    }

    #[test]
    fn fixed_findings_are_excluded() {
        let mut f = finding("x", Severity::Error, "src/a.rs", 9);
        f.status = FindingStatus::Fixed;
        let brief = build(".", vec![], true, &[f], vec![], "cs");
        assert!(brief.tasks.is_empty());
    }
}
