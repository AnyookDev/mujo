//! Scenario 2: git workflow against a real store and a real (local bare)
//! remote: init, commit existing passwords, add remote, push, verify.

use crate::common::TestStore;
use mujo::git;
use mujo::pass::PassCommand;
use std::process::Command;

#[test]
fn test_git_workflow() {
    let Some(store) = TestStore::new() else {
        return;
    };

    // An existing password so the initial commit has content.
    PassCommand::insert("work/github", "hunter2\nusername: octocat")
        .expect("pass insert should succeed");

    // Init git in the password store via `pass git init`.
    PassCommand::git(&["init"]).expect("pass git init should succeed");
    assert!(
        store.store.join(".git").exists(),
        ".git should exist after init"
    );

    // git refuses to commit without an identity; configure one in the store.
    PassCommand::git(&["config", "user.name", "mujo test"]).expect("git config name");
    PassCommand::git(&["config", "user.email", "mujo@example.com"]).expect("git config email");

    // Stage and commit using mujo's own arg builders.
    PassCommand::git(&["add", "-A"]).expect("git add should succeed");
    let commit_args = git::commit_args("Initial commit: add existing passwords");
    let refs: Vec<&str> = commit_args.iter().map(String::as_str).collect();
    PassCommand::git(&refs).expect("git commit should succeed");

    // The commit exists in history.
    let log = PassCommand::git(&["log", "--oneline"]).expect("git log should succeed");
    assert!(
        log.contains("Initial commit"),
        "history should contain the initial commit, got: {}",
        log
    );

    // mujo's status parser reads real git output: clean tree on a branch.
    let status_out = PassCommand::git(&["status", "--porcelain", "-b"]).expect("git status");
    let status = git::parse_status(&status_out);
    assert!(status.is_clean(), "tree should be clean after commit");
    assert!(status.branch.is_some(), "status should resolve the branch");

    // Create a real local bare repo to act as the remote.
    let remote_path = store.root().join("remote.git");
    let out = Command::new("git")
        .args(["init", "--bare"])
        .arg(&remote_path)
        .output()
        .expect("git init --bare should spawn");
    assert!(out.status.success(), "bare remote should initialize");

    // Add the remote URL and read it back.
    PassCommand::git(&["remote", "add", "origin", remote_path.to_str().unwrap()])
        .expect("git remote add should succeed");
    let url = PassCommand::git(&["remote", "get-url", "origin"]).expect("get-url should succeed");
    assert_eq!(url.trim(), remote_path.to_str().unwrap());

    // Push using mujo's push arg builder (remote + current branch). No
    // upstream is tracked yet, so --set-upstream is passed, like the TUI's
    // first push.
    let push_args = git::push_args("origin", Some("HEAD"), true);
    let refs: Vec<&str> = push_args.iter().map(String::as_str).collect();
    PassCommand::git(&refs).expect("git push should succeed");

    // The remote actually received the commit.
    let out = Command::new("git")
        .arg("-C")
        .arg(&remote_path)
        .args(["log", "--oneline"])
        .output()
        .expect("remote log should run");
    assert!(out.status.success(), "remote should have a valid history");
    let remote_log = String::from_utf8_lossy(&out.stdout);
    assert!(
        remote_log.contains("Initial commit"),
        "remote should contain the pushed commit, got: {}",
        remote_log
    );
}
