//! Binary entry point for codesafe.
//!
//! Every command supports human output (default) plus machine-consumable JSON
//! and JSONL envelopes for agent consumption: `{ ok, result, error, meta }`.

use clap::Parser;
use codeguard::brief;
use codeguard::config;
use codeguard::optimizer;
use codeguard::report;
use codeguard::rule_engine;
use codeguard::scan;
use codeguard::{
    Result,
    aci::{self, Envelope, OutputMode, print_envelope},
    adapters::{LanguageAdapter, cpp::Cpp, python::Python, rust::Rust},
    cli::{Cli, Command, RuntimeConfig, schema_json, skill_document},
    types::{
        BenchmarkResult, Finding, FindingStatus, Language, Report, Severity, ToolResult, TraceId,
    },
    verification,
};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("codesafe: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let output_mode = cli
        .output
        .parse::<OutputMode>()
        .map_err(codeguard::Error::Config)?;
    let root = command_root(&cli.command);
    let (config, config_path) = config::resolve(cli.config.as_deref(), root.as_deref())?;
    let rt = RuntimeConfig::from_cli(&cli, config, config_path, output_mode)?;
    let trace_root = root.as_ref().map(|p| p.to_string_lossy().to_string());
    let trace_id = aci::make_trace_id("codesafe", trace_root.as_deref());

    match cli.command {
        Command::Scan { path } => cmd_scan(&path, &rt, &trace_id),
        Command::Test { path } => cmd_test(&path, &rt, &trace_id),
        Command::Bench { path } => cmd_bench(&path, &rt, &trace_id),
        Command::Fix { path } => cmd_fix(&path, &rt, &trace_id),
        Command::Report { path } => cmd_report(&path, &rt, &trace_id),
        Command::All { path } => cmd_all(&path, &rt, &trace_id),
        Command::Init { path } => cmd_init(&path, &rt, &trace_id),
        Command::Spec {
            id,
            description,
            allowlist,
        } => cmd_spec(&id, &description, &allowlist, &rt, &trace_id),
        Command::Red {
            spec_id,
            test_name,
            test_file,
            test_source,
        } => cmd_red(
            &spec_id,
            &test_name,
            &test_file,
            &test_source,
            &rt,
            &trace_id,
        ),
        Command::Verify { path } => cmd_verify(&path, &rt, &trace_id),
        Command::Green { path } => cmd_green(&path, &rt, &trace_id),
        Command::Cover {
            path,
            min_complexity,
            max_coverage_pct,
        } => cmd_cover(
            path.as_path(),
            min_complexity,
            max_coverage_pct,
            &rt,
            &trace_id,
        ),
        Command::GenTest {
            path,
            max_tests,
            write,
        } => cmd_gen_test(path.as_path(), max_tests, write, &rt, &trace_id),
        Command::Heal { path, max_retries } => {
            cmd_heal(path.as_path(), max_retries, &rt, &trace_id)
        }
        Command::Replay { path } => cmd_replay(path.as_path(), &rt, &trace_id),
        Command::Hermetic {
            command,
            args,
            path,
        } => cmd_hermetic(&command, &args, path.as_path(), &rt, &trace_id),
        Command::Mutate { path, max_mutants } => cmd_mutate(&path, max_mutants, &rt, &trace_id),
        Command::Optimize { path, max } => cmd_optimize(&path, max, &rt, &trace_id),
        Command::Brief { path, write } => cmd_brief(&path, write, &rt, &trace_id),
        Command::Schema => cmd_schema(&rt),
        Command::GenerateSkill { path } => cmd_generate_skill(&path, &rt, &trace_id),
    }
}

/// Repository root implied by a command, used for config auto-discovery.
fn command_root(command: &Command) -> Option<PathBuf> {
    let path = match command {
        Command::Scan { path }
        | Command::Test { path }
        | Command::Bench { path }
        | Command::Fix { path }
        | Command::Report { path }
        | Command::All { path }
        | Command::Init { path }
        | Command::Verify { path }
        | Command::Green { path }
        | Command::Cover { path, .. }
        | Command::GenTest { path, .. }
        | Command::Heal { path, .. }
        | Command::Optimize { path, .. }
        | Command::Mutate { path, .. }
        | Command::Brief { path, .. }
        | Command::GenerateSkill { path }
        | Command::Hermetic { path, .. } => path.clone(),
        Command::Replay { path } => {
            if path.is_dir() {
                path.clone()
            } else {
                path.parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."))
            }
        }
        Command::Spec { .. } | Command::Red { .. } | Command::Schema => {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        }
    };
    Some(path)
}

fn emit_ok<T: Serialize>(
    mode: OutputMode,
    result: T,
    command: &str,
    path: Option<&str>,
    trace_id: &TraceId,
) -> ExitCode {
    let env = Envelope::ok(result, command, path, trace_id);
    print_envelope(mode, &env);
    ExitCode::from(0)
}

#[allow(clippy::too_many_arguments)]
fn emit_err(
    mode: OutputMode,
    code: &str,
    category: &str,
    message: &str,
    suggestion: Option<&str>,
    is_retryable: bool,
    command: &str,
    path: Option<&str>,
    trace_id: &TraceId,
) -> ExitCode {
    let env: Envelope<()> = Envelope::err(
        code,
        category,
        message,
        suggestion,
        is_retryable,
        command,
        path,
        trace_id,
    );
    print_envelope(mode, &env);
    ExitCode::from(1)
}

fn adapters_for() -> Vec<Box<dyn LanguageAdapter>> {
    vec![
        Box::new(Python) as Box<dyn LanguageAdapter>,
        Box::new(Rust) as Box<dyn LanguageAdapter>,
        Box::new(Cpp) as Box<dyn LanguageAdapter>,
    ]
}

fn run_tools(path: &Path, rt: &RuntimeConfig) -> Result<Vec<(Language, Vec<ToolResult>)>> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let adapters = adapters_for();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(rt.jobs.max(1))
        .build()
        .map_err(|e| codeguard::Error::Config(format!("thread pool: {e}")))?;

    let results: Vec<(Language, Vec<ToolResult>)> = pool.install(|| {
        adapters
            .into_par_iter()
            .filter_map(|adapter| {
                if adapter.detect(&path).ok() == Some(true) {
                    Some(adapter)
                } else {
                    None
                }
            })
            .map(|adapter| {
                let language = adapter.language();
                let ctx = codeguard::adapters::Context {
                    path: &path,
                    jobs: rt.jobs,
                    timeout_s: rt.timeout_s,
                    dry_run: rt.dry_run,
                    force: rt.force,
                };
                let results = vec![
                    adapter.build(ctx)?,
                    adapter.test(ctx)?,
                    adapter.lint(ctx)?,
                    adapter.fuzz(ctx)?,
                ];
                Ok((language, results))
            })
            .collect::<Result<Vec<_>>>()
    })?;

    Ok(results)
}

fn flatten(per_language: &[(Language, Vec<ToolResult>)]) -> Vec<ToolResult> {
    per_language
        .iter()
        .flat_map(|(_, r)| r.iter().cloned())
        .collect()
}

fn worst_severity(results: &[ToolResult]) -> Option<Severity> {
    let mut worst: Option<Severity> = None;
    for r in results {
        if r.skipped {
            continue;
        }
        let sev = r.severity();
        worst = Some(match worst {
            None => sev,
            Some(w) if sev > w => sev,
            Some(w) => w,
        });
    }
    worst
}

fn any_failed(results: &[ToolResult]) -> bool {
    results.iter().any(|r| !r.skipped && r.exit_code != Some(0))
}

fn print_results(results: &[ToolResult]) {
    for r in results {
        let status = if r.skipped {
            "skipped"
        } else if r.success() {
            "ok"
        } else {
            "FAILED"
        };
        println!("[{}] {} ({} ms)", status, r.tool, r.duration_ms);
        if !r.stdout.is_empty() {
            for line in r.stdout.lines().take(12) {
                println!("    {line}");
            }
        }
        if !r.stderr.is_empty() {
            for line in r.stderr.lines().take(12) {
                println!("    {line}");
            }
        }
    }
}

fn gate_failed(results: &[ToolResult], rt: &RuntimeConfig) -> bool {
    let failed = any_failed(results);
    let worst = worst_severity(results);
    let gated = worst.is_some_and(|s| rt.fail_on.is_some_and(|f| s >= f));
    match rt.fail_on {
        None => failed,
        Some(_) => gated,
    }
}

/// Scan command — detect languages, build systems, tools.
fn cmd_scan(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let result = scan::scan(path)?;
    let mode = rt.output_mode;

    #[derive(Serialize)]
    struct ScanResult {
        languages: Vec<String>,
        source_files: usize,
        build_systems: Vec<String>,
        config_files: usize,
        available_tools: Vec<String>,
        trace_id: String,
    }

    let scan_result = ScanResult {
        languages: result.languages.iter().map(|l| l.to_string()).collect(),
        source_files: result.source_files.len(),
        build_systems: result.build_systems.iter().map(|b| b.to_string()).collect(),
        config_files: result.config_files.len(),
        available_tools: result.tool_results.iter().map(|t| t.tool.clone()).collect(),
        trace_id: trace_id.clone(),
    };

    if mode == OutputMode::Human {
        println!("{}", scan::render(&result));
    } else {
        let _ = emit_ok(
            mode,
            scan_result,
            "scan",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Test command — build, test, lint across all languages.
fn cmd_test(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let results = flatten(&run_tools(path, rt)?);
    let mode = rt.output_mode;

    if mode == OutputMode::Human {
        print_results(&results);
    }

    let fail = gate_failed(&results, rt);
    let worst = worst_severity(&results);

    #[derive(Serialize)]
    struct TestResult {
        tests_passed: bool,
        tools_total: u64,
        tools_failed: u64,
        tools: Vec<ToolResult>,
        worst_severity: Option<String>,
        fail_on: Option<String>,
        gated: bool,
        trace_id: String,
    }

    let tools_total = results.iter().filter(|r| !r.skipped).count() as u64;
    let tools_failed = results
        .iter()
        .filter(|r| !r.skipped && r.exit_code != Some(0))
        .count() as u64;
    let gated = worst.is_some_and(|s| rt.fail_on.is_some_and(|f| s >= f));

    let test_result = TestResult {
        tests_passed: !fail,
        tools_total,
        tools_failed,
        tools: results,
        worst_severity: worst.map(|s| s.to_string()),
        fail_on: rt.fail_on.map(|s| s.to_string()),
        gated,
        trace_id: trace_id.clone(),
    };

    if mode == OutputMode::Human {
        if fail {
            println!("\ncodesafe test: FAIL (fail_on={:?})", rt.fail_on);
        } else {
            println!("\ncodesafe test: pass");
        }
    } else {
        let _ = emit_ok(
            mode,
            test_result,
            "test",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(if fail {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    })
}

#[derive(Default)]
struct BenchSummary {
    benchmarks: Vec<BenchmarkResult>,
    hotspots: Vec<codeguard::adapters::Hotspot>,
    regressions: u64,
}

/// Bench command — run benchmarks, profile, rank hotspots.
fn cmd_bench(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let scan_result = match scan::scan(&path) {
        Ok(s) => s,
        Err(_) => {
            if rt.output_mode == OutputMode::Human {
                println!("codesafe bench: {} (no language detected)", path.display());
            } else {
                #[derive(Serialize)]
                struct BenchResult {
                    benchmarks: Vec<BenchmarkResult>,
                    hotspots: Vec<codeguard::adapters::Hotspot>,
                    regressions: u64,
                    trace_id: String,
                }
                let _ = emit_ok(
                    rt.output_mode,
                    BenchResult {
                        benchmarks: Vec::new(),
                        hotspots: Vec::new(),
                        regressions: 0,
                        trace_id: trace_id.clone(),
                    },
                    "bench",
                    Some(&path.to_string_lossy()),
                    trace_id,
                );
            }
            return Ok(ExitCode::from(0));
        }
    };
    let source_hash = codeguard::types::hash_files(&scan_result.source_files)?;
    let summary = collect_benchmarks(&path, rt, &source_hash)?;

    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        println!("codesafe bench: {}", path.display());
        if summary.benchmarks.is_empty() {
            println!("  benchmarks: none (no benchmark tooling found)");
        } else {
            println!("  benchmarks (median of {} runs):", rt.bench_runs);
            for b in &summary.benchmarks {
                println!(
                    "    {} = {:?} {} (change {:?}%)",
                    b.name, b.after, b.unit, b.change_pct
                );
            }
        }
        if summary.regressions > 0 {
            println!(
                "  {} regression(s) flagged (threshold {}%)",
                summary.regressions, rt.regression_threshold_pct
            );
        }
        if summary.hotspots.is_empty() {
            println!("  hotspots: none (no profiler data available)");
        } else {
            println!("  ranked hotspots:");
            for h in &summary.hotspots {
                println!(
                    "    {:5.1}%  {} ({}:{:?})",
                    h.percent_runtime, h.function, h.file, h.line
                );
            }
        }
    } else {
        #[derive(Serialize)]
        struct BenchResult {
            benchmarks: Vec<BenchmarkResult>,
            hotspots: Vec<codeguard::adapters::Hotspot>,
            regressions: u64,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            BenchResult {
                benchmarks: summary.benchmarks,
                hotspots: summary.hotspots,
                regressions: summary.regressions,
                trace_id: trace_id.clone(),
            },
            "bench",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(if summary.regressions > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    })
}

fn collect_benchmarks(path: &Path, rt: &RuntimeConfig, source_hash: &str) -> Result<BenchSummary> {
    let mut cache = codeguard::bench::BenchmarkCache::new(path)?;
    let mut adapters = adapters_for();
    let mut samples: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    let mut units: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut hotspots = Vec::new();

    for adapter in adapters.iter_mut() {
        if !adapter.detect(path)? {
            continue;
        }
        let language = adapter.language();
        let ctx = codeguard::adapters::Context {
            path,
            jobs: rt.jobs,
            timeout_s: rt.timeout_s,
            dry_run: rt.dry_run,
            force: rt.force,
        };
        for _ in 0..rt.bench_runs.max(1) {
            let before_count: usize = samples.values().map(|v| v.len()).sum();
            let mut added = false;
            for b in adapter.bench(ctx)? {
                if let Some(value) = b.after {
                    let key = (language.to_string(), b.name.clone());
                    samples.entry(key.clone()).or_default().push(value);
                    units.insert(key, b.unit);
                    added = true;
                }
            }
            let after_count: usize = samples.values().map(|v| v.len()).sum();
            if !added && after_count == before_count {
                break;
            }
        }
        hotspots.extend(adapter.profile(ctx)?);
    }

    hotspots.sort_by(|a, b| {
        b.percent_runtime
            .partial_cmp(&a.percent_runtime)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut benchmarks = Vec::new();
    let mut regressions = 0u64;
    for ((language_str, name), mut values) in samples {
        let median = codeguard::bench::median(&mut values);
        let Some(after) = median else {
            continue;
        };
        let language = match language_str.as_str() {
            "rust" => Language::Rust,
            "cpp" => Language::Cpp,
            _ => Language::Python,
        };
        let before = cache.previous(language, source_hash, &name);
        let change_pct = before.map(|base| {
            if base == 0.0 {
                0.0
            } else {
                (after - base) / base * 100.0
            }
        });
        if change_pct.is_some_and(|c| c > rt.regression_threshold_pct) {
            regressions += 1;
        }
        cache.store(language, source_hash, &name, after)?;
        let unit = units
            .get(&(language.to_string(), name.clone()))
            .cloned()
            .unwrap_or_else(|| "s".to_string());
        benchmarks.push(BenchmarkResult {
            name,
            before,
            after: Some(after),
            unit,
            change_pct,
        });
    }
    Ok(BenchSummary {
        benchmarks,
        hotspots,
        regressions,
    })
}

/// Locate the rules directory, resolving relative paths against the repo.
fn rules_dir_for(path: &Path, rt: &RuntimeConfig) -> PathBuf {
    if let Some(d) = &rt.rules_dir {
        let p = Path::new(d);
        return if p.is_absolute() {
            p.to_path_buf()
        } else {
            path.join(p)
        };
    }
    let project_rules = path.join("rules");
    if project_rules.is_dir() {
        return project_rules;
    }
    let nested = path.join(".codesafe").join("rules");
    if nested.is_dir() {
        return nested;
    }
    project_rules
}

/// Fix command — apply safe rules with verification loop.
fn cmd_fix(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    if !rt.dry_run
        && codeguard::git::is_git_repo(&path)
        && !rt.force
        && codeguard::git::is_dirty(&path)?
    {
        return Ok(emit_err(
            rt.output_mode,
            "E001",
            "git",
            "working tree is dirty; commit or stash changes, or use --force",
            Some("commit or stash your changes, then re-run; or pass --force to bypass"),
            true,
            "fix",
            Some(&path.to_string_lossy()),
            trace_id,
        ));
    }
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_with_builtin(&rules_dir)?;
    let mut adapters = adapters_for();
    let findings = rule_engine::safe_fix_loop(&path, rt, &engine, &mut adapters)?;
    let had_revert = findings
        .iter()
        .any(|f| f.description.contains("failed verification"));

    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        println!(
            "codesafe fix: {} rule(s) {}",
            engine.rules().len(),
            if engine.is_builtin() {
                "(built-in pack)"
            } else {
                "(project rules)"
            }
        );
        for f in &findings {
            println!("[fix] {} [{:?}] {}", f.id, f.status, f.description);
        }
        if rt.dry_run {
            println!("codesafe fix: dry run, no changes applied");
        } else {
            let kept = findings
                .iter()
                .filter(|f| f.status == FindingStatus::Fixed)
                .count();
            println!("codesafe fix: applied {kept} verified fix(es)");
        }
    } else {
        #[derive(Serialize)]
        struct FixResult {
            findings: Vec<Finding>,
            applied: usize,
            skipped: usize,
            dry_run: bool,
            builtin_rules: bool,
            trace_id: String,
        }
        let applied = findings
            .iter()
            .filter(|f| f.status == FindingStatus::Fixed)
            .count();
        let skipped = findings.len() - applied;
        let _ = emit_ok(
            mode,
            FixResult {
                findings,
                applied,
                skipped,
                dry_run: rt.dry_run,
                builtin_rules: engine.is_builtin(),
                trace_id: trace_id.clone(),
            },
            "fix",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(if had_revert {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    })
}

/// Build the full report (tool results, findings, tips, optimizations) and write it.
fn build_and_write_report(path: &Path, rt: &RuntimeConfig) -> Result<Report> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let scan_result = scan::scan(&path)?;
    let per_language = run_tools(&path, rt)?;
    let results = flatten(&per_language);
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_with_builtin(&rules_dir)?;

    let mut findings = Vec::new();
    let mut parsed_diagnostics = Vec::new();
    for (language, lang_results) in &per_language {
        findings.extend(rule_engine::findings_from_results_in(
            Some(&path),
            &engine,
            lang_results,
            *language,
        ));
        for r in lang_results {
            if !r.skipped && r.exit_code != Some(0) {
                let trace = aci::make_trace_id("report", Some(&r.tool));
                parsed_diagnostics.extend(rule_engine::parsed_diagnostics_for(r, &trace));
            }
        }
    }
    codeguard::diagnostics::attach_recovery_hints(&mut parsed_diagnostics);

    let mut report = Report::new(path.to_string_lossy());
    report.summary.tools_found = results.iter().filter(|r| !r.skipped).count();
    report.summary.tools_missing = results.iter().filter(|r| r.skipped).count();
    report.summary.tests_passed = !any_failed(&results);
    report.summary.tests_total = results.iter().filter(|r| !r.skipped).count() as u64;
    report.summary.findings_total = findings.len();
    report.summary.findings_fixed = findings
        .iter()
        .filter(|f| f.status == FindingStatus::Fixed)
        .count();
    report.summary.language = scan_result.languages.first().copied();
    report.ai_tips = report::ai_tips_for_root(Some(&path), &findings);
    report.findings = findings;
    report.tool_results = results;
    report.optimizations = optimizer::analyze_repo(&path);
    report.optimizations.truncate(250);
    report.summary.benchmarks_total = 0;
    report.parsed_diagnostics = parsed_diagnostics;
    report::write(&path, &report)?;
    Ok(report)
}

/// Report command — write report.md and report.json.
fn cmd_report(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let report = build_and_write_report(path, rt)?;
    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        println!(
            "wrote report.md and report.json to {} ({} findings, {} optimizations, {} ai tips)",
            path.display(),
            report.summary.findings_total,
            report.optimizations.len(),
            report.ai_tips.len()
        );
    } else {
        #[derive(Serialize)]
        struct ReportResult {
            path: String,
            findings_total: usize,
            findings_fixed: usize,
            optimizations: usize,
            ai_tips: usize,
            tools_found: usize,
            tools_missing: usize,
            tests_passed: bool,
            parsed_diagnostics_count: usize,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            ReportResult {
                path: path.to_string_lossy().to_string(),
                findings_total: report.summary.findings_total,
                findings_fixed: report.summary.findings_fixed,
                optimizations: report.optimizations.len(),
                ai_tips: report.ai_tips.len(),
                tools_found: report.summary.tools_found,
                tools_missing: report.summary.tools_missing,
                tests_passed: report.summary.tests_passed,
                parsed_diagnostics_count: report.parsed_diagnostics.len(),
                trace_id: trace_id.clone(),
            },
            "report",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// All command — scan → test → bench → fix → report.
fn cmd_all(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        let _ = cmd_scan(path, rt, trace_id)?;
        let test_code = cmd_test(path, rt, trace_id)?;
        let _ = cmd_bench(path, rt, trace_id)?;
        let fix_code = cmd_fix(path, rt, trace_id)?;
        let _ = cmd_report(path, rt, trace_id)?;
        if test_code == ExitCode::from(0) && fix_code == ExitCode::from(0) {
            println!("\ncodesafe all: complete");
            return Ok(ExitCode::from(0));
        }
        println!("\ncodesafe all: completed with failures");
        return Ok(ExitCode::from(1));
    }

    // Machine mode: emit exactly one envelope for the whole pipeline.
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let initial = run_tools(&path, rt)?;
    let initial_results = flatten(&initial);
    let test_passed = !any_failed(&initial_results);
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_with_builtin(&rules_dir)?;
    let mut adapters = adapters_for();
    let fix_findings = if rt.dry_run {
        Vec::new()
    } else {
        rule_engine::safe_fix_loop(&path, rt, &engine, &mut adapters).unwrap_or_default()
    };
    let fixes_applied = fix_findings
        .iter()
        .filter(|f| f.status == FindingStatus::Fixed)
        .count();
    let report = build_and_write_report(&path, rt)?;
    let bench = {
        let bench_hash = codeguard::types::hash_files(&scan::scan(&path)?.source_files)?;
        collect_benchmarks(&path, rt, &bench_hash)?
    };

    #[derive(Serialize)]
    struct AllResult {
        scan_passed: bool,
        test_passed: bool,
        fixes_applied: usize,
        benches: usize,
        regressions: u64,
        report_written: bool,
        findings_total: usize,
        optimizations: usize,
        trace_id: String,
    }
    let _ = emit_ok(
        mode,
        AllResult {
            scan_passed: true,
            test_passed,
            fixes_applied,
            benches: bench.benchmarks.len(),
            regressions: bench.regressions,
            report_written: true,
            findings_total: report.summary.findings_total,
            optimizations: report.optimizations.len(),
            trace_id: trace_id.clone(),
        },
        "all",
        Some(&path.to_string_lossy()),
        trace_id,
    );
    Ok(if test_passed && bench.regressions == 0 {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    })
}

/// Init command — generate codesafe.toml and a comprehensive rules/ pack.
fn cmd_init(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let config_path = path.join("codesafe.toml");
    if config_path.exists() {
        return Ok(emit_err(
            rt.output_mode,
            "E002",
            "config",
            &format!("{config_path:?} already exists; refusing to overwrite"),
            Some("remove the existing file or choose a different path"),
            false,
            "init",
            Some(&path.to_string_lossy()),
            trace_id,
        ));
    }
    std::fs::write(&config_path, config::example())
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    let rules_dir = path.join("rules");
    std::fs::create_dir_all(&rules_dir)
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
    for language in [Language::Python, Language::Rust, Language::Cpp] {
        let rules_path = rules_dir.join(format!("{language}.toml"));
        if !rules_path.exists() {
            std::fs::write(&rules_path, codeguard::builtin_rules::rules_toml(language))
                .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
        }
    }

    let rule_count = codeguard::builtin_rules::default_rules().len();
    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        println!(
            "Initialized codesafe:\n  config: {}\n  rules:  {}/ ({} rules)\n\nEdit codesafe.toml and rules/*.toml to tune behavior.\nRun `codesafe test <path>` to verify.",
            config_path.display(),
            rules_dir.display(),
            rule_count
        );
    } else {
        #[derive(Serialize)]
        struct InitResult {
            config_path: String,
            rules_dir: String,
            rules: usize,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            InitResult {
                config_path: config_path.to_string_lossy().to_string(),
                rules_dir: rules_dir.to_string_lossy().to_string(),
                rules: rule_count,
                trace_id: trace_id.clone(),
            },
            "init",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Schema command — emit JSON schema for agent introspection.
fn cmd_schema(rt: &RuntimeConfig) -> Result<ExitCode> {
    let schema = schema_json();
    match rt.output_mode {
        OutputMode::Human => match serde_json::to_string_pretty(&schema) {
            Ok(text) => {
                println!("{text}");
                Ok(ExitCode::from(0))
            }
            Err(e) => Err(codeguard::Error::Json(e)),
        },
        OutputMode::Json | OutputMode::Jsonl => {
            let _ = emit_ok(
                rt.output_mode,
                schema,
                "schema",
                None,
                &aci::make_trace_id("schema", None),
            );
            Ok(ExitCode::from(0))
        }
    }
}

/// GenerateSkill command — emit an agent skill document for LLM tool-use.
fn cmd_generate_skill(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let skill = skill_document(&path.to_string_lossy());
    match rt.output_mode {
        OutputMode::Human => {
            println!("{skill}");
            Ok(ExitCode::from(0))
        }
        OutputMode::Json | OutputMode::Jsonl => {
            let _ = emit_ok(
                rt.output_mode,
                skill,
                "generate-skill",
                Some(&path.to_string_lossy()),
                trace_id,
            );
            Ok(ExitCode::from(0))
        }
    }
}

/// Spec command — write and persist acceptance criteria (test-writing agent only).
fn cmd_spec(
    id: &str,
    description: &str,
    allowlist: &[PathBuf],
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let cwd =
        std::env::current_dir().map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
    let language = scan::scan(&cwd)
        .ok()
        .and_then(|s| s.languages.first().copied())
        .unwrap_or(Language::Python);
    let normalized: Vec<PathBuf> = allowlist
        .iter()
        .map(|p| {
            p.strip_prefix(&cwd)
                .map(|rel| rel.to_path_buf())
                .unwrap_or_else(|_| p.clone())
        })
        .collect();
    let spec = verification::create_spec(
        id,
        description,
        normalized
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect(),
        normalized.clone(),
        language,
        trace_id,
    );
    let path = verification::save_spec(&cwd, &spec)?;

    if mode == OutputMode::Human {
        println!("Spec created: {}", spec.id);
        println!("  Description: {}", spec.description);
        println!("  Criteria: {}", spec.acceptance_criteria.len());
        println!("  Allowlist: {:?}", spec.file_allowlist);
        println!("  Saved: {}", path.display());
    } else {
        let _ = emit_ok(mode, spec, "spec", Some(&path.to_string_lossy()), trace_id);
    }
    Ok(ExitCode::from(0))
}

/// Red command — write a RED test from a spec (test-writing agent only).
fn cmd_red(
    spec_id: &str,
    test_name: &str,
    test_file: &Path,
    test_source: &str,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let language = test_file
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| match ext {
            "py" => Some(Language::Python),
            "rs" => Some(Language::Rust),
            "c" | "cpp" | "cc" | "cxx" => Some(Language::Cpp),
            _ => None,
        })
        .unwrap_or(Language::Python);

    let unit_under_test = test_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unit")
        .trim_start_matches("test_")
        .to_string();
    let spec = verification::load_spec(
        &std::env::current_dir().map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?,
    )?
    .unwrap_or_else(|| verification::Spec {
        id: spec_id.to_string(),
        description: String::new(),
        acceptance_criteria: Vec::new(),
        file_allowlist: Vec::new(),
        language,
        trace_id: trace_id.clone(),
    });

    let gaming = verification::check_stub_passability(test_source, &unit_under_test);
    let tautology = verification::check_tautological(test_source);
    for check in [gaming, tautology] {
        if matches!(check, verification::AntiGamingResult::GamingDetected { .. }) {
            return Ok(emit_err(
                mode,
                "E003",
                "anti-gaming",
                &format!("{check:?}"),
                Some("remove mocks/tautologies from the RED test; exercise real code"),
                false,
                "red",
                Some(&test_file.to_string_lossy()),
                trace_id,
            ));
        }
    }

    let pre_existing = std::fs::read_to_string(test_file).ok();
    if let Some(parent) = test_file.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
    }
    std::fs::write(test_file, test_source)
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    let test_case = verification::write_red_test(
        &spec,
        test_name,
        test_source,
        test_file.to_path_buf(),
        trace_id,
    );
    let integrity = pre_existing
        .as_deref()
        .map(|pre| verification::check_test_integrity(pre, test_source));

    if mode == OutputMode::Human {
        println!("RED test written: {}", test_case.id);
        println!("  File: {}", test_case.file.display());
        println!(
            "  Anti-gaming: {}",
            match integrity {
                Some(verification::AntiGamingResult::GamingDetected { .. }) => "FAILED",
                _ => "passed",
            }
        );
    } else {
        #[derive(Serialize)]
        struct RedResult {
            test_case: verification::TestCase,
            integrity: Option<verification::AntiGamingResult>,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            RedResult {
                test_case,
                integrity,
                trace_id: trace_id.clone(),
            },
            "red",
            Some(&test_file.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Verify command — real anti-gaming verification of the working tree.
fn cmd_verify(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let spec = verification::load_spec(&path)?;
    let report = verification::verify_repository(&path, spec.as_ref());

    if mode == OutputMode::Human {
        println!(
            "codesafe verify: {} ({} changed file(s), {} test file(s) checked)",
            path.display(),
            report.changed_files.len(),
            report.checked_files.len()
        );
        if report.violations > 0 {
            println!(
                "codesafe verify: FAIL - {} anti-gaming violation(s)",
                report.violations
            );
            for check in &report.checks {
                if matches!(check, verification::AntiGamingResult::GamingDetected { .. }) {
                    println!("  - {check:?}");
                }
            }
        } else {
            println!("codesafe verify: pass - no anti-gaming violations");
        }
    } else {
        #[derive(Serialize)]
        struct VerifyResult {
            gaming_violations: usize,
            scope_ok: bool,
            checks: Vec<verification::AntiGamingResult>,
            changed_files: Vec<String>,
            checked_files: Vec<String>,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            VerifyResult {
                gaming_violations: report.violations,
                scope_ok: report.scope_ok,
                checks: report.checks,
                changed_files: report.changed_files,
                checked_files: report.checked_files,
                trace_id: trace_id.clone(),
            },
            "verify",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(if report.passed {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    })
}

/// Green command — run GREEN verification (implementation against RED tests).
fn cmd_green(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let results = flatten(&run_tools(path, rt)?);
    let passed = !any_failed(&results);

    if mode == OutputMode::Human {
        println!("codesafe green: {}", path.display());
        print_results(&results);
        if passed {
            println!("codesafe green: PASS - implementation verified against RED tests");
        } else {
            println!("codesafe green: FAIL - implementation does not satisfy RED tests");
        }
    } else {
        #[derive(Serialize)]
        struct GreenResult {
            passed: bool,
            tools: Vec<ToolResult>,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            GreenResult {
                passed,
                tools: results,
                trace_id: trace_id.clone(),
            },
            "green",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(if passed {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    })
}

/// Cover command — static coverage estimate and hotspots.
fn cmd_cover(
    path: &Path,
    min_complexity: u32,
    max_coverage_pct: f64,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let mode = rt.output_mode;
    let coverage = codeguard::test_generation::static_coverage(&path);
    let total_functions = coverage.len();
    let hotspots =
        codeguard::test_generation::identify_hotspots(&coverage, min_complexity, max_coverage_pct);

    if mode == OutputMode::Human {
        println!("codesafe cover: {} (static estimate)", path.display());
        println!(
            "  {} function(s), {} hotspot(s) with complexity >= {} and coverage < {}%",
            total_functions,
            hotspots.len(),
            min_complexity,
            max_coverage_pct
        );
        for h in hotspots.iter().take(30) {
            println!(
                "  {:>5.1}%  complexity {:>2}  {}  ({}:{:?})",
                h.coverage_pct,
                h.complexity,
                h.function,
                h.file.display(),
                h.line.map(|l| l.to_string()).unwrap_or_else(|| "-".into())
            );
        }
    } else {
        #[derive(Serialize)]
        struct CoverResult {
            hotspots: Vec<codeguard::test_generation::Hotspot>,
            total_functions: usize,
            estimated: bool,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            CoverResult {
                hotspots,
                total_functions,
                estimated: true,
                trace_id: trace_id.clone(),
            },
            "cover",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// GenTest command — generate tests targeting static-coverage hotspots.
fn cmd_gen_test(
    path: &Path,
    max_tests: u32,
    write: bool,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let mode = rt.output_mode;
    let coverage = codeguard::test_generation::static_coverage(&path);
    let hotspots = codeguard::test_generation::identify_hotspots(&coverage, 1, 100.0);
    let mut generated = Vec::new();
    for hotspot in hotspots.into_iter().take(max_tests as usize) {
        let language = scan::language_of_path(&hotspot.file).unwrap_or(Language::Python);
        let test =
            codeguard::test_generation::generate_test_for_hotspot(&hotspot, language, trace_id);
        if write && !rt.dry_run {
            let dest = path.join(&test.test_file);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
            }
            if !dest.exists() {
                std::fs::write(&dest, &test.source)
                    .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
            }
        }
        generated.push(test);
    }

    if mode == OutputMode::Human {
        println!(
            "codesafe gen-test: {} test(s) targeted at {} hotspot(s) for {}",
            generated.len(),
            coverage.len(),
            path.display()
        );
        for test in generated.iter().take(20) {
            println!(
                "  {} -> {} (complexity {})",
                test.hotspot.function,
                test.test_file.display(),
                test.hotspot.complexity
            );
        }
        if !write {
            println!("  report-only; pass --write to create the test skeletons");
        }
    } else {
        #[derive(Serialize)]
        struct GenTestResult {
            tests_generated: usize,
            written: bool,
            tests: Vec<codeguard::test_generation::GeneratedTest>,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            GenTestResult {
                tests_generated: generated.len(),
                written: write && !rt.dry_run,
                tests: generated,
                trace_id: trace_id.clone(),
            },
            "gen-test",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Heal command — self-healing loop: test → safe fix → retest.
fn cmd_heal(
    path: &Path,
    max_retries: u32,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_with_builtin(&rules_dir)?;
    let mut adapters = adapters_for();

    let mut attempts = 0u32;
    let mut passed = false;
    let mut blocking: Vec<String> = Vec::new();
    let mut fixes_applied = 0usize;
    let attempts_budget = max_retries.max(1);

    while attempts < attempts_budget {
        attempts += 1;
        let results = flatten(&run_tools(&path, rt)?);
        if !any_failed(&results) {
            passed = true;
            blocking.clear();
            break;
        }
        blocking = results
            .iter()
            .filter(|r| !r.skipped && r.exit_code != Some(0))
            .map(|r| r.tool.clone())
            .collect();
        if attempts == attempts_budget {
            break;
        }
        match rule_engine::safe_fix_loop(&path, rt, &engine, &mut adapters) {
            Ok(findings) => {
                let kept = findings
                    .iter()
                    .filter(|f| f.status == FindingStatus::Fixed)
                    .count();
                fixes_applied += kept;
                if kept == 0 {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let result = codeguard::test_generation::HealingResult {
        tests_generated: 0,
        tests_passed: usize::from(passed),
        tests_failed: if passed { 0 } else { blocking.len() },
        tests_exhausted: if passed { 0 } else { 1 },
        total_attempts: attempts,
        converged: passed,
        blocking_tests: blocking,
        trace_id: trace_id.clone(),
    };

    if mode == OutputMode::Human {
        println!(
            "codesafe heal: {} after {} attempt(s), {} fix(es) kept",
            if passed {
                "converged"
            } else {
                "did not converge"
            },
            attempts,
            fixes_applied
        );
        for b in &result.blocking_tests {
            println!("  blocking: {b}");
        }
    } else {
        #[derive(Serialize)]
        struct HealResult {
            result: codeguard::test_generation::HealingResult,
            fixes_applied: usize,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            HealResult {
                result,
                fixes_applied,
                trace_id: trace_id.clone(),
            },
            "heal",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(if passed {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    })
}

/// Replay command — replay the event-sourced orchestration log.
fn cmd_replay(path: &Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let log_path = if path.is_dir() {
        codeguard::orchestration::events_path(path)
    } else {
        path.to_path_buf()
    };
    let events = codeguard::orchestration::read_events(&log_path)?;
    let state = codeguard::orchestration::replay_events(&events);
    let phase = format!("{:?}", state.current_phase);

    if mode == OutputMode::Human {
        println!(
            "codesafe replay: {} event(s) from {}",
            events.len(),
            log_path.display()
        );
        for event in &events {
            println!(
                "  #{} {:?} ({})",
                event.seq_no, event.event_type, event.trace_id
            );
        }
        println!("final phase: {phase}");
    } else {
        #[derive(Serialize)]
        struct ReplayResult {
            events_replayed: usize,
            final_phase: String,
            spec: Option<String>,
            events: Vec<codeguard::orchestration::Event>,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            ReplayResult {
                events_replayed: events.len(),
                final_phase: phase,
                spec: state.current_spec,
                events,
                trace_id: trace_id.clone(),
            },
            "replay",
            Some(&log_path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Mutation testing scoped to the current agent diff.
///
/// Surviving mutants are the strongest signal that a test suite executes code
/// without actually defending its behaviour. Tools are optional: when neither
/// cargo-mutants nor mutmut is installed the command reports a skip instead of
/// failing, so agent pipelines can call it unconditionally.
fn cmd_mutate(
    path: &Path,
    max_mutants: u32,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let scan_result = scan::scan(&path)?;
    let languages = scan_result.languages.clone();

    let mut tool: Option<&str> = None;
    if languages.contains(&Language::Rust) && which::which("cargo-mutants").is_ok() {
        tool = Some("cargo-mutants");
    } else if languages.contains(&Language::Python) && which::which("mutmut").is_ok() {
        tool = Some("mutmut");
    }

    let Some(tool) = tool else {
        #[derive(Serialize)]
        struct MutationReport {
            ran: bool,
            tool: Option<String>,
            total: u64,
            killed: u64,
            survived: u64,
            timed_out: u64,
            not_covered: u64,
            note: String,
            report_path: Option<String>,
            diff_path: Option<String>,
            max_mutants: u32,
            trace_id: String,
        }
        let report = MutationReport {
            ran: false,
            tool: None,
            total: 0,
            killed: 0,
            survived: 0,
            timed_out: 0,
            not_covered: 0,
            note: "no mutation tool installed (install cargo-mutants or mutmut)".to_string(),
            report_path: None,
            diff_path: None,
            max_mutants,
            trace_id: trace_id.clone(),
        };
        if mode == OutputMode::Human {
            println!("codesafe mutate: skipped - {}", report.note);
        } else {
            let _ = emit_ok(
                mode,
                report,
                "mutate",
                Some(&path.to_string_lossy()),
                trace_id,
            );
        }
        return Ok(ExitCode::from(0));
    };

    let diff_path = if codeguard::git::is_git_repo(&path) {
        let output = std::process::Command::new("git")
            .args([
                "-C",
                path.to_str().unwrap_or("."),
                "diff",
                "HEAD",
                "--binary",
            ])
            .output()
            .map_err(|e| codeguard::Error::Git(format!("git diff: {e}")))?;
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.trim().is_empty() {
            None
        } else {
            let dir = path.join(".codesafe");
            std::fs::create_dir_all(&dir)
                .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
            let file = dir.join("agent.diff");
            std::fs::write(&file, diff)
                .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
            Some(file)
        }
    } else {
        None
    };

    let command = match tool {
        "cargo-mutants" => match &diff_path {
            Some(diff) => format!(
                "cargo mutants --in-diff {} --timeout 60",
                diff.to_string_lossy()
            ),
            None => "cargo mutants --timeout 60".to_string(),
        },
        _ => "mutmut run".to_string(),
    };
    let started = std::time::Instant::now();
    let result = codeguard::adapters::run_command_in_dir(&path, &command, rt.timeout_s)?;
    let output = format!("{}\n{}", result.stdout, result.stderr);
    let counts = parse_mutation_counts(&output);
    let report_path = path.join(".codesafe").join("mutation-report.txt");
    let _ = std::fs::create_dir_all(path.join(".codesafe"));
    let _ = std::fs::write(&report_path, &output);

    let survived = counts.survived;
    let ran = counts.total > 0 || counts.killed > 0 || survived > 0;

    #[derive(Serialize)]
    struct MutationReport {
        ran: bool,
        tool: Option<String>,
        total: u64,
        killed: u64,
        survived: u64,
        timed_out: u64,
        not_covered: u64,
        note: String,
        report_path: Option<String>,
        diff_path: Option<String>,
        max_mutants: u32,
        trace_id: String,
    }
    let report = MutationReport {
        ran,
        tool: Some(tool.to_string()),
        total: counts.total,
        killed: counts.killed,
        survived,
        timed_out: counts.timed_out,
        not_covered: counts.not_covered,
        note: if ran {
            "surviving mutants are concrete follow-up test tasks".to_string()
        } else {
            format!(
                "tool produced no parseable summary (exit {:?})",
                result.exit_code
            )
        },
        report_path: Some(report_path.to_string_lossy().to_string()),
        diff_path: diff_path.map(|p| p.to_string_lossy().to_string()),
        max_mutants,
        trace_id: trace_id.clone(),
    };

    if mode == OutputMode::Human {
        println!(
            "codesafe mutate: {} in {} ms",
            if ran {
                "completed"
            } else {
                "no parseable result"
            },
            started.elapsed().as_millis()
        );
        println!(
            "  total {} | killed {} | survived {} | timeout {} | not covered {}",
            report.total, report.killed, report.survived, report.timed_out, report.not_covered
        );
        if report.survived > 0 {
            println!(
                "  {} surviving mutant(s): add tests that fail when this behaviour changes",
                report.survived
            );
        }
        println!("  report: {}", report_path.display());
    } else {
        let _ = emit_ok(
            mode,
            report,
            "mutate",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(if survived > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    })
}

#[derive(Default)]
struct MutationCounts {
    total: u64,
    killed: u64,
    survived: u64,
    timed_out: u64,
    not_covered: u64,
}

/// Parse the summary lines of cargo-mutants and mutmut without depending on
/// either tool's exact output format.
fn parse_mutation_counts(output: &str) -> MutationCounts {
    let mut counts = MutationCounts::default();
    for line in output.lines() {
        let lower = line.to_lowercase();
        for label in ["mutants:", "total:"] {
            if let Some(n) = number_after(&lower, label) {
                counts.total = n;
            }
        }
        for label in ["caught:", "killed:"] {
            if let Some(n) = number_after(&lower, label) {
                counts.killed = n;
            }
        }
        for label in ["missed:", "survived:"] {
            if let Some(n) = number_after(&lower, label) {
                counts.survived = n;
            }
        }
        for label in ["timeout:", "timeouts:"] {
            if let Some(n) = number_after(&lower, label) {
                counts.timed_out = n;
            }
        }
        for label in ["unviable:", "skipped:", "not covered:"] {
            if let Some(n) = number_after(&lower, label) {
                counts.not_covered = n;
            }
        }
    }
    if counts.total == 0 {
        counts.total = counts.killed + counts.survived + counts.timed_out + counts.not_covered;
    }
    counts
}

fn number_after(line: &str, label: &str) -> Option<u64> {
    let idx = line.find(label)?;
    let rest = &line[idx + label.len()..];
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Optimize command — emit concrete optimization suggestions.
fn cmd_optimize(
    path: &Path,
    max: usize,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let mode = rt.output_mode;
    let mut optimizations = optimizer::analyze_repo(&path);
    optimizations.truncate(max);
    let json_path = path.join(".codesafe").join("optimizations.json");
    std::fs::create_dir_all(path.join(".codesafe"))
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;
    let json = serde_json::to_string_pretty(&optimizations).map_err(codeguard::Error::Json)?;
    std::fs::write(&json_path, json).map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    if mode == OutputMode::Human {
        println!(
            "codesafe optimize: {} suggestion(s) for {}",
            optimizations.len(),
            path.display()
        );
        for o in optimizations.iter().take(40) {
            println!(
                "  [{}] {}:{} ({}) {}",
                o.category,
                o.file,
                o.line.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
                o.symbol.as_deref().unwrap_or("-"),
                o.title
            );
            println!("      change: {}", o.suggested_change);
            println!(
                "      gain: {} | risk: {} | verify: {}",
                o.expected_gain, o.risk, o.how_to_verify
            );
        }
        println!("  full list: {}", json_path.display());
    } else {
        #[derive(Serialize)]
        struct OptimizeResult {
            optimizations: Vec<codeguard::types::Optimization>,
            written: String,
            trace_id: String,
        }
        let _ = emit_ok(
            mode,
            OptimizeResult {
                optimizations,
                written: json_path.to_string_lossy().to_string(),
                trace_id: trace_id.clone(),
            },
            "optimize",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Brief command — build an LLM-ready task packet.
fn cmd_brief(path: &Path, write: bool, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let scan_result = scan::scan(&path)?;
    let per_language = run_tools(&path, rt)?;
    let results = flatten(&per_language);
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_with_builtin(&rules_dir)?;
    let mut findings: Vec<Finding> = Vec::new();
    for (language, lang_results) in &per_language {
        findings.extend(rule_engine::findings_from_results_in(
            Some(&path),
            &engine,
            lang_results,
            *language,
        ));
    }
    let mut optimizations = optimizer::analyze_repo(&path);
    optimizations.truncate(120);
    let tests_passed = !any_failed(&results);
    let brief = brief::build(
        path.to_string_lossy(),
        scan_result.languages.clone(),
        tests_passed,
        &findings,
        optimizations,
        trace_id,
    );
    if write {
        brief::write(&path, &brief)?;
    }

    if mode == OutputMode::Human {
        if write {
            println!(
                "codesafe brief: wrote {}/AGENT_TASK.md and .codesafe/agent-brief.json",
                path.display()
            );
        }
        print!("{}", brief.render_markdown());
    } else {
        let _ = emit_ok(
            mode,
            brief,
            "brief",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Hermetic command — run a command in an isolated execution context.
fn cmd_hermetic(
    command: &str,
    args: &[String],
    path: &Path,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let ctx = codeguard::orchestration::create_hermetic_context(
        path.to_path_buf(),
        rt.timeout_s.unwrap_or(30),
        vec![],
    );
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = codeguard::orchestration::execute_hermetic(command, &arg_refs, &ctx, trace_id);

    if mode == OutputMode::Human {
        println!(
            "codesafe hermetic: ran '{} {}' with exit code {:?}",
            command,
            args.join(" "),
            result.exit_code
        );
        if !result.stdout.is_empty() {
            println!("stdout: {}", result.stdout);
        }
        if !result.stderr.is_empty() {
            println!("stderr: {}", result.stderr);
        }
        println!("duration: {} ms", result.duration_ms);
        Ok(ExitCode::from(result.exit_code.unwrap_or(1) as u8))
    } else {
        let exit_code = result.exit_code;
        let _ = emit_ok(
            mode,
            result,
            "hermetic",
            Some(&path.to_string_lossy()),
            trace_id,
        );
        match exit_code {
            Some(0) => Ok(ExitCode::from(0)),
            Some(code) => Ok(ExitCode::from(code as u8)),
            None => Ok(ExitCode::from(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_summary_parsing_supports_both_tools() {
        let cargo = "MUTANTS: 10, caught: 7, missed: 2, timeout: 1, unviable: 0";
        let c = parse_mutation_counts(cargo);
        assert_eq!(c.total, 10);
        assert_eq!(c.killed, 7);
        assert_eq!(c.survived, 2);
        assert_eq!(c.timed_out, 1);

        let mutmut = "Killed: 5\nSurvived: 3\nTotal: 8\n";
        let m = parse_mutation_counts(mutmut);
        assert_eq!(m.total, 8);
        assert_eq!(m.killed, 5);
        assert_eq!(m.survived, 3);
    }
}
