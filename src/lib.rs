//! codesafe - test, benchmark, profile and safely auto-fix Python, Rust and C++ repositories.
//!
//! Epistemic infrastructure for agentic coding: independently certifies
//! whether agent work is correct, making gaming of the feedback loop
//! structurally impossible.
//!
//! Pillars:
//!   1. ACI Foundation — structured output, schemas, skill docs, parsed diagnostics
//!   2. Evidence Pipeline — parsed diagnostics, recovery hints, fault localization
//!   3. Verifiable Verification — asymmetric test/code separation, RED-first gating
//!   4. Test Generation — coverage-guided, self-healing loop with retry budget
//!   5. Durable Orchestration — event-sourced state, hermetic execution, receipt-bound verdicts

pub mod aci;
pub mod adapters;
pub mod bench;
pub mod brief;
pub mod builtin_rules;
pub mod cache;
pub mod cli;
pub mod config;
pub mod context;
pub mod diagnostics;
pub mod git;
pub mod optimizer;
pub mod orchestration;
pub mod profile;
pub mod report;
pub mod rule_engine;
pub mod scan;
pub mod test_generation;
pub mod types;
pub mod verification;

use thiserror::Error;

/// Result type used throughout codesafe.
pub type Result<T> = std::result::Result<T, Error>;

/// Fatal/recoverable errors. None of these use `unwrap` internally.
#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error at {path}: {message}")]
    Toml { path: String, message: String },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("command not found: {0}")]
    CommandNotFound(String),
    #[error("command failed: {command} (exit {exit:?})\nstderr: {stderr}")]
    CommandFailed {
        command: String,
        exit: Option<i32>,
        stderr: String,
    },
    #[error("config error: {0}")]
    Config(String),
    #[error("rule error: {0}")]
    Rule(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("no language detected in {path}")]
    NoLanguageDetected { path: String },
    #[error("aborted: {0}")]
    Aborted(String),
}

impl From<toml::de::Error> for Error {
    fn from(err: toml::de::Error) -> Self {
        Error::Toml {
            path: "<toml>".into(),
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_is_clear() {
        let err = Error::CommandNotFound("cargo".into());
        assert_eq!(err.to_string(), "command not found: cargo");
    }

    #[test]
    fn command_failed_captures_stderr() {
        let err = Error::CommandFailed {
            command: "pytest".into(),
            exit: Some(1),
            stderr: "ModuleNotFoundError".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("pytest"));
        assert!(msg.contains("ModuleNotFoundError"));
    }
}
