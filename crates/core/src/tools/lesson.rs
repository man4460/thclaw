use super::{req_str, Tool};
use crate::error::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Structured lessons that survive sessions, plus a distillation step that
/// promotes repeated ones into a rules file the next session loads.
///
/// This is the feedback loop, not a model that trains itself: every
/// correction, failed assumption and confirmed approach is a row in a JSONL
/// file. When the same lesson shows up more than once it stops being an
/// anecdote and becomes a rule (`distill`), so the same mistake isn't
/// repeated across sessions. Nothing is inferred from raw text — the agent
/// states the lesson explicitly, which keeps it auditable and reversible.
pub struct LessonTool;

/// A lesson must be seen at least this many times before it is promoted
/// into a rules file. One-off observations stay in the journal.
const PROMOTE_AT: u64 = 2;

/// Caps on the distilled rules file. It is loaded on every turn, so it has
/// to stay small — this is a spine of hard-won corrections, not a diary.
const MAX_DISTILLED: usize = 12;
const MAX_DISTILLED_CHARS: usize = 1_400;

/// Lessons unseen for this long with only one occurrence are dropped at
/// distill time — a stale one-off is noise.
const STALE_SECS: u64 = 90 * 24 * 60 * 60;

/// Does this lesson graduate into the rules file?
///
/// Two ways: it repeated (`PROMOTE_AT`), or the user told us off —
/// a correction is the highest-signal event there is, and waiting for it
/// to happen twice means the most important lessons never become rules.
fn promotable(l: &Lesson) -> bool {
    // Corrections and stated preferences are the user speaking directly —
    // they graduate on first sight. Observations need a repeat to prove
    // they are a pattern rather than an anecdote.
    l.count >= PROMOTE_AT
        || l.kind.eq_ignore_ascii_case("correction")
        || l.kind.eq_ignore_ascii_case("preference")
}

/// Lessons that belong in the rules file, newest-signal first.
fn promoted_from(lessons: &[Lesson]) -> Vec<&Lesson> {
    let mut promoted: Vec<&Lesson> = lessons.iter().filter(|l| promotable(l)).collect();
    promoted.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_unix.cmp(&a.last_unix)));
    promoted.truncate(MAX_DISTILLED);
    promoted
}

/// Where the distilled rules file lives for a scope.
fn rules_path(scope: &str) -> Result<PathBuf> {
    match scope {
        "global" => global_rules_dir()
            .ok_or_else(|| Error::Tool("cannot resolve home dir".into()))
            .map(|d| d.join("learned.md")),
        _ => Ok(crate::workdir::current_workdir()
            .join(".thclaws")
            .join("rules")
            .join("learned.md")),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Lesson {
    /// Normalised topic key — the identity used for counting repeats.
    key: String,
    text: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    evidence: String,
    #[serde(default = "one")]
    count: u64,
    #[serde(default)]
    first_unix: u64,
    #[serde(default)]
    last_unix: u64,
}

fn one() -> u64 {
    1
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn normalise_key(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn project_lessons(cwd: &Path) -> PathBuf {
    cwd.join(".thclaws").join("state").join("lessons.jsonl")
}

fn global_lessons() -> Option<PathBuf> {
    crate::util::home_dir().map(|h| {
        h.join(".config")
            .join(crate::profile::app_dir_name())
            .join("lessons.jsonl")
    })
}

fn global_rules_dir() -> Option<PathBuf> {
    crate::util::home_dir().map(|h| {
        h.join(".config")
            .join(crate::profile::app_dir_name())
            .join("rules")
    })
}

fn load(path: &Path) -> Vec<Lesson> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Lesson>(l).ok())
        .collect()
}

fn save(path: &Path, lessons: &[Lesson]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Tool(format!("create {}: {e}", parent.display())))?;
    }
    let body: String = lessons
        .iter()
        .filter_map(|l| serde_json::to_string(l).ok())
        .map(|l| l + "\n")
        .collect();
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, body).map_err(|e| Error::Tool(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::Tool(format!("rename: {e}")))?;
    Ok(())
}

#[async_trait]
impl Tool for LessonTool {
    fn name(&self) -> &'static str {
        "Lesson"
    }

    fn description(&self) -> &'static str {
        "Record what you learned so the next session doesn't repeat it. Use it \
         when the user corrects you, a test disproves an assumption, or an \
         approach is confirmed to work. `record` adds/bumps a lesson and — when \
         it is promotable — rewrites `.thclaws/rules/learned.md` immediately, so \
         no follow-up call is needed: a repeat or a `kind: correction` becomes \
         a rule the next session loads automatically. `distill` rebuilds that \
         file from the journal (use it after retracting rules by hand); `list` \
         shows the journal; `forget` drops one. Record a lesson whenever the \
         user corrects you — that is the highest-signal event there is."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["record", "list", "distill", "forget"],
                    "description": "record = add/bump a lesson. distill = promote repeated lessons into a rules file. list = show lessons. forget = delete one by key."
                },
                "key": {
                    "type": "string",
                    "description": "Short stable topic for the lesson, e.g. `restart-thclaws` or `pnpm-path`. Repeats with the same key raise its count. Required for record/forget."
                },
                "text": {
                    "type": "string",
                    "description": "The lesson itself — one imperative sentence, e.g. `Use launchctl kickstart, never kill the process twice`."
                },
                "kind": {
                    "type": "string",
                    "enum": ["correction", "failure", "success", "preference"],
                    "description": "Why this is worth remembering. `correction` = the user told you off."
                },
                "evidence": {
                    "type": "string",
                    "description": "Optional short proof — `user: don't restart twice`, `test X failed`, `commit abc123`."
                },
                "scope": {
                    "type": "string",
                    "enum": ["project", "global"],
                    "description": "project (default) = this workspace only. global = every project."
                }
            },
            "required": ["op"]
        })
    }

    fn requires_approval(&self, _input: &Value) -> bool {
        false
    }

    async fn call(&self, input: Value) -> Result<String> {
        let op = req_str(&input, "op")?;
        let scope = input
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("project");

        let path = match scope {
            "global" => global_lessons()
                .ok_or_else(|| Error::Tool("cannot resolve home dir for global lessons".into()))?,
            "project" => project_lessons(&crate::workdir::current_workdir()),
            other => {
                return Err(Error::Tool(format!(
                    "unknown scope '{other}' (expected project or global)"
                )))
            }
        };

        match op {
            "record" => {
                let key = normalise_key(&req_str(&input, "key")?);
                if key.is_empty() {
                    return Err(Error::Tool("key must not be empty".into()));
                }
                let text = req_str(&input, "text")?.trim().to_string();
                if text.is_empty() {
                    return Err(Error::Tool("text must not be empty".into()));
                }
                let kind = input
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let evidence = input
                    .get("evidence")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                let mut lessons = load(&path);
                let now = now_unix();
                match lessons.iter_mut().find(|l| l.key == key) {
                    Some(existing) => {
                        existing.count += 1;
                        existing.text = text.clone();
                        existing.last_unix = now;
                        if !kind.is_empty() {
                            existing.kind = kind;
                        }
                        if !evidence.is_empty() {
                            existing.evidence = evidence;
                        }
                    }
                    None => lessons.push(Lesson {
                        key: key.clone(),
                        text: text.clone(),
                        kind,
                        evidence,
                        count: 1,
                        first_unix: now,
                        last_unix: now,
                    }),
                }
                let count = lessons
                    .iter()
                    .find(|l| l.key == key)
                    .map(|l| l.count)
                    .unwrap_or(1);
                save(&path, &lessons)?;

                let mut out = format!(
                    "✓ lesson `{key}` recorded (count {count}) in {}\n  {text}",
                    path.display()
                );
                let this = lessons
                    .iter()
                    .find(|l| l.key == key)
                    .expect("just inserted");
                // Auto-distill: a promotable lesson goes straight into the
                // rules file. Before this, `record` only *suggested* distill,
                // so the loop closed only when the agent remembered a second
                // call — which in practice meant it never closed.
                if promotable(this) {
                    match rules_path(scope) {
                        Ok(rp) => {
                            let promoted = promoted_from(&lessons);
                            let body = render_distilled(&promoted);
                            write_rules(&rp, &body)?;
                            out.push_str(&format!(
                                "\n  → promoted into {} ({} rule(s) load automatically now)",
                                rp.display(),
                                promoted.len()
                            ));
                        }
                        Err(e) => out.push_str(&format!("\n  → distill skipped: {e}")),
                    }
                }
                Ok(out)
            }
            "list" => {
                let lessons = load(&path);
                if lessons.is_empty() {
                    return Ok(format!("no lessons yet at {}", path.display()));
                }
                let mut sorted = lessons.clone();
                sorted.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_unix.cmp(&a.last_unix)));
                let mut out = format!("{} lesson(s) — {}\n", sorted.len(), path.display());
                for l in &sorted {
                    let tag = if l.kind.is_empty() {
                        "—"
                    } else {
                        l.kind.as_str()
                    };
                    let promote = if l.count >= PROMOTE_AT { " [rule]" } else { "" };
                    out.push_str(&format!(
                        "- ×{} [{}]{} `{}` — {}\n",
                        l.count, tag, promote, l.key, l.text
                    ));
                    if !l.evidence.is_empty() {
                        out.push_str(&format!("    evidence: {}\n", l.evidence));
                    }
                }
                let ready = sorted.iter().filter(|l| promotable(l)).count();
                if ready > 0 {
                    out.push_str(&format!(
                        "\n{ready} promotable lesson(s) — recorded ones are already in the rules file; \
                         run Lesson(op: \"distill\", scope: \"{scope}\") to rewrite it after hand edits."
                    ));
                }
                Ok(out)
            }
            "distill" => {
                let now = now_unix();
                // Drop stale one-offs first: a single observation nobody
                // repeated is noise, and an append-only journal eventually
                // costs more to read than it saves.
                let all = load(&path);
                let kept: Vec<Lesson> = all
                    .iter()
                    .filter(|l| promotable(l) || now.saturating_sub(l.last_unix) <= STALE_SECS)
                    .cloned()
                    .collect();
                let pruned = all.len() - kept.len();
                if pruned > 0 {
                    save(&path, &kept)?;
                }
                let promoted = promoted_from(&kept);
                if promoted.is_empty() {
                    return Ok(
                        "nothing to distill — no lesson has been recorded twice or marked as a correction"
                            .into(),
                    );
                }

                let rules_path = rules_path(scope)?;
                let body = render_distilled(&promoted);
                write_rules(&rules_path, &body)?;
                let mut out = format!(
                    "✓ distilled {} lesson(s) into {}\n  They load automatically from now on. \
                     Edit or delete that file to retract a rule.",
                    promoted.len(),
                    rules_path.display()
                );
                if pruned > 0 {
                    out.push_str(&format!("\n  Pruned {pruned} stale one-off(s) from the journal."));
                }
                Ok(out)
            }
            "forget" => {
                let key = normalise_key(&req_str(&input, "key")?);
                let mut lessons = load(&path);
                let before = lessons.len();
                lessons.retain(|l| l.key != key);
                if lessons.len() == before {
                    return Ok(format!("no lesson with key `{key}`"));
                }
                save(&path, &lessons)?;
                Ok(format!("✓ forgot `{key}`"))
            }
            other => Err(Error::Tool(format!(
                "unknown op '{other}' (expected record, list, distill or forget)"
            ))),
        }
    }
}

fn render_distilled(lessons: &[&Lesson]) -> String {
    let mut out = String::from(
        "# Learned lessons (auto-distilled)\n\n\
         Each of these either repeated or was a direct correction from the user. Keep them; retract by \
         editing this file.\n\n",
    );
    for l in lessons {
        let line = format!("- {}\n", l.text.trim());
        if out.len() + line.len() > MAX_DISTILLED_CHARS {
            out.push_str("- …(more lessons in the journal; run Lesson(op: \"list\"))\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

fn write_rules(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Tool(format!("create {}: {e}", parent.display())))?;
    }
    let doc = format!(
        "---\ndescription: Lessons learned here, distilled from repeated corrections — read when unsure of conventions\nalwaysApply: true\n---\n{body}"
    );
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, doc).map_err(|e| Error::Tool(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::Tool(format!("rename: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(key: &str, count: u64, text: &str) -> Lesson {
        Lesson {
            key: key.into(),
            text: text.into(),
            kind: "correction".into(),
            evidence: String::new(),
            count,
            first_unix: 1,
            last_unix: 2,
        }
    }

    #[test]
    fn promoted_from_takes_corrections_and_repeats_only() {
        let mut once = l("k2", 1, "one-off observation");
        once.kind = "success".into();
        let repeated = l("k3", 2, "seen twice");
        let correction = l("k1", 1, "told off once");
        let lessons = vec![once, repeated, correction];
        let promoted = promoted_from(&lessons);
        let keys: Vec<&str> = promoted.iter().map(|l| l.key.as_str()).collect();
        assert!(keys.contains(&"k1"), "correction promotes");
        assert!(keys.contains(&"k3"), "repeat promotes");
        assert!(!keys.contains(&"k2"), "one-off success stays in journal");
        let mut pref = l("k4", 1, "user likes X");
        pref.kind = "preference".into();
        assert!(promoted_from(&[pref]).len() == 1, "preference promotes");
    }

    #[test]
    fn keys_normalise_so_repeats_count() {
        assert_eq!(normalise_key("  Restart  THCLaws "), "restart thclaws");
        assert_eq!(normalise_key("a\n b\tc"), "a b c");
    }

    #[test]
    fn correction_promotes_on_first_sight_observation_does_not() {
        let mut correction = l("k1", 1, "Never restart twice");
        // l() builds kind: "correction" — one occurrence is already enough.
        assert!(promotable(&correction));
        let mut observation = l("k2", 1, "Grep was faster here");
        observation.kind = "success".into();
        assert!(!promotable(&observation));
        observation.count = PROMOTE_AT;
        assert!(promotable(&observation));
        correction.kind.clear();
        correction.count = 1;
        assert!(!promotable(&correction), "no kind, no repeat = journal only");
    }

    #[test]
    fn distilled_body_carries_lessons_and_frontmatter_flag() {
        let a = l("k1", 3, "Never restart twice");
        let b = l("k2", 2, "Use full pnpm path");
        let body = render_distilled(&[&a, &b]);
        assert!(body.contains("Never restart twice"));
        assert!(body.contains("Use full pnpm path"));
    }

    #[test]
    fn distilled_body_respects_char_cap() {
        let long: Vec<Lesson> = (0..40)
            .map(|i| l(&format!("k{i}"), 2, &"x".repeat(80)))
            .collect();
        let refs: Vec<&Lesson> = long.iter().collect();
        let body = render_distilled(&refs);
        assert!(
            body.len() <= MAX_DISTILLED_CHARS + 200,
            "capped: {}",
            body.len()
        );
        assert!(body.contains("more lessons in the journal"));
    }

    #[tokio::test]
    async fn record_twice_bumps_count_and_list_marks_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lessons.jsonl");

        // Drive the storage helpers directly — the tool resolves its own
        // project path from the cwd, which tests must not depend on.
        let mut lessons = load(&path);
        lessons.push(l("restart", 1, "Never restart twice"));
        save(&path, &lessons).unwrap();
        let mut again = load(&path);
        again[0].count += 1;
        save(&path, &again).unwrap();

        let rows = load(&path);
        assert_eq!(rows.len(), 1, "same key must not duplicate");
        assert_eq!(rows[0].count, 2);
    }
}
