use super::{App, PULL_CONFIRM_SENTENCE};
use crate::git;
use crate::models::{AppMode, GitPaneState, SyncStatus};
use crate::pass::PassCommand;
use anyhow::Result;
use crossterm::{
    execute,
    style::Print,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io::{stdout, Write};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

impl App {
    fn prompts_on_tty(args: &[&str]) -> bool {
        matches!(args.first(), Some(&"push") | Some(&"pull") | Some(&"fetch"))
    }

    /// Run a child that may prompt on /dev/tty (git push/pull/fetch asking
    /// ssh for a password) and run `f`, restoring the terminal afterwards.
    ///
    /// The child stays on the ALTERNATE screen: git and ssh write their
    /// prompts straight to /dev/tty, which is readable whether the
    /// alternate or the normal screen is active, so the old switch to the
    /// normal screen (and back) was never needed to see the prompt - and
    /// leaving the alternate screen revealed the bare terminal for an
    /// instant before mujo repainted, which is the flash this used to
    /// cause. Leaving raw mode IS needed: with it off, the tty is in
    /// canonical mode and line-buffered, so the child can read a typed
    /// passphrase (and the user still sees it echoed, as git expects).
    /// needs_clear is still set: the child wrote its own output onto the
    /// alternate screen, so ratatui's buffer no longer matches the
    /// terminal and the next frame must repaint in full rather than diff.
    /// The handover only runs when a TUI actually owns the screen
    /// (tui_active): under `cargo test` no TUI is running, and the
    /// terminal-mode calls would garble libtest's output. The unit-test
    /// build still records the decision (needs_clear) so the gating stays
    /// observable.
    pub(crate) fn with_terminal_handover<T>(
        &mut self,
        interactive: bool,
        context: Option<String>,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let on_tty = interactive && self.tui_active && !cfg!(test);
        if on_tty {
            let _ = disable_raw_mode();
            let _ = stdout().flush();
            // One context line so the silent case (e.g. a passwordless
            // YubiKey key that only needs a touch) still shows that mujo
            // handed the terminal to the child and is not frozen. Written
            // to stderr, where the child's teed output follows.
            if let Some(line) = context {
                eprintln!("{}", line);
            }
        }
        let result = f();
        if interactive && self.tui_active {
            if on_tty {
                let _ = enable_raw_mode();
                let _ = stdout().flush();
            }
            self.needs_clear = true;
        }
        result
    }

    /// Run a gpg-invoking pass command with the alt screen already up, then
    /// repair afterwards. pinentry-curses emits ?1049l on exit, which drops
    /// the terminal to the normal screen, so re-assert the alt screen with
    /// ?1049h. DECSET 47 (?47h) and 1047 are ignored by some terminals,
    /// making the repair a silent no-op there; 1049 is honoured (it is the
    /// same sequence crossterm's EnterAlternateScreen emits at startup).
    /// 1049h CLEARS the alt screen on entry, which needs_clear already
    /// absorbs via the full repaint before the next draw.
    pub(crate) fn with_alt_repair<T>(
        &mut self,
        interactive: bool,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let result = f();
        if interactive && self.tui_active {
            if !cfg!(test) {
                let _ = execute!(stdout(), Print("\x1b[?1049h"));
                let _ = stdout().flush();
            }
            self.needs_clear = true;
        }
        result
    }

    /// Run a git subcommand in the store, capturing stdout/stderr. Git is
    /// invoked directly (`git -C <store>`) so its real exit status
    /// propagates - `pass git` exits 0 regardless of what git did, which
    /// made every failure invisible. EXCEPTION: `init` still goes through
    /// `pass git init`, whose cmd_git adds .gitattributes and makes the
    /// initial commit, with a reliable status (`git "$@" || exit 1`).
    /// For push/pull/fetch the terminal is handed over first so a /dev/tty
    /// prompt from git or ssh is readable without the flash of leaving the
    /// alternate screen (see with_terminal_handover).
    pub(super) fn git_cmd(&mut self, args: &[&str]) -> Result<String> {
        let interactive = Self::prompts_on_tty(args) && self.tui_active;
        let on_tty = interactive && !cfg!(test);
        let context = if on_tty {
            Some(format!("mujo: git {}", args.first().copied().unwrap_or("")))
        } else {
            None
        };
        // Interactive runs tee the child's stderr to the (now visible)
        // terminal while still capturing it, so prompts like OpenSSH's
        // "Confirm user presence for key ..." are seen live AND end up in
        // git_output. Non-interactive runs keep the plain captured path.
        let store = self.store_path.clone();
        self.with_terminal_handover(interactive, context, || {
            if args.first() == Some(&"init") {
                PassCommand::git(args)
            } else if on_tty {
                PassCommand::git_in_tee(&store, args)
            } else {
                PassCommand::git_in(&store, args)
            }
        })
    }

    /// Like git_cmd but for argument lists that own their strings (built by
    /// the git module's command builders).
    fn git_cmd_owned(&mut self, args: &[String]) -> Result<String> {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.git_cmd(&refs)
    }

    /// The remote URL of the password store for `remote`, resolved from git
    /// itself with config.toml as fallback. Used to label sync messages in
    /// the Git pane.
    fn remote_url(&mut self, remote: &str) -> String {
        self.git_cmd(&["remote", "get-url", remote])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.config.git.remote_url.clone())
    }

    /// The store's configured git remotes, in `git remote` order, run
    /// through the same non-interactive `git -C <store>` path as the other
    /// plain queries. Empty when the store directory is absent or git
    /// lists nothing: callers treat an empty list as "no remote
    /// configured" rather than falling back to a hardcoded "origin" that
    /// git would then fail to resolve.
    fn remotes(&self) -> Vec<String> {
        if !self.store_path.exists() {
            return Vec::new();
        }
        PassCommand::git_in(&self.store_path, &["remote"])
            .map(|out| {
                out.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve the remote to push/pull, in order of preference:
    /// (a) the remote of the branch's tracked upstream, parsed from
    ///     `git rev-parse --abbrev-ref --symbolic-full-name @{u}`
    ///     (e.g. "fudge/master" -> "fudge");
    /// (b) the sole configured remote when `git remote` lists exactly one;
    /// (c) a remote literally named "origin" when one exists;
    /// (d) otherwise Err describing the configuration problem: no remote
    ///     at all, or several remotes with no upstream to disambiguate.
    /// Callers surface the message on the Sync line and skip git
    /// entirely - the old fallback to a literal "origin" made git fail
    /// with "fatal: 'origin' does not appear to be a git repository",
    /// which reads like a broken URL rather than "you have no remote".
    fn resolve_remote_name(&mut self) -> std::result::Result<String, String> {
        if let Ok(upstream) =
            self.git_cmd(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        {
            if let Some(remote) = git::remote_from_upstream(upstream.trim()) {
                return Ok(remote);
            }
        }
        let remotes = self.remotes();
        if remotes.len() == 1 {
            return Ok(remotes[0].clone());
        }
        if remotes.iter().any(|r| r == "origin") {
            return Ok("origin".to_string());
        }
        if remotes.is_empty() {
            return Err(
                "no remote configured - git remote add <name> <url> (or set git.remote_url in config.toml)"
                    .to_string(),
            );
        }
        Err(
            "multiple remotes and no upstream - set one with git branch -u <remote>/<branch>"
                .to_string(),
        )
    }

    /// True when the branch's tracked upstream resolves to a remote. The
    /// push path derives --set-upstream from this: resolve_remote_name's
    /// Ok cases are exactly "upstream tracked" (flag not needed) and "a
    /// remote chosen without an upstream" (flag needed, so the first push
    /// establishes tracking).
    fn has_tracked_upstream(&mut self) -> bool {
        self.git_cmd(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
            .ok()
            .and_then(|upstream| git::remote_from_upstream(upstream.trim()))
            .is_some()
    }

    /// Record a push/pull that never reached git: resolve_remote_name
    /// could not pick a remote. No git command runs and no terminal
    /// handover happens - the failure is a configuration problem, and
    /// running git would only report a misleading "'origin' does not
    /// appear to be a git repository". The Sync line renders the fix-it
    /// message ("Push failed: no remote configured - ...") through the
    /// usual failed-sync path.
    fn sync_blocked(&mut self, action: &str, msg: String) {
        self.last_sync = Some(SyncStatus {
            action: action.to_string(),
            // Blocked before any remote could be resolved: none to name.
            remote: String::new(),
            url: String::new(),
            at: Instant::now(),
            success: false,
            detail: Self::short_sync_reason(&msg),
        });
        self.message = Some((
            format!("Git {} failed: {}", action.to_lowercase(), msg),
            Instant::now(),
        ));
    }

    /// The short Sync-line reason for a blocked sync. The full message
    /// ("no remote configured - git remote add ...") is too wide for the
    /// half-pane Sync line and truncates, so the line carries just the
    /// problem's name; the full text stays on the message bar and the
    /// Status hint line.
    fn short_sync_reason(msg: &str) -> String {
        if msg.contains("multiple remotes") {
            "multiple remotes".to_string()
        } else if msg.contains("no remote") {
            "no remote".to_string()
        } else {
            msg.to_string()
        }
    }

    /// The fallback Sync line for a successful sync whose git output was
    /// empty: "<verb> <remote> (<url>)". The resolved remote name is
    /// included so a local-path URL, or a store with several remotes,
    /// still says which remote was used.
    fn sync_fallback_line(verb: &str, remote: &str, url: &str) -> String {
        format!("{} {} ({})", verb, remote, url)
    }

    /// The Sync line's fallback label from a remote-tracking ref's reflog
    /// timestamp (%ct, unix seconds): "last remote update <relative>",
    /// through the same relative-time formatter as the in-session sync
    /// age. "(unknown)" when the reflog gave no parsable timestamp (e.g.
    /// the ref does not exist yet). The wording is deliberately neutral:
    /// a fetch updates the ref too, so the label must never imply push.
    fn format_remote_update_label(ts: Option<u64>, now: SystemTime) -> String {
        let age = ts
            .and_then(|t| std::time::UNIX_EPOCH.checked_add(Duration::from_secs(t)))
            .and_then(|t| now.duration_since(t).ok());
        match age {
            Some(age) => format!("last remote update {}", crate::ui::relative_time(age)),
            None => "(unknown)".to_string(),
        }
    }

    /// Read the remote-tracking ref's last update time via its reflog and
    /// format the Sync line's fallback label. Called from
    /// refresh_git_info, so the git round trip happens per refresh, never
    /// per frame.
    fn last_remote_update_label(&mut self, remote: &str, branch: &str) -> String {
        let tracking = format!("{}/{}", remote, branch);
        let ts = self
            .git_cmd(&["reflog", "show", "-1", "--format=%ct", &tracking])
            .ok()
            .and_then(|out| out.trim().parse::<u64>().ok());
        Self::format_remote_update_label(ts, SystemTime::now())
    }

    /// The current branch name, for explicit `push <remote> <branch>` /
    /// `pull <remote> <branch>`. None on a detached or unborn HEAD, where
    /// no branch ref resolves.
    fn current_branch(&mut self) -> Option<String> {
        self.git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD")
    }

    pub fn git_is_initialized(&self) -> bool {
        self.store_path.join(".git").exists()
    }

    pub fn open_git_pane(&mut self) {
        self.git_scroll = 0;
        self.git_h_scroll = 0;
        self.command_buffer.clear();
        // last_sync is intentionally NOT reset here: the last sync outcome
        // should stay visible until the next push/pull (or app restart),
        // so the user sees "Pushed to <url> 5 min ago" when they return.
        if self.git_is_initialized() {
            self.git_pane = GitPaneState::Ready;
            self.refresh_git_info();
        } else {
            self.git_pane = GitPaneState::Uninitialized;
        }
        self.mode = AppMode::Git;
    }

    fn apply_git_config_remote(&mut self) {
        // Operate on the resolved remote (upstream's remote, sole remote,
        // or "origin") rather than a hardcoded "origin". With no usable
        // remote resolve_remote_name errs; the (Some(cfg), None) arm below
        // then auto-configures "origin" from config.toml via
        // git_add_remote, so probe that name - but only when no remotes
        // exist at all (e.g. .git was recreated). With remotes present but
        // none named "origin", the resolver's ambiguity error stands
        // instead of being silently resolved by conjuring a third remote.
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(_) => "origin".to_string(),
        };
        let git_url = self
            .git_cmd(&["remote", "get-url", &remote])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let config_url = self.config.git.remote_opt().map(|s| s.to_string());

        match (config_url, git_url) {
            // Both set: if config.toml was edited externally with a different
            // URL, treat config.toml as the source of truth and update git.
            (Some(cfg), Some(git)) => {
                if cfg != git && self.git_cmd(&["remote", "set-url", &remote, &cfg]).is_ok() {
                    self.git_output = format!("Remote {} updated to {}", remote, cfg);
                }
            }
            // Only in config.toml (e.g. .git was recreated): auto-configure.
            // git_add_remote also persists the URL back to config.toml.
            // Only conjure a remote when none exists: with remotes present
            // but none named "origin", the resolver's ambiguity error should
            // stand rather than be silently resolved by adding a third
            // remote.
            (Some(cfg), None) => {
                if self.remotes().is_empty() {
                    let _ = self.git_add_remote(&cfg);
                }
            }
            // Only in git: persist it to config.toml so it survives a
            // recreated .git directory.
            (None, Some(git)) => {
                self.config.git.remote_url = git;
                let _ = self.config.save();
            }
            (None, None) => {}
        }
    }

    pub fn refresh_git_info(&mut self) {
        if !self.git_is_initialized() {
            self.git_pane = GitPaneState::Uninitialized;
            return;
        }

        self.apply_git_config_remote();

        // An empty repo (no commits yet) has no HEAD: commands that resolve
        // it fail with raw git stderr like "ambiguous argument 'HEAD'".
        // Detect that case up front and handle it gracefully instead.
        let has_commits = self
            .git_cmd(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .is_ok();

        self.git_branch = if has_commits {
            self.git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| "(unknown)".to_string())
                .trim()
                .to_string()
        } else {
            // symbolic-ref works on an unborn branch (e.g. "main").
            self.git_cmd(&["symbolic-ref", "--short", "HEAD"])
                .map(|s| format!("{} (no commits yet)", s.trim()))
                .unwrap_or_else(|_| "(no commits yet)".to_string())
        };

        // Structured status: branch line gives upstream/ahead/behind, the
        // remaining lines list dirty files. The first line doubles as the
        // short status display (same format as `git status -sb`).
        let status_out = self
            .git_cmd(&["--no-pager", "status", "--porcelain", "-b"])
            .unwrap_or_default();
        self.git_status_short = status_out.lines().next().unwrap_or("").to_string();
        let status = git::parse_status(&status_out);
        self.git_has_upstream = status.upstream.is_some();
        self.git_ahead = status.ahead;
        self.git_behind = status.behind;
        self.git_uncommitted = status.uncommitted;
        // Whether any remote exists at all, from the same refresh that
        // sets ahead/behind and has_upstream: the Status row omits the
        // remote name and tracking reads "no remote" when this is false.
        self.git_has_remote = !self.remotes().is_empty();
        // Keep the pane's remote label in sync with what push/pull will
        // actually use - never a hardcoded "origin". Empty when no remote
        // resolves (the row omits the prefix in that case).
        self.git_remote_name = self.resolve_remote_name().unwrap_or_default();

        // Derive the Sync line's fallback label from the remote-tracking
        // ref's reflog, piggybacking on this refresh (never per frame).
        // Only with an upstream and commits: otherwise the no-upstream
        // logic applies and there is no ref to read a reflog from.
        self.git_last_remote_update = if has_commits && self.git_has_upstream {
            // Clone the field strings before the &mut self call: passing
            // &self.git_remote_name into last_remote_update_label would
            // hold an immutable borrow of self across its mutable one
            // (E0502).
            let remote = self.git_remote_name.clone();
            let branch = self.git_branch.clone();
            Some(self.last_remote_update_label(&remote, &branch))
        } else {
            None
        };

        self.git_log = if has_commits {
            self.git_cmd(&[
                "--no-pager",
                "log",
                "-n",
                "50",
                "--pretty=format:%h %ad %s",
                "--date=format:%Y-%m-%d %H:%M",
            ])
            .unwrap_or_else(|_| "(no commits yet)".to_string())
        } else {
            "No commits yet".to_string()
        };
    }

    /// True only if BOTH user.name and user.email resolve to non-empty
    /// values (git accepts empty-string config values, so a successful
    /// exit is not enough - the value must be non-empty too).
    fn git_identity_configured(&mut self) -> bool {
        let name = self
            .git_cmd(&["config", "user.name"])
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let email = self
            .git_cmd(&["config", "user.email"])
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        !name.is_empty() && !email.is_empty()
    }

    /// Run the commit and VERIFY it landed. Returns the actual git error
    /// on failure so the UI never reports success for a commit that did
    /// not happen.
    ///
    /// The verification compares HEAD before and after, not merely whether
    /// it resolves: `git commit` can exit 0 without creating a commit - a
    /// staged tree that matches HEAD is a no-op ("nothing to commit") - and
    /// on a repo that already has commits `rev-parse --verify HEAD` still
    /// succeeds, so the old check could not tell a landed commit from a
    /// HEAD that never moved. Direct git exits 1 for the no-op, but the
    /// `pass git` wrapper swallows that to 0, so the guard keeps the no-op
    /// from ever reading as success on either path.
    pub(super) fn commit_staged(&mut self, message: &str) -> Result<()> {
        let before = self.head_rev();
        self.git_cmd_owned(&git::commit_args(message))?;
        let after = self.head_rev();
        Self::verify_commit_moved(before.as_deref(), after.as_deref())
    }

    /// Classify a commit from HEAD's before/after ids. Ok when HEAD moved,
    /// including unborn (None) -> a real id, which is the first commit and
    /// the one legitimate move from nothing. Err when HEAD is unchanged -
    /// same id, or still unborn - since then git reported success without
    /// creating a commit.
    fn verify_commit_moved(before: Option<&str>, after: Option<&str>) -> Result<()> {
        if before == after {
            return Err(anyhow::anyhow!(
                "git reported success but HEAD did not move - nothing was staged to commit"
            ));
        }
        Ok(())
    }

    /// Seed every empty directory under the store with an empty .gitkeep:
    /// git cannot track empty directories, so without this a folder created
    /// in the UI would be lost by a clone. Both pass and mujo skip dotfiles
    /// when listing, so the marker stays invisible. .git itself is excluded.
    fn add_gitkeeps_to_empty_dirs(&self) {
        for entry in WalkDir::new(&self.store_path)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_dir()
                && std::fs::read_dir(entry.path())
                    .map(|mut rd| rd.next().is_none())
                    .unwrap_or(false)
            {
                let _ = std::fs::write(entry.path().join(".gitkeep"), "");
            }
        }
    }

    pub fn git_init(&mut self) -> Result<()> {
        let out = self.git_cmd(&["init"])?;
        self.git_output = out;
        self.command_buffer.clear();

        // Fix existing empty folders before counting: each seeded .gitkeep
        // shows up as a pending file and is captured by the initial commit.
        self.add_gitkeeps_to_empty_dirs();

        // Count pending password files. `status --porcelain` covers both
        // untracked files and anything already staged by an earlier failed
        // init attempt.
        let pending = self
            .git_cmd(&["status", "--porcelain"])?
            .lines()
            .filter(|l| l.contains(".gpg"))
            .count();
        self.git_pending_count = pending;

        if pending > 0 {
            self.git_cmd(&["add", "-A"])?;

            // git refuses to commit without user.name/user.email. Instead of
            // failing (or silently inventing an identity), ask the user.
            if !self.git_identity_configured() {
                self.git_identity_name.clear();
                self.git_pane = GitPaneState::AskIdentityName;
                self.message = Some((
                    "Git requires user.name and user.email to commit".to_string(),
                    Instant::now(),
                ));
                return Ok(());
            }

            self.finish_initial_commit();
            return Ok(());
        }

        self.message = Some(("Git initialized".to_string(), Instant::now()));
        self.git_pane = GitPaneState::AskRemote;
        Ok(())
    }

    /// Commit the staged files and report the real outcome. Called both
    /// from git_init (identity already configured) and after the user
    /// enters a name/email in the identity prompts.
    fn finish_initial_commit(&mut self) {
        let pending = self.git_pending_count;
        match self.commit_staged("Initial commit: add existing passwords") {
            Ok(()) => {
                // Only claim success now that HEAD verifiably exists.
                self.git_output = format!("Initial commit: {} passwords", pending);
                self.message = Some((
                    format!(
                        "Git initialized and committed {} existing passwords",
                        pending
                    ),
                    Instant::now(),
                ));
            }
            Err(e) => {
                // Surface the actual git error instead of a generic success.
                self.message = Some((
                    format!("Git initialized, but the initial commit failed: {}", e),
                    Instant::now(),
                ));
            }
        }
        self.git_pending_count = 0;
        self.git_pane = GitPaneState::AskRemote;
        self.command_buffer.clear();
    }

    /// Called after the user enters their email in the identity prompt.
    /// Sets the identity store-locally and retries the initial commit.
    pub fn finish_git_identity(&mut self, email: String) {
        let name = self.git_identity_name.trim().to_string();
        let _ = self.git_cmd(&["config", "user.name", &name]);
        let _ = self.git_cmd(&["config", "user.email", email.trim()]);

        if self.git_pending_count > 0 {
            self.finish_initial_commit();
        } else {
            self.message = Some(("Git identity configured".to_string(), Instant::now()));
            self.git_pane = GitPaneState::AskRemote;
            self.command_buffer.clear();
        }
    }

    /// User declined to configure an identity: do NOT fake success. Tell
    /// them exactly what is staged and how to finish manually.
    pub fn skip_git_identity(&mut self) {
        self.message = Some((
            if self.git_pending_count > 0 {
                format!(
                    "{} file(s) staged but NOT committed. Configure user.name/user.email, then run 'pass git commit'",
                    self.git_pending_count
                )
            } else {
                "Git identity setup skipped".to_string()
            },
            Instant::now(),
        ));
        self.git_pending_count = 0;
        self.git_pane = GitPaneState::AskRemote;
        self.command_buffer.clear();
    }

    pub fn git_add_remote(&mut self, url: &str) -> Result<()> {
        let out = self.git_cmd(&["remote", "add", "origin", url])?;
        self.git_output = out;

        // Persist the remote URL to mujo's config.toml as well, so it
        // stays in sync with git's config and survives a recreated .git.
        self.config.git.remote_url = url.trim().to_string();
        if let Err(e) = self.config.save() {
            self.message = Some((
                format!("Warning: could not save config: {}", e),
                Instant::now(),
            ));
        }
        Ok(())
    }

    /// True when git's own output says the sync was a no-op: push prints
    /// "Everything up-to-date" and pull prints "Already up to date." when
    /// the remote already matches. Matched on the up-to-date wording rather
    /// than a whole line, so either phrasing (and any surrounding output)
    /// classifies correctly.
    fn output_is_up_to_date(out: &str) -> bool {
        let lower = out.to_lowercase();
        lower.contains("up-to-date") || lower.contains("up to date")
    }

    /// The SyncStatus action for a successful sync, read from the outcome
    /// instead of hardcoded: "Up to date" when git reported nothing to
    /// transfer, otherwise the caller's past-tense verb ("Pushed to" /
    /// "Pulled from").
    fn sync_success_action(verb: &str, out: &str) -> String {
        if Self::output_is_up_to_date(out) {
            "Up to date".to_string()
        } else {
            verb.to_string()
        }
    }

    /// The SyncStatus action for a successful pull, derived from repository
    /// state rather than git's text: a merge over a local-path remote
    /// prints "Already up to date." and never "Merge made by", so text
    /// matching mislabels it. HEAD unchanged by the pull -> "Already up to
    /// date"; the new HEAD has more than one parent -> "Merged"; otherwise
    /// (single-parent new HEAD) -> "Fast-forwarded".
    fn classify_pull_outcome(
        before: Option<&str>,
        after: Option<&str>,
        parent_count: usize,
    ) -> &'static str {
        if before.is_some() && before == after {
            "Already up to date"
        } else if parent_count > 1 {
            "Merged"
        } else {
            "Fast-forwarded"
        }
    }

    /// The prompt line for AppMode::ConfirmDelete, which the delete flow and
    /// the pull-confirmation flow share (pending_pull says which). Only the
    /// question differs; the y/n handling in handle_confirm_delete is common
    /// to both. The pull sentence comes from PULL_CONFIRM_SENTENCE, the
    /// same const the status bar renders.
    pub fn confirm_prompt(&self) -> String {
        if self.pending_pull {
            format!("{}y:confirm n:cancel", PULL_CONFIRM_SENTENCE)
        } else {
            "Delete this entry? y:confirm n:cancel".to_string()
        }
    }

    /// True when a pull needs explicit confirmation: behind > 0 with either
    /// ahead > 0 (diverged - the pull will merge) or uncommitted changes
    /// (the merge may abort or conflict over a dirty tree). A clean
    /// fast-forward (behind > 0, ahead == 0, clean tree) and an up-to-date
    /// tree are not confirmed - neither has a side effect worth a prompt.
    pub(crate) fn pull_needs_confirm(ahead: u32, behind: u32, uncommitted: usize) -> bool {
        behind > 0 && (ahead > 0 || uncommitted > 0)
    }

    /// git's output when it is non-empty, else the formatted fallback line.
    /// A no-op push connects, exits 0 and may print nothing on stdout, so an
    /// empty output must not blank the pane.
    fn sync_output(out: &str, fallback: String) -> String {
        if out.trim().is_empty() {
            fallback
        } else {
            out.to_string()
        }
    }

    pub fn git_push(&mut self) -> Result<()> {
        let needs_upstream = !self.has_tracked_upstream();
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(msg) => {
                // No remote to push to: record the configuration problem
                // on the Sync line and return without running git (and
                // without the terminal handover) - git would only fail
                // with a misleading "'origin' does not appear to be a git
                // repository".
                self.sync_blocked("Push", msg);
                return Ok(());
            }
        };
        let branch = self.current_branch();
        let url = self.remote_url(&remote);
        match self.git_cmd_owned(&git::push_args(&remote, branch.as_deref(), needs_upstream)) {
            Ok(out) => {
                let action = Self::sync_success_action("Pushed to", &out);
                self.git_output =
                    Self::sync_output(&out, Self::sync_fallback_line("Pushed to", &remote, &url));
                self.last_sync = Some(SyncStatus {
                    action,
                    remote,
                    url,
                    at: Instant::now(),
                    success: true,
                    detail: String::new(),
                });
                self.message = Some(("Git: pushed".to_string(), Instant::now()));
            }
            Err(e) => {
                self.git_output = e.to_string();
                self.last_sync = Some(SyncStatus {
                    action: "Push".to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: false,
                    detail: e.to_string(),
                });
                self.message = Some(("Git push failed".to_string(), Instant::now()));
                self.refresh_git_info();
                return Err(e);
            }
        }
        self.refresh_git_info();
        Ok(())
    }

    /// The current HEAD commit id, None on an unborn or unresolvable HEAD.
    fn head_rev(&mut self) -> Option<String> {
        self.git_cmd(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// The number of parents of the current HEAD commit (0 when HEAD is
    /// unborn or unresolvable; a root commit has no parents).
    fn head_parent_count(&mut self) -> usize {
        self.git_cmd(&["show", "-s", "--format=%P", "HEAD"])
            .map(|s| s.split_whitespace().count())
            .unwrap_or(0)
    }

    /// The arguments for a pull: the git module's pull arguments plus
    /// "--no-rebase". git 2.27+ refuses a divergent pull with "fatal: Need
    /// to specify how to reconcile divergent branches" unless the user has
    /// pull.rebase or pull.ff configured; passing the strategy explicitly
    /// keeps mujo's merge-on-divergence behaviour independent of global
    /// config (a fast-forwardable pull still fast-forwards).
    fn pull_args_with_strategy(remote: &str, branch: Option<&str>) -> Vec<String> {
        let mut args = git::pull_args(remote, branch);
        args.push("--no-rebase".to_string());
        args
    }

    /// The arguments for the local merge of a fetched upstream ref.
    /// --no-edit matters: the child owns the terminal during the handover,
    /// so a merge-message editor must not open.
    fn merge_upstream_args(upstream: &str) -> Vec<String> {
        vec![
            "merge".to_string(),
            "--no-edit".to_string(),
            upstream.to_string(),
        ]
    }

    /// Fetch from the resolved remote without merging. Run before the
    /// pull-confirmation check: pull_needs_confirm reads the last-known
    /// ahead/behind counts, which are stale until a fetch, so an unfetched
    /// divergence would otherwise merge without prompting. Goes through the
    /// same git_cmd handover path as push/pull (fetch may prompt for ssh
    /// credentials). A missing remote is reported through the usual
    /// blocked-sync path and returns Ok, like git_push/git_pull.
    pub fn git_fetch(&mut self) -> Result<()> {
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(msg) => {
                self.sync_blocked("Pull", msg);
                return Ok(());
            }
        };
        match self.git_cmd(&["fetch", &remote]) {
            Ok(out) => {
                let url = self.remote_url(&remote);
                self.git_output = Self::sync_output(
                    &out,
                    Self::sync_fallback_line("Fetched from", &remote, &url),
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// The shared pull flow, used by both the git pane's `p` and Normal
    /// mode's `p`. A pull that would merge (or may abort over a dirty
    /// tree) is the one accidental keypress with a side effect, so it is
    /// confirmed first with the same y/n mechanism the delete flow uses; a
    /// clean fast-forward merges immediately.
    ///
    /// The confirm predicate reads the last-known status, which is stale
    /// until a fetch: fetch first, refresh the git info, THEN evaluate
    /// pull_needs_confirm on the fresh ahead/behind/uncommitted counts. An
    /// unfetched divergence would otherwise merge without prompting. The
    /// fetch IS the network half of the pull, so what follows is the
    /// local-only merge (git_merge_upstream) - calling git_pull here would
    /// fetch a second time, costing a second ssh round trip, password
    /// prompt and terminal handover. The confirmation bar itself is the
    /// prompt, so no extra message is set here.
    ///
    /// Records self.mode in pull_return_mode before raising the
    /// confirmation, so handle_confirm_delete returns the user to where
    /// the pull was asked from (the git pane or Normal mode).
    ///
    /// A blocked fetch (no remote, or no upstream to merge from) returns
    /// Ok and has already reported why on the Sync line, so the merge step
    /// is skipped when either is missing: running `git merge @{u}` anyway
    /// would only overwrite the fix-it message with a raw git error like
    /// "fatal: No upstream configured for branch 'master'".
    pub fn pull_with_confirm(&mut self) {
        self.pull_return_mode = self.mode;
        if let Err(e) = self.git_fetch() {
            self.message = Some((format!("Git fetch failed: {}", e), Instant::now()));
        } else {
            self.refresh_git_info();
            // refresh_git_info just re-derived both flags from the store,
            // so they reflect the state the fetch ran against.
            if !self.git_has_remote || !self.git_has_upstream {
                return;
            }
            if App::pull_needs_confirm(self.git_ahead, self.git_behind, self.git_uncommitted) {
                self.pending_pull = true;
                self.mode = AppMode::ConfirmDelete;
            } else if let Err(e) = self.git_merge_upstream() {
                self.message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    pub fn git_pull(&mut self) -> Result<()> {
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(msg) => {
                // No remote to pull from: see git_push's blocked path.
                self.sync_blocked("Pull", msg);
                return Ok(());
            }
        };
        let branch = self.current_branch();
        let url = self.remote_url(&remote);
        // Capture HEAD before the pull: the outcome is classified from
        // repository state below, not from git's text (a merge over a
        // local-path remote prints "Already up to date." and never "Merge
        // made by", so text matching mislabels it).
        let before_head = self.head_rev();
        match self.git_cmd_owned(&Self::pull_args_with_strategy(&remote, branch.as_deref())) {
            Ok(out) => {
                let after_head = self.head_rev();
                let parent_count = self.head_parent_count();
                let action = Self::classify_pull_outcome(
                    before_head.as_deref(),
                    after_head.as_deref(),
                    parent_count,
                );
                self.git_output =
                    Self::sync_output(&out, Self::sync_fallback_line("Pulled from", &remote, &url));
                self.last_sync = Some(SyncStatus {
                    action: action.to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: true,
                    detail: String::new(),
                });
                self.message = Some(("Git: pulled".to_string(), Instant::now()));
                self.reload_entries()?;
            }
            Err(e) => {
                self.git_output = e.to_string();
                self.last_sync = Some(SyncStatus {
                    action: "Pull".to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: false,
                    detail: e.to_string(),
                });
                self.message = Some(("Git pull failed".to_string(), Instant::now()));
                self.refresh_git_info();
                return Err(e);
            }
        }
        self.refresh_git_info();
        Ok(())
    }

    /// Merge the already-fetched upstream ref into the current branch: the
    /// local half of a pull, run after git_fetch has refreshed the remote
    /// tracking ref. Splitting the two keeps the pull handler from
    /// fetching twice (two ssh round trips, password prompts and terminal
    /// handovers per pull). Records the outcome on the Sync line and
    /// reloads the entries, like git_pull.
    pub(crate) fn git_merge_upstream(&mut self) -> Result<()> {
        let remote = self.resolve_remote_name().unwrap_or_default();
        let url = if remote.is_empty() {
            String::new()
        } else {
            self.remote_url(&remote)
        };
        // Capture HEAD before the merge: the outcome is classified from
        // repository state, not from git's text (see classify_pull_outcome).
        let before_head = self.head_rev();
        match self.git_cmd_owned(&Self::merge_upstream_args("@{u}")) {
            Ok(out) => {
                let after_head = self.head_rev();
                let parent_count = self.head_parent_count();
                let action = Self::classify_pull_outcome(
                    before_head.as_deref(),
                    after_head.as_deref(),
                    parent_count,
                );
                self.git_output =
                    Self::sync_output(&out, Self::sync_fallback_line("Pulled from", &remote, &url));
                self.last_sync = Some(SyncStatus {
                    action: action.to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: true,
                    detail: String::new(),
                });
                self.message = Some(("Git: pulled".to_string(), Instant::now()));
                self.reload_entries()?;
            }
            Err(e) => {
                self.git_output = e.to_string();
                self.last_sync = Some(SyncStatus {
                    action: "Pull".to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: false,
                    detail: e.to_string(),
                });
                self.message = Some(("Git pull failed".to_string(), Instant::now()));
                self.refresh_git_info();
                return Err(e);
            }
        }
        self.refresh_git_info();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::EnvGuard;
    use std::process::Command;

    fn temp_app(tui_active: bool) -> App {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");
        app.tui_active = tui_active;
        app
    }

    #[test]
    fn noop_push_reports_up_to_date() {
        let action = App::sync_success_action("Pushed to", "Everything up-to-date");
        assert_eq!(action, "Up to date");
        assert_ne!(action, "Pushed to");
    }

    #[test]
    fn real_push_keeps_pushed_wording() {
        let out = "To /tmp/store\n   abc123..def456  main -> main\n";
        assert_eq!(App::sync_success_action("Pushed to", out), "Pushed to");
    }

    #[test]
    fn noop_pull_reports_up_to_date() {
        let action = App::sync_success_action("Pulled from", "Already up to date.");
        assert_eq!(action, "Up to date");
        assert_ne!(action, "Pulled from");
    }

    #[test]
    fn real_pull_keeps_pulled_wording() {
        let out = "Updating abc123..def456\nFast-forward\n  file | 1 +\n";
        assert_eq!(App::sync_success_action("Pulled from", out), "Pulled from");
    }

    #[test]
    fn pull_needs_confirm_classifies_outcomes() {
        // Diverged: the pull will merge.
        assert!(App::pull_needs_confirm(1, 2, 0));
        // Dirty tree and behind: the merge may abort or conflict.
        assert!(App::pull_needs_confirm(0, 1, 3));
        // Clean fast-forward: no confirmation.
        assert!(!App::pull_needs_confirm(0, 2, 0));
        // Up to date and clean.
        assert!(!App::pull_needs_confirm(0, 0, 0));
        // Dirty but not behind: the pull is a no-op.
        assert!(!App::pull_needs_confirm(0, 0, 5));
    }

    #[test]
    fn empty_output_falls_back_to_formatted_line() {
        assert_eq!(
            App::sync_output("", "Pushed to origin".to_string()),
            "Pushed to origin"
        );
        assert_eq!(
            App::sync_output("   \n\t", "Pushed to origin".to_string()),
            "Pushed to origin"
        );
        assert_eq!(
            App::sync_output("To origin\n", "fallback".to_string()),
            "To origin\n"
        );
    }

    #[test]
    fn pull_args_include_no_rebase() {
        let args = App::pull_args_with_strategy("origin", Some("main"));
        assert!(
            args.iter().any(|a| a == "--no-rebase"),
            "pull args must contain --no-rebase: {:?}",
            args
        );
        assert!(args.contains(&"origin".to_string()));
        assert!(args.contains(&"main".to_string()));
    }

    #[test]
    fn merge_upstream_args_include_no_edit() {
        let args = App::merge_upstream_args("@{u}");
        assert!(
            args.iter().any(|a| a == "--no-edit"),
            "merge args must contain --no-edit: {:?}",
            args
        );
        assert!(args.contains(&"merge".to_string()));
        assert!(args.contains(&"@{u}".to_string()));
    }

    #[test]
    fn pull_outcome_classified_from_head_state() {
        // HEAD unchanged by the pull: nothing was transferred.
        assert_eq!(
            App::classify_pull_outcome(Some("abc123"), Some("abc123"), 1),
            "Already up to date"
        );
        // New HEAD with more than one parent: the pull merged.
        assert_eq!(
            App::classify_pull_outcome(Some("abc123"), Some("def456"), 2),
            "Merged"
        );
        // New HEAD with a single parent: fast-forward.
        assert_eq!(
            App::classify_pull_outcome(Some("abc123"), Some("def456"), 1),
            "Fast-forwarded"
        );
    }

    /// A remote-less store (git init only, no remote): git_fetch blocks
    /// with the "no remote configured" Sync line and pull_with_confirm
    /// must stop there - no merge may run whose raw git error ("No
    /// upstream configured for branch ...") overwrites that message.
    #[test]
    fn pull_with_confirm_without_remote_keeps_blocked_message() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);

        app.pull_with_confirm();

        // The Sync line still reflects the blocked fetch, not a merge
        // error, and names the actual problem.
        let sync = app.last_sync.expect("blocked fetch recorded on Sync line");
        assert!(!sync.success);
        assert!(
            sync.detail.contains("no remote"),
            "detail must name the missing remote: {}",
            sync.detail
        );
        assert!(
            app.message
                .as_ref()
                .map(|(m, _)| m.contains("no remote configured"))
                .unwrap_or(false),
            "message must keep the blocked-fetch wording"
        );
        // No merge ran: the confirmation prompt was never raised.
        assert!(!app.pending_pull);
        assert_eq!(app.mode, AppMode::Normal);
    }

    /// The pure classification behind commit_staged: HEAD that moved is
    /// success (including unborn -> real id, the first commit), HEAD that
    /// stayed put is the no-op git's exit 0 can hide.
    #[test]
    fn verify_commit_moved_classifies_head_transitions() {
        // Unborn HEAD becomes a real commit: the first commit is a success.
        App::verify_commit_moved(None, Some("abc123")).expect("unborn -> real must succeed");
        // HEAD moved to a new commit.
        App::verify_commit_moved(Some("abc123"), Some("def456")).expect("moved HEAD must succeed");
        // HEAD resolved before and after to the same id: nothing happened.
        let err = App::verify_commit_moved(Some("abc123"), Some("abc123"))
            .expect_err("unchanged HEAD must be rejected");
        assert!(
            err.to_string().contains("HEAD did not move"),
            "error must name the no-move cause: {}",
            err
        );
        // An unborn HEAD that is still unborn never produced a commit, so
        // it is not success either.
        assert!(
            App::verify_commit_moved(None, None).is_err(),
            "unborn HEAD that stayed unborn must be rejected"
        );
    }

    /// commit_staged against a real repo: a genuine commit (first and
    /// second) succeeds, and a commit with nothing staged is not reported
    /// as success. This exercises the real git path - whose own exit code
    /// for the no-op is 1, unlike the `pass git` wrapper's 0 - so the
    /// guard is what keeps the no-op from ever reading as success.
    #[test]
    fn commit_staged_rejects_noop_and_accepts_real_commit() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);

        // A real commit from an unborn HEAD: None -> Some must be success.
        std::fs::write(store_dir.join("a.txt"), "one\n").expect("write a.txt");
        run(&["add", "-A"]);
        app.commit_staged("first")
            .expect("first commit from unborn HEAD should succeed");

        // Same tree, nothing staged: this must not report success.
        assert!(
            app.commit_staged("noop").is_err(),
            "no-op commit must not report success"
        );

        // A real second commit still succeeds.
        std::fs::write(store_dir.join("b.txt"), "two\n").expect("write b.txt");
        run(&["add", "-A"]);
        app.commit_staged("second")
            .expect("second commit should succeed");
    }

    #[test]
    fn pull_confirm_prompt_differs_from_delete_prompt() {
        let mut app = temp_app(false);
        assert_eq!(
            app.confirm_prompt(),
            "Delete this entry? y:confirm n:cancel"
        );
        app.pending_pull = true;
        assert_eq!(
            app.confirm_prompt(),
            "Pull would merge - continue? y:confirm n:cancel"
        );
    }

    /// A store with no remotes: remotes() lists nothing (whether or not
    /// git is even initialized), a sole remote resolves to itself, and
    /// with none at all resolve_remote_name errs with the fix-it hint
    /// instead of returning a literal "origin" that git would fail to
    /// resolve.
    #[test]
    fn remotes_and_resolve_remote_name_without_remotes() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        // Store directory absent: empty, and no error.
        assert!(app.remotes().is_empty());

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        // Capture the path in a local before the closure: borrowing
        // app.store_path in the closure would hold an immutable borrow of
        // app across the later resolve_remote_name() call (E0502).
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        // A git-initialized store with no remote still lists none, and
        // resolve_remote_name reports the configuration problem.
        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        assert!(app.remotes().is_empty());
        let err = app.resolve_remote_name().expect_err("no remotes must err");
        assert!(
            err.contains("no remote configured"),
            "error must name the missing remote: {err}"
        );

        // Exactly one remote resolves to it without an upstream.
        run(&["remote", "add", "fudge", "/tmp/mujo-test-remote"]);
        assert_eq!(app.remotes(), vec!["fudge".to_string()]);
        assert_eq!(app.resolve_remote_name().ok(), Some("fudge".to_string()));
    }

    /// With remotes present but none named "origin", apply_git_config_remote
    /// must NOT conjure a third remote from config.toml's git.remote_url:
    /// the resolver's "multiple remotes" ambiguity error should stand as a
    /// prompt to act, not be silently resolved away.
    #[test]
    fn apply_git_config_remote_does_not_add_origin_when_remotes_exist() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        // Capture the path in a local before the closure: borrowing
        // app.store_path in the closure would hold an immutable borrow of
        // app across the later resolve_remote_name() call (E0502).
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["remote", "add", "fudge2", "/tmp/mujo-test-remote-a"]);
        run(&["remote", "add", "second", "/tmp/mujo-test-remote-b"]);

        app.config.git.remote_url = "/tmp/mujo-test-remote-config".to_string();
        app.apply_git_config_remote();

        let mut remotes = app.remotes();
        remotes.sort();
        assert_eq!(
            remotes,
            vec!["fudge2".to_string(), "second".to_string()],
            "no third remote may be conjured from config.toml"
        );
        let err = app
            .resolve_remote_name()
            .expect_err("ambiguity must still err");
        assert!(
            err.contains("multiple remotes"),
            "error must report the ambiguity: {err}"
        );
    }

    /// With no remotes at all (e.g. .git was recreated),
    /// apply_git_config_remote auto-configures "origin" from config.toml's
    /// git.remote_url, after which resolve_remote_name resolves to it.
    #[test]
    fn apply_git_config_remote_adds_origin_when_no_remotes() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);

        let url = "/tmp/mujo-test-remote-config".to_string();
        app.config.git.remote_url = url.clone();
        app.apply_git_config_remote();

        assert_eq!(app.remotes(), vec!["origin".to_string()]);
        assert_eq!(app.resolve_remote_name().ok(), Some("origin".to_string()));
        // The remote git ended up with points at config.toml's URL.
        assert_eq!(
            app.git_cmd(&["remote", "get-url", "origin"]).ok(),
            Some(url)
        );
    }

    #[test]
    fn git_cmd_without_tui_leaves_needs_clear_false() {
        let mut app = temp_app(false);
        assert!(!app.tui_active, "tui_active must default to false");

        // "push" is an interactive subcommand, but with tui_active false
        // git_cmd must take the plain captured path and never hand over
        // the terminal.
        let _ = app.git_cmd(&["push"]);
        assert!(!app.needs_clear);
    }

    #[test]
    fn git_cmd_with_tui_forces_repaint_for_interactive() {
        // The child writes onto the alternate screen, so ratatui's buffer
        // is stale afterwards and the next frame must repaint in full.
        // needs_clear does that - and does NOT mean the screen changed:
        // the alternate screen is never left (that was the flash).
        let mut app = temp_app(true);
        let _ = app.git_cmd(&["push"]);
        assert!(app.needs_clear);
    }

    #[test]
    fn git_cmd_with_tui_non_interactive_leaves_needs_clear_false() {
        let mut app = temp_app(true);
        let _ = app.git_cmd(&["status"]);
        assert!(!app.needs_clear);
    }

    #[test]
    fn handover_records_repaint_but_keeps_the_alternate_screen() {
        let mut app = temp_app(true);
        let _ = app.with_terminal_handover(true, None, || Ok::<(), anyhow::Error>(()));
        assert!(app.needs_clear);
    }

    #[test]
    fn non_interactive_handover_leaves_needs_clear_false() {
        // The background preview decrypt calls with interactive=false: it
        // must not force a repaint (and must not touch the terminal).
        let mut app = temp_app(true);
        let _ = app.with_terminal_handover(false, None, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    #[test]
    fn handover_does_nothing_without_a_tui() {
        let mut app = temp_app(false);
        let _ = app.with_terminal_handover(true, None, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    #[test]
    fn alt_repair_sets_needs_clear_for_interactive_with_tui() {
        let mut app = temp_app(true);
        let _ = app.with_alt_repair(true, || Ok::<(), anyhow::Error>(()));
        assert!(app.needs_clear);
    }

    #[test]
    fn alt_repair_requires_tui_active() {
        let mut app = temp_app(false);
        let _ = app.with_alt_repair(true, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    #[test]
    fn alt_repair_requires_interactive() {
        let mut app = temp_app(true);
        let _ = app.with_alt_repair(false, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    /// The fallback Sync line names the resolved remote, so a local-path
    /// URL (or several remotes) still says which remote was used.
    #[test]
    fn sync_fallback_line_names_remote() {
        let line = App::sync_fallback_line("Pushed to", "origin", "/home/me/pass-store.git");
        assert_eq!(line, "Pushed to origin (/home/me/pass-store.git)");
        assert!(line.contains("origin"));
    }

    /// Blocked-sync Sync lines carry the short reason (the half-pane line
    /// would otherwise truncate the full fix-it text).
    #[test]
    fn blocked_sync_reason_is_shortened() {
        assert_eq!(
            App::short_sync_reason(
                "no remote configured - git remote add <name> <url> (or set git.remote_url in config.toml)"
            ),
            "no remote"
        );
        assert_eq!(
            App::short_sync_reason(
                "multiple remotes and no upstream - set one with git branch -u <remote>/<branch>"
            ),
            "multiple remotes"
        );
    }

    /// The reflog-derived fallback label: "last remote update <relative>"
    /// through the shared relative-time formatter, "(unknown)" when the
    /// reflog gave no parsable timestamp.
    #[test]
    fn remote_update_label_formats_reflog_timestamp() {
        let now = SystemTime::now();
        let ts = now
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs()
            - 90;
        let label = App::format_remote_update_label(Some(ts), now);
        assert!(
            label.starts_with("last remote update "),
            "label must name the reflog source: {label}"
        );
        assert!(label.ends_with("1 min ago"), "{label}");
        assert_eq!(App::format_remote_update_label(None, now), "(unknown)");
    }
}
