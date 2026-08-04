<!-- markdownlint-disable MD004 MD033 -->

<div align="center">

**English** | [Русский](README_RU.md)

<img src="src/icons/256x256.png" width="112" alt="Codex Usage Monitor icon">

# Codex Usage Monitor

[![Build](https://github.com/darhanger/Codex-Usage-Monitor/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/darhanger/Codex-Usage-Monitor/actions/workflows/build.yml)
![Windows 10/11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?style=flat-square&logo=windows11&logoColor=white)
![Rust 2021](https://img.shields.io/badge/Rust-2021-000000?style=flat-square&logo=rust&logoColor=white)
[![Latest release](https://img.shields.io/github/v/release/darhanger/Codex-Usage-Monitor?style=flat-square)](https://github.com/darhanger/Codex-Usage-Monitor/releases/latest)
[![License](https://img.shields.io/github/license/darhanger/Codex-Usage-Monitor?style=flat-square)](LICENSE)
[![Latest release downloads](https://img.shields.io/github/downloads/darhanger/Codex-Usage-Monitor/latest/total?style=flat-square)](https://github.com/darhanger/Codex-Usage-Monitor/releases/latest)
[![All downloads](https://img.shields.io/github/downloads/darhanger/Codex-Usage-Monitor/total?style=flat-square)](https://github.com/darhanger/Codex-Usage-Monitor/releases)
[![Discord](https://img.shields.io/badge/Discord-7289DA?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/ZKFkvrzaU4)

A lightweight native Windows taskbar widget for **Codex**, **Claude Code**, and **Google Antigravity** usage limits.

</div>

## Features

- Segmented short-window and weekly usage bars with reset countdowns.
- Independent columns and percentage tray badges for each enabled provider.
- Provider-specific authentication notifications and isolated provider failures.
- One-click refresh, configurable polling, provider selection, and 11 UI languages.
- Drag placement on primary and secondary taskbars.
- Automatic recovery after Windows Explorer restarts.
- Per-monitor DPI handling and Windows light/dark taskbar colors.
- Single-instance operation and optional start with Windows.
- Portable self-update and WinGet-aware update handling.
- Opt-in diagnostic logging with no analytics or telemetry.

## Screenshots

No sanitized UI screenshot is currently tracked in the repository. The application is a compact two-row taskbar widget; screenshots added to the project must be checked for account names, local paths, notifications, and other personal information.

## Requirements

- Windows 10 or Windows 11 on an x64-capable system.
- A supported provider installed and authenticated as described below.
- For source builds: the stable Rust toolchain and the Visual Studio 2022 MSVC C++ build tools.

Codex quota windows require ChatGPT account authentication. API-key-only authentication does not expose the same account rate-limit data.

## Installation

Every GitHub Release contains two portable options:

- `codex-usage-monitor.exe` — the raw executable used by the built-in portable updater and WinGet manifest.
- `CodexUsageMonitor-<version>-Windows-x64.zip` — the executable together with `LICENSE`, `README.md`, and `README_RU.md`.

Both downloads have adjacent `.sha256` files. To verify the raw executable in PowerShell, compare the command output with `codex-usage-monitor.exe.sha256`:

```powershell
Get-FileHash .\codex-usage-monitor.exe -Algorithm SHA256
```

### Portable installation

1. Open the [latest GitHub Release](https://github.com/darhanger/Codex-Usage-Monitor/releases/latest).
2. Download the raw EXE or the versioned ZIP and its checksum.
3. If using the ZIP, extract all files to a user-writable directory.
4. Run `codex-usage-monitor.exe`.

No installer, browser extension, or background service is added by the portable package. Keep the executable in a user-writable directory if you want portable self-update to replace it automatically.

### WinGet installation

The release automation targets package ID `DarhangeR.CodexUsageMonitor`:

```powershell
winget install --id DarhangeR.CodexUsageMonitor --exact
```

The command becomes available only after the corresponding manifest has been accepted into the public WinGet repository. If WinGet cannot find the ID, use the portable release while the manifest is pending.

The automated WingetCreate job updates an existing public manifest. The initial manifest for this package ID must be submitted and accepted before automatic version updates can succeed.

## First launch

1. Sign in to at least one supported provider before starting the monitor.
2. Run the application. Codex is the only provider enabled by default.
3. Right-click the widget or a tray badge and open **Models** to enable other providers.
4. Use **Settings > Start with Windows** if desired.
5. Drag the thin divider at the left edge to position the widget.

Only one instance runs at a time. If the taskbar cannot accept the embedded child window, the application uses a topmost fallback window.

## Usage

- Drag the thin left divider to reposition the widget or move it to another taskbar.
- Left-click a provider tray badge to show or hide the widget.
- Right-click the widget or a badge to open the menu.
- Select **Refresh** for an immediate poll and to repeat a relevant authentication notification.
- Select **Models** to enable or disable provider columns. At least one remains enabled.
- Choose a polling interval of 1 minute, 5 minutes, 15 minutes, or 1 hour.
- Reset the saved taskbar position from **Settings**.
- Use the version entry in **Settings** to check for or apply an update.

The tray badge uses the short-window percentage when available and falls back to the weekly percentage.

## Provider configuration

### Codex

Install and sign in to a recent Codex CLI or the official OpenAI extension in VS Code, VS Code Insiders, VS Code OSS, Cursor, or Windsurf. The monitor starts `codex app-server` over stdio and requests `account/rateLimits/read`; the official Codex process owns authentication and token refresh. The monitor watches only metadata for `%CODEX_HOME%\auth.json` or `%USERPROFILE%\.codex\auth.json` when waiting for a login change.

### Claude Code

Sign in with Claude Code on Windows or in WSL. The monitor looks for OAuth credentials in `%USERPROFILE%\.claude\.credentials.json`, then in installed WSL distributions at `~/.claude/.credentials.json`. An expired token may cause the corresponding Claude CLI to run a minimal request so the CLI can refresh its own credentials.

### Google Antigravity

Sign in to Antigravity on Windows. The monitor reads the `gemini:antigravity` generic credential from Windows Credential Manager and requests the available quota summary from Google Cloud Code endpoints.

Never copy real credentials into an issue, diagnostic excerpt, configuration file, or test fixture.

## Supported providers

| Provider | Authentication source | Usage source |
| --- | --- | --- |
| Codex | Official Codex CLI or bundled IDE CLI | Local Codex App Server |
| Claude Code | Windows or WSL Claude OAuth credentials | Anthropic usage endpoint, with rate-limit header fallback |
| Google Antigravity | Windows Credential Manager | Google Cloud Code quota endpoints |

Providers are polled independently at the error-model level. A failure in one enabled provider does not discard successful data from the others.

## Configuration

The menu controls provider visibility, poll interval, widget visibility and position, language, startup, and updates. UI settings are stored at:

```text
%APPDATA%\CodexUsageMonitor\settings.json
```

Start with Windows is stored separately in the current user's `Run` registry key. See [Configuration](docs/CONFIGURATION.md) for the fields, defaults, proxy variables, and provider requirements.

Supported UI languages are English, Dutch, Spanish, French, German, Japanese, Korean, Traditional Chinese, Simplified Chinese, Russian, and Brazilian Portuguese.

## Update channels

| Channel | Detection | Update behavior |
| --- | --- | --- |
| Portable | Executable is outside known WinGet package roots | Downloads the raw `codex-usage-monitor.exe` asset, replaces the current executable through a helper process, and restarts it |
| WinGet | Executable is under a known WinGet package root | Runs `winget upgrade --id DarhangeR.CodexUsageMonitor --exact` and restarts after success |

Automatic update checks run at most once every 24 hours and can also be started from the menu. The portable updater requires a writable installation directory. Release checksums are provided for manual verification; the current in-app portable updater relies on HTTPS and the GitHub Release asset name and does not independently verify the published `.sha256` file.

## Privacy and security

The application has no analytics or telemetry and does not inspect prompts, conversations, or project files. It reads only the enabled providers' standard authentication sources, its settings, and metadata used to notice credential changes.

Direct provider and GitHub requests use HTTPS. If `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY` is set, those requests may use the configured proxy; use only a trusted proxy. Tokens are held in memory for provider requests and must never be logged or committed.

Read [SECURITY.md](SECURITY.md) before reporting a vulnerability. Security reports involving credentials, update replacement, proxies, diagnostics, or WinGet must use private GitHub Security Advisories rather than public Issues.

## Diagnostics

Start the application with:

```powershell
.\codex-usage-monitor.exe --diagnose
```

The log is recreated at `%TEMP%\codex-usage-monitor.log`. It records startup, taskbar placement, provider discovery, poll outcomes, and notification delivery, but not bearer-token values. It can contain local paths and environment-specific details, so share only a short sanitized excerpt.

## Building from source

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

The executable is written to `target\release\codex-usage-monitor.exe`. The version comes from `Cargo.toml`; `build.rs` embeds the icon and Windows version metadata.

## Portable packaging

After a Release build:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\package.ps1
```

The script obtains the version through `cargo metadata`, validates the AMD64 PE header, version metadata, and associated icon, produces the raw EXE and ZIP with SHA-256 files under `artifacts`, and reopens the ZIP to verify its contents. See [Release checklist](docs/RELEASE_CHECKLIST.md) for the complete validation sequence.

## Release process

- Pushes and pull requests targeting `main`, plus manual runs, execute formatting, Clippy, tests, Release build, packaging, and artifact upload through [`build.yml`](.github/workflows/build.yml).
- A successful push to `main` publishes or updates `v<version>` from the exact tested artifact. An existing tag on another commit is rejected.
- [`release.yml`](.github/workflows/release.yml) rebuilds and validates an explicitly pushed or manually selected existing `vMAJOR.MINOR.PATCH` tag.
- A successful GitHub Release may submit the raw EXE to WinGet when `WINGETCREATE_GITHUB_TOKEN` is configured.

`Cargo.toml` is the only application version source. Update it and [CHANGELOG.md](CHANGELOG.md) before releasing.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) · [Архитектура](docs/ARCHITECTURE_RU.md)
- [Configuration](docs/CONFIGURATION.md) · [Настройка](docs/CONFIGURATION_RU.md)
- [Release checklist](docs/RELEASE_CHECKLIST.md)
- [Changelog](CHANGELOG.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Support

- Use [GitHub Issues](https://github.com/darhanger/Codex-Usage-Monitor/issues) for reproducible bugs and focused feature requests.
- Use [Discord](https://discord.gg/ZKFkvrzaU4) for community support.
- Use [GitHub Security Advisories](https://github.com/darhanger/Codex-Usage-Monitor/security/advisories/new) for vulnerabilities.

Sanitize diagnostic logs, screenshots, local paths, and account information before sharing them.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), keep changes focused, preserve provider failure isolation and non-blocking UI behavior, synchronize user-facing documentation and localizations, and run every required Cargo check before opening a pull request.

## Attribution

The multi-provider taskbar architecture is adapted from [CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor), licensed under MIT.

This project is unofficial and is not affiliated with or endorsed by OpenAI, Anthropic, or Google.

## License

Codex Usage Monitor is distributed under the [MIT License](LICENSE).

---

<div align="center">

[Download](https://github.com/darhanger/Codex-Usage-Monitor/releases/latest) · [Report an issue](https://github.com/darhanger/Codex-Usage-Monitor/issues) · [Discord](https://discord.gg/ZKFkvrzaU4)

</div>
