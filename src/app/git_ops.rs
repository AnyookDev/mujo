use super::{App, PULL_CONFIRM_SENTENCE};
use crate::git;
use crate::models::{AppMode, GitPaneState, SyncStatus};
use crate::pass::PassCommand;
use anyhow::Result;
use crossterm::{
    execute,
    style::Print,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{stdout, Write};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

impl App {
    fn prompts_on_tty(args: &[&str]) -> bool {
        matches!(args.first(), Some(&"push") | Some(&"pull") | Some(&"fetch"))
    }

    /// Hand the terminal over to a child that may prompt on /dev/tty (git
    /// push/pull/fetch asking ssh for a password, or the pass commands
    /// that invoke gpg and therefore pinentry-curses, which switches the
    /// terminal to the normal screen on exit while mujo keeps drawing on
    /// the alternate screen) and run `f`, restoring the TUI afterwards.
    /// While handed over: raw mode off, alternate screen left, so the
    /// child's prompt is readable on the normal screen; afterwards the
    /// alternate screen is re-entered and needs_clear tells the event
    /// loop to clear before the next draw, since EnterAlternateScreen
    /// clears on entry and ratatui's diff would otherwise consider every
    /// cell unchanged. The handover only runs when a TUI actually owns
    /// the screen (tui_active): under `cargo test` no TUI is running, and
    /// the escape sequences would go straight to fd 1, bypassing
    /// libtest's capture and garbling the terminal. The unit-test build
    /// still records the handover decision (needs_clear) so the gating
    /// stays observable.
    pub(crate) fn with_terminal_handover<T>(
        &mut self,
        interactive: bool,
        context: Option<String>,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let on_tty = interactive && self.tui_active && !cfg!(test);
        if on_tty {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
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
                let _ = execute!(stdout(), EnterAlternateScreen);
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
    /// For push/pull/fetch the terminal is handed over first so an ssh
    /// prompt is readable (see with_terminal_handover).
    pub(super) fn git_cmd(&mut self, args: &[&str]) -> Result<String> {
        let interactive = Self::prompts_on_tty(args) && self.tui_active;
        let on_tty = interactive && !cfg!(test);
        // One context line so the silent case (e.g. a passwordless
        // YubiKey key that only needs a touch) still shows that mujo
        // handed the terminal to git and is not frozen. The handover
        // helper emits it after leaving the alternate screen, so it lands
        // on the visible terminal where the child's teed output follows.
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
    pub(super) fn commit_staged(&mut self, message: &str) -> Result<()> {
        self.git_cmd_owned(&git::commit_args(message))?;
        // If HEAD still does not resolve, the commit did not land - do not
        // report success.
        self.git_cmd(&["rev-parse", "--verify", "HEAD"])?;
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
