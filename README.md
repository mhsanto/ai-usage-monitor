# AI Usage Monitor

A Windows 11 tray app that shows how close you are to your AI plan limits.
Left-click the tray ring for the popover; right-click for Refresh, Start with Windows, and Quit.

The tray ring fills with your highest Claude limit and turns amber at 75%, red at 90%.
Hover it for a summary, e.g. `Claude: 5h 43% (2h 34m left) · Week 85% · Fable 91%`; the time left is recomputed on hover.

## Where the numbers come from

| Meter | Source |
|---|---|
| Claude (5-hour, weekly, per-model weekly) | `GET https://api.anthropic.com/api/oauth/usage`, authorised with the login Claude Code already stores in `~/.claude/.credentials.json`. The token is read on each check and never stored or refreshed by this app. |
| Codex (ChatGPT account) | `GET https://chatgpt.com/backend-api/wham/usage`, authorised with the login Codex stores in `~/.codex/auth.json` (read per check, never stored or refreshed). Without a working login it falls back to the newest `rate_limits` record in `~/.codex/sessions/**/*.jsonl`, which is only as fresh as your last Codex session. |

Both endpoints are undocumented (they are what Claude Code's `/usage` screen and the official Codex app use), so they can change without notice.
If it does, the popover says so instead of showing wrong numbers.

## When it checks

- Every 10 minutes.
- After Claude Code activity (it watches `~/.claude` for log writes), at most once every 5 minutes.
- When you open the popover, if the last check is over 2 minutes old.
- When you press refresh, at most once a minute.

The usage endpoint rate-limits hard (about a dozen calls in an hour was enough to get `429`), so these gaps are deliberately wide.
On `429` it backs off for 15 minutes and keeps showing the last good numbers.

Idle, it does nothing: no window, no WebView2, no timers besides the 10-minute check.
Launching the app a second time opens the popover.

## Codex auto-reset

Paid ChatGPT plans sometimes get *banked resets*: one-time credits that refill your Codex 5-hour and weekly limits.
The Codex card shows how many you have, with a **Use reset** button (it asks first).
The **Codex auto-reset** card can spend one for you. It is off until you tick a box.

| Setting | Default |
|---|---|
| Use a reset when weekly usage reaches | 95% |
| …but not if the weekly limit resets on its own within | 24 h |
| Also when the 5-hour window blocks me for more than | 60 min (off) |

Rules it always follows:

- It only spends resets you already own. The request cannot buy one; with none left, OpenAI answers `no_credit`.
- It uses the reset that expires soonest, the same choice the official Codex app makes.
- It fires once per crossing. It re-arms only after usage drops back below the threshold.
- A failed request is retried at most twice, with the same `redeem_request_id`, so OpenAI can never apply it twice.
- A reset restarts your weekly clock, which is why it skips when a free weekly reset is close.

It uses the same three requests as the official Codex app: `GET /wham/usage`, `GET /wham/rate-limit-reset-credits`, and `POST /wham/rate-limit-reset-credits/consume`.

## Build

Every push builds on GitHub Actions (`.github/workflows/build.yml`): tests, lint, then the installer and portable exe as a downloadable artifact.
To build locally instead:

You need Rust (MSVC toolchain), the Visual Studio C++ build tools, and Node.

```
npm install
npm run dev      # run with a console
npm run build    # installer in src-tauri/target/release/bundle/nsis/
```

## Not done yet

- GitHub Copilot monthly usage (needs the GitHub API and the right account).
- ChatGPT web-chat limits: there is no API for them.
