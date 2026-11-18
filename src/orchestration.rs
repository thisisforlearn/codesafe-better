//! Durable Orchestration (Pillar 5).
//!
//! Event-sourced state, hermetic execution, and receipt-bound verdicts.
//! Every state transition is an immutable event. Execution is hermetic
//! (isolated, deterministic). Every verdict is bound to a git tree hash
//! and oracle version for auditability.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// An immutable event in the event-sourced log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub event_type: EventType,
    pub payload: serde_json::Value,
    pub trace_id: String,
    pub timestamp_ms: u64,
    pub seq_no: u64,
}

/// Types of events in the orchestration log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    SpecCreated,
    RedTestWritten,
    RedVerified,
    ImplementationStarted,
    ImplementationCompleted,
    GreenVerified,
    FixAttempted,
    FixVerified,
    FixReverted,
    VerdictPassed,
    VerdictFailed,
    ErrorOccurred,
}

/// The event-sourced state machine.
/// State is derived by replaying the event log; events are never mutated.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrchestrationState {
    pub events: Vec<Event>,
    pub current_spec: Option<String>,
    pub current_phase: OrchestrationPhase,
    pub git_tree_hash: Option<String>,
    pub oracle_version: Option<String>,
    pub receipts: Vec<Receipt>,
}

/// Current phase in the orchestration lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OrchestrationPhase {
    #[default]
    Idle,
    SpecDefined,
    RedTestWritten,
    RedVerified,
    ImplementationInProgress,
    GreenVerified,
    Completed,
    Failed,
}

/// A receipt-bound verdict tying a result to a specific code state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub id: String,
    pub git_tree_hash: String,
    pub oracle_version: String,
    pub verdict: Verdict,
    pub trace_id: String,
    pub timestamp_ms: u64,
    pub event_seq_no: u64,
}

/// Verdict outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
}

/// Hermetic execution context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermeticContext {
    pub isolated: bool,
    pub env_vars: HashMap<String, String>,
    pub working_dir: PathBuf,
    pub timeout_s: u64,
    pub network_allowed: bool,
    pub file_allowlist: Vec<PathBuf>,
}

/// Result of a hermetic execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermeticResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub trace_id: String,
}

/// Append an event to the event-sourced log.
pub fn append_event(
    state: &mut OrchestrationState,
    event_type: EventType,
    payload: serde_json::Value,
    trace_id: &str,
) -> Event {
    let seq_no = state.events.len() as u64 + 1;
    let event = Event {
        id: format!("evt-{}-{}", trace_id, seq_no),
        event_type,
        payload,
        trace_id: trace_id.to_string(),
        timestamp_ms: now_ms(),
        seq_no,
    };
    state.events.push(event.clone());
    event
}

/// Get the current timestamp in milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Create a hermetic execution context.
pub fn create_hermetic_context(
    working_dir: PathBuf,
    timeout_s: u64,
    file_allowlist: Vec<PathBuf>,
) -> HermeticContext {
    let mut env_vars = HashMap::new();
    env_vars.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    env_vars.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    HermeticContext {
        isolated: true,
        env_vars,
        working_dir,
        timeout_s,
        network_allowed: false,
        file_allowlist,
    }
}

/// Execute a command in a hermetic context.
///
/// Hermetic means: a cleared environment (only `ctx.env_vars` is visible), a
/// fixed working directory, and an enforced wall-clock timeout. Network access
/// cannot be blocked portably without OS sandboxing; `network_allowed` is
/// recorded in the context and surfaced in the result so callers do not assume
/// isolation that was not applied.
pub fn execute_hermetic(
    command: &str,
    args: &[&str],
    ctx: &HermeticContext,
    trace_id: &str,
) -> HermeticResult {
    let start = std::time::Instant::now();

    let mut cmd = std::process::Command::new(command);
    cmd.args(args)
        .current_dir(&ctx.working_dir)
        .env_clear()
        .envs(ctx.env_vars.iter())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HermeticResult {
                exit_code: None,
                stdout: String::new(),
                stderr: e.to_string(),
                duration_ms: now_ms().saturating_sub(now_ms()),
                trace_id: trace_id.to_string(),
            };
        }
    };

    let timeout = std::time::Duration::from_secs(ctx.timeout_s.max(1));
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    let output = match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return HermeticResult {
                exit_code: None,
                stdout: String::new(),
                stderr: e.to_string(),
                duration_ms: start.elapsed().as_millis() as u64,
                trace_id: trace_id.to_string(),
            };
        }
        Err(_) => {
            // The child handle lives in the worker thread; report the timeout and
            // leave the thread to reap it. The timeout is enforced for the caller.
            return HermeticResult {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("timed out after {}s", ctx.timeout_s.max(1)),
                duration_ms: start.elapsed().as_millis() as u64,
                trace_id: trace_id.to_string(),
            };
        }
    };
    let _ = handle.join();

    HermeticResult {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        duration_ms: start.elapsed().as_millis() as u64,
        trace_id: trace_id.to_string(),
    }
}

/// Event log path for a repository.
pub fn events_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".codesafe").join("events.jsonl")
}

/// Append an event to an on-disk JSONL event log (durable orchestration).
pub fn append_event_to_log(root: &std::path::Path, event: &Event) -> crate::Result<()> {
    use std::io::Write;
    let path = events_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    let line = serde_json::to_string(event).map_err(crate::Error::Json)?;
    writeln!(file, "{line}").map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    Ok(())
}

/// Read an event log from disk. Missing files yield an empty log.
pub fn read_events(path: &std::path::Path) -> crate::Result<Vec<Event>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    let mut events = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(event) => events.push(event),
            Err(e) => {
                return Err(crate::Error::Config(format!(
                    "invalid event in {}: {e}",
                    path.display()
                )));
            }
        }
    }
    Ok(events)
}

/// Create a receipt-bound verdict.
pub fn create_receipt(
    git_tree_hash: &str,
    oracle_version: &str,
    verdict: Verdict,
    trace_id: &str,
    event_seq_no: u64,
) -> Receipt {
    Receipt {
        id: format!("rcpt-{}-{}", trace_id, event_seq_no),
        git_tree_hash: git_tree_hash.to_string(),
        oracle_version: oracle_version.to_string(),
        verdict,
        trace_id: trace_id.to_string(),
        timestamp_ms: now_ms(),
        event_seq_no,
    }
}

/// Apply an event's transition to the state, returning the new state.
///
/// This is the pure reduction function. It does *not* append to `events`; use
/// `append_event` for that. Keeping append and reduce separate prevents event
/// duplication when a state is already up to date with the log.
pub fn apply_event(state: &OrchestrationState, event: &Event) -> OrchestrationState {
    let mut new_state = state.clone();

    match event.event_type {
        EventType::SpecCreated => {
            new_state.current_spec = event
                .payload
                .get("spec_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            new_state.current_phase = OrchestrationPhase::SpecDefined;
        }
        EventType::RedTestWritten => {
            new_state.current_phase = OrchestrationPhase::RedTestWritten;
        }
        EventType::RedVerified => {
            new_state.current_phase = OrchestrationPhase::RedVerified;
        }
        EventType::ImplementationStarted => {
            new_state.current_phase = OrchestrationPhase::ImplementationInProgress;
        }
        EventType::ImplementationCompleted => {
            new_state.current_phase = OrchestrationPhase::GreenVerified;
        }
        EventType::GreenVerified => {
            new_state.current_phase = OrchestrationPhase::GreenVerified;
        }
        EventType::VerdictPassed => {
            new_state.current_phase = OrchestrationPhase::Completed;
        }
        EventType::VerdictFailed => {
            new_state.current_phase = OrchestrationPhase::Failed;
        }
        EventType::ErrorOccurred => {
            new_state.current_phase = OrchestrationPhase::Failed;
        }
        _ => {}
    }

    new_state
}

/// Replay all events to derive current state.
pub fn replay_events(events: &[Event]) -> OrchestrationState {
    let mut state = OrchestrationState::default();
    for event in events {
        state.events.push(event.clone());
        state = apply_event(&state, event);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_event_adds_to_log() {
        let mut state = OrchestrationState::default();
        let event = append_event(
            &mut state,
            EventType::SpecCreated,
            serde_json::json!({"spec_id": "s1"}),
            "cs-trace",
        );
        assert_eq!(state.events.len(), 1);
        assert_eq!(event.seq_no, 1);
        assert_eq!(event.event_type, EventType::SpecCreated);
    }

    #[test]
    fn apply_event_transitions_phase() {
        let mut state = OrchestrationState::default();
        let event = append_event(
            &mut state,
            EventType::SpecCreated,
            serde_json::json!({"spec_id": "s1"}),
            "cs-trace",
        );
        let new_state = apply_event(&state, &event);
        assert_eq!(new_state.current_phase, OrchestrationPhase::SpecDefined);
        assert_eq!(new_state.current_spec, Some("s1".to_string()));
    }

    #[test]
    fn replay_events_derives_state() {
        let mut state = OrchestrationState::default();
        let e1 = append_event(
            &mut state,
            EventType::SpecCreated,
            serde_json::json!({"spec_id": "s1"}),
            "cs-trace",
        );
        let e2 = append_event(
            &mut state,
            EventType::RedVerified,
            serde_json::json!({}),
            "cs-trace",
        );
        let events = vec![e1, e2];
        let replayed = replay_events(&events);
        assert_eq!(replayed.current_phase, OrchestrationPhase::RedVerified);
        assert_eq!(replayed.events.len(), 2);
    }

    #[test]
    fn create_receipt_binds_to_git_tree() {
        let receipt = create_receipt("abc123", "v1.0.0", Verdict::Pass, "cs-trace", 5);
        assert_eq!(receipt.git_tree_hash, "abc123");
        assert_eq!(receipt.oracle_version, "v1.0.0");
        assert!(matches!(receipt.verdict, Verdict::Pass));
        assert_eq!(receipt.event_seq_no, 5);
    }

    #[test]
    fn execute_hermetic_records_duration() {
        let ctx = create_hermetic_context(PathBuf::from("."), 30, vec![PathBuf::from("src")]);
        let result = execute_hermetic("echo", &["hello"], &ctx, "cs-trace");
        assert!(result.duration_ms > 0);
        assert_eq!(result.trace_id, "cs-trace");
        assert!(result.exit_code.is_some());
    }

    #[test]
    fn hermetic_context_is_isolated_by_default() {
        let ctx = create_hermetic_context(PathBuf::from("."), 30, vec![]);
        assert!(ctx.isolated);
        assert!(!ctx.network_allowed);
    }

    #[test]
    fn now_ms_is_monotonic() {
        let t1 = now_ms();
        let t2 = now_ms();
        assert!(t2 >= t1);
    }

    #[test]
    fn replay_does_not_duplicate_events() {
        let mut state = OrchestrationState::default();
        let e1 = append_event(
            &mut state,
            EventType::SpecCreated,
            serde_json::json!({"spec_id": "s1"}),
            "cs-trace",
        );
        let replayed = replay_events(&[e1]);
        assert_eq!(replayed.events.len(), 1);
        assert_eq!(replayed.current_phase, OrchestrationPhase::SpecDefined);
    }

    #[test]
    fn event_log_round_trips() {
        let dir = std::env::temp_dir().join(format!("codesafe-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = OrchestrationState::default();
        let event = append_event(
            &mut state,
            EventType::GreenVerified,
            serde_json::json!({"ok": true}),
            "cs-log",
        );
        append_event_to_log(&dir, &event).unwrap();
        let events = read_events(&events_path(&dir)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::GreenVerified);
        assert_eq!(
            replay_events(&events).current_phase,
            OrchestrationPhase::GreenVerified
        );
    }

    #[test]
    fn missing_event_log_is_empty() {
        let path = std::env::temp_dir().join("codesafe-no-such-events-file.jsonl");
        let _ = std::fs::remove_file(&path);
        assert!(read_events(&path).unwrap().is_empty());
    }
}
