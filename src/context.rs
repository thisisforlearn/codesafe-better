//! Source-context intelligence for agentic fixes.
//!
//! Given a file and an optional line number, this module describes *where* a
//! problem lives and *what* the surrounding code is responsible for:
//!
//! - repository-relative folder and file
//! - the enclosing function/class/impl/module symbol
//! - a numbered source snippet around the location
//! - the enclosing block body (bounded)
//! - a short module summary and the import list
//!
//! The result is attached to findings so a coding agent receives enough context
//! to make a targeted, low-risk change without extra repository exploration.

use crate::types::{Finding, Language};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Maximum file size we are willing to read for context extraction.
const MAX_CONTEXT_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum number of lines emitted for an enclosing block.
const MAX_BLOCK_LINES: usize = 80;
/// Lines of context on each side of the reported location.
const SNIPPET_RADIUS: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceContext {
    pub file: String,
    pub folder: String,
    pub language: Option<Language>,
    pub line: Option<u32>,
    pub symbol: Option<String>,
    pub symbol_kind: Option<String>,
    pub affected_area: Option<String>,
    pub module_summary: String,
    pub imports: Vec<String>,
    pub snippet: String,
    pub enclosing: String,
}

impl SourceContext {
    pub fn is_empty(&self) -> bool {
        self.file.is_empty()
    }
}

/// Build source context for `file` (relative to `root`, or absolute).
pub fn analyze(root: &Path, file: &str, line: Option<u32>) -> SourceContext {
    if file.trim().is_empty() {
        return SourceContext::default();
    }
    let path = resolve_path(root, file);
    let Some(language) = crate::scan::language_of_path(&path) else {
        return SourceContext {
            file: crate::scan::relative_to(root, &path),
            folder: folder_of(root, &path),
            ..Default::default()
        };
    };
    match read_text(&path) {
        Some(text) => analyze_text(root, &path, &text, language, line),
        None => SourceContext {
            file: crate::scan::relative_to(root, &path),
            folder: folder_of(root, &path),
            language: Some(language),
            line,
            ..Default::default()
        },
    }
}

fn resolve_path(root: &Path, file: &str) -> PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        // Prefer the path relative to root when the absolute path is inside it.
        if let Ok(stripped) = p.strip_prefix(root) {
            return root.join(stripped);
        }
        return p.to_path_buf();
    }
    root.join(p)
}

fn folder_of(root: &Path, path: &Path) -> String {
    let rel = crate::scan::relative_to(root, path);
    match rel.rfind('/') {
        Some(idx) => rel[..idx].to_string(),
        None => ".".to_string(),
    }
}

fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_CONTEXT_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn analyze_text(
    root: &Path,
    path: &Path,
    text: &str,
    language: Language,
    line: Option<u32>,
) -> SourceContext {
    let lines: Vec<&str> = text.lines().collect();
    let target_idx = line.and_then(|l| {
        if l == 0 || lines.is_empty() {
            return None;
        }
        let idx = (l as usize).min(lines.len());
        if idx == 0 { None } else { Some(idx - 1) }
    });

    let symbol = target_idx
        .and_then(|idx| find_enclosing_symbol(&lines, language, idx))
        .or_else(|| find_first_symbol(&lines, language));

    let (symbol_name, symbol_kind, symbol_start) = match symbol {
        Some(s) => (Some(s.name.clone()), Some(s.kind.clone()), Some(s.start)),
        None => (None, None, None),
    };

    let enclosing = symbol_start
        .map(|start| extract_block(&lines, language, start, target_idx))
        .unwrap_or_default();

    let snippet = target_idx
        .map(|idx| render_snippet(&lines, idx))
        .unwrap_or_default();

    let folder = folder_of(root, path);
    let affected_area = symbol_name.as_ref().map(|name| {
        format!(
            "{} `{}` in {}",
            symbol_kind.as_deref().unwrap_or("symbol"),
            name,
            folder
        )
    });

    SourceContext {
        file: crate::scan::relative_to(root, path),
        folder,
        language: Some(language),
        line,
        symbol: symbol_name,
        symbol_kind,
        affected_area,
        module_summary: module_summary(&lines, language),
        imports: imports(&lines),
        snippet,
        enclosing,
    }
}

/// Attach folder/symbol/snippet/impact to a finding in place.
pub fn enrich_finding(root: &Path, finding: &mut Finding) {
    let ctx = analyze(root, &finding.file, finding.line);
    if !ctx.file.is_empty() {
        finding.file = ctx.file.clone();
    }
    if finding.folder.is_empty() {
        finding.folder = ctx.folder.clone();
    }
    if finding.symbol.is_none() {
        finding.symbol = ctx.symbol.clone();
    }
    if finding.snippet.is_empty() {
        finding.snippet = if ctx.snippet.is_empty() {
            ctx.enclosing.clone()
        } else {
            ctx.snippet.clone()
        };
    }
    if finding.affected_area.is_none() {
        finding.affected_area = ctx.affected_area.clone().or_else(|| {
            if ctx.module_summary.is_empty() {
                None
            } else {
                Some(ctx.module_summary.clone())
            }
        });
    }
    if finding.impact.is_none() {
        finding.impact = Some(infer_impact(finding));
    }
    if finding.fix_risk.is_none() {
        finding.fix_risk = Some(infer_fix_risk(finding));
    }
    if finding.context_lines.is_empty() && !finding.snippet.is_empty() {
        finding.context_lines = finding.snippet.lines().map(|l| l.to_string()).collect();
    }
}

fn infer_impact(finding: &Finding) -> String {
    let category = finding.category.to_lowercase();
    let error_type = finding
        .error_type
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    if category.contains("security") || error_type.contains("bandit") {
        "potential security weakness; input validation and data handling are affected".to_string()
    } else if category.contains("test") || error_type.contains("test") {
        "the test suite cannot certify this behaviour, so regressions can ship unnoticed"
            .to_string()
    } else if category.contains("build") || error_type.contains("compile") {
        "the project does not build; every downstream task is blocked".to_string()
    } else if error_type.contains("type") || category.contains("type") {
        "type contract is violated; callers may receive unexpected values at runtime".to_string()
    } else if category.contains("format") {
        "style drift only; behaviour is unchanged but review/diff noise increases".to_string()
    } else {
        "the reported behaviour in this code path is unreliable until the finding is resolved"
            .to_string()
    }
}

fn infer_fix_risk(finding: &Finding) -> String {
    if finding.fix.is_some() {
        if finding.category.contains("format") {
            "low: formatter output is behaviour-preserving; verify with the test suite".to_string()
        } else {
            "low-medium: automated fix is available; run tests before keeping the change"
                .to_string()
        }
    } else {
        "medium: no automatic fix; change the smallest possible surface and re-run tests"
            .to_string()
    }
}

#[derive(Debug, Clone)]
struct Symbol {
    name: String,
    kind: String,
    indent: i32,
    start: usize,
}

fn find_first_symbol(lines: &[&str], language: Language) -> Option<Symbol> {
    for (idx, _) in lines.iter().enumerate() {
        if let Some(sym) = symbol_at(lines, language, idx) {
            return Some(sym);
        }
    }
    None
}

fn find_enclosing_symbol(lines: &[&str], language: Language, target: usize) -> Option<Symbol> {
    let target_indent = match language {
        Language::Python => effective_indent(lines, target),
        _ => 0,
    };
    for idx in (0..=target).rev() {
        if let Some(sym) = symbol_at(lines, language, idx) {
            match language {
                Language::Python => {
                    if sym.indent < target_indent || idx == target {
                        return Some(sym);
                    }
                }
                _ => return Some(sym),
            }
        }
    }
    None
}

fn symbol_at(lines: &[&str], language: Language, idx: usize) -> Option<Symbol> {
    let raw = lines.get(idx)?;
    let trimmed = raw.trim_start();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return None;
    }
    match language {
        Language::Python => {
            for keyword in ["async def", "def", "class"] {
                if let Some(rest) = trimmed.strip_prefix(keyword) {
                    if !rest.chars().next().is_none_or(|c| c.is_whitespace()) {
                        continue;
                    }
                    let name = rest
                        .trim_start()
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    return Some(Symbol {
                        name: name.to_string(),
                        kind: if keyword == "class" {
                            "class"
                        } else {
                            "function"
                        }
                        .to_string(),
                        indent: indent_of(raw),
                        start: idx,
                    });
                }
            }
            None
        }
        Language::Rust => {
            for (kw, kind) in [
                ("fn", "function"),
                ("struct", "struct"),
                ("enum", "enum"),
                ("trait", "trait"),
                ("mod", "module"),
                ("impl", "impl"),
            ] {
                if let Some(name) = name_after_keyword(trimmed, kw) {
                    return Some(Symbol {
                        name,
                        kind: kind.to_string(),
                        indent: indent_of(raw),
                        start: idx,
                    });
                }
            }
            None
        }
        Language::Cpp => {
            if trimmed.starts_with('#')
                || trimmed.starts_with("if ")
                || trimmed.starts_with("for ")
                || trimmed.starts_with("while ")
                || trimmed.starts_with("switch ")
                || trimmed.starts_with("return ")
            {
                return None;
            }
            let open = raw.find('(')?;
            let before = raw[..open].trim_end();
            if before.is_empty() || before.contains('=') || before.contains(';') {
                return None;
            }
            let name = before
                .rsplit(|c: char| c.is_whitespace() || c == ':' || c == '*' || c == '&' || c == '>')
                .next()
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                return None;
            }
            Some(Symbol {
                name: name.to_string(),
                kind: "function".to_string(),
                indent: indent_of(raw),
                start: idx,
            })
        }
    }
}

fn name_after_keyword(line: &str, kw: &str) -> Option<String> {
    let pos = find_keyword(line, kw)?;
    let rest = line[pos + kw.len()..].trim_start();
    if rest.starts_with('<') {
        let end = rest.find('>')?;
        let name = rest[..=end].trim();
        if name.is_empty() {
            return None;
        }
        return Some(name.to_string());
    }
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

/// Find `kw` as a whole word in a line.
fn find_keyword(line: &str, kw: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(pos) = line[start..].find(kw) {
        let abs = start + pos;
        let preceded_ok =
            abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric() && bytes[abs - 1] != b'_';
        let after = abs + kw.len();
        let followed_ok =
            after >= bytes.len() || !bytes[after].is_ascii_alphanumeric() && bytes[after] != b'_';
        if preceded_ok && followed_ok {
            return Some(abs);
        }
        start = abs + kw.len();
    }
    None
}

fn effective_indent(lines: &[&str], target: usize) -> i32 {
    for idx in (0..=target).rev() {
        let line = lines[idx];
        if !line.trim().is_empty() {
            return indent_of(line);
        }
    }
    0
}

fn indent_of(line: &str) -> i32 {
    let mut n = 0;
    for c in line.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 4,
            _ => break,
        }
    }
    n
}

fn extract_block(
    lines: &[&str],
    language: Language,
    start: usize,
    target: Option<usize>,
) -> String {
    match language {
        Language::Python => {
            let base = indent_of(lines[start]);
            let mut end = lines.len();
            for (idx, line) in lines.iter().enumerate().skip(start + 1) {
                if line.trim().is_empty() {
                    continue;
                }
                if indent_of(line) <= base {
                    end = idx;
                    break;
                }
            }
            render_range(lines, start, end.min(start + MAX_BLOCK_LINES))
        }
        _ => {
            let mut depth = 0i32;
            let mut seen_open = false;
            let mut end = start;
            'outer: for (idx, line) in lines.iter().enumerate().skip(start) {
                for ch in line.chars() {
                    if ch == '{' {
                        depth += 1;
                        seen_open = true;
                    } else if ch == '}' {
                        depth -= 1;
                        if seen_open && depth <= 0 {
                            end = idx;
                            break 'outer;
                        }
                    }
                }
                end = idx;
                if idx - start >= MAX_BLOCK_LINES {
                    break;
                }
            }
            let _ = target;
            render_range(lines, start, (end + 1).min(start + MAX_BLOCK_LINES))
        }
    }
}

fn render_snippet(lines: &[&str], target: usize) -> String {
    let start = target.saturating_sub(SNIPPET_RADIUS);
    let end = (target + SNIPPET_RADIUS + 1).min(lines.len());
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        let marker = if idx == target { ">" } else { " " };
        out.push_str(&format!("{marker} {:>5} | {line}\n", idx + 1));
    }
    out
}

fn render_range(lines: &[&str], start: usize, end: usize) -> String {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        out.push_str(&format!("{:>5} | {line}\n", idx + 1));
    }
    out
}

fn module_summary(lines: &[&str], language: Language) -> String {
    let mut out = String::new();
    let mut in_docstring: Option<&str> = None;
    for line in lines.iter().take(40) {
        let trimmed = line.trim();
        if let Some(delim) = in_docstring {
            out.push_str(trimmed);
            out.push(' ');
            if trimmed.contains(delim) && trimmed.len() > delim.len() {
                break;
            }
            continue;
        }
        if language == Language::Python
            && (trimmed.starts_with("\"\"\"") || trimmed.starts_with("'''"))
        {
            let delim = if trimmed.starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            let body = trimmed.trim_start_matches(delim);
            if !body.is_empty() {
                out.push_str(body);
                out.push(' ');
            }
            if body.contains(delim) {
                break;
            }
            in_docstring = Some(delim);
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            if let Some(rest) = trimmed
                .strip_prefix("//!")
                .or_else(|| trimmed.strip_prefix("///"))
                .or_else(|| trimmed.strip_prefix("//"))
                .or_else(|| trimmed.strip_prefix('#'))
            {
                let rest = rest.trim();
                if !rest.is_empty() && !rest.starts_with("include") {
                    out.push_str(rest);
                    out.push(' ');
                }
            }
            continue;
        }
        if trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("use ")
            || trimmed.starts_with("#include")
        {
            continue;
        }
        out.push_str(trimmed);
        break;
    }
    let out = out.trim();
    truncate_chars(out, 280)
}

fn imports(lines: &[&str]) -> Vec<String> {
    let mut imports = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if (trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("use ")
            || trimmed.starts_with("#include"))
            && imports.len() < 40
        {
            imports.push(trimmed.to_string());
        }
    }
    imports
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("codesafe-context-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        dir
    }

    #[test]
    fn python_context_finds_function_and_snippet() {
        let dir = fixture_dir("py");
        let src = "import os\n\n\ndef slow_sum(n):\n    total = 0\n    for i in range(n):\n        total += i\n    return total\n";
        std::fs::write(dir.join("pkg/math.py"), src).unwrap();
        let ctx = analyze(&dir, "pkg/math.py", Some(6));
        assert_eq!(ctx.file, "pkg/math.py");
        assert_eq!(ctx.folder, "pkg");
        assert_eq!(ctx.symbol.as_deref(), Some("slow_sum"));
        assert_eq!(ctx.symbol_kind.as_deref(), Some("function"));
        assert!(ctx.snippet.contains("total += i"));
        assert!(ctx.snippet.contains('>'));
        assert!(ctx.imports.iter().any(|i| i.contains("import os")));
        assert!(ctx.affected_area.as_deref().unwrap().contains("slow_sum"));
    }

    #[test]
    fn rust_context_finds_fn() {
        let dir = fixture_dir("rs");
        let src = "fn slow_sum(n: u64) -> u64 {\n    let mut t = 0;\n    for i in 0..n {\n        t += i;\n    }\n    t\n}\n";
        std::fs::write(dir.join("pkg/lib.rs"), src).unwrap();
        let ctx = analyze(&dir, "pkg/lib.rs", Some(4));
        assert_eq!(ctx.symbol.as_deref(), Some("slow_sum"));
        assert!(ctx.enclosing.contains("fn slow_sum"));
        assert!(ctx.enclosing.contains('}'));
    }

    #[test]
    fn cpp_context_finds_function() {
        let dir = fixture_dir("cpp");
        let src = "int slow_sum(int n) {\n  int t = 0;\n  for (int i = 0; i < n; ++i) t += i;\n  return t;\n}\n";
        std::fs::write(dir.join("pkg/main.cpp"), src).unwrap();
        let ctx = analyze(&dir, "pkg/main.cpp", Some(3));
        assert_eq!(ctx.symbol.as_deref(), Some("slow_sum"));
        assert!(ctx.enclosing.contains("return t"));
    }

    #[test]
    fn missing_file_is_graceful() {
        let dir = fixture_dir("missing");
        let ctx = analyze(&dir, "nope.py", Some(3));
        assert_eq!(ctx.file, "nope.py");
        assert!(ctx.snippet.is_empty());
    }

    #[test]
    fn enrich_sets_impact_and_folder() {
        let dir = fixture_dir("enrich");
        std::fs::write(dir.join("pkg/a.py"), "def f():\n    return 1\n").unwrap();
        let mut finding = Finding::new(
            "x",
            crate::types::Severity::Warning,
            Language::Python,
            "lint",
            "bad",
        );
        finding.file = "pkg/a.py".into();
        finding.line = Some(2);
        enrich_finding(&dir, &mut finding);
        assert_eq!(finding.folder, "pkg");
        assert_eq!(finding.symbol.as_deref(), Some("f"));
        assert!(finding.impact.is_some());
        assert!(finding.fix_risk.is_some());
    }
}
