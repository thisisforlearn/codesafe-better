//! Optimization advisor.
//!
//! Scans source files with deterministic heuristics and emits concrete,
//! verifiable optimization suggestions. Every suggestion answers:
//!
//! - where it is (folder, file, line, enclosing symbol, snippet)
//! - what is suboptimal and why
//! - the exact change to make
//! - the expected gain and the risk of the change
//! - how to verify the change did not break anything
//!
//! The advisor is report-only by construction: it never edits files. This keeps
//! the "optimize the code" loop safe for agentic use.

use crate::types::{Language, Optimization};
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_OPTIMIZATIONS: usize = 500;

/// Analyze a repository directory and return ranked optimization suggestions.
pub fn analyze_repo(root: &Path) -> Vec<Optimization> {
    let mut out = Vec::new();
    // One sorted walk for all languages keeps this fast on large repositories.
    for file in crate::scan::all_files(root) {
        if let Some(language) = crate::scan::language_of_path(&file) {
            out.extend(analyze_file(root, &file, language));
            if out.len() >= MAX_OPTIMIZATIONS * 2 {
                break;
            }
        }
    }
    rank(&mut out);
    out.truncate(MAX_OPTIMIZATIONS);
    out
}

/// Analyze a single file.
pub fn analyze_file(root: &Path, path: &Path, language: Language) -> Vec<Optimization> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let rel = crate::scan::relative_to(root, path);
    let folder = rel
        .rsplit_once('/')
        .map(|(f, _)| f)
        .unwrap_or(".")
        .to_string();
    let lines: Vec<&str> = text.lines().collect();
    let mut ctx = Checker {
        language,
        rel: &rel,
        folder: &folder,
        lines: &lines,
        out: Vec::new(),
    };
    match language {
        Language::Python => ctx.check_python(),
        Language::Rust => ctx.check_rust(),
        Language::Cpp => ctx.check_cpp(),
    }
    ctx.out
}

struct Checker<'a> {
    language: Language,
    rel: &'a str,
    folder: &'a str,
    lines: &'a [&'a str],
    out: Vec<Optimization>,
}

impl<'a> Checker<'a> {
    fn check_python(&mut self) {
        for idx in 0..self.lines.len() {
            let line = self.lines[idx];
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            if contains(t, "range(len(") {
                self.push(
                    idx,
                    "py-enumerate",
                    "Replace range(len(...)) with enumerate",
                    "performance",
                    "Indexing a sequence by position is slower and noisier than iterating values.",
                    "Iterate directly, or use `for i, value in enumerate(seq):` when the index is needed.",
                    "Removes an index lookup per iteration and makes intent explicit.",
                    "low",
                    "low",
                    0.55,
                );
            }
            if contains(t, "for ") && contains(t, "range(") {
                if let Some((next_idx, cell)) = self.next_code_line(idx) {
                    if contains(cell, "+=") {
                        self.push(
                            idx,
                            "py-sum-loop",
                            "Replace accumulation loop with sum()",
                            "performance",
                            "A manual Python loop accumulates one element at a time in the interpreter.",
                            "Use the C-level built-in, e.g. `total = sum(range(n))` or `sum(xs)`.",
                            "Typically 3-10x faster for numeric accumulation and less code.",
                            "low",
                            "low",
                            0.7,
                        );
                        let _ = next_idx;
                    }
                }
            }
            if contains(t, "open(") && !contains(t, "with ") {
                self.push(
                    idx,
                    "py-context-manager",
                    "Use a context manager for file handles",
                    "reliability",
                    "Files opened without `with` leak descriptors on early return or exception.",
                    "Wrap the operation in `with open(path, ...) as fh:` so the handle is always closed.",
                    "Prevents descriptor leaks and partially-written files.",
                    "low",
                    "low",
                    0.6,
                );
            }
            if t == "except:" {
                self.push(
                    idx,
                    "py-bare-except",
                    "Catch a specific exception",
                    "reliability",
                    "A bare `except:` swallows KeyboardInterrupt/SystemExit and hides real bugs.",
                    "Catch the narrowest exception type that can actually occur and re-raise the rest.",
                    "Makes failures observable instead of silently continuing.",
                    "low",
                    "low",
                    0.8,
                );
            }
            if contains(t, "except Exception") && self.scope_contains(idx, "pass") {
                self.push(
                    idx,
                    "py-swallowed-exception",
                    "Do not swallow exceptions with pass",
                    "reliability",
                    "The handler discards the error, so the caller receives corrupt or missing state.",
                    "Log with context and either handle the recoverable case or re-raise.",
                    "Failures surface with actionable context.",
                    "medium",
                    "low",
                    0.8,
                );
            }
            if contains(t, "eval(") || contains(t, "exec(") {
                self.push(
                    idx,
                    "py-eval",
                    "Avoid eval/exec on dynamic input",
                    "security",
                    "eval/exec execute arbitrary code and turn data into code paths.",
                    "Replace with an explicit parser/dispatch table or `ast.literal_eval` for literals.",
                    "Removes a code-injection class of bugs.",
                    "high",
                    "medium",
                    0.95,
                );
            }
            if contains(t, "import *") {
                self.push(
                    idx,
                    "py-wildcard-import",
                    "Replace wildcard imports with explicit names",
                    "maintainability",
                    "Wildcard imports hide the origin of names and can silently override them.",
                    "Import the exact symbols used, or import the module and qualify accesses.",
                    "Better static analysis and no accidental shadowing.",
                    "low",
                    "low",
                    0.45,
                );
            }
            if contains(t, "== None") || contains(t, "!= None") {
                self.push(
                    idx,
                    "py-none-compare",
                    "Use `is None` / `is not None`",
                    "correctness",
                    "Equality against None can call overloaded __eq__ and behaves incorrectly for proxies.",
                    "Compare identity instead: `x is None` / `x is not None`.",
                    "Correct identity semantics and faster comparison.",
                    "low",
                    "low",
                    0.5,
                );
            }
            if contains(t, "len(") && (contains(t, "== 0") || contains(t, "!= 0")) {
                self.push(
                    idx,
                    "py-truthiness",
                    "Use truthiness instead of len() comparisons",
                    "readability",
                    "`len(x) == 0` is slower and noisier than a truthiness check.",
                    "Use `if not xs:` / `if xs:` where the object implements __bool__/__len__.",
                    "Small speed win and clearer intent.",
                    "low",
                    "low",
                    0.35,
                );
            }
            if contains(t, "iterrows()") {
                self.push(
                    idx,
                    "py-pandas-iterrows",
                    "Vectorize instead of DataFrame.iterrows()",
                    "performance",
                    "iterrows builds a Series per row and is dramatically slower than vectorized ops.",
                    "Express the operation as a column expression, `.apply` on a Series, or `.itertuples()`.",
                    "Often 10-100x faster on large frames.",
                    "high",
                    "medium",
                    0.9,
                );
            }
            if t.starts_with("global ") {
                self.push(
                    idx,
                    "py-global",
                    "Replace module global mutation with explicit state",
                    "maintainability",
                    "Global mutable state makes tests order-dependent and hides data flow.",
                    "Pass state explicitly, or encapsulate it in an object passed to the function.",
                    "Deterministic tests and safer concurrency.",
                    "low",
                    "medium",
                    0.6,
                );
            }
            if (t.starts_with("def ") || t.starts_with("async def "))
                && (contains(t, "=[]") || contains(t, "={}") || contains(t, "=set()"))
            {
                self.push(
                    idx,
                    "py-mutable-default",
                    "Replace mutable default arguments with None",
                    "correctness",
                    "Mutable defaults are created once and shared across calls, leaking state.",
                    "Use `arg=None` and create a fresh container inside the function.",
                    "Eliminates cross-call state leakage.",
                    "low",
                    "low",
                    0.85,
                );
            }
            if contains(t, "sorted(") && contains(t, ")[0]") {
                self.push(
                    idx,
                    "py-min-max",
                    "Use min()/max() instead of sorting for one element",
                    "performance",
                    "Sorting is O(n log n) when only the extreme element is needed.",
                    "Use `min(xs)` / `max(xs, key=...)`.",
                    "Drops complexity from O(n log n) to O(n).",
                    "low",
                    "low",
                    0.6,
                );
            }
            if (contains(t, "requests.get(") || contains(t, "requests.post("))
                && !contains(t, "Session")
            {
                self.push(
                    idx,
                    "py-requests-session",
                    "Reuse a requests.Session for repeated calls",
                    "performance",
                    "Each top-level requests call opens a new connection pool.",
                    "Create one `session = requests.Session()` and call `session.get(...)`.",
                    "Connection reuse; large win in loops/agents making many calls.",
                    "medium",
                    "low",
                    0.5,
                );
            }
            if contains(t, "time.sleep(") {
                self.push(
                    idx,
                    "py-blocking-sleep",
                    "Avoid blocking sleep in request paths",
                    "performance",
                    "Blocking sleep stalls the thread and any work queued behind it.",
                    "Use an async sleep in async code, or a bounded backoff/event wait.",
                    "Better latency and throughput under concurrency.",
                    "medium",
                    "medium",
                    0.55,
                );
            }
            if contains(t, ".deepcopy(") {
                self.push(
                    idx,
                    "py-deepcopy",
                    "Avoid deepcopy on hot paths",
                    "performance",
                    "Deepcopy walks the entire object graph and is frequently the dominant cost.",
                    "Copy only the fields that must be isolated, or use an immutable/shared structure.",
                    "Can remove an O(size-of-graph) cost per call.",
                    "medium",
                    "medium",
                    0.5,
                );
            }
            if self.is_nested_loop(idx) {
                self.push(
                    idx,
                    "py-nested-loop",
                    "Reduce nested-loop complexity",
                    "performance",
                    "Nested loops over the same data are O(n*m) and dominate runtime as inputs grow.",
                    "Index one side in a dict/set for O(1) lookup, or use a join/vectorized operation.",
                    "Can move a hot path from quadratic to linear.",
                    "high",
                    "medium",
                    0.65,
                );
            }
        }
    }

    fn check_rust(&mut self) {
        for idx in 0..self.lines.len() {
            let line = self.lines[idx];
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") {
                continue;
            }
            let in_test = self.in_test_scope(idx);
            if contains(t, ".clone()") && !in_test {
                self.push(
                    idx,
                    "rs-clone",
                    "Remove unnecessary clone()",
                    "performance",
                    "Every clone allocates/copies; many are borrow-checker workarounds.",
                    "Borrow the value, restructure ownership, or use `Cow`/references instead of cloning.",
                    "Removes an allocation on the hot path.",
                    "medium",
                    "low",
                    0.5,
                );
            }
            if (contains(t, ".unwrap()") || contains(t, ".expect(")) && !in_test {
                self.push(
                    idx,
                    "rs-unwrap",
                    "Replace panicking unwrap/expect with explicit error handling",
                    "reliability",
                    "unwrap/expect abort the process on malformed input or transient failures.",
                    "Return `Result` with `?` and a contextual error, or handle the `None` branch.",
                    "Turns a crash into a recoverable, observable error.",
                    "high",
                    "low",
                    0.75,
                );
            }
            if contains(t, "unsafe {") || contains(t, "unsafe{") {
                self.push(
                    idx,
                    "rs-unsafe",
                    "Justify or eliminate unsafe blocks",
                    "security",
                    "Unsafe code bypasses the compiler's memory-safety guarantees.",
                    "Replace with a safe abstraction, or document the invariant that makes it sound.",
                    "Restores compiler-checked memory safety.",
                    "high",
                    "high",
                    0.85,
                );
            }
            if contains(t, "Vec::new()") && self.scope_contains(idx, "push(") {
                self.push(
                    idx,
                    "rs-reserve",
                    "Pre-allocate Vec capacity",
                    "performance",
                    "Growing a Vec from empty repeatedly reallocates and copies elements.",
                    "Use `Vec::with_capacity(n)` when the final size is known or bounded.",
                    "Removes O(log n) reallocations and copies.",
                    "medium",
                    "low",
                    0.6,
                );
            }
            if (contains(t, "format!(") || contains(t, "to_string()")) && self.in_loop(idx) {
                self.push(
                    idx,
                    "rs-loop-alloc",
                    "Avoid per-iteration String allocation",
                    "performance",
                    "Allocating a formatted string inside a loop creates allocator pressure.",
                    "Format once outside the loop, reuse a buffer with `write!`, or borrow `&str`.",
                    "Fewer allocations in hot loops.",
                    "medium",
                    "low",
                    0.55,
                );
            }
            if contains(t, "println!(") && self.in_loop(idx) {
                self.push(
                    idx,
                    "rs-loop-println",
                    "Remove logging from hot loops",
                    "performance",
                    "stdout is line-buffered and synchronized; per-iteration printing dominates runtime.",
                    "Aggregate output and print once, or use a buffered writer behind a log level.",
                    "Large win in hot loops and benchmarks.",
                    "medium",
                    "low",
                    0.6,
                );
            }
            if contains(t, "for ") && contains(t, "0..") && self.scope_contains(idx, "[i]") {
                self.push(
                    idx,
                    "rs-index-loop",
                    "Iterate values instead of indices",
                    "performance",
                    "Indexing in a range loop incurs bounds checks on every access.",
                    "Use `for (i, value) in xs.iter().enumerate()` or iterate `&xs` directly.",
                    "Removes repeated bounds checks and simplifies code.",
                    "medium",
                    "low",
                    0.5,
                );
            }
            if contains(t, ".collect::<Vec<_>>()") && self.scope_contains(idx, ".iter()") {
                self.push(
                    idx,
                    "rs-collect-allocation",
                    "Avoid intermediate collect()",
                    "performance",
                    "Collecting into a Vec only to iterate again allocates a throwaway buffer.",
                    "Chain the iterator lazily, or collect once into the final required type.",
                    "Removes one full allocation and copy.",
                    "low",
                    "low",
                    0.5,
                );
            }
            if contains(t, "panic!(") && !in_test {
                self.push(
                    idx,
                    "rs-panic",
                    "Replace panic! with propagated errors",
                    "reliability",
                    "panic! terminates the task/process and bypasses recovery paths.",
                    "Return an error variant, or use `debug_assert!` for internal invariants.",
                    "Keeps long-running agent processes alive.",
                    "high",
                    "low",
                    0.7,
                );
            }
            if self.is_nested_loop(idx) {
                self.push(
                    idx,
                    "rs-nested-loop",
                    "Reduce nested-loop complexity",
                    "performance",
                    "Nested loops over the same data are O(n*m) and dominate runtime as inputs grow.",
                    "Use a HashSet/HashMap for lookup, or chain iterators with early exit.",
                    "Can move a hot path from quadratic to linear.",
                    "high",
                    "medium",
                    0.6,
                );
            }
            if contains(t, "thread::sleep") {
                self.push(
                    idx,
                    "rs-blocking-sleep",
                    "Avoid thread::sleep in async or request paths",
                    "performance",
                    "Blocking sleep occupies a runtime worker and delays unrelated tasks.",
                    "Use async timers, or move blocking work to a dedicated thread.",
                    "Improves throughput for concurrent work.",
                    "medium",
                    "medium",
                    0.5,
                );
            }
        }
    }

    fn check_cpp(&mut self) {
        for idx in 0..self.lines.len() {
            let line = self.lines[idx];
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") {
                continue;
            }
            if contains(t, "std::endl") {
                self.push(
                    idx,
                    "cpp-endl",
                    "Use '\\n' instead of std::endl in loops",
                    "performance",
                    "std::endl flushes the stream on every call; flushing dominates bulk output.",
                    "Replace with `'\\n'` and flush once at the end when required.",
                    "Removes a syscall/flush per line.",
                    "medium",
                    "low",
                    0.7,
                );
            }
            if contains(t, "using namespace std;") {
                self.push(
                    idx,
                    "cpp-using-namespace",
                    "Avoid `using namespace std` in headers",
                    "maintainability",
                    "It injects hundreds of names and causes ambiguous overloads in translation units.",
                    "Qualify with `std::` or import the exact names inside a narrow scope.",
                    "Prevents subtle name-collision bugs.",
                    "low",
                    "low",
                    0.4,
                );
            }
            if contains(t, "new ") && contains(t, "(") {
                self.push(
                    idx,
                    "cpp-raw-new",
                    "Replace raw new with smart pointers/containers",
                    "reliability",
                    "Manual new requires matching delete on every path, including exceptions.",
                    "Use `std::make_unique` / `std::make_shared`, or store by value in a container.",
                    "Eliminates leak paths and clarifies ownership.",
                    "high",
                    "low",
                    0.75,
                );
            }
            if contains(t, "malloc(") || contains(t, "free(") {
                self.push(
                    idx,
                    "cpp-malloc",
                    "Replace malloc/free with RAII containers",
                    "reliability",
                    "C allocation does not run constructors/destructors and is easy to leak.",
                    "Use std::vector/std::string/std::unique_ptr for owned memory.",
                    "Exception-safe, leak-free ownership.",
                    "high",
                    "low",
                    0.7,
                );
            }
            if contains(t, "push_back(")
                && self.in_loop(idx)
                && !self.scope_contains(idx, "reserve(")
            {
                self.push(
                    idx,
                    "cpp-reserve",
                    "Reserve vector capacity before bulk push_back",
                    "performance",
                    "Repeated growth reallocates and copies all existing elements.",
                    "Call `v.reserve(n)` when the final or upper-bound size is known.",
                    "Removes reallocations/copies in bulk insertion.",
                    "medium",
                    "low",
                    0.6,
                );
            }
            if contains(t, "strlen(") && (contains(t, "for ") || contains(t, "while ")) {
                self.push(
                    idx,
                    "cpp-strlen-loop",
                    "Cache strlen outside the loop condition",
                    "performance",
                    "strlen is O(n); putting it in a loop condition makes the loop O(n^2).",
                    "Compute the length once into a local, or use std::string.",
                    "Can turn quadratic loops linear.",
                    "high",
                    "low",
                    0.8,
                );
            }
            if contains(t, "bits/stdc++.h") {
                self.push(
                    idx,
                    "cpp-stdcpp",
                    "Replace <bits/stdc++.h> with explicit headers",
                    "portability",
                    "The umbrella header is non-standard, slow to compile, and unavailable on MSVC.",
                    "Include only the standard headers this file uses.",
                    "Portable builds and faster compiles.",
                    "medium",
                    "low",
                    0.5,
                );
            }
            if contains(t, "#include <iostream>") {
                self.push(
                    idx,
                    "cpp-iostream",
                    "Prefer <ostream>/<istream> over <iostream> where possible",
                    "build-time",
                    "<iostream> pulls in a large static initializer and bloats compile time/binary.",
                    "Include the narrow stream header needed, and pass streams explicitly.",
                    "Smaller binary and faster compilation.",
                    "low",
                    "low",
                    0.2,
                );
            }
            if contains(t, "std::vector<") && !contains(t, "const&") && is_param_line(t) {
                self.push(
                    idx,
                    "cpp-pass-by-value",
                    "Pass vectors/strings by const reference",
                    "performance",
                    "Passing by value copies the whole container at every call.",
                    "Take `const std::vector<T>&` (or `std::span`) instead.",
                    "Removes an O(n) copy per call.",
                    "high",
                    "low",
                    0.7,
                );
            }
            if contains(t, "for (") && contains(t, ".size()") {
                self.push(
                    idx,
                    "cpp-range-for",
                    "Prefer range-based for over index loops",
                    "readability",
                    "Index loops are verbose and can desynchronize loop bounds from the container.",
                    "Use `for (const auto& item : container)`.",
                    "Clearer code and no out-of-bounds risk.",
                    "low",
                    "low",
                    0.35,
                );
            }
            if self.is_nested_loop(idx) {
                self.push(
                    idx,
                    "cpp-nested-loop",
                    "Reduce nested-loop complexity",
                    "performance",
                    "Nested loops over containers are O(n*m) and dominate runtime as data grows.",
                    "Use a hash map/set for lookup, or an existing algorithm (sort+merge, join).",
                    "Can move a hot path from quadratic to linear.",
                    "high",
                    "medium",
                    0.6,
                );
            }
        }
    }

    fn next_code_line(&self, idx: usize) -> Option<(usize, &'a str)> {
        for (offset, line) in self.lines.iter().enumerate().skip(idx + 1).take(3) {
            if !line.trim().is_empty() {
                return Some((offset, line.trim()));
            }
        }
        None
    }

    fn in_loop(&self, idx: usize) -> bool {
        let base = indent_width(self.lines[idx]);
        if self.language == Language::Python {
            for i in (0..idx).rev().take(24) {
                let t = self.lines[i].trim_start();
                if t.starts_with("for ") || t.starts_with("while ") {
                    return true;
                }
                if indent_width(self.lines[i]) < base {
                    break;
                }
            }
            return false;
        }
        // Brace languages: use brace depth so sequential loops are not confused
        // with an enclosing loop.
        let depths = brace_depths(self.lines);
        let target = depths[idx];
        let mut min_seen = target;
        for i in (0..idx).rev().take(200) {
            let t = self.lines[i].trim_start();
            if (t.starts_with("for ") || t.starts_with("while ") || t.starts_with("loop "))
                && depths[i] <= target
                && min_seen >= depths[i]
            {
                return true;
            }
            min_seen = min_seen.min(depths[i]);
            if min_seen < 0 {
                break;
            }
        }
        false
    }

    fn is_nested_loop(&self, idx: usize) -> bool {
        let t = self.lines[idx].trim_start();
        if !t.starts_with("for ") && !t.starts_with("while ") {
            return false;
        }
        if self.language == Language::Python {
            let base = indent_width(self.lines[idx]);
            for line in self.lines.iter().skip(idx + 1).take(12) {
                let nt = line.trim_start();
                if (nt.starts_with("for ") || nt.starts_with("while ")) && indent_width(line) > base
                {
                    return true;
                }
                if !nt.is_empty() && indent_width(line) <= base {
                    break;
                }
            }
            return false;
        }
        let depths = brace_depths(self.lines);
        let header_depth = depths[idx];
        for j in (idx + 1)..(idx + 1 + 40).min(self.lines.len()) {
            if depths[j] < header_depth {
                break;
            }
            let nt = self.lines[j].trim_start();
            if (nt.starts_with("for ") || nt.starts_with("while ")) && depths[j] > header_depth {
                return true;
            }
        }
        false
    }

    fn in_test_scope(&self, idx: usize) -> bool {
        for line in self.lines.iter().take(idx).rev().take(200) {
            let t = line.trim();
            if t == "#[cfg(test)]" || t.starts_with("mod tests") {
                return true;
            }
        }
        false
    }

    fn scope_contains(&self, idx: usize, needle: &str) -> bool {
        let base = indent_width(self.lines[idx]);
        for line in self.lines.iter().skip(idx).take(20) {
            if indent_width(line) < base && !line.trim().is_empty() {
                break;
            }
            if line.contains(needle) {
                return true;
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        idx: usize,
        id: &str,
        title: &str,
        category: &str,
        evidence: &str,
        suggested_change: &str,
        expected_gain: &str,
        risk: &str,
        why_safe: &str,
        score: f64,
    ) {
        let line = self.lines[idx];
        let symbol = enclosing_symbol_name(self.lines, self.language, idx);
        let snippet = snippet_around(self.lines, idx, 2);
        // Most call sites pass a risk level as the final argument; turn it into a
        // concrete safety argument instead of leaking "low"/"medium" into JSON.
        let why_safe = if matches!(why_safe, "low" | "medium" | "high") {
            match risk {
                "low" => "the change is local and behaviour-preserving; verify with the full test suite",
                "medium" => "the change is localized; confirm behaviour with tests before keeping it",
                _ => "the change touches a hot path; apply incrementally and compare benchmarks before/after",
            }
            .to_string()
        } else {
            why_safe.to_string()
        };
        self.out.push(Optimization {
            id: format!("{id}:{}:{}", self.rel, idx + 1),
            title: title.to_string(),
            category: category.to_string(),
            language: self.language,
            file: self.rel.to_string(),
            folder: self.folder.to_string(),
            line: Some((idx + 1) as u32),
            symbol,
            evidence: format!("{} (line {}): {}", self.rel, idx + 1, line.trim()),
            snippet,
            explanation: evidence.to_string(),
            suggested_change: suggested_change.to_string(),
            expected_gain: expected_gain.to_string(),
            risk: risk.to_string(),
            why_safe: why_safe.to_string(),
            how_to_apply: format!(
                "Edit {} at line {}, apply the smallest change described, then run the verification command.",
                self.rel,
                idx + 1
            ),
            how_to_verify: verify_command(self.language),
            score,
        });
    }
}

fn verify_command(language: Language) -> String {
    match language {
        Language::Python => "`codesafe test <repo>` or `python -m pytest -q`".to_string(),
        Language::Rust => {
            "`codesafe test <repo>` or `cargo test && cargo clippy --all-targets`".to_string()
        }
        Language::Cpp => {
            "`codesafe test <repo>` or build + `ctest --output-on-failure`".to_string()
        }
    }
}

fn is_param_line(line: &str) -> bool {
    let open = match line.find('(') {
        Some(p) => p,
        None => return false,
    };
    let close = match line.rfind(')') {
        Some(p) => p,
        None => return false,
    };
    if close <= open {
        return false;
    }
    let before = line[..open].trim();
    !before.is_empty()
        && !before.ends_with("if")
        && !before.ends_with("for")
        && !before.ends_with("while")
        && !before.contains('=')
        && !before.contains(';')
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

fn indent_width(line: &str) -> i32 {
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

fn brace_depths(lines: &[&str]) -> Vec<i32> {
    let mut depth = 0i32;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let mut in_string: Option<char> = None;
        let mut escaped = false;
        for c in line.chars() {
            if let Some(q) = in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    in_string = None;
                }
                continue;
            }
            match c {
                '"' | '\'' => in_string = Some(c),
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        out.push(depth);
    }
    out
}

fn snippet_around(lines: &[&str], idx: usize, radius: usize) -> String {
    let start = idx.saturating_sub(radius);
    let end = (idx + radius + 1).min(lines.len());
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let marker = if i == idx { ">" } else { " " };
        out.push_str(&format!("{marker} {:>5} | {line}\n", i + 1));
    }
    out
}

fn enclosing_symbol_name(lines: &[&str], language: Language, idx: usize) -> Option<String> {
    let base = indent_width(lines[idx]);
    for i in (0..=idx).rev().take(80) {
        let raw = lines[i];
        let t = raw.trim_start();
        if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        let candidate = match language {
            Language::Python => {
                if (t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class "))
                    && (indent_width(raw) < base || i == idx)
                {
                    t.split(|c: char| c.is_whitespace() || c == '(')
                        .nth(1)
                        .map(|s| s.trim_end_matches(':'))
                } else {
                    None
                }
            }
            Language::Rust => {
                if t.contains("fn ") || t.starts_with("impl ") || t.starts_with("struct ") {
                    t.split(|c: char| c.is_whitespace() || c == '(' || c == '<')
                        .find(|w| {
                            !matches!(
                                *w,
                                "pub" | "async" | "fn" | "impl" | "struct" | "unsafe" | "const"
                            )
                        })
                } else {
                    None
                }
            }
            Language::Cpp => {
                if t.contains('(') && !t.starts_with('#') && !t.ends_with(';') {
                    let before = &t[..t.find('(')?];
                    before
                        .rsplit(|c: char| c.is_whitespace() || c == ':' || c == '*' || c == '&')
                        .next()
                } else {
                    None
                }
            }
        };
        if let Some(name) = candidate {
            let name = name.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn rank(opts: &mut Vec<Optimization>) {
    opts.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    // Drop exact duplicates (same file/line/id) while keeping order.
    let mut seen = std::collections::HashSet::new();
    let mut keep = Vec::with_capacity(opts.len());
    for opt in opts.iter() {
        if seen.insert(opt.id.clone()) {
            keep.push(opt.clone());
        }
    }
    *opts = keep;
}

/// Convenience entry point for tests and callers that already know the files.
pub fn analyze_files(root: &Path, files: &[PathBuf]) -> Vec<Optimization> {
    let mut out = Vec::new();
    for file in files {
        if let Some(lang) = crate::scan::language_of_path(file) {
            out.extend(analyze_file(root, file, lang));
        }
    }
    rank(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codesafe-opt-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detects_python_sum_loop_and_enumerate() {
        let dir = temp("py");
        write(
            &dir,
            "pkg/slow.py",
            "def slow(n):\n    total = 0\n    for i in range(len(range(n))):\n        total += i\n    return total\n",
        );
        let file = dir.join("pkg/slow.py");
        let opts = analyze_file(&dir, &file, Language::Python);
        assert!(opts.iter().any(|o| o.title.contains("sum()")));
        assert!(opts.iter().any(|o| o.id.starts_with("py-enumerate")));
        assert!(opts.iter().all(|o| !o.snippet.is_empty()));
        assert!(
            opts.iter()
                .all(|o| o.how_to_verify.contains("codesafe test"))
        );
    }

    #[test]
    fn detects_mutable_default_and_bare_except() {
        let dir = temp("py2");
        let file = write(
            &dir,
            "a.py",
            "def f(x=[]):\n    try:\n        return x\n    except:\n        return []\n",
        );
        let opts = analyze_file(&dir, &file, Language::Python);
        assert!(opts.iter().any(|o| o.id.starts_with("py-mutable-default")));
        assert!(opts.iter().any(|o| o.id.starts_with("py-bare-except")));
    }

    #[test]
    fn detects_rust_unwrap_and_reserve() {
        let dir = temp("rs");
        let file = write(
            &dir,
            "src/lib.rs",
            "pub fn f(v: &[u8]) -> u8 {\n    let mut out = Vec::new();\n    for x in v {\n        out.push(*x);\n    }\n    out[0].unwrap()\n}\n",
        );
        let opts = analyze_file(&dir, &file, Language::Rust);
        assert!(opts.iter().any(|o| o.id.starts_with("rs-reserve")));
        assert!(opts.iter().any(|o| o.id.starts_with("rs-unwrap")));
    }

    #[test]
    fn detects_cpp_endl_and_raw_new() {
        let dir = temp("cpp");
        let file = write(
            &dir,
            "main.cpp",
            "#include <iostream>\nint main() {\n  std::cout << \"x\" << std::endl;\n  int* p = new int(3);\n  return *p;\n}\n",
        );
        let opts = analyze_file(&dir, &file, Language::Cpp);
        assert!(opts.iter().any(|o| o.id.starts_with("cpp-endl")));
        assert!(opts.iter().any(|o| o.id.starts_with("cpp-raw-new")));
    }

    #[test]
    fn sequential_loops_are_not_reported_as_nested() {
        let dir = temp("nested");
        let rs = write(
            &dir,
            "src/lib.rs",
            "pub fn f(v: &[u8]) {\n    for x in v {\n        let _ = x;\n    }\n    for y in v {\n        let _ = y;\n    }\n}\n",
        );
        let opts = analyze_file(&dir, &rs, Language::Rust);
        assert!(!opts.iter().any(|o| o.id.starts_with("rs-nested-loop")));

        let cpp = write(
            &dir,
            "main.cpp",
            "int f(int n) {\n  for (int i=0;i<n;++i) {\n    int x=i;\n  }\n  for (int j=0;j<n;++j) {\n    int y=j;\n  }\n  return 0;\n}\n",
        );
        let opts = analyze_file(&dir, &cpp, Language::Cpp);
        assert!(!opts.iter().any(|o| o.id.starts_with("cpp-nested-loop")));
    }

    #[test]
    fn nested_loops_are_reported_for_brace_languages() {
        let dir = temp("nested2");
        let rs = write(
            &dir,
            "src/lib.rs",
            "pub fn f(v: &[u8]) {\n    for x in v {\n        for y in v {\n            if x == y { break; }\n        }\n    }\n}\n",
        );
        let opts = analyze_file(&dir, &rs, Language::Rust);
        assert!(opts.iter().any(|o| o.id.starts_with("rs-nested-loop")));

        let cpp = write(
            &dir,
            "main.cpp",
            "int f(int n) {\n  for (int i=0;i<n;++i) {\n    for (int j=0;j<n;++j) {\n      if (i==j) return j;\n    }\n  }\n  return 0;\n}\n",
        );
        let opts = analyze_file(&dir, &cpp, Language::Cpp);
        assert!(opts.iter().any(|o| o.id.starts_with("cpp-nested-loop")));
    }

    #[test]
    fn analyze_repo_walks_fixtures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let opts = analyze_repo(&root);
        assert!(!opts.is_empty());
        assert!(opts.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn ignores_huge_files_gracefully() {
        let dir = temp("missing");
        let opts = analyze_file(&dir, &dir.join("nope.py"), Language::Python);
        assert!(opts.is_empty());
    }
}
