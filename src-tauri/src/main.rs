// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autoreset;
mod claude;
mod codex;
mod codex_api;
mod popover;
mod settings;
mod state;
mod sync;
mod tray;
mod watch;

use codex_api::{Auth, CodexApi};
use settings::{Settings, SettingsStore};
use state::{AppState, ResetEvent, Snapshot, Trigger};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, State};
use tauri_plugin_autostart::MacosLauncher;

struct CodexHome(PathBuf);

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> Snapshot {
    state.snapshot.lock().unwrap().clone()
}

#[tauri::command]
fn refresh_now(app: AppHandle) {
    state::request(&app, Trigger::Refresh);
}

#[tauri::command]
fn get_settings(store: State<SettingsStore>) -> Settings {
    store.get()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    let previous = app.state::<SettingsStore>().get();
    let saved = app.state::<SettingsStore>().save(settings)?;
    popover::set_always_on_top(&app, saved.always_on_top).map_err(|error| format!("Settings saved, but couldn't update Always on top: {error}"))?;
    if saved.codex_auto_reset != previous.codex_auto_reset {
        state::request(&app, Trigger::CodexChanged);
    }
    Ok(saved)
}

#[tauri::command]
async fn use_codex_reset(app: AppHandle) -> Result<ResetEvent, String> {
    let api = app.state::<CodexApi>().inner().clone();
    let codex_home = app.state::<CodexHome>().0.clone();
    let state = app.state::<AppState>();
    let event = {
        let _spending = state.reset_lock.lock().await;
        match Auth::load(&codex_home) {
            Ok(auth) => autoreset::use_now(&api, &auth).await,
            Err(error) => ResetEvent::failed(error.message),
        }
    };
    state.snapshot.lock().unwrap().codex_resets.last_event = Some(event.clone());
    state::request(&app, Trigger::CodexChanged);
    Ok(event)
}

fn main() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| popover::show(app, None)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .manage(AppState::new(tx.clone()))
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh_now,
            get_settings,
            save_settings,
            use_codex_reset,
            popover::fit_popover,
            popover::drag_popover,
            popover::hide_popover
        ])
        .setup(move |app| {
            let home = app.path().home_dir()?;
            let claude_dir = claude::config_dir(&home);
            let codex_home = codex::home_dir(&home);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("ai-usage-monitor/", env!("CARGO_PKG_VERSION")))
                .build()?;

            app.manage(SettingsStore::load(app.path().app_config_dir()?.join("settings.json")));
            app.manage(CodexApi::new(client.clone(), codex_api::BASE_URL));
            app.manage(CodexHome(codex_home.clone()));
            app.manage(watch::start(&claude_dir, &codex_home, tx));
            tray::build(app.handle())?;
            tauri::async_runtime::spawn(sync::run(app.handle().clone(), rx, claude_dir, codex_home, client));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start AI Usage Monitor")
        .run(|_, event| {
            // Closing the popover must not quit the tray app; only the Quit menu item does.
            if let RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
