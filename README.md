# AI Usage Monitor

A Windows 11 tray app that shows how close you are to your AI plan limits.
Left-click the tray ring for the popover; right-click for Refresh, Start with Windows, and Quit.

The tray ring fills with your highest Claude limit and turns amber at 75%, red at 90%.
Hover it for a summary, e.g. `Claude: 5h 43% (2h 34m left) · Week 85% · Fable 91%`; the time left is recomputed on hover.

## Where the numbers come from

| Meter | Source |
|---|---|
| Claude (5-hour, weekly, per-model weekly) | `GET https://api.anthropic.com/api/oauth/usage`, authorised with the login Claude Code already stores in `~/.claude/.credentials.json`. The token is read on each check and never stored or refreshed by this app. |
| Codex (ChatGPT account) | The newest `rate_limits` record Codex CLI writes to `~/.codex/sessions/**/*.jsonl`. Only as fresh as your last Codex session. |

The Claude endpoint is undocumented (it is what Claude Code's `/usage` screen uses), so it can change without notice.
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

## Build

Needs Rust (MSVC toolchain), the Visual Studio C++ build tools, and Node.

```
npm install
npm run dev      # run with a console
npm run build    # installer in src-tauri/target/release/bundle/nsis/
```

## Not done yet

- GitHub Copilot monthly usage (needs the GitHub API and the right account).
- ChatGPT web-chat limits: there is no API for them.
