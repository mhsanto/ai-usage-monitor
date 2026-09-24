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

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub claude: Provider,
    pub codex: Provider,
}

pub enum Trigger {
    ClaudeActivity,
    CodexActivity,
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
    trigger: UnboundedSender<Trigger>,
}

impl AppState {
    pub fn new(trigger: UnboundedSender<Trigger>) -> Self {
        let snapshot = Snapshot {
            claude: Provider { available: true, ..Default::default() },
            codex: Provider::default(),
        };
        Self { snapshot: Mutex::new(snapshot), popover: Mutex::default(), trigger }
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
