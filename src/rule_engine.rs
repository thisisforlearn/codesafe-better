//! Rule engine: editable TOML rules and the verified safe-fix loop.
//!
//! Rules are stored in `rules/<language>.toml` as an array of rule tables:
//!
//! ```toml
//! [[rules]]
//! id = "py-ruff-format"
//! language = "python"
//! trigger = "ruff"
//! action = "ruff format ."
//! safety = "safe"
//! globs = ["**/*.py"]
//! requires = ["ruff"]
//! explanation = "Format Python sources with ruff (output-preserving)."
//! ```
//!
//! `trigger` is matched against tool output. It is either a substring pattern or,
//! when it contains a colon (`tool:code`), it also requires the result to come
//! from that tool and the code to appear in the output.
//!
//! Only `safety = "safe"` rules are auto-applied. Everything else is report-only.
//! When a repository has no rules directory, the built-in rule pack is used.

use crate::Result;
use crate::adapters::LanguageAdapter;
use crate::cli::RuntimeConfig;
use crate::git;
use crate::types::{Finding, FindingStatus, Language, Rule, RuleSafety, Severity, ToolResult};
use std::path::{Path, PathBuf};

/// Wrapper for TOML files that use [[rules]] arrays.
#[derive(Debug, Clone, serde::Deserialize)]
struct RulesFile {
    rules: Vec<Rule>,
}

/// Loaded rule set.
pub struct RuleEngine {
    rules: Vec<Rule>,
    builtin: bool,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            builtin: false,
        }
    }

    /// Load every `*.toml` rules file from `dir`. Missing directories yield an
    /// empty engine (callers that want a default pack use `load_with_builtin`).
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut rules = Vec::new();
        if dir.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
                .map_err(crate::Error::Io)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
                .collect();
            entries.sort();
            for path in entries {
                let text = std::fs::read_to_string(&path).map_err(crate::Error::Io)?;
                let parsed: RulesFile = toml::from_str(&text)
                    .map_err(|e| crate::Error::Rule(format!("{path:?}: {e}")))?;
                rules.extend(parsed.rules);
            }
        }
        Ok(Self::from_rules(rules))
    }

    /// Load project rules, falling back to the built-in pack when the directory
    /// is missing or contains no rules. This makes `codesafe fix` useful out of
    /// the box while still letting projects override everything.
    pub fn load_with_builtin(dir: &Path) -> Result<Self> {
        let mut engine = Self::load_from_dir(dir)?;
        if engine.rules.is_empty() {
            engine = Self::from_rules(crate::builtin_rules::default_rules());
            engine.builtin = true;
        }
        Ok(engine)
    }

    fn from_rules(rules: Vec<Rule>) -> Self {
        // Deduplicate by id, keeping the first occurrence (project overrides win
        // when files are sorted deterministically).
        let mut seen = std::collections::HashSet::new();
        let rules = rules
            .into_iter()
            .filter(|r| seen.insert(r.id.clone()))
            .collect();
        Self {
            rules,
            builtin: false,
        }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn is_builtin(&self) -> bool {
        self.builtin
    }

    pub fn safe_rules(&self) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.safety == RuleSafety::Safe)
            .collect()
    }

    pub fn review_rules(&self) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.safety == RuleSafety::Review)
            .collect()
    }

    pub fn for_language(&self, language: Language) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(move |r| r.language == language)
            .collect()
    }

    pub fn trigger_matches(trigger: &str, tool: &str, output: &str) -> bool {
        free_trigger_matches(trigger, tool, output)
    }
}

/// Does trigger match output from tool?
fn free_trigger_matches(trigger: &str, tool: &str, output: &str) -> bool {
    if let Some((t, code)) = trigger.split_once(':') {
        t == tool && output.contains(code)
    } else {
        output.contains(trigger)
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract file:line style location hints from tool output.
///
/// Handles Unix (`src/a.rs:10:5`) and Windows (`C:\src\a.rs:10:5`) paths by
/// scanning from the right for a numeric line segment.
fn scan_location(output: &str) -> Option<(String, u32)> {
    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 2 {
            continue;
        }
        for i in (1..parts.len()).rev() {
            let maybe_line = parts[i].split_whitespace().next().unwrap_or("");
            if let Ok(line_num) = maybe_line.parse::<u32>() {
                let file = parts[..i].join(":");
                let file = file.trim().to_string();
                if looks_like_file(&file) {
                    return Some((file, line_num));
                }
            }
        }
    }
    None
}

fn looks_like_file(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains(' ') {
        return false;
    }
    let ext = std::path::Path::new(text)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "py" | "rs" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx"
    )
}

/// Turn failed tool results into report-only findings, matched against rules.
///
/// This is the compatibility wrapper; prefer `findings_from_results_in` when a
/// repository root is available so findings can be enriched with context.
pub fn findings_from_results(
    engine: &RuleEngine,
    results: &[ToolResult],
    language: Language,
) -> Vec<Finding> {
    findings_from_results_in(None, engine, results, language)
}

/// Turn failed tool results into findings enriched with source context.
pub fn findings_from_results_in(
    root: Option<&Path>,
    engine: &RuleEngine,
    results: &[ToolResult],
    language: Language,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for result in results {
        if result.skipped {
            continue;
        }
        let output = format!("{}\n{}", result.stdout, result.stderr);
        let (file, line) = match scan_location(&output) {
            Some((f, l)) => (Some(f), Some(l)),
            None => (None, None),
        };
        let matched: Vec<&Rule> = engine
            .for_language(language)
            .into_iter()
            .filter(|r| free_trigger_matches(&r.trigger, &result.tool, &output))
            .collect();

        let trace = crate::aci::make_trace_id("finding", Some(&result.tool));
        let mut diagnostics = parse_diagnostics(result, &trace);
        crate::diagnostics::attach_recovery_hints(&mut diagnostics);

        if diagnostics.is_empty() && result.success() {
            // Successful lint runs with no diagnostics carry no findings.
            continue;
        }

        if !diagnostics.is_empty() {
            for d in diagnostics {
                let mut f = d.to_finding();
                f.language = language;
                f.id = format!("{}-{}", result.tool, sanitize_id(&d.error_type));
                f.category = matched
                    .first()
                    .and_then(|r| r.category.clone())
                    .unwrap_or_else(|| category_for_result(&result.tool, &result.command));
                f.status = FindingStatus::Reported;
                if let Some(rule) = matched.iter().find(|r| {
                    d.error_type.contains(&r.trigger) || r.trigger.contains(&d.error_type)
                }) {
                    f.fix = Some(rule.action.clone());
                }
                // Attach the language's safe autofix for this category even when
                // the diagnostic code did not match a trigger (e.g. every clippy
                // lint is machine-fixable by `cargo clippy --fix`).
                if f.fix.is_none() {
                    f.fix = engine
                        .for_language(language)
                        .into_iter()
                        .find(|r| {
                            r.safety == RuleSafety::Safe
                                && r.category.as_deref() == Some(f.category.as_str())
                        })
                        .map(|r| r.action.clone());
                }
                if let Some(root) = root {
                    crate::context::enrich_finding(root, &mut f);
                }
                findings.push(f);
            }
            continue;
        }

        if matched.is_empty() {
            let mut f = Finding::new(
                &format!("{}-failure", result.tool),
                result.severity(),
                language,
                &category_for_result(&result.tool, &result.command),
                &first_non_empty_line(&output),
            );
            f.file = file.unwrap_or_default();
            f.line = line;
            f.evidence = output.trim().to_string();
            if let Some(root) = root {
                crate::context::enrich_finding(root, &mut f);
            }
            findings.push(f);
            continue;
        }
        for rule in matched {
            let mut f = Finding::new(
                &rule.id,
                rule.severity.unwrap_or_else(|| result.severity()),
                language,
                rule.category.as_deref().unwrap_or("rule"),
                &rule.explanation,
            );
            f.file = file.clone().unwrap_or_default();
            f.line = line;
            f.fix = Some(rule.action.clone());
            f.evidence = output.trim().to_string();
            if let Some(root) = root {
                crate::context::enrich_finding(root, &mut f);
            }
            findings.push(f);
        }
    }
    dedupe_findings(findings)
}

/// Public wrapper used by the report/brief pipelines.
pub fn parsed_diagnostics_for(
    result: &ToolResult,
    trace_id: &str,
) -> Vec<crate::diagnostics::ParsedDiagnostic> {
    parse_diagnostics(result, trace_id)
}

fn parse_diagnostics(
    result: &ToolResult,
    trace_id: &str,
) -> Vec<crate::diagnostics::ParsedDiagnostic> {
    let mut parsed =
        crate::diagnostics::parse_generic_compiler(&result.stderr, &result.tool, trace_id);
    parsed.extend(crate::diagnostics::parse_generic_compiler(
        &result.stdout,
        &result.tool,
        trace_id,
    ));
    match result.tool.as_str() {
        "python3" | "python" => {
            parsed.extend(crate::diagnostics::parse_python_compiler(
                &result.stderr,
                trace_id,
            ));
            parsed.extend(crate::diagnostics::parse_pytest(&result.stdout, trace_id));
            parsed.extend(crate::diagnostics::parse_pytest(&result.stderr, trace_id));
        }
        "ruff" => {
            parsed.extend(crate::diagnostics::parse_ruff(&result.stderr, trace_id));
            parsed.extend(crate::diagnostics::parse_ruff(&result.stdout, trace_id));
        }
        "pytest" => {
            parsed.extend(crate::diagnostics::parse_pytest(&result.stderr, trace_id));
            parsed.extend(crate::diagnostics::parse_pytest(&result.stdout, trace_id));
        }
        "mypy" => parsed.extend(crate::diagnostics::parse_mypy(
            &result.stdout,
            &result.stderr,
            trace_id,
        )),
        "bandit" => parsed.extend(crate::diagnostics::parse_bandit(
            &result.stdout,
            &result.stderr,
            trace_id,
        )),
        "cargo" | "rustc" => {
            parsed.extend(crate::diagnostics::parse_cargo_json(
                &result.stdout,
                trace_id,
            ));
            parsed.extend(crate::diagnostics::parse_cargo_json(
                &result.stderr,
                trace_id,
            ));
        }
        "clang-format" => {
            for line in result.stderr.lines() {
                if let Some(files) = line.strip_prefix("files need formatting:") {
                    for file in files.split(',') {
                        let file = file.trim();
                        if file.is_empty() {
                            continue;
                        }
                        parsed.push(crate::diagnostics::ParsedDiagnostic {
                            trace_id: trace_id.to_string(),
                            tool: "clang-format".to_string(),
                            severity: Severity::Warning,
                            file: Some(file.to_string()),
                            line: None,
                            column: None,
                            error_type: "Formatting".to_string(),
                            message: "file is not clang-format clean".to_string(),
                            context_lines: vec![line.to_string()],
                            suggestion: Some(format!("run `clang-format -i {file}`")),
                            confidence: 0.9,
                            is_retryable: true,
                        });
                    }
                }
            }
        }
        _ => {}
    }
    dedupe_diagnostics(parsed)
}

fn dedupe_diagnostics(
    diagnostics: Vec<crate::diagnostics::ParsedDiagnostic>,
) -> Vec<crate::diagnostics::ParsedDiagnostic> {
    let mut seen = std::collections::HashSet::new();
    diagnostics
        .into_iter()
        .filter(|d| {
            seen.insert((
                d.file.clone(),
                d.line,
                d.column,
                d.error_type.clone(),
                d.message.clone(),
            ))
        })
        .collect()
}

fn dedupe_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut seen = std::collections::HashSet::new();
    findings
        .into_iter()
        .filter(|f| {
            seen.insert((
                f.id.clone(),
                f.file.clone(),
                f.line,
                f.column,
                f.description.clone(),
            ))
        })
        .collect()
}

fn sanitize_id(text: &str) -> String {
    let id: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = id.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "diagnostic".to_string()
    } else {
        trimmed
    }
}

fn category_for_result(tool: &str, command: &str) -> String {
    let cmd = command.to_lowercase();
    if cmd.contains("clippy") || cmd.contains("clang-tidy") || cmd.contains("cppcheck") {
        return "lint".to_string();
    }
    if cmd.contains("fmt") || cmd.contains("format") {
        return "format".to_string();
    }
    if cmd.contains("cargo build") || cmd.contains("compileall") || cmd.contains("cmake --build") {
        return "build".to_string();
    }
    if cmd.contains("cargo test") || cmd.contains("pytest") || cmd.contains("ctest") {
        return "tests".to_string();
    }
    match tool {
        "pytest" | "ctest" | "make" => "tests",
        "ruff" | "mypy" | "clippy-driver" | "clang-tidy" | "cppcheck" | "vulture" => "lint",
        "bandit" | "cargo-audit" | "cargo-deny" | "mutmut" | "cargo-mutants" => "security",
        "clang-format" | "rustfmt" | "black" | "isort" => "format",
        "python3" | "python" => "build",
        _ => "tool",
    }
    .to_string()
}

fn first_non_empty_line(output: &str) -> String {
    output
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(no output)")
        .trim()
        .to_string()
}

/// Outcome of applying one rule inside the safe-fix loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixVerdict {
    Kept,
    Reverted,
    NoOp,
    SkippedDryRun,
    SkippedMissingTool(String),
    SkippedNoFiles,
    /// A higher-priority rule in the same category already ran successfully.
    SkippedAlreadyCovered(String),
}

/// Run the verified safe-fix loop over every applicable adapter.
///
/// For each safe rule: apply the action, rebuild, rerun tests, compare output
/// and keep the change only if everything passes. Otherwise revert automatically
/// (git reset to the checkpoint, or restore a full workspace snapshot for
/// non-git repositories). One fix per commit.
pub fn safe_fix_loop(
    path: &Path,
    rt: &RuntimeConfig,
    engine: &RuleEngine,
    adapters: &mut [Box<dyn LanguageAdapter>],
) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();

    let is_git = git::is_git_repo(path);
    if !rt.dry_run && is_git && !rt.force && git::is_dirty(path)? {
        return Err(crate::Error::Git(
            "working tree is dirty; commit or stash changes, or use --force".into(),
        ));
    }
    // Dry runs must never create a commit or touch the working tree.
    let checkpoint_head = if is_git && !rt.dry_run {
        Some(git::checkpoint(path)?)
    } else {
        None
    };
    let mut snapshot = if is_git || rt.dry_run {
        None
    } else {
        Some(git::Snapshot::create(path)?)
    };

    let all_files = crate::scan::all_files(path);
    let mut baseline = run_adapter_tools(path, rt, adapters)?;

    let mut covered_categories: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    let applicable: Vec<(Language, usize)> = adapters
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            a.detect(path)
                .ok()
                .filter(|&d| d)
                .map(|_| (a.language(), i))
        })
        .collect();

    for (language, idx) in applicable {
        for rule in engine.safe_rules() {
            if rule.language != language {
                continue;
            }
            if !crate::scan::any_glob_matches(path, &rule.globs, &all_files) {
                findings.push(finding_from_verdict(
                    rule,
                    language,
                    FixVerdict::SkippedNoFiles,
                ));
                continue;
            }
            if let Some(missing) = missing_tool(&rule.requires) {
                findings.push(finding_from_verdict(
                    rule,
                    language,
                    FixVerdict::SkippedMissingTool(missing),
                ));
                continue;
            }
            // Formatters/import sorters in the same language conflict; run the
            // first available one and skip the rest to avoid style churn.
            let exclusive = matches!(rule.category.as_deref(), Some("format") | Some("imports"));
            let category_key = format!("{language}:{}", rule.category.as_deref().unwrap_or(""));
            if exclusive && covered_categories.contains(&category_key) {
                findings.push(finding_from_verdict(
                    rule,
                    language,
                    FixVerdict::SkippedAlreadyCovered(rule.category.clone().unwrap_or_default()),
                ));
                continue;
            }
            let verdict = if rt.dry_run {
                FixVerdict::SkippedDryRun
            } else {
                let adapter = &mut adapters[idx];
                apply_and_verify(
                    path,
                    rt,
                    checkpoint_head.as_deref(),
                    rule,
                    &mut **adapter,
                    &baseline,
                    snapshot.as_ref(),
                )?
            };
            match verdict {
                FixVerdict::Kept => {
                    if exclusive {
                        covered_categories.insert(category_key.clone());
                    }
                    baseline = run_adapter_tools(path, rt, adapters)?;
                    if !is_git {
                        // Refresh the snapshot so the next failure reverts to the
                        // post-fix state, not the original state.
                        snapshot = Some(git::Snapshot::create(path)?);
                    }
                }
                FixVerdict::Reverted => {
                    if let Some(head) = &checkpoint_head {
                        git::revert_to(path, head)?;
                    } else if let Some(snap) = &snapshot {
                        snap.restore()?;
                    }
                    baseline = run_adapter_tools(path, rt, adapters)?;
                }
                FixVerdict::NoOp
                | FixVerdict::SkippedDryRun
                | FixVerdict::SkippedMissingTool(_)
                | FixVerdict::SkippedNoFiles
                | FixVerdict::SkippedAlreadyCovered(_) => {}
            }
            findings.push(finding_from_verdict(rule, language, verdict));
        }
    }
    Ok(dedupe_findings(findings))
}

fn missing_tool(requires: &[String]) -> Option<String> {
    requires
        .iter()
        .find(|tool| which::which(tool).is_err())
        .cloned()
}

/// Run build/test/lint/fuzz across adapters and collect results.
fn run_adapter_tools(
    path: &Path,
    rt: &RuntimeConfig,
    adapters: &mut [Box<dyn LanguageAdapter>],
) -> Result<Vec<ToolResult>> {
    let mut results = Vec::new();
    for adapter in adapters.iter_mut() {
        if !adapter.detect(path)? {
            continue;
        }
        let ctx = crate::adapters::Context {
            path,
            jobs: rt.jobs,
            timeout_s: rt.timeout_s,
            dry_run: rt.dry_run,
            force: rt.force,
        };
        results.push(adapter.build(ctx)?);
        results.push(adapter.test(ctx)?);
        results.push(adapter.lint(ctx)?);
        results.push(adapter.fuzz(ctx)?);
    }
    Ok(results)
}

fn is_test_like(result: &ToolResult) -> bool {
    let cmd = result.command.to_lowercase();
    cmd.contains("pytest")
        || cmd.contains("cargo test")
        || cmd.contains("ctest")
        || cmd.contains("make test")
        || cmd.contains("nextest")
}

fn baseline_test_output(baseline: &[ToolResult], tool: &str) -> Option<String> {
    baseline
        .iter()
        .find(|r| r.tool == tool && is_test_like(r))
        .or_else(|| baseline.iter().find(|r| is_test_like(r)))
        .map(|r| normalize_test_output(&r.stdout))
}

/// Strip variable timing/count info from test output so reruns compare stably.
fn normalize_test_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let l = line.to_lowercase();
            !l.contains("total test time")
                && !l.contains("test time")
                && !l.contains("tests passed")
                && !l.contains("tests failed")
                && !l.contains("test result:")
                && !l.contains("running ")
                && !l.contains("elapsed:")
                && !l.contains("duration:")
                && !l.contains("finished in")
        })
        .map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() >= 2 {
                let last = tokens[tokens.len() - 1];
                let second_last = tokens[tokens.len() - 2];
                let looks_like_time = last.contains("sec")
                    || last.ends_with("ms")
                    || last.ends_with("us")
                    || (last.len() > 1
                        && last.ends_with('s')
                        && second_last.chars().all(|c| c.is_ascii_digit() || c == '.'));
                if looks_like_time {
                    return tokens[..tokens.len() - 2].join(" ");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_and_verify(
    path: &Path,
    rt: &RuntimeConfig,
    checkpoint_head: Option<&str>,
    rule: &Rule,
    adapter: &mut dyn LanguageAdapter,
    baseline: &[ToolResult],
    snapshot: Option<&git::Snapshot>,
) -> Result<FixVerdict> {
    let clean_before = !git::is_git_repo(path) || !git::is_dirty(path)?;
    if !run_action(path, rt, rule)? {
        return Ok(FixVerdict::Reverted);
    }
    // An action that changed nothing must not spend a build+test cycle.
    if clean_before && git::is_git_repo(path) && !git::is_dirty(path)? {
        return Ok(FixVerdict::NoOp);
    }
    if let Some(snap) = snapshot
        && !snap.changed()
    {
        return Ok(FixVerdict::NoOp);
    }
    let ctx = crate::adapters::Context {
        path,
        jobs: rt.jobs,
        timeout_s: rt.timeout_s,
        dry_run: rt.dry_run,
        force: rt.force,
    };
    let build = adapter.build(ctx)?;
    if !build.success() {
        revert_to_checkpoint(path, checkpoint_head)?;
        return Ok(FixVerdict::Reverted);
    }
    let test = adapter.test(ctx)?;
    if !test.success() {
        revert_to_checkpoint(path, checkpoint_head)?;
        return Ok(FixVerdict::Reverted);
    }
    if let Some(before) = baseline_test_output(baseline, &test.tool) {
        let after = normalize_test_output(&test.stdout);
        if before != after {
            revert_to_checkpoint(path, checkpoint_head)?;
            return Ok(FixVerdict::Reverted);
        }
    }
    if git::is_git_repo(path) {
        if !git::is_dirty(path)? {
            return Ok(FixVerdict::NoOp);
        }
        git::commit(path, &format!("codesafe: apply safe fix ({})", rule.id))?;
    }
    Ok(FixVerdict::Kept)
}

/// Revert the working tree to the checkpoint head (or HEAD if no checkpoint).
fn revert_to_checkpoint(path: &Path, checkpoint_head: Option<&str>) -> Result<()> {
    if git::is_git_repo(path) {
        if let Some(head) = checkpoint_head {
            git::revert_to(path, head)?;
        } else {
            git::revert_to(path, "HEAD")?;
        }
    }
    Ok(())
}

/// Run a rule's `action` command in the repository directory.
fn run_action(path: &Path, rt: &RuntimeConfig, rule: &Rule) -> Result<bool> {
    if rule.action.trim().is_empty() {
        return Ok(false);
    }
    let timeout = rule.timeout_s.or(rt.timeout_s);
    let result = crate::adapters::run_command_in_dir(path, rule.action.trim(), timeout)?;
    Ok(result.success())
}

fn finding_from_verdict(rule: &Rule, language: Language, verdict: FixVerdict) -> Finding {
    let (status, description) = match verdict {
        FixVerdict::Kept => (FindingStatus::Fixed, "applied and verified".to_string()),
        FixVerdict::Reverted => (
            FindingStatus::Reported,
            "applied but failed verification, reverted".to_string(),
        ),
        FixVerdict::NoOp => (
            FindingStatus::Reported,
            "already clean; no changes needed".to_string(),
        ),
        FixVerdict::SkippedDryRun => (
            FindingStatus::Reported,
            "dry run, would apply: ".to_string() + &rule.action,
        ),
        FixVerdict::SkippedMissingTool(tool) => (
            FindingStatus::Reported,
            format!("skipped: required tool '{tool}' not found on PATH"),
        ),
        FixVerdict::SkippedNoFiles => (
            FindingStatus::Reported,
            "skipped: no files match the rule globs".to_string(),
        ),
        FixVerdict::SkippedAlreadyCovered(category) => (
            FindingStatus::Reported,
            format!("skipped: {category} already handled by an earlier rule"),
        ),
    };
    let mut f = Finding::new(
        &rule.id,
        rule.severity.unwrap_or(Severity::Info),
        language,
        rule.category.as_deref().unwrap_or("rule"),
        &description,
    );
    f.status = status;
    f.fix = Some(rule.action.clone());
    f.evidence = rule.explanation.clone();
    f.fix_risk = Some("low: action is applied only when build and tests still pass".to_string());
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_matches_tool_code() {
        assert!(free_trigger_matches(
            "ruff:E501",
            "ruff",
            "main.py:4:8: E501 too long"
        ));
        assert!(!free_trigger_matches(
            "ruff:E501",
            "ruff",
            "main.py:4:8: F401 unused"
        ));
        assert!(!free_trigger_matches(
            "ruff:E501",
            "mypy",
            "main.py:4:8: E501 too long"
        ));
    }

    #[test]
    fn trigger_matches_substring() {
        assert!(free_trigger_matches("warning", "ruff", "some warning here"));
        assert!(!free_trigger_matches("warning", "ruff", "no match"));
    }

    #[test]
    fn scan_location_extracts_file_line() {
        assert_eq!(
            scan_location("src/lib.rs:10:5: error: unused variable"),
            Some(("src/lib.rs".to_string(), 10))
        );
        assert_eq!(scan_location("main.cpp: undefined reference"), None);
        assert_eq!(scan_location("no location here"), None);
    }

    #[test]
    fn scan_location_handles_windows_paths() {
        assert_eq!(
            scan_location(r"C:\repo\src\main.rs:12: error: broken"),
            Some((r"C:\repo\src\main.rs".to_string(), 12))
        );
    }

    #[test]
    fn findings_from_results_wrap_tool_failures() {
        let engine = RuleEngine::new();
        let results = vec![ToolResult {
            tool: "ruff".into(),
            command: "ruff check .".into(),
            exit_code: Some(1),
            stdout: "main.py:4:8: E501 line too long\n".into(),
            stderr: String::new(),
            duration_ms: 5,
            skipped: false,
            skip_reason: None,
        }];
        let findings = findings_from_results(&engine, &results, Language::Python);
        assert!(!findings.is_empty());
        assert_eq!(findings[0].status, FindingStatus::Reported);
        assert_eq!(findings[0].file, "main.py");
        assert_eq!(findings[0].line, Some(4));
    }

    #[test]
    fn load_with_builtin_falls_back() {
        let dir = std::env::temp_dir().join(format!("codesafe-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = RuleEngine::load_with_builtin(&dir).unwrap();
        assert!(engine.is_builtin());
        assert!(engine.rules().len() >= 25);
    }

    #[test]
    fn project_rules_override_builtin() {
        let dir = std::env::temp_dir().join(format!("codesafe-rules2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("python.toml"),
            r#"
[[rules]]
id = "custom"
language = "python"
trigger = "ruff"
action = "ruff format ."
safety = "safe"
explanation = "custom"
"#,
        )
        .unwrap();
        let engine = RuleEngine::load_with_builtin(&dir).unwrap();
        assert!(!engine.is_builtin());
        assert_eq!(engine.rules().len(), 1);
    }

    #[test]
    fn missing_tool_reports_first_missing() {
        assert_eq!(
            missing_tool(&["definitely-not-a-real-tool-xyz".to_string()]),
            Some("definitely-not-a-real-tool-xyz".to_string())
        );
        assert_eq!(missing_tool(&[]), None);
    }
}
