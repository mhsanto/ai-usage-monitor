use crate::popover;
use crate::state::{now_ms, request, AppState, Meter, Severity, Snapshot, Trigger};
use std::f64::consts::TAU;
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

const TRAY_ID: &str = "usage";
const ICON_SIZE: usize = 32;
const TOOLTIP_MAX: usize = 127;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(app, "autostart", "Start with Windows", true, autostart_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit AI Usage Monitor", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&refresh, &autostart, &PredefinedMenuItem::separator(app)?, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(gauge_icon(None, Severity::Normal))
        .tooltip("AI Usage Monitor")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "refresh" => request(app, Trigger::Refresh),
            "autostart" => toggle_autostart(app, &autostart),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } => {
                popover::toggle(tray.app_handle(), rect);
            }
            // Recomputed on hover so "time left" is current, with no timer running while idle.
            TrayIconEvent::Enter { .. } | TrayIconEvent::Move { .. } => {
                let text = tooltip(&tray.app_handle().state::<AppState>().snapshot.lock().unwrap(), now_ms());
                set_tooltip(tray, text);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn update(app: &AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let worst = snapshot.claude.meters.iter().max_by(|a, b| a.percent.total_cmp(&b.percent));
    let _ = tray.set_icon(Some(gauge_icon(worst.map(|m| m.percent), worst.map_or(Severity::Normal, |m| m.severity))));
    set_tooltip(&tray, tooltip(snapshot, now_ms()));
}

/// Skips the shell call when the text is unchanged, since hover fires on every mouse move.
fn set_tooltip(tray: &TrayIcon, text: String) {
    static LAST: Mutex<String> = Mutex::new(String::new());
    {
        let mut last = LAST.lock().unwrap();
        if *last == text {
            return;
        }
        last.clone_from(&text);
    }
    let _ = tray.set_tooltip(Some(text));
}

fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<Wry>) {
    let launcher = app.autolaunch();
    let _ = if launcher.is_enabled().unwrap_or(false) { launcher.disable() } else { launcher.enable() };
    let _ = item.set_checked(launcher.is_enabled().unwrap_or(false));
}

fn tooltip(snapshot: &Snapshot, now: i64) -> String {
    let lines: Vec<String> = [("Claude", &snapshot.claude), ("Codex", &snapshot.codex)]
        .into_iter()
        .filter_map(|(name, provider)| {
            if provider.meters.is_empty() {
                return provider.error.as_ref().map(|_| format!("{name}: needs attention"));
            }
            let parts: Vec<String> = provider.meters.iter().map(|m| meter_tip(m, now)).collect();
            Some(format!("{name}: {}", parts.join(" · ")))
        })
        .collect();
    let text = if lines.is_empty() { "AI Usage Monitor".to_string() } else { lines.join("\n") };
    text.chars().take(TOOLTIP_MAX).collect()
}

fn meter_tip(meter: &Meter, now: i64) -> String {
    let usage = format!("{} {:.0}%", meter.short, meter.percent);
    match (meter.short.as_str(), meter.resets_at) {
        ("5h", Some(resets_at)) => format!("{usage} ({})", time_left(resets_at - now)),
        _ => usage,
    }
}

fn time_left(ms: i64) -> String {
    // Rounded up, so it never says "0m left" while time remains.
    let minutes = (ms + 59_999).div_euclid(60_000);
    match minutes {
        m if m <= 0 => "resetting now".to_string(),
        m if m < 60 => format!("{m}m left"),
        m => format!("{}h {}m left", m / 60, m % 60),
    }
}

/// A ring that fills clockwise from 12 o'clock with the highest Claude usage.
fn gauge_icon(percent: Option<f64>, severity: Severity) -> Image<'static> {
    let (outer, inner) = (15.5, 9.0);
    let center = ICON_SIZE as f64 / 2.0;
    let fill = match severity {
        Severity::Normal => [59, 130, 246],
        Severity::Warning => [245, 158, 11],
        Severity::Critical => [239, 68, 68],
    };
    let sweep = percent.map_or(0.0, |p| p.clamp(0.0, 100.0) / 100.0 * TAU);

    let mut rgba = vec![0u8; ICON_SIZE * ICON_SIZE * 4];
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let (dx, dy) = (x as f64 + 0.5 - center, y as f64 + 0.5 - center);
            let distance = dx.hypot(dy);
            let coverage = (outer - distance + 0.5).clamp(0.0, 1.0) * (distance - inner + 0.5).clamp(0.0, 1.0);
            if coverage == 0.0 {
                continue;
            }
            let angle = dx.atan2(-dy).rem_euclid(TAU);
            let (rgb, alpha) = if angle < sweep { (fill, 1.0) } else { ([140, 140, 140], 0.6) };
            let i = (y * ICON_SIZE + x) * 4;
            rgba[i..i + 3].copy_from_slice(&rgb);
            rgba[i + 3] = (coverage * alpha * 255.0).round() as u8;
        }
    }
    Image::new_owned(rgba, ICON_SIZE as u32, ICON_SIZE as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Meter, Provider};

    const MINUTE: i64 = 60_000;

    fn meter(short: &str, percent: f64, resets_at: Option<i64>) -> Meter {
        Meter { label: short.into(), short: short.into(), percent, severity: Severity::Normal, resets_at }
    }

    #[test]
    fn time_left_rounds_up_to_the_minute() {
        assert_eq!(time_left(170 * MINUTE), "2h 50m left");
        assert_eq!(time_left(45 * MINUTE), "45m left");
        assert_eq!(time_left(30_000), "1m left");
        assert_eq!(time_left(0), "resetting now");
    }

    #[test]
    fn tooltip_shows_time_left_on_the_five_hour_window_only() {
        let now = 1_000 * MINUTE;
        let snapshot = Snapshot {
            claude: Provider {
                available: true,
                meters: vec![
                    meter("5h", 42.0, Some(now + 130 * MINUTE)),
                    meter("Week", 85.0, Some(now + 5_000 * MINUTE)),
                    meter("Fable", 91.0, None),
                ],
                ..Default::default()
            },
            codex: Provider { available: true, meters: vec![meter("30d", 0.0, None)], ..Default::default() },
        };
        assert_eq!(tooltip(&snapshot, now), "Claude: 5h 42% (2h 10m left) · Week 85% · Fable 91%\nCodex: 30d 0%");
    }
}
