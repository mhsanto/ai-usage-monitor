use crate::codex::window_label;
use crate::state::{capitalize, Meter, Severity};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

// Undocumented: the ChatGPT endpoints the official Codex app uses for usage and resets.
pub const BASE_URL: &str = "https://chatgpt.com/backend-api";
const WEEK_SECS: i64 = 6 * 24 * 3600;
const FIVE_HOUR_SECS: i64 = 6 * 3600;

pub struct Auth {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct AuthFile {
    auth_mode: Option<String>,
    tokens: Option<AuthTokens>,
}

#[derive(Deserialize)]
struct AuthTokens {
    access_token: String,
    account_id: Option<String>,
}

impl Auth {
    /// Reads Codex's own login on every call; this app never stores or refreshes it.
    pub fn load(codex_home: &Path) -> Result<Self, ApiError> {
        let missing = || ApiError::new("Log in to Codex with ChatGPT to see live usage and resets.", None);
        let text = std::fs::read_to_string(codex_home.join("auth.json")).map_err(|_| missing())?;
        let file: AuthFile = serde_json::from_str(&text).map_err(|_| missing())?;
        if file.auth_mode.as_deref().is_some_and(|mode| mode != "chatgpt") {
            return Err(missing());
        }
        let tokens = file.tokens.ok_or_else(missing)?;
        Ok(Self { access_token: tokens.access_token, account_id: tokens.account_id })
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub message: String,
    pub retry_in: Option<Duration>,
    /// The request may not have reached OpenAI, so repeating it with the same key is safe.
    pub retryable: bool,
}

impl ApiError {
    fn new(message: impl Into<String>, retry_in: Option<Duration>) -> Self {
        Self { message: message.into(), retry_in, retryable: false }
    }

    fn retryable(message: impl Into<String>, retry_in: Duration) -> Self {
        Self { message: message.into(), retry_in: Some(retry_in), retryable: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub used_percent: f64,
    pub length_secs: i64,
    pub resets_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveUsage {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub available_resets: i64,
}

impl LiveUsage {
    pub fn weekly(&self) -> Option<&Window> {
        self.windows.iter().filter(|w| w.length_secs >= WEEK_SECS).max_by_key(|w| w.length_secs)
    }

    pub fn five_hour(&self) -> Option<&Window> {
        self.windows.iter().filter(|w| w.length_secs <= FIVE_HOUR_SECS).min_by_key(|w| w.length_secs)
    }

    pub fn meters(&self) -> Vec<Meter> {
        self.windows
            .iter()
            .map(|window| {
                let (label, short) = window_label(Some((window.length_secs / 60) as u64));
                Meter {
                    label,
                    short,
                    percent: window.used_percent,
                    severity: Severity::from_percent(window.used_percent),
                    resets_at: window.resets_at_ms,
                }
            })
            .collect()
    }

    fn from_payload(payload: UsagePayload, now_ms: i64) -> Self {
        let limit = payload.rate_limit.unwrap_or_default();
        let mut windows: Vec<Window> = [limit.primary_window, limit.secondary_window]
            .into_iter()
            .flatten()
            .map(|w| Window {
                used_percent: w.used_percent,
                length_secs: w.limit_window_seconds,
                resets_at_ms: w
                    .reset_at
                    .map(|secs| secs * 1000)
                    .or(w.reset_after_seconds.map(|secs| now_ms + secs * 1000)),
            })
            .collect();
        windows.sort_by_key(|w| w.length_secs);
        Self {
            plan: payload.plan_type.as_deref().map(capitalize),
            windows,
            available_resets: payload.rate_limit_reset_credits.map_or(0, |c| c.available_count),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResetCredit {
    pub id: String,
    pub expires_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumeOutcome {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct UsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<LimitPayload>,
    rate_limit_reset_credits: Option<CreditsSummary>,
}

#[derive(Default, Deserialize)]
struct LimitPayload {
    primary_window: Option<WindowPayload>,
    secondary_window: Option<WindowPayload>,
}

#[derive(Deserialize)]
struct WindowPayload {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: Option<i64>,
    reset_after_seconds: Option<i64>,
}

#[derive(Deserialize)]
struct CreditsSummary {
    available_count: i64,
}

#[derive(Deserialize)]
struct CreditsPayload {
    #[serde(default)]
    credits: Vec<CreditPayload>,
}

#[derive(Deserialize)]
struct CreditPayload {
    id: String,
    reset_type: String,
    status: String,
    expires_at: Option<String>,
}

#[derive(Serialize)]
struct ConsumeRequest<'a> {
    redeem_request_id: &'a str,
    credit_id: &'a str,
}

#[derive(Deserialize)]
struct ConsumeResponse {
    code: ConsumeOutcome,
}

#[derive(Clone)]
pub struct CodexApi {
    client: reqwest::Client,
    base_url: String,
}

impl CodexApi {
    pub fn new(client: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self { client, base_url: base_url.into() }
    }

    pub async fn usage(&self, auth: &Auth) -> Result<LiveUsage, ApiError> {
        let payload: UsagePayload = send(self.authorized(self.client.get(self.url("/wham/usage")), auth)).await?;
        Ok(LiveUsage::from_payload(payload, crate::state::now_ms()))
    }

    /// Only credits that are available and reset Codex limits; anything else is left alone.
    pub async fn reset_credits(&self, auth: &Auth) -> Result<Vec<ResetCredit>, ApiError> {
        let request = self.client.get(self.url("/wham/rate-limit-reset-credits"));
        let payload: CreditsPayload = send(self.authorized(request, auth)).await?;
        Ok(payload
            .credits
            .into_iter()
            .filter(|credit| credit.status == "available" && credit.reset_type == "codex_rate_limits")
            .map(|credit| ResetCredit {
                id: credit.id,
                expires_at_ms: credit.expires_at.as_deref().and_then(parse_time),
            })
            .collect())
    }

    /// Spends one owned credit; OpenAI dedupes on `redeem_request_id`, so a retry with the same key is safe.
    pub async fn consume(&self, auth: &Auth, redeem_request_id: &str, credit_id: &str) -> Result<ConsumeOutcome, ApiError> {
        let request = self
            .client
            .post(self.url("/wham/rate-limit-reset-credits/consume"))
            .json(&ConsumeRequest { redeem_request_id, credit_id });
        let response: ConsumeResponse = send(self.authorized(request, auth)).await?;
        Ok(response.code)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn authorized(&self, request: reqwest::RequestBuilder, auth: &Auth) -> reqwest::RequestBuilder {
        let request = request.bearer_auth(&auth.access_token);
        match &auth.account_id {
            Some(account_id) => request.header("ChatGPT-Account-Id", account_id),
            None => request,
        }
    }
}

pub fn earliest_expiring(credits: &[ResetCredit]) -> Option<&ResetCredit> {
    credits.iter().min_by_key(|credit| credit.expires_at_ms.unwrap_or(i64::MAX))
}

async fn send<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, ApiError> {
    let response = request
        .send()
        .await
        .map_err(|_| ApiError::retryable("Can't reach OpenAI.", Duration::from_secs(120)))?;
    match response.status().as_u16() {
        200..=299 => {}
        401 | 403 => return Err(ApiError::new("Codex login expired. Open Codex once and this refreshes by itself.", None)),
        429 => return Err(ApiError::retryable("OpenAI is throttling usage checks.", Duration::from_secs(900))),
        code @ 500..=599 => return Err(ApiError::retryable(format!("OpenAI answered HTTP {code}."), Duration::from_secs(300))),
        code => return Err(ApiError::new(format!("OpenAI answered HTTP {code}."), Some(Duration::from_secs(300)))),
    }
    response
        .json()
        .await
        .map_err(|_| ApiError::new("OpenAI changed its Codex usage format. This app needs an update.", None))
}

fn parse_time(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp_millis())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A one-request HTTP server that answers `body` and reports what it received.
    pub(crate) fn fake_server(status: u16, body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut request_body = vec![0; content_length];
            reader.read_exact(&mut request_body).unwrap();
            let _ = tx.send(format!("{head}{}", String::from_utf8_lossy(&request_body)));
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.into_inner().write_all(reply.as_bytes()).unwrap();
        });
        (base, rx)
    }

    pub(crate) fn auth() -> Auth {
        Auth { access_token: "test-token".into(), account_id: Some("acct-1".into()) }
    }

    pub(crate) const PLUS_USAGE: &str = r#"{
        "plan_type": "plus",
        "rate_limit": {
            "allowed": true, "limit_reached": false,
            "primary_window": {"used_percent": 40, "limit_window_seconds": 18000, "reset_after_seconds": 3600, "reset_at": 1790000000},
            "secondary_window": {"used_percent": 96, "limit_window_seconds": 604800, "reset_after_seconds": 400000, "reset_at": 1790400000}
        },
        "credits": null,
        "rate_limit_reset_credits": {"available_count": 2},
        "account_id": "acct-1"
    }"#;

    #[tokio::test]
    async fn reads_live_usage_with_the_codex_login_headers() {
        let (base, requests) = fake_server(200, PLUS_USAGE);
        let usage = CodexApi::new(reqwest::Client::new(), base).usage(&auth()).await.unwrap();

        let request = requests.recv().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /wham/usage "));
        assert!(request.contains("authorization: bearer test-token"));
        assert!(request.contains("chatgpt-account-id: acct-1"));

        assert_eq!(usage.plan.as_deref(), Some("Plus"));
        assert_eq!(usage.available_resets, 2);
        assert_eq!(usage.five_hour().map(|w| w.used_percent), Some(40.0));
        assert_eq!(usage.weekly().map(|w| (w.used_percent, w.resets_at_ms)), Some((96.0, Some(1_790_400_000_000))));
        let labels: Vec<_> = usage.meters().into_iter().map(|m| m.label).collect();
        assert_eq!(labels, ["5-hour window", "Weekly window"]);
    }

    #[tokio::test]
    async fn lists_only_available_codex_credits() {
        let (base, _) = fake_server(
            200,
            r#"{"available_count": 2, "credits": [
                {"id": "a", "reset_type": "codex_rate_limits", "status": "available", "granted_at": "2026-09-01T00:00:00Z", "expires_at": "2026-10-12T00:00:00Z"},
                {"id": "b", "reset_type": "codex_rate_limits", "status": "redeemed", "granted_at": "2026-09-01T00:00:00Z", "expires_at": null},
                {"id": "c", "reset_type": "something_else", "status": "available", "granted_at": "2026-09-01T00:00:00Z", "expires_at": null},
                {"id": "d", "reset_type": "codex_rate_limits", "status": "available", "granted_at": "2026-09-02T00:00:00Z", "expires_at": "2026-10-01T00:00:00Z"}
            ]}"#,
        );
        let credits = CodexApi::new(reqwest::Client::new(), base).reset_credits(&auth()).await.unwrap();

        let ids: Vec<_> = credits.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "d"]);
        assert_eq!(earliest_expiring(&credits).map(|c| c.id.as_str()), Some("d"));
    }

    #[tokio::test]
    async fn consume_sends_the_credit_and_idempotency_key() {
        let (base, requests) = fake_server(200, r#"{"code": "reset", "windows_reset": 2}"#);
        let outcome = CodexApi::new(reqwest::Client::new(), base).consume(&auth(), "key-1", "credit-9").await.unwrap();

        let request = requests.recv().unwrap();
        assert!(request.starts_with("POST /wham/rate-limit-reset-credits/consume "));
        assert!(request.ends_with(r#"{"redeem_request_id":"key-1","credit_id":"credit-9"}"#));
        assert_eq!(outcome, ConsumeOutcome::Reset);
    }

    #[tokio::test]
    async fn expired_login_is_not_retryable() {
        let (base, _) = fake_server(401, "{}");
        let error = CodexApi::new(reqwest::Client::new(), base).usage(&auth()).await.unwrap_err();
        assert!(!error.retryable);
        assert!(error.message.contains("login expired"));
    }
}
