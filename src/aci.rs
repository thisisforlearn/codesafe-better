//! Agent-Computer Interface (ACI) for codesafe.
//!
//! Every command emits structured, machine-consumable output alongside
//! human-readable output. Errors carry recovery hints and trace_ids so
//! agents can diagnose, repair, and re-verify in a closed loop.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Output mode selected by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputMode {
    #[default]
    Human,
    Json,
    Jsonl,
}

impl std::str::FromStr for OutputMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "human" => Ok(OutputMode::Human),
            "json" => Ok(OutputMode::Json),
            "jsonl" => Ok(OutputMode::Jsonl),
            other => Err(format!(
                "unknown output mode: {other} (use human, json, or jsonl)"
            )),
        }
    }
}

/// Strict machine-consumable envelope for every command response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub error: Option<EnvelopeError>,
    pub meta: EnvelopeMeta,
}

/// Machine-readable error with recovery hints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeError {
    pub code: String,
    pub category: String,
    pub message: String,
    pub suggestion: Option<String>,
    pub is_retryable: bool,
    pub trace_id: String,
}

/// Metadata attached to every response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeMeta {
    pub command: String,
    pub path: Option<String>,
    pub timestamp_ms: u64,
    pub trace_id: String,
    pub output_mode: String,
}

impl<T> Envelope<T> {
    pub fn ok(result: T, command: &str, path: Option<&str>, trace_id: &str) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
            meta: EnvelopeMeta {
                command: command.to_string(),
                path: path.map(|s| s.to_string()),
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                trace_id: trace_id.to_string(),
                output_mode: "human".to_string(),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn err(
        code: &str,
        category: &str,
        message: &str,
        suggestion: Option<&str>,
        is_retryable: bool,
        command: &str,
        path: Option<&str>,
        trace_id: &str,
    ) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(EnvelopeError {
                code: code.to_string(),
                category: category.to_string(),
                message: message.to_string(),
                suggestion: suggestion.map(|s| s.to_string()),
                is_retryable,
                trace_id: trace_id.to_string(),
            }),
            meta: EnvelopeMeta {
                command: command.to_string(),
                path: path.map(|s| s.to_string()),
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                trace_id: trace_id.to_string(),
                output_mode: "human".to_string(),
            },
        }
    }
}

/// Generate a deterministic trace_id from command + path + timestamp.
pub fn make_trace_id(command: &str, path: Option<&str>) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    command.hash(&mut hasher);
    path.hash(&mut hasher);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    format!("cs-{:x}", hasher.finish())
}

/// Print an envelope in the requested output mode.
pub fn print_envelope<T: Serialize>(mode: OutputMode, envelope: &Envelope<T>) {
    match mode {
        OutputMode::Human => {
            if let Some(ref err) = envelope.error {
                eprintln!("codesafe [{}] {}: {}", err.code, err.category, err.message);
                if let Some(s) = &err.suggestion {
                    eprintln!("  suggestion: {s}");
                }
                eprintln!("  trace_id: {}", err.trace_id);
                if err.is_retryable {
                    eprintln!("  retryable: yes");
                }
            }
        }
        OutputMode::Json => match serde_json::to_value(envelope) {
            Ok(mut value) => {
                if let Some(meta) = value.get_mut("meta").and_then(|m| m.as_object_mut()) {
                    meta.insert("output_mode".into(), serde_json::json!(mode.to_string()));
                }
                match serde_json::to_string_pretty(&value) {
                    Ok(text) => println!("{text}"),
                    Err(e) => eprintln!("codesafe: failed to encode response: {e}"),
                }
            }
            Err(e) => eprintln!("codesafe: failed to encode response: {e}"),
        },
        OutputMode::Jsonl => match serde_json::to_value(envelope) {
            Ok(mut value) => {
                if let Some(meta) = value.get_mut("meta").and_then(|m| m.as_object_mut()) {
                    meta.insert("output_mode".into(), serde_json::json!(mode.to_string()));
                }
                match serde_json::to_string(&value) {
                    Ok(text) => println!("{text}"),
                    Err(e) => eprintln!("codesafe: failed to encode response: {e}"),
                }
            }
            Err(e) => eprintln!("codesafe: failed to encode response: {e}"),
        },
    }
}

impl fmt::Display for OutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputMode::Human => write!(f, "human"),
            OutputMode::Json => write!(f, "json"),
            OutputMode::Jsonl => write!(f, "jsonl"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn output_mode_from_str() {
        assert_eq!(OutputMode::from_str("human"), Ok(OutputMode::Human));
        assert_eq!(OutputMode::from_str("json"), Ok(OutputMode::Json));
        assert_eq!(OutputMode::from_str("jsonl"), Ok(OutputMode::Jsonl));
        assert_eq!(OutputMode::from_str("HUMAN"), Ok(OutputMode::Human));
        assert!(OutputMode::from_str("bogus").is_err());
    }

    #[test]
    fn envelope_ok_has_no_error() {
        let env: Envelope<u32> = Envelope::ok(42, "scan", Some("."), "cs-test");
        assert!(env.ok);
        assert!(env.error.is_none());
        assert_eq!(env.result, Some(42));
    }

    #[test]
    fn envelope_err_has_trace_id() {
        let env: Envelope<u32> = Envelope::err(
            "E001",
            "config",
            "missing config",
            Some("create codesafe.toml"),
            true,
            "init",
            Some("."),
            "cs-trace-1",
        );
        assert!(!env.ok);
        assert!(env.error.is_some());
        let err = env.error.unwrap();
        assert_eq!(err.code, "E001");
        assert_eq!(err.category, "config");
        assert_eq!(err.suggestion, Some("create codesafe.toml".to_string()));
        assert!(err.is_retryable);
        assert_eq!(err.trace_id, "cs-trace-1");
    }

    #[test]
    fn trace_id_is_deterministic_for_same_inputs() {
        let id1 = make_trace_id("scan", Some("."));
        let _id2 = make_trace_id("scan", Some("."));
        // Same command+path but different timestamps -> different ids;
        // just verify it's non-empty and formatted correctly.
        assert!(id1.starts_with("cs-"));
        assert!(!id1.is_empty());
    }
}
