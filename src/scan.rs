//! Language and build-system detection.
//!
//! Detection reads config files only - it never guesses commands.

use crate::Result;
use crate::types::{Language, ToolResult};
use std::path::{Path, PathBuf};

/// Build systems we can recognize per language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSystem {
    PythonPip,
    PythonPoetry,
    PythonSetuptools,
    RustCargo,
    CppCMake,
    CppMake,
    CppAutotools,
    CppUnknown,
}

impl std::fmt::Display for BuildSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildSystem::PythonPip => write!(f, "pip"),
            BuildSystem::PythonPoetry => write!(f, "poetry"),
            BuildSystem::PythonSetuptools => write!(f, "setuptools"),
            BuildSystem::RustCargo => write!(f, "cargo"),
            BuildSystem::CppCMake => write!(f, "cmake"),
            BuildSystem::CppMake => write!(f, "make"),
            BuildSystem::CppAutotools => write!(f, "autotools"),
            BuildSystem::CppUnknown => write!(f, "unknown"),
        }
    }
}

/// Detection result for one directory.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub path: PathBuf,
    pub languages: Vec<Language>,
    pub build_systems: Vec<BuildSystem>,
    pub source_files: Vec<PathBuf>,
    pub config_files: Vec<PathBuf>,
    pub tool_results: Vec<ToolResult>,
}

impl ScanResult {
    pub fn has_language(&self, lang: Language) -> bool {
        self.languages.contains(&lang)
    }
}

/// Available external tools per language.
#[derive(Debug, Clone, Default)]
pub struct AvailableTools {
    pub python: Vec<String>,
    pub rust: Vec<String>,
    pub cpp: Vec<String>,
}

/// Candidate config filenames that imply a language / build system.
const PYTHON_CONFIG_FILES: &[(&str, BuildSystem)] = &[
    ("pyproject.toml", BuildSystem::PythonPoetry),
    ("setup.py", BuildSystem::PythonSetuptools),
    ("setup.cfg", BuildSystem::PythonSetuptools),
    ("requirements.txt", BuildSystem::PythonPip),
    ("Pipfile", BuildSystem::PythonPip),
];

const RUST_CONFIG_FILES: &[(&str, BuildSystem)] = &[("Cargo.toml", BuildSystem::RustCargo)];

const CPP_CONFIG_FILES: &[(&str, BuildSystem)] = &[
    ("CMakeLists.txt", BuildSystem::CppCMake),
    ("Makefile", BuildSystem::CppMake),
    ("makefile", BuildSystem::CppMake),
    ("configure.ac", BuildSystem::CppAutotools),
    ("configure.in", BuildSystem::CppAutotools),
];

const PYTHON_SOURCE_EXTENSIONS: &[&str] = &["py"];
const RUST_SOURCE_EXTENSIONS: &[&str] = &["rs"];
const CPP_SOURCE_EXTENSIONS: &[&str] = &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"];

/// Scan a directory for languages, build systems, sources and available tools.
pub fn scan(path: &Path) -> Result<ScanResult> {
    if !path.exists() {
        return Err(crate::Error::NoLanguageDetected {
            path: path.display().to_string(),
        });
    }

    let mut config_files = Vec::new();
    let mut source_files = Vec::new();
    let mut build_systems = Vec::new();
    let mut languages = Vec::new();

    for entry in walkdir::WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        let file_name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if p.is_file() {
            // Config files imply build systems / languages.
            for (name, bs) in PYTHON_CONFIG_FILES {
                if file_name == *name {
                    config_files.push(p.to_path_buf());
                    if !build_systems.contains(bs) {
                        build_systems.push(*bs);
                    }
                    if !languages.contains(&Language::Python) {
                        languages.push(Language::Python);
                    }
                }
            }
            for (name, bs) in RUST_CONFIG_FILES {
                if file_name == *name {
                    config_files.push(p.to_path_buf());
                    if !build_systems.contains(bs) {
                        build_systems.push(*bs);
                    }
                    if !languages.contains(&Language::Rust) {
                        languages.push(Language::Rust);
                    }
                }
            }
            for (name, bs) in CPP_CONFIG_FILES {
                if file_name == *name {
                    config_files.push(p.to_path_buf());
                    if !build_systems.contains(bs) {
                        build_systems.push(*bs);
                    }
                    if !languages.contains(&Language::Cpp) {
                        languages.push(Language::Cpp);
                    }
                }
            }

            // Source files confirm languages.
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if PYTHON_SOURCE_EXTENSIONS.contains(&ext) {
                source_files.push(p.to_path_buf());
                push_lang(&mut languages, Language::Python);
            }
            if RUST_SOURCE_EXTENSIONS.contains(&ext) {
                source_files.push(p.to_path_buf());
                push_lang(&mut languages, Language::Rust);
            }
            if CPP_SOURCE_EXTENSIONS.contains(&ext) {
                source_files.push(p.to_path_buf());
                push_lang(&mut languages, Language::Cpp);
            }
        }
    }

    if languages.is_empty() && source_files.is_empty() {
        return Err(crate::Error::NoLanguageDetected {
            path: path.display().to_string(),
        });
    }

    source_files.sort();
    config_files.sort();
    languages.sort_by_key(|l| l.to_string());
    build_systems.sort_by_key(|b| b.to_string());

    let tools = detect_tools(&languages);
    let tool_results = flatten_tools(&tools);

    Ok(ScanResult {
        path: path.to_path_buf(),
        languages,
        build_systems,
        source_files,
        config_files,
        tool_results,
    })
}

fn push_lang(list: &mut Vec<Language>, lang: Language) {
    if !list.contains(&lang) {
        list.push(lang);
    }
}

pub(crate) fn is_ignored_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir()
        && matches!(
            entry.file_name().to_str(),
            Some(".git")
                | Some(".codeguard")
                | Some(".codesafe")
                | Some("target")
                | Some("node_modules")
                | Some(".venv")
                | Some("venv")
                | Some(".tox")
                | Some("__pycache__")
                | Some(".mypy_cache")
                | Some(".pytest_cache")
                | Some(".ruff_cache")
                | Some("build")
                | Some("dist")
                | Some("coverage")
                | Some("vendor")
                | Some("third_party")
        )
}

/// All files under `root` with one of the given extensions (without the dot).
/// Results are sorted so callers get deterministic behaviour and stable hashes.
pub fn files_with_ext(root: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| exts.contains(&ext))
        })
        .map(|e| e.into_path())
        .collect();
    files.sort();
    files
}

/// True if at least one file with one of the extensions exists. Short-circuits
/// the directory walk as soon as a match is found (cheaper than `files_with_ext`
/// when only presence matters, e.g. language detection).
pub fn has_files_with_ext(root: &Path, exts: &[&str]) -> bool {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .any(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| exts.contains(&ext))
        })
}

/// Extensions considered source code for a language.
pub fn source_extensions(language: Language) -> &'static [&'static str] {
    match language {
        Language::Python => &["py"],
        Language::Rust => &["rs"],
        Language::Cpp => &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"],
    }
}

/// All regular files under `root` (sorted), ignoring common build/cache dirs.
pub fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    files.sort();
    files
}

/// Match a repository-relative path against a simple glob.
///
/// Supports `*` (within one path segment), `**` (any number of segments), and
/// `?` (one character). This is intentionally small and dependency-free.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    fn matches(pat: &[u8], name: &[u8]) -> bool {
        if pat.is_empty() {
            return name.is_empty();
        }
        match pat[0] {
            b'*' => {
                if pat.len() > 1 && pat[1] == b'*' {
                    // `**` matches across separators.
                    let rest = &pat[2..];
                    let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                    if rest.is_empty() {
                        return true;
                    }
                    for i in 0..=name.len() {
                        if matches(rest, &name[i..]) {
                            return true;
                        }
                    }
                    false
                } else {
                    let rest = &pat[1..];
                    for i in 0..=name.len() {
                        if i > 0 && name[i - 1] == b'/' {
                            break;
                        }
                        if matches(rest, &name[i..]) {
                            return true;
                        }
                    }
                    false
                }
            }
            b'?' => !name.is_empty() && name[0] != b'/' && matches(&pat[1..], &name[1..]),
            c => !name.is_empty() && name[0] == c && matches(&pat[1..], &name[1..]),
        }
    }
    matches(pattern.as_bytes(), path.as_bytes())
}

/// True if any file under `files` matches any of the globs (empty globs = all).
pub fn any_glob_matches(root: &Path, globs: &[String], files: &[PathBuf]) -> bool {
    if globs.is_empty() {
        return true;
    }
    for file in files {
        let rel = relative_to(root, file);
        if globs.iter().any(|g| glob_matches(g, &rel)) {
            return true;
        }
    }
    false
}

/// Classify a path by its extension.
pub fn language_of_path(path: &Path) -> Option<Language> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    if source_extensions(Language::Python).contains(&ext) {
        Some(Language::Python)
    } else if source_extensions(Language::Rust).contains(&ext) {
        Some(Language::Rust)
    } else if source_extensions(Language::Cpp).contains(&ext) {
        Some(Language::Cpp)
    } else {
        None
    }
}

/// Path relative to `root` (falls back to the original path when it is outside).
pub fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

/// Tool names we look for per language.
fn language_tools(lang: Language) -> Vec<&'static str> {
    match lang {
        Language::Python => vec![
            "python3",
            "python",
            "pytest",
            "ruff",
            "mypy",
            "bandit",
            "hypothesis",
            "py-spy",
            "cProfile",
        ],
        Language::Rust => vec![
            "cargo",
            "rustc",
            "rustfmt",
            "clippy-driver",
            "cargo-audit",
            "proptest",
            "criterion",
            "flamegraph",
            "perf",
            "miri",
        ],
        Language::Cpp => vec![
            "clang++",
            "g++",
            "cmake",
            "make",
            "clang-format",
            "clang-tidy",
            "cppcheck",
            "clang",
            "ASan",
            "UBSan",
            "TSan",
            "libFuzzer",
            "google-benchmark",
            "perf",
            "valgrind",
        ],
    }
}

/// Check which tools exist on PATH.
fn detect_tools(languages: &[Language]) -> AvailableTools {
    let mut out = AvailableTools::default();
    for lang in languages {
        for tool in language_tools(*lang) {
            match lang {
                Language::Python => {
                    if which::which(tool).is_ok() {
                        out.python.push(tool.to_string());
                    }
                }
                Language::Rust => {
                    if which::which(tool).is_ok() {
                        out.rust.push(tool.to_string());
                    }
                }
                Language::Cpp => {
                    if which::which(tool).is_ok() {
                        out.cpp.push(tool.to_string());
                    }
                }
            }
        }
    }
    out
}

fn flatten_tools(tools: &AvailableTools) -> Vec<ToolResult> {
    let mut results = Vec::new();
    for t in &tools.python {
        results.push(tool_result(t, true));
    }
    for t in &tools.rust {
        results.push(tool_result(t, true));
    }
    for t in &tools.cpp {
        results.push(tool_result(t, true));
    }
    results
}

fn tool_result(tool: &str, found: bool) -> ToolResult {
    if found {
        ToolResult {
            tool: tool.to_string(),
            command: format!("which {tool}"),
            exit_code: Some(0),
            stdout: format!("{tool} found on PATH"),
            stderr: String::new(),
            duration_ms: 0,
            skipped: false,
            skip_reason: None,
        }
    } else {
        ToolResult {
            tool: tool.to_string(),
            command: format!("which {tool}"),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            skipped: true,
            skip_reason: Some("not found on PATH".into()),
        }
    }
}

/// Render the scan result as a human-readable summary.
pub fn render(scan: &ScanResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("codesafe scan: {}\n", scan.path.display()));
    out.push_str(&format!(
        "  languages: {}\n",
        languages_str(&scan.languages)
    ));
    out.push_str(&format!(
        "  build systems: {}\n",
        build_systems_str(&scan.build_systems)
    ));
    out.push_str(&format!("  source files: {}\n", scan.source_files.len()));
    out.push_str(&format!("  config files: {}\n", scan.config_files.len()));
    out.push_str("\n  available tools:\n");
    let tools = detect_tools(&scan.languages);
    if tools.python.is_empty() && tools.rust.is_empty() && tools.cpp.is_empty() {
        out.push_str("    (none found on PATH)\n");
    } else {
        if !tools.python.is_empty() {
            out.push_str(&format!("    python: {}\n", tools.python.join(", ")));
        }
        if !tools.rust.is_empty() {
            out.push_str(&format!("    rust: {}\n", tools.rust.join(", ")));
        }
        if !tools.cpp.is_empty() {
            out.push_str(&format!("    cpp: {}\n", tools.cpp.join(", ")));
        }
    }
    out
}

fn languages_str(langs: &[Language]) -> String {
    if langs.is_empty() {
        "none detected".into()
    } else {
        langs
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn build_systems_str(bss: &[BuildSystem]) -> String {
    if bss.is_empty() {
        "none detected".into()
    } else {
        bss.iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_python_repo() {
        let dir = test_dir("scan-python");
        std::fs::write(dir.join("pyproject.toml"), b"[project]\nname = \"x\"\n").unwrap();
        std::fs::write(dir.join("main.py"), b"print(1)\n").unwrap();
        let res = scan(&dir).unwrap();
        assert!(res.has_language(Language::Python));
        assert!(res.build_systems.contains(&BuildSystem::PythonPoetry));
        assert!(!res.source_files.is_empty());
    }

    #[test]
    fn detects_rust_repo() {
        let dir = test_dir("scan-rust");
        std::fs::write(
            dir.join("Cargo.toml"),
            b"[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("lib.rs"), b"fn main() {}\n").unwrap();
        let res = scan(&dir).unwrap();
        assert!(res.has_language(Language::Rust));
        assert!(res.build_systems.contains(&BuildSystem::RustCargo));
    }

    #[test]
    fn detects_cpp_repo() {
        let dir = test_dir("scan-cpp");
        std::fs::write(
            dir.join("CMakeLists.txt"),
            b"cmake_minimum_required(VERSION 3.10)\n",
        )
        .unwrap();
        std::fs::write(dir.join("main.cpp"), b"int main() { return 0; }\n").unwrap();
        let res = scan(&dir).unwrap();
        assert!(res.has_language(Language::Cpp));
        assert!(res.build_systems.contains(&BuildSystem::CppCMake));
    }

    #[test]
    fn errors_on_empty_dir() {
        let dir = test_dir("scan-empty");
        let err = scan(&dir).unwrap_err();
        assert!(matches!(err, crate::Error::NoLanguageDetected { .. }));
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("codeguard-scan-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
