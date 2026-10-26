use super::{req_str, Tool};
use crate::error::{Error, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Per-project work journal: what was done, what's next, and where the
/// work is focused — plus the exact `path:start-end` ranges touched.
///
/// Why it exists: a coding session in a project can end mid-task (context
/// compaction, a new session, a different agent). Without a durable record
/// the next turn re-derives everything from scratch and re-reads whole
/// files. This keeps a short, append-only log under
/// `<project>/.thclaws/state/worklog.md` so the next session — or the same
/// one after compaction — can read ten lines and go straight to the right
/// region instead of exploring the codebase again.
///
/// The `files` field is the anchor: entries record `src/app.ts:120-180`,
/// and the caller reads exactly that span with `Read(offset, limit)`.
pub struct WorkLogTool;

/// How many recent entries `read` returns by default. Enough to pick up
/// where the last session left off, short enough that it never becomes
/// its own context problem.
const DEFAULT_TAIL: usize = 5;
const MAX_TAIL: usize = 50;

/// Keep the journal bounded — an append-only file that grows forever
/// eventually costs more to read than it saves.
const MAX_FILE_CHARS: usize = 60_000;

const FOCUS_HEADING: &str = "## Focus";

fn worklog_path(cwd: &Path) -> PathBuf {
    cwd.join(".thclaws").join("state").join("worklog.md")
}

fn now_stamp() -> String {
    // Local time without pulling in chrono: seconds since epoch is enough
    // for ordering, and the human-readable date comes from `date`-style
    // formatting below when available.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("t{secs}")
}

#[async_trait]
impl Tool for WorkLogTool {
    fn name(&self) -> &'static str {
        "WorkLog"
    }

    fn parallelizable(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Per-project work journal under `.thclaws/state/worklog.md`. Use it to \
         carry work across sessions and context compaction: `read` at the start \
         of a task in an existing project (what was done, what's next, which \
         file:line ranges are in play), `append` when you finish a meaningful \
         chunk, `focus` to record where the work is centred right now. Entries \
         keep `path:start-end` anchors so the next turn can Read exactly that \
         span instead of re-exploring the codebase. Read-only ops: `read`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["read", "append", "focus", "clear"],
                    "description": "read = show focus + recent entries (read-only). append = record a finished chunk. focus = set the current focus. clear = wipe the journal."
                },
                "summary": {
                    "type": "string",
                    "description": "append: one line describing what you did."
                },
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Paths touched, ideally as `path:start-end` (e.g. `src/server.rs:120-180`) so a later turn can Read that exact span."
                },
                "next": {
                    "type": "string",
                    "description": "append: the single next action for whoever picks this up."
                },
                "focus": {
                    "type": "string",
                    "description": "focus: where the work is centred right now (feature, bug, file)."
                },
                "tail": {
                    "type": "integer",
                    "description": "read: how many recent entries to return (default 5, max 50)."
                }
            },
            "required": ["op"]
        })
    }

    // Writing here mutates a project file, but it is the agent's own
    // scratch record — not user source. Prompting on every journal entry
    // would make the habit too expensive to keep.
    fn requires_approval(&self, _input: &Value) -> bool {
        false
    }

    async fn call(&self, input: Value) -> Result<String> {
        let op = req_str(&input, "op")?;
        let cwd = crate::workdir::current_workdir();
        let path = worklog_path(&cwd);

        match op {
            "read" => {
                let tail = input
                    .get("tail")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_TAIL as u64)
                    .clamp(1, MAX_TAIL as u64) as usize;
                let raw = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(format!(
                            "no worklog yet at {} — this project has no recorded work. \
                             Create one with op: \"append\" once you finish a chunk.",
                            path.display()
                        ));
                    }
                    Err(e) => {
                        return Err(Error::Tool(format!("read {}: {e}", path.display())));
                    }
                };
                Ok(render_read(&raw, tail, &path))
            }
            "append" => {
                let summary = req_str(&input, "summary")?.trim().to_string();
                if summary.is_empty() {
                    return Err(Error::Tool("summary must not be empty".into()));
                }
                let files = string_list(&input, "files");
                let next = input
                    .get("next")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());

                let entry = render_entry(&now_stamp(), &summary, &files, next);
                append_bounded(&path, &entry)?;

                // Recording a finished chunk changes where the work is
                // centred — but only if the caller didn't say otherwise,
                // so `focus` stays an explicit signal.
                Ok(format!(
                    "✓ worklog entry added ({}): {}\n  next: {}",
                    path.display(),
                    summary,
                    next.unwrap_or("—")
                ))
            }
            "focus" => {
                let focus = req_str(&input, "focus")?.trim().to_string();
                if focus.is_empty() {
                    return Err(Error::Tool("focus must not be empty".into()));
                }
                let files = string_list(&input, "files");
                let block = render_focus(&focus, &files);
                set_focus(&path, &block)?;
                Ok(format!("✓ focus set: {focus}"))
            }
            "clear" => match std::fs::remove_file(&path) {
                Ok(()) => Ok(format!("cleared {}", path.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok("no worklog to clear".into())
                }
                Err(e) => Err(Error::Tool(format!("clear {}: {e}", path.display()))),
            },
            other => Err(Error::Tool(format!(
                "unknown op '{other}' (expected read, append, focus or clear)"
            ))),
        }
    }
}

fn string_list(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn render_entry(stamp: &str, summary: &str, files: &[String], next: Option<&str>) -> String {
    let mut out = format!("\n## {stamp}\n- did: {summary}\n");
    if !files.is_empty() {
        out.push_str(&format!("- files: {}\n", files.join(", ")));
    }
    if let Some(n) = next {
        out.push_str(&format!("- next: {n}\n"));
    }
    out
}

fn render_focus(focus: &str, files: &[String]) -> String {
    let mut out = format!("{FOCUS_HEADING}\n{focus}\n");
    if !files.is_empty() {
        out.push_str(&format!("- files: {}\n", files.join(", ")));
    }
    out
}

/// Replace the `## Focus` block in place, leaving entries untouched.
fn set_focus(path: &Path, block: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in existing.lines() {
        if line.trim() == FOCUS_HEADING {
            skipping = true;
            continue;
        }
        if skipping {
            if line.starts_with("## ") {
                skipping = false; // next entry begins
            } else {
                continue;
            }
        }
        parts.push(line.to_string());
    }
    let body = parts.join("\n").trim_start().to_string();
    let out = format!("{block}{}", if body.is_empty() { String::new() } else { format!("\n{body}\n") });
    write_atomic(path, &out)
}

fn append_bounded(path: &Path, entry: &str) -> Result<()> {
    let mut existing = std::fs::read_to_string(path).unwrap_or_default();
    existing.push_str(entry);
    if existing.len() > MAX_FILE_CHARS {
        existing = trim_to_recent(&existing, MAX_FILE_CHARS / 2);
    }
    write_atomic(path, &existing)
}

/// Drop the oldest entries until the journal fits. The focus block and the
/// most recent entries are what a resuming session needs; older history is
/// what git already records.
fn trim_to_recent(text: &str, target: usize) -> String {
    let mut sections: Vec<&str> = text.split("\n## ").collect();
    if sections.len() <= 1 {
        return text.to_string();
    }
    let head = sections.remove(0).to_string(); // "# WorkLog\n" + focus
    let mut out = sections
        .iter()
        .map(|s| format!("\n## {s}"))
        .collect::<Vec<_>>();
    while out.iter().map(|s| s.len()).sum::<usize>() + head.len() > target && out.len() > 1 {
        out.remove(0);
    }
    format!("{head}{}", out.join(""))
}

fn render_read(raw: &str, tail: usize, path: &Path) -> String {
    let focus = raw
        .split("\n## ")
        .find(|s| s.starts_with("Focus"))
        .map(|s| s.trim_end().to_string());

    let entries: Vec<&str> = raw
        .split("\n## ")
        .filter(|s| !s.starts_with("Focus"))
        .skip(1) // the first chunk is the "# WorkLog" preamble
        .collect();
    let start = entries.len().saturating_sub(tail);

    let mut out = format!("worklog — {} ({} entries, showing last {})\n", path.display(), entries.len(), entries.len() - start);
    if let Some(f) = focus {
        out.push('\n');
        out.push_str(&f);
        out.push('\n');
    }
    if entries.is_empty() {
        out.push_str("\n(no entries yet)\n");
    } else {
        for e in &entries[start..] {
            out.push_str(&format!("\n## {}\n", e.trim_end()));
        }
    }
    out.push_str(
        "\nTip: the `files:` lines carry `path:start-end` anchors — Read exactly \
         those spans (offset/limit) instead of re-reading whole files.",
    );
    out
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Tool(format!("create {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| Error::Tool(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| Error::Tool(format!("rename to {}: {e}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_renders_anchors_and_next() {
        let e = render_entry(
            "t1",
            "wired the auth guard",
            &["src/auth.rs:40-90".to_string()],
            Some("add tests"),
        );
        assert!(e.contains("did: wired the auth guard"));
        assert!(e.contains("src/auth.rs:40-90"));
        assert!(e.contains("next: add tests"));
    }

    #[test]
    fn read_shows_focus_and_tail_only() {
        let raw = "# WorkLog\n## Focus\nport the importer\n\n## t1\n- did: a\n\n## t2\n- did: b\n\n## t3\n- did: c\n";
        let out = render_read(raw, 2, Path::new("/p/.thclaws/state/worklog.md"));
        assert!(out.contains("port the importer"), "{out}");
        assert!(out.contains("did: b") && out.contains("did: c"), "{out}");
        assert!(!out.contains("did: a"), "oldest entry trimmed: {out}");
    }

    #[test]
    fn focus_replace_keeps_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worklog.md");
        std::fs::write(&path, "## Focus\nold focus\n\n## t1\n- did: a\n").unwrap();
        set_focus(&path, "## Focus\nnew focus\n").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("new focus"), "{after}");
        assert!(!after.contains("old focus"), "{after}");
        assert!(after.contains("did: a"), "entry preserved: {after}");
    }

    #[test]
    fn trimming_keeps_recent_entries_and_focus() {
        let mut text = String::from("## Focus\nkeep me\n");
        for i in 0..50 {
            text.push_str(&format!("\n## t{i}\n- did: {}\n", "x".repeat(50)));
        }
        let trimmed = trim_to_recent(&text, 800);
        assert!(trimmed.contains("keep me"), "{trimmed}");
        assert!(trimmed.contains("t49"), "newest kept: {trimmed}");
        assert!(!trimmed.contains("t0\n"), "oldest dropped: {trimmed}");
    }
}
