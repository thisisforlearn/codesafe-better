//! Safety and reliability tests for the fix loop's workspace protection.

use codeguard::git;
use std::path::Path;
use std::process::Command;

fn git_cmd(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-C", dir.to_str().unwrap()])
        .args(args)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(dir: &Path) {
    git_cmd(dir, &["init", "-q"]);
    git_cmd(dir, &["config", "user.email", "test@example.com"]);
    git_cmd(dir, &["config", "user.name", "Test"]);
    git_cmd(dir, &["add", "-A"]);
    git_cmd(dir, &["commit", "-q", "-m", "initial"]);
}

#[test]
fn checkpoint_returns_new_commit_and_preserves_dirty_work() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    init_repo(dir.path());

    // Simulate `--force` on a dirty tree: the user has uncommitted work.
    std::fs::write(dir.path().join("a.txt"), "one plus user work").unwrap();

    let checkpoint = git::checkpoint(dir.path()).unwrap();
    let head = git::current_head(dir.path()).unwrap();
    assert_eq!(
        checkpoint, head,
        "checkpoint must return the commit that contains the captured work"
    );

    // A failed fix reverts to the checkpoint, not to before it.
    git::revert_to(dir.path(), &checkpoint).unwrap();
    let restored = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(
        restored, "one plus user work",
        "user changes captured by the checkpoint must survive a revert"
    );
}

#[test]
fn generated_artifacts_do_not_make_tree_dirty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    init_repo(dir.path());

    std::fs::write(dir.path().join("report.md"), "# report").unwrap();
    std::fs::write(dir.path().join("report.json"), "{}").unwrap();
    std::fs::create_dir_all(dir.path().join(".codesafe")).unwrap();
    std::fs::write(dir.path().join(".codesafe/agent-brief.json"), "{}").unwrap();
    assert!(!git::is_dirty(dir.path()).unwrap());

    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    assert!(git::is_dirty(dir.path()).unwrap());
}

#[test]
fn snapshot_round_trip_is_lossless_for_source_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();
    let snap = git::Snapshot::create(dir.path()).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "corrupted").unwrap();
    assert!(snap.changed());
    snap.restore().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        "fn main() {}\n"
    );
}

#[test]
fn changed_files_reports_modifications() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    std::fs::write(dir.path().join("b.txt"), "new").unwrap();
    let changed = git::changed_files(dir.path()).unwrap();
    assert!(changed.iter().any(|p| p.ends_with("a.txt")));
    assert!(changed.iter().any(|p| p.ends_with("b.txt")));
}

#[test]
fn file_at_revision_reads_committed_content() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "committed").unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "working").unwrap();
    let old = git::file_at_revision(dir.path(), "HEAD", "a.txt")
        .unwrap()
        .unwrap();
    assert_eq!(old, "committed");
}

#[test]
fn checkpoint_does_not_commit_generated_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    std::fs::write(dir.path().join("report.md"), "# generated").unwrap();
    std::fs::write(dir.path().join("report.json"), "{}").unwrap();
    std::fs::create_dir_all(dir.path().join(".codesafe")).unwrap();
    std::fs::write(dir.path().join(".codesafe/agent-brief.json"), "{}").unwrap();

    git::checkpoint(dir.path()).unwrap();

    let tracked = Command::new("git")
        .args(["-C", dir.path().to_str().unwrap(), "ls-files"])
        .output()
        .unwrap();
    let tracked = String::from_utf8_lossy(&tracked.stdout);
    assert!(tracked.contains("a.txt"));
    assert!(!tracked.contains("report.md"));
    assert!(!tracked.contains("report.json"));
    assert!(!tracked.contains(".codesafe"));
}
