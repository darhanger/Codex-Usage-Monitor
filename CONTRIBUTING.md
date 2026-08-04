# Contributing to Codex Usage Monitor

Thank you for improving Codex Usage Monitor. Keep changes small, focused, and compatible with the existing native Windows architecture.

## Workflow

1. Create a branch from the current `main` branch.
2. Make one logically focused change. Avoid unrelated formatting or refactoring.
3. Add or update tests for deterministic logic and edge cases.
4. Synchronize user-facing text across every affected localization.
5. Keep `README.md` and `README_RU.md` structurally and factually synchronized.
6. Update `CHANGELOG.md` for user-visible, compatibility, security, build, or delivery changes.
7. Run every local verification command below.
8. Open a Pull Request that explains motivation, implementation, compatibility impact, and verification.

Do not rewrite established modules, rename public concepts, or remove functionality unless the change requires it and the Pull Request explains the trade-off.

## Local verification

Run from a Windows PowerShell prompt with the stable Rust toolchain and MSVC build tools installed:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\package.ps1
```

Packaging is mandatory when changing `Cargo.toml`, `Cargo.lock`, `build.rs`, icons, readmes, release scripts, updater behavior, or GitHub workflows.

## Rust and architecture requirements

- Use Rust 2021 and preserve the existing module boundaries.
- Keep the Win32 message thread responsive. Network access, provider discovery, process waits, update checks, downloads, and other blocking work must not run on the UI thread.
- Preserve per-provider error isolation. A Claude Code, Codex, or Antigravity failure must not discard successful results from another provider.
- Avoid unnecessary allocation, cloning, process launches, filesystem probes, and registry calls in timers, painting, tray updates, or other frequently executed paths.
- Keep worker-to-UI communication explicit through the existing state and window-message flow.
- Continue to support Windows 10 and Windows 11, primary and secondary taskbars, Explorer restarts, and per-monitor DPI.
- Do not change the raw updater asset name `codex-usage-monitor.exe`.
- Keep `Cargo.toml` as the only version source; the tag, archive, PE metadata, Release title, and WinGet version must derive from it.

## Localization and documentation

- Update every affected file under `src/localization` when changing visible strings.
- Keep language identifiers, menu item mappings, and `Strings` fields aligned.
- Update both readmes in the same Pull Request.
- Keep `docs/ARCHITECTURE*.md` and `docs/CONFIGURATION*.md` synchronized when implementation details change.
- Do not document a provider, command, asset, setting, update guarantee, or WinGet availability that the code or release configuration does not provide.

## Credentials, privacy, and diagnostics

- Access only the credential stores already required by the affected provider.
- Never persist provider access tokens or copy them into application settings.
- Never log access tokens, bearer tokens, OAuth refresh tokens, authorization-file contents, Windows Credential Manager contents, GitHub tokens, or signing keys.
- Diagnostic messages must remain useful without secrets. Treat user names, local paths, WSL distribution names, and account details as personal data and minimize them.
- Preserve proxy behavior and make any new network endpoint, request body, or credential boundary explicit in the Pull Request.
- Validate downloaded metadata, release names, archive paths, and filesystem destinations before use.

## Files that must never be committed

Do not commit:

- `target`, `artifacts`, executables, PDB files, archives, or generated WinGet manifests;
- logs, diagnostic output, temporary package directories, or local settings;
- `.env` files, authentication files, token files, credential exports, certificates, or signing keys;
- IDE state, personal paths, screenshots containing private data, or files copied from `%APPDATA%` or `%LOCALAPPDATA%`.

Review `git status --short` and the complete staged diff before every commit.

## Reporting problems

Use the repository Issue forms for reproducible bugs and feature requests. Vulnerabilities must be reported privately according to [SECURITY.md](SECURITY.md), never through a public Issue.
