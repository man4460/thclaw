use super::{read_walker, req_str, targets_hidden_path, Tool};
use crate::error::{Error, Result};
use async_trait::async_trait;
use globset::Glob;
use serde_json::{json, Value};

pub struct GrepTool;

/// Default cap on returned rows. A bare `Grep` over a large tree can match
/// thousands of lines; the whole payload then rides in context until
/// compaction, so the model pays for every one of them. 200 rows is enough
/// to see the shape of a result and decide to narrow — and the truncation
/// note tells it exactly how.
const DEFAULT_HEAD_LIMIT: usize = 200;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn parallelizable(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Search file contents for a regex pattern under a directory. Respects \
         .gitignore. Returns matching lines as `path:line:text`, one per line, \
         capped at `head_limit` (default 200); pass 0 for no cap. Set \
         `files_only: true` when you only need which files match — far cheaper \
         than every matching line."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex pattern"},
                "path":    {"type": "string", "description": "Base directory (default: cwd)"},
                "glob":    {"type": "string", "description": "Optional file filter, e.g. '*.rs'"},
                "head_limit": {
                    "type": "integer",
                    "description": format!("Max rows to return (default {DEFAULT_HEAD_LIMIT}, 0 = unlimited)")
                },
                "files_only": {
                    "type": "boolean",
                    "default": false,
                    "description": "Return just the paths of matching files (one per line), no line text"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, input: Value) -> Result<String> {
        let pattern = req_str(&input, "pattern")?;
        let raw_base = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let base = crate::sandbox::Sandbox::check(raw_base)?;
        let glob_filter = input.get("glob").and_then(Value::as_str);

        // Explicit dot-path target (e.g. grep inside `.thclaws/sessions/`)
        // → descend past the default hidden/gitignore filters.
        let include_hidden = targets_hidden_path([raw_base, glob_filter.unwrap_or("")]);

        let re = regex::Regex::new(pattern).map_err(|e| Error::Tool(format!("regex: {e}")))?;

        let glob_matcher = glob_filter
            .map(|g| Glob::new(g).map(|g| g.compile_matcher()))
            .transpose()
            .map_err(|e| Error::Tool(format!("glob filter: {e}")))?;

        let head_limit = input
            .get("head_limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_HEAD_LIMIT as u64) as usize;
        let files_only = input
            .get("files_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut results: Vec<String> = Vec::new();
        // BTreeSet: unique + sorted, so `files_only` output is stable across
        // runs without a second sort (and one file can't appear twice).
        let mut files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut truncated = false;
        let walker = read_walker(&base, include_hidden).build();

        'walk: for entry in walker.flatten() {
            let path = entry.path();
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if let Some(m) = &glob_matcher {
                // Match the file name alone to avoid dir-path false matches.
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                if !m.is_match(name.as_ref()) {
                    continue;
                }
            }
            let Ok(contents) = std::fs::read_to_string(path) else {
                continue;
            };
            for (i, line) in contents.lines().enumerate() {
                if !re.is_match(line) {
                    continue;
                }
                if files_only {
                    let key = path.display().to_string();
                    if head_limit > 0 && files.len() >= head_limit && !files.contains(&key) {
                        truncated = true;
                        break 'walk;
                    }
                    files.insert(key);
                    break; // one row per file
                }
                if head_limit > 0 && results.len() >= head_limit {
                    truncated = true;
                    break 'walk;
                }
                results.push(format!("{}:{}:{}", path.display(), i + 1, line));
            }
        }

        if files_only {
            let mut out: Vec<String> = files.into_iter().collect();
            if truncated {
                out.push(format!(
                    "[truncated at {head_limit} files — narrow the pattern, add path/glob, or raise head_limit]"
                ));
            }
            return Ok(out.join("\n"));
        }

        results.sort();
        let mut out = results.join("\n");
        if truncated {
            out.push_str(&format!(
                "\n[truncated at {head_limit} matches — narrow the pattern, add path/glob, or raise head_limit]"
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_tree() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn greet() -> String {\n    \"hello\".into()\n}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("README.md"), "# hello world\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn finds_matching_lines() {
        let dir = setup_tree();
        let out = GrepTool
            .call(json!({
                "pattern": "hello",
                "path": dir.path().to_string_lossy(),
            }))
            .await
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // 3 occurrences: main.rs:2, lib.rs:2, README.md:1
        assert_eq!(lines.len(), 3, "got: {out}");
        assert!(lines
            .iter()
            .any(|l| l.contains("main.rs:2:") && l.contains("hello")));
        assert!(lines
            .iter()
            .any(|l| l.contains("lib.rs:2:") && l.contains("hello")));
        assert!(lines.iter().any(|l| l.contains("README.md:1:")));
    }

    #[tokio::test]
    async fn regex_pattern_works() {
        let dir = setup_tree();
        let out = GrepTool
            .call(json!({
                "pattern": r"fn \w+\(\)",
                "path": dir.path().to_string_lossy(),
            }))
            .await
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // fn main() and fn greet()
        assert_eq!(lines.len(), 2, "got: {out}");
    }

    #[tokio::test]
    async fn glob_filter_restricts_files() {
        let dir = setup_tree();
        let out = GrepTool
            .call(json!({
                "pattern": "hello",
                "path": dir.path().to_string_lossy(),
                "glob": "*.md"
            }))
            .await
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("README.md:"));
    }

    #[tokio::test]
    async fn no_matches_returns_empty() {
        let dir = setup_tree();
        let out = GrepTool
            .call(json!({
                "pattern": "nosuchpattern",
                "path": dir.path().to_string_lossy(),
            }))
            .await
            .unwrap();
        assert_eq!(out, "");
    }

    /// A broad pattern over a big tree must not dump everything into the
    /// context. `head_limit` caps it and the note says so, so the model
    /// narrows instead of assuming it saw the whole result.
    #[tokio::test]
    async fn head_limit_truncates_and_says_so() {
        let dir = tempdir().unwrap();
        for n in 0..10 {
            std::fs::write(dir.path().join(format!("f{n}.txt")), "needle\n").unwrap();
        }
        let out = GrepTool
            .call(json!({
                "pattern": "needle",
                "path": dir.path().to_string_lossy(),
                "head_limit": 3
            }))
            .await
            .unwrap();
        let mut lines = out.lines();
        let rows: Vec<&str> = (&mut lines).take(3).collect();
        assert_eq!(rows.len(), 3, "got: {out}");
        assert!(rows.iter().all(|l| l.contains("needle")), "got: {out}");
        assert!(out.contains("[truncated at 3 matches"), "got: {out}");
    }

    #[tokio::test]
    async fn head_limit_zero_means_unlimited() {
        let dir = tempdir().unwrap();
        for n in 0..10 {
            std::fs::write(dir.path().join(format!("f{n}.txt")), "needle\n").unwrap();
        }
        let out = GrepTool
            .call(json!({
                "pattern": "needle",
                "path": dir.path().to_string_lossy(),
                "head_limit": 0
            }))
            .await
            .unwrap();
        assert_eq!(out.lines().count(), 10, "got: {out}");
        assert!(!out.contains("truncated"), "got: {out}");
    }

    /// `files_only` is the cheap "where does this live" query: one row per
    /// file, no line text, no duplicates even when several lines match.
    #[tokio::test]
    async fn files_only_returns_unique_paths() {
        let dir = setup_tree();
        let out = GrepTool
            .call(json!({
                "pattern": "hello",
                "path": dir.path().to_string_lossy(),
                "files_only": true
            }))
            .await
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "one row per file: {out}");
        assert!(
            lines.iter().all(|l| !l.contains(':')),
            "no line numbers expected: {out}"
        );
        assert!(lines.iter().any(|l| l.ends_with("main.rs")), "got: {out}");
    }

    #[tokio::test]
    async fn files_only_respects_head_limit() {
        let dir = tempdir().unwrap();
        for n in 0..5 {
            std::fs::write(dir.path().join(format!("f{n}.txt")), "needle\nneedle\n").unwrap();
        }
        let out = GrepTool
            .call(json!({
                "pattern": "needle",
                "path": dir.path().to_string_lossy(),
                "files_only": true,
                "head_limit": 2
            }))
            .await
            .unwrap();
        assert!(out.contains("[truncated at 2 files"), "got: {out}");
        assert_eq!(out.lines().filter(|l| l.ends_with(".txt")).count(), 2);
    }

    #[tokio::test]
    async fn bad_regex_errors() {
        let err = GrepTool
            .call(json!({"pattern": "[unclosed", "path": "."}))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("regex"));
    }

    #[tokio::test]
    async fn respects_gitignore() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "secret").unwrap();
        std::fs::write(dir.path().join("visible.txt"), "secret").unwrap();
        // Make it look like a git repo so ignore actually applies .gitignore.
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();

        let out = GrepTool
            .call(json!({
                "pattern": "secret",
                "path": dir.path().to_string_lossy(),
            }))
            .await
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1, "got: {out}");
        assert!(lines[0].contains("visible.txt"));
    }
}
