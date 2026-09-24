use crate::codex::sessions_dir;
use crate::state::Trigger;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// Keeps the OS watch handles alive for the life of the app.
pub struct Watchers(#[allow(dead_code)] Mutex<Option<RecommendedWatcher>>);

pub fn start(claude_dir: &Path, codex_home: &Path, tx: UnboundedSender<Trigger>) -> Watchers {
    let claude = claude_dir.to_path_buf();
    let codex = codex_home.to_path_buf();
    let codex_sessions = sessions_dir(codex_home);

    let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let Ok(event) = result else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        for path in &event.paths {
            let trigger = if path.starts_with(&codex) && is_codex_activity(path) {
                Trigger::CodexActivity
            } else if path.starts_with(&claude) && is_claude_activity(path) {
                Trigger::ClaudeActivity
            } else {
                continue;
            };
            let _ = tx.send(trigger);
        }
    });

    let watcher = watcher.ok().map(|mut watcher| {
        let dirs = [
            (claude_dir, RecursiveMode::Recursive),
            (codex_sessions.as_path(), RecursiveMode::Recursive),
            // Codex rewrites auth.json here when it refreshes its login.
            (codex_home, RecursiveMode::NonRecursive),
        ];
        for (dir, mode) in dirs {
            if dir.is_dir() {
                let _ = watcher.watch(dir, mode);
            }
        }
        watcher
    });
    Watchers(Mutex::new(watcher))
}

fn is_claude_activity(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        || path.file_name().is_some_and(|name| name == ".credentials.json")
}

fn is_codex_activity(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl") || path.file_name().is_some_and(|name| name == "auth.json")
}
