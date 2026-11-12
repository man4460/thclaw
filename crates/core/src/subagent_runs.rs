//! Handle-based ("background") subagent runs.
//!
//! `Task` used to be a blocking tool call: the parent's turn froze until
//! the child finished, the parent saw nothing in between, and could not
//! correct course. This module is the other half of that contract — the
//! child runs on its own tokio task and reports into a [`SubagentRun`]
//! the parent can read, wait on, steer and abort:
//!
//! | tool | effect |
//! |---|---|
//! | `TaskStatus` | current state + activity tail (all runs, or one id) |
//! | `TaskWait` | block until the run produces new activity or finishes |
//! | `TaskSay` | queue a message the child reads at its next tool boundary |
//! | `TaskCancel` | fire the run's cancel token |
//!
//! Runs live in a process-global registry (one engine process per
//! workspace, same model as `side_channel`). Finished runs stay readable
//! for [`FINISHED_TTL`] so a parent that was busy can still collect the
//! result instead of having it vanish.

use crate::tools::{req_str, Tool};
use crate::cancel::CancelToken;
use crate::error::{Error, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const STATUS_TOOL: &str = "TaskStatus";
pub const WAIT_TOOL: &str = "TaskWait";
pub const SAY_TOOL: &str = "TaskSay";
pub const CANCEL_TOOL: &str = "TaskCancel";

/// A queue of messages the child agent drains at every tool boundary.
pub type InjectionQueue = Arc<Mutex<VecDeque<String>>>;

/// Activity ring buffer per run — enough to reconstruct what the child did
/// without keeping the whole transcript in memory.
const MAX_ACTIVITY: usize = 200;
/// Finished runs stay in the registry this long, so a parent that was busy
/// elsewhere can still read the result.
const FINISHED_TTL: Duration = Duration::from_secs(30 * 60);
/// Characters of the child's final answer carried in a digest.
const DIGEST_CHARS: usize = 1_500;

/// Terminal (or running) state of a background run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Running,
    Done {
        text: String,
        stop_reason: Option<String>,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

impl RunState {
    pub fn is_finished(&self) -> bool {
        !matches!(self, RunState::Running)
    }
}

#[derive(Debug, Clone)]
pub struct ActivityLine {
    pub at: Instant,
    /// `tool` | `ok` | `err` | `round` | `steer` | `note`
    pub kind: &'static str,
    pub text: String,
}

pub struct SubagentRun {
    pub id: String,
    /// Agent def name, or `Task` for the default child.
    pub agent: String,
    /// The short label the parent passed as `description`.
    pub label: String,
    pub started: Instant,
    activity: Mutex<Vec<ActivityLine>>,
    state: Mutex<RunState>,
    finished_at: Mutex<Option<Instant>>,
    pub injections: InjectionQueue,
    pub cancel: CancelToken,
}

impl SubagentRun {
    fn new(id: String, agent: String, label: String) -> Self {
        Self {
            id,
            agent,
            label,
            started: Instant::now(),
            activity: Mutex::new(Vec::new()),
            state: Mutex::new(RunState::Running),
            finished_at: Mutex::new(None),
            injections: Arc::new(Mutex::new(VecDeque::new())),
            cancel: CancelToken::new(),
        }
    }

    pub fn push(&self, kind: &'static str, text: impl Into<String>) {
        let mut a = self.activity.lock().unwrap_or_else(|e| e.into_inner());
        a.push(ActivityLine {
            at: Instant::now(),
            kind,
            text: text.into(),
        });
        if a.len() > MAX_ACTIVITY {
            let drop = a.len() - MAX_ACTIVITY;
            a.drain(0..drop);
        }
    }

    /// Number of activity lines recorded so far — a cursor for `since`.
    pub fn cursor(&self) -> usize {
        self.activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Lines recorded after `since` (a cursor from [`Self::cursor`]).
    /// Returns the lines and the new cursor.
    pub fn lines_since(&self, since: usize) -> (Vec<ActivityLine>, usize) {
        let a = self.activity.lock().unwrap_or_else(|e| e.into_inner());
        // Cursors survive trimming: past MAX_ACTIVITY the oldest lines are
        // gone, so clamp to the retained window instead of slicing blind.
        let len = a.len();
        let start = since.min(len);
        (a[start..].to_vec(), len)
    }

    pub fn state(&self) -> RunState {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn finish(&self, state: RunState) {
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.is_finished() {
                return;
            }
            *s = state;
        }
        *self.finished_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    pub fn is_finished(&self) -> bool {
        self.state().is_finished()
    }

    pub fn elapsed(&self) -> Duration {
        self.finished_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(self.started)
    }

    pub fn pending_injections(&self) -> usize {
        self.injections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// One-screen summary: state, elapsed, last activity lines, and the
    /// child's answer when it finished.
    pub fn render(&self, since: usize) -> String {
        let (lines, cursor) = self.lines_since(since);
        let mut out = String::new();
        out.push_str(&format!(
            "[{}] {} ({}) — {} · {:.1}s\n",
            self.id,
            self.label,
            self.agent,
            match self.state() {
                RunState::Running => "running".to_string(),
                RunState::Cancelled => "cancelled".to_string(),
                RunState::Failed { .. } => "failed".to_string(),
                RunState::Done { .. } => "done".to_string(),
            },
            self.elapsed().as_secs_f64()
        ));
        if !lines.is_empty() {
            out.push_str(&format!("activity (+{} since cursor {}):\n", lines.len(), since));
            for l in &lines {
                out.push_str(&format!("  [{}] {}\n", l.kind, l.text));
            }
        }
        match self.state() {
            RunState::Done { text, stop_reason } => {
                out.push_str(&format!(
                    "result{}:\n{}\n",
                    stop_reason
                        .map(|s| format!(" (stop: {s})"))
                        .unwrap_or_default(),
                    clip(&text, DIGEST_CHARS)
                ));
            }
            RunState::Failed { error } => out.push_str(&format!("error: {error}\n")),
            RunState::Cancelled => out.push_str("cancelled by parent\n"),
            RunState::Running => {
                let pending = self.pending_injections();
                if pending > 0 {
                    out.push_str(&format!("{pending} message(s) queued for the child\n"));
                }
            }
        }
        out.push_str(&format!("cursor: {cursor}"));
        out
    }

    /// Digest pushed into the parent's injection queue when the run ends,
    /// so the parent learns about it without polling.
    pub fn completion_digest(&self) -> String {
        let mut out = format!(
            "[{} done] {} ({}) after {:.1}s\n",
            self.id,
            self.label,
            self.agent,
            self.elapsed().as_secs_f64()
        );
        match self.state() {
            RunState::Done { text, .. } => out.push_str(&clip(&text, DIGEST_CHARS)),
            RunState::Failed { error } => out.push_str(&format!("FAILED: {error}")),
            RunState::Cancelled => out.push_str("cancelled"),
            RunState::Running => out.push_str("(still running)"),
        }
        out
    }
}

fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max).collect();
    out.push_str("…");
    out
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

type Registry = Arc<Mutex<HashMap<String, Arc<SubagentRun>>>>;

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Register a new run and hand back its handle. Ids are short and stable
/// (`sub-1a2b3c4d`) so the parent can quote one in a later message.
pub fn start_run(agent: impl Into<String>, label: impl Into<String>) -> Arc<SubagentRun> {
    let id = format!(
        "sub-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("anon")
    );
    let run = Arc::new(SubagentRun::new(id.clone(), agent.into(), label.into()));
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    prune(&mut reg);
    reg.insert(id, run.clone());
    run
}

fn prune(reg: &mut HashMap<String, Arc<SubagentRun>>) {
    let now = Instant::now();
    reg.retain(|_, r| {
        let finished = r
            .finished_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .to_owned();
        match finished {
            Some(at) => now.saturating_duration_since(at) < FINISHED_TTL,
            None => true,
        }
    });
}

pub fn get(id: &str) -> Option<Arc<SubagentRun>> {
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    prune(&mut reg);
    reg.get(id).cloned()
}

pub fn list() -> Vec<Arc<SubagentRun>> {
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    prune(&mut reg);
    let mut runs: Vec<Arc<SubagentRun>> = reg.values().cloned().collect();
    runs.sort_by_key(|r| r.started);
    runs
}

fn require(id: &str) -> Result<Arc<SubagentRun>> {
    get(id).ok_or_else(|| {
        let known: Vec<String> = list().into_iter().map(|r| r.id.clone()).collect();
        Error::Tool(format!(
            "no such run '{id}' (known: {})",
            if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            }
        ))
    })
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// `TaskStatus` — read-only snapshot. Without `id`: every run, one line
/// each. With `id`: state, activity tail and the result if it finished.
pub struct TaskStatusTool;

#[async_trait]
impl Tool for TaskStatusTool {
    fn name(&self) -> &'static str {
        STATUS_TOOL
    }

    fn description(&self) -> &'static str {
        "Inspect running background subagents (spawned by `Task`). With no \
         `id`, lists every known run with its state and last activity. With \
         `id`, returns that run's state, recent activity and — once it has \
         finished — its final answer. `since` is a cursor from a previous \
         call: only activity after it is shown."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Run id from Task (e.g. sub-1a2b3c4d). Omit to list all runs." },
                "since": { "type": "integer", "description": "Only show activity recorded after this cursor (from a previous TaskStatus/TaskWait result)." }
            }
        })
    }

    fn parallelizable(&self) -> bool {
        true
    }

    async fn call(&self, input: Value) -> Result<String> {
        let since = input.get("since").and_then(Value::as_u64).unwrap_or(0) as usize;
        match input.get("id").and_then(Value::as_str) {
            Some(id) => Ok(require(id)?.render(since)),
            None => {
                let runs = list();
                if runs.is_empty() {
                    return Ok("no subagent runs in this session".into());
                }
                let mut out = String::new();
                for r in runs {
                    let state = match r.state() {
                        RunState::Running => "running".to_string(),
                        RunState::Done { .. } => "done".to_string(),
                        RunState::Failed { error } => format!("failed: {}", clip(&error, 120)),
                        RunState::Cancelled => "cancelled".to_string(),
                    };
                    let last = r
                        .lines_since(0)
                        .0
                        .last()
                        .map(|l| clip(&l.text, 80))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "{} — {} ({}) {} {:.0}s{} — cursor {}\n",
                        r.id,
                        r.label,
                        r.agent,
                        state,
                        r.elapsed().as_secs_f64(),
                        if last.is_empty() {
                            String::new()
                        } else {
                            format!(" · {last}")
                        },
                        r.cursor()
                    ));
                }
                Ok(out)
            }
        }
    }
}

/// `TaskWait` — block until the run produces something new or finishes.
/// This is the "stay in the conversation" primitive: the parent parks here
/// instead of burning turns polling.
pub struct TaskWaitTool;

#[async_trait]
impl Tool for TaskWaitTool {
    fn name(&self) -> &'static str {
        WAIT_TOOL
    }

    fn description(&self) -> &'static str {
        "Wait for a background subagent to make progress. Returns as soon as \
         it records new activity or finishes; on no change within \
         `timeout_ms` it returns the current state so you can decide whether \
         to keep waiting. Pass `since` (the cursor from the previous result) \
         to get only what happened since then."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Run id from Task." },
                "since": { "type": "integer", "description": "Cursor from the previous TaskStatus/TaskWait result." },
                "timeout_ms": { "type": "integer", "description": "Max wait in milliseconds (default 60000, cap 300000)." }
            },
            "required": ["id"]
        })
    }

    fn parallelizable(&self) -> bool {
        true
    }

    async fn call(&self, input: Value) -> Result<String> {
        let id = req_str(&input, "id")?.to_string();
        let since = input.get("since").and_then(Value::as_u64).unwrap_or(0) as usize;
        let timeout = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(60_000)
            .min(300_000);
        let run = require(&id)?;
        let deadline = Instant::now() + Duration::from_millis(timeout);
        loop {
            if run.cursor() > since || run.is_finished() {
                return Ok(run.render(since));
            }
            if Instant::now() >= deadline {
                return Ok(format!(
                    "{}\n(no change within {}ms)",
                    run.render(since),
                    timeout
                ));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// `TaskSay` — queue a message for the child. Folded into its context at
/// the next tool boundary (same mechanism as a user message typed while a
/// turn is running).
pub struct TaskSayTool;

#[async_trait]
impl Tool for TaskSayTool {
    fn name(&self) -> &'static str {
        SAY_TOOL
    }

    fn description(&self) -> &'static str {
        "Send a message to a running background subagent. The child reads it \
         at its next tool boundary (after the call it is currently running \
         returns), so it can change course mid-task. Use it to narrow scope, \
         correct a wrong assumption, or tell it to wrap up. The message is \
         prefixed with `[from parent]` so the child knows it is steering, \
         not a fresh user turn."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Run id from Task." },
                "text": { "type": "string", "description": "The message for the child." }
            },
            "required": ["id", "text"]
        })
    }

    async fn call(&self, input: Value) -> Result<String> {
        let id = req_str(&input, "id")?.to_string();
        let text = req_str(&input, "text")?.to_string();
        let run = require(&id)?;
        if run.is_finished() {
            return Err(Error::Tool(format!(
                "{id} has already finished — read its result with TaskStatus instead"
            )));
        }
        {
            let mut q = run
                .injections
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            q.push_back(format!("[from parent] {text}"));
        }
        run.push("steer", format!("queued: {}", clip(&text, 120)));
        Ok(format!(
            "queued for {id} ({} message(s) pending; delivered at its next tool boundary)",
            run.pending_injections()
        ))
    }
}

/// `TaskCancel` — fire the run's cancel token.
pub struct TaskCancelTool;

#[async_trait]
impl Tool for TaskCancelTool {
    fn name(&self) -> &'static str {
        CANCEL_TOOL
    }

    fn description(&self) -> &'static str {
        "Cancel a background subagent. Its current turn stops at the next \
         event boundary and it is marked cancelled; the run stays readable \
         via TaskStatus for 30 minutes."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Run id from Task." }
            },
            "required": ["id"]
        })
    }

    async fn call(&self, input: Value) -> Result<String> {
        let id = req_str(&input, "id")?.to_string();
        let run = require(&id)?;
        if run.is_finished() {
            return Ok(format!("{id} already finished ({:?})", run.state()));
        }
        run.cancel.cancel();
        run.push("note", "cancel requested by parent");
        Ok(format!("cancel sent to {id}"))
    }
}

/// All four control tools, ready to register.
pub fn control_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TaskStatusTool),
        Arc::new(TaskWaitTool),
        Arc::new(TaskSayTool),
        Arc::new(TaskCancelTool),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Arc<SubagentRun> {
        start_run("coder-local", "fix the parser")
    }

    #[test]
    fn activity_is_cursored() {
        let r = run();
        r.push("tool", "Read: src/lib.rs");
        let cursor = r.cursor();
        r.push("ok", "Read done");
        let (lines, next) = r.lines_since(cursor);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Read done");
        assert_eq!(next, 2);
        // A stale cursor past the end must not panic.
        assert!(r.lines_since(99).0.is_empty());
    }

    #[test]
    fn activity_is_bounded() {
        let r = run();
        for i in 0..(MAX_ACTIVITY + 25) {
            r.push("tool", format!("call {i}"));
        }
        assert_eq!(r.cursor(), MAX_ACTIVITY);
        let (lines, _) = r.lines_since(0);
        assert_eq!(lines.first().unwrap().text, "call 25");
    }

    #[test]
    fn finish_is_idempotent_and_reports_result() {
        let r = run();
        r.finish(RunState::Done {
            text: "wrote tmp/x.txt".into(),
            stop_reason: None,
        });
        r.finish(RunState::Failed {
            error: "late".into(),
        });
        let out = r.render(0);
        assert!(out.contains("done"), "{out}");
        assert!(out.contains("wrote tmp/x.txt"), "{out}");
        assert!(!out.contains("late"), "{out}");
        assert!(r.is_finished());
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_finished() {
        let r = run();
        r.finish(RunState::Done {
            text: "ok".into(),
            stop_reason: None,
        });
        let out = TaskWaitTool
            .call(json!({ "id": r.id, "timeout_ms": 5000 }))
            .await
            .unwrap();
        assert!(out.contains("result"), "{out}");
    }

    #[tokio::test]
    async fn wait_returns_new_activity_without_timeout() {
        let r = run();
        let id = r.id.clone();
        let handle = tokio::spawn(async move {
            TaskWaitTool
                .call(json!({ "id": id, "since": 0, "timeout_ms": 5000 }))
                .await
                .unwrap()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        r.push("tool", "Bash: cargo test");
        let out = handle.await.unwrap();
        assert!(out.contains("cargo test"), "{out}");
    }

    #[tokio::test]
    async fn say_queues_prefixed_message() {
        let r = run();
        TaskSayTool
            .call(json!({ "id": r.id, "text": "only touch parser.rs" }))
            .await
            .unwrap();
        let q = r.injections.lock().unwrap();
        assert_eq!(q.len(), 1);
        assert!(q[0].starts_with("[from parent]"));
    }

    #[tokio::test]
    async fn say_and_cancel_refuse_finished_runs() {
        let r = run();
        r.finish(RunState::Done {
            text: "x".into(),
            stop_reason: None,
        });
        assert!(TaskSayTool
            .call(json!({ "id": r.id, "text": "too late" }))
            .await
            .is_err());
        let out = TaskCancelTool
            .call(json!({ "id": r.id }))
            .await
            .unwrap();
        assert!(out.contains("already finished"), "{out}");
    }

    #[tokio::test]
    async fn unknown_id_names_the_known_runs() {
        let r = run();
        let err = TaskStatusTool
            .call(json!({ "id": "sub-nope" }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(&r.id), "{err}");
    }

    #[test]
    fn digest_carries_the_answer() {
        let r = run();
        r.finish(RunState::Done {
            text: "all green".into(),
            stop_reason: Some("end_turn".into()),
        });
        let d = r.completion_digest();
        assert!(d.starts_with("[sub-"), "{d}");
        assert!(d.contains("all green"), "{d}");
    }
}
