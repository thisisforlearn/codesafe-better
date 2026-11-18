//! Loading and discovery of `codesafe.toml` / `codeguard.toml`.

use crate::Result;
use crate::types::Config;
use std::path::{Path, PathBuf};

/// Config filenames we recognize. `codesafe.toml` is preferred; the legacy
/// `codeguard.toml` name keeps existing projects working.
pub const CONFIG_NAMES: &[&str] = &["codesafe.toml", "codeguard.toml"];

/// Load a config file if it exists; missing files yield an empty config (never an error).
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| crate::Error::Config(format!("reading {path:?}: {e}")))?;
    let parsed: Config = toml::from_str(&text)
        .map_err(|e| crate::Error::Config(format!("parsing {path:?}: {e}")))?;
    Ok(parsed)
}

/// Discover the config for a command.
///
/// Order:
/// 1. an explicit `--config` path (missing file is an error, not a silent default)
/// 2. the target repository's `codesafe.toml` / `codeguard.toml`
/// 3. the current directory's `codesafe.toml` / `codeguard.toml`
/// 4. an empty default config
pub fn resolve(explicit: Option<&Path>, root: Option<&Path>) -> Result<(Config, PathBuf)> {
    if let Some(path) = explicit {
        if !path.is_file() {
            return Err(crate::Error::Config(format!(
                "config file not found: {}",
                path.display()
            )));
        }
        return Ok((load(path)?, path.to_path_buf()));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(root) = root {
        for name in CONFIG_NAMES {
            candidates.push(root.join(name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for name in CONFIG_NAMES {
            let p = cwd.join(name);
            if !candidates.contains(&p) {
                candidates.push(p);
            }
        }
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok((load(candidate)?, candidate.clone()));
        }
    }
    let fallback = root
        .map(|r| r.join(CONFIG_NAMES[0]))
        .unwrap_or_else(|| PathBuf::from(CONFIG_NAMES[0]));
    Ok((Config::default(), fallback))
}

/// Example config documentation.
pub fn example() -> &'static str {
    r#"# codesafe.toml - edit to tune behavior

# Parallel jobs for language adapters (1-64).
jobs = 4

# Timeout in seconds for each external tool invocation.
timeout_s = 300

# Benchmark repetitions; results are reduced to a median.
bench_runs = 5

# Flag a benchmark as regressed when it is slower by more than this percentage.
regression_threshold_pct = 5.0

# CI gate: fail when any finding is at least this severe.
fail_on = "error"

# Report what would change without modifying files.
dry_run = false

# Allow `fix` to run on a dirty git working tree.
force = false

# Rules directory, relative to the repository root.
rules_dir = "rules"
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_default() {
        let cfg = load(Path::new("/definitely/not/here.toml")).unwrap();
        assert!(cfg.jobs.is_none());
    }

    #[test]
    fn explicit_missing_config_is_an_error() {
        let err = resolve(Some(Path::new("/definitely/not/here.toml")), None).unwrap_err();
        assert!(matches!(err, crate::Error::Config(_)));
    }

    #[test]
    fn discovers_repo_config() {
        let dir = std::env::temp_dir().join(format!("codesafe-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("codesafe.toml"), "jobs = 2\n").unwrap();
        let (cfg, path) = resolve(None, Some(&dir)).unwrap();
        assert_eq!(cfg.jobs(), 2);
        assert_eq!(path, dir.join("codesafe.toml"));
    }

    #[test]
    fn legacy_config_name_works() {
        let dir = std::env::temp_dir().join(format!("codesafe-config2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("codeguard.toml"), "jobs = 3\n").unwrap();
        let (cfg, path) = resolve(None, Some(&dir)).unwrap();
        assert_eq!(cfg.jobs(), 3);
        assert_eq!(path, dir.join("codeguard.toml"));
    }
}
