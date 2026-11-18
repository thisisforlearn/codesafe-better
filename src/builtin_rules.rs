//! Built-in rule packs.
//!
//! These rules are used when a repository has no `rules/` directory, and are
//! also written by `codesafe init`. They only orchestrate tools that the user
//! already has installed; rules with `requires` are skipped when the tool is
//! missing, so the fix loop never fails because of an optional tool.
//!
//! Rules marked `safe` are behaviour-preserving by construction (formatters and
//! linters' safe autofixes) and may be auto-applied. Everything else is
//! report-only (`review`), so a coding agent gets guidance without silently
//! rewriting semantics.

use crate::types::{Language, Rule, RuleSafety};

#[allow(clippy::too_many_arguments)]
fn rule(
    id: &str,
    language: Language,
    trigger: &str,
    action: &str,
    safety: RuleSafety,
    category: &str,
    globs: &[&str],
    requires: &[&str],
    explanation: &str,
) -> Rule {
    Rule {
        id: id.to_string(),
        language,
        trigger: trigger.to_string(),
        action: action.to_string(),
        safety,
        explanation: explanation.to_string(),
        globs: globs.iter().map(|g| g.to_string()).collect(),
        requires: requires.iter().map(|t| t.to_string()).collect(),
        timeout_s: None,
        category: Some(category.to_string()),
        severity: None,
    }
}

/// Full built-in rule pack.
pub fn default_rules() -> Vec<Rule> {
    let mut rules = Vec::with_capacity(64);

    // ---- Python: safe, behaviour-preserving ----
    rules.push(rule(
        "py-ruff-format",
        Language::Python,
        "ruff",
        "ruff format .",
        RuleSafety::Safe,
        "format",
        &["**/*.py"],
        &["ruff"],
        "Format Python sources with ruff (AST-preserving formatter).",
    ));
    rules.push(rule(
        "py-ruff-imports",
        Language::Python,
        "ruff:I001",
        "ruff check --select I --fix .",
        RuleSafety::Safe,
        "imports",
        &["**/*.py"],
        &["ruff"],
        "Sort and deduplicate imports (isort-compatible, behaviour-preserving).",
    ));
    rules.push(rule(
        "py-ruff-upgrade",
        Language::Python,
        "ruff:UP",
        "ruff check --select UP --fix .",
        RuleSafety::Safe,
        "modernize",
        &["**/*.py"],
        &["ruff"],
        "Apply pyupgrade-safe syntax modernizations (behaviour-preserving).",
    ));
    rules.push(rule(
        "py-ruff-fix",
        Language::Python,
        "ruff",
        "ruff check --fix .",
        RuleSafety::Safe,
        "lint",
        &["**/*.py"],
        &["ruff"],
        "Apply ruff's safe autofixes (unused imports, shadowed names, etc.).",
    ));
    rules.push(rule(
        "py-black",
        Language::Python,
        "black",
        "black --quiet .",
        RuleSafety::Safe,
        "format",
        &["**/*.py"],
        &["black"],
        "Format with Black (AST-equivalent output).",
    ));
    rules.push(rule(
        "py-isort",
        Language::Python,
        "isort",
        "isort --quiet .",
        RuleSafety::Safe,
        "imports",
        &["**/*.py"],
        &["isort"],
        "Sort imports with isort.",
    ));
    rules.push(rule(
        "py-autoflake",
        Language::Python,
        "autoflake",
        "autoflake --in-place --remove-all-unused-imports --remove-unused-variables -r .",
        RuleSafety::Safe,
        "dead-code",
        &["**/*.py"],
        &["autoflake"],
        "Remove unused imports and variables with autoflake.",
    ));

    // ---- Python: review-only ----
    rules.push(rule(
        "py-ruff-unsafe-fix",
        Language::Python,
        "ruff",
        "ruff check --fix --unsafe-fixes .",
        RuleSafety::Review,
        "lint",
        &["**/*.py"],
        &["ruff"],
        "Ruff unsafe fixes can change behaviour; review the diff before applying.",
    ));
    rules.push(rule(
        "py-mypy",
        Language::Python,
        "mypy",
        "mypy .",
        RuleSafety::Review,
        "types",
        &["**/*.py"],
        &["mypy"],
        "Static type checking. Fix reported type errors without narrowing public APIs.",
    ));
    rules.push(rule(
        "py-bandit",
        Language::Python,
        "bandit",
        "bandit -q -r .",
        RuleSafety::Review,
        "security",
        &["**/*.py"],
        &["bandit"],
        "Security lint (eval/exec, weak crypto, shell injection).",
    ));
    rules.push(rule(
        "py-vulture",
        Language::Python,
        "vulture",
        "vulture .",
        RuleSafety::Review,
        "dead-code",
        &["**/*.py"],
        &["vulture"],
        "Find dead code. Delete only symbols that are provably unreferenced.",
    ));
    rules.push(rule(
        "py-pytest",
        Language::Python,
        "pytest",
        "python3 -m pytest -q",
        RuleSafety::Review,
        "tests",
        &["**/*.py"],
        &["pytest"],
        "Run the test suite; generated diagnostics point at failing tests.",
    ));
    rules.push(rule(
        "py-radon",
        Language::Python,
        "radon",
        "radon cc -s -a .",
        RuleSafety::Review,
        "complexity",
        &["**/*.py"],
        &["radon"],
        "Cyclomatic-complexity report to prioritize refactors.",
    ));

    // ---- Rust: safe ----
    rules.push(rule(
        "rs-fmt",
        Language::Rust,
        "rustfmt",
        "cargo fmt --all",
        RuleSafety::Safe,
        "format",
        &["**/*.rs"],
        &["cargo"],
        "Format all Rust sources with rustfmt (AST-preserving).",
    ));
    rules.push(rule(
        "rs-clippy-fix",
        Language::Rust,
        "clippy",
        "cargo clippy --fix --all-targets --allow-dirty --allow-staged",
        RuleSafety::Safe,
        "lint",
        &["**/*.rs"],
        &["cargo"],
        "Apply clippy's machine-applicable suggestions (behaviour-preserving).",
    ));
    rules.push(rule(
        "rs-cargo-sort",
        Language::Rust,
        "cargo-sort",
        "cargo sort -w",
        RuleSafety::Safe,
        "manifest",
        &["**/Cargo.toml"],
        &["cargo", "cargo-sort"],
        "Sort and normalize Cargo.toml dependencies.",
    ));

    // ---- Rust: review ----
    rules.push(rule(
        "rs-clippy-pedantic",
        Language::Rust,
        "clippy::pedantic",
        "cargo clippy --all-targets -- -W clippy::pedantic",
        RuleSafety::Review,
        "lint",
        &["**/*.rs"],
        &["cargo"],
        "Pedantic clippy lints: high signal, but fixes may be stylistic.",
    ));
    rules.push(rule(
        "rs-audit",
        Language::Rust,
        "cargo-audit",
        "cargo audit",
        RuleSafety::Review,
        "security",
        &["**/Cargo.lock"],
        &["cargo", "cargo-audit"],
        "Scan Cargo.lock for known vulnerabilities (RUSTSEC advisories).",
    ));
    rules.push(rule(
        "rs-deny",
        Language::Rust,
        "cargo-deny",
        "cargo deny check",
        RuleSafety::Review,
        "security",
        &["**/Cargo.toml"],
        &["cargo", "cargo-deny"],
        "License, advisory, ban and source policy checks.",
    ));
    rules.push(rule(
        "rs-udeps",
        Language::Rust,
        "cargo-udeps",
        "cargo +nightly udeps",
        RuleSafety::Review,
        "dead-code",
        &["**/Cargo.toml"],
        &["cargo"],
        "Find unused dependencies (nightly toolchain required).",
    ));
    rules.push(rule(
        "rs-miri",
        Language::Rust,
        "miri",
        "cargo +nightly miri test",
        RuleSafety::Review,
        "soundness",
        &["**/*.rs"],
        &["cargo"],
        "Run tests under Miri to catch undefined behaviour in unsafe code.",
    ));
    rules.push(rule(
        "rs-nextest",
        Language::Rust,
        "nextest",
        "cargo nextest run",
        RuleSafety::Review,
        "tests",
        &["**/*.rs"],
        &["cargo", "cargo-nextest"],
        "Run tests with nextest for faster, better-isolated failures.",
    ));

    // ---- C++: safe ----
    rules.push(rule(
        "cpp-clang-format",
        Language::Cpp,
        "clang-format",
        r#"find . -type f \( -name '*.cpp' -o -name '*.cc' -o -name '*.cxx' -o -name '*.c' -o -name '*.h' -o -name '*.hpp' -o -name '*.hxx' \) -not -path './build/*' -not -path './.git/*' -exec clang-format -i {} +"#,
        RuleSafety::Safe,
        "format",
        &["**/*.cpp", "**/*.hpp", "**/*.h", "**/*.cc", "**/*.cxx"],
        &["clang-format"],
        "Format all C/C++ sources with clang-format (token-preserving).",
    ));
    rules.push(rule(
        "cpp-cmake-format",
        Language::Cpp,
        "cmake-format",
        "cmake-format -i CMakeLists.txt",
        RuleSafety::Safe,
        "format",
        &["**/CMakeLists.txt"],
        &["cmake-format"],
        "Format CMakeLists.txt (comments and commands preserved).",
    ));

    // ---- C++: review ----
    rules.push(rule(
        "cpp-clang-tidy",
        Language::Cpp,
        "clang-tidy",
        "clang-tidy -p build $(find . -name '*.cpp' -not -path './build/*')",
        RuleSafety::Review,
        "lint",
        &["**/*.cpp", "**/*.cc", "**/*.cxx"],
        &["clang-tidy"],
        "Static analysis for bugprone patterns and modernize opportunities.",
    ));
    rules.push(rule(
        "cpp-cppcheck",
        Language::Cpp,
        "cppcheck",
        "cppcheck --enable=warning,performance,portability --quiet .",
        RuleSafety::Review,
        "lint",
        &["**/*.cpp", "**/*.cc", "**/*.cxx", "**/*.h", "**/*.hpp"],
        &["cppcheck"],
        "Portable static analysis; review each finding against real invariants.",
    ));
    rules.push(rule(
        "cpp-iwyu",
        Language::Cpp,
        "include-what-you-use",
        "include-what-you-use -p build $(find . -name '*.cpp' -not -path './build/*')",
        RuleSafety::Review,
        "includes",
        &["**/*.cpp"],
        &["include-what-you-use"],
        "Suggest explicit includes; review transitive-include assumptions first.",
    ));
    rules.push(rule(
        "cpp-valgrind",
        Language::Cpp,
        "valgrind",
        "ctest --test-dir build --output-on-failure",
        RuleSafety::Review,
        "memory",
        &["**/*.cpp"],
        &["valgrind"],
        "Run the test suite; use valgrind for memory/thread diagnostics.",
    ));
    // ---- Cross-language: mutation testing (review) ----
    rules.push(rule(
        "rs-mutants",
        Language::Rust,
        "cargo-mutants",
        "cargo mutants --timeout 60",
        RuleSafety::Review,
        "mutation",
        &["**/*.rs"],
        &["cargo", "cargo-mutants"],
        "Introduce deliberate faults and report mutants that no test kills.",
    ));
    rules.push(rule(
        "py-mutmut",
        Language::Python,
        "mutmut",
        "mutmut run",
        RuleSafety::Review,
        "mutation",
        &["**/*.py"],
        &["mutmut"],
        "Mutation-test the Python suite; surviving mutants are follow-up test tasks.",
    ));

    rules.push(rule(
        "cpp-sanitizers",
        Language::Cpp,
        "sanitizer",
        "cmake --build build --target all",
        RuleSafety::Review,
        "memory",
        &["**/CMakeLists.txt"],
        &["cmake"],
        "Build with ASan/UBSan/TSan to catch undefined behaviour.",
    ));

    rules
}

/// Rules for a single language, sorted by id for deterministic output.
pub fn rules_for(language: Language) -> Vec<Rule> {
    let mut rules: Vec<Rule> = default_rules()
        .into_iter()
        .filter(|r| r.language == language)
        .collect();
    rules.sort_by(|a, b| a.id.cmp(&b.id));
    rules
}

/// Render rules as a `[[rules]]` TOML document. Used by `codesafe init`.
pub fn rules_toml(language: Language) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# codesafe rules for {language}\n#\n# safety = \"safe\"   -> auto-applied and verified\n# safety = \"review\" -> reported for manual/agent review only\n# globs/requires     -> optional filters; missing tools are skipped\n\n"
    ));
    for r in rules_for(language) {
        out.push_str("[[rules]]\n");
        out.push_str(&format!("id = {}\n", toml_string(&r.id)));
        out.push_str(&format!(
            "language = {}\n",
            toml_string(&r.language.to_string())
        ));
        out.push_str(&format!("trigger = {}\n", toml_string(&r.trigger)));
        out.push_str(&format!("action = {}\n", toml_string(&r.action)));
        out.push_str(&format!(
            "safety = {}\n",
            toml_string(&r.safety.to_string())
        ));
        out.push_str(&format!("explanation = {}\n", toml_string(&r.explanation)));
        if !r.category.as_deref().unwrap_or("").is_empty() {
            out.push_str(&format!(
                "category = {}\n",
                toml_string(r.category.as_deref().unwrap_or("lint"))
            ));
        }
        if !r.globs.is_empty() {
            let globs = r
                .globs
                .iter()
                .map(|g| toml_string(g))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("globs = [{globs}]\n"));
        }
        if !r.requires.is_empty() {
            let req = r
                .requires
                .iter()
                .map(|t| toml_string(t))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("requires = [{req}]\n"));
        }
        out.push('\n');
    }
    out
}

fn toml_string(value: &str) -> String {
    // TOML basic strings: escape backslashes, quotes and control characters.
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_pack_covers_all_languages_and_has_many_rules() {
        let rules = default_rules();
        assert!(rules.len() >= 25, "expected a comprehensive pack");
        for lang in [Language::Python, Language::Rust, Language::Cpp] {
            assert!(
                rules
                    .iter()
                    .any(|r| r.language == lang && r.safety == RuleSafety::Safe)
            );
            assert!(
                rules
                    .iter()
                    .any(|r| r.language == lang && r.safety == RuleSafety::Review)
            );
        }
    }

    #[test]
    fn rule_ids_are_unique() {
        let rules = default_rules();
        let mut ids: Vec<_> = rules.iter().map(|r| r.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), rules.len());
    }

    #[test]
    fn toml_round_trips() {
        for lang in [Language::Python, Language::Rust, Language::Cpp] {
            let text = rules_toml(lang);
            let parsed: RulesFile = toml::from_str(&text).expect("generated TOML must parse");
            assert_eq!(parsed.rules, rules_for(lang));
        }
    }

    #[derive(serde::Deserialize)]
    struct RulesFile {
        rules: Vec<Rule>,
    }

    impl PartialEq for Rule {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id && self.action == other.action && self.safety == other.safety
        }
    }
}
