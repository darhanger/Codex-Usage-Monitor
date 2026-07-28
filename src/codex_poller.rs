use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::diagnose;
use crate::models::{UsageData, UsageSection};
use crate::poller::PollError;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(30);
const INITIALIZE_REQUEST_ID: u64 = 0;
const RATE_LIMITS_REQUEST_ID: u64 = 1;
const VSCODE_EXTENSION_ID: &str = "openai.chatgpt";

#[cfg(target_arch = "aarch64")]
const VSCODE_CODEX_PLATFORM_DIRECTORY: &str = "windows-aarch64";
#[cfg(not(target_arch = "aarch64"))]
const VSCODE_CODEX_PLATFORM_DIRECTORY: &str = "windows-x86_64";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitsResponse {
    rate_limits: Option<RateLimitSnapshot>,
    #[serde(default)]
    rate_limits_by_limit_id: Option<HashMap<String, RateLimitSnapshot>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitSnapshot {
    primary: Option<RateLimitWindow>,
    secondary: Option<RateLimitWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindow {
    used_percent: f64,
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

#[derive(Clone, Debug)]
enum CodexLauncher {
    Direct(String),
    CommandPrompt(String),
    PowerShell(String),
}

impl CodexLauncher {
    fn path(&self) -> &str {
        match self {
            Self::Direct(path) | Self::CommandPrompt(path) | Self::PowerShell(path) => path,
        }
    }

    fn command(&self) -> Command {
        let mut command = match self {
            Self::Direct(path) => Command::new(path),
            Self::CommandPrompt(path) => {
                let mut command = Command::new("cmd.exe");
                command.args(["/d", "/s", "/c"]).arg(path);
                command
            }
            Self::PowerShell(path) => {
                let mut command = Command::new("powershell.exe");
                command
                    .args([
                        "-NoLogo",
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                    ])
                    .arg(path);
                command
            }
        };

        add_launcher_directory_to_path(&mut command, self.path());

        command.args([
            "app-server",
            "--disable",
            "plugins",
            "--disable",
            "apps",
            "--stdio",
        ]);
        command
    }
}

struct AppServerProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    reader_thread: Option<JoinHandle<()>>,
}

impl Drop for AppServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}

pub fn poll() -> Result<UsageData, PollError> {
    let mut server = start_app_server()?;

    send_message(
        &mut server.stdin,
        &json!({
            "method": "initialize",
            "id": INITIALIZE_REQUEST_ID,
            "params": {
                "clientInfo": {
                    "name": "codex_usage_monitor",
                    "title": "Codex Usage Monitor",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
    )?;
    receive_response(&server.lines, INITIALIZE_REQUEST_ID, false)?;

    send_message(
        &mut server.stdin,
        &json!({"method": "initialized", "params": {}}),
    )?;
    send_message(
        &mut server.stdin,
        &json!({"method": "account/rateLimits/read", "id": RATE_LIMITS_REQUEST_ID}),
    )?;

    let result = receive_response(&server.lines, RATE_LIMITS_REQUEST_ID, true)?;
    let usage = usage_from_rate_limits_value(result)?;
    diagnose::log("Codex rate limits received");
    Ok(usage)
}

fn start_app_server() -> Result<AppServerProcess, PollError> {
    let launchers = discover_codex_launchers();
    let mut last_error = None;

    for launcher in launchers {
        let mut command = launcher.command();
        command
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };

        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        };

        let (sender, lines) = mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        diagnose::log(format!("started Codex app-server via {}", launcher.path()));
        return Ok(AppServerProcess {
            child,
            stdin,
            lines,
            reader_thread: Some(reader_thread),
        });
    }

    if let Some(error) = last_error {
        diagnose::log_error("unable to start Codex CLI app-server", error);
    } else {
        diagnose::log("unable to start Codex CLI app-server: no launcher found");
    }
    Err(PollError::RequestFailed)
}

fn discover_codex_launchers() -> Vec<CodexLauncher> {
    let mut launchers = Vec::new();

    for name in ["codex.exe", "codex.cmd", "codex.ps1", "codex"] {
        let output = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();

        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        for path in String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            push_launcher(&mut launchers, launcher_for_path(path));
        }
    }

    for path in discover_vscode_codex_executables() {
        diagnose::log(format!(
            "discovered bundled Codex CLI at {}",
            path.display()
        ));
        push_launcher(
            &mut launchers,
            CodexLauncher::Direct(path.to_string_lossy().into_owned()),
        );
    }

    push_launcher(
        &mut launchers,
        CodexLauncher::Direct("codex.exe".to_string()),
    );
    push_launcher(&mut launchers, CodexLauncher::Direct("codex".to_string()));
    push_launcher(
        &mut launchers,
        CodexLauncher::CommandPrompt("codex.cmd".to_string()),
    );
    push_launcher(
        &mut launchers,
        CodexLauncher::PowerShell("codex.ps1".to_string()),
    );

    launchers
}

fn add_launcher_directory_to_path(command: &mut Command, launcher_path: &str) {
    let path = Path::new(launcher_path);
    if !path.is_absolute() {
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };

    let mut entries: Vec<PathBuf> = std::env::var_os("PATH")
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect();
    if entries.iter().any(|entry| paths_equal(entry, parent)) {
        return;
    }
    entries.push(parent.to_path_buf());
    if let Ok(path_value) = std::env::join_paths(entries) {
        command.env("PATH", path_value);
    }
}

fn discover_vscode_codex_executables() -> Vec<PathBuf> {
    let mut executables = Vec::new();

    for root in vscode_extension_roots() {
        for extension in active_openai_extensions(&root) {
            push_bundled_codex_executable(&mut executables, &extension);
        }

        let mut fallback_extensions = Vec::new();
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.starts_with("openai.chatgpt-") {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .unwrap_or(UNIX_EPOCH);
            fallback_extensions.push((modified, entry.path()));
        }
        fallback_extensions.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        for (_, extension) in fallback_extensions {
            push_bundled_codex_executable(&mut executables, &extension);
        }
    }

    executables
}

fn vscode_extension_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(root) = nonempty_env_path("VSCODE_EXTENSIONS") {
        push_unique_path(&mut roots, root);
    }
    if let Some(portable_root) = nonempty_env_path("VSCODE_PORTABLE") {
        push_unique_path(&mut roots, portable_root.join("data").join("extensions"));
    }
    if let Some(home) = user_home_dir() {
        for relative in [
            ".vscode/extensions",
            ".vscode-insiders/extensions",
            ".vscode-oss/extensions",
            ".cursor/extensions",
            ".windsurf/extensions",
        ] {
            push_unique_path(&mut roots, home.join(relative));
        }
    }

    roots
}

fn active_openai_extensions(root: &Path) -> Vec<PathBuf> {
    let Ok(content) = std::fs::read_to_string(root.join("extensions.json")) else {
        return Vec::new();
    };
    let Ok(entries) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    let Some(entries) = entries.as_array() else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| active_openai_extension_path(root, entry))
        .collect()
}

fn active_openai_extension_path(root: &Path, entry: &Value) -> Option<PathBuf> {
    let id = entry.pointer("/identifier/id")?.as_str()?;
    if !id.eq_ignore_ascii_case(VSCODE_EXTENSION_ID) {
        return None;
    }

    if let Some(path) = entry.pointer("/location/fsPath").and_then(Value::as_str) {
        let path = PathBuf::from(path);
        return Some(if path.is_absolute() {
            path
        } else {
            root.join(path)
        });
    }

    entry
        .get("relativeLocation")
        .and_then(Value::as_str)
        .map(|relative| root.join(relative))
}

fn push_bundled_codex_executable(executables: &mut Vec<PathBuf>, extension: &Path) {
    let executable = extension
        .join("bin")
        .join(VSCODE_CODEX_PLATFORM_DIRECTORY)
        .join("codex.exe");
    if executable.is_file() {
        push_unique_path(executables, executable);
    }
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn user_home_dir() -> Option<PathBuf> {
    nonempty_env_path("USERPROFILE")
        .filter(|path| path.is_absolute())
        .or_else(dirs::home_dir)
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| paths_equal(existing, &path)) {
        paths.push(path);
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = left.as_os_str().as_encoded_bytes();
    let right = right.as_os_str().as_encoded_bytes();
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            let normalize = |byte: u8| match byte {
                b'/' | b'\\' => b'\\',
                _ => byte.to_ascii_lowercase(),
            };
            normalize(*left) == normalize(*right)
        })
}

fn launcher_for_path(path: &str) -> CodexLauncher {
    let lowercase = path.to_ascii_lowercase();
    if lowercase.ends_with(".cmd") || lowercase.ends_with(".bat") {
        CodexLauncher::CommandPrompt(path.to_string())
    } else if lowercase.ends_with(".ps1") {
        CodexLauncher::PowerShell(path.to_string())
    } else {
        CodexLauncher::Direct(path.to_string())
    }
}

fn push_launcher(launchers: &mut Vec<CodexLauncher>, launcher: CodexLauncher) {
    if launchers
        .iter()
        .any(|existing| existing.path().eq_ignore_ascii_case(launcher.path()))
    {
        return;
    }
    launchers.push(launcher);
}

fn send_message(stdin: &mut ChildStdin, message: &Value) -> Result<(), PollError> {
    serde_json::to_writer(&mut *stdin, message).map_err(|error| {
        diagnose::log_error("unable to serialize Codex app-server request", error);
        PollError::RequestFailed
    })?;
    stdin.write_all(b"\n").map_err(|error| {
        diagnose::log_error("unable to write Codex app-server request", error);
        PollError::RequestFailed
    })?;
    stdin.flush().map_err(|error| {
        diagnose::log_error("unable to flush Codex app-server request", error);
        PollError::RequestFailed
    })
}

fn receive_response(
    lines: &Receiver<String>,
    expected_id: u64,
    classify_auth_errors: bool,
) -> Result<Value, PollError> {
    let deadline = Instant::now() + APP_SERVER_TIMEOUT;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            diagnose::log(format!("Codex app-server response {expected_id} timed out"));
            return Err(PollError::RequestFailed);
        }

        let line = match lines.recv_timeout(remaining) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                diagnose::log(format!("Codex app-server response {expected_id} timed out"));
                return Err(PollError::RequestFailed);
            }
            Err(RecvTimeoutError::Disconnected) => {
                diagnose::log("Codex app-server closed before returning a response");
                return Err(PollError::RequestFailed);
            }
        };

        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                diagnose::log_error("unable to parse Codex app-server message", error);
                continue;
            }
        };

        if message.get("id").and_then(Value::as_u64) != Some(expected_id) {
            continue;
        }

        if let Some(error) = message.get("error") {
            let error_message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown Codex app-server error");
            diagnose::log(format!(
                "Codex app-server request {expected_id} failed: {error_message}"
            ));
            return Err(if classify_auth_errors {
                classify_rpc_error(error_message)
            } else {
                PollError::RequestFailed
            });
        }

        return message
            .get("result")
            .cloned()
            .ok_or(PollError::RequestFailed);
    }
}

fn classify_rpc_error(message: &str) -> PollError {
    let message = message.to_ascii_lowercase();
    if message.contains("authentication required")
        || message.contains("login required")
        || message.contains("not logged in")
    {
        PollError::NoCredentials
    } else if message.contains("expired") {
        PollError::TokenExpired
    } else if message.contains("unauthorized")
        || message.contains("auth is not supported")
        || message.contains("chatgpt authentication")
    {
        PollError::AuthRequired
    } else {
        PollError::RequestFailed
    }
}

fn usage_from_rate_limits_value(value: Value) -> Result<UsageData, PollError> {
    let response: RateLimitsResponse = serde_json::from_value(value).map_err(|error| {
        diagnose::log_error("unable to parse Codex rate-limit response", error);
        PollError::RequestFailed
    })?;

    let RateLimitsResponse {
        rate_limits,
        rate_limits_by_limit_id,
    } = response;
    let mut rate_limits_by_limit_id = rate_limits_by_limit_id.unwrap_or_default();
    let codex_limits = rate_limits_by_limit_id.remove("codex");
    let sole_named_limit = if rate_limits_by_limit_id.len() == 1 {
        rate_limits_by_limit_id.into_values().next()
    } else {
        None
    };
    let snapshot = [codex_limits, rate_limits, sole_named_limit]
        .into_iter()
        .flatten()
        .find(|snapshot| snapshot.primary.is_some() || snapshot.secondary.is_some());
    let Some(snapshot) = snapshot else {
        diagnose::log("Codex rate-limit response did not contain quota windows");
        return Err(PollError::RequestFailed);
    };

    Ok(usage_data_from_snapshot(snapshot))
}

fn usage_data_from_snapshot(snapshot: RateLimitSnapshot) -> UsageData {
    let primary = snapshot.primary.map(usage_section_from_window);
    let secondary = snapshot.secondary.map(usage_section_from_window);

    match (primary, secondary) {
        (Some(primary), Some(secondary)) => {
            let should_swap = matches!(
                (
                    primary.window_duration_minutes,
                    secondary.window_duration_minutes
                ),
                (Some(primary_minutes), Some(secondary_minutes))
                    if primary_minutes > secondary_minutes
            );
            if should_swap {
                UsageData {
                    session: secondary,
                    weekly: primary,
                }
            } else {
                UsageData {
                    session: primary,
                    weekly: secondary,
                }
            }
        }
        (Some(primary), None) => usage_data_from_single_window(primary),
        (None, Some(secondary)) => usage_data_from_single_window(secondary),
        (None, None) => UsageData::default(),
    }
}

fn usage_data_from_single_window(window: UsageSection) -> UsageData {
    if window
        .window_duration_minutes
        .is_some_and(|minutes| minutes >= 24 * 60)
    {
        UsageData {
            weekly: window,
            ..UsageData::default()
        }
    } else {
        UsageData {
            session: window,
            ..UsageData::default()
        }
    }
}

fn usage_section_from_window(window: RateLimitWindow) -> UsageSection {
    UsageSection {
        available: true,
        percentage: window.used_percent.clamp(0.0, 100.0),
        resets_at: unix_to_system_time(window.resets_at),
        window_duration_minutes: window
            .window_duration_mins
            .and_then(|minutes| u64::try_from(minutes).ok()),
    }
}

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64))
}

pub fn credential_watch_signature() -> String {
    codex_auth_path()
        .map(|path| credential_watch_signature_for_path(&path))
        .unwrap_or_else(|| "codex-auth|unresolved".to_string())
}

fn codex_auth_path() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        return Some(codex_home.join("auth.json"));
    }
    Some(user_home_dir()?.join(".codex").join("auth.json"))
}

fn credential_watch_signature_for_path(path: &Path) -> String {
    let key = format!("codex-auth:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_nanos())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_codex_rate_limit_shape() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 25.0,
                    "windowDurationMins": 300,
                    "resetsAt": 1_800_000_000
                },
                "secondary": {
                    "usedPercent": 42.0,
                    "windowDurationMins": 10_080,
                    "resetsAt": 1_800_500_000
                }
            }
        }))
        .expect("valid rate-limit response");

        assert_eq!(usage.session.percentage, 25.0);
        assert_eq!(usage.session.window_duration_minutes, Some(300));
        assert_eq!(usage.weekly.percentage, 42.0);
        assert_eq!(usage.weekly.window_duration_minutes, Some(10_080));
    }

    #[test]
    fn prefers_codex_bucket_from_multi_bucket_response() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": { "usedPercent": 99.0 }
            },
            "rateLimitsByLimitId": {
                "codex_other": {
                    "primary": { "usedPercent": 77.0 }
                },
                "codex": {
                    "primary": { "usedPercent": 12.0 }
                }
            }
        }))
        .expect("valid multi-bucket response");

        assert_eq!(usage.session.percentage, 12.0);
    }

    #[test]
    fn falls_back_when_codex_bucket_has_no_windows() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": { "usedPercent": 31 }
            },
            "rateLimitsByLimitId": {
                "codex": {}
            }
        }))
        .expect("valid backward-compatible response");

        assert_eq!(usage.session.percentage, 31.0);
    }

    #[test]
    fn rejects_response_without_quota_windows() {
        assert_eq!(
            usage_from_rate_limits_value(json!({ "rateLimits": {} })).unwrap_err(),
            PollError::RequestFailed
        );
    }

    #[test]
    fn accepts_null_multi_bucket_view() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": { "usedPercent": 18 },
                "secondary": null
            },
            "rateLimitsByLimitId": null
        }))
        .expect("valid backward-compatible response");

        assert_eq!(usage.session.percentage, 18.0);
        assert_eq!(usage.weekly.percentage, 0.0);
    }

    #[test]
    fn places_a_single_long_window_in_the_weekly_row() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 16,
                    "windowDurationMins": 10_080,
                    "resetsAt": 1_800_000_000
                },
                "secondary": null
            }
        }))
        .expect("valid single-window response");

        assert!(!usage.session.available);
        assert!(usage.weekly.available);
        assert_eq!(usage.weekly.percentage, 16.0);
        assert_eq!(usage.weekly.window_duration_minutes, Some(10_080));
    }

    #[test]
    fn orders_two_windows_by_duration() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 40,
                    "windowDurationMins": 10_080
                },
                "secondary": {
                    "usedPercent": 20,
                    "windowDurationMins": 300
                }
            }
        }))
        .expect("valid reversed-window response");

        assert_eq!(usage.session.percentage, 20.0);
        assert_eq!(usage.session.window_duration_minutes, Some(300));
        assert_eq!(usage.weekly.percentage, 40.0);
        assert_eq!(usage.weekly.window_duration_minutes, Some(10_080));
    }

    #[test]
    fn ignores_negative_window_duration() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 18,
                    "windowDurationMins": -1
                }
            }
        }))
        .expect("valid rate-limit response");

        assert_eq!(usage.session.window_duration_minutes, None);
    }

    #[test]
    fn clamps_percentage_to_display_range() {
        let usage = usage_from_rate_limits_value(json!({
            "rateLimits": {
                "primary": { "usedPercent": 140 },
                "secondary": { "usedPercent": -8 }
            }
        }))
        .expect("valid rate-limit response");

        assert_eq!(usage.session.percentage, 100.0);
        assert_eq!(usage.weekly.percentage, 0.0);
    }

    #[test]
    fn classifies_authentication_errors() {
        assert_eq!(
            classify_rpc_error("codex account authentication required to read rate limits"),
            PollError::NoCredentials
        );
        assert_eq!(classify_rpc_error("Unauthorized"), PollError::AuthRequired);
        assert_eq!(classify_rpc_error("token expired"), PollError::TokenExpired);
    }

    #[test]
    fn resolves_active_vscode_extension_from_absolute_location() {
        let root = Path::new(r"C:\Users\Test\.vscode\extensions");
        let extension = active_openai_extension_path(
            root,
            &json!({
                "identifier": { "id": "OpenAI.ChatGPT" },
                "location": {
                    "fsPath": r"C:\Users\Test\.vscode\extensions\openai.chatgpt-1.2.3-win32-x64"
                }
            }),
        );

        assert_eq!(
            extension,
            Some(PathBuf::from(
                r"C:\Users\Test\.vscode\extensions\openai.chatgpt-1.2.3-win32-x64"
            ))
        );
    }

    #[test]
    fn resolves_active_vscode_extension_from_relative_location() {
        let root = Path::new(r"C:\Users\Test\.vscode\extensions");
        let extension = active_openai_extension_path(
            root,
            &json!({
                "identifier": { "id": "openai.chatgpt" },
                "relativeLocation": "openai.chatgpt-1.2.3-win32-x64"
            }),
        );

        assert_eq!(extension, Some(root.join("openai.chatgpt-1.2.3-win32-x64")));
    }

    #[test]
    fn ignores_unrelated_vscode_extension_manifest_entry() {
        let root = Path::new(r"C:\Users\Test\.vscode\extensions");
        assert_eq!(
            active_openai_extension_path(
                root,
                &json!({
                    "identifier": { "id": "example.other" },
                    "relativeLocation": "example.other-1.0.0"
                }),
            ),
            None
        );
    }

    #[test]
    fn deduplicates_windows_paths_with_mixed_case_and_separators() {
        assert!(paths_equal(
            Path::new(r"C:\Users\Test\.vscode\extensions"),
            Path::new("c:/users/test/.vscode/extensions")
        ));
    }
}
