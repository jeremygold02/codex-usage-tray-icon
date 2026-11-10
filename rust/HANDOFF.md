# Handoff: finishing the Rust port of Codex Usage Tray

> **Status update, 2026-11-10:** Sections 4.1 and 4.2 are implemented and
> wired into the tray. The chart and importer were checked with synthetic
> data, and the full test suite, formatting, and Clippy checks pass. HTTP-date
> `Retry-After` support and the MSVC executable icon from section 4.3 are
> also implemented. The other non-blocking gaps listed there remain; GNU
> builds do not yet embed the icon. The rest of this document records the
> original handoff and its earlier state, not the current implementation.
> Build the current release with `cargo build --release` from `rust/`.

This folder (`rust/`) is a from-scratch Rust rewrite of the C# tray app in the
repository root (`src/`, `wpf/`, `tests/`). Most of it is done, tested and
running. Two features are unfinished and exist only as drafts in `rust/wip/`.
This document says what is finished, what is left, and how to verify your work.

Read the repository `README.md` first: it specifies the behaviour being ported.

## 1. State of the crate

```
cargo test   --manifest-path rust/Cargo.toml     # 338 passed, 4 ignored (sample writers and a live fetch)
cargo clippy --manifest-path rust/Cargo.toml --all-targets
cargo build  --manifest-path rust/Cargo.toml --release
```

- Builds with no errors. Clippy reports only `dead_code`, and every remaining
  one is API that the two unfinished features will call (see section 5).
- Output is one executable, `rust/target/release/usage-tray-rs.exe` (about
  500 KB), with no runtime dependency.
- Nothing is committed. `rust/` is untracked; the other modified files in
  `git status` are the owner's own uncommitted C# work and must not be touched.
- A release build may be running on this machine. Stop it before building the
  release binary, because the running exe is locked:
  `rust/target/release/usage-tray-rs.exe --exit`

### Done and wired in

| Area | Rust modules | Ported from |
|---|---|---|
| Tray icon, tooltip, menu, refresh schedule, state | `tray.rs`, `main.rs` | `TrayAppContext.cs`, `CodexUsageTray.cs`, `CodexActivityMonitor.cs` |
| Icon rendering (pixel-identical to the C#) | `gfx.rs` | `IconRenderer.cs` |
| Popup (own process) | `popup.rs`, `render.rs`, `view.rs` | `UsagePopup.cs`, `wpf/Popup/UsagePopupView.cs` |
| Codex rate limits and reset redemption request | `codex.rs`, `model.rs` | `CodexRateLimitClient.cs` |
| Claude quota with token refresh, caches, `--claude-statusline` | `claude.rs`, `claude/` | `ClaudeQuotaClient.cs`, `ClaudeUsageSource.cs` |
| History store, forecasting, 7-day summary | `history.rs`, `history_file.rs`, `history_view.rs`, `forecast.rs`, `pattern.rs`, `localtime.rs` | `UsageHistory.cs`, `UsagePatternForecast.cs` |
| Reset notifications, thresholds, reminders, auto-redeem | `resets.rs`, `resets/` | `UsageResetDetector.cs`, `ResetExpiryReminder.cs`, `AutomaticResetRedemptionPolicy.cs`, parts of `TrayAppContext.cs` |
| Settings model and Settings window (own process) | `settings.rs`, `settings_view.rs`, `settings_window.rs`, `controls.rs` | `AppSettings.cs`, `SettingsForm.cs`, `wpf/Settings/*` |
| Self-update, version, start with Windows | `update.rs`, `version.rs`, `startup.rs` | `UpdateService.cs`, `AppVersion.cs`, `StartupManager.cs` |
| HTTPS client, paths, time formatting | `http.rs`, `paths.rs`, `timefmt.rs`, `win.rs` | (new) |

### Not done

1. **Usage history chart window** (`UsageHistoryForm.cs`, `wpf/History/*`). Drafts in `rust/wip/history-chart/`.
2. **Import of past readings from Codex session logs** (`CodexHistoryImporter.cs`). Drafts in `rust/wip/log-importer/`.

Until the chart exists, the popup's history button does nothing (the popup
posts `EVENT_OPEN_HISTORY`, which `tray.rs` ignores) and the tray menu has no
"Usage history" item.

## 2. Architecture you must keep

One executable, several process roles chosen by the first argument (`main.rs`):

| Start as | Role | Lifetime |
|---|---|---|
| (no argument) | Tray: icon, timers, all state, all network and file writes | Long-lived, single instance |
| `--popup <tray hwnd> <l> <t> <r> <b>` | Usage popup | Only while open |
| `--settings <tray hwnd> <settings path>` | Settings window | Only while open |
| `--claude-statusline` | Captures Claude status-line input and exits | Momentary |
| `--exit` | Asks the running tray to exit | Momentary |

The owner asked for this split to keep idle memory low (tray is about 6 MB
private; each window process about 4 MB while open). The chart must be a
further role, `--history`, built the same way.

- **IPC** (`ipc.rs`): windows post `WM_*_EVENT` messages to the tray window;
  the tray answers with `WM_COPYDATA` carrying JSON. The tray verifies that a
  window announcing itself belongs to the process it started.
- **State** lives in a thread-local `App` in `tray.rs`, borrowed only for
  short sections through `with_app`. Never hold that borrow across a call
  that can dispatch window messages (showing or destroying windows, menus,
  cross-process sends). The header comment in `tray.rs` explains why.
- **Workers**: slow work runs on `std::thread` and returns through
  `post_boxed(window, WM_..., value)`. Tray-private messages are `WM_APP + 1`,
  `+ 2` and `+ 20..=23`; `ipc.rs` uses `+ 3`, `+ 0x20` (and the chart draft
  `+ 0x30`).
- **Data directory**: `%APPDATA%\CodexUsageTrayRs` (`paths.rs`), separate from
  the C# app's `%APPDATA%\CodexUsageTray`. On first run the C# app's files are
  copied in, with "Start with Windows" and "Auto-use resets" switched off in
  the copy. File formats match the C# app's.

## 3. Rules

- The executable name must not start with `codex`: both apps treat any such
  process as a running Codex CLI.
- Never write under `%APPDATA%\CodexUsageTray`, never use the C# app's
  `CodexUsageTray` Run-key value, single-instance mutex or window classes.
- Dependencies are `serde_json` and `windows-sys` 0.61 only. Add `windows-sys`
  features as needed; no other crates.
- Never print, log or store Claude or Codex tokens. Never read the real
  `.credentials.json` or real Codex session logs in tests; build fixtures by
  hand.
- Logic takes `now` and file paths as parameters so tests are deterministic.
  Tests use uniquely named folders under the temp directory and remove them.
- No `unwrap`/`expect`/panics on runtime data outside tests. Every `unsafe`
  block has a `// SAFETY:` comment. `cargo fmt` before finishing.
- **A constant used as a `match` pattern must be imported.** An unimported
  name compiles as a catch-all binding and silently matches everything. After
  each build, check for "should have a snake case name" warnings.
- Stay faithful to the C#: same text, thresholds and rules. Port the C# tests
  for whatever you port.

## 4. Remaining work

### 4.1 Usage history chart window

Specification: `src/UsageHistoryForm.cs`, `wpf/History/HistoryHeader.xaml(.cs)`,
`tests/UsageHistoryForm.Tests.cs`, `docs/images/usage-history.png`, and in
`README.md` the "Estimates and history" bullets from "Open **Usage history**".

Drafts in `rust/wip/history-chart/` (written against the current sources,
never run):

| File | What it is | State |
|---|---|---|
| `chart_data.rs` | The JSON payload the tray sends: `ChartData::build(store, usage, fresh, settings, now)`, `to_json`, `from_json`, `ChartProvider` | Compiles. One test does not: it builds a `Settings` with a struct literal, but `Settings` has a private field. Mutate `Settings::default()` instead. Tests never run. |
| `chart_view.rs` | Layout, painting and input for the window (`View`: `set_data`, `render`, pointer and key handlers) | Compiles. No tests. Never rendered or looked at. |
| `gfx.rs` | The current `src/gfx.rs` plus `Dash`, `Graphics::dashed_polyline`, `Graphics::fill_polygon` | Drop-in replacement; only additions and a reflowed import list. |
| `ipc.rs` | The current `src/ipc.rs` plus `HISTORY_ARGUMENT`, `WM_HISTORY_EVENT`, event and provider codes, `HISTORY_DATA`, `MAX_HISTORY_DATA_BYTES` | Drop-in replacement; additions only. |

To finish:

1. Copy the four files into `rust/src/`, add `mod chart_data; mod chart_view;`
   to `main.rs`, fix the one test, and get `cargo test` green.
2. Write `chart_window.rs`: the `--history <tray hwnd> <provider>` process.
   Model it on `settings_window.rs` (window class, thread-local state, DPI,
   double-buffered paint, `WM_COPYDATA` in, events out, single window per
   provider). Add the role to `main.rs`.
3. Wire the tray (`tray.rs`), following how Settings is wired
   (`open_settings`, `send_settings_status`, `handle_settings_event`):
   - one chart process per provider; opening an open one raises it;
   - on ready, and whenever a reading arrives or settings change, send
     `ChartData::build(...)` for that provider;
   - handle `EVENT_OPEN_HISTORY` from the popup (open the chart for the
     provider the popup shows);
   - add menu items "Usage history" and, when Claude data exists, "Claude
     usage history", in the C# order (`TrayAppContext.cs` around line 250);
   - close chart windows on exit and before installing an update.
4. Tests: the UI-independent assertions from `tests/UsageHistoryForm.Tests.cs`
   that `history_view.rs` does not already cover (snapping, selection, footer
   text, projected rounding, selector visibility, payload round trip and size
   bound).
5. Visual check: render to PNG with the test-only `gfx::tests::Canvas` (see
   `write_popup_samples` in `render.rs`), in both themes at 100% and 150%,
   and compare with `docs/images/usage-history.png`.

`history_view.rs` and `forecast.rs` already hold the data rules for the chart
(`ChartSeries`, `axis_ticks`, `Projection::remaining_at`, `quiet_periods`,
`WindowSample::continues`). Use them; do not re-derive.

### 4.2 Session-log importer

Specification: `src/CodexHistoryImporter.cs`, `tests/CodexHistoryImporter.Tests.cs`,
and in `README.md` the bullet beginning "After the first successful Codex
refresh, a one-time background import".

Drafts in `rust/wip/log-importer/` (never run):

| File | What it is | State |
|---|---|---|
| `importer.rs` | UI-thread half: `Session::new(codex_home)`, `request_scan(&history, &current, now) -> Option<ScanRequest>`, `scan_finished(outcome, &mut history, now) -> usize`, `cancel` | Compiles. One test error: it uses a private `tests` module of another file. |
| `importer/scan.rs` | Worker half: `scan(request, &stopwatch) -> ScanOutcome`, budgets, resume state | Compiles. One test error: borrow of a partially moved value. |
| `importer/event.rs` | Which log lines count as quota readings: `parse_line` | Compiles. One test error: borrow of a partially moved value. |

To finish:

1. Copy `importer.rs` and the `importer/` folder into `rust/src/`, add
   `mod importer;`, fix the three test errors, and get the ported tests
   passing. Confirm every scenario in the C# test file is covered.
2. Wire the tray: after the first successful Codex refresh call
   `request_scan`; run `scan` on a worker; post the outcome back with
   `post_boxed`; call `scan_finished` on the UI thread; repeat on later
   refreshes until the session reports it is complete. Cancel on exit.
3. The store side already exists: `HistoryStore::import`, `import_completed`,
   and the `.imported` companion file in `history_file.rs`. If the draft needs
   a change there, check it against `UsageHistory.cs` first.

Privacy: session logs under `%USERPROFILE%\.codex` contain the owner's prompts
and code. The importer keeps only quota percentages and timestamps. Do not
copy log content into tests, fixtures or notes.

### 4.3 After both are wired

- `cargo clippy --all-targets` must be free of `dead_code`. Delete any item
  that is still unused rather than silencing the warning.
- Smaller gaps, none of them blocking:
  - `http.rs` reads `Retry-After` only as seconds; the C# also accepts an
    HTTP date (affects Claude's rate-limit cooldown).
  - The executable has no icon resource (`icon.ico` in the repo root).
  - Windows are not exposed to screen readers and have no high-contrast mode.
  - The popup has no narrow ("compact") layout.
  - No release publishes `usage-tray-rs.exe`, so the updater always reports
    "no compatible update". Publishing that asset is a release-process task.
  - The root `README.md` does not mention the Rust build.

## 5. What has and has not been verified

Verified by running the real release build:

- Tray icon and popup with live Codex data; automatic and manual refresh.
- Icon pixels equal to the C# renderer's for nine cases
  (`rust/tools/compare-icons.ps1`, see section 6).
- Settings window opens with the saved values; Save rewrites the file, keeps a
  backup and the tray reloads.
- Update check against GitHub: reports that release 0.1.29 has no Rust asset.
- History is recorded and the popup's 7-day chart and forecast line appear.
- First-run import of the C# app's files into the Rust data directory.

Covered by tests only, never observed live:

- Any notification (limit reset, threshold, banked reset, expiry reminder).
- Auto-redeem of a reset credit. It is off in the Rust settings copy; the
  tray-side wiring (`start_redemption`, `finish_redemption` in `tray.rs`) has
  never executed. Review it before anyone enables the setting.
- Claude token refresh and credential write-back. The owner's Claude CLI
  login had expired, so Claude stayed hidden.
- Download and swap of an update, and rollback.
- Enabling "Start with Windows".
- Real mouse and keyboard use of the Settings window; DPI other than 100%.

The unused-code warnings that remain all belong to the two unfinished
features: `forecast::projection`, `Projection::remaining_at`/`quiet_periods`,
`history_view::ChartSeries` and `axis_ticks`, `history::is_reset`/
`is_continuous` and several `HistoryStore` methods (`samples`, `zone`,
`import`, `import_completed`, `projection`, `chart`, `clear`),
`history_file::write_completion`, and a few `localtime` helpers.

## 6. Verification tools

`rust/tools/*.ps1` are helper scripts (ignored by git through the root
`.gitignore`). Run them with PowerShell 7.

- `drive-tray.ps1 -Action status|click|capture|popup-event|close-popup|exit`
  drives the running tray without real input: `click` posts the same message
  the shell posts for a tray click, `capture -Out file.png` screenshots the
  popup, `popup-event -Event 6` asks the tray to open Settings.
- `capture-window.ps1 -Class UsageTrayRs.Settings -Out file.png [-Close]`
  captures any window by class even when covered.
- `compare-icons.ps1` renders icons with the C# `IconRenderer` by reflection
  and compares them with the PNGs from
  `cargo test write_icon_samples -- --ignored`. It needs Windows PowerShell
  5.1; start it with `powershell.exe -EncodedCommand` (the script takes
  `-Exe`, `-Samples`, `-Out`). Both sides go through bitmap, HICON, bitmap so
  transparent icons compare like for like.

Sample images: `cargo test --manifest-path rust/Cargo.toml -- --ignored`
writes popup and icon PNGs to `rust/target/samples/` (and runs one live Codex
fetch).

To try a build without touching the owner's Rust data, set
`USAGE_TRAY_DATA_DIR` to a scratch folder before starting it.

## 7. Suggested commits

Conventional Commits, one per feature:

- `feat(rust): add usage history chart window`
- `feat(rust): import past readings from Codex session logs`

The existing, uncommitted work in `rust/` fits
`feat(rust): port the usage tray to a native multi-process Rust app`.
