# Architecture

## Process entry and Windows subsystem

`src/main.rs` is the process entry point and declares the Windows GUI subsystem, so normal launches do not create a console window. It enables diagnostics only when `--diagnose` is present, delegates the private `--apply-update` helper mode to `updater`, and otherwise enters `window::run`.

`window::run` enables per-monitor DPI awareness, enforces a single instance with the global `CodexUsageMonitor` mutex, registers the Win32 window class, restores settings, creates the layered widget, attaches it to a taskbar when possible, registers tray icons, starts background work, and owns the Win32 message loop.

## Module composition

| Module | Responsibility |
| --- | --- |
| `main.rs` | Process modes and startup hand-off |
| `window.rs` | UI state, message loop, taskbar widget, menus, settings, timers, polling coordination, notifications, startup, and update coordination |
| `menu_popup.rs` | Native popup menu pages, keyboard interaction, rendering, and placement |
| `native_interop.rs` | Taskbar enumeration, Win32 child-window embedding, positioning, accessibility hooks, messages, timers, and shared color/string helpers |
| `tray_icon.rs` | Dynamic provider badges, tooltips, shell registration, balloon notifications, and tray input |
| `poller.rs` | Poll orchestration, Claude Code and Antigravity pollers, credential discovery, provider error model, formatting, and reset timing |
| `codex_poller.rs` | Codex CLI discovery and Codex App Server protocol |
| `models.rs` | Provider-independent usage-window and aggregate data models |
| `localization/` | Language selection and 11 complete `Strings` tables |
| `theme.rs` | Windows taskbar light/dark preference from the current-user registry |
| `diagnose.rs` | Optional synchronized diagnostic file |
| `updater.rs` | GitHub Release checks, portable replacement helper, install-channel detection, and WinGet update launch |
| `build.rs` | Application icon and PE string/numeric version resources |

## UI and taskbar ownership

The main thread owns window creation, menus, drawing, timers, and message dispatch. `AppState` is the shared snapshot for provider data, text, retry state, notification latches, taskbar location, settings, language, theme, and update state. A process-wide mutex protects it; code drops the lock before Win32 calls that can synchronously re-enter the window procedure.

The widget is initially a layered popup. `native_interop` enumerates `Shell_TrayWnd` and `Shell_SecondaryTrayWnd`, and `window` reparents the widget as a taskbar child. If embedding fails, the popup remains topmost. The saved `taskbar_index` and `tray_offset` restore placement. Drag calculations clamp the widget before the notification area and support moving it between taskbars.

Rendering uses GDI into a 32-bit surface, then updates the layered window. Provider palettes, two quota rows, text, dimensions, and DPI scaling are calculated in `window.rs`. `theme.rs` reads `SystemUsesLightTheme`; theme and language changes cause repainting without restarting.

The context menu is a separate top-level `WS_EX_TOOLWINDOW` popup in `menu_popup.rs`. It has no taskbar parent or browser runtime. Native owner-drawn button controls provide focus, keyboard input, and accessible names; a single popup navigates between Main, Models, Refresh interval, Settings, Theme, and Language pages. It closes on deactivation or Escape. Its rows are rebuilt from a read-only snapshot of Rust-owned state, and actions use the existing `WM_COMMAND` handlers. Checked controls include a localized selected marker in their accessible names. The old system menu remains available if popup creation fails.

Popup placement uses the invocation cursor, the containing monitor's work area, taskbar edge, and the popup window's per-monitor DPI. Coordinates remain physical pixels until row dimensions are scaled from DIP. The menu uses no timer or background thread. Its HWND, font, child controls, and GDI regions are destroyed when it closes.

### Menu architecture decision

The native popup preserves the existing Win32 message loop and portable EXE while avoiding WebView2 startup and browser-process overhead. The widget stays in the existing native layered rendering path. Provider polling, credentials, updates, tray icons, settings, and the Explorer watchdog remain in Rust. The WebView2 option was not validated in an in-application proof of concept, so this decision does not claim evidence about its focus or performance on this application. The native popup still requires manual keyboard, screen-reader, DPI, and multi-monitor validation on Windows 10 and 11; see `docs/MENU_WINDOWS_VALIDATION.md`.

The widget is shown only when the live taskbar and `TrayNotifyWnd` rectangles leave room for its full width and height. Its invariant is `taskbar.left <= widget.left` and `widget.right <= tray.left`. The notification-area location hook triggers immediate reconciliation, and a five-second timer checks stable geometry as a fallback (one second while geometry is unavailable). Unsupported vertical taskbar geometry hides the widget instead of placing it across shell controls. A cross-process `SetParent` is skipped when DPI awareness contexts differ. The Ukrainian input-layout regression has not been reproduced or proven fixed; `--diagnose` and `scripts/check-input-languages.ps1` distinguish loaded-layout changes from a shell visibility problem.

A dedicated watchdog checks taskbar handles every two seconds. Explorer destroys an embedded child window during a shell restart, so the watchdog relaunches a fresh process with bounded storm protection. The relaunch waits for the previous single-instance mutex before taking ownership.

## Tray icons

`tray_icon.rs` creates provider-specific percentage badges in memory with GDI and registers them through `Shell_NotifyIconW`. There is one badge for every enabled provider. A badge uses the short-window percentage when available and the weekly percentage otherwise. Left-click toggles widget visibility; right-click opens the same application menu as the widget. Authentication warnings use shell balloon notifications and a per-provider latch to avoid repeated alerts during one error episode.

## Polling coordinator and provider isolation

UI timers start `do_poll` on a worker thread; provider network and process operations therefore do not block the message loop. Within that worker, `poller::poll` invokes the enabled provider pollers sequentially. They are logically separate but do not each own a permanent thread.

`PollOutcome` contains independent data and `ProviderPollErrors` fields for Claude Code, Codex, and Antigravity. A successful provider remains available even if another fails. Only when every enabled provider fails does the aggregate operation return an error. Authentication failures latch a notification and switch to credential-change observation; transient request failures use exponential retry delay capped by the configured normal poll interval.

Countdown timers update only display text. When a recorded reset time passes, a separate fast reset poll is scheduled to replace stale quota data.

## Provider pollers

### Codex

`codex_poller.rs` discovers `codex.exe`, `codex.cmd`, `codex.ps1`, and extension-bundled executables from active OpenAI extensions under VS Code, VS Code Insiders, VS Code OSS, Cursor, and Windsurf roots. It starts `codex app-server --disable plugins --disable apps --stdio`, performs the initialize handshake, and requests `account/rateLimits/read`.

The app-server stdout reader has its own short-lived thread and sends newline-delimited JSON messages through a channel. Requests have a 30-second deadline, and the child is killed and joined on scope exit. Primary and secondary windows are normalized into short and weekly `UsageSection` values based on their durations.

### Claude Code

The Claude poller in `poller.rs` searches `%USERPROFILE%\.claude\.credentials.json`, then `~/.claude/.credentials.json` in enumerated WSL distributions. It prefers Anthropic's OAuth usage endpoint. If reset data or the endpoint is unavailable, it makes a one-token Messages request and reads unified rate-limit headers. An expired credential can trigger the matching Windows or WSL Claude CLI so that CLI can refresh its token; every spawned process has a timeout.

### Google Antigravity

The Antigravity poller reads the generic credential target `gemini:antigravity` through `CredReadW`. It probes the configured production, sandbox, and fallback Google Cloud Code bases, loads the Code Assist project, and prefers `retrieveUserQuotaSummary`. If that summary is unavailable, `fetchAvailableModels` supplies a short-window fallback. Gemini quota groups are preferred when a response contains multiple groups.

## Data model and localization

`UsageSection` records availability, used percentage, optional reset time, and optional window duration. `UsageData` holds the short and weekly sections for one provider; `AppUsageData` holds optional values for all three providers. Missing data and failed data remain distinguishable in UI state.

`localization/mod.rs` selects an explicit saved language or detects Windows preferred UI languages. Every localization module supplies the same `Strings` fields. Language changes update titles, menus, tooltips, countdown suffixes, notifications, and visible provider labels.

## Native state and settings

UI settings are JSON at `%APPDATA%\CodexUsageMonitor\settings.json`. They include taskbar placement, polling, language override, update-check timestamp, widget visibility, and provider visibility. If all providers deserialize as disabled, Codex is re-enabled. Startup is not stored in JSON; it is the `CodexUsageMonitor` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and is accepted only when it matches the current executable.

## Diagnostics

Diagnostics are disabled unless `--diagnose` is supplied. `diagnose.rs` creates and truncates `%TEMP%\codex-usage-monitor.log`, then serializes writes through a mutex. Call sites record operational state and error classes, not token values. Some messages can contain executable or credential-file paths and WSL names, so the log is not automatically safe to publish in full.

## Thread model

| Execution context | Work |
| --- | --- |
| Main Win32 thread | Window messages, menus, rendering, timers, settings decisions, and UI state transitions |
| Poll worker | Sequential enabled-provider polling, credential/process waits, and result publication through `PostMessageW` |
| Codex reader | Reads one app-server child's stdout and forwards JSON lines |
| Update-check worker | Queries GitHub Releases and posts completion state |
| Portable-update worker/helper | Downloads on a worker; a copied helper waits for process exit, replaces the EXE, and restarts it |
| Taskbar watchdog | Detects Explorer/taskbar replacement and relaunches the application |

Additional short-lived workers are created by timer-driven polls and explicit update actions; there is no async runtime or permanent provider thread pool.

## Network boundaries

- Codex traffic and authentication stay behind the official local Codex App Server process.
- Claude Code sends HTTPS requests to Anthropic usage or Messages endpoints.
- Antigravity sends HTTPS requests to Google Cloud Code endpoints.
- Update checks call the GitHub Releases API; portable updates download a GitHub Release asset.
- WinGet installations delegate network and manifest handling to `winget.exe`.

The native `ureq` clients use native TLS, 30-second request timeouts, and environment-proxy support. `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` therefore extend the trust boundary to the configured proxy.

## Credential-store boundaries

- Codex bearer credentials are not parsed by the monitor. Only `auth.json` metadata is inspected while watching for a sign-in change.
- Claude OAuth credential files are parsed for the active request, on Windows or through a bounded WSL command. They are not written back.
- Antigravity token JSON is read from Windows Credential Manager and is not persisted by the application.
- `settings.json` contains UI state only and must never contain provider credentials.
- Diagnostic code receives error classes and operational messages, not authorization headers.

## Update flow

1. At most once every 24 hours, or on explicit menu action, `updater` fetches the latest GitHub Release.
2. The tag is compared with the compile-time `CARGO_PKG_VERSION` as a three-component version.
3. The executable path determines the portable or WinGet channel.
4. Portable mode prefers the exact `codex-usage-monitor.exe` asset and otherwise falls back to the first `.exe` asset in the Release. It verifies that the current directory is writable, downloads through a `.part` file, and atomically renames the completed download in the staging directory. Release automation always publishes the exact preferred name.
5. A copy of the running executable becomes `updater-helper.exe`. It waits for the original process, renames the current target to a backup, copies the download into place, restores the backup if copying fails, and restarts the target.
6. WinGet mode starts a PowerShell process that waits for the application, runs `winget upgrade --id DarhangeR.CodexUsageMonitor --exact`, and restarts only after a successful exit code.

Published SHA-256 files support manual validation and CI verifies them before release. The current in-app portable update path does not download or validate the checksum file and has no code-signature check; HTTPS and GitHub asset ownership are therefore part of its security boundary.

## Build and release

`Cargo.toml` is the version source. `build.rs` uses `CARGO_PKG_VERSION`, the source icon, and `package.metadata.winres` to generate PE resources. `scripts/package.ps1` and `scripts/verify-release.ps1` verify the compiled metadata, icon exposure, raw EXE, ZIP layout, forbidden content, and both checksums. GitHub workflows publish only assets that pass those checks.
