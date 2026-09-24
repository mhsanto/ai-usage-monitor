// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod codex;
mod popover;
mod state;
mod sync;
mod tray;
mod watch;

use state::{AppState, Snapshot, Trigger};
use tauri::{AppHandle, Manager, RunEvent, State};
use tauri_plugin_autostart::MacosLauncher;

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> Snapshot {
    state.snapshot.lock().unwrap().clone()
}

#[tauri::command]
fn refresh_now(app: AppHandle) {
    state::request(&app, Trigger::Refresh);
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
            popover::fit_popover,
            popover::hide_popover
        ])
        .setup(move |app| {
            let home = app.path().home_dir()?;
            let claude_dir = claude::config_dir(&home);
            let codex_home = codex::home_dir(&home);
            app.manage(watch::start(&claude_dir, &codex::sessions_dir(&codex_home), tx));
            tray::build(app.handle())?;
            tauri::async_runtime::spawn(sync::run(app.handle().clone(), rx, claude_dir, codex_home));
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
