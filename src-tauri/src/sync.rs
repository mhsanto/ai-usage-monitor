use crate::state::{now_ms, AppState, Provider, Trigger};
use crate::{claude, codex, tray};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::sleep_until;

const BASELINE: Duration = Duration::from_secs(10 * 60);
const ACTIVITY_GAP: Duration = Duration::from_secs(5 * 60);
const OPENED_GAP: Duration = Duration::from_secs(2 * 60);
const REFRESH_GAP: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_secs(3);

pub async fn run(app: AppHandle, mut rx: UnboundedReceiver<Trigger>, claude_dir: PathBuf, codex_home: PathBuf) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("ai-usage-monitor/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("HTTP client");

    let mut last_claude: Option<Instant> = None;
    let mut claude_due = Instant::now();
    let mut codex_due = Some(Instant::now());

    loop {
        let next = codex_due.map_or(claude_due, |due| due.min(claude_due));
        tokio::select! {
            _ = sleep_until(next.into()) => {}
            trigger = rx.recv() => {
                let Some(trigger) = trigger else { return };
                let now = Instant::now();
                let not_before = |gap: Duration| last_claude.map_or(now, |last| (last + gap).max(now));
                match trigger {
                    Trigger::ClaudeActivity => claude_due = claude_due.min(not_before(ACTIVITY_GAP).max(now + DEBOUNCE)),
                    Trigger::CodexActivity => codex_due = Some(codex_due.map_or(now + DEBOUNCE, |due| due.min(now + DEBOUNCE))),
                    Trigger::PopoverOpened => claude_due = claude_due.min(not_before(OPENED_GAP)),
                    Trigger::Refresh => {
                        claude_due = claude_due.min(not_before(REFRESH_GAP));
                        codex_due = Some(now);
                    }
                }
                continue;
            }
        }

        let now = Instant::now();
        if claude_due <= now {
            let result = claude::fetch(&client, &claude_dir).await;
            last_claude = Some(Instant::now());
            let retry_in = result.as_ref().err().and_then(|e| e.retry_in).unwrap_or(BASELINE);
            claude_due = Instant::now() + retry_in;
            apply_claude(&app, result);
        }
        if codex_due.is_some_and(|due| due <= now) {
            codex_due = None;
            let provider = codex::read(&codex_home);
            app.state::<AppState>().snapshot.lock().unwrap().codex = provider;
        }
        publish(&app);
    }
}

fn apply_claude(app: &AppHandle, result: Result<claude::Fetched, claude::FetchError>) {
    let state = app.state::<AppState>();
    let mut snapshot = state.snapshot.lock().unwrap();
    match result {
        Ok(fetched) => {
            snapshot.claude = Provider {
                available: true,
                plan: fetched.plan,
                meters: fetched.meters,
                as_of: Some(now_ms()),
                error: None,
                note: None,
            };
        }
        // Keep the last good meters; the error line says they may be stale.
        Err(error) => snapshot.claude.error = Some(error.message),
    }
}

fn publish(app: &AppHandle) {
    let snapshot = app.state::<AppState>().snapshot.lock().unwrap().clone();
    tray::update(app, &snapshot);
    let _ = app.emit("usage-updated", &snapshot);
}
