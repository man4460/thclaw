//! dev-plan/42: per-session working-directory chokepoint.
//!
//! In multiuser `--serve` (one pod, a `workspace-<id>/` per user) many
//! sessions share one process, so the agent's working directory must
//! NOT come from the process-global `std::env::current_dir()` — that's a
//! single mutable global racing across tenants (a `set_current_dir` on
//! one user's turn would relocate another user's in-flight path
//! resolution; see dev-plan/42 §Security). Each worker establishes a
//! task-local working root around its agent run, and every path-resolving
//! tool reads it through [`current_workdir`].
//!
//! A `tokio::task_local!` (not a `thread_local!`) is required because
//! each worker runs its own *multi-threaded* runtime, so a tool future
//! can resume on a different runtime thread after an `.await`; a
//! task-local follows the task across those hops, a thread-local would
//! not.
//!
//! Single-tenant `--serve`, desktop, and CLI never enter the scope, so
//! [`current_workdir`] falls back to `std::env::current_dir()` —
//! behaviour unchanged. In multiuser the worker always establishes the
//! scope, so process cwd is never consulted (fail-closed).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

tokio::task_local! {
    static WORKDIR: PathBuf;
}

/// Process-wide "this is a `--serve --multiuser` pod" flag. Set once at
/// serve start. Unlike [`workdir_is_scoped`] (true only *inside* a
/// worker's per-turn task-local scope), this is readable everywhere —
/// including IPC handlers that run outside any session scope — so global
/// mutators (`set_current_dir`, `Sandbox::init`) can refuse to fire in a
/// shared multi-tenant process where they'd clobber every tenant.
static MULTIUSER: AtomicBool = AtomicBool::new(false);

/// Mark the process as a multiuser serve pod. Called once from
/// `server::run` when multi-tenant mode is configured.
pub fn set_multiuser(on: bool) {
    MULTIUSER.store(on, Ordering::Relaxed);
}

/// True in a `--serve --multiuser` process. Global cwd/sandbox mutators
/// guard on this to stay no-ops (per-session roots come from the
/// task-local scope instead).
pub fn is_multiuser() -> bool {
    MULTIUSER.load(Ordering::Relaxed)
}

/// The active session's working directory: the task-local root when a
/// worker has scoped one (multiuser), else the process cwd (single-
/// tenant / desktop / CLI). This is the single site path-resolving tools
/// consult instead of `std::env::current_dir()`.
pub fn current_workdir() -> PathBuf {
    WORKDIR
        .try_with(|p| p.clone())
        .unwrap_or_else(|_| workspace_root())
}

/// dev-plan/61: the folder a user's files live in. An agent under a workspace
/// host runs with its own folder as the process cwd, so its settings,
/// sessions and memory stay its own, but its files are the workspace's: the
/// host passes `THCLAWS_WORKSPACE_ROOT`, and every agent reads and writes
/// there. Without the variable this is the process cwd, as before.
///
/// A workspace the user switched to from the GUI (`workspace_override`) wins
/// over the host's default: files, bash cwd and the sandbox follow the user's
/// folder while the agent's own folder — its cwd — stays where it is.
pub fn workspace_root() -> PathBuf {
    if let Some(dir) = workspace_override() {
        return dir;
    }
    match std::env::var("THCLAWS_WORKSPACE_ROOT") {
        Ok(s) if !s.trim().is_empty() => PathBuf::from(s),
        _ => std::env::current_dir().unwrap_or_default(),
    }
}

/// The folder the user picked as their workspace, or `None` for the default.
///
/// Persisted at the agent's own `<cwd>/.thclaws/state/workspace_root` so a
/// restart lands back in the folder they left off in; read once, lazily, since
/// [`workspace_root`] sits on every tool path.
static OVERRIDE: std::sync::RwLock<(bool, Option<PathBuf>)> =
    std::sync::RwLock::new((false, None));

fn override_file() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".thclaws").join("state").join("workspace_root"))
}

/// The persisted switch, or `None` when the workspace is the default one.
pub fn workspace_override() -> Option<PathBuf> {
    if let Ok(g) = OVERRIDE.read() {
        if g.0 {
            return g.1.clone();
        }
    }
    let loaded = override_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_dir());
    if let Ok(mut w) = OVERRIDE.write() {
        w.0 = true;
        w.1 = loaded.clone();
    }
    loaded
}

/// Point the workspace at `dir`, or clear the switch with `None`. Persisted, so
/// the choice survives a restart. Callers re-init the sandbox afterwards.
pub fn set_workspace_override(dir: Option<PathBuf>) -> std::io::Result<()> {
    if let Some(file) = override_file() {
        match &dir {
            Some(d) => {
                if let Some(parent) = file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&file, format!("{}\n", d.display()))?;
            }
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
    if let Ok(mut w) = OVERRIDE.write() {
        w.0 = true;
        w.1 = dir;
    }
    Ok(())
}

/// finding 11: the agent's OWN folder under a workspace host.
///
/// A bot runs with its folder as the process cwd while
/// `THCLAWS_WORKSPACE_ROOT` points at the user's files, so the two differ
/// exactly when there is a host. Without one they are the same directory and
/// this is `None`, which keeps every rule below a no-op off a host.
pub fn agent_dir() -> Option<PathBuf> {
    let root = match std::env::var("THCLAWS_WORKSPACE_ROOT") {
        Ok(s) if !s.trim().is_empty() => PathBuf::from(s),
        _ => return None,
    };
    let cwd = std::env::current_dir().ok()?;
    (cwd != root).then_some(cwd)
}

/// Subpaths of `.thclaws/` that belong to the WORKSPACE rather than to the
/// agent: the project KMS and the engine's log, which every agent shares and
/// which `kms.rs` and `util.rs` already resolve at the workspace root.
const SHARED_THCLAWS: &[&str] = &["state/kms", "state/logs"];

/// Where a relative path the agent typed into a tool resolves from.
///
/// `.thclaws/…` is the agent's own — holding its settings, sessions, skills
/// and whatever it ships — and `AGENT_ENTRIES` in the migration says as much.
/// Everything else is the user's and resolves at the workspace root, as
/// before. Two subpaths are carved out because they are shared, not agent
/// state (see [`SHARED_THCLAWS`]).
///
/// Bash cannot follow this rule: its cwd is one directory for the whole
/// command, so a bare `.thclaws/…` inside a shell string cannot mean the
/// agent's folder for one process and the host's for another. Agents reach
/// their own folder from a shell through `THCLAWS_AGENT_DIR` instead.
pub fn tool_base(path: &str) -> PathBuf {
    match agent_dir() {
        Some(agent) if is_agent_owned(path) => agent,
        _ => current_workdir(),
    }
}

/// True for a relative `.thclaws/…` that belongs to the agent.
fn is_agent_owned(path: &str) -> bool {
    let p = path.strip_prefix("./").unwrap_or(path);
    if std::path::Path::new(p).is_absolute() {
        return false;
    }
    let Some(rest) = p
        .strip_prefix(".thclaws/")
        .or_else(|| (p == ".thclaws").then_some(""))
    else {
        return false;
    };
    !SHARED_THCLAWS
        .iter()
        .any(|s| rest == *s || rest.strip_prefix(s).is_some_and(|t| t.starts_with('/')))
}

/// True when a per-session working root is active (i.e. we're inside a
/// multiuser worker scope). Lets callers fail-closed instead of touching
/// process cwd when isolation is expected.
pub fn workdir_is_scoped() -> bool {
    WORKDIR.try_with(|_| ()).is_ok()
}

/// Run `fut` with `root` as the task-local working directory. The
/// multiuser worker wraps each agent turn in this so every awaited tool
/// call resolves against the user's `workspace-<id>/`.
pub async fn scope_workdir<F, T>(root: PathBuf, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    WORKDIR.scope(root, fut).await
}

#[cfg(test)]
mod tests {

    /// A workspace the user switched to from the GUI wins over the host's
    /// default, and is persisted in the agent's OWN folder — so the agent's
    /// state stays put while the files follow the pick, and a restart lands
    /// back in it.
    #[test]
    fn a_switched_workspace_wins_and_persists() {
        let _g = crate::kms::test_env_lock();
        let prev_env = std::env::var("THCLAWS_WORKSPACE_ROOT").ok();
        let prev_cwd = std::env::current_dir().unwrap();
        let host_root = tempfile::tempdir().unwrap();
        let agent = tempfile::tempdir().unwrap();
        let picked = tempfile::tempdir().unwrap();

        std::env::set_var("THCLAWS_WORKSPACE_ROOT", host_root.path());
        std::env::set_current_dir(agent.path()).unwrap();
        assert_eq!(workspace_root(), host_root.path(), "host default first");

        set_workspace_override(Some(picked.path().to_path_buf())).unwrap();
        assert_eq!(workspace_root(), picked.path());
        assert_eq!(current_workdir(), picked.path(), "tools follow the switch");
        assert_eq!(
            agent_dir(),
            std::env::current_dir().ok(),
            "the agent keeps its own folder"
        );
        let persisted = std::env::current_dir()
            .unwrap()
            .join(".thclaws/state/workspace_root");
        assert!(persisted.exists(), "the pick survives a restart");

        set_workspace_override(None).unwrap();
        assert_eq!(workspace_root(), host_root.path(), "cleared = host default");
        assert!(!persisted.exists(), "clearing removes the file");

        std::env::set_current_dir(prev_cwd).unwrap();
        match prev_env {
            Some(v) => std::env::set_var("THCLAWS_WORKSPACE_ROOT", v),
            None => std::env::remove_var("THCLAWS_WORKSPACE_ROOT"),
        }
    }

    /// dev-plan/61: an agent under a workspace host shares the workspace's
    /// files. The host passes `THCLAWS_WORKSPACE_ROOT`; the agent's own folder
    /// stays its process cwd.
    #[test]
    fn an_agent_under_a_host_resolves_files_against_the_workspace() {
        let _g = crate::kms::test_env_lock();
        let prev = std::env::var("THCLAWS_WORKSPACE_ROOT").ok();
        let ws = tempfile::tempdir().unwrap();

        std::env::remove_var("THCLAWS_WORKSPACE_ROOT");
        assert_eq!(workspace_root(), std::env::current_dir().unwrap());

        std::env::set_var("THCLAWS_WORKSPACE_ROOT", ws.path());
        assert_eq!(workspace_root(), ws.path());
        assert_eq!(
            current_workdir(),
            ws.path(),
            "unscoped tools use the shared root"
        );

        std::env::set_var("THCLAWS_WORKSPACE_ROOT", "   ");
        assert_eq!(
            workspace_root(),
            std::env::current_dir().unwrap(),
            "blank is unset"
        );

        match prev {
            Some(v) => std::env::set_var("THCLAWS_WORKSPACE_ROOT", v),
            None => std::env::remove_var("THCLAWS_WORKSPACE_ROOT"),
        }
    }

    use super::*;

    /// finding 11: an agent's own `.thclaws/…` resolves in its folder, so it
    /// can reach the scripts and state it ships. The user's files and the two
    /// shared subpaths keep resolving at the workspace root.
    #[test]
    fn an_agents_own_thclaws_resolves_in_its_folder() {
        let _g = crate::kms::test_env_lock();
        let prev = std::env::var("THCLAWS_WORKSPACE_ROOT").ok();
        let ws = tempfile::tempdir().unwrap();
        let agent = std::env::current_dir().unwrap();

        std::env::set_var("THCLAWS_WORKSPACE_ROOT", ws.path());
        assert_eq!(
            agent_dir(),
            Some(agent.clone()),
            "cwd differs from the root"
        );

        for p in [
            ".thclaws",
            ".thclaws/book.json",
            "./.thclaws/scripts/book.py",
            ".thclaws/state/sessions/a.jsonl",
            ".thclaws/state/kmsx/not-the-carve-out",
        ] {
            assert_eq!(tool_base(p), agent, "{p} is the agent's");
        }
        for p in [
            "chapters/ch01.md",
            ".thclaws/state/kms",
            ".thclaws/state/kms/vault/page.md",
            ".thclaws/state/logs/engine.log",
            "/etc/hosts",
        ] {
            assert_eq!(tool_base(p), ws.path(), "{p} is not the agent's");
        }

        // Off a host cwd IS the root, so nothing moves.
        std::env::remove_var("THCLAWS_WORKSPACE_ROOT");
        assert_eq!(agent_dir(), None);
        assert_eq!(tool_base(".thclaws/book.json"), agent);

        match prev {
            Some(v) => std::env::set_var("THCLAWS_WORKSPACE_ROOT", v),
            None => std::env::remove_var("THCLAWS_WORKSPACE_ROOT"),
        }
    }

    #[tokio::test]
    async fn unscoped_falls_back_to_process_cwd() {
        // Other tests move the process cwd under this lock; without it the
        // two reads below can straddle one of those moves.
        let _g = crate::kms::test_env_lock();
        let expected = std::env::current_dir().unwrap();
        assert_eq!(current_workdir(), expected);
        assert!(!workdir_is_scoped());
    }

    #[tokio::test]
    async fn scope_overrides_and_is_isolated() {
        let a = PathBuf::from("/tmp/workspace-alice");
        let b = PathBuf::from("/tmp/workspace-bob");

        let got_a = scope_workdir(a.clone(), async {
            assert!(workdir_is_scoped());
            // Survives an await point (the multi-thread-runtime hazard).
            tokio::task::yield_now().await;
            current_workdir()
        })
        .await;
        let got_b = scope_workdir(b.clone(), async { current_workdir() }).await;

        assert_eq!(got_a, a);
        assert_eq!(got_b, b);
        assert_ne!(got_a, got_b, "concurrent sessions resolve independently");
        // Back outside any scope.
        assert!(!workdir_is_scoped());
    }
}
