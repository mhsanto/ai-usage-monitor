use crate::settings::SettingsStore;
use crate::state::{request, AppState, Trigger};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, Rect, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const LABEL: &str = "main";
const WIDTH: f64 = 320.0;
const COMPACT_WIDTH: f64 = 240.0;
const MARGIN: f64 = 12.0;
// The click that blurs the popover also lands on the tray icon; don't reopen on it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
// WebView2 bounces focus while the window is created and just after it is shown.
const FOCUS_SETTLE: Duration = Duration::from_millis(300);

pub fn toggle(app: &AppHandle, anchor: Rect) {
    if app.get_webview_window(LABEL).is_some() {
        close(app);
        return;
    }
    let closed_at = app.state::<AppState>().popover.lock().unwrap().closed_at;
    if closed_at.is_some_and(|at| at.elapsed() < REOPEN_GUARD) {
        return;
    }
    show(app, Some(anchor));
}

/// Opens the popover above `anchor`, or in the bottom-right corner without one.
pub fn show(app: &AppHandle, anchor: Option<Rect>) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.set_focus();
        return;
    }
    app.state::<AppState>().popover.lock().unwrap().anchor = anchor;
    // Created on demand and destroyed on close, so WebView2 isn't resident while hidden.
    // Off the main thread: building a webview inside an event handler deadlocks on Windows.
    let app = app.clone();
    std::thread::spawn(move || match open(&app) {
        Ok(()) => request(&app, Trigger::PopoverOpened),
        Err(error) => eprintln!("popover failed to open: {error}"),
    });
}

pub fn close(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    {
        let state = app.state::<AppState>();
        let mut popover = state.popover.lock().unwrap();
        popover.closed_at = Some(Instant::now());
        popover.shown_at = None;
    }
    let _ = window.destroy();
}

fn open(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("AI Usage Monitor")
        .inner_size(WIDTH, 360.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .always_on_top(app.state::<SettingsStore>().get().always_on_top)
        .visible(false)
        .build()?;

    let handle = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Focused(false)) && settled(&handle) && !handle.state::<SettingsStore>().get().keep_open {
            close(&handle);
        }
    });
    Ok(())
}

pub fn set_always_on_top(app: &AppHandle, enabled: bool) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.set_always_on_top(enabled)?;
    }
    Ok(())
}

fn settled(app: &AppHandle) -> bool {
    let shown_at = app.state::<AppState>().popover.lock().unwrap().shown_at;
    shown_at.is_some_and(|at| at.elapsed() >= FOCUS_SETTLE)
}

/// Called by the page once it has rendered, with its content height in CSS pixels.
#[tauri::command]
pub fn fit_popover(app: AppHandle, window: WebviewWindow, height: f64) {
    let anchor = app.state::<AppState>().popover.lock().unwrap().anchor;
    place(&app, &window, height, anchor);
    if !window.is_visible().unwrap_or(false) {
        let _ = window.show();
        let _ = window.set_focus();
        app.state::<AppState>().popover.lock().unwrap().shown_at = Some(Instant::now());
    }
}

#[tauri::command]
pub fn hide_popover(app: AppHandle) {
    close(&app);
}

#[tauri::command]
pub fn drag_popover(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if app.state::<SettingsStore>().get().keep_open {
        window.start_dragging().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn place(app: &AppHandle, window: &WebviewWindow, height: f64, anchor: Option<Rect>) {
    let settings = app.state::<SettingsStore>().get();
    let width = if settings.compact_mode { COMPACT_WIDTH } else { WIDTH };
    // A kept-open window belongs where the user placed it, including after refreshes.
    if settings.keep_open && window.is_visible().unwrap_or(false) {
        if let (Ok(Some(monitor)), Ok(position)) = (window.current_monitor(), window.outer_position()) {
            let scale = monitor.scale_factor();
            let size = PhysicalSize::new((width * scale).round() as u32, (height * scale).round() as u32);
            let area = monitor.work_area();
            let position = clamp_position(position, size, area.position, area.size);
            let _ = window.set_size(size);
            let _ = window.set_position(position);
            return;
        }
    }
    let anchor = anchor.map(|rect| (rect.position.to_physical::<f64>(1.0), rect.size.to_physical::<f64>(1.0)));
    let monitor = anchor
        .and_then(|(pos, _)| app.monitor_from_point(pos.x, pos.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };

    let scale = monitor.scale_factor();
    let size = PhysicalSize::new((width * scale).round() as u32, (height * scale).round() as u32);
    let position = position_for(&monitor, size, anchor, (MARGIN * scale).round() as i32);
    let _ = window.set_position(position);
    let _ = window.set_size(size);
    // Moving across monitors with different DPI rescales the window; pin it again.
    let _ = window.set_position(position);
}

fn clamp_position(position: PhysicalPosition<i32>, size: PhysicalSize<u32>, origin: PhysicalPosition<i32>, area: PhysicalSize<u32>) -> PhysicalPosition<i32> {
    PhysicalPosition::new(
        position.x.clamp(origin.x, (origin.x + area.width as i32 - size.width as i32).max(origin.x)),
        position.y.clamp(origin.y, (origin.y + area.height as i32 - size.height as i32).max(origin.y)),
    )
}

fn position_for(
    monitor: &Monitor,
    size: PhysicalSize<u32>,
    anchor: Option<(PhysicalPosition<f64>, PhysicalSize<f64>)>,
    margin: i32,
) -> PhysicalPosition<i32> {
    let area = monitor.work_area();
    let (left, top) = (area.position.x, area.position.y);
    let (right, bottom) = (left + area.size.width as i32, top + area.size.height as i32);
    let (w, h) = (size.width as i32, size.height as i32);

    let Some((pos, icon)) = anchor else {
        return PhysicalPosition::new(right - w - margin, bottom - h - margin);
    };
    let icon_center = (pos.x + icon.width / 2.0) as i32;
    let x = (icon_center - w / 2).clamp(left + margin, (right - w - margin).max(left + margin));
    let y = if (pos.y as i32) < top { top + margin } else { bottom - h - margin };
    PhysicalPosition::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_keeps_the_dragged_position_unless_it_would_go_off_screen() {
        let origin = PhysicalPosition::new(-1920, 0);
        let area = PhysicalSize::new(1920, 1040);
        let position = PhysicalPosition::new(-400, 700);
        assert_eq!(clamp_position(position, PhysicalSize::new(240, 200), origin, area), position);
        assert_eq!(clamp_position(position, PhysicalSize::new(320, 500), origin, area), PhysicalPosition::new(-400, 540));
        assert_eq!(clamp_position(PhysicalPosition::new(-200, 100), PhysicalSize::new(320, 500), origin, area), PhysicalPosition::new(-320, 100));
    }
}
