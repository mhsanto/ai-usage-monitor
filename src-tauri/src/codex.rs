use crate::state::{capitalize, now_ms, Meter, Provider, Severity};
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const TAIL_BYTES: u64 = 512 * 1024;
const FILES_TO_TRY: usize = 10;

pub fn home_dir(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from)
}

pub fn sessions_dir(codex_home: &Path) -> PathBuf {
    codex_home.join("sessions")
}

#[derive(Deserialize)]
struct RateLimits {
    primary: Option<Window>,
    secondary: Option<Window>,
    plan_type: Option<String>,
}

#[derive(Deserialize)]
struct Window {
    used_percent: f64,
    window_minutes: Option<u64>,
    /// Epoch seconds.
    resets_at: Option<i64>,
}

pub fn read(codex_home: &Path) -> Provider {
    let sessions = sessions_dir(codex_home);
    if !sessions.is_dir() {
        return Provider::default();
    }

    let mut files = Vec::new();
    collect_jsonl(&sessions, 4, &mut files);
    files.sort_by_key(|(_, modified_ms)| std::cmp::Reverse(*modified_ms));

    for (path, modified_ms) in files.into_iter().take(FILES_TO_TRY) {
        if let Some(limits) = last_rate_limits(&path) {
            return provider(limits, modified_ms);
        }
    }
    Provider { available: true, note: Some("No Codex usage recorded yet.".into()), ..Default::default() }
}

fn collect_jsonl(dir: &Path, depth: u8, out: &mut Vec<(PathBuf, i64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() && depth > 0 {
            collect_jsonl(&path, depth - 1, out);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_millis() as i64);
            out.push((path, modified));
        }
    }
}

fn last_rate_limits(path: &Path) -> Option<RateLimits> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);

    text.lines()
        .rev()
        .filter(|line| line.contains("\"rate_limits\""))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|event| find_key(&event, "rate_limits").and_then(|limits| serde_json::from_value(limits.clone()).ok()))
}

fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .filter(|found| found.is_object())
            .or_else(|| map.values().find_map(|child| find_key(child, key))),
        Value::Array(items) => items.iter().find_map(|child| find_key(child, key)),
        _ => None,
    }
}

fn provider(limits: RateLimits, modified_ms: i64) -> Provider {
    let meters = [limits.primary, limits.secondary].into_iter().flatten().map(meter).collect();
    Provider {
        available: true,
        plan: limits.plan_type.as_deref().map(capitalize),
        meters,
        as_of: Some(modified_ms),
        error: None,
        note: None,
    }
}

fn meter(window: Window) -> Meter {
    let (label, short) = window_label(window.window_minutes);
    let resets_at = window.resets_at.map(|secs| secs * 1000);
    // A window that reset since the last Codex session is empty until the next one.
    let reset_passed = resets_at.is_some_and(|at| at <= now_ms());
    let percent = if reset_passed { 0.0 } else { window.used_percent };
    Meter {
        label,
        short,
        percent,
        severity: Severity::from_percent(percent),
        resets_at: resets_at.filter(|_| !reset_passed),
    }
}

pub(crate) fn window_label(minutes: Option<u64>) -> (String, String) {
    match minutes {
        Some(300) => ("5-hour window".into(), "5h".into()),
        Some(10080) => ("Weekly window".into(), "Week".into()),
        Some(m) if m % 1440 == 0 => (format!("{}-day window", m / 1440), format!("{}d", m / 1440)),
        Some(m) if m % 60 == 0 => (format!("{}-hour window", m / 60), format!("{}h", m / 60)),
        _ => ("Usage window".into(), "Limit".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_newest_rate_limits_in_a_session_log() {
        let dir = std::env::temp_dir().join(format!("codex-test-{}", std::process::id()));
        let day = dir.join("sessions/2026/08/25");
        std::fs::create_dir_all(&day).unwrap();
        let future = chrono::Utc::now().timestamp() + 3600;
        let log = format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"rate_limits\":{{\"primary\":{{\"used_percent\":40.0,\"window_minutes\":300,\"resets_at\":{future}}}}}}}}}\n\
             {{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"rate_limits\":{{\"primary\":{{\"used_percent\":2.0,\"window_minutes\":43200,\"resets_at\":{future}}},\"secondary\":null,\"plan_type\":\"free\"}}}}}}\n\
             {{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"rate_limits\":null}}}}\n"
        );
        std::fs::write(day.join("rollout-test.jsonl"), log).unwrap();

        let provider = read(&dir);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(provider.available);
        assert_eq!(provider.plan.as_deref(), Some("Free"));
        assert_eq!(provider.meters.len(), 1);
        assert_eq!((provider.meters[0].label.as_str(), provider.meters[0].percent), ("30-day window", 2.0));
    }

    #[test]
    fn a_window_that_already_reset_reads_empty() {
        let meter = meter(Window { used_percent: 80.0, window_minutes: Some(10080), resets_at: Some(1) });
        assert_eq!((meter.short.as_str(), meter.percent, meter.resets_at), ("Week", 0.0, None));
    }

    #[test]
    fn missing_codex_home_hides_the_card() {
        assert!(!read(Path::new("Z:/definitely/not/here")).available);
    }
}
