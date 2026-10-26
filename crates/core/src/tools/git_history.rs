use super::{req_str, Tool};
use crate::error::{Error, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// Read-only git history, shaped for building a knowledge base.
///
/// Exists for the codebase-scholar agent: a learning vault is only as good
/// as its raw material, and "what changed, why, and in which files" is the
/// densest signal a repository carries. Every op here is bounded (commit
/// count, files per commit, bytes per field) so one call can never flood a
/// subagent's context — the caller asks for more, with narrower filters,
/// when it needs them.
pub struct GitHistoryTool;

const DEFAULT_COMMITS: usize = 20;
const MAX_COMMITS: usize = 200;
/// Per-commit body cap when `op: "show"` — a giant squashed merge commit
/// can carry a wall of text; the point is the decision, not the full log.
const MAX_BODY_CHARS: usize = 4_000;
const MAX_FILES_LISTED: usize = 60;

#[async_trait]
impl Tool for GitHistoryTool {
    fn name(&self) -> &'static str {
        "GitHistory"
    }

    fn parallelizable(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Read-only git history, shaped for learning from past work. \
         `op: \"commits\"` (default) lists recent commits with date, subject, \
         files changed and +/− counts; `op: \"show\"` returns one commit's full \
         message plus its per-file numstat. Use it to mine *why* the codebase \
         looks the way it does — conventions, reversals, and decisions — then \
         write what you learn into a KMS. Bounded: pass `max` for more commits, \
         `path` to scope, `since` (e.g. `2.weeks.ago`) to bound history."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["commits", "show"],
                    "description": "commits = recent history (default). show = one commit in detail."
                },
                "rev": {
                    "type": "string",
                    "description": "show: the commit to inspect (sha, HEAD~3, tag). Required for op=show."
                },
                "max": {
                    "type": "integer",
                    "description": "commits: how many to list (default 20, max 200)."
                },
                "path": {
                    "type": "string",
                    "description": "Repo directory (default: current working directory)."
                },
                "paths": {
                    "type": "string",
                    "description": "Optional pathspec to scope history, e.g. `crates/core/src`."
                },
                "since": {
                    "type": "string",
                    "description": "Optional git date bound, e.g. `2.weeks.ago`, `2026-01-01`."
                }
            }
        })
    }

    fn requires_approval(&self, _input: &Value) -> bool {
        false
    }

    async fn call(&self, input: Value) -> Result<String> {
        let op = input.get("op").and_then(Value::as_str).unwrap_or("commits");
        let cwd = match input.get("path").and_then(Value::as_str) {
            Some(p) => crate::sandbox::Sandbox::check(p)?,
            None => crate::workdir::current_workdir(),
        };
        if !ok(&["rev-parse", "--is-inside-work-tree"], &cwd) {
            return Err(Error::Tool(format!(
                "{} is not a git repository",
                cwd.display()
            )));
        }

        match op {
            "commits" => {
                let max = input
                    .get("max")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_COMMITS as u64)
                    .clamp(1, MAX_COMMITS as u64);
                let mut args: Vec<String> = vec![
                    "log".into(),
                    format!("-n{max}"),
                    // Unit separator between fields, record separator
                    // between commits — unambiguous to split, unlike `|`.
                    "--pretty=format:%h\x1f%ad\x1f%s".into(),
                    "--date=short".into(),
                    "--shortstat".into(),
                ];
                if let Some(since) = input.get("since").and_then(Value::as_str) {
                    args.push(format!("--since={since}"));
                }
                if let Some(paths) = input.get("paths").and_then(Value::as_str) {
                    args.push("--".into());
                    args.push(paths.into());
                }
                let raw = run(&args, &cwd);
                Ok(render_commits(&raw))
            }
            "show" => {
                let rev = req_str(&input, "rev")?;
                let body = run(&["show", "--no-color", "--stat", "-M", rev], &cwd);
                if body.trim().is_empty() {
                    return Err(Error::Tool(format!("no such revision '{rev}'")));
                }
                Ok(clip(&body, MAX_BODY_CHARS * 4))
            }
            other => Err(Error::Tool(format!(
                "unknown op '{other}' (expected commits or show)"
            ))),
        }
    }
}

/// Parse `git log --pretty=%h\x1f%ad\x1f%s --shortstat` into one block per
/// commit. The shortstat line follows its commit, separated by a blank line.
fn render_commits(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "no commits in range".into();
    }
    let mut out = String::new();
    let mut pending: Option<(String, String, String)> = None;
    for line in raw.lines() {
        if line.contains('\x1f') {
            if let Some((h, d, s)) = pending.take() {
                out.push_str(&format!("- {h} {d} {s}\n"));
            }
            let mut parts = line.split('\x1f');
            let h = parts.next().unwrap_or("").to_string();
            let d = parts.next().unwrap_or("").to_string();
            let s = parts.next().unwrap_or("").to_string();
            pending = Some((h, d, s));
        } else if let Some((h, d, s)) = pending.clone() {
            let stat = line.trim();
            if stat.is_empty() {
                out.push_str(&format!("- {h} {d} {s}\n"));
                pending = None;
            } else {
                let files = stat
                    .split(',')
                    .find(|p| p.contains("file"))
                    .map(|p| p.trim())
                    .unwrap_or("");
                out.push_str(&format!("- {h} {d} {s}  [{files}]\n"));
                pending = None;
            }
        }
    }
    if let Some((h, d, s)) = pending {
        out.push_str(&format!("- {h} {d} {s}\n"));
    }
    out.push_str(
        "\nTip: call `GitHistory(op: \"show\", rev: \"<sha>\")` on the commits \
         that changed design decisions — the message usually says why.",
    );
    out
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n…[truncated at {max} chars]", &s[..cut])
}

fn ok(args: &[&str], cwd: &Path) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run(args: &[impl AsRef<std::ffi::OsStr>], cwd: &Path) -> String {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(args: &[&str], dir: &Path) {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    }

    fn repo(dir: &Path) {
        git(&["init", "-q"], dir);
        git(&["config", "user.email", "t@t"], dir);
        git(&["config", "user.name", "t"], dir);
    }

    #[tokio::test]
    async fn lists_commits_with_stats() {
        let d = tempfile::tempdir().unwrap();
        repo(d.path());
        fs::write(d.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "."], d.path());
        git(&["commit", "-q", "-m", "feat: add a"], d.path());
        fs::write(d.path().join("b.txt"), "two\n").unwrap();
        git(&["add", "."], d.path());
        git(&["commit", "-q", "-m", "feat: add b"], d.path());

        let out = GitHistoryTool
            .call(json!({"path": d.path().to_string_lossy(), "max": 5}))
            .await
            .unwrap();
        assert!(out.contains("feat: add a"), "{out}");
        assert!(out.contains("feat: add b"), "{out}");
        assert!(out.contains("file"), "shortstat rendered: {out}");
    }

    #[tokio::test]
    async fn show_returns_message_and_numstat() {
        let d = tempfile::tempdir().unwrap();
        repo(d.path());
        fs::write(d.path().join("x.rs"), "fn a() {}\n").unwrap();
        git(&["add", "."], d.path());
        git(&["commit", "-q", "-m", "fix: why we did it"], d.path());

        let out = GitHistoryTool
            .call(json!({"path": d.path().to_string_lossy(), "op": "show", "rev": "HEAD"}))
            .await
            .unwrap();
        assert!(out.contains("fix: why we did it"), "{out}");
        assert!(out.contains("x.rs"), "file change shown: {out}");
    }

    #[tokio::test]
    async fn pathspec_scopes_history() {
        let d = tempfile::tempdir().unwrap();
        repo(d.path());
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        git(&["add", "."], d.path());
        git(&["commit", "-q", "-m", "src work"], d.path());
        fs::write(d.path().join("docs.md"), "x\n").unwrap();
        git(&["add", "."], d.path());
        git(&["commit", "-q", "-m", "docs work"], d.path());

        let out = GitHistoryTool
            .call(json!({"path": d.path().to_string_lossy(), "paths": "src"}))
            .await
            .unwrap();
        assert!(out.contains("src work"), "{out}");
        assert!(
            !out.contains("docs work"),
            "docs commit filtered out: {out}"
        );
    }

    #[tokio::test]
    async fn non_repo_errors() {
        let d = tempfile::tempdir().unwrap();
        let err = GitHistoryTool
            .call(json!({"path": d.path().to_string_lossy()}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a git repository"));
    }
}
