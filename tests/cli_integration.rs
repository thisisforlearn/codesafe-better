//! End-to-end CLI tests. These exercise the real binary in the same way an
//! agent or CI pipeline would: structured envelopes, exit codes, and artifacts.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

fn bin() -> Command {
    Command::cargo_bin("codeguard").expect("binary builds")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn write_python_project(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[tool.pytest.ini_options]\npythonpath = [\"src\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/demo.py"),
        "def slow(n):\n    total = 0\n    for i in range(len(range(n))):\n        total += i\n    return total\n\ndef untested(x):\n    if x:\n        return 1\n    return 0\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("tests/test_demo.py"),
        "from demo import slow\n\ndef test_slow():\n    assert slow(4) == 6\n",
    )
    .unwrap();
}

#[test]
fn version_and_help_work() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("0.3.0"));
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("codesafe"));
}

#[test]
fn schema_is_valid_json_with_output_mode() {
    let out = bin()
        .args(["schema", "--output", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["meta"]["output_mode"], "json");
    assert!(value["result"]["commands"]["brief"].is_object());
}

#[test]
fn scan_detects_python_and_emits_envelope() {
    let path = fixture("python/buggy_pkg");
    bin()
        .args(["scan"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("python"));

    let out = bin()
        .args(["scan", "--output", "json"])
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["ok"], true);
    assert!(
        value["result"]["languages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "python")
    );
}

#[test]
fn init_scaffolds_config_and_full_rule_pack() {
    let dir = tempfile::tempdir().unwrap();
    bin().arg("init").arg(dir.path()).assert().success();

    assert!(dir.path().join("codesafe.toml").is_file());
    for lang in ["python", "rust", "cpp"] {
        let path = dir.path().join("rules").join(format!("{lang}.toml"));
        assert!(path.is_file(), "missing {lang} rules");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("[[rules]]"));
        assert!(text.contains("safety = \"safe\""));
    }

    // Refuses to overwrite an existing config.
    bin()
        .arg("init")
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn report_writes_json_and_markdown_with_context_and_optimizations() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    bin()
        .args(["report", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success();

    let report_text = std::fs::read_to_string(dir.path().join("report.json")).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert!(report["summary"].is_object());
    assert!(report["optimizations"].is_array());
    assert!(!report["optimizations"].as_array().unwrap().is_empty());

    let md = std::fs::read_to_string(dir.path().join("report.md")).unwrap();
    assert!(md.contains("# codesafe report"));
    assert!(md.contains("## Optimization opportunities"));
}

#[test]
fn optimize_emits_actionable_suggestions() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let out = bin()
        .args(["optimize", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let opts = value["result"]["optimizations"].as_array().unwrap();
    assert!(!opts.is_empty());
    let first = &opts[0];
    for key in [
        "file",
        "folder",
        "line",
        "explanation",
        "suggested_change",
        "expected_gain",
        "risk",
        "why_safe",
        "how_to_apply",
        "how_to_verify",
    ] {
        assert!(first.get(key).is_some(), "missing optimization field {key}");
    }
    assert!(dir.path().join(".codesafe/optimizations.json").is_file());
}

#[test]
fn brief_produces_llm_ready_task_packet() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    // Force a localized finding so the task packet has a real task.
    std::fs::write(
        dir.path().join("src/broken.py"),
        "def broken(:\n    return 1\n",
    )
    .unwrap();
    let out = bin()
        .args(["brief", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let brief = &value["result"];
    assert_eq!(brief["schema_version"], "1.0");
    assert!(brief["policy"].as_array().unwrap().len() >= 4);
    assert!(brief["verification_plan"].as_array().unwrap().len() >= 2);
    assert!(dir.path().join("AGENT_TASK.md").is_file());
    assert!(dir.path().join(".codesafe/agent-brief.json").is_file());
    let md = std::fs::read_to_string(dir.path().join("AGENT_TASK.md")).unwrap();
    assert!(md.contains("## Tasks"));
    assert!(md.contains("definition of done"));
}

#[test]
fn cover_and_gen_test_work_on_real_project() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let out = bin()
        .args(["cover", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["result"]["estimated"], true);
    assert!(value["result"]["total_functions"].as_u64().unwrap() >= 1);

    bin()
        .args(["gen-test"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("report-only"));
}

#[test]
fn fix_dry_run_does_not_modify_files() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let before = std::fs::read_to_string(dir.path().join("src/demo.py")).unwrap();
    bin()
        .args(["fix", "--dry-run"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("dry run"));
    let after = std::fs::read_to_string(dir.path().join("src/demo.py")).unwrap();
    assert_eq!(before, after);
}

#[test]
fn verify_and_replay_are_usable_out_of_the_box() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    bin()
        .args(["verify", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("\"gaming_violations\": 0"));

    bin()
        .args(["replay", "--output", "json"])
        .arg(dir.path().join(".codesafe/events.jsonl"))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"events_replayed\": 0"));
}

#[test]
fn hermetic_execution_returns_output() {
    let out = bin()
        .args(["hermetic", "--output", "json", "echo", "hello"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["result"]["exit_code"], 0);
    assert!(
        value["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("hello")
    );
}

#[test]
fn invalid_fail_on_is_a_config_error() {
    bin()
        .args(["test", "--fail-on", "bogus"])
        .arg(fixture("python/buggy_pkg"))
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unknown severity"));
}

#[test]
fn generated_skill_lists_all_commands() {
    let out = bin()
        .args(["generate-skill", "--output", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let skill: serde_json::Value = serde_json::from_str(value["result"].as_str().unwrap()).unwrap();
    let names: Vec<String> = skill["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    for expected in [
        "codesafe brief",
        "codesafe optimize",
        "codesafe fix",
        "codesafe verify",
        "codesafe mutate",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}");
    }
}

#[test]
fn all_command_emits_a_single_json_envelope() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let out = bin()
        .args(["all", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8_lossy(&out);
    assert_eq!(
        text.lines().filter(|l| l.contains("\"ok\"")).count(),
        1,
        "all must emit exactly one envelope: {text}"
    );
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["meta"]["command"], "all");
    assert_eq!(value["result"]["report_written"], true);
}

#[test]
fn gen_test_write_creates_scaffold_and_preserves_source() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let before = std::fs::read_to_string(dir.path().join("src/demo.py")).unwrap();
    bin()
        .args(["gen-test", "--write", "--max-tests", "1"])
        .arg(dir.path())
        .assert()
        .success();
    let generated = std::fs::read_dir(dir.path().join("tests"))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().contains("_generated.py"));
    let generated = generated.expect("a generated scaffold must be written");
    let text = std::fs::read_to_string(generated.path()).unwrap();
    assert!(text.contains("Generated by codesafe"));
    assert!(!text.contains("assert True"));
    let after = std::fs::read_to_string(dir.path().join("src/demo.py")).unwrap();
    assert_eq!(before, after, "gen-test must never overwrite source files");
}

#[test]
fn red_rejects_tautological_tests_and_verify_accepts_real_ones() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("calc.py"),
        "def add(a, b):\n    return a + b\n",
    )
    .unwrap();
    // git repo so verify can compare against HEAD
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "T"],
        vec!["add", "-A"],
        vec!["commit", "-q", "-m", "base"],
    ] {
        std::process::Command::new("git")
            .args(["-C", dir.path().to_str().unwrap()])
            .args(&args)
            .status()
            .unwrap();
    }
    bin()
        .current_dir(dir.path())
        .args(["spec", "s1", "addition works", "."])
        .assert()
        .success();

    bin()
        .current_dir(dir.path())
        .args([
            "red",
            "s1",
            "test_add",
            "tests/test_calc.py",
            "def test_add():\n    assert True\n",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("anti-gaming"));

    bin()
        .current_dir(dir.path())
        .args([
            "red",
            "s1",
            "test_add",
            "tests/test_calc.py",
            "from calc import add\n\ndef test_add():\n    assert add(1, 2) == 3\n",
        ])
        .assert()
        .success();
    assert!(dir.path().join("tests/test_calc.py").is_file());

    bin()
        .current_dir(dir.path())
        .args(["verify", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"gaming_violations\": 0"));
}

#[test]
fn config_is_discovered_from_the_target_repo() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    std::fs::write(
        dir.path().join("codesafe.toml"),
        "fail_on = \"critical\"\njobs = 1\n",
    )
    .unwrap();
    let out = bin()
        .args(["test", "--output", "json"])
        .arg(dir.path())
        .assert()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["result"]["fail_on"], "critical");
}

#[test]
fn heal_converges_on_a_healthy_project() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let out = bin()
        .args(["heal", "--output", "json", "--max-retries", "2"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["result"]["result"]["converged"], true);
}

#[test]
fn mutate_reports_skip_when_no_tool_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    write_python_project(dir.path());
    let out = bin()
        .args(["mutate", "--output", "json"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["result"]["ran"], false);
    assert!(
        value["result"]["note"]
            .as_str()
            .unwrap()
            .contains("no mutation tool")
    );
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn fix_reverts_when_verification_fails_and_preserves_head() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture("rust"), dir.path());
    std::fs::create_dir_all(dir.path().join("rules")).unwrap();
    std::fs::write(
        dir.path().join("rules/rust.toml"),
        r#"
[[rules]]
id = "break-it"
language = "rust"
trigger = "none"
action = "sed -i 's/let mut total = 0u64;/let mut total = 1u64;/' src/main.rs"
safety = "safe"
explanation = "deliberately breaks behaviour for a safety test"
category = "lint"
"#,
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "T"],
        vec!["add", "-A"],
        vec!["commit", "-q", "-m", "base"],
    ] {
        std::process::Command::new("git")
            .args(["-C", dir.path().to_str().unwrap()])
            .args(&args)
            .status()
            .unwrap();
    }
    let head = std::process::Command::new("git")
        .args(["-C", dir.path().to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();

    bin()
        .arg("fix")
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("reverted"));

    let source = std::fs::read_to_string(dir.path().join("src/main.rs")).unwrap();
    assert!(source.contains("let mut total = 0u64;"));
    let head_after = std::process::Command::new("git")
        .args(["-C", dir.path().to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(head, String::from_utf8_lossy(&head_after.stdout).trim());
}
