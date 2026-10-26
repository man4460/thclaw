use super::{read_walker, Tool};
use crate::error::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Structural map of a codebase: a directory tree plus, per source file,
/// its top-level symbols (fn / struct / class / … with line numbers).
///
/// The point is orientation — the model sees *where things live* across the
/// whole repo without reading file bodies, then zooms into the files that
/// matter. It reuses [`crate::tools::read::outline_of`] for the per-file
/// symbol extraction, so the language coverage and the "no signature
/// syntax known" behavior match the `Read` tool's `outline` flag exactly.
pub struct RepoMapTool;

/// Cap on files we actually open. A monorepo can have thousands of source
/// files; opening all of them is the exact token sink this tool exists to
/// avoid. The walker still counts everything, so the header is honest.
const DEFAULT_MAX_FILES: usize = 200;
const MAX_FILES_CAP: usize = 500;

/// Skip opening source files larger than this. A 5 MB generated file
/// (bundled JS, a minified lib, a `.d.ts` dump) has no top-level structure
/// worth mapping and would just bloat the walk.
const MAX_FILE_BYTES: u64 = 1_000_000;

/// Hard cap on total output chars. The per-file symbol blocks are bounded,
/// but `max_files` × blocks can still run long; truncate defensively so a
/// map never rivals reading the files themselves.
const MAX_OUTPUT_CHARS: usize = 9_000;

#[async_trait]
impl Tool for RepoMapTool {
    fn name(&self) -> &'static str {
        "RepoMap"
    }

    fn parallelizable(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Map a directory into a compact structural index: a directory tree \
         plus, for each source file, its top-level symbols (fn/struct/class/… \
         with line numbers). Use it to orient in a large or unfamiliar \
         codebase before reading specific files — it shows where things live \
         without reading whole files. Respects .gitignore. Read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Base directory (default: current working directory)."
                },
                "max_files": {
                    "type": "integer",
                    "description": "Cap on source files whose symbols are listed (default 200, max 500)."
                },
                "ext": {
                    "type": "string",
                    "description": "Optional filter, e.g. \"rs\" or \"ts,tsx\". Only these extensions are mapped."
                }
            }
        })
    }

    async fn call(&self, input: Value) -> Result<String> {
        let raw_base = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let base = crate::sandbox::Sandbox::check(raw_base)?;

        let max_files = input
            .get("max_files")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_FILES)
            .clamp(1, MAX_FILES_CAP);

        let ext_filter: Option<Vec<String>> = input
            .get("ext")
            .and_then(Value::as_str)
            .map(|s| {
                s.split(',')
                    .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                    .filter(|e| !e.is_empty())
                    .collect()
            })
            .filter(|v: &Vec<String>| !v.is_empty());

        // Collect source files (extensions `outline_of` understands),
        // honoring the optional ext filter. Directories are derived from
        // file parents, so the tree reflects only dirs that contain code.
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for entry in read_walker(&base, false).build().flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if crate::tools::read::outline_matcher(&ext).is_none() {
                continue;
            }
            if let Some(filter) = &ext_filter {
                if !filter.contains(&ext) {
                    continue;
                }
            }
            // Skip generated/huge files up front so both the tree count
            // and the symbol listing agree (a 5 MB bundle has no structure
            // worth mapping and would bloat the walk).
            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
            }
            files.push(path.to_path_buf());
        }
        files.sort();

        if files.is_empty() {
            return Ok(format!(
                "No source files to map under {} ({}).",
                base.display(),
                ext_filter
                    .map(|v| format!("ext filter: {}", v.join(", ")))
                    .unwrap_or_else(|| "all supported languages".to_string())
            ));
        }

        // Directory tree: count source files under every ancestor dir of
        // every file, so a parent dir reflects everything nested beneath it
        // (not just its immediate children).
        let mut dir_counts: std::collections::BTreeMap<std::path::PathBuf, usize> =
            std::collections::BTreeMap::new();
        for f in &files {
            let rel = f.strip_prefix(&base).unwrap_or(f);
            let mut cur = rel.parent();
            while let Some(p) = cur {
                if p.as_os_str().is_empty() {
                    break;
                }
                *dir_counts.entry(p.to_path_buf()).or_default() += 1;
                cur = p.parent();
            }
        }

        let mut out = String::new();
        out.push_str(&format!(
            "RepoMap · {} · {} source file(s)\n\n",
            base.display(),
            files.len()
        ));

        let mut dirs: Vec<&std::path::PathBuf> = dir_counts.keys().collect();
        dirs.sort_by_key(|d| {
            d.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        });
        for d in &dirs {
            let depth = d.components().count();
            let name = d
                .components()
                .last()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            let indent = "  ".repeat(depth.saturating_sub(1).min(8));
            out.push_str(&format!(
                "{}{}/ ({})\n",
                indent,
                name,
                dir_counts.get(*d).copied().unwrap_or(0)
            ));
        }
        if !dirs.is_empty() {
            out.push('\n');
        }

        out.push_str(&format!("==== symbols (max {max_files} shown) ====\n"));

        // Per-file symbol blocks, capped by both `max_files` and the total
        // output budget. The header line of each block carries the relative
        // path, so the map stays grep-able.
        let mut listed = 0usize;
        for f in files.iter().take(max_files) {
            if out.len() >= MAX_OUTPUT_CHARS {
                break;
            }
            let rel = f.strip_prefix(&base).unwrap_or(f);
            let Ok(contents) = std::fs::read_to_string(f) else {
                continue;
            };
            if let Ok(block) = crate::tools::read::outline_of(rel, &contents) {
                out.push_str(&block);
                if !block.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
                listed += 1;
            }
        }

        if files.len() > max_files || listed < files.len() {
            out.push_str(&format!(
                "\n[...truncated: {} of {} files mapped — narrow with `path`/`ext`, \
                 raise `max_files`, or read specific files directly]\n",
                listed,
                files.len()
            ));
        }

        if out.len() > MAX_OUTPUT_CHARS {
            let mut cut = MAX_OUTPUT_CHARS;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push_str("\n[...map truncated to output budget]\n");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn seed(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(
            dir.join("src/main.rs"),
            "use std::fmt;\n\nfn main() {}\n\npub struct App {\n    name: String,\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn hello() -> &'static str { \"hi\" }\n\npub mod inner;\n",
        )
        .unwrap();
        std::fs::write(dir.join("tests/it.rs"), "#[test]\nfn works() {}\n").unwrap();
        std::fs::write(dir.join("tests/it.ts"), "export function tsOnly() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "not source\n").unwrap();
        // A generated/huge-ish file that must be skipped by the byte cap.
        std::fs::write(dir.join("src/big.rs"), "x".repeat(1_100_000)).unwrap();
    }

    #[tokio::test]
    async fn maps_symbols_and_tree_skipping_docs_and_big_files() {
        let dir = tempdir().unwrap();
        seed(dir.path());
        let out = RepoMapTool
            .call(json!({ "path": dir.path().to_string_lossy() }))
            .await
            .unwrap();
        assert!(
            out.contains("src/ (2)"),
            "tree should list src/ with 2 mapped files (big.rs skipped): {out}"
        );
        assert!(
            out.contains("tests/ (2)"),
            "tree should list tests/ with 2 files: {out}"
        );
        assert!(out.contains("fn main()"), "should map fn main: {out}");
        assert!(
            out.contains("pub struct App"),
            "should map struct App: {out}"
        );
        assert!(
            out.contains("pub fn hello"),
            "should map lib.rs symbol: {out}"
        );
        assert!(!out.contains("README.md"), "docs are not source: {out}");
        assert!(
            !out.contains("big.rs"),
            "oversized file must be skipped: {out}"
        );
    }

    #[tokio::test]
    async fn ext_filter_narrows_the_map() {
        let dir = tempdir().unwrap();
        seed(dir.path());
        let out = RepoMapTool
            .call(json!({ "path": dir.path().to_string_lossy(), "ext": "rs" }))
            .await
            .unwrap();
        assert!(out.contains("fn main()"), "rs symbols still present: {out}");
        assert!(
            !out.contains("tsOnly"),
            "non-rs symbols filtered out: {out}"
        );
        assert!(
            out.contains("fn works"),
            "tests/it.rs is .rs and stays: {out}"
        );
    }

    #[tokio::test]
    async fn empty_tree_reports_cleanly() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "hi").unwrap();
        let out = RepoMapTool
            .call(json!({ "path": dir.path().to_string_lossy() }))
            .await
            .unwrap();
        assert!(
            out.contains("No source files"),
            "should report no source files: {out}"
        );
    }
}
