# Codex Usage Tray

Codex Usage Tray is a Windows tray utility for monitoring the usage limits of the account signed in to the Codex CLI. The tray icon shows the selected remaining-usage percentage, and a left-click opens a compact popup with weekly and 5-hour remaining usage, estimated time until depletion, and reset timing.

![Codex Usage tray popup showing time remaining at current pace and available resets](docs/images/tray-usage-details.png)

_Usage values and reset dates shown above are sample data._

## Requirements

- Windows.
- A Windows-accessible Codex CLI installation. Confirm it is available with `codex --version`.
- A Codex account signed in through the CLI for the same Windows user that runs the tray app. Run `codex login` before starting the app if needed.

The app looks for the standard npm Codex installation and then for `codex` on the Windows `PATH`. It does not provide a separate sign-in flow.

## Download

Download the latest `CodexUsageTray.exe` from the [GitHub releases page](https://github.com/jeremygold02/codex-usage-tray-icon/releases/latest).

## How usage is read

Each refresh starts a short-lived local `codex app-server --stdio` process and reads the signed-in account's rate-limit data directly. The tray app does not submit a prompt or make a synthetic model request to obtain usage data.

## Optional Claude support

Sign in once with the native Windows Claude CLI (`claude auth login`). The tray app reuses that subscription login to fetch account quota directly, including usage shared with Claude Desktop on the same account. Claude does not need to stay open, run prompts, or execute a status-line command. The tray icon and notifications continue to represent Codex.

The app reads `%USERPROFILE%\.claude\.credentials.json` (or the directory set by `CLAUDE_CONFIG_DIR`) and polls every five minutes. Like Codex, manual refresh is available again as soon as the current request finishes. Automatic retries after failed requests wait one minute, but manual refresh can retry immediately. Actual server rate-limit responses pause both kinds of refresh for at least five minutes and honor longer server cooldowns. Expiring OAuth tokens are refreshed and saved back to the CLI credential file, preserving its other fields and file permissions. Tokens are sent only to Anthropic's fixed HTTPS endpoints and are never included in usage history or diagnostics. A revoked login may require signing in again.

This follows [OpenQuota's Claude integration](https://github.com/deviffyy/OpenQuota/blob/main/src-tauri/src/providers/claude/client.rs). The subscription endpoints are not a stable public API, so future changes may require an app update.

An existing Claude Code **v2.1.251 or later** [status-line collector](https://code.claude.com/docs/en/statusline) can remain as an optional fallback. It is no longer required. Example:

```json
{
  "statusLine": {
    "type": "command",
    "command": "\"C:/Tools/CodexUsageTray.exe\" --claude-statusline"
  }
}
```

This optional command captures usage silently. If you already have a status-line command, preserve it and use a wrapper that passes the same stdin to both commands instead of replacing it. The tray app does not edit Claude settings.

- Direct polling does not send prompts or consume model tokens. The optional status-line fallback receives `rate_limits` only after a normal CLI API response.
- When data becomes available, click the provider name in the popup header to switch between **Codex** and **Claude**. The chart button opens the selected provider's history. A **Claude usage history** tray-menu item also appears.
- Claude refresh shows a spinner and **Refreshing...**, then confirms a successful manual update. Clicking during a server rate-limit cooldown explains the rate limit and shows when another refresh is available. Both Claude and Codex bars display remaining allowance with the same percentage formatting.
- Claude uses the same pace estimates and charts, with separate history in `claude-usage-history.json`. Only quota percentages, reset times, observation timestamps, and reset-adjustment metadata are saved in `claude-usage.json` and history.
- The last successful account response is cached in `claude-account-usage.json`; the optional status-line cache is `claude-usage.json`. Failures retain unexpired cached readings with a stale-data notice and suppress forecasts. Reset windows expire independently. Without CLI credentials or any usable reading, Claude remains hidden. A new account request runs on tray startup regardless of Codex activity.

Claude Desktop and CLI share account allowances, but **Desktop sign-in alone does not enable this integration**: sign in through the CLI once to supply the OAuth credentials. Subsequent quota updates are independent of CLI activity. Historical token logs do not reconstruct past quota percentages; the quota chart accumulates actual readings collected by the tray app.

## Popup and details

The popup shows each primary weekly or 5-hour limit returned by Codex. Temporarily unavailable windows are hidden; if neither window is returned, the refresh fails with a clear status instead of displaying unknown values. A status line also appears while refreshing or when automatic checks are paused.

Available banked resets are always listed directly below usage, with their count, titles, and local expiration dates when supplied by Codex. There is no collapse control or setting to hide them. The section is absent when no resets are available. Settings can still hide usage reset times or last-updated times.

## Estimates and history

- Each usage bar shows the estimated depletion date and time with a duration in parentheses, using the same local date/time and duration format as **Next reset**, such as **Estimated: Oct 9, 1:00 PM (3d 3h)** or **Estimated: Oct 6, 4:45 PM (1h 45m)**. The popup and chart footer use the same estimate. The date comes from the precise calculated depletion time; the duration uses days and hours for a day or longer, hours and minutes for shorter estimates, and minutes or seconds below an hour. A steady 20% per day with 80% remaining gives a depletion date four days ahead. The 5-hour estimate uses observations within the last hour; the weekly estimate uses up to 48 hours. Newer intervals carry more weight: their influence halves every 15 minutes for the 5-hour limit and every six hours for the weekly limit. This responds to both increasing and decreasing usage while retaining idle time; refreshing more often does not give those intervals extra weight. Estimates assume that this recent pace continues.
- Recent-trend estimates need at least three observations spanning 15 minutes for the 5-hour window or six hours for the weekly window. Before that, the app uses the current window's percentage used divided by elapsed time since its inferred start (`reset time - window duration`). For example, 20% used one day into a weekly window gives approximately four days (96 hours) remaining immediately. This initial pacing follows the approach in [OpenQuota](https://github.com/deviffyy/OpenQuota/blob/main/src-tauri/src/pacing.rs).
- Codex and Claude share the same history, forecast, and chart logic, with separate readings for each provider. Weekly forecasts compare a recent-session estimate (up to six hours, one-hour weight half-life, at least three readings over 30 minutes), a 24-hour average, an hourly daily pattern, and an inactivity-aware recent-session estimate. Session estimates exclude monitoring gaps over two hours. The daily pattern averages the same hours across three or more observed days within four weeks, weighting newer days more heavily, and scales to the latest day's consumption when coverage is sufficient.
- The inactivity-aware candidate learns weekday and weekend routines separately in local time. An hour must be flat on its two most recent well-observed days of that kind, within a repeated quiet period of at least three hours. It keeps the recent working pace for other hours and can reuse the last observed working session after sleep or a weekend. Activity during a usual quiet period overrides the first projected hour. Unsupported hours and long positive gaps are never assumed idle. These are empirical usage patterns; no fixed sleep schedule is supplied.
- Model selection uses [rolling-origin evaluation](https://otexts.com/fpp3/tscv.html): predict later readings using only information available at each earlier timestamp. It compares usage-rate errors across 1-, 6-, and 24-hour horizons over the latest seven days, gives newer checks more weight with a three-day half-life, requires at least eight valid checks spanning a day, and switches only for an error reduction of at least 10%. Reset and plan boundaries are excluded. Daily patterns use [seasonal forecasting](https://otexts.com/fpp3/forecasting-decomposition.html); otherwise the selected estimate stays linear. These are estimates of continued behavior, not guaranteed depletion times.
- The cycle-average estimate waits until at least 1% of the window (and at least one minute) has elapsed, needs known window/reset timing, and is suppressed after an observed usage decrease within the same reset window. When neither method has enough information, the popup shows **Learning usage pace...**. Recent observed trends take precedence once available.
- When the allowance is projected to last until or beyond its scheduled reset, the estimate shows **Will last until reset**. Otherwise, it shows the estimated time remaining. This applies to both recent trends and cycle averages. No observed increase is reported explicitly rather than inventing a depletion time.
- Failed, paused, or more than 30-minute-old readings do not produce current estimates. Observed usage decreases or changed reset deadlines start a new forecast segment. Weekly estimates can span overnight gaps only when both observations share a known reset deadline.
- Open **Usage history** from the tray menu or the chart icon beside Refresh. Each chart starts at the first recorded reading after its latest reset and advances automatically when a new reset is observed. Prior cycles remain available for learning but are hidden from the chart. The 5-hour option appears only when the account reports a 5-hour limit. There is no Clear history button. Move or drag across the recorded chart to snap a vertical guide to the nearest percentage change. Repeated unchanged polls become a flat segment, retaining its start and latest time. The footer shows the reading's local timestamp and usage percentages; leaving the chart restores the latest reading. Arrow keys also select percentage changes.
- Readings are connected with a blue line. Dashed segments span gaps over two hours; they connect observations without implying measurements were taken in between. Reset or usage-adjustment boundaries remain disconnected and have vertical dotted markers at the observation time. Only endpoints, resets, and the selected reading are emphasized with point markers.
- The time axis extends to the current window's reset deadline whenever it is known, with local calendar/time ticks and labels aligned to their ticks. When a valid estimate is available, an amber dashed **Projected** path extends from the latest recorded reading through the reset. If it reaches zero earlier, it stays flat at zero until the reset; the forecast text still reports the actual estimated depletion time. Validated hourly patterns vary their slope, including flat inactive periods; pace estimates remain straight. Hover or drag over the entire projected period to inspect the estimated time and whole-number percentages, explicitly labeled **Projected**. Remaining is rounded once and used is its complement, so they always add to 100%; the underlying path and depletion calculations retain full precision. Refreshing keeps the last fresh projection visible until the new reading replaces it. Stale or insufficient data hides the projection.
- Successful quota readings are saved locally in `%APPDATA%\CodexUsageTray\usage-history.json` and `claude-usage-history.json`, with the same rolling caps of 28 days and 45,000 observations. Repeated unchanged readings are compacted while preserving endpoints and hourly anchors. Patterns can learn across reset cycles and survive restart; interval consumption and validation never cross a reset. Older readings are removed automatically. Claude's CLI token logs cannot reconstruct quota percentages, so its learning uses actual quota readings collected by the tray.
- After the first successful Codex refresh, a one-time background import reads up to four weeks of account-quota events from local `sessions` and `archived_sessions` under `CODEX_HOME` (or `%USERPROFILE%\.codex`). Prior reset cycles are included. Cached regressive events and events contradicting live readings are excluded rather than interpreted as resets; token counts are never converted into quota percentages. Import scans recent files first, with a 30-second or 8-GiB budget per attempt; interrupted scans resume on a later refresh and only reread unfinished/appended data. Completion and retained imported days are stored in `usage-history.json.imported`, so normal launches with intact history do not rescan. If an older app removed the backfilled cycles, the missing coverage triggers a repair import. Normal four-week expiry does not trigger a scan. Existing installations receive one repair backfill when upgrading from completion flags that did not record coverage. Afterward, normal usage refreshes update the local learning window for both providers. History belongs to the Windows user and is not separated by account. Source logs remain untouched.

![Weekly usage history with a reset marker and an observation gap](docs/images/usage-history.png)

_The history chart above uses sample data._

## Refresh and settings

- Use the popup refresh icon or **Refresh now** in the tray menu for an immediate check. The refresh control is disabled while a check is already running.
- Automatic refresh defaults to every 300 seconds while Codex is running and can be set from 30 to 3600 seconds.
- Idle refresh defaults to off. When no Codex process is running, automatic checks pause unless an idle interval is configured; manual refresh remains available.
- A failed refresh keeps the last successful values visible and marks them as stale instead of replacing them with empty data.
- The first successful refresh that observes a weekly or 5-hour limit return to 100% shows one reset notification. Startup-at-100, stale refreshes, and brief 99%-to-100% jitter do not produce duplicate alerts.
- The app caches the last observed banked resets in `%APPDATA%\\CodexUsageTray\\banked-reset-state.json` and shows a tray notification whenever a new one becomes available, including resets added while the app was closed. The first refresh without an existing cache establishes a baseline quietly.
- **Reset expiry reminders** defaults to 24 hours before expiration and can be disabled or set from 1 to 72 hours. Each itemized credit with a known expiration produces one reminder, independent of **Auto-use resets**. The app checks once at startup and revalidates known credits when their reminder becomes due, including while idle checks are paused. Notification state is saved in `reset-reminders.json` to avoid repeats after restart. The app must be running and able to refresh to deliver a reminder.
- **Tray icon shows → Auto** selects whichever available limit has the lower remaining percentage, preferring weekly on a tie. The tooltip identifies the selected limit. Explicit Weekly and 5-hour choices remain available.
- **Auto-use resets** is opt-in and disabled by default. When enabled, the app can redeem an itemized Codex reset credit shortly before expiration, defaulting to five minutes, only when there is usage to reset. Redemption attempts use persisted idempotency state to prevent duplicates.
- Open Settings from the popup gear or tray menu. Changes are saved only when **OK** is selected and immediately update the icon, popup, notifications, startup behavior, and refresh schedule as applicable.
- Settings also provides theme and tray appearance controls, threshold notifications, popup visibility options, Windows startup, and update checks.

![Codex Usage Tray settings with grouped usage, refresh, appearance, popup, and update controls](docs/images/settings.png)

## Troubleshooting

- **Codex CLI was not found:** verify `codex --version` in Windows, install or add the CLI to `PATH`, then restart the tray app so it inherits the updated environment.
- **Codex is not signed in:** run `codex login` as the same Windows user, then refresh again.
- **App-server errors or timeouts:** update the Codex CLI, confirm it starts normally, and retry from the popup. The last successful snapshot remains visible after a failed refresh.
- **No weekly or 5-hour limits returned:** retry the refresh and update the Codex CLI if the error persists. The app treats auxiliary model limits as supplemental data, not a replacement for the account's primary limits.
- **Waiting for Codex / checks paused:** start Codex, use manual refresh, or configure an idle refresh interval in Settings.
- **Reset expiration details are missing:** those rows appear only when Codex returns the corresponding account data and the related popup option is enabled.

## Privacy

Codex credentials remain managed by the Codex CLI; the tray app does not copy or store them. Usage history stores local timestamps, usage percentages, reset timing, and import/reset-adjustment metadata. Preferences, history, and notification state are saved locally under `%APPDATA%\CodexUsageTray`; history contains no prompts or conversation content.

The Codex CLI contacts OpenAI for authenticated rate-limit reads and, when **Auto-use resets** is enabled, supported reset-credit redemption requests. The tray app also contacts GitHub to check for releases and, when requested, download an update. It has no separate telemetry or analytics path and does not send prompts or synthetic model requests.

## Attribution

Codex Usage Tray is inspired by the tray display style of [Bluetooth Battery Monitor](https://www.bluetoothgoodies.com/) from Luculent Systems, LLC.

Bluetooth Battery Monitor is © 2017-2026 Luculent Systems, LLC. All Rights Reserved.
