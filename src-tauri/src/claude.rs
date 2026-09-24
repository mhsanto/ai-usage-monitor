use crate::state::{capitalize, now_ms, Meter, Severity};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

// Undocumented: the endpoint behind Claude Code's /usage screen.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

pub fn config_dir(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(|| home.join(".claude"), PathBuf::from)
}

pub struct Fetched {
    pub plan: Option<String>,
    pub meters: Vec<Meter>,
}

pub struct FetchError {
    pub message: String,
    pub retry_in: Option<Duration>,
}

impl FetchError {
    fn new(message: impl Into<String>, retry_in: Option<Duration>) -> Self {
        Self { message: message.into(), retry_in }
    }

    fn login_expired() -> Self {
        Self::new("Claude login expired. Use Claude Code once and this refreshes by itself.", None)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialsFile {
    claude_ai_oauth: Option<OAuth>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuth {
    access_token: String,
    expires_at: Option<i64>,
    subscription_type: Option<String>,
    rate_limit_tier: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
    limits: Option<Vec<ApiLimit>>,
    five_hour: Option<ApiWindow>,
    seven_day: Option<ApiWindow>,
}

#[derive(Deserialize)]
struct ApiLimit {
    kind: String,
    percent: Option<f64>,
    severity: Option<String>,
    resets_at: Option<String>,
    scope: Option<Value>,
}

#[derive(Deserialize)]
struct ApiWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

pub async fn fetch(client: &reqwest::Client, dir: &Path) -> Result<Fetched, FetchError> {
    let oauth = read_oauth(dir)?;
    if oauth.expires_at.is_some_and(|at| at <= now_ms()) {
        return Err(FetchError::login_expired());
    }

    let response = client
        .get(USAGE_URL)
        .bearer_auth(&oauth.access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|_| FetchError::new("Can't reach Anthropic. Showing the last numbers I had.", Some(Duration::from_secs(120))))?;

    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(FetchError::login_expired()),
        429 => return Err(FetchError::new("Anthropic is throttling usage checks. Retrying in 15 minutes.", Some(Duration::from_secs(900)))),
        code => return Err(FetchError::new(format!("Anthropic answered HTTP {code}."), Some(Duration::from_secs(300)))),
    }

    let body: UsageResponse = response
        .json()
        .await
        .map_err(|_| FetchError::new("Anthropic changed its usage format. This app needs an update.", None))?;

    Ok(Fetched { plan: plan_label(&oauth), meters: meters(body) })
}

fn read_oauth(dir: &Path) -> Result<OAuth, FetchError> {
    let not_logged_in = || FetchError::new("Not logged in to Claude Code on this PC.", None);
    let text = std::fs::read_to_string(dir.join(".credentials.json")).map_err(|_| not_logged_in())?;
    let file: CredentialsFile = serde_json::from_str(&text).map_err(|_| not_logged_in())?;
    file.claude_ai_oauth.ok_or_else(not_logged_in)
}

fn plan_label(oauth: &OAuth) -> Option<String> {
    let plan = oauth.subscription_type.as_deref().map(capitalize)?;
    let multiplier = oauth
        .rate_limit_tier
        .as_deref()
        .and_then(|tier| tier.rsplit('_').next())
        .filter(|last| last.ends_with('x') && last[..last.len() - 1].parse::<u32>().is_ok());
    Some(match multiplier {
        Some(m) => format!("{plan} · {m}"),
        None => plan,
    })
}

fn meters(body: UsageResponse) -> Vec<Meter> {
    if let Some(limits) = body.limits.filter(|limits| !limits.is_empty()) {
        return limits.into_iter().filter_map(meter_from_limit).collect();
    }
    [("5-hour session", "5h", body.five_hour), ("Weekly · all models", "Week", body.seven_day)]
        .into_iter()
        .filter_map(|(label, short, window)| {
            let window = window?;
            let percent = window.utilization?;
            Some(Meter {
                label: label.into(),
                short: short.into(),
                percent,
                severity: Severity::from_percent(percent),
                resets_at: window.resets_at.as_deref().and_then(parse_time),
            })
        })
        .collect()
}

fn meter_from_limit(limit: ApiLimit) -> Option<Meter> {
    let percent = limit.percent?;
    let (label, short) = match (limit.kind.as_str(), scope_name(limit.scope.as_ref())) {
        ("session", None) => ("5-hour session".to_string(), "5h".to_string()),
        ("weekly_all", None) => ("Weekly · all models".to_string(), "Week".to_string()),
        (kind, Some(name)) if kind.starts_with("weekly") => (format!("Weekly · {name}"), name),
        (kind, Some(name)) => (format!("{} · {name}", humanize(kind)), name),
        (kind, None) => (humanize(kind), humanize(kind)),
    };
    let severity = limit
        .severity
        .as_deref()
        .and_then(Severity::parse)
        .unwrap_or_else(|| Severity::from_percent(percent));
    Some(Meter { label, short, percent, severity, resets_at: limit.resets_at.as_deref().and_then(parse_time) })
}

fn scope_name(scope: Option<&Value>) -> Option<String> {
    let scope = scope?;
    ["model", "surface"].iter().find_map(|key| match &scope[key] {
        Value::String(name) => Some(name.clone()),
        other => other["display_name"].as_str().map(str::to_string),
    })
}

fn humanize(kind: &str) -> String {
    capitalize(&kind.replace('_', " "))
}

fn parse_time(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_live_limits_list() {
        let body: UsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 27.0, "resets_at": "2026-09-24T12:50:00.093664+00:00"},
                "limits": [
                    {"kind": "session", "percent": 27, "severity": "normal", "resets_at": "2026-09-24T12:50:00.093664+00:00", "scope": null},
                    {"kind": "weekly_all", "percent": 82, "severity": "warning", "resets_at": "2026-09-28T01:00:00.093688+00:00", "scope": null},
                    {"kind": "weekly_scoped", "percent": 91, "severity": "critical", "resets_at": null,
                     "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}}
                ]
            }"#,
        )
        .unwrap();

        let meters = meters(body);
        let summary: Vec<_> = meters.iter().map(|m| (m.label.as_str(), m.short.as_str(), m.percent, m.severity)).collect();
        assert_eq!(
            summary,
            [
                ("5-hour session", "5h", 27.0, Severity::Normal),
                ("Weekly · all models", "Week", 82.0, Severity::Warning),
                ("Weekly · Fable", "Fable", 91.0, Severity::Critical),
            ]
        );
        assert_eq!(meters[0].resets_at, Some(1_790_254_200_093));
        assert_eq!(meters[2].resets_at, None);
    }

    #[test]
    fn falls_back_to_the_older_window_fields() {
        let body: UsageResponse = serde_json::from_str(
            r#"{"limits": null, "five_hour": {"utilization": 95.0, "resets_at": null}, "seven_day": null}"#,
        )
        .unwrap();

        let meters = meters(body);
        assert_eq!(meters.len(), 1);
        assert_eq!((meters[0].short.as_str(), meters[0].severity), ("5h", Severity::Critical));
    }

    #[test]
    fn labels_the_plan_with_its_multiplier() {
        let oauth = |tier: &str| OAuth {
            access_token: String::new(),
            expires_at: None,
            subscription_type: Some("team".into()),
            rate_limit_tier: Some(tier.into()),
        };
        assert_eq!(plan_label(&oauth("default_claude_max_5x")).as_deref(), Some("Team · 5x"));
        assert_eq!(plan_label(&oauth("default_claude_ai")).as_deref(), Some("Team"));
    }
}
