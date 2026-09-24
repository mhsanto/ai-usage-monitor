use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Manager, Rect};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Normal,
    Warning,
    Critical,
}

impl Severity {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "normal" => Some(Self::Normal),
            "warning" => Some(Self::Warning),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn from_percent(percent: f64) -> Self {
        if percent >= 90.0 {
            Self::Critical
        } else if percent >= 75.0 {
            Self::Warning
        } else {
            Self::Normal
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meter {
    pub label: String,
    pub short: String,
    pub percent: f64,
    pub severity: Severity,
    /// Epoch milliseconds.
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub available: bool,
    pub plan: Option<String>,
    pub meters: Vec<Meter>,
    /// Epoch milliseconds of the moment the meters describe.
    pub as_of: Option<i64>,
    pub error: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetEvent {
    /// Epoch milliseconds.
    pub at: i64,
    pub ok: bool,
    pub message: String,
}

impl ResetEvent {
    pub fn succeeded(message: impl Into<String>) -> Self {
        Self { at: now_ms(), ok: true, message: message.into() }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self { at: now_ms(), ok: false, message: message.into() }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResets {
    /// Numbers came from OpenAI just now, not from Codex's session log.
    pub live: bool,
    pub available: i64,
    /// Epoch milliseconds of the soonest-expiring reset.
    pub next_expiry: Option<i64>,
    pub last_event: Option<ResetEvent>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub claude: Provider,
    pub codex: Provider,
    pub codex_resets: CodexResets,
}

pub enum Trigger {
    ClaudeActivity,
    CodexActivity,
    CodexChanged,
    PopoverOpened,
    Refresh,
}

#[derive(Default)]
pub struct Popover {
    pub anchor: Option<Rect>,
    pub closed_at: Option<Instant>,
    pub shown_at: Option<Instant>,
}

pub struct AppState {
    pub snapshot: Mutex<Snapshot>,
    pub popover: Mutex<Popover>,
    /// Held while a reset is chosen and spent, so the button and the auto rule never both spend one.
    pub reset_lock: tokio::sync::Mutex<()>,
    trigger: UnboundedSender<Trigger>,
}

impl AppState {
    pub fn new(trigger: UnboundedSender<Trigger>) -> Self {
        let snapshot = Snapshot { claude: Provider { available: true, ..Default::default() }, ..Default::default() };
        Self { snapshot: Mutex::new(snapshot), popover: Mutex::default(), reset_lock: tokio::sync::Mutex::new(()), trigger }
    }
}

pub fn request(app: &AppHandle, trigger: Trigger) {
    let _ = app.state::<AppState>().trigger.send(trigger);
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}
