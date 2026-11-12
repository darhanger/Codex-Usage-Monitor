# Release checklist

Use this checklist for every public version. `Cargo.toml` is the only version source; do not hand-maintain a second version in workflows or scripts.

## Source and version

- [ ] Pull the current `main` branch and confirm the worktree contains no unrelated changes.
- [ ] Set the next `MAJOR.MINOR.PATCH` version in `Cargo.toml`.
- [ ] Run `cargo update --workspace --offline` to refresh the root package entry in `Cargo.lock` without updating dependencies, then review that lockfile change.
- [ ] Run `cargo metadata --no-deps --locked --format-version 1` and confirm the root package version.
- [ ] Update `CHANGELOG.md` without inventing unverified release history.
- [ ] Synchronize `README.md` and `README_RU.md`.
- [ ] Synchronize `docs/ARCHITECTURE.md` with `docs/ARCHITECTURE_RU.md`.
- [ ] Synchronize `docs/CONFIGURATION.md` with `docs/CONFIGURATION_RU.md`.
- [ ] Synchronize every affected file under `src/localization`.
- [ ] Confirm repository links, workflow names, commands, package ID, and asset names still exist.

## Automated verification

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo clippy --all-targets --locked -- -D warnings` with no warnings.
- [ ] Run `cargo test --all-targets --locked`.
- [ ] Run `cargo build --release --locked`.
- [ ] Run `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\package.ps1`.
- [ ] Run `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-release.ps1` independently.
- [ ] Validate all GitHub workflow and Issue-form YAML files.
- [ ] Confirm `git status --short` contains no `target`, `artifacts`, executable, archive, log, settings, credential, or manifest output.

## PE and portable assets

- [ ] Confirm `artifacts\codex-usage-monitor.exe` exists.
- [ ] Confirm its PE machine type is AMD64 (`0x8664`).
- [ ] Confirm its `FileVersion` and `ProductVersion` exactly match `Cargo.toml` (an optional fourth `.0` component is acceptable).
- [ ] Confirm `CompanyName`, `ProductName`, `FileDescription`, `OriginalFilename`, and copyright match `package.metadata.winres`.
- [ ] Extract the associated icon and confirm it is the intended project icon.
- [ ] Confirm `artifacts\codex-usage-monitor.exe.sha256` has `<64 lowercase hex> *codex-usage-monitor.exe` format and verifies.
- [ ] Confirm `artifacts\CodexUsageMonitor-<version>-Windows-x64.zip` uses the Cargo version.
- [ ] Confirm the ZIP contains `codex-usage-monitor.exe`, `LICENSE`, `README.md`, and `README_RU.md` at its root.
- [ ] Confirm the ZIP contains no PDB, debug output, logs, diagnostics, settings, credentials, tokens, manifests, IDE state, `.git`, or unexpected EXE.
- [ ] Confirm the EXE in the ZIP is byte-identical to the raw EXE.
- [ ] Confirm the ZIP SHA-256 verifies and its checksum line names the exact archive.

## Application compatibility

- [ ] Test the raw portable EXE on a supported Windows 10 x64 system.
- [ ] Test the raw portable EXE on a supported Windows 11 x64 system.
- [ ] Test light and dark Windows taskbar themes.
- [ ] Test common DPI scales and mixed-DPI monitors.
- [ ] Test the primary taskbar.
- [ ] Test at least one secondary taskbar and drag between taskbars.
- [ ] Restart Windows Explorer and confirm the widget and tray icons recover.
- [ ] Test hide/show, position reset, polling intervals, language selection, and Start with Windows.
- [ ] Test Codex with a supported CLI or bundled extension and ChatGPT sign-in.
- [ ] Test Claude Code with Windows credentials.
- [ ] Test Claude Code through WSL when that path is supported by the release.
- [ ] Test Google Antigravity with Windows Credential Manager.
- [ ] Confirm one provider failure does not suppress successful providers.
- [ ] Run `--diagnose`, inspect the complete log for tokens and credential contents, then remove the local log.

## Update channels

- [ ] On a writable portable copy, check for the previous-to-current update and confirm download, replacement, rollback behavior, and restart.
- [ ] Confirm the GitHub Release contains the exact raw name `codex-usage-monitor.exe`; do not replace it with only a ZIP.
- [ ] Manually compare the downloaded portable EXE with its published SHA-256.
- [ ] Test a WinGet-installed previous version and confirm `winget upgrade --id DarhangeR.CodexUsageMonitor --exact` succeeds.
- [ ] Confirm install-channel detection chooses WinGet only under known WinGet package roots.

## Git tag and GitHub Release

- [ ] Merge the version and changelog update into `main` only after CI passes.
- [ ] Confirm the Build workflow tests and packages the exact intended commit.
- [ ] Confirm tag `v<version>` is absent, or already points to that exact commit.
- [ ] If using the tag/manual workflow, create and push the exact `vMAJOR.MINOR.PATCH` tag or select that existing tag in `workflow_dispatch`.
- [ ] Confirm release title `Codex Usage Monitor v<version>`.
- [ ] Review automatically generated release notes for sensitive or misleading content.
- [ ] Confirm the Release is public, not a draft or prerelease unless intentionally changed.
- [ ] Confirm all four assets exist:
  - [ ] `codex-usage-monitor.exe`
  - [ ] `codex-usage-monitor.exe.sha256`
  - [ ] `CodexUsageMonitor-<version>-Windows-x64.zip`
  - [ ] `CodexUsageMonitor-<version>-Windows-x64.zip.sha256`
- [ ] Download all assets from GitHub and verify both checksums again.

## WinGet submission

- [ ] If the package ID is not yet public, submit and obtain acceptance for the initial manifest; WingetCreate `update` automates subsequent versions only.
- [ ] Confirm repository secret `WINGETCREATE_GITHUB_TOKEN` exists before expecting automatic submission.
- [ ] Confirm the token belongs to the intended account/fork and has only the access WingetCreate requires.
- [ ] Confirm the manifest version matches the Release and its installer URL points to the raw EXE asset.
- [ ] Review the WingetCreate pull request for installer type, architecture, URL, SHA-256, and package metadata.
- [ ] Wait for the manifest to be accepted before claiming public WinGet availability.
- [ ] Test a clean `winget install --id DarhangeR.CodexUsageMonitor --exact` after acceptance.

## Repository settings

- [ ] Enable GitHub Actions and allow the workflow `GITHUB_TOKEN` to write repository contents so Release jobs can create tags and Releases.
- [ ] Add `WINGETCREATE_GITHUB_TOKEN` as an Actions repository secret; do not place it in variables or files.
- [ ] Protect `main` and require the Windows build/test job before merge.
- [ ] Keep GitHub Security Advisories/private vulnerability reporting enabled.
- [ ] Confirm artifact retention and Actions storage policies permit the configured 14-day build artifact.
