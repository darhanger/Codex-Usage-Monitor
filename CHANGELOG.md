# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Locked, repeatable Windows x64 Release builds on a fixed GitHub-hosted runner.
- Automated formatting, Clippy, test, and Release-build checks for pushes, pull requests, and manual runs.
- A verified portable package containing the executable, license, and synchronized English and Russian readmes.
- SHA-256 files for both the raw executable and versioned ZIP archive.
- Automatic versioned GitHub Releases from tested `main` artifacts.
- A separate tag/manual workflow that verifies the exact tag, Cargo version, commit, PE metadata, package, and checksums.
- Safe optional WinGet manifest submission for `DarhangeR.CodexUsageMonitor`.
- GitHub Issue forms and a Pull Request template.
- Architecture, configuration, release, contribution, and security documentation.

### Changed

- CI and release packaging now use `Cargo.toml` metadata as the single version source.
- Release publication preserves the raw `codex-usage-monitor.exe` asset required by the portable updater.
- English and Russian project readmes now document matching installation, update, security, diagnostics, build, and release flows.

### Fixed

- No application behavior fixes are recorded for this documentation and delivery update.

### Security

- Release jobs use least-privilege GitHub token permissions and refuse version/tag mismatches.
- Packaging rejects debug symbols, logs, settings, credential material, unsafe ZIP paths, and unexpected executables.
- WinGet credentials are passed through WingetCreate's supported environment variable and are never written to command arguments or logs.

[Unreleased]: https://github.com/darhanger/Codex-Usage-Monitor/commits/main
