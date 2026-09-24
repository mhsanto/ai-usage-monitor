use crate::codex_api::{earliest_expiring, ApiError, Auth, CodexApi, ConsumeOutcome, LiveUsage, ResetCredit};
use crate::settings::AutoReset;
use crate::state::{now_ms, ResetEvent};

const MAX_ATTEMPTS: u8 = 3;
const MINUTE_MS: i64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reason {
    Weekly { percent: f64 },
    FiveHour { minutes_left: i64 },
}

impl Reason {
    fn describe(self) -> String {
        match self {
            Reason::Weekly { percent } => format!("weekly usage hit {percent:.0}%"),
            Reason::FiveHour { minutes_left } => format!("5-hour window was blocked for {}", duration(minutes_left)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Idle,
    Skip(String),
    Reset(Reason),
}

/// Whether usage alone calls for a reset, before availability and timing guards.
fn trigger(settings: &AutoReset, usage: &LiveUsage, now: i64) -> Option<Reason> {
    let weekly = usage.weekly().filter(|_| settings.weekly_enabled);
    if let Some(window) = weekly.filter(|w| w.used_percent >= settings.weekly_percent) {
        return Some(Reason::Weekly { percent: window.used_percent });
    }
    let five_hour = usage.five_hour().filter(|_| settings.five_hour_enabled);
    let blocked = five_hour.filter(|w| w.used_percent >= 100.0)?;
    let minutes_left = minutes_until(blocked.resets_at_ms?, now);
    (minutes_left as f64 > settings.five_hour_minutes).then_some(Reason::FiveHour { minutes_left })
}

pub fn decide(settings: &AutoReset, usage: &LiveUsage, now: i64) -> Verdict {
    let Some(reason) = trigger(settings, usage, now) else { return Verdict::Idle };
    if usage.available_resets <= 0 {
        return Verdict::Skip("Limit reached, but no resets are available.".into());
    }
    // A reset restarts the weekly clock, so one used just before a free weekly reset is mostly wasted.
    let weekly_resets_in = usage.weekly().and_then(|w| w.resets_at_ms).map(|at| minutes_until(at, now));
    if let Some(minutes) = weekly_resets_in.filter(|&m| (m as f64) < settings.skip_within_hours * 60.0) {
        return Verdict::Skip(format!("Saving your reset: the weekly limit resets on its own in {}.", duration(minutes)));
    }
    Verdict::Reset(reason)
}

struct Pending {
    key: String,
    credit_id: String,
    reason: Reason,
    attempts: u8,
}

/// Per-run memory: fire once per crossing, and retry a failed request with the same key.
#[derive(Default)]
pub struct AutoState {
    disarmed: bool,
    pending: Option<Pending>,
}

pub struct Cycle {
    pub usage: LiveUsage,
    pub credits: Vec<ResetCredit>,
    pub event: Option<ResetEvent>,
    pub status: Option<String>,
    pub retry_soon: bool,
}

pub async fn cycle(api: &CodexApi, auth: &Auth, settings: &AutoReset, state: &mut AutoState) -> Result<Cycle, ApiError> {
    let mut usage = api.usage(auth).await?;
    let mut event = None;
    let mut status = None;

    if let Some(pending) = state.pending.take() {
        event = Some(redeem(api, auth, state, pending).await);
    } else if !state.disarmed {
        match decide(settings, &usage, now_ms()) {
            Verdict::Idle => {}
            Verdict::Skip(reason) => status = Some(reason),
            Verdict::Reset(reason) => event = Some(start(api, auth, state, reason).await),
        }
    }

    if event.as_ref().is_some_and(|e| e.ok) {
        if let Ok(fresh) = api.usage(auth).await {
            usage = fresh;
        }
    }
    if trigger(settings, &usage, now_ms()).is_none() {
        state.disarmed = false;
    }
    let credits = if usage.available_resets > 0 { api.reset_credits(auth).await.unwrap_or_default() } else { Vec::new() };
    Ok(Cycle { usage, credits, event, status, retry_soon: state.pending.is_some() })
}

async fn start(api: &CodexApi, auth: &Auth, state: &mut AutoState, reason: Reason) -> ResetEvent {
    let credits = match api.reset_credits(auth).await {
        Ok(credits) => credits,
        Err(error) => return ResetEvent::failed(format!("Couldn't list your resets: {}", error.message)),
    };
    let Some(credit) = earliest_expiring(&credits) else {
        state.disarmed = true;
        return ResetEvent::failed("Limit reached, but no usable reset was found.");
    };
    let pending = Pending { key: uuid::Uuid::new_v4().to_string(), credit_id: credit.id.clone(), reason, attempts: 0 };
    redeem(api, auth, state, pending).await
}

async fn redeem(api: &CodexApi, auth: &Auth, state: &mut AutoState, mut pending: Pending) -> ResetEvent {
    pending.attempts += 1;
    let result = api.consume(auth, &pending.key, &pending.credit_id).await;
    match &result {
        Err(error) if error.retryable && pending.attempts < MAX_ATTEMPTS => {
            let message = format!("{} Retrying in a minute.", error.message);
            state.pending = Some(pending);
            return ResetEvent::failed(message);
        }
        _ => state.disarmed = true,
    }
    match result {
        Ok(ConsumeOutcome::Reset | ConsumeOutcome::AlreadyRedeemed) => {
            ResetEvent::succeeded(format!("Auto-used a reset: {}.", pending.reason.describe()))
        }
        Ok(ConsumeOutcome::NothingToReset) => ResetEvent::failed("OpenAI said no reset was needed yet."),
        Ok(ConsumeOutcome::NoCredit) => ResetEvent::failed("That reset is no longer available."),
        Ok(ConsumeOutcome::Unknown) => ResetEvent::failed("OpenAI gave an unexpected answer, so the app stopped."),
        Err(error) => ResetEvent::failed(format!("Couldn't use a reset: {}", error.message)),
    }
}

/// The "Use reset" button: same safeguards on OpenAI's side, no threshold on ours.
pub async fn use_now(api: &CodexApi, auth: &Auth) -> ResetEvent {
    let credits = match api.reset_credits(auth).await {
        Ok(credits) => credits,
        Err(error) => return ResetEvent::failed(format!("Couldn't list your resets: {}", error.message)),
    };
    let Some(credit) = earliest_expiring(&credits) else { return ResetEvent::failed("No resets are available.") };
    let key = uuid::Uuid::new_v4().to_string();
    match api.consume(auth, &key, &credit.id).await {
        Ok(ConsumeOutcome::Reset | ConsumeOutcome::AlreadyRedeemed) => ResetEvent::succeeded("Used a reset."),
        Ok(ConsumeOutcome::NothingToReset) => ResetEvent::failed("OpenAI said your usage doesn't need a reset yet."),
        Ok(ConsumeOutcome::NoCredit) => ResetEvent::failed("That reset is no longer available."),
        Ok(ConsumeOutcome::Unknown) => ResetEvent::failed("OpenAI gave an unexpected answer."),
        Err(error) => ResetEvent::failed(format!("Couldn't use a reset: {}", error.message)),
    }
}

fn minutes_until(at_ms: i64, now: i64) -> i64 {
    (at_ms - now).div_euclid(MINUTE_MS)
}

fn duration(minutes: i64) -> String {
    match minutes.max(0) {
        m if m < 60 => format!("{m}m"),
        m if m < 48 * 60 => format!("{}h {}m", m / 60, m % 60),
        m => format!("{} days", m / (24 * 60)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_api::tests::{auth, fake_server};
    use crate::codex_api::Window;

    const NOW: i64 = 1_000_000 * MINUTE_MS;
    const HOUR: i64 = 60 * MINUTE_MS;

    fn usage(five_hour: f64, five_hour_reset_in: i64, weekly: f64, weekly_reset_in: i64, resets: i64) -> LiveUsage {
        LiveUsage {
            plan: Some("Plus".into()),
            windows: vec![
                Window { used_percent: five_hour, length_secs: 5 * 3600, resets_at_ms: Some(NOW + five_hour_reset_in) },
                Window { used_percent: weekly, length_secs: 7 * 24 * 3600, resets_at_ms: Some(NOW + weekly_reset_in) },
            ],
            available_resets: resets,
        }
    }

    fn on() -> AutoReset {
        AutoReset { weekly_enabled: true, ..AutoReset::default() }
    }

    #[test]
    fn fires_when_weekly_usage_reaches_the_threshold() {
        assert_eq!(decide(&on(), &usage(10.0, HOUR, 95.0, 72 * HOUR, 1), NOW), Verdict::Reset(Reason::Weekly { percent: 95.0 }));
        assert_eq!(decide(&on(), &usage(10.0, HOUR, 94.0, 72 * HOUR, 1), NOW), Verdict::Idle);
    }

    #[test]
    fn does_nothing_while_switched_off() {
        assert_eq!(decide(&AutoReset::default(), &usage(100.0, 3 * HOUR, 99.0, 72 * HOUR, 1), NOW), Verdict::Idle);
    }

    #[test]
    fn saves_the_reset_when_the_weekly_limit_resets_soon_anyway() {
        let verdict = decide(&on(), &usage(10.0, HOUR, 97.0, 20 * HOUR, 1), NOW);
        assert_eq!(verdict, Verdict::Skip("Saving your reset: the weekly limit resets on its own in 20h 0m.".into()));
    }

    #[test]
    fn needs_a_reset_to_spend() {
        assert!(matches!(decide(&on(), &usage(10.0, HOUR, 99.0, 72 * HOUR, 0), NOW), Verdict::Skip(_)));
    }

    #[test]
    fn five_hour_trigger_needs_a_long_block() {
        let settings = AutoReset { five_hour_enabled: true, ..AutoReset::default() };
        assert_eq!(
            decide(&settings, &usage(100.0, 3 * HOUR, 50.0, 72 * HOUR, 1), NOW),
            Verdict::Reset(Reason::FiveHour { minutes_left: 180 })
        );
        assert_eq!(decide(&settings, &usage(100.0, 30 * MINUTE_MS, 50.0, 72 * HOUR, 1), NOW), Verdict::Idle);
        assert_eq!(decide(&settings, &usage(99.0, 3 * HOUR, 50.0, 72 * HOUR, 1), NOW), Verdict::Idle);
    }

    #[test]
    fn describes_durations_plainly() {
        assert_eq!(duration(45), "45m");
        assert_eq!(duration(130), "2h 10m");
        assert_eq!(duration(5 * 24 * 60), "5 days");
    }

    #[tokio::test]
    async fn a_non_retryable_failure_disarms_until_usage_drops() {
        let (base, _) = fake_server(400, "{}");
        let api = CodexApi::new(reqwest::Client::new(), base);
        let mut state = AutoState::default();
        let pending = Pending { key: "k".into(), credit_id: "c".into(), reason: Reason::Weekly { percent: 96.0 }, attempts: 0 };

        let event = redeem(&api, &auth(), &mut state, pending).await;
        assert!(!event.ok);
        assert!(state.disarmed && state.pending.is_none());
    }

    #[tokio::test]
    async fn a_dropped_connection_keeps_the_same_key_for_the_retry() {
        let api = CodexApi::new(reqwest::Client::new(), "http://127.0.0.1:9");
        let mut state = AutoState::default();
        let pending = Pending { key: "same-key".into(), credit_id: "c".into(), reason: Reason::Weekly { percent: 96.0 }, attempts: 0 };

        let event = redeem(&api, &auth(), &mut state, pending).await;
        assert!(event.message.contains("Retrying"));
        let kept = state.pending.as_ref().unwrap();
        assert_eq!((kept.key.as_str(), kept.attempts), ("same-key", 1));
    }
}
