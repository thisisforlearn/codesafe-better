//! Benchmark repetition, result caching and regression detection.
//!
//! Benchmarks are run several times and reduced to a median. Results are cached by
//! source-file hash so reruns are fast and before/after comparisons (with change_pct)
//! are possible. Changes beyond `regression_threshold_pct` are flagged as regressions.

use crate::Result;
use crate::cache::Cache;
use crate::types::Language;
use std::path::Path;

/// Median of `runs` measurements produced by `f`.
pub fn measure<F>(runs: u32, mut f: F) -> Result<Option<f64>>
where
    F: FnMut() -> Result<Option<f64>>,
{
    let mut values = Vec::new();
    for _ in 0..runs {
        if let Some(v) = f()? {
            values.push(v);
        }
    }
    Ok(median(&mut values[..]))
}

/// Median of a non-empty slice; `None` when empty.
pub fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(values[values.len() / 2])
}

/// Persisted benchmark cache: `<language>:<source hash>:<benchmark name>` -> median.
pub struct BenchmarkCache {
    cache: Cache,
}

impl BenchmarkCache {
    pub fn new(path: &Path) -> Result<Self> {
        let dir = path.join(".codesafe").join("cache");
        Ok(Self {
            cache: Cache::new(&dir)?,
        })
    }

    fn key(language: Language, name: &str) -> String {
        // Deliberately not keyed by source hash: the point of regression
        // detection is to compare the current code with the previous run of the
        // same benchmark, even (especially) after the source changed.
        format!("bench:{language}:{name}")
    }

    pub fn previous(&self, language: Language, _source_hash: &str, name: &str) -> Option<f64> {
        self.cache
            .get(&Self::key(language, name))
            .and_then(|v| v.parse::<f64>().ok())
    }

    pub fn store(
        &mut self,
        language: Language,
        _source_hash: &str,
        name: &str,
        value: f64,
    ) -> Result<()> {
        self.cache
            .put(&Self::key(language, name), &value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_returns_median() {
        let mut calls = 0;
        let result = measure(5, || {
            calls += 1;
            Ok(Some((calls as f64) * 10.0))
        })
        .unwrap();
        assert_eq!(result, Some(30.0));
    }

    #[test]
    fn measure_empty_is_none() {
        let result = measure(3, || Ok(None)).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn benchmark_cache_detects_change_across_source_versions() {
        let dir = std::env::temp_dir().join(format!("codesafe-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cache = BenchmarkCache::new(&dir).unwrap();
        assert_eq!(cache.previous(Language::Rust, "hash-a", "parse"), None);
        cache
            .store(Language::Rust, "hash-a", "parse", 100.0)
            .unwrap();
        // A different source hash must still find the previous run.
        assert_eq!(
            cache.previous(Language::Rust, "hash-b", "parse"),
            Some(100.0)
        );
    }

    #[test]
    fn median_sorts() {
        let mut v = vec![5.0, 1.0, 3.0];
        assert_eq!(median(&mut v), Some(3.0));
    }
}
