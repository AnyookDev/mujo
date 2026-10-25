//! Pure helpers for parsing git command output and building git argument
//! lists. Nothing here invokes the git binary, so all of it can be unit
//! tested without a repository.

/// A single modified/untracked file from `git status --porcelain` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusFile {
    pub index: char,
    pub worktree: char,
    pub path: String,
}

/// Structured result of parsing `git status` output. Handles both the
/// machine-readable `--porcelain -b` format (preferred: branch line plus
/// per-file XY entries) and the human-readable default format (branch and
/// ahead/behind sentences); file lists are only available in porcelain
/// output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusInfo {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty_files: Vec<StatusFile>,
    /// Staged, modified and untracked entries together - everything that
    /// keeps the working tree dirty while ahead/behind (commit counts)
    /// can both read zero.
    pub uncommitted: usize,
}

impl StatusInfo {
    pub fn is_clean(&self) -> bool {
        self.dirty_files.is_empty()
    }
}

/// One parsed line of `git log --pretty=format:"%h %ad %s"` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub hash: String,
    pub date: String,
    pub time: String,
    pub message: String,
}

/// Parse `git status` output (porcelain -b preferred, human format
/// tolerated) into structured information.
pub fn parse_status(output: &str) -> StatusInfo {
    let mut info = StatusInfo::default();

    for line in output.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("## ") {
            parse_branch_line(rest, &mut info);
        } else if let Some(branch) = line.strip_prefix("On branch ") {
            let branch = branch.trim();
            if !branch.is_empty() {
                info.branch = Some(branch.to_string());
            }
        } else if line.starts_with("No commits yet") {
            // Unborn branch (human format): no resolvable branch ref yet.
            info.branch = None;
        } else if let Some(path) = line.strip_prefix("?? ") {
            info.dirty_files.push(StatusFile {
                index: '?',
                worktree: '?',
                path: clean_path(path.trim()),
            });
        } else if is_porcelain_file_line(line) {
            let bytes = line.as_bytes();
            info.dirty_files.push(StatusFile {
                index: bytes[0] as char,
                worktree: bytes[1] as char,
                path: clean_path(line[3..].trim()),
            });
        } else if let Some(rest) = line.strip_prefix("Your branch is ahead of '") {
            if let Some(end) = rest.find("' by ") {
                info.upstream = Some(rest[..end].to_string());
                if let Some(n) = number_after(rest, "' by ") {
                    info.ahead = n;
                }
            }
        } else if let Some(rest) = line.strip_prefix("Your branch is behind '") {
            if let Some(end) = rest.find("' by ") {
                info.upstream = Some(rest[..end].to_string());
                if let Some(n) = number_after(rest, "' by ") {
                    info.behind = n;
                }
            }
        } else if let Some(rest) = line.strip_prefix("Your branch is up to date with '") {
            if let Some(end) = rest.find('\'') {
                info.upstream = Some(rest[..end].to_string());
            }
        }
        // Anything else (hints, blank lines, "nothing to commit...") is
        // simply ignored: a clean tree produces no file entries.
    }

    info.uncommitted = info.dirty_files.len();
    info
}

/// Parse the content of a porcelain branch line (after "## ").
fn parse_branch_line(rest: &str, info: &mut StatusInfo) {
    if rest.starts_with("No commits yet on ") || rest.starts_with("HEAD (no branch)") {
        info.branch = None;
        return;
    }

    let (branch_part, tracking) = match rest.find(" [") {
        Some(i) => (&rest[..i], Some(rest[i + 2..].trim_end_matches(']'))),
        None => (rest, None),
    };

    let (branch, upstream) = match branch_part.find("...") {
        Some(i) => (&branch_part[..i], Some(branch_part[i + 3..].to_string())),
        None => (branch_part, None),
    };

    let branch = branch.trim();
    if !branch.is_empty() {
        info.branch = Some(branch.to_string());
    }
    if let Some(upstream) = upstream {
        if !upstream.is_empty() {
            info.upstream = Some(upstream);
        }
    }
    if let Some(tracking) = tracking {
        for part in tracking.split(',') {
            let part = part.trim();
            if let Some(n) = part.strip_prefix("ahead ") {
                info.ahead = n.trim().parse().unwrap_or(info.ahead);
            } else if let Some(n) = part.strip_prefix("behind ") {
                info.behind = n.trim().parse().unwrap_or(info.behind);
            }
        }
    }
}

/// Porcelain file lines are "XY path" where X/Y are status codes. Require
/// the mandatory space after XY so human-format hint lines are not
/// mistaken for files.
fn is_porcelain_file_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    line.len() >= 4 && is_status_code(bytes[0]) && is_status_code(bytes[1]) && bytes[2] == b' '
}

fn is_status_code(b: u8) -> bool {
    matches!(
        b,
        b'M' | b'A' | b'D' | b'R' | b'C' | b'U' | b'?' | b'!' | b' '
    )
}

/// For renames ("old -> new") keep the destination path.
fn clean_path(path: &str) -> String {
    match path.find(" -> ") {
        Some(i) => path[i + 4..].to_string(),
        None => path.to_string(),
    }
}

fn number_after(s: &str, marker: &str) -> Option<u32> {
    let idx = s.find(marker)? + marker.len();
    let digits: String = s[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Parse one `git log --pretty=format:"%h %ad %s"` line (with
/// --date=format:"%Y-%m-%d %H:%M") into its parts. Returns None for lines
/// that do not look like a commit entry (e.g. "No commits yet").
pub fn parse_log_line(line: &str) -> Option<LogEntry> {
    let mut parts = line.splitn(4, ' ');
    let hash = parts.next()?;
    let date = parts.next()?;
    let time = parts.next()?;
    let message = parts.next()?;

    if hash.is_empty() || message.is_empty() || !is_valid_date(date) || !is_valid_time(time) {
        return None;
    }

    Some(LogEntry {
        hash: hash.to_string(),
        date: date.to_string(),
        time: time.to_string(),
        message: message.to_string(),
    })
}

fn is_valid_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Validate "HH:MM" with real clock ranges: hours 00-23, minutes 00-59.
fn is_valid_time(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    for (i, c) in bytes.iter().enumerate() {
        if i == 2 {
            continue;
        }
        if !c.is_ascii_digit() {
            return false;
        }
    }
    let hours: u32 = s[..2].parse().unwrap_or(u32::MAX);
    let minutes: u32 = s[3..].parse().unwrap_or(u32::MAX);
    hours <= 23 && minutes <= 59
}

/// Arguments for `pass git commit -m <message>`.
pub fn commit_args(message: &str) -> Vec<String> {
    vec!["commit".to_string(), "-m".to_string(), message.to_string()]
}

/// Extract the remote name from a tracked upstream ref like
/// "fudge/master" -> "fudge". The remote is everything before the first
/// slash; a ref with no slash is not an upstream tracking ref.
pub fn remote_from_upstream(upstream: &str) -> Option<String> {
    let (remote, _) = upstream.split_once('/')?;
    if remote.is_empty() {
        None
    } else {
        Some(remote.to_string())
    }
}

/// Pick the only remote from `git remote` output, but only when exactly
/// one is configured - with several remotes the choice is ambiguous.
pub fn single_remote(output: &str) -> Option<String> {
    let remotes: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.len() == 1 {
        Some(remotes[0].to_string())
    } else {
        None
    }
}

/// Arguments for `pass git push [--set-upstream] <remote> [branch]`.
/// `set_upstream` is true when the branch has no tracked upstream yet, so
/// the first push from the TUI also establishes tracking.
pub fn push_args(remote: &str, branch: Option<&str>, set_upstream: bool) -> Vec<String> {
    let mut args = vec!["push".to_string()];
    if set_upstream {
        args.push("--set-upstream".to_string());
    }
    args.push(remote.to_string());
    if let Some(b) = branch {
        args.push(b.to_string());
    }
    args
}

/// Arguments for `pass git pull <remote> [branch]`.
pub fn pull_args(remote: &str, branch: Option<&str>) -> Vec<String> {
    let mut args = vec!["pull".to_string(), remote.to_string()];
    if let Some(b) = branch {
        args.push(b.to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_status_clean() {
        // Human-readable `git status` output on a clean tree.
        let info = parse_status("On branch master\nnothing to commit, working tree clean\n");
        assert_eq!(info.branch.as_deref(), Some("master"));
        assert!(info.is_clean());
        assert!(info.dirty_files.is_empty());
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);

        // Porcelain output on a clean tree with an upstream.
        let info = parse_status("## master...origin/master\n");
        assert_eq!(info.branch.as_deref(), Some("master"));
        assert_eq!(info.upstream.as_deref(), Some("origin/master"));
        assert!(info.is_clean());
    }

    #[test]
    fn test_parse_git_status_ahead() {
        let info = parse_status(
            "On branch master\n\
             Your branch is ahead of 'origin/master' by 3 commits.\n\
             \x20 (use \"git push\" to publish your local commits)\n\
             nothing to commit, working tree clean\n",
        );
        assert_eq!(info.ahead, 3);
        assert_eq!(info.behind, 0);
        assert_eq!(info.upstream.as_deref(), Some("origin/master"));
        assert!(info.is_clean());

        // Porcelain -b format.
        let info = parse_status("## master...origin/master [ahead 3]\n");
        assert_eq!(info.ahead, 3);
        assert_eq!(info.behind, 0);
        assert_eq!(info.upstream.as_deref(), Some("origin/master"));
    }

    #[test]
    fn test_parse_git_status_behind() {
        let info = parse_status(
            "On branch master\n\
             Your branch is behind 'origin/master' by 2 commits.\n",
        );
        assert_eq!(info.behind, 2);
        assert_eq!(info.ahead, 0);
        assert_eq!(info.upstream.as_deref(), Some("origin/master"));

        // Porcelain -b format.
        let info = parse_status("## master...origin/master [behind 2]\n");
        assert_eq!(info.behind, 2);
        assert_eq!(info.ahead, 0);

        // Diverged: both counts present.
        let info = parse_status("## master...origin/master [ahead 1, behind 2]\n");
        assert_eq!(info.ahead, 1);
        assert_eq!(info.behind, 2);
    }

    #[test]
    fn test_parse_git_status_dirty() {
        let output = "\
## master...origin/master [ahead 1]
 M src/main.rs
M  staged.txt
?? new-file.txt
MM both-modified.txt
";
        let info = parse_status(output);
        assert!(!info.is_clean());
        assert_eq!(info.dirty_files.len(), 4);

        assert_eq!(info.dirty_files[0].path, "src/main.rs");
        assert_eq!(info.dirty_files[0].index, ' ');
        assert_eq!(info.dirty_files[0].worktree, 'M');

        assert_eq!(info.dirty_files[1].path, "staged.txt");
        assert_eq!(info.dirty_files[1].index, 'M');
        assert_eq!(info.dirty_files[1].worktree, ' ');

        assert_eq!(info.dirty_files[2].path, "new-file.txt");
        assert_eq!(info.dirty_files[2].index, '?');
        assert_eq!(info.dirty_files[2].worktree, '?');

        assert_eq!(info.dirty_files[3].path, "both-modified.txt");
        assert_eq!(info.dirty_files[3].index, 'M');
        assert_eq!(info.dirty_files[3].worktree, 'M');

        // Renames record the destination path.
        let info = parse_status("## master\nR  old-name.txt -> new-name.txt\n");
        assert_eq!(info.dirty_files.len(), 1);
        assert_eq!(info.dirty_files[0].path, "new-name.txt");
    }

    #[test]
    fn test_parse_git_status_uncommitted_count() {
        // Staged, modified and untracked entries count together.
        let info = parse_status(
            "## master...origin/master\n\
             M  staged.txt\n\
             \x20M modified.txt\n\
             ?? untracked.txt\n",
        );
        assert_eq!(info.uncommitted, 3);
        assert!(!info.is_clean());

        // A clean tree reports zero, so the UI can omit the clause.
        let info = parse_status("## master...origin/master\n");
        assert_eq!(info.uncommitted, 0);
        assert!(info.is_clean());
    }

    #[test]
    fn test_build_git_commit_command() {
        assert_eq!(
            commit_args("Initial commit"),
            vec!["commit", "-m", "Initial commit"]
        );
        // Messages with spaces and special characters pass through verbatim.
        assert_eq!(
            commit_args("fix: handle 'quotes' and \"double quotes\""),
            vec!["commit", "-m", "fix: handle 'quotes' and \"double quotes\""]
        );
    }

    #[test]
    fn test_build_git_push_command() {
        // Upstream present: push to that remote and branch, no --set-upstream.
        assert_eq!(
            push_args("fudge", Some("master"), false),
            vec!["push", "fudge", "master"]
        );
        // No upstream (fallback): --set-upstream establishes tracking.
        assert_eq!(
            push_args("origin", Some("main"), true),
            vec!["push", "--set-upstream", "origin", "main"]
        );
        // No branch resolved: push the remote's default.
        assert_eq!(push_args("origin", None, false), vec!["push", "origin"]);
    }

    #[test]
    fn test_build_git_pull_command() {
        assert_eq!(
            pull_args("fudge", Some("master")),
            vec!["pull", "fudge", "master"]
        );
        assert_eq!(pull_args("origin", None), vec!["pull", "origin"]);
    }

    #[test]
    fn test_remote_from_upstream() {
        assert_eq!(
            remote_from_upstream("fudge/master"),
            Some("fudge".to_string())
        );
        assert_eq!(
            remote_from_upstream("origin/main"),
            Some("origin".to_string())
        );
        // Slashed branch names: the remote is still the first component.
        assert_eq!(
            remote_from_upstream("origin/feature/x"),
            Some("origin".to_string())
        );
        // Not an upstream tracking ref.
        assert_eq!(remote_from_upstream("master"), None);
        assert_eq!(remote_from_upstream(""), None);
    }

    #[test]
    fn test_single_remote() {
        assert_eq!(single_remote("fudge\n"), Some("fudge".to_string()));
        assert_eq!(single_remote("fudge"), Some("fudge".to_string()));
        // Several remotes: ambiguous, no pick.
        assert_eq!(single_remote("fudge\norigin\n"), None);
        assert_eq!(single_remote(""), None);
    }

    #[test]
    fn test_parse_git_log_output() {
        let entry = parse_log_line("abc1234 2024-01-15 10:30 Fix login bug").unwrap();
        assert_eq!(entry.hash, "abc1234");
        assert_eq!(entry.date, "2024-01-15");
        assert_eq!(entry.time, "10:30");
        assert_eq!(entry.message, "Fix login bug");

        // Messages containing colons and parentheses are preserved whole.
        let entry = parse_log_line("deadbeef 2023-12-31 23:59 feat: add QR display (v2)").unwrap();
        assert_eq!(entry.hash, "deadbeef");
        assert_eq!(entry.date, "2023-12-31");
        assert_eq!(entry.time, "23:59");
        assert_eq!(entry.message, "feat: add QR display (v2)");

        // Non-commit lines are rejected.
        assert!(parse_log_line("No commits yet").is_none());
        assert!(parse_log_line("").is_none());
        assert!(parse_log_line("abc1234 not-a-date 10:30 msg").is_none());
        assert!(parse_log_line("abc1234 2024-01-15 99:99 msg").is_none());
        // Boundary times are valid.
        assert!(parse_log_line("abc1234 2024-01-15 00:00 msg").is_some());
        assert!(parse_log_line("abc1234 2024-01-15 23:59 msg").is_some());
        // Out-of-range hours/minutes are not.
        assert!(parse_log_line("abc1234 2024-01-15 24:00 msg").is_none());
        assert!(parse_log_line("abc1234 2024-01-15 10:60 msg").is_none());
    }

    #[test]
    fn test_parse_status_upstream_remote() {
        // The parsed upstream feeds remote_from_upstream, so the Sync pane
        // can name "fudge" from the raw "## master...fudge/master" line.
        let info = parse_status("## master...fudge/master\n");
        assert_eq!(info.upstream.as_deref(), Some("fudge/master"));
        assert_eq!(
            remote_from_upstream(info.upstream.as_deref().unwrap()),
            Some("fudge".to_string())
        );
        // No tracking section: no upstream, remote must come from fallback.
        let info = parse_status("## master\n");
        assert_eq!(info.upstream, None);
    }

    #[test]
    fn test_extract_branch_name() {
        assert_eq!(
            parse_status("## master...origin/master [ahead 1]\n")
                .branch
                .as_deref(),
            Some("master")
        );
        assert_eq!(
            parse_status("## feature-branch\n").branch.as_deref(),
            Some("feature-branch")
        );
        assert_eq!(
            parse_status("On branch feature-branch\n").branch.as_deref(),
            Some("feature-branch")
        );
        // Unborn branch and detached HEAD have no resolvable branch ref.
        assert_eq!(parse_status("## No commits yet on main\n").branch, None);
        assert_eq!(parse_status("## HEAD (no branch)\n").branch, None);
    }
}
