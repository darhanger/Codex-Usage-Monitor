## Summary

Describe the result of this change.

## Motivation

Explain the problem, constraints, and why this change is needed.

## Implementation

Describe the chosen design and any important trade-offs.

## Verification

List automated checks and relevant manual Windows, provider, taskbar, update, or packaging tests.

## Security and privacy

Describe changes to credentials, network endpoints, proxies, diagnostics, downloads, update replacement, or filesystem access. State explicitly when there is no impact.

## Compatibility impact

List effects on Windows 10/11, x64, taskbars, DPI, themes, providers, local settings, portable installs, WinGet, and release assets.

## Checklist

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets --locked -- -D warnings` passes without warnings.
- [ ] `cargo test --all-targets --locked` passes.
- [ ] `cargo build --release --locked` succeeds.
- [ ] Portable packaging succeeds when delivery, metadata, updater, icon, or documentation files changed.
- [ ] `README.md` and `README_RU.md` remain synchronized.
- [ ] All affected localizations remain synchronized.
- [ ] `CHANGELOG.md` is updated when appropriate.
- [ ] No secrets, credentials, logs, binaries, local settings, user data, or personal paths are included.
- [ ] The updater asset remains exactly `codex-usage-monitor.exe`.
- [ ] Existing architecture and functionality were not rewritten or removed without a documented reason.
