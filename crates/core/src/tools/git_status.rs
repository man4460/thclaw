use super::Tool;
use crate::error::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// Snapshot of the git working tree: uncommitted work, recent history.
///
/// Exists to kill the "claim something is missing, then find out it was
/// already built in a prior session and left uncommitted" loop. Before the
/// model asserts a feature doesn't exist or starts work, it can see — in
/// ONE read-only call — what's untracked, what's modified, and what was
/// just committed, instead of trusting `git log` (committed history) alone.
pub struct GitStatusTool;

const MAX_STATUS_LINES: usize = 200;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &'static str {
        "GitStatus"
    }

    fn parallelizable(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Snapshot of uncommitted work in the working tree: untracked files, \
         modified files (diff --stat), staged changes, recent commits, stash \
         list, and current branch. Read-only. Call it BEFORE claiming a \
         feature is missing or before starting work — a prior session may \
         have left the work uncommitted, and `git log` alone would not show it."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repo directory (default: current working directory)."
                },
                "recent": {
                    "type": "integer",
                    "description": "Number of recent commits to show (default 10)."
                }
            }
        })
    }

    async fn call(&self, input: Value) -> Result<String> {
        let recent = input
            .get("recent")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 100) as usize;

        let cwd = match input.get("path").and_then(Value::as_str) {
            Some(p) => crate::sandbox::Sandbox::check(p)?,
            None => crate::workdir::current_workdir(),
        };

        if !run(&["rev-parse", "--is-inside-work-tree"], &cwd).status {
            return Ok(format!("{} is not a git repository.", cwd.display()));
        }

        let mut out = String::new();

        let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"], &cwd).trim;
        out.push_str(&format!("branch: {branch}\n"));

        let status = run(&["status", "--short", "--untracked-files=all"], &cwd).trim;
        if status.is_empty() {
            out.push_str("status: clean (no uncommitted changes)\n");
        } else {
            let lines: Vec<&str> = status.lines().collect();
            let capped = lines.len() > MAX_STATUS_LINES;
            let shown: Vec<&str> = lines.into_iter().take(MAX_STATUS_LINES).collect();
            out.push_str(&format!("status:\n{}\n", shown.join("\n")));
            if capped {
                out.push_str(&format!("  …[truncated at {MAX_STATUS_LINES} entries]\n"));
            }
        }

        let untracked_only = run(&["ls-files", "--others", "--exclude-standard"], &cwd).trim;
        if !untracked_only.is_empty() {
            out.push_str(&format!("\nuntracked:\n{untracked_only}\n"));
        }

        let diffstat = run(&["diff", "--stat"], &cwd).trim;
        if !diffstat.is_empty() {
            out.push_str(&format!("\nmodified (unstaged):\n{diffstat}\n"));
        }

        let staged = run(&["diff", "--staged", "--stat"], &cwd).trim;
        if !staged.is_empty() {
            out.push_str(&format!("\nstaged:\n{staged}\n"));
        }

        let log = run(
            &["log", "--oneline", "--decorate", &format!("-n{recent}")],
            &cwd,
        )
        .trim;
        out.push_str(&format!("\nrecent commits:\n{log}\n"));

        let stash = run(&["stash", "list"], &cwd).trim;
        if !stash.is_empty() {
            out.push_str(&format!("\nstash:\n{stash}\n"));
        }

        out.push_str(
            "\nTip: untracked/modified files above are work that may already \
             implement what you're about to build — check them before redoing it.",
        );

        Ok(out)
    }
}

struct GitOut {
    status: bool,
    trim: String,
}

fn run(args: &[&str], cwd: &Path) -> GitOut {
    let out = Command::new("git").args(args).current_dir(cwd).output();
    match out {
        Ok(o) => GitOut {
            status: o.status.success(),
            trim: String::from_utf8_lossy(&o.stdout).trim().to_string(),
        },
        Err(_) => GitOut {
            status: false,
            trim: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init_repo(dir: &Path) {
        run(&["init", "-q"], dir);
        run(&["config", "user.email", "t@t"], dir);
        run(&["config", "user.name", "t"], dir);
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&["add", "a.txt"], dir);
        run(&["commit", "-q", "-m", "init"], dir);
    }

    #[tokio::test]
    async fn reports_uncommitted_and_untracked_work() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        // Uncommitted modification + untracked file = the exact state the
        // tool exists to surface (prior session left work in the tree).
        fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.path().join("new_feature.rs"), "fn x() {}").unwrap();

        let out = GitStatusTool
            .call(json!({ "path": dir.path().to_string_lossy() }))
            .await
            .unwrap();

        assert!(out.contains("branch:"), "should show branch: {out}");
        assert!(
            out.contains("new_feature.rs"),
            "untracked file listed: {out}"
        );
        assert!(
            out.contains("M a.txt") || out.contains("a.txt"),
            "modification listed: {out}"
        );
        assert!(out.contains("recent commits:"), "history section: {out}");
        assert!(out.contains("init"), "commit present: {out}");
    }

    #[tokio::test]
    async fn clean_repo_says_clean() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        let out = GitStatusTool
            .call(json!({ "path": dir.path().to_string_lossy() }))
            .await
            .unwrap();
        assert!(out.contains("clean"), "clean repo reported: {out}");
    }

    #[tokio::test]
    async fn non_repo_errors_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let out = GitStatusTool
            .call(json!({ "path": dir.path().to_string_lossy() }))
            .await
            .unwrap();
        assert!(out.contains("not a git repository"), "{out}");
    }
}
