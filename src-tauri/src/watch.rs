use crate::state::Trigger;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// Keeps the OS watch handles alive for the life of the app.
pub struct Watchers(#[allow(dead_code)] Mutex<Option<RecommendedWatcher>>);

pub fn start(claude_dir: &Path, codex_sessions: &Path, tx: UnboundedSender<Trigger>) -> Watchers {
    let claude = claude_dir.to_path_buf();
    let codex = codex_sessions.to_path_buf();

    let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let Ok(event) = result else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        for path in &event.paths {
            let trigger = if path.starts_with(&codex) {
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
        for dir in [claude_dir, codex_sessions] {
            if dir.is_dir() {
                let _ = watcher.watch(dir, RecursiveMode::Recursive);
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
