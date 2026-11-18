//! Deterministic robustness tests: parsers and analyzers must never panic on
//! hostile, truncated, or random input (tool output is untrusted input).

use codeguard::{context, diagnostics, scan, types, verification};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[(self.next() as usize) % items.len()]
    }
}

fn random_text(seed: u64, len: usize) -> String {
    let alphabet = [
        "a",
        "Z",
        "0",
        "9",
        " ",
        "\n",
        "\t",
        ":",
        "/",
        "\\",
        ".",
        "-",
        "_",
        "[",
        "]",
        "(",
        ")",
        "{",
        "}",
        "'",
        "\"",
        "%",
        "#",
        ";",
        ",",
        "=",
        "!",
        "?",
        "|",
        "\u{00e9}",
        "\u{1f600}",
        "error:",
        "warning:",
        "SyntaxError",
        "assert",
        "mock",
        "fn ",
        "def ",
        "struct ",
    ];
    let mut rng = Lcg(seed);
    let mut out = String::new();
    for _ in 0..len {
        out.push_str(rng.pick(&alphabet));
    }
    out
}

#[test]
fn parsers_never_panic_on_random_input() {
    for seed in 1..=400u64 {
        let text = random_text(seed, 60);
        let _ = diagnostics::parse_ruff(&text, "trace");
        let _ = diagnostics::parse_pytest(&text, "trace");
        let _ = diagnostics::parse_python_compiler(&text, "trace");
        let _ = diagnostics::parse_generic_compiler(&text, "gcc", "trace");
        let _ = diagnostics::parse_mypy(&text, &text, "trace");
        let _ = diagnostics::parse_bandit(&text, &text, "trace");
        let _ = diagnostics::parse_cargo_json(&text, "trace");
        let _ = codeguard::adapters::parse_perf_report(&text);
        let _ = codeguard::adapters::parse_raw_profile(&text);
        let _ = verification::check_tautological(&text);
        let _ = verification::check_test_integrity(&text, &text);
        let _ = verification::check_stub_passability(&text, "unit");
    }
}

#[test]
fn context_analysis_never_panics_on_random_sources() {
    let dir = tempfile::tempdir().unwrap();
    let exts = ["py", "rs", "cpp"];
    for seed in 1..=120u64 {
        for ext in exts {
            let file = format!("fuzz.{ext}");
            let text = random_text(seed, 80);
            std::fs::write(dir.path().join(&file), &text).unwrap();
            let ctx = context::analyze(dir.path(), &file, Some((seed % 90) as u32));
            assert_eq!(ctx.file, file);
        }
    }
}

#[test]
fn glob_never_panics_and_handles_edge_patterns() {
    let patterns = [
        "",
        "*",
        "**",
        "**/",
        "/",
        "***",
        "src/*.rs",
        "src/**/x.rs",
        "**/*.py",
        "a?c",
        "?",
        "\\",
        ":.git",
        "src/**/**/a.rs",
    ];
    let names = [
        "",
        "/",
        "a",
        "src/a.rs",
        "src/x/y/z/a.rs",
        ".git/config",
        "a\u{00e9}c",
        "a/b/c",
        "**",
        "src//a.rs",
    ];
    for p in patterns {
        for n in names {
            let _ = scan::glob_matches(p, n);
        }
    }
    assert!(scan::glob_matches("*", "anything"));
    assert!(scan::glob_matches("**", "a/b/c"));
    assert!(scan::glob_matches("a?c", "abc"));
    assert!(!scan::glob_matches("a?c", "a/c"));
    assert!(!scan::glob_matches("src/*.rs", "src/a/b.rs"));
}

#[test]
fn hashing_is_order_independent_and_content_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, b"alpha").unwrap();
    std::fs::write(&b, b"beta").unwrap();
    let h1 = types::hash_files(&[a.clone(), b.clone()]).unwrap();
    let h2 = types::hash_files(&[b.clone(), a.clone()]).unwrap();
    assert_eq!(h1, h2);
    std::fs::write(&a, b"alpha changed").unwrap();
    let h3 = types::hash_files(&[a, b]).unwrap();
    assert_ne!(h1, h3);
}

#[test]
fn empty_and_binary_files_are_handled() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.py");
    let binary = dir.path().join("bin.rs");
    std::fs::write(&empty, b"").unwrap();
    std::fs::write(&binary, [0u8, 159, 146, 150, 255, 0, 1]).unwrap();
    let _ = context::analyze(dir.path(), "empty.py", Some(1));
    let _ = context::analyze(dir.path(), "bin.rs", Some(1));
    let _ = types::hash_files(&[empty, binary]).unwrap();
}

#[test]
fn optimizer_and_coverage_never_panic_on_random_sources() {
    let dir = tempfile::tempdir().unwrap();
    for seed in 1..=80u64 {
        for ext in ["py", "rs", "cpp"] {
            let rel = format!("src/f{seed}.{ext}");
            let path = dir.path().join(&rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, random_text(seed, 120)).unwrap();
            let language = scan::language_of_path(&path).unwrap();
            let _ = codeguard::optimizer::analyze_file(dir.path(), &path, language);
        }
    }
    let _ = codeguard::test_generation::static_coverage(dir.path());
}
