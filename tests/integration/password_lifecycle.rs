//! Scenario 1: full password lifecycle against a real store - init with a
//! GPG key, add, verify encryption, edit, delete, verify removal.

use crate::common::TestStore;
use mujo::pass::PassCommand;

#[test]
fn test_full_password_lifecycle() {
    let Some(store) = TestStore::new() else {
        return;
    };

    // Init: pass init ran during setup and wrote the key id file.
    assert!(
        store.store.join(".gpg-id").exists(),
        "store must have a .gpg-id"
    );
    // The key id file must contain exactly the generated key.
    let gpg_id =
        std::fs::read_to_string(store.store.join(".gpg-id")).expect(".gpg-id should be readable");
    assert_eq!(
        gpg_id.trim(),
        store.gpg_key_id,
        ".gpg-id must name the generated GPG key"
    );

    // Add "work/github" through mujo's pass wrapper.
    PassCommand::insert(
        "work/github",
        "hunter2\nusername: octocat\nurl: https://github.com",
    )
    .expect("pass insert should succeed");

    // The encrypted file exists at the expected place in the store.
    let entry = store.store.join("work/github.gpg");
    assert!(entry.exists(), "work/github.gpg should exist in the store");

    // The file is encrypted: the raw bytes must not contain the plaintext.
    let raw = std::fs::read(&entry).expect("entry file should be readable");
    assert!(!raw.is_empty());
    assert!(
        !raw.windows(7).any(|w| w == b"hunter2"),
        "entry file must be GPG-encrypted, not plaintext"
    );

    // It decrypts back to exactly what was stored.
    let shown = PassCommand::show("work/github").expect("pass show should succeed");
    assert!(shown.contains("hunter2"));
    assert!(shown.contains("username: octocat"));

    // Edit the password (pass insert -m -f overwrites non-interactively;
    // mujo's insert wrapper is used for fresh entries, so the edit goes
    // through the pass binary directly with the force flag).
    let mut child = std::process::Command::new("pass")
        .args(["insert", "-m", "-f", "work/github"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("pass insert -m -f should spawn");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(b"correct horse battery staple\nusername: octocat\n")
        .expect("writing new content");
    let status = child.wait().expect("wait for pass insert");
    assert!(status.success(), "editing the entry should succeed");

    // The edit landed and the old password is gone.
    let shown = PassCommand::show("work/github").expect("pass show after edit");
    assert!(shown.contains("correct horse battery staple"));
    assert!(
        !shown.contains("hunter2"),
        "old password should be replaced"
    );

    // Delete the password.
    PassCommand::remove("work/github", false).expect("pass rm should succeed");

    // The file is gone and decryption now fails.
    assert!(!entry.exists(), "entry file should be removed after delete");
    assert!(
        PassCommand::show("work/github").is_err(),
        "pass show must fail after delete"
    );
}
