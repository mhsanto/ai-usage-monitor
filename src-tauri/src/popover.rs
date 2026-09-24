use crate::state::{request, AppState, Trigger};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, Rect, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const LABEL: &str = "main";
const WIDTH: f64 = 320.0;
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
        .always_on_top(true)
        .visible(false)
        .build()?;

    let handle = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Focused(false)) && settled(&handle) {
            close(&handle);
        }
    });
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

fn place(app: &AppHandle, window: &WebviewWindow, height: f64, anchor: Option<Rect>) {
    let anchor = anchor.map(|rect| (rect.position.to_physical::<f64>(1.0), rect.size.to_physical::<f64>(1.0)));
    let monitor = anchor
        .and_then(|(pos, _)| app.monitor_from_point(pos.x, pos.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };

    let scale = monitor.scale_factor();
    let size = PhysicalSize::new((WIDTH * scale).round() as u32, (height * scale).round() as u32);
    let position = position_for(&monitor, size, anchor, (MARGIN * scale).round() as i32);
    let _ = window.set_position(position);
    let _ = window.set_size(size);
    // Moving across monitors with different DPI rescales the window; pin it again.
    let _ = window.set_position(position);
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
