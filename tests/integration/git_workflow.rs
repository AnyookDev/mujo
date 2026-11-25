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

    // Stage and commit using mujo's own arg builders. Whether this creates a
    // commit depends on the environment: pass's `git init` above commits the
    // store contents itself when git has an identity available (e.g. from the
    // host's global config), but on a host where git refuses to auto-detect
    // one, pass's commit fails and the contents are left staged for this one.
    // Either way the store must end up with work/github.gpg in history, so the
    // assertions below are about content, not about commit subjects.
    PassCommand::git(&["add", "-A"]).expect("git add should succeed");
    let commit_args = git::commit_args("Initial commit: add existing passwords");
    let refs: Vec<&str> = commit_args.iter().map(String::as_str).collect();
    PassCommand::git(&refs).expect("git commit should succeed");

    // The entry is in history. `log -- <path>` is empty only when the path was
    // never committed, whatever the subject of the commit that added it.
    let entry_log = PassCommand::git(&["log", "--oneline", "--", "work/github.gpg"])
        .expect("git log for the entry should succeed");
    assert!(
        !entry_log.trim().is_empty(),
        "work/github.gpg should be in history, got empty log"
    );

    // The blob in the repo is the encrypted entry, not the plaintext: the
    // stored file must never contain the password in the clear. The tree is
    // clean after the commit, so the working file is the committed blob.
    let entry_path = store.store.join("work/github.gpg");
    let raw = std::fs::read(&entry_path).expect("entry file should be readable");
    assert!(!raw.is_empty());
    assert!(
        !raw.windows(7).any(|w| w == b"hunter2"),
        "stored entry must be GPG-encrypted, not plaintext"
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

    // The remote actually received the entry. Check the path in the pushed
    // history rather than a commit subject, for the same reason as above.
    let out = Command::new("git")
        .arg("-C")
        .arg(&remote_path)
        .args(["log", "--oneline", "--", "work/github.gpg"])
        .output()
        .expect("remote log should run");
    assert!(out.status.success(), "remote should have a valid history");
    let remote_log = String::from_utf8_lossy(&out.stdout);
    assert!(
        !remote_log.trim().is_empty(),
        "remote should contain the pushed entry in history, got empty log"
    );
}
