//! Tests against realistic fixture repositories: location intelligence,
//! optimization detection, static coverage, and rule discovery.

use codeguard::{context, optimizer, scan, test_generation};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn finds_python_bug_location_in_correct_folder_and_symbol() {
    let root = fixture("python/buggy_pkg");
    let file = "src/buggy_pkg/app.py";
    let ctx = context::analyze(&root, file, Some(25));
    assert_eq!(ctx.file, file);
    assert_eq!(ctx.folder, "src/buggy_pkg");
    assert_eq!(ctx.symbol.as_deref(), Some("parse_config"));
    assert!(ctx.snippet.contains("open(path)"));
    assert!(
        ctx.affected_area
            .as_deref()
            .unwrap()
            .contains("parse_config")
    );
    assert!(ctx.imports.iter().any(|i| i.contains("import os")));
}

#[test]
fn optimizer_detects_real_world_python_anti_patterns() {
    let root = fixture("python/buggy_pkg");
    let file = root.join("src/buggy_pkg/app.py");
    let opts = optimizer::analyze_file(&root, &file, codeguard::types::Language::Python);
    let ids: Vec<&str> = opts
        .iter()
        .map(|o| o.id.split(':').next().unwrap())
        .collect();
    for expected in [
        "py-enumerate",
        "py-context-manager",
        "py-mutable-default",
        "py-nested-loop",
        "py-blocking-sleep",
    ] {
        assert!(ids.contains(&expected), "missing {expected}; got {ids:?}");
    }
    for opt in &opts {
        assert!(
            opt.why_safe.split_whitespace().count() > 3,
            "why_safe must be a sentence, got {:?}",
            opt.why_safe
        );
        assert!(!opt.how_to_apply.is_empty());
        assert!(!opt.how_to_verify.is_empty());
        assert!(opt.score > 0.0 && opt.score <= 1.0);
    }
}

#[test]
fn static_coverage_identifies_untested_functions() {
    let root = fixture("python/buggy_pkg");
    let coverage = test_generation::static_coverage(&root);
    let names: Vec<&str> = coverage.iter().map(|c| c.function.as_str()).collect();
    assert!(names.contains(&"slow_sum"));
    assert!(names.contains(&"parse_config"));
    // slow_sum is referenced by the fixture test suite; rate_limit is not.
    let rate = coverage
        .iter()
        .find(|c| c.function == "rate_limit")
        .unwrap();
    assert_eq!(rate.covered_lines, 0);
    assert!(rate.complexity >= 2);
    let hotspots = test_generation::identify_hotspots(&coverage, 1, 50.0);
    assert!(hotspots.iter().any(|h| h.function == "rate_limit"));
}

#[test]
fn scan_detects_all_three_language_fixtures() {
    for (dir, language) in [
        ("python/buggy_pkg", "python"),
        ("rust", "rust"),
        ("cpp", "cpp"),
    ] {
        let result = scan::scan(&fixture(dir)).unwrap();
        assert!(
            result.languages.iter().any(|l| l.to_string() == language),
            "{dir} should detect {language}"
        );
    }
}

#[test]
fn cpp_context_maps_line_to_function_and_block() {
    let root = fixture("cpp");
    let ctx = context::analyze(&root, "main.cpp", Some(4));
    assert_eq!(ctx.symbol.as_deref(), Some("slow_sum"));
    assert!(ctx.enclosing.contains("int slow_sum"));
    assert!(ctx.enclosing.contains("return t"));
}

#[test]
fn glob_matching_covers_real_rule_patterns() {
    assert!(scan::glob_matches("**/*.py", "src/a/b.py"));
    assert!(scan::glob_matches("**/*.py", "a.py"));
    assert!(!scan::glob_matches("**/*.py", "a.rs"));
    assert!(scan::glob_matches("**/Cargo.toml", "crates/x/Cargo.toml"));
    assert!(scan::glob_matches("src/**/*.rs", "src/a/b.rs"));
    assert!(scan::glob_matches("**/*.cpp", "main.cpp"));
}
