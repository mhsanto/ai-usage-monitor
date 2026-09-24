use crate::autoreset::{self, AutoState};
use crate::codex_api::{Auth, CodexApi};
use crate::settings::SettingsStore;
use crate::state::{now_ms, AppState, CodexResets, Provider, Trigger};
use crate::{claude, codex, tray};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::sleep_until;

const BASELINE: Duration = Duration::from_secs(10 * 60);
const ACTIVITY_GAP: Duration = Duration::from_secs(5 * 60);
const OPENED_GAP: Duration = Duration::from_secs(2 * 60);
const REFRESH_GAP: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_secs(3);
const CODEX_ACTIVITY_GAP: Duration = Duration::from_secs(2 * 60);
const CODEX_CHANGED_GAP: Duration = Duration::from_secs(15);
const CODEX_RETRY: Duration = Duration::from_secs(60);

pub async fn run(app: AppHandle, mut rx: UnboundedReceiver<Trigger>, claude_dir: PathBuf, codex_home: PathBuf, client: reqwest::Client) {
    let mut last_claude: Option<Instant> = None;
    let mut last_codex: Option<Instant> = None;
    let mut claude_due = Instant::now();
    let mut codex_due = Instant::now();
    let mut auto = AutoState::default();

    loop {
        tokio::select! {
            _ = sleep_until(claude_due.min(codex_due).into()) => {}
            trigger = rx.recv() => {
                let Some(trigger) = trigger else { return };
                let now = Instant::now();
                let claude_after = |gap| not_before(last_claude, gap, now);
                let codex_after = |gap| not_before(last_codex, gap, now);
                match trigger {
                    Trigger::ClaudeActivity => claude_due = claude_due.min(claude_after(ACTIVITY_GAP).max(now + DEBOUNCE)),
                    Trigger::CodexActivity => codex_due = codex_due.min(codex_after(CODEX_ACTIVITY_GAP).max(now + DEBOUNCE)),
                    Trigger::CodexChanged => codex_due = codex_due.min(codex_after(CODEX_CHANGED_GAP)),
                    Trigger::PopoverOpened => claude_due = claude_due.min(claude_after(OPENED_GAP)),
                    Trigger::Refresh => {
                        claude_due = claude_due.min(claude_after(REFRESH_GAP));
                        codex_due = codex_due.min(codex_after(REFRESH_GAP));
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
        if codex_due <= now {
            let next_in = refresh_codex(&app, &codex_home, &mut auto).await;
            last_codex = Some(Instant::now());
            codex_due = Instant::now() + next_in;
        }
        publish(&app);
    }
}

fn not_before(last: Option<Instant>, gap: Duration, now: Instant) -> Instant {
    last.map_or(now, |last| (last + gap).max(now))
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

/// Live numbers and auto-reset when Codex's login works; the session log otherwise.
async fn refresh_codex(app: &AppHandle, codex_home: &Path, auto: &mut AutoState) -> Duration {
    let settings = app.state::<SettingsStore>().get().codex_auto_reset;
    let api = app.state::<CodexApi>().inner().clone();
    let state = app.state::<AppState>();
    let result = {
        let _spending = state.reset_lock.lock().await;
        match Auth::load(codex_home) {
            Ok(auth) => autoreset::cycle(&api, &auth, &settings, auto).await,
            Err(error) => Err(error),
        }
    };

    let mut snapshot = state.snapshot.lock().unwrap();
    let last_event = snapshot.codex_resets.last_event.take();
    match result {
        Ok(cycle) => {
            snapshot.codex = Provider {
                available: true,
                plan: cycle.usage.plan.clone(),
                meters: cycle.usage.meters(),
                as_of: Some(now_ms()),
                error: None,
                note: None,
            };
            snapshot.codex_resets = CodexResets {
                live: true,
                available: cycle.usage.available_resets,
                next_expiry: crate::codex_api::earliest_expiring(&cycle.credits).and_then(|c| c.expires_at_ms),
                last_event: cycle.event.or(last_event),
                status: cycle.status,
            };
            if cycle.retry_soon { CODEX_RETRY } else { BASELINE }
        }
        Err(error) => {
            let mut provider = codex::read(codex_home);
            provider.note = provider.available.then_some(error.message);
            snapshot.codex = provider;
            snapshot.codex_resets = CodexResets { last_event, ..CodexResets::default() };
            error.retry_in.unwrap_or(BASELINE)
        }
    }
}

fn publish(app: &AppHandle) {
    let snapshot = app.state::<AppState>().snapshot.lock().unwrap().clone();
    tray::update(app, &snapshot);
    let _ = app.emit("usage-updated", &snapshot);
}
