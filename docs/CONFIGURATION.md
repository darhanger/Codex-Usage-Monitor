# Configuration

## Settings location

The application stores UI state in:

```text
%APPDATA%\CodexUsageMonitor\settings.json
```

If `APPDATA` cannot be resolved, the implementation falls back to `CodexUsageMonitor\settings.json` relative to the working directory. A normal Windows user session should always provide `APPDATA`.

The file is created when a persistent menu option or position changes. Close the application before editing it manually; a later in-process save replaces the complete JSON document. Unknown JSON fields are not preserved intentionally.

## Stored fields

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `tray_offset` | integer | `0` | Horizontal offset from the notification area on the selected taskbar |
| `taskbar_index` | non-negative integer | `0` | Taskbar index after stable screen-position sorting |
| `poll_interval_ms` | non-negative integer | `900000` | Normal provider polling interval in milliseconds |
| `language` | string or `null` | `null` | Explicit language code; `null` uses Windows language detection |
| `last_update_check_unix` | integer or `null` | `null` | Last completed update-check time in Unix seconds |
| `widget_visible` | boolean | `true` | Saved widget visibility |
| `show_claude_code` | boolean | `false` | Show and poll Claude Code |
| `show_codex` | boolean | `true` | Show and poll Codex |
| `show_antigravity` | boolean | `false` | Show and poll Google Antigravity |

If all three provider flags are false after loading, the application enables Codex. Invalid JSON falls back to every default. Use the menu for supported values; raw numeric settings are deserialized without a separate range-normalization pass.

## Menu settings

Right-click the widget or any provider tray badge.

- **Refresh** starts an immediate background poll and allows a relevant authentication notification to be shown again.
- **Update frequency** selects 1 minute (`60000`), 5 minutes (`300000`), 15 minutes (`900000`), or 1 hour (`3600000`).
- **Models** independently enables Claude Code, Codex, and Antigravity, while preventing the last enabled provider from being disabled.
- **Settings > Start with Windows** updates `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\CodexUsageMonitor` with the current executable path.
- **Settings > Reset position** returns the widget to the default taskbar offset.
- **Settings > Language** uses the Windows default or an explicit supported language.
- The version entry checks for updates, applies an available portable update, or delegates to WinGet for a WinGet installation.
- **Show widget** changes persisted visibility. A tray icon remains available while the widget is hidden.

The widget can also be dragged. `taskbar_index` changes when it is dropped on another taskbar, and `tray_offset` records its clamped position before the system notification area.

## Languages

Saved explicit codes are `en`, `nl`, `es`, `fr`, `de`, `ja`, `ko`, `zh-TW`, `zh-CN`, `ru`, and `pt-BR`. With no override, the application examines preferred Windows UI languages, then the default UI language and locale.

## Provider authorization

### Codex

- Install a recent standalone Codex CLI or an official OpenAI extension containing the Windows Codex executable.
- Supported extension roots include VS Code, VS Code Insiders, VS Code OSS, Cursor, and Windsurf. `VSCODE_EXTENSIONS` and `VSCODE_PORTABLE` are also considered.
- Sign in with a ChatGPT account through the official Codex UI or CLI. API-key-only mode does not provide the account quota windows used by the widget.
- The monitor launches the official CLI App Server. It does not parse the Codex bearer token. `%CODEX_HOME%\auth.json` or `%USERPROFILE%\.codex\auth.json` metadata is used only to notice a login change.

### Claude Code

- Sign in to Claude Code on Windows, WSL, or both.
- Windows credentials are read from `%USERPROFILE%\.claude\.credentials.json`.
- Every installed WSL distribution may be checked for `~/.claude/.credentials.json`; Windows credentials take precedence, followed by WSL enumeration order.
- The file must contain a `claudeAiOauth` object with an access token. The monitor does not support a separate manually configured API-key field.
- When OAuth data is expired, the monitor may run the corresponding Claude CLI with a minimal prompt to let that CLI refresh its credentials.

### Google Antigravity

- Sign in to Google Antigravity on Windows.
- The monitor reads generic credential target `gemini:antigravity` from Windows Credential Manager.
- The credential JSON must contain `token.access_token`.
- No file-path or token override is available in `settings.json`.

Credential sources are read only for enabled providers. Do not copy credential contents into the settings file.

## Polling and retry behavior

The configured interval controls normal provider polls. Polls execute off the UI thread. Enabled providers are called sequentially and return independent success/error fields.

- Transient request failures retry with exponential delay starting at 30 seconds and capped at the selected normal interval.
- Authentication and missing-credential errors show at most one notification per provider error episode and wait for later credential-state changes or an explicit refresh.
- Display countdowns have their own timer and do not perform network requests.
- A passed reset time schedules a faster refresh so stale quota data can be replaced.

## Widget, taskbars, theme, and startup

The selected taskbar and offset are persisted. Taskbars are enumerated by screen position; changing monitor topology can therefore change which physical display corresponds to an old index. Use **Reset position** or drag the widget again after a topology change.

The widget tracks per-monitor DPI. Theme colors follow `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme`; there is no independent application theme setting.

Start with Windows is enabled only when the registry value exactly matches the current executable. Moving a portable EXE intentionally makes the old value appear disabled until the option is enabled again from the new location.

## Update configuration

Automatic update checks run at most once every 24 hours; a manual menu check can run sooner. `last_update_check_unix` records the attempt. The GitHub repository is derived at compile time from `package.repository` in `Cargo.toml`.

- Portable mode is used outside known WinGet package roots. The directory must be writable and the latest Release must contain `codex-usage-monitor.exe`.
- WinGet mode is selected for executables under per-user or machine WinGet `Packages` roots and runs `winget upgrade --id DarhangeR.CodexUsageMonitor --exact`.

There is no setting to switch channels manually. The current executable path determines the channel.

## Proxy variables

The native HTTP client is built with environment-proxy support. Depending on the URL, `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY` may affect Claude Code, Antigravity, GitHub update checks, and portable downloads. Child CLI processes inherit the environment unless that CLI applies its own rules.

Do not place proxy credentials in diagnostics or Issues. A proxy can observe or alter provider and update traffic within the limits of TLS trust configured on the machine, so use only a trusted proxy and certificate chain.

## Diagnostics

Diagnostics are not a saved menu setting. Start a process explicitly with:

```powershell
.\codex-usage-monitor.exe --diagnose
```

The file `%TEMP%\codex-usage-monitor.log` is truncated on each diagnostic launch. It can include discovered executable paths, credential-file paths, WSL distribution names, taskbar coordinates, and error categories. It must be sanitized before sharing even though token values are not intentionally logged.
