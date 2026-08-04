# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| Latest published release | Yes |
| Current `main` branch | Yes |
| Older releases | Not guaranteed |

Security fixes are prepared for the latest release line and current development branch. Upgrade before reporting an issue that affects only an older build.

## Reporting a vulnerability

Report vulnerabilities privately through [GitHub Security Advisories](https://github.com/darhanger/Codex-Usage-Monitor/security/advisories/new). Do not open a public Issue for a vulnerability involving:

- OAuth credentials, bearer tokens, or local credential stores;
- Windows Credential Manager;
- Codex or Claude authorization files;
- GitHub Release checks, downloads, or updater behavior;
- release-asset substitution or version confusion;
- arbitrary file write, replacement, path traversal, or unsafe restart behavior;
- execution of untrusted content;
- unsafe proxy handling or interception;
- leakage through diagnostic output or secret logging;
- the WinGet install or update flow.

Include:

- the application version or commit;
- the Windows version and build number;
- portable or WinGet installation method;
- the affected provider, if applicable;
- security impact and a realistic attack scenario;
- minimal, reproducible steps;
- a minimal proof of concept that contains no real secrets;
- a proposed mitigation, if known.

Do not send real bearer tokens, OAuth tokens, credential files, Windows Credential Manager contents, personal data, full unsanitized diagnostic logs, GitHub tokens, certificates, or signing keys. Replace sensitive values with clearly marked placeholders and include only the smallest evidence needed to reproduce the problem.

Please allow reasonable time to reproduce, assess, and fix the issue before public disclosure. Release timing depends on severity, exploitability, and the safety of the proposed fix.

## Scope notes

The application reads provider credentials only for enabled provider requests and keeps them in process memory. Codex authentication is delegated to the official Codex executable; the monitor observes Codex authorization-file metadata only. Claude Code credentials may be read from Windows or WSL files, and Antigravity credentials are read from Windows Credential Manager.

Portable updates currently select the raw `codex-usage-monitor.exe` asset from the latest GitHub Release over HTTPS. Published SHA-256 files support manual verification, but the in-app updater does not independently verify that checksum or a code signature. Reports that can turn this boundary into a realistic asset-substitution or arbitrary-file-replacement attack are in scope.

Diagnostic logs are opt-in and do not intentionally include token values, but they can contain local paths and environment-specific details. Always sanitize them before sharing.

## Политика безопасности

Поддерживаются последний опубликованный релиз и текущая ветка `main`; поддержка старых версий не гарантируется.

Сообщайте об уязвимостях приватно через [GitHub Security Advisories](https://github.com/darhanger/Codex-Usage-Monitor/security/advisories/new). Не создавайте публичный Issue, если проблема связана с OAuth или bearer-токенами, локальными хранилищами учётных данных, Windows Credential Manager, файлами авторизации Codex/Claude, подменой GitHub Release asset, произвольной заменой файлов, запуском недоверенного содержимого, небезопасным proxy, диагностикой, логированием секретов или WinGet.

В отчёте укажите версию приложения или commit, версию и сборку Windows, способ установки, затронутого провайдера, влияние, реалистичный сценарий атаки, воспроизводимые шаги, минимальный безопасный proof of concept и возможное исправление. Не отправляйте настоящие токены, файлы учётных данных, содержимое Windows Credential Manager, персональные данные, полные неочищенные логи, GitHub tokens, сертификаты или ключи подписи.
