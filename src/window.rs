use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, GetCurrentThreadId, WaitForSingleObject,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_CHECKED, ODS_DISABLED, ODS_GRAYED, ODS_SELECTED,
    ODT_MENU,
};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, ReleaseCapture, SetCapture, HKL,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ExtractIconExW, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::menu_popup::{self, MenuSnapshot};
use crate::models::{AppUsageData, UsageData};
use crate::native_interop::{
    self, Color, TIMER_COUNTDOWN, TIMER_POLL, TIMER_RESET_POLL, TIMER_TASKBAR_GEOMETRY,
    TIMER_UPDATE_CHECK, WM_APP_TRAY, WM_APP_TRAY_MOVED, WM_APP_USAGE_UPDATED,
};
use crate::poller;
use crate::theme::{self, ThemeMode};
use crate::tray_icon;
use crate::updater::{self, InstallChannel, ReleaseDescriptor, UpdateCheckResult};

/// Wrapper to make HWND sendable across threads (safe for PostMessage usage)
#[derive(Clone, Copy)]
struct SendHwnd(isize);

unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    taskbar_hwnd: Option<HWND>,
    tray_notify_hwnd: Option<HWND>,
    win_event_hook: Option<HWINEVENTHOOK>,
    is_dark: bool,
    system_dark: bool,
    theme_mode: ThemeMode,
    embedded: bool,
    geometry_ready: bool,
    language_override: Option<LanguageId>,
    language: LanguageId,
    install_channel: InstallChannel,

    session_percent: f64,
    session_text: String,
    weekly_percent: f64,
    weekly_text: String,
    codex_session_percent: f64,
    codex_session_text: String,
    codex_weekly_percent: f64,
    codex_weekly_text: String,
    antigravity_session_percent: f64,
    antigravity_session_text: String,
    antigravity_weekly_percent: f64,
    antigravity_weekly_text: String,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,

    data: Option<AppUsageData>,

    poll_interval_ms: u32,
    retry_count: u32,
    force_notify_auth_error: bool,
    auth_notifications: ProviderAuthNotifications,
    auth_error_paused_polling: bool,
    auth_watch_mode: poller::CredentialWatchMode,
    auth_watch_snapshot: poller::CredentialWatchSnapshot,
    last_poll_ok: bool,
    update_status: UpdateStatus,
    last_update_check_unix: Option<u64>,

    taskbar_index: usize,
    tray_offset: i32,
    dragging: bool,
    drag_start_mouse_x: i32,
    drag_start_client_x: i32,
    drag_start_offset: i32,

    widget_visible: bool,
}

#[derive(Clone, Debug)]
enum UpdateStatus {
    Idle,
    Checking,
    Applying,
    UpToDate,
    Available(ReleaseDescriptor),
}

#[derive(Default)]
struct ProviderAuthNotifications {
    claude_code: bool,
    codex: bool,
    antigravity: bool,
}

#[derive(Clone, Copy)]
struct AuthBalloon {
    kind: tray_icon::TrayIconKind,
    title: &'static str,
    body: &'static str,
}

const RETRY_BASE_MS: u32 = 30_000; // 30 seconds

const POLL_1_MIN: u32 = 60_000;
const POLL_5_MIN: u32 = 300_000;
const POLL_15_MIN: u32 = 900_000;
const POLL_1_HOUR: u32 = 3_600_000;

// Menu item IDs for update frequency
pub(crate) const IDM_FREQ_1MIN: u16 = 10;
pub(crate) const IDM_FREQ_5MIN: u16 = 11;
pub(crate) const IDM_FREQ_15MIN: u16 = 12;
pub(crate) const IDM_FREQ_1HOUR: u16 = 13;
pub(crate) const IDM_START_WITH_WINDOWS: u16 = 20;
pub(crate) const IDM_RESET_POSITION: u16 = 30;
pub(crate) const IDM_VERSION_ACTION: u16 = 31;
pub(crate) const IDM_LANG_SYSTEM: u16 = 40;
const IDM_LANG_ENGLISH: u16 = 41;
const IDM_LANG_DUTCH: u16 = 42;
const IDM_LANG_SPANISH: u16 = 43;
const IDM_LANG_FRENCH: u16 = 44;
const IDM_LANG_GERMAN: u16 = 45;
const IDM_LANG_JAPANESE: u16 = 46;
const IDM_LANG_KOREAN: u16 = 47;
const IDM_LANG_TRADITIONAL_CHINESE: u16 = 48;
const IDM_LANG_RUSSIAN: u16 = 49;
const IDM_LANG_PORTUGUESE_BRAZIL: u16 = 50;
const IDM_LANG_SIMPLIFIED_CHINESE: u16 = 51;
pub(crate) const IDM_MODEL_CLAUDE_CODE: u16 = 60;
pub(crate) const IDM_MODEL_CODEX: u16 = 61;
pub(crate) const IDM_MODEL_ANTIGRAVITY: u16 = 62;
pub(crate) const IDM_THEME_SYSTEM: u16 = 70;
pub(crate) const IDM_THEME_LIGHT: u16 = 71;
pub(crate) const IDM_THEME_DARK: u16 = 72;

const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_DPICHANGED_AFTERPARENT_MSG: u32 = 0x02E3;
const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 2;

/// How often the watchdog thread polls for an explorer.exe restart (which
/// recreates the taskbar and wipes our tray-icon registration).
const TASKBAR_WATCH_INTERVAL_SECS: u64 = 2;

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);
static MENU_BACKGROUND_COLOR: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

#[derive(PartialEq, Eq)]
struct InputSnapshot {
    layouts: [usize; 16],
    count: i32,
    foreground_layout: usize,
}

static INPUT_SNAPSHOT: Mutex<Option<InputSnapshot>> = Mutex::new(None);

fn log_input_state(reason: &str, force: bool) {
    if !diagnose::is_enabled() {
        return;
    }
    unsafe {
        let mut handles = [HKL::default(); 16];
        let count = GetKeyboardLayoutList(Some(&mut handles));
        let mut layouts = [0usize; 16];
        for (slot, handle) in layouts.iter_mut().zip(handles) {
            *slot = handle.0 as usize;
        }
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let foreground_layout = GetKeyboardLayout(foreground_thread).0 as usize;
        let snapshot = InputSnapshot {
            layouts,
            count,
            foreground_layout,
        };
        let mut previous = INPUT_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
        if !force && previous.as_ref() == Some(&snapshot) {
            return;
        }
        let visible = count.max(0).min(layouts.len() as i32) as usize;
        let values: Vec<String> = layouts[..visible]
            .iter()
            .map(|layout| format!("{layout:08X}"))
            .collect();
        diagnose::log(format!(
            "input-state {reason}: count={count} layouts=[{}] foreground={:?} thread={foreground_thread} foreground_hkl={foreground_layout:08X} own_hkl={:08X}",
            values.join(","),
            foreground.0,
            GetKeyboardLayout(0).0 as usize,
        ));
        *previous = Some(snapshot);
    }
}

/// Scale a base pixel value (designed at 96 DPI) to the current DPI.
fn sc(px: i32) -> i32 {
    let dpi = CURRENT_DPI.load(Ordering::Relaxed);
    (px as f64 * dpi as f64 / 96.0).round() as i32
}

fn lerp_channel(start: u8, end: u8, amount: f64) -> u8 {
    (start as f64 + (end as f64 - start as f64) * amount.clamp(0.0, 1.0)).round() as u8
}

fn lerp_color(start: Color, end: Color, amount: f64) -> Color {
    Color::new(
        lerp_channel(start.r, end.r, amount),
        lerp_channel(start.g, end.g, amount),
        lerp_channel(start.b, end.b, amount),
    )
}

#[derive(Clone, Copy)]
struct UsagePalette {
    start: Color,
    middle: Color,
    end: Color,
    track: Color,
}

impl UsagePalette {
    fn segment_color(self, segment: i32, segment_count: i32) -> Color {
        let amount = if segment_count > 1 {
            segment as f64 / (segment_count - 1) as f64
        } else {
            0.0
        };
        if amount <= 0.5 {
            lerp_color(self.start, self.middle, amount * 2.0)
        } else {
            lerp_color(self.middle, self.end, (amount - 0.5) * 2.0)
        }
    }
}

/// Re-query the monitor DPI for our window and update the cached value.
/// Uses GetDpiForWindow which returns the live DPI (unlike GetDpiForSystem
/// which is cached at process startup and never changes).
fn refresh_dpi() {
    let hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.hwnd.to_hwnd())
    };
    if let Some(hwnd) = hwnd {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            CURRENT_DPI.store(dpi, Ordering::Relaxed);
        }
    }
}

/// Spacing below which two relaunches are treated as a storm (e.g. explorer.exe
/// crash-looping); when detected we back off instead of spawning in a tight loop.
const RELAUNCH_THROTTLE_SECS: u64 = 10;
const RELAUNCH_BACKOFF_SECS: u64 = 30;
/// Environment flag set on a relaunched child so it waits for the previous
/// instance's single-instance mutex instead of exiting immediately.
const ENV_RELAUNCH: &str = "CODEX_USAGE_MONITOR_RELAUNCH";
/// Unix timestamp (seconds) of the relaunch that spawned this process, passed to
/// the child so it can detect a relaunch storm.
const ENV_LAST_RELAUNCH_UNIX: &str = "CODEX_USAGE_MONITOR_LAST_RELAUNCH_UNIX";

/// Relaunch the widget as a fresh process after explorer.exe has restarted.
///
/// When the shell restarts it destroys our embedded child window outright (the
/// window is gone, not merely orphaned - `IsWindow` returns false) and leaves
/// the UI thread parked in `GetMessage` with no window to recreate in place.
/// Spawning a clean new process - which re-embeds into the freshly created
/// taskbar - and exiting this one is the robust recovery. The child is flagged
/// via `ENV_RELAUNCH` so it waits for this instance's single-instance mutex to
/// be released before taking over (see the guard in `run`).
fn relaunch_self() {
    // Back off if we are relaunching very soon after the relaunch that spawned
    // us: that signals the shell is crash-looping, not a one-off restart.
    let now = now_unix_secs();
    let last = std::env::var(ENV_LAST_RELAUNCH_UNIX)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if last != 0 && now.saturating_sub(last) < RELAUNCH_THROTTLE_SECS {
        diagnose::log("relaunch storm detected; backing off before relaunching");
        std::thread::sleep(Duration::from_secs(RELAUNCH_BACKOFF_SECS));
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            diagnose::log_error("watchdog: unable to resolve current executable", error);
            return;
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::process::Command::new(exe)
        .args(&args)
        .env(ENV_RELAUNCH, "1")
        .env(ENV_LAST_RELAUNCH_UNIX, now.to_string())
        .spawn()
    {
        Ok(_) => {
            diagnose::log("watchdog: relaunched fresh instance, exiting old one");
            std::process::exit(0);
        }
        Err(error) => {
            diagnose::log_error("watchdog: unable to spawn relaunched instance", error);
        }
    }
}

/// Detect explorer.exe restarts and recover from them.
///
/// Once explorer destroys the taskbar, our embedded child window is destroyed
/// and the UI message loop is dead, so recovery cannot happen in-process. This
/// dedicated thread (independent of the dead message loop) polls the taskbar
/// handle and, when it changes, relaunches the widget as a fresh process.
fn spawn_taskbar_watchdog() {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TASKBAR_WATCH_INTERVAL_SECS));
        let stored = {
            let state = lock_state();
            state.as_ref().and_then(|s| s.taskbar_hwnd)
        };
        // Only relevant once we have embedded into a taskbar at least once.
        let Some(old) = stored else {
            continue;
        };
        let taskbars = native_interop::find_taskbars();
        if !taskbars.is_empty() && !taskbars.iter().any(|taskbar| taskbar.hwnd == old) {
            let new = taskbars[0].hwnd;
            diagnose::log(format!(
                "watchdog: taskbar changed old={:?} new={:?} -> relaunching",
                old.0, new.0
            ));
            relaunch_self();
        }
    });
}

fn load_embedded_app_icons() -> (HICON, HICON) {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return (HICON::default(), HICON::default());
        }

        let mut large_icon = HICON::default();
        let mut small_icon = HICON::default();
        let extracted = ExtractIconExW(
            PCWSTR::from_raw(exe_buf.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        );

        if extracted == 0 {
            (HICON::default(), HICON::default())
        } else {
            (large_icon, small_icon)
        }
    }
}

unsafe impl Send for AppState {}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

/// Lock STATE safely, recovering from poisoned mutex
fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn settings_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("CodexUsageMonitor")
        .join("settings.json")
}

#[derive(Debug, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    tray_offset: i32,
    #[serde(default)]
    taskbar_index: usize,
    #[serde(default = "default_poll_interval")]
    poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default)]
    theme: ThemeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_update_check_unix: Option<u64>,
    #[serde(default = "default_widget_visible")]
    widget_visible: bool,
    #[serde(default = "default_show_claude_code")]
    show_claude_code: bool,
    #[serde(default = "default_show_codex")]
    show_codex: bool,
    #[serde(default = "default_show_antigravity")]
    show_antigravity: bool,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            tray_offset: 0,
            taskbar_index: 0,
            poll_interval_ms: default_poll_interval(),
            language: None,
            theme: ThemeMode::System,
            last_update_check_unix: None,
            widget_visible: true,
            show_claude_code: false,
            show_codex: true,
            show_antigravity: false,
        }
    }
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}

fn default_widget_visible() -> bool {
    true
}

fn default_show_claude_code() -> bool {
    false
}

fn default_show_codex() -> bool {
    true
}

fn default_show_antigravity() -> bool {
    false
}

fn load_settings() -> SettingsFile {
    let content = match std::fs::read_to_string(settings_path()) {
        Ok(c) => c,
        Err(_) => return SettingsFile::default(),
    };
    let mut settings: SettingsFile = serde_json::from_str(&content).unwrap_or_default();
    if !settings.show_claude_code && !settings.show_codex && !settings.show_antigravity {
        settings.show_codex = true;
    }
    settings
}

fn save_settings(settings: &SettingsFile) {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

fn save_state_settings() {
    let state = lock_state();
    if let Some(s) = state.as_ref() {
        save_settings(&SettingsFile {
            tray_offset: s.tray_offset,
            taskbar_index: s.taskbar_index,
            poll_interval_ms: s.poll_interval_ms,
            language: s
                .language_override
                .map(|language| language.code().to_string()),
            theme: s.theme_mode,
            last_update_check_unix: s.last_update_check_unix,
            widget_visible: s.widget_visible,
            show_claude_code: s.show_claude_code,
            show_codex: s.show_codex,
            show_antigravity: s.show_antigravity,
        });
    }
}

fn badge_percentage(data: &UsageData) -> Option<f64> {
    if data.session.available {
        Some(data.session.percentage)
    } else if data.weekly.available {
        Some(data.weekly.percentage)
    } else {
        None
    }
}

fn tray_icon_data_from_state() -> Vec<tray_icon::TrayIconData> {
    let state = lock_state();
    match state.as_ref() {
        Some(s) if s.last_poll_ok => {
            let mut icons = Vec::new();
            if s.show_claude_code {
                icons.push(tray_icon::TrayIconData {
                    kind: tray_icon::TrayIconKind::Claude,
                    percent: s
                        .data
                        .as_ref()
                        .and_then(|data| data.claude_code.as_ref())
                        .and_then(badge_percentage),
                    tooltip: format!(
                        "{} 5h: {} | 7d: {}",
                        s.language.strings().claude_code_model,
                        s.session_text,
                        s.weekly_text
                    ),
                });
            }
            if s.show_codex {
                icons.push(tray_icon::TrayIconData {
                    kind: tray_icon::TrayIconKind::Codex,
                    percent: s
                        .data
                        .as_ref()
                        .and_then(|data| data.codex.as_ref())
                        .and_then(badge_percentage),
                    tooltip: format!(
                        "{} 5h: {} | 7d: {}",
                        s.language.strings().codex_model,
                        s.codex_session_text,
                        s.codex_weekly_text
                    ),
                });
            }
            if s.show_antigravity {
                icons.push(tray_icon::TrayIconData {
                    kind: tray_icon::TrayIconKind::Antigravity,
                    percent: s
                        .data
                        .as_ref()
                        .and_then(|data| data.antigravity.as_ref())
                        .and_then(badge_percentage),
                    tooltip: format!(
                        "{} 5h: {} | 7d: {}",
                        s.language.strings().antigravity_model,
                        s.antigravity_session_text,
                        s.antigravity_weekly_text
                    ),
                });
            }
            icons
        }
        Some(s) => {
            let mut icons = Vec::new();
            if s.show_claude_code {
                icons.push(tray_icon::TrayIconData {
                    kind: tray_icon::TrayIconKind::Claude,
                    percent: None,
                    tooltip: s.language.strings().claude_code_model.to_string(),
                });
            }
            if s.show_codex {
                icons.push(tray_icon::TrayIconData {
                    kind: tray_icon::TrayIconKind::Codex,
                    percent: None,
                    tooltip: s.language.strings().codex_window_title.to_string(),
                });
            }
            if s.show_antigravity {
                icons.push(tray_icon::TrayIconData {
                    kind: tray_icon::TrayIconKind::Antigravity,
                    percent: None,
                    tooltip: s.language.strings().antigravity_window_title.to_string(),
                });
            }
            icons
        }
        None => Vec::new(),
    }
}

fn sync_tray_icons(hwnd: HWND) {
    let icons = tray_icon_data_from_state();
    tray_icon::sync(hwnd, &icons);
}

fn toggle_widget_visibility(hwnd: HWND) {
    let new_visible = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.widget_visible = !s.widget_visible;
            s.widget_visible
        } else {
            return;
        }
    };
    save_state_settings();
    unsafe {
        if new_visible {
            position_at_taskbar();
            render_layered();
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn attach_to_taskbar(hwnd: HWND, requested_index: usize) -> bool {
    let taskbars = native_interop::find_taskbars();
    if taskbars.is_empty() {
        diagnose::log("taskbar not found; using fallback popup window");
        return false;
    }

    let index = requested_index.min(taskbars.len().saturating_sub(1));
    let taskbar = taskbars[index];
    diagnose::log(format!(
        "taskbar selected index={index} count={} hwnd={:?} rect=({}, {}, {}, {})",
        taskbars.len(),
        taskbar.hwnd,
        taskbar.rect.left,
        taskbar.rect.top,
        taskbar.rect.right,
        taskbar.rect.bottom
    ));

    let was_embedded = {
        let state = lock_state();
        state.as_ref().is_some_and(|s| s.embedded)
    };
    let embedded = native_interop::embed_in_taskbar(hwnd, taskbar.hwnd);
    if !embedded && was_embedded {
        diagnose::log("taskbar move rejected; keeping previous parent");
        return false;
    }
    if !embedded {
        diagnose::log("taskbar embedding unavailable; keeping independent popup");
    }

    let old_hook = {
        let mut state = lock_state();
        state.as_mut().and_then(|s| s.win_event_hook.take())
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook);
    }

    let tray_notify = native_interop::find_child_window(taskbar.hwnd, "TrayNotifyWnd");
    if tray_notify.is_some() {
        diagnose::log("TrayNotifyWnd found");
    } else {
        diagnose::log("TrayNotifyWnd not found");
    }

    let hook = tray_notify.and_then(|tray_hwnd| {
        let thread_id = native_interop::get_window_thread_id(tray_hwnd);
        native_interop::set_tray_event_hook(thread_id, on_tray_location_changed)
    });
    if hook.is_some() {
        diagnose::log("tray event hook installed");
    } else {
        diagnose::log("tray event hook could not be installed");
    }

    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.taskbar_hwnd = Some(taskbar.hwnd);
        s.tray_notify_hwnd = tray_notify;
        s.win_event_hook = hook;
        s.taskbar_index = index;
        s.embedded = embedded;
    }
    embedded
}

fn taskbar_at_point(pt: POINT) -> Option<(usize, native_interop::TaskbarWindow)> {
    native_interop::find_taskbars()
        .into_iter()
        .enumerate()
        .find(|(_, taskbar)| {
            pt.x >= taskbar.rect.left
                && pt.x < taskbar.rect.right
                && pt.y >= taskbar.rect.top
                && pt.y < taskbar.rect.bottom
        })
}

fn tray_left_for_taskbar(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            if tray_rect.right > tray_rect.left
                && tray_rect.left > taskbar_rect.left
                && tray_rect.left < taskbar_rect.right
            {
                return tray_rect.left;
            }
        }
    }
    taskbar_rect.left
}

fn clamp_offset_for_taskbar(taskbar_hwnd: HWND, taskbar_rect: RECT, offset: i32) -> i32 {
    let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
    clamp_widget_offset(taskbar_rect.left, tray_left, total_widget_width(), offset)
}

fn clamp_widget_offset(taskbar_left: i32, tray_left: i32, widget_width: i32, offset: i32) -> i32 {
    let max_offset = (tray_left - taskbar_left - widget_width).max(0);
    offset.clamp(0, max_offset)
}

fn offset_for_drop_point(
    taskbar_hwnd: HWND,
    taskbar_rect: RECT,
    pt: POINT,
    drag_start_client_x: i32,
) -> i32 {
    let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
    let desired_left = pt.x - taskbar_rect.left - drag_start_client_x;
    let offset = tray_left - taskbar_rect.left - total_widget_width() - desired_left;
    clamp_offset_for_taskbar(taskbar_hwnd, taskbar_rect, offset)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn update_check_interval() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

fn auto_update_check_due(last_update_check_unix: Option<u64>) -> bool {
    let Some(last_update_check_unix) = last_update_check_unix else {
        return true;
    };

    now_unix_secs().saturating_sub(last_update_check_unix) >= update_check_interval().as_secs()
}

fn schedule_auto_update_check(hwnd: HWND) {
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };

        if auto_update_check_due(s.last_update_check_unix) {
            None
        } else {
            let elapsed = now_unix_secs().saturating_sub(s.last_update_check_unix.unwrap_or(0));
            let remaining_secs = update_check_interval().as_secs().saturating_sub(elapsed);
            Some((remaining_secs.saturating_mul(1000)).min(u32::MAX as u64) as u32)
        }
    };

    unsafe {
        let _ = KillTimer(hwnd, TIMER_UPDATE_CHECK);
        if let Some(delay_ms) = delay_ms {
            SetTimer(hwnd, TIMER_UPDATE_CHECK, delay_ms.max(1), None);
        }
    }
}

fn refresh_usage_texts(state: &mut AppState) {
    if !state.last_poll_ok {
        return;
    }

    let strings = state.language.strings();
    let Some(data) = state.data.as_ref() else {
        return;
    };

    if let Some(claude_code) = data.claude_code.as_ref() {
        state.session_text = poller::format_line(&claude_code.session, strings);
        state.weekly_text = poller::format_line(&claude_code.weekly, strings);
    } else if state.show_claude_code {
        state.session_text = "!".to_string();
        state.weekly_text = "!".to_string();
    }

    if let Some(codex) = data.codex.as_ref() {
        state.codex_session_text = poller::format_line(&codex.session, strings);
        state.codex_weekly_text = poller::format_line(&codex.weekly, strings);
    } else if state.show_codex {
        state.codex_session_text = "!".to_string();
        state.codex_weekly_text = "!".to_string();
    }

    if let Some(antigravity) = data.antigravity.as_ref() {
        state.antigravity_session_text = poller::format_line(&antigravity.session, strings);
        state.antigravity_weekly_text = poller::format_line(&antigravity.weekly, strings);
    } else if state.show_antigravity {
        state.antigravity_session_text = "!".to_string();
        state.antigravity_weekly_text = "!".to_string();
    }
}

fn set_window_title(hwnd: HWND, strings: Strings) {
    unsafe {
        let title = native_interop::wide_str(strings.window_title);
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(title.as_ptr()));
    }
}

fn show_info_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn show_update_prompt(hwnd: HWND, strings: Strings, release: &ReleaseDescriptor) -> bool {
    let message = strings
        .update_prompt_now
        .replace("{version}", &release.latest_version);

    unsafe {
        let title_wide = native_interop::wide_str(strings.update_available);
        let message_wide = native_interop::wide_str(&message);
        MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn apply_language_to_state(state: &mut AppState, language_override: Option<LanguageId>) {
    state.language_override = language_override;
    state.language = localization::resolve_language(language_override);
    set_window_title(state.hwnd.to_hwnd(), state.language.strings());
    refresh_usage_texts(state);
}

fn update_language_change() -> bool {
    let mut state = lock_state();
    let Some(app_state) = state.as_mut() else {
        return false;
    };

    if app_state.language_override.is_some() {
        return false;
    }

    let new_language = localization::detect_system_language();
    if new_language == app_state.language {
        return false;
    }

    apply_language_to_state(app_state, None);
    true
}

fn version_action_label(
    strings: Strings,
    language: LanguageId,
    install_channel: InstallChannel,
    status: &UpdateStatus,
) -> String {
    let current = env!("CARGO_PKG_VERSION");
    match status {
        UpdateStatus::Idle => format!("v{current} - {}", strings.check_for_updates),
        UpdateStatus::Checking => format!("v{current} - {}", strings.checking_for_updates),
        UpdateStatus::Applying => format!("v{current} - {}", strings.applying_update),
        UpdateStatus::UpToDate => format!("v{current} - {}", strings.up_to_date_short),
        UpdateStatus::Available(release) => match install_channel {
            InstallChannel::Portable => {
                format!(
                    "v{current} - {} v{}",
                    strings.update_to, release.latest_version
                )
            }
            InstallChannel::Winget => format!(
                "v{current} - {} v{}",
                localization::update_via_winget(language),
                release.latest_version
            ),
        },
    }
}

fn begin_update_check(hwnd: HWND, interactive: bool) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let (strings, install_channel) = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            if interactive {
                show_info_message(
                    hwnd,
                    app_state.language.strings().updates,
                    app_state.language.strings().update_in_progress,
                );
            }
            return;
        }

        app_state.update_status = UpdateStatus::Checking;
        (app_state.language.strings(), app_state.install_channel)
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        let checked_at = now_unix_secs();
        match updater::check_for_updates() {
            Ok(UpdateCheckResult::UpToDate) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::UpToDate;
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    show_info_message(hwnd, strings.updates, strings.up_to_date);
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
            Ok(UpdateCheckResult::Available(release)) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release.clone());
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive && show_update_prompt(hwnd, strings, &release) {
                    match install_channel {
                        InstallChannel::Portable => begin_update_apply(hwnd, release),
                        InstallChannel::Winget => begin_winget_update(hwnd),
                    }
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Idle;
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    let message = format!("{}.\n\n{}", strings.update_failed, error);
                    show_error_message(hwnd, strings.updates, &message);
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

fn begin_update_apply(hwnd: HWND, release: ReleaseDescriptor) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let strings = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            show_info_message(
                hwnd,
                app_state.language.strings().updates,
                app_state.language.strings().update_in_progress,
            );
            return;
        }

        app_state.update_status = UpdateStatus::Applying;
        app_state.language.strings()
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        match updater::begin_self_update(&release) {
            Ok(()) => unsafe {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release);
                    }
                }
                let message = format!("{}.\n\n{}", strings.update_failed, error);
                show_error_message(hwnd, strings.updates, &message);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

fn begin_winget_update(hwnd: HWND) {
    let strings = {
        let state = lock_state();
        state.as_ref().map(|s| s.language.strings())
    }
    .unwrap_or(LanguageId::English.strings());

    match updater::begin_winget_update() {
        Ok(()) => unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Err(error) => {
            let message = format!("{}.\n\n{}", strings.update_failed, error);
            show_error_message(hwnd, strings.updates, &message);
        }
    }
}

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "CodexUsageMonitor";

/// Returns true only if the startup registry value points to this executable.
fn is_startup_enabled() -> bool {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if result.is_err() {
            return false;
        }

        // Query the size of the value
        let mut data_size: u32 = 0;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut data_size),
        );
        if result.is_err() || data_size == 0 {
            let _ = RegCloseKey(hkey);
            return false;
        }

        // Read the value
        let mut buf = vec![0u8; data_size as usize];
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut data_size),
        );
        let _ = RegCloseKey(hkey);
        if result.is_err() {
            return false;
        }

        // Convert the registry value (UTF-16) to a string
        let wide_slice =
            std::slice::from_raw_parts(buf.as_ptr() as *const u16, data_size as usize / 2);
        let reg_value = String::from_utf16_lossy(wide_slice)
            .trim_end_matches('\0')
            .to_string();

        // Get the current executable path
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return false;
        }
        let current_exe = String::from_utf16_lossy(&exe_buf[..len]);

        // Case-insensitive comparison (Windows paths are case-insensitive)
        reg_value.eq_ignore_ascii_case(&current_exe)
    }
}

fn set_startup_enabled(enable: bool) {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if result.is_err() {
            return;
        }

        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        if enable {
            let mut exe_buf = [0u16; 260];
            let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
            if len > 0 {
                // Write the wide string including null terminator
                let byte_len = ((len + 1) * 2) as u32;
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR::from_raw(key_name.as_ptr()),
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        exe_buf.as_ptr() as *const u8,
                        byte_len as usize,
                    )),
                );
            }
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
        }

        let _ = RegCloseKey(hkey);
    }
}

// Dimensions matching the C# version
const SEGMENT_W: i32 = 10;
const SEGMENT_H: i32 = 13;
const SEGMENT_GAP: i32 = 1;
const SEGMENT_COUNT: i32 = 10;
const CORNER_RADIUS: i32 = 2;

const LEFT_DIVIDER_W: i32 = 3;
const DIVIDER_RIGHT_MARGIN: i32 = 10;
const LABEL_WIDTH: i32 = 18;
const LABEL_RIGHT_MARGIN: i32 = 10;
const BAR_RIGHT_MARGIN: i32 = 4;
const TEXT_WIDTH: i32 = 62;
const MODEL_RIGHT_MARGIN: i32 = 3;
const RIGHT_MARGIN: i32 = 1;
const WIDGET_HEIGHT: i32 = 46;

fn is_drag_handle_point(client_x: i32, client_y: i32) -> bool {
    let divider_h = sc(25);
    let divider_top = (sc(WIDGET_HEIGHT) - divider_h) / 2;
    client_x >= 0
        && client_x < sc(LEFT_DIVIDER_W)
        && client_y >= divider_top
        && client_y < divider_top + divider_h
}

fn cursor_is_on_drag_handle(hwnd: HWND) -> bool {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() || !ScreenToClient(hwnd, &mut pt).as_bool() {
            return false;
        }
        is_drag_handle_point(pt.x, pt.y)
    }
}

fn active_model_count(show_claude_code: bool, show_codex: bool, show_antigravity: bool) -> i32 {
    (show_claude_code as i32 + show_codex as i32 + show_antigravity as i32).max(1)
}

fn toggle_model_visibility(
    show_claude_code: &mut bool,
    show_codex: &mut bool,
    show_antigravity: &mut bool,
    id: u16,
) -> bool {
    let active_models = active_model_count(*show_claude_code, *show_codex, *show_antigravity);
    match id {
        IDM_MODEL_CLAUDE_CODE if !*show_claude_code || active_models > 1 => {
            *show_claude_code = !*show_claude_code;
            true
        }
        IDM_MODEL_CODEX if !*show_codex || active_models > 1 => {
            *show_codex = !*show_codex;
            true
        }
        IDM_MODEL_ANTIGRAVITY if !*show_antigravity || active_models > 1 => {
            *show_antigravity = !*show_antigravity;
            true
        }
        _ => false,
    }
}

fn row_bar_segment_count(active_models: i32) -> i32 {
    match active_models {
        1 => SEGMENT_COUNT,
        2 => 5,
        _ => 4,
    }
}

fn total_widget_width_for(active_models: i32) -> i32 {
    let bar_segments = row_bar_segment_count(active_models);
    let model_width = (sc(SEGMENT_W) + sc(SEGMENT_GAP)) * bar_segments - sc(SEGMENT_GAP)
        + sc(BAR_RIGHT_MARGIN)
        + sc(TEXT_WIDTH);

    sc(LEFT_DIVIDER_W)
        + sc(DIVIDER_RIGHT_MARGIN)
        + sc(LABEL_WIDTH)
        + sc(LABEL_RIGHT_MARGIN)
        + model_width * active_models
        + sc(MODEL_RIGHT_MARGIN) * (active_models - 1)
        + sc(RIGHT_MARGIN)
}

fn total_widget_width_for_state(state: &AppState) -> i32 {
    total_widget_width_for(active_model_count(
        state.show_claude_code,
        state.show_codex,
        state.show_antigravity,
    ))
}

fn total_widget_width() -> i32 {
    let active_models = {
        let state = lock_state();
        state
            .as_ref()
            .map(|s| active_model_count(s.show_claude_code, s.show_codex, s.show_antigravity))
            .unwrap_or(1)
    };
    total_widget_width_for(active_models)
}

fn claude_palette(is_dark: bool) -> UsagePalette {
    UsagePalette {
        start: Color::from_hex("#D97757"),
        middle: Color::from_hex("#E79569"),
        end: Color::from_hex("#C45020"),
        track: if is_dark {
            Color::from_hex("#4A3732")
        } else {
            Color::from_hex("#EADBD5")
        },
    }
}

fn codex_palette(is_dark: bool) -> UsagePalette {
    UsagePalette {
        start: Color::from_hex("#A99BFF"),
        middle: Color::from_hex("#617CF7"),
        end: Color::from_hex("#302FE5"),
        track: if is_dark {
            Color::from_hex("#393A5A")
        } else {
            Color::from_hex("#DAD9F6")
        },
    }
}

fn antigravity_palette(is_dark: bool) -> UsagePalette {
    UsagePalette {
        start: Color::from_hex("#4285F4"),
        middle: Color::from_hex("#34A853"),
        end: Color::from_hex("#A142F4"),
        track: if is_dark {
            Color::from_hex("#29405F")
        } else {
            Color::from_hex("#D9E6FB")
        },
    }
}

fn claude_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#F09A7A")
    } else {
        Color::from_hex("#A94F32")
    }
}

fn codex_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#AEB7FF")
    } else {
        Color::from_hex("#4652D9")
    }
}

fn antigravity_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#8AB4F8")
    } else {
        Color::from_hex("#1967D2")
    }
}

pub fn run() {
    // Enable Per-Monitor DPI Awareness V2 for crisp rendering at any scale factor
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Single-instance guard: silently exit if another instance is running.
    // Exception: when relaunched after an explorer restart (ENV_RELAUNCH set),
    // wait for the previous instance to release the mutex, then take over.
    let is_relaunch = std::env::var(ENV_RELAUNCH).is_ok();
    let mutex_name = native_interop::wide_str("Global\\CodexUsageMonitor");
    let _mutex = unsafe {
        let handle = CreateMutexW(None, true, PCWSTR::from_raw(mutex_name.as_ptr()));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    if is_relaunch {
                        diagnose::log("relaunch: waiting for previous instance to exit");
                        let wait_result = WaitForSingleObject(h, 10_000);
                        if wait_result != WAIT_OBJECT_0 && wait_result != WAIT_ABANDONED {
                            diagnose::log(format!(
                                "startup aborted: previous instance did not exit cleanly ({wait_result:?})"
                            ));
                            return;
                        }
                    } else {
                        diagnose::log("startup aborted: another instance is already running");
                        return;
                    }
                }
                h
            }
            Err(error) => {
                diagnose::log_error(
                    "startup aborted: unable to create single-instance mutex",
                    error,
                );
                return;
            }
        }
    };

    let class_name = native_interop::wide_str("CodexUsageMonitor");
    let taskbar_created = native_interop::wide_str("TaskbarCreated");
    TASKBAR_CREATED_MSG.store(
        unsafe { RegisterWindowMessageW(PCWSTR::from_raw(taskbar_created.as_ptr())) },
        Ordering::Relaxed,
    );

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let (large_icon, small_icon) = load_embedded_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("RegisterClassExW returned 0");
        }

        let settings = load_settings();
        let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
        let language = localization::resolve_language(language_override);
        let install_channel = updater::current_install_channel();

        // Create as layered popup (will be reparented into taskbar)
        let title = native_interop::wide_str(language.strings().window_title);
        let initial_model_count = active_model_count(
            settings.show_claude_code,
            settings.show_codex,
            settings.show_antigravity,
        );
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            total_widget_width_for(initial_model_count),
            sc(WIDGET_HEIGHT),
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .unwrap();

        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(large_icon.0 as isize),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as usize),
                LPARAM(small_icon.0 as isize),
            );
        }

        diagnose::log(format!("main window created hwnd={:?}", hwnd));

        let system_dark = theme::is_dark_mode();
        let is_dark = settings.theme.resolve_dark(system_dark);
        let mut embedded = false;

        {
            let mut state = lock_state();
            *state = Some(AppState {
                hwnd: SendHwnd::from_hwnd(hwnd),
                taskbar_hwnd: None,
                tray_notify_hwnd: None,
                win_event_hook: None,
                is_dark,
                system_dark,
                theme_mode: settings.theme,
                embedded: false,
                geometry_ready: false,
                language_override,
                language,
                install_channel,
                session_percent: 0.0,
                session_text: "--".to_string(),
                weekly_percent: 0.0,
                weekly_text: "--".to_string(),
                codex_session_percent: 0.0,
                codex_session_text: "--".to_string(),
                codex_weekly_percent: 0.0,
                codex_weekly_text: "--".to_string(),
                antigravity_session_percent: 0.0,
                antigravity_session_text: "--".to_string(),
                antigravity_weekly_percent: 0.0,
                antigravity_weekly_text: "--".to_string(),
                show_claude_code: settings.show_claude_code,
                show_codex: settings.show_codex,
                show_antigravity: settings.show_antigravity,
                data: None,
                poll_interval_ms: settings.poll_interval_ms,
                retry_count: 0,
                force_notify_auth_error: false,
                auth_notifications: ProviderAuthNotifications::default(),
                auth_error_paused_polling: false,
                auth_watch_mode: poller::CredentialWatchMode::ActiveSource,
                auth_watch_snapshot: Vec::new(),
                last_poll_ok: false,
                update_status: UpdateStatus::Idle,
                last_update_check_unix: settings.last_update_check_unix,
                taskbar_index: settings.taskbar_index,
                tray_offset: settings.tray_offset,
                dragging: false,
                drag_start_mouse_x: 0,
                drag_start_client_x: 0,
                drag_start_offset: 0,
                widget_visible: settings.widget_visible,
            });
        }

        log_input_state("before taskbar attachment", true);
        // Try to embed in taskbar
        if attach_to_taskbar(hwnd, settings.taskbar_index) {
            embedded = true;
        }

        // Keep the same per-pixel layered rendering path in popup fallback.
        if !embedded {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }

        // Register system tray icon(s)
        sync_tray_icons(hwnd);

        // Position only after the notification area has a usable rectangle.
        position_at_taskbar();
        log_input_state("after taskbar attachment", true);

        // Initial render via UpdateLayeredWindow.
        render_layered();

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        SetTimer(hwnd, TIMER_POLL, initial_poll_ms, None);
        let geometry_ready = {
            let state = lock_state();
            state.as_ref().is_some_and(|s| s.geometry_ready)
        };
        SetTimer(
            hwnd,
            TIMER_TASKBAR_GEOMETRY,
            if geometry_ready { 5_000 } else { 1_000 },
            None,
        );

        // Watch for explorer.exe restarts so we can re-embed and re-add the tray
        // icon (the shell discards tray registrations when it restarts). This
        // runs on a dedicated thread, NOT a window timer: once explorer destroys
        // the taskbar, our embedded child window stops receiving all messages
        // (WM_TIMER included), so a timer would never fire again.
        spawn_taskbar_watchdog();

        // Initial poll
        let send_hwnd = SendHwnd::from_hwnd(hwnd);
        std::thread::spawn(move || {
            diagnose::log("initial poll thread started");
            do_poll(send_hwnd);
        });

        schedule_auto_update_check(hwnd);
        let should_check_updates = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| auto_update_check_due(s.last_update_check_unix))
                .unwrap_or(false)
        };
        if should_check_updates {
            begin_update_check(hwnd, false);
        }

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Render widget content and push to the layered window via UpdateLayeredWindow.
/// Renders widget content with a transparent background over the taskbar.
fn render_layered() {
    refresh_dpi();
    let (
        hwnd_val,
        widget_dark,
        strings,
        session_pct,
        session_text,
        weekly_pct,
        weekly_text,
        codex_session_pct,
        codex_session_text,
        codex_weekly_pct,
        codex_weekly_text,
        antigravity_session_pct,
        antigravity_session_text,
        antigravity_weekly_pct,
        antigravity_weekly_text,
        show_claude_code,
        show_codex,
        show_antigravity,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.hwnd,
                s.system_dark,
                s.language.strings(),
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
                s.codex_session_percent,
                s.codex_session_text.clone(),
                s.codex_weekly_percent,
                s.codex_weekly_text.clone(),
                s.antigravity_session_percent,
                s.antigravity_session_text.clone(),
                s.antigravity_weekly_percent,
                s.antigravity_weekly_text.clone(),
                s.show_claude_code,
                s.show_codex,
                s.show_antigravity,
            ),
            None => return,
        }
    };

    let hwnd = hwnd_val.to_hwnd();

    let width = total_widget_width();
    let height = sc(WIDGET_HEIGHT);

    let text_color = if widget_dark {
        Color::from_hex("#888888")
    } else {
        Color::from_hex("#404040")
    };
    let bg_color = if widget_dark {
        Color::from_hex("#1C1C1C")
    } else {
        Color::from_hex("#F3F3F3")
    };
    let widget_paint = WidgetPaint {
        background: bg_color,
        text_color,
        strings,
        claude: ProviderPaint {
            visible: show_claude_code,
            palette: claude_palette(widget_dark),
            usage_text_color: claude_usage_text_color(widget_dark),
            session: UsageValuePaint {
                percent: session_pct,
                text: &session_text,
            },
            weekly: UsageValuePaint {
                percent: weekly_pct,
                text: &weekly_text,
            },
        },
        codex: ProviderPaint {
            visible: show_codex,
            palette: codex_palette(widget_dark),
            usage_text_color: codex_usage_text_color(widget_dark),
            session: UsageValuePaint {
                percent: codex_session_pct,
                text: &codex_session_text,
            },
            weekly: UsageValuePaint {
                percent: codex_weekly_pct,
                text: &codex_weekly_text,
            },
        },
        antigravity: ProviderPaint {
            visible: show_antigravity,
            palette: antigravity_palette(widget_dark),
            usage_text_color: antigravity_usage_text_color(widget_dark),
            session: UsageValuePaint {
                percent: antigravity_session_pct,
                text: &antigravity_session_text,
            },
            weekly: UsageValuePaint {
                percent: antigravity_weekly_pct,
                text: &antigravity_weekly_text,
            },
        },
    };

    unsafe {
        let screen_dc = GetDC(hwnd);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mem_dc = CreateCompatibleDC(screen_dc);
        let dib =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();

        if dib.is_invalid() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return;
        }

        let old_bmp = SelectObject(mem_dc, dib);
        let pixel_count = (width * height) as usize;

        // Render against a theme-matched colour before marking background pixels transparent.
        paint_content(mem_dc, width, height, &widget_paint);

        // Background pixels retain minimal alpha for mouse hit-testing.
        let bg_bgr = bg_color.to_colorref();
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        for px in pixel_data.iter_mut() {
            let rgb = *px & 0x00FFFFFF;
            if rgb == bg_bgr {
                *px = 0x01000000;
            } else {
                *px = rgb | 0xFF000000;
            }
        }

        // Push to window via UpdateLayeredWindow
        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA
        };

        let result = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        if let Err(error) = result {
            diagnose::log_error("UpdateLayeredWindow failed", error);
        }

        // Cleanup
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

#[derive(Clone, Copy)]
struct UsageValuePaint<'a> {
    percent: f64,
    text: &'a str,
}

#[derive(Clone, Copy)]
struct ProviderPaint<'a> {
    visible: bool,
    palette: UsagePalette,
    usage_text_color: Color,
    session: UsageValuePaint<'a>,
    weekly: UsageValuePaint<'a>,
}

#[derive(Clone, Copy)]
enum QuotaWindow {
    Session,
    Weekly,
}

impl<'a> ProviderPaint<'a> {
    fn value(self, window: QuotaWindow) -> UsageValuePaint<'a> {
        match window {
            QuotaWindow::Session => self.session,
            QuotaWindow::Weekly => self.weekly,
        }
    }
}

struct WidgetPaint<'a> {
    background: Color,
    text_color: Color,
    strings: Strings,
    claude: ProviderPaint<'a>,
    codex: ProviderPaint<'a>,
    antigravity: ProviderPaint<'a>,
}

/// Paint all widget content onto a DC with a given background color.
fn paint_content(hdc: HDC, width: i32, height: i32, paint: &WidgetPaint<'_>) {
    unsafe {
        let client_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };

        let bg_brush = CreateSolidBrush(COLORREF(paint.background.to_colorref()));
        FillRect(hdc, &client_rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        // Left divider
        let divider_h = sc(25);
        let divider_top = (height - divider_h) / 2;
        let divider_bottom = divider_top + divider_h;

        let divider_palette = if paint.claude.visible {
            paint.claude.palette
        } else if paint.codex.visible {
            paint.codex.palette
        } else {
            paint.antigravity.palette
        };
        let div_left = divider_palette.middle;
        let div_right = divider_palette.track;

        let left_brush = CreateSolidBrush(COLORREF(div_left.to_colorref()));
        let left_rect = RECT {
            left: 0,
            top: divider_top,
            right: sc(2),
            bottom: divider_bottom,
        };
        FillRect(hdc, &left_rect, left_brush);
        let _ = DeleteObject(left_brush);

        let right_brush = CreateSolidBrush(COLORREF(div_right.to_colorref()));
        let right_rect = RECT {
            left: sc(2),
            top: divider_top,
            right: sc(3),
            bottom: divider_bottom,
        };
        FillRect(hdc, &right_rect, right_brush);
        let _ = DeleteObject(right_brush);

        let content_x = sc(LEFT_DIVIDER_W) + sc(DIVIDER_RIGHT_MARGIN);
        let row2_y = height - sc(5) - sc(SEGMENT_H);
        let row1_y = row2_y - sc(10) - sc(SEGMENT_H);

        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(paint.text_color.to_colorref()));

        let font_name = native_interop::wide_str("Segoe UI");
        let font = CreateFontW(
            sc(-12),
            0,
            0,
            0,
            FW_MEDIUM.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let old_font = SelectObject(hdc, font);

        draw_row(hdc, content_x, row1_y, paint, QuotaWindow::Session);
        draw_row(hdc, content_x, row2_y, paint, QuotaWindow::Weekly);

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    }
}

fn credential_watch_mode(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    all_claude_sources: bool,
) -> poller::CredentialWatchMode {
    if active_model_count(show_claude_code, show_codex, show_antigravity) > 1 {
        poller::CredentialWatchMode::AllProviders
    } else if show_codex {
        poller::CredentialWatchMode::Codex
    } else if show_antigravity {
        poller::CredentialWatchMode::Antigravity
    } else if all_claude_sources {
        poller::CredentialWatchMode::AllSources
    } else {
        poller::CredentialWatchMode::ActiveSource
    }
}

fn is_auth_notification_error(error: Option<poller::PollError>) -> bool {
    matches!(
        error,
        Some(
            poller::PollError::AuthRequired
                | poller::PollError::NoCredentials
                | poller::PollError::TokenExpired
        )
    )
}

fn update_auth_notification_latch(
    notified: &mut bool,
    enabled: bool,
    succeeded: bool,
    error: Option<poller::PollError>,
    force: bool,
) -> bool {
    if !enabled || succeeded {
        *notified = false;
        return false;
    }

    if !is_auth_notification_error(error) {
        return false;
    }

    let should_notify = force || !*notified;
    *notified = true;
    should_notify
}

fn auth_balloons_for_poll(
    state: &mut AppState,
    data: &AppUsageData,
    errors: poller::ProviderPollErrors,
) -> Vec<AuthBalloon> {
    let force = state.force_notify_auth_error;
    state.force_notify_auth_error = false;

    let notify_claude = update_auth_notification_latch(
        &mut state.auth_notifications.claude_code,
        state.show_claude_code,
        data.claude_code.is_some(),
        errors.claude_code,
        force,
    );
    let notify_codex = update_auth_notification_latch(
        &mut state.auth_notifications.codex,
        state.show_codex,
        data.codex.is_some(),
        errors.codex,
        force,
    );
    let notify_antigravity = update_auth_notification_latch(
        &mut state.auth_notifications.antigravity,
        state.show_antigravity,
        data.antigravity.is_some(),
        errors.antigravity,
        force,
    );

    let strings = state.language.strings();
    let mut balloons = Vec::with_capacity(3);
    if notify_claude {
        balloons.push(AuthBalloon {
            kind: tray_icon::TrayIconKind::Claude,
            title: strings.token_expired_title,
            body: strings.token_expired_body,
        });
    }
    if notify_codex {
        balloons.push(AuthBalloon {
            kind: tray_icon::TrayIconKind::Codex,
            title: strings.codex_token_expired_title,
            body: strings.codex_token_expired_body,
        });
    }
    if notify_antigravity {
        balloons.push(AuthBalloon {
            kind: tray_icon::TrayIconKind::Antigravity,
            title: strings.antigravity_token_expired_title,
            body: strings.antigravity_token_expired_body,
        });
    }
    balloons
}

fn show_auth_balloons(hwnd: HWND, balloons: Vec<AuthBalloon>) {
    for balloon in balloons {
        let accepted = tray_icon::notify_balloon(hwnd, balloon.kind, balloon.title, balloon.body);
        diagnose::log(format!(
            "authentication notification requested title={:?} shell_accepted={accepted}",
            balloon.title
        ));
    }
}

fn do_poll(send_hwnd: SendHwnd) {
    let hwnd = send_hwnd.to_hwnd();
    let (show_claude_code, show_codex, show_antigravity) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|s| (s.show_claude_code, s.show_codex, s.show_antigravity))
            .unwrap_or((false, true, false))
    };

    match poller::poll(show_claude_code, show_codex, show_antigravity).into_result() {
        Ok((data, errors)) => {
            let balloons = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if let Some(claude_code) = data.claude_code.as_ref() {
                        s.session_percent = claude_code.session.percentage;
                        s.weekly_percent = claude_code.weekly.percentage;
                    } else if s.show_claude_code {
                        s.session_percent = 0.0;
                        s.weekly_percent = 0.0;
                    }
                    if let Some(codex) = data.codex.as_ref() {
                        s.codex_session_percent = codex.session.percentage;
                        s.codex_weekly_percent = codex.weekly.percentage;
                    } else if s.show_codex {
                        s.codex_session_percent = 0.0;
                        s.codex_weekly_percent = 0.0;
                    }
                    if let Some(antigravity) = data.antigravity.as_ref() {
                        s.antigravity_session_percent = antigravity.session.percentage;
                        s.antigravity_weekly_percent = antigravity.weekly.percentage;
                    } else if s.show_antigravity {
                        s.antigravity_session_percent = 0.0;
                        s.antigravity_weekly_percent = 0.0;
                    }
                    // Stop fast-poll if reset data is now fresh
                    if !poller::app_is_past_reset(&data) {
                        unsafe {
                            let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                        }
                    }

                    let balloons = auth_balloons_for_poll(s, &data, errors);
                    s.data = Some(data);
                    s.last_poll_ok = true;
                    refresh_usage_texts(s);

                    // Recovered from errors — restore normal poll interval
                    if s.retry_count > 0 {
                        s.retry_count = 0;
                        let interval = s.poll_interval_ms;
                        unsafe {
                            SetTimer(hwnd, TIMER_POLL, interval, None);
                        }
                    }
                    s.auth_error_paused_polling = false;
                    s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource;
                    s.auth_watch_snapshot.clear();
                    balloons
                } else {
                    Vec::new()
                }
            };

            show_auth_balloons(hwnd, balloons);
            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err((e, errors)) => {
            let auth_watch = match e {
                poller::PollError::AuthRequired | poller::PollError::TokenExpired => {
                    let mode = credential_watch_mode(
                        show_claude_code,
                        show_codex,
                        show_antigravity,
                        false,
                    );
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::NoCredentials => {
                    let mode =
                        credential_watch_mode(show_claude_code, show_codex, show_antigravity, true);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::RequestFailed => None,
            };
            // Distinguish auth-required errors from transient errors.
            let failed_data = AppUsageData::default();
            let balloons = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    let balloons = auth_balloons_for_poll(s, &failed_data, errors);
                    s.last_poll_ok = false;
                    match auth_watch {
                        Some((watch_mode, watch_snapshot)) => {
                            s.auth_error_paused_polling = true;
                            s.auth_watch_mode = watch_mode;
                            s.auth_watch_snapshot = watch_snapshot;
                            s.session_text = "!".to_string();
                            s.weekly_text = "!".to_string();
                            s.codex_session_text = "!".to_string();
                            s.codex_weekly_text = "!".to_string();
                            s.antigravity_session_text = "!".to_string();
                            s.antigravity_weekly_text = "!".to_string();
                            s.retry_count = s.retry_count.saturating_add(1);
                            unsafe {
                                let _ = KillTimer(hwnd, TIMER_POLL);
                                let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                                let _ = KillTimer(hwnd, TIMER_COUNTDOWN);
                                SetTimer(hwnd, TIMER_POLL, s.poll_interval_ms, None);
                            }
                        }
                        _ => {
                            // Transient request errors use exponential backoff.
                            s.auth_error_paused_polling = false;
                            s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource;
                            s.auth_watch_snapshot.clear();
                            s.session_text = "...".to_string();
                            s.weekly_text = "...".to_string();
                            s.codex_session_text = "...".to_string();
                            s.codex_weekly_text = "...".to_string();
                            s.antigravity_session_text = "...".to_string();
                            s.antigravity_weekly_text = "...".to_string();
                            s.retry_count = s.retry_count.saturating_add(1);
                            let backoff = RETRY_BASE_MS.saturating_mul(
                                1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX),
                            );
                            let retry_ms = backoff.min(s.poll_interval_ms);
                            unsafe {
                                let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                                SetTimer(hwnd, TIMER_POLL, retry_ms, None);
                            }
                        }
                    }
                    balloons
                } else {
                    Vec::new()
                }
            };

            show_auth_balloons(hwnd, balloons);

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn schedule_countdown_timer() {
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();
    if !s.last_poll_ok {
        unsafe {
            let _ = KillTimer(hwnd, TIMER_COUNTDOWN);
            let _ = KillTimer(hwnd, TIMER_RESET_POLL);
        }
        return;
    }

    let data = match &s.data {
        Some(d) => d,
        None => return,
    };

    // If a reset time has passed, poll every 5s to pick up fresh data
    if poller::app_is_past_reset(data) {
        unsafe {
            SetTimer(hwnd, TIMER_RESET_POLL, 5_000, None);
        }
    }

    let delays = [
        data.claude_code
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.session.resets_at)),
        data.claude_code
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.weekly.resets_at)),
        data.codex
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.session.resets_at)),
        data.codex
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.weekly.resets_at)),
        data.antigravity
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.session.resets_at)),
        data.antigravity
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.weekly.resets_at)),
    ];
    let min_delay = delays.into_iter().flatten().min();

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    unsafe {
        SetTimer(hwnd, TIMER_COUNTDOWN, ms, None);
    }
}

fn check_theme_change() {
    let system_dark = theme::is_dark_mode();
    let widget_changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            let widget_changed = s.system_dark != system_dark;
            s.system_dark = system_dark;
            s.is_dark = s.theme_mode.resolve_dark(system_dark);
            widget_changed
        } else {
            false
        }
    };
    if widget_changed {
        render_layered();
    }
}

fn check_language_change() {
    if update_language_change() {
        render_layered();
    }
}

fn update_display() {
    let mut state = lock_state();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };

    // Don't overwrite error text with stale cached data
    if !s.last_poll_ok {
        return;
    }

    refresh_usage_texts(s);
}

fn update_tray_hook(tray_hwnd: Option<HWND>) {
    let old_hook = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        if s.tray_notify_hwnd == tray_hwnd {
            return;
        }
        s.tray_notify_hwnd = tray_hwnd;
        s.win_event_hook.take()
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook);
    }
    let hook = tray_hwnd.and_then(|hwnd| {
        native_interop::set_tray_event_hook(
            native_interop::get_window_thread_id(hwnd),
            on_tray_location_changed,
        )
    });
    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.win_event_hook = hook;
    }
}

fn usable_taskbar_space(taskbar: RECT, tray: RECT, widget_width: i32, widget_height: i32) -> bool {
    let bar_width = taskbar.right - taskbar.left;
    let bar_height = taskbar.bottom - taskbar.top;
    bar_width > bar_height
        && bar_height >= widget_height
        && tray.right > tray.left
        && tray.left >= taskbar.left + widget_width
        && tray.right <= taskbar.right
        && tray.top < taskbar.bottom
        && tray.bottom > taskbar.top
}

fn set_geometry_ready(hwnd: HWND, ready: bool) -> bool {
    let (changed, visible) = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return false;
        };
        let changed = s.geometry_ready != ready;
        s.geometry_ready = ready;
        (changed, s.widget_visible)
    };
    unsafe {
        if ready && visible {
            if !IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        } else if IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    if changed {
        unsafe {
            SetTimer(
                hwnd,
                TIMER_TASKBAR_GEOMETRY,
                if ready { 5_000 } else { 1_000 },
                None,
            );
        }
        diagnose::log(if ready {
            "taskbar geometry ready; widget positioned"
        } else {
            "taskbar geometry unavailable; widget hidden until retry"
        });
    }
    changed
}

fn position_at_taskbar() {
    refresh_dpi();
    // Drop the app-state lock before any Win32 call that may synchronously
    // re-enter our window procedure.
    let (hwnd, embedded, tray_offset, taskbar_hwnd, taskbar_index) = {
        let state = lock_state();
        let s = match state.as_ref() {
            Some(s) => s,
            None => return,
        };

        // Don't fight the user's drag
        if s.dragging {
            return;
        }

        (
            s.hwnd.to_hwnd(),
            s.embedded,
            s.tray_offset,
            s.taskbar_hwnd,
            s.taskbar_index,
        )
    };

    let taskbar_hwnd = match taskbar_hwnd {
        Some(h) if unsafe { IsWindow(h).as_bool() } => h,
        _ => {
            let _ = attach_to_taskbar(hwnd, taskbar_index);
            let state = lock_state();
            let Some(h) = state.as_ref().and_then(|s| s.taskbar_hwnd) else {
                drop(state);
                set_geometry_ready(hwnd, false);
                return;
            };
            h
        }
    };
    let embedded = {
        let state = lock_state();
        state.as_ref().map(|s| s.embedded).unwrap_or(embedded)
    };

    let taskbar_rect = match native_interop::get_taskbar_rect(taskbar_hwnd) {
        Some(r) => r,
        None => {
            diagnose::log("taskbar rectangle unavailable");
            set_geometry_ready(hwnd, false);
            return;
        }
    };

    let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
    let anchor_top = taskbar_rect.top;
    let anchor_height = taskbar_height;
    let tray_hwnd = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd");
    update_tray_hook(tray_hwnd);
    let tray_rect = tray_hwnd.and_then(native_interop::get_window_rect_safe);
    let widget_width = total_widget_width();
    let widget_height = sc(WIDGET_HEIGHT);
    let Some(tray_rect) = tray_rect else {
        diagnose::log("TrayNotifyWnd rectangle unavailable");
        set_geometry_ready(hwnd, false);
        return;
    };
    if !usable_taskbar_space(taskbar_rect, tray_rect, widget_width, widget_height) {
        diagnose::log(format!(
            "taskbar space unavailable: taskbar=({}, {}, {}, {}) tray=({}, {}, {}, {}) widget={}x{}",
            taskbar_rect.left,
            taskbar_rect.top,
            taskbar_rect.right,
            taskbar_rect.bottom,
            tray_rect.left,
            tray_rect.top,
            tray_rect.right,
            tray_rect.bottom,
            widget_width,
            widget_height,
        ));
        set_geometry_ready(hwnd, false);
        return;
    }
    let tray_left = tray_rect.left;
    let tray_offset = clamp_widget_offset(taskbar_rect.left, tray_left, widget_width, tray_offset);
    let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
    let x = tray_left - widget_width - tray_offset;
    debug_assert!(x >= taskbar_rect.left && x + widget_width <= tray_left);
    let current = native_interop::get_window_rect_safe(hwnd);
    let needs_move = current.is_none_or(|rect| {
        rect.left != x
            || rect.top != y
            || rect.right - rect.left != widget_width
            || rect.bottom - rect.top != widget_height
    });
    if embedded {
        if needs_move {
            native_interop::move_window(
                hwnd,
                x - taskbar_rect.left,
                y - taskbar_rect.top,
                widget_width,
                widget_height,
            );
        }
    } else if needs_move {
        native_interop::move_window(hwnd, x, y, widget_width, widget_height);
    }
    if set_geometry_ready(hwnd, true) {
        render_layered();
    }
}

fn compute_anchor_y(anchor_top: i32, anchor_height: i32, widget_height: i32) -> i32 {
    let anchor_bottom = anchor_top + anchor_height;
    (anchor_bottom - widget_height).max(anchor_top)
}

/// WinEvent callback for tray icon location changes
unsafe extern "system" fn on_tray_location_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let target = {
        let state = lock_state();
        state
            .as_ref()
            .and_then(|s| (s.tray_notify_hwnd == Some(hwnd)).then_some(s.hwnd.to_hwnd()))
    };
    if let Some(target) = target {
        let _ = PostMessageW(target, WM_APP_TRAY_MOVED, WPARAM(0), LPARAM(0));
    }
}

/// Main window procedure
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MEASUREITEM if wparam.0 == 0 && lparam.0 != 0 => {
            let measure = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
            if measure.CtlType == ODT_MENU && measure.itemData != 0 {
                measure_menu_item(hwnd, measure);
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_DRAWITEM if wparam.0 == 0 && lparam.0 != 0 => {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            if draw.CtlType == ODT_MENU && draw.itemData != 0 {
                draw_menu_item(draw);
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_INPUTLANGCHANGE | WM_INPUTLANGCHANGEREQUEST => {
            log_input_state("input language message", true);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DISPLAYCHANGE
        | WM_DPICHANGED_MSG
        | WM_DPICHANGED_AFTERPARENT_MSG
        | WM_SETTINGCHANGE
        | WM_THEMECHANGED
        | WM_SYSCOLORCHANGE => {
            if msg == WM_DPICHANGED_MSG {
                let new_dpi = (wparam.0 & 0xFFFF) as u32;
                CURRENT_DPI.store(new_dpi, Ordering::Relaxed);
            }
            if msg == WM_SETTINGCHANGE || msg == WM_THEMECHANGED || msg == WM_SYSCOLORCHANGE {
                check_theme_change();
                check_language_change();
            }
            refresh_dpi();
            position_at_taskbar();
            render_layered();
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            match timer_id {
                TIMER_POLL => {
                    let auth_watch = {
                        let state = lock_state();
                        state.as_ref().map(|s| {
                            (
                                s.auth_error_paused_polling,
                                s.auth_watch_mode,
                                s.auth_watch_snapshot.clone(),
                            )
                        })
                    };
                    match auth_watch {
                        Some((true, watch_mode, previous_snapshot)) => {
                            let current_snapshot = poller::credential_watch_snapshot(watch_mode);
                            if current_snapshot != previous_snapshot {
                                let mut state = lock_state();
                                if let Some(s) = state.as_mut() {
                                    if s.auth_error_paused_polling
                                        && s.auth_watch_mode == watch_mode
                                    {
                                        s.auth_watch_snapshot = current_snapshot;
                                    }
                                }
                                drop(state);
                                let sh = SendHwnd::from_hwnd(hwnd);
                                std::thread::spawn(move || {
                                    do_poll(sh);
                                });
                            }
                        }
                        Some((false, _, _)) => {
                            let sh = SendHwnd::from_hwnd(hwnd);
                            std::thread::spawn(move || {
                                do_poll(sh);
                            });
                        }
                        None => {}
                    }
                }
                TIMER_COUNTDOWN => {
                    update_display();
                    render_layered();
                    schedule_countdown_timer();
                }
                TIMER_RESET_POLL => {
                    let should_poll = {
                        let state = lock_state();
                        state
                            .as_ref()
                            .map(|s| !s.auth_error_paused_polling)
                            .unwrap_or(false)
                    };
                    if should_poll {
                        let sh = SendHwnd::from_hwnd(hwnd);
                        std::thread::spawn(move || {
                            do_poll(sh);
                        });
                    }
                }
                TIMER_UPDATE_CHECK => {
                    begin_update_check(hwnd, false);
                }
                TIMER_TASKBAR_GEOMETRY => {
                    position_at_taskbar();
                    check_theme_change();
                    log_input_state("periodic check", false);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            check_theme_change();
            check_language_change();
            render_layered();
            schedule_countdown_timer();
            sync_tray_icons(hwnd);
            position_at_taskbar();
            LRESULT(0)
        }
        WM_APP_TRAY_MOVED => {
            position_at_taskbar();
            LRESULT(0)
        }
        WM_APP_UPDATE_CHECK_COMPLETE => {
            schedule_auto_update_check(hwnd);
            menu_popup::refresh();
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            if cursor_is_on_drag_handle(hwnd) {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            let client_x = (lparam.0 & 0xFFFF) as i16 as i32;
            let client_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if !is_drag_handle_point(client_x, client_y) {
                return LRESULT(0);
            }

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.dragging = true;
                s.drag_start_mouse_x = pt.x;
                s.drag_start_client_x = client_x;
                s.drag_start_offset = s.tray_offset;
            }
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let move_target = {
                    let mut state = lock_state();
                    let s = match state.as_mut() {
                        Some(s) => s,
                        None => return LRESULT(0),
                    };

                    // Moving mouse left = positive delta = larger offset (further left)
                    let delta = s.drag_start_mouse_x - pt.x;
                    let mut new_offset = s.drag_start_offset + delta;

                    // Clamp: offset >= 0 (can't go right of default)
                    if new_offset < 0 {
                        new_offset = 0;
                    }

                    let taskbar_hwnd = s.taskbar_hwnd;
                    let embedded = s.embedded;
                    let hwnd_val = s.hwnd.to_hwnd();

                    // Clamp: don't go past left edge of taskbar
                    if let Some(taskbar_hwnd) = taskbar_hwnd {
                        if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                            let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
                            let widget_width = total_widget_width_for_state(s);
                            if tray_left - taskbar_rect.left < widget_width {
                                return LRESULT(0);
                            }
                            let max_offset = (tray_left - taskbar_rect.left - widget_width).max(0);
                            if new_offset > max_offset {
                                new_offset = max_offset;
                            }

                            s.tray_offset = new_offset;

                            let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
                            let anchor_top = taskbar_rect.top;
                            let anchor_height = taskbar_height;
                            let widget_height = sc(WIDGET_HEIGHT);
                            let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
                            let x = if embedded {
                                tray_left - taskbar_rect.left - widget_width - new_offset
                            } else {
                                tray_left - widget_width - new_offset
                            };
                            Some((
                                hwnd_val,
                                embedded,
                                x,
                                y,
                                taskbar_rect.top,
                                widget_width,
                                widget_height,
                            ))
                        } else {
                            s.tray_offset = new_offset;
                            None
                        }
                    } else {
                        s.tray_offset = new_offset;
                        None
                    }
                };

                if let Some((hwnd_val, embedded, x, y, taskbar_top, widget_width, widget_height)) =
                    move_target
                {
                    if embedded {
                        native_interop::move_window(
                            hwnd_val,
                            x,
                            y - taskbar_top,
                            widget_width,
                            widget_height,
                        );
                    } else {
                        native_interop::move_window(hwnd_val, x, y, widget_width, widget_height);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let drag_result = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.dragging {
                        s.dragging = false;
                        Some((s.taskbar_index, s.drag_start_client_x, s.drag_start_offset))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some((current_taskbar_index, drag_start_client_x, old_offset)) = drag_result {
                let _ = ReleaseCapture();
                if let Some((target_index, target_taskbar)) = taskbar_at_point(pt) {
                    if target_index != current_taskbar_index {
                        let new_offset = offset_for_drop_point(
                            target_taskbar.hwnd,
                            target_taskbar.rect,
                            pt,
                            drag_start_client_x,
                        );
                        {
                            let mut state = lock_state();
                            if let Some(s) = state.as_mut() {
                                s.tray_offset = new_offset;
                            }
                        }
                        if attach_to_taskbar(hwnd, target_index) {
                            position_at_taskbar();
                            render_layered();
                        } else {
                            let mut state = lock_state();
                            if let Some(s) = state.as_mut() {
                                s.tray_offset = old_offset;
                            }
                        }
                    }
                }
                save_state_settings();
                position_at_taskbar();
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u16;
            match id {
                1 => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.session_text = "...".to_string();
                            s.weekly_text = "...".to_string();
                            s.codex_session_text = "...".to_string();
                            s.codex_weekly_text = "...".to_string();
                            s.antigravity_session_text = "...".to_string();
                            s.antigravity_weekly_text = "...".to_string();
                            s.force_notify_auth_error = true;
                        }
                    }
                    render_layered();
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                IDM_VERSION_ACTION => {
                    let (install_channel, release) = {
                        let state = lock_state();
                        match state.as_ref() {
                            Some(s) => (
                                s.install_channel,
                                match &s.update_status {
                                    UpdateStatus::Available(release) => Some(release.clone()),
                                    _ => None,
                                },
                            ),
                            None => (InstallChannel::Portable, None),
                        }
                    };

                    match install_channel {
                        InstallChannel::Winget => {
                            if release.is_some() {
                                begin_winget_update(hwnd);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                        InstallChannel::Portable => {
                            if let Some(release) = release {
                                begin_update_apply(hwnd, release);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                    }
                }
                2 => {
                    let hook = {
                        let state = lock_state();
                        state.as_ref().and_then(|s| s.win_event_hook)
                    };
                    if let Some(h) = hook {
                        native_interop::unhook_win_event(h);
                    }
                    PostQuitMessage(0);
                }
                IDM_RESET_POSITION => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.tray_offset = 0;
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                }
                IDM_START_WITH_WINDOWS => {
                    set_startup_enabled(!is_startup_enabled());
                }
                IDM_FREQ_1MIN | IDM_FREQ_5MIN | IDM_FREQ_15MIN | IDM_FREQ_1HOUR => {
                    let new_interval = match id {
                        IDM_FREQ_1MIN => POLL_1_MIN,
                        IDM_FREQ_5MIN => POLL_5_MIN,
                        IDM_FREQ_15MIN => POLL_15_MIN,
                        IDM_FREQ_1HOUR => POLL_1_HOUR,
                        _ => POLL_15_MIN,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.poll_interval_ms = new_interval;
                        }
                    }
                    save_state_settings();
                    // Reset the poll timer with the new interval
                    SetTimer(hwnd, TIMER_POLL, new_interval, None);
                }
                IDM_THEME_SYSTEM | IDM_THEME_LIGHT | IDM_THEME_DARK => {
                    let mode = match id {
                        IDM_THEME_LIGHT => ThemeMode::Light,
                        IDM_THEME_DARK => ThemeMode::Dark,
                        _ => ThemeMode::System,
                    };
                    let changed = {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            let changed = s.theme_mode != mode;
                            s.theme_mode = mode;
                            changed
                        } else {
                            false
                        }
                    };
                    if changed {
                        save_state_settings();
                        check_theme_change();
                    }
                }
                IDM_MODEL_CLAUDE_CODE | IDM_MODEL_CODEX | IDM_MODEL_ANTIGRAVITY => {
                    let changed = {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            let changed = toggle_model_visibility(
                                &mut s.show_claude_code,
                                &mut s.show_codex,
                                &mut s.show_antigravity,
                                id,
                            );
                            if changed {
                                if !s.show_claude_code {
                                    s.auth_notifications.claude_code = false;
                                }
                                if !s.show_codex {
                                    s.auth_notifications.codex = false;
                                }
                                if !s.show_antigravity {
                                    s.auth_notifications.antigravity = false;
                                }
                                s.session_text = "...".to_string();
                                s.weekly_text = "...".to_string();
                                s.codex_session_text = "...".to_string();
                                s.codex_weekly_text = "...".to_string();
                                s.antigravity_session_text = "...".to_string();
                                s.antigravity_weekly_text = "...".to_string();
                            }
                            changed
                        } else {
                            false
                        }
                    };
                    if changed {
                        save_state_settings();
                        position_at_taskbar();
                        render_layered();
                        sync_tray_icons(hwnd);
                        let sh = SendHwnd::from_hwnd(hwnd);
                        std::thread::spawn(move || {
                            do_poll(sh);
                        });
                    }
                }
                IDM_LANG_SYSTEM
                | IDM_LANG_ENGLISH
                | IDM_LANG_DUTCH
                | IDM_LANG_SPANISH
                | IDM_LANG_FRENCH
                | IDM_LANG_GERMAN
                | IDM_LANG_JAPANESE
                | IDM_LANG_KOREAN
                | IDM_LANG_TRADITIONAL_CHINESE
                | IDM_LANG_SIMPLIFIED_CHINESE
                | IDM_LANG_RUSSIAN
                | IDM_LANG_PORTUGUESE_BRAZIL => {
                    let language_override = match id {
                        IDM_LANG_SYSTEM => None,
                        IDM_LANG_ENGLISH => Some(LanguageId::English),
                        IDM_LANG_DUTCH => Some(LanguageId::Dutch),
                        IDM_LANG_SPANISH => Some(LanguageId::Spanish),
                        IDM_LANG_FRENCH => Some(LanguageId::French),
                        IDM_LANG_GERMAN => Some(LanguageId::German),
                        IDM_LANG_JAPANESE => Some(LanguageId::Japanese),
                        IDM_LANG_KOREAN => Some(LanguageId::Korean),
                        IDM_LANG_TRADITIONAL_CHINESE => Some(LanguageId::TraditionalChinese),
                        IDM_LANG_SIMPLIFIED_CHINESE => Some(LanguageId::SimplifiedChinese),
                        IDM_LANG_RUSSIAN => Some(LanguageId::Russian),
                        IDM_LANG_PORTUGUESE_BRAZIL => Some(LanguageId::PortugueseBrazil),
                        _ => None,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            apply_language_to_state(s, language_override);
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                id if id == tray_icon::IDM_TOGGLE_WIDGET => {
                    toggle_widget_visibility(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ if msg == WM_APP_TRAY => {
            match tray_icon::handle_message(lparam) {
                tray_icon::TrayAction::ToggleWidget => {
                    toggle_widget_visibility(hwnd);
                }
                tray_icon::TrayAction::ShowContextMenu => {
                    show_context_menu(hwnd);
                }
                tray_icon::TrayAction::None => {}
            }
            LRESULT(0)
        }
        _ if TASKBAR_CREATED_MSG.load(Ordering::Relaxed) != 0
            && msg == TASKBAR_CREATED_MSG.load(Ordering::Relaxed) =>
        {
            menu_popup::close();
            sync_tray_icons(hwnd);
            position_at_taskbar();
            LRESULT(0)
        }
        WM_DESTROY => {
            menu_popup::close();
            let hook = {
                let state = lock_state();
                state.as_ref().and_then(|s| s.win_event_hook)
            };
            if let Some(h) = hook {
                native_interop::unhook_win_event(h);
            }
            tray_icon::remove_all(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[derive(Clone, Copy)]
struct MenuPalette {
    background: HBRUSH,
    selected: HBRUSH,
    divider: HBRUSH,
    foreground: Color,
    disabled: Color,
    accent: Color,
}

struct MenuItem {
    text: Vec<u16>,
    text_len: usize,
    separator: bool,
    submenu: bool,
    dpi: u32,
    font: HFONT,
    palette: MenuPalette,
}

fn menu_px(px: i32, dpi: u32) -> i32 {
    (px * dpi as i32 + 48) / 96
}

const POPUP_MENU_CLASS: [u16; 6] = [
    b'#' as u16,
    b'3' as u16,
    b'2' as u16,
    b'7' as u16,
    b'6' as u16,
    b'8' as u16,
];
const WM_MENU_BORDER_PAINT: u32 = WM_APP + 100;
const MENU_BORDER_TIMER_ID: usize = 0xc0de;

unsafe extern "system" fn menu_creation_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 && lparam.0 != 0 {
        let create = &mut *(lparam.0 as *mut CBT_CREATEWNDW);
        if !create.lpcs.is_null() {
            let params = &mut *create.lpcs;
            let class_atom = params.lpszClass.0 as usize;
            if diagnose::is_enabled() {
                diagnose::log(format!(
                    "menu CBT class={class_atom:#x} style={:#x} exstyle={:#x}",
                    params.style, params.dwExStyle.0
                ));
            }
            if class_atom == 0x8000 {
                params.style &= !(WS_BORDER.0 as i32);
                params.dwExStyle &= !(WS_EX_WINDOWEDGE | WS_EX_DLGMODALFRAME);
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

unsafe fn paint_menu_border(hwnd: HWND, color: COLORREF) {
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_err() || client.right <= 1 || client.bottom <= 1 {
        return;
    }
    let brush = CreateSolidBrush(color);
    if brush.is_invalid() {
        return;
    }
    let dc = GetDC(hwnd);
    if !dc.is_invalid() {
        for edge in [
            RECT {
                left: 0,
                top: 0,
                right: client.right,
                bottom: 1,
            },
            RECT {
                left: 0,
                top: 1,
                right: 1,
                bottom: client.bottom,
            },
            RECT {
                left: client.right - 1,
                top: 1,
                right: client.right,
                bottom: client.bottom,
            },
            RECT {
                left: 1,
                top: client.bottom - 1,
                right: client.right - 1,
                bottom: client.bottom,
            },
        ] {
            FillRect(dc, &edge, brush);
        }
        ReleaseDC(hwnd, dc);
    }
    let _ = DeleteObject(brush);
}

unsafe extern "system" fn menu_border_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    color: usize,
) -> LRESULT {
    if message == WM_MENU_BORDER_PAINT {
        diagnose::log("menu border posted paint");
        paint_menu_border(hwnd, COLORREF(color as u32));
        return LRESULT(0);
    }
    if message == WM_TIMER && wparam.0 == MENU_BORDER_TIMER_ID {
        diagnose::log("menu border timer paint");
        let _ = KillTimer(hwnd, MENU_BORDER_TIMER_ID);
        paint_menu_border(hwnd, COLORREF(color as u32));
        return LRESULT(0);
    }
    if message == WM_NCPAINT {
        return LRESULT(0);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if diagnose::is_enabled()
        && (message == WM_PAINT || message == WM_NCPAINT || message == WM_NCDESTROY)
    {
        diagnose::log(format!("menu subclass hwnd={:?} msg={message:#x}", hwnd));
    }
    if message == WM_PAINT {
        paint_menu_border(hwnd, COLORREF(color as u32));
    } else if message == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(menu_border_subclass), 0);
    }
    result
}

unsafe extern "system" fn style_menu_window(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object_id: i32,
    child_id: i32,
    _thread_id: u32,
    _event_time: u32,
) {
    if object_id != 0 || child_id != 0 {
        return;
    }
    let mut class_name = [0u16; 8];
    if GetClassNameW(hwnd, &mut class_name) != POPUP_MENU_CLASS.len() as i32
        || class_name[..POPUP_MENU_CLASS.len()] != POPUP_MENU_CLASS
    {
        return;
    }
    // Menu windows keep a light non-client frame even when their items are owner-drawn.
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_BORDER.0 as isize));
    let _ = SetWindowLongPtrW(
        hwnd,
        GWL_EXSTYLE,
        exstyle & !((WS_EX_WINDOWEDGE.0 | WS_EX_DLGMODALFRAME.0) as isize),
    );
    let _ = SetWindowPos(
        hwnd,
        HWND::default(),
        0,
        0,
        0,
        0,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return;
    }
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    let radius = menu_px(12, GetDpiForWindow(hwnd).max(96));
    let region = CreateRoundRectRgn(
        0,
        0,
        rect.right - rect.left,
        rect.bottom - rect.top,
        radius,
        radius,
    );
    if !region.is_invalid() && SetWindowRgn(hwnd, region, true) == 0 {
        let _ = DeleteObject(region);
    }
    let color = MENU_BACKGROUND_COLOR.load(Ordering::Relaxed);
    let subclassed = SetWindowSubclass(hwnd, Some(menu_border_subclass), 0, color as usize);
    if diagnose::is_enabled() {
        diagnose::log(format!(
            "menu hook hwnd={:?} current_thread={} window_thread={} subclassed={}",
            hwnd,
            GetCurrentThreadId(),
            GetWindowThreadProcessId(hwnd, None),
            subclassed.as_bool()
        ));
    }
    let _ = RedrawWindow(
        hwnd,
        None,
        HRGN::default(),
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_UPDATENOW,
    );
    paint_menu_border(hwnd, COLORREF(color));
    let _ = PostMessageW(hwnd, WM_MENU_BORDER_PAINT, WPARAM(0), LPARAM(0));
    let _ = SetTimer(hwnd, MENU_BORDER_TIMER_ID, 30, None);
}

// Win32 stores item pointers until TrackPopupMenu returns; Box keeps them stable.
#[allow(clippy::vec_box)]
unsafe fn style_menu(
    menu: HMENU,
    dpi: u32,
    font: HFONT,
    palette: MenuPalette,
    items: &mut Vec<Box<MenuItem>>,
) {
    let menu_info = MENUINFO {
        cbSize: std::mem::size_of::<MENUINFO>() as u32,
        fMask: MIM_BACKGROUND | MIM_STYLE,
        dwStyle: MNS_NOCHECK,
        hbrBack: palette.background,
        ..Default::default()
    };
    if let Err(error) = SetMenuInfo(menu, &menu_info) {
        diagnose::log_error("menu background failed", error);
    }

    let count = GetMenuItemCount(menu);
    for index in 0..count {
        let mut text = [0u16; 256];
        let mut info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_SUBMENU | MIIM_STRING,
            dwTypeData: PWSTR(text.as_mut_ptr()),
            cch: (text.len() - 1) as u32,
            ..Default::default()
        };
        if GetMenuItemInfoW(menu, index as u32, true, &mut info).is_err() {
            continue;
        }
        let separator = (info.fType.0 & MFT_SEPARATOR.0) != 0;
        let text_len = (info.cch as usize).min(text.len() - 1);
        let mut item = Box::new(MenuItem {
            text: text[..=text_len].to_vec(),
            text_len,
            separator,
            submenu: !info.hSubMenu.0.is_null(),
            dpi,
            font,
            palette,
        });
        info.fMask = MIIM_FTYPE | MIIM_DATA | MIIM_STRING;
        info.fType = MFT_OWNERDRAW;
        if separator {
            info.fMask |= MIIM_STATE;
            info.fState = MFS_DISABLED;
        }
        info.dwItemData = (&mut *item) as *mut MenuItem as usize;
        info.dwTypeData = PWSTR(item.text.as_mut_ptr());
        info.cch = text_len as u32;
        if SetMenuItemInfoW(menu, index as u32, true, &info).is_ok() {
            items.push(item);
        }
    }
}

unsafe fn measure_menu_item(hwnd: HWND, measure: &mut MEASUREITEMSTRUCT) {
    let item = &mut *(measure.itemData as *mut MenuItem);
    if item.separator {
        measure.itemWidth = menu_px(180, item.dpi) as u32;
        measure.itemHeight = menu_px(8, item.dpi) as u32;
        return;
    }
    let dc = GetDC(hwnd);
    let old_font = SelectObject(dc, item.font);
    let mut rect = RECT::default();
    let _ = DrawTextW(
        dc,
        &mut item.text[..item.text_len],
        &mut rect,
        DT_CALCRECT | DT_SINGLELINE,
    );
    SelectObject(dc, old_font);
    ReleaseDC(hwnd, dc);
    measure.itemWidth = (rect.right + menu_px(52, item.dpi)).max(menu_px(180, item.dpi)) as u32;
    measure.itemHeight = menu_px(25, item.dpi) as u32;
}

unsafe fn draw_menu_item(draw: &DRAWITEMSTRUCT) {
    let item = &mut *(draw.itemData as *mut MenuItem);
    FillRect(draw.hDC, &draw.rcItem, item.palette.background);
    if item.separator {
        let line = RECT {
            left: draw.rcItem.left + menu_px(29, item.dpi),
            top: (draw.rcItem.top + draw.rcItem.bottom) / 2,
            right: draw.rcItem.right - menu_px(8, item.dpi),
            bottom: (draw.rcItem.top + draw.rcItem.bottom) / 2 + 1,
        };
        FillRect(draw.hDC, &line, item.palette.divider);
        return;
    }

    let disabled = (draw.itemState.0 & (ODS_DISABLED.0 | ODS_GRAYED.0)) != 0;
    if (draw.itemState.0 & ODS_SELECTED.0) != 0 && !disabled {
        let region = CreateRoundRectRgn(
            draw.rcItem.left + menu_px(3, item.dpi),
            draw.rcItem.top + menu_px(1, item.dpi),
            draw.rcItem.right - menu_px(3, item.dpi),
            draw.rcItem.bottom - menu_px(1, item.dpi),
            menu_px(6, item.dpi),
            menu_px(6, item.dpi),
        );
        let _ = FillRgn(draw.hDC, region, item.palette.selected);
        let _ = DeleteObject(region);
    }

    let old_font = SelectObject(draw.hDC, item.font);
    let old_mode = SetBkMode(draw.hDC, TRANSPARENT);
    let foreground = if disabled {
        item.palette.disabled
    } else {
        item.palette.foreground
    };
    let old_color = SetTextColor(draw.hDC, COLORREF(foreground.to_colorref()));
    let mut text_rect = RECT {
        left: draw.rcItem.left + menu_px(31, item.dpi),
        top: draw.rcItem.top,
        right: draw.rcItem.right - menu_px(23, item.dpi),
        bottom: draw.rcItem.bottom,
    };
    let _ = DrawTextW(
        draw.hDC,
        &mut item.text[..item.text_len],
        &mut text_rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    if (draw.itemState.0 & ODS_CHECKED.0) != 0 {
        let _ = SetTextColor(draw.hDC, COLORREF(item.palette.accent.to_colorref()));
        let mut check = [0x2713u16];
        let mut check_rect = RECT {
            left: draw.rcItem.left + menu_px(6, item.dpi),
            top: draw.rcItem.top,
            right: draw.rcItem.left + menu_px(27, item.dpi),
            bottom: draw.rcItem.bottom,
        };
        let _ = DrawTextW(
            draw.hDC,
            &mut check,
            &mut check_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    if item.submenu {
        let _ = SetTextColor(draw.hDC, COLORREF(foreground.to_colorref()));
        let mut arrow = [0x203Au16];
        let mut arrow_rect = RECT {
            left: draw.rcItem.right - menu_px(21, item.dpi),
            top: draw.rcItem.top,
            right: draw.rcItem.right - menu_px(5, item.dpi),
            bottom: draw.rcItem.bottom,
        };
        let _ = DrawTextW(
            draw.hDC,
            &mut arrow,
            &mut arrow_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    let _ = SetTextColor(draw.hDC, old_color);
    let _ = SetBkMode(draw.hDC, BACKGROUND_MODE(old_mode as u32));
    SelectObject(draw.hDC, old_font);
    if item.submenu {
        // USER32 paints a second submenu arrow after WM_DRAWITEM.
        let _ = ExcludeClipRect(
            draw.hDC,
            draw.rcItem.right - menu_px(22, item.dpi),
            draw.rcItem.top,
            draw.rcItem.right + menu_px(4, item.dpi),
            draw.rcItem.bottom,
        );
    }
}

pub(crate) fn language_command_id(language: LanguageId) -> u16 {
    match language {
        LanguageId::English => IDM_LANG_ENGLISH,
        LanguageId::Dutch => IDM_LANG_DUTCH,
        LanguageId::Spanish => IDM_LANG_SPANISH,
        LanguageId::French => IDM_LANG_FRENCH,
        LanguageId::German => IDM_LANG_GERMAN,
        LanguageId::Japanese => IDM_LANG_JAPANESE,
        LanguageId::Korean => IDM_LANG_KOREAN,
        LanguageId::TraditionalChinese => IDM_LANG_TRADITIONAL_CHINESE,
        LanguageId::SimplifiedChinese => IDM_LANG_SIMPLIFIED_CHINESE,
        LanguageId::Russian => IDM_LANG_RUSSIAN,
        LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
    }
}

pub(crate) fn menu_snapshot() -> MenuSnapshot {
    let snapshot = {
        let state = lock_state();
        let s = state
            .as_ref()
            .expect("menu requires initialized application state");
        MenuSnapshot {
            strings: s.language.strings(),
            interval: s.poll_interval_ms,
            theme: s.theme_mode,
            language_override: s.language_override,
            widget_visible: s.widget_visible,
            claude_visible: s.show_claude_code,
            codex_visible: s.show_codex,
            antigravity_visible: s.show_antigravity,
            startup_enabled: false,
            version_label: version_action_label(
                s.language.strings(),
                s.language,
                s.install_channel,
                &s.update_status,
            ),
            update_disabled: matches!(
                s.update_status,
                UpdateStatus::Checking | UpdateStatus::Applying
            ),
            dark: s.is_dark,
        }
    };
    MenuSnapshot {
        startup_enabled: is_startup_enabled(),
        ..snapshot
    }
}

pub(crate) fn refresh_menu_snapshot() -> MenuSnapshot {
    check_theme_change();
    check_language_change();
    menu_snapshot()
}

fn show_context_menu(hwnd: HWND) {
    if !menu_popup::show(hwnd) {
        show_legacy_context_menu(hwnd);
    }
}

fn show_legacy_context_menu(hwnd: HWND) {
    unsafe {
        let (
            current_interval,
            strings,
            theme_mode,
            is_dark,
            language,
            language_override,
            install_channel,
            update_status,
            widget_visible,
            show_claude_code,
            show_codex,
            show_antigravity,
        ) = {
            let state = lock_state();
            match state.as_ref() {
                Some(s) => (
                    s.poll_interval_ms,
                    s.language.strings(),
                    s.theme_mode,
                    s.is_dark,
                    s.language,
                    s.language_override,
                    s.install_channel,
                    s.update_status.clone(),
                    s.widget_visible,
                    s.show_claude_code,
                    s.show_codex,
                    s.show_antigravity,
                ),
                None => (
                    POLL_15_MIN,
                    LanguageId::English.strings(),
                    ThemeMode::System,
                    false,
                    LanguageId::English,
                    None,
                    InstallChannel::Portable,
                    UpdateStatus::Idle,
                    true,
                    false,
                    true,
                    false,
                ),
            }
        };

        let menu = CreatePopupMenu().unwrap();

        let refresh_str = native_interop::wide_str(strings.refresh);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            1,
            PCWSTR::from_raw(refresh_str.as_ptr()),
        );

        let widget_label = native_interop::wide_str(strings.show_widget);
        let widget_flags = if widget_visible {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            menu,
            widget_flags,
            tray_icon::IDM_TOGGLE_WIDGET as usize,
            PCWSTR::from_raw(widget_label.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // Update Frequency submenu
        let freq_menu = CreatePopupMenu().unwrap();
        let freq_items: [(u16, u32, &str); 4] = [
            (IDM_FREQ_1MIN, POLL_1_MIN, strings.one_minute),
            (IDM_FREQ_5MIN, POLL_5_MIN, strings.five_minutes),
            (IDM_FREQ_15MIN, POLL_15_MIN, strings.fifteen_minutes),
            (IDM_FREQ_1HOUR, POLL_1_HOUR, strings.one_hour),
        ];
        for (id, interval, label) in freq_items {
            let label_str = native_interop::wide_str(label);
            let flags = if interval == current_interval {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                freq_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let freq_label = native_interop::wide_str(strings.update_frequency);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            freq_menu.0 as usize,
            PCWSTR::from_raw(freq_label.as_ptr()),
        );

        // Models submenu
        let models_menu = CreatePopupMenu().unwrap();
        let claude_model = native_interop::wide_str(strings.claude_code_model);
        let claude_flags = if show_claude_code {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            models_menu,
            claude_flags,
            IDM_MODEL_CLAUDE_CODE as usize,
            PCWSTR::from_raw(claude_model.as_ptr()),
        );

        let codex_model = native_interop::wide_str(strings.codex_model);
        let codex_flags = if show_codex {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            models_menu,
            codex_flags,
            IDM_MODEL_CODEX as usize,
            PCWSTR::from_raw(codex_model.as_ptr()),
        );

        let antigravity_model = native_interop::wide_str(strings.antigravity_model);
        let antigravity_flags = if show_antigravity {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            models_menu,
            antigravity_flags,
            IDM_MODEL_ANTIGRAVITY as usize,
            PCWSTR::from_raw(antigravity_model.as_ptr()),
        );

        let models_label = native_interop::wide_str(strings.models);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            models_menu.0 as usize,
            PCWSTR::from_raw(models_label.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // Settings submenu
        let settings_menu = CreatePopupMenu().unwrap();

        let theme_menu = CreatePopupMenu().unwrap();
        for (id, mode, label) in [
            (IDM_THEME_SYSTEM, ThemeMode::System, strings.system_default),
            (IDM_THEME_LIGHT, ThemeMode::Light, strings.light_theme),
            (IDM_THEME_DARK, ThemeMode::Dark, strings.dark_theme),
        ] {
            let label = native_interop::wide_str(label);
            let flags = if theme_mode == mode {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                theme_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label.as_ptr()),
            );
        }
        let theme_label = native_interop::wide_str(strings.theme);
        let _ = AppendMenuW(
            settings_menu,
            MF_POPUP,
            theme_menu.0 as usize,
            PCWSTR::from_raw(theme_label.as_ptr()),
        );
        let _ = AppendMenuW(settings_menu, MF_SEPARATOR, 0, PCWSTR::null());

        let startup_str = native_interop::wide_str(strings.start_with_windows);
        let startup_flags = if is_startup_enabled() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            settings_menu,
            startup_flags,
            IDM_START_WITH_WINDOWS as usize,
            PCWSTR::from_raw(startup_str.as_ptr()),
        );

        let reset_pos_str = native_interop::wide_str(strings.reset_position);
        let _ = AppendMenuW(
            settings_menu,
            MENU_ITEM_FLAGS(0),
            IDM_RESET_POSITION as usize,
            PCWSTR::from_raw(reset_pos_str.as_ptr()),
        );

        let language_menu = CreatePopupMenu().unwrap();
        let system_label = native_interop::wide_str(strings.system_default);
        let system_flags = if language_override.is_none() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            language_menu,
            system_flags,
            IDM_LANG_SYSTEM as usize,
            PCWSTR::from_raw(system_label.as_ptr()),
        );

        for language in LanguageId::ALL {
            let id = match language {
                LanguageId::English => IDM_LANG_ENGLISH,
                LanguageId::Dutch => IDM_LANG_DUTCH,
                LanguageId::Spanish => IDM_LANG_SPANISH,
                LanguageId::French => IDM_LANG_FRENCH,
                LanguageId::German => IDM_LANG_GERMAN,
                LanguageId::Japanese => IDM_LANG_JAPANESE,
                LanguageId::Korean => IDM_LANG_KOREAN,
                LanguageId::TraditionalChinese => IDM_LANG_TRADITIONAL_CHINESE,
                LanguageId::SimplifiedChinese => IDM_LANG_SIMPLIFIED_CHINESE,
                LanguageId::Russian => IDM_LANG_RUSSIAN,
                LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
            };
            let label_str = native_interop::wide_str(language.native_name());
            let flags = if language_override == Some(language) {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                language_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let language_label = native_interop::wide_str(strings.language);
        let _ = AppendMenuW(
            settings_menu,
            MF_POPUP,
            language_menu.0 as usize,
            PCWSTR::from_raw(language_label.as_ptr()),
        );

        let _ = AppendMenuW(settings_menu, MF_SEPARATOR, 0, PCWSTR::null());

        let version_label =
            version_action_label(strings, language, install_channel, &update_status);
        let version_str = native_interop::wide_str(&version_label);
        let version_flags = if matches!(
            update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            MF_GRAYED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            settings_menu,
            version_flags,
            IDM_VERSION_ACTION as usize,
            PCWSTR::from_raw(version_str.as_ptr()),
        );

        let settings_label = native_interop::wide_str(strings.settings);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            settings_menu.0 as usize,
            PCWSTR::from_raw(settings_label.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let exit_str = native_interop::wide_str(strings.exit);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            2,
            PCWSTR::from_raw(exit_str.as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let dpi = GetDpiForWindow(hwnd).max(96);
        let font_name = native_interop::wide_str("Segoe UI");
        let font = CreateFontW(
            -menu_px(13, dpi),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let (background, selected, divider, foreground, disabled, accent) = if is_dark {
            (
                Color::new(30, 31, 33),
                Color::new(57, 60, 65),
                Color::new(72, 75, 80),
                Color::new(242, 243, 245),
                Color::new(142, 145, 151),
                Color::new(134, 183, 255),
            )
        } else {
            (
                Color::new(250, 250, 252),
                Color::new(231, 235, 243),
                Color::new(220, 223, 230),
                Color::new(30, 32, 37),
                Color::new(137, 140, 147),
                Color::new(38, 103, 202),
            )
        };
        let palette = MenuPalette {
            background: CreateSolidBrush(COLORREF(background.to_colorref())),
            selected: CreateSolidBrush(COLORREF(selected.to_colorref())),
            divider: CreateSolidBrush(COLORREF(divider.to_colorref())),
            foreground,
            disabled,
            accent,
        };
        MENU_BACKGROUND_COLOR.store(background.to_colorref(), Ordering::Relaxed);
        let mut menu_items = Vec::new();
        if !font.is_invalid()
            && !palette.background.is_invalid()
            && !palette.selected.is_invalid()
            && !palette.divider.is_invalid()
        {
            for popup in [
                menu,
                freq_menu,
                models_menu,
                settings_menu,
                theme_menu,
                language_menu,
            ] {
                style_menu(popup, dpi, font, palette, &mut menu_items);
            }
        }
        let _ = SetForegroundWindow(hwnd);
        let creation_hook = SetWindowsHookExW(
            WH_CBT,
            Some(menu_creation_hook),
            HINSTANCE::default(),
            GetCurrentThreadId(),
        )
        .ok();
        let menu_hook = SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(style_menu_window),
            GetCurrentProcessId(),
            GetCurrentThreadId(),
            native_interop::WINEVENT_OUTOFCONTEXT,
        );
        // The system fade exposes the native frame before the themed popup is painted.
        let _ = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_NOANIMATION,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        if !menu_hook.is_invalid() {
            let _ = UnhookWinEvent(menu_hook);
        }
        if let Some(hook) = creation_hook {
            let _ = UnhookWindowsHookEx(hook);
        }
        let _ = DestroyMenu(menu);
        drop(menu_items);
        if !font.is_invalid() {
            let _ = DeleteObject(font);
        }
        for brush in [palette.background, palette.selected, palette.divider] {
            if !brush.is_invalid() {
                let _ = DeleteObject(brush);
            }
        }
    }
}

fn draw_row(hdc: HDC, x: i32, y: i32, paint: &WidgetPaint<'_>, window: QuotaWindow) {
    let seg_h = sc(SEGMENT_H);
    let active_models = active_model_count(
        paint.claude.visible,
        paint.codex.visible,
        paint.antigravity.visible,
    );
    let segment_count = row_bar_segment_count(active_models);
    let label = match window {
        QuotaWindow::Session => paint.strings.session_window,
        QuotaWindow::Weekly => paint.strings.weekly_window,
    };

    unsafe {
        let _ = SetTextColor(hdc, COLORREF(paint.text_color.to_colorref()));
        let mut label_wide: Vec<u16> = label.encode_utf16().collect();
        let mut label_rect = RECT {
            left: x,
            top: y,
            right: x + sc(LABEL_WIDTH),
            bottom: y + seg_h,
        };
        let _ = DrawTextW(
            hdc,
            &mut label_wide,
            &mut label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let mut model_x = x + sc(LABEL_WIDTH) + sc(LABEL_RIGHT_MARGIN);
        for provider in [paint.claude, paint.codex, paint.antigravity] {
            if !provider.visible {
                continue;
            }

            draw_usage_bar(
                hdc,
                model_x,
                y,
                segment_count,
                provider.value(window),
                &provider.palette,
                &provider.usage_text_color,
            );
            model_x += model_usage_width(segment_count) + sc(MODEL_RIGHT_MARGIN);
        }
    }
}

fn model_usage_width(segment_count: i32) -> i32 {
    (sc(SEGMENT_W) + sc(SEGMENT_GAP)) * segment_count - sc(SEGMENT_GAP)
        + sc(BAR_RIGHT_MARGIN)
        + sc(TEXT_WIDTH)
}

fn draw_usage_bar(
    hdc: HDC,
    bar_x: i32,
    y: i32,
    segment_count: i32,
    value: UsageValuePaint<'_>,
    palette: &UsagePalette,
    text_color: &Color,
) {
    let seg_w = sc(SEGMENT_W);
    let seg_h = sc(SEGMENT_H);
    let seg_gap = sc(SEGMENT_GAP);
    let corner_r = sc(CORNER_RADIUS);

    unsafe {
        let percent_clamped = value.percent.clamp(0.0, 100.0);
        let segment_percent = 100.0 / segment_count as f64;

        for i in 0..segment_count {
            let seg_x = bar_x + i * (seg_w + seg_gap);
            let seg_start = (i as f64) * segment_percent;
            let seg_end = seg_start + segment_percent;
            let fill_color = palette.segment_color(i, segment_count);

            let seg_rect = RECT {
                left: seg_x,
                top: y,
                right: seg_x + seg_w,
                bottom: y + seg_h,
            };

            if percent_clamped >= seg_end {
                draw_rounded_rect(hdc, &seg_rect, &fill_color, corner_r);
            } else if percent_clamped <= seg_start {
                draw_rounded_rect(hdc, &seg_rect, &palette.track, corner_r);
            } else {
                draw_rounded_rect(hdc, &seg_rect, &palette.track, corner_r);
                let fraction = (percent_clamped - seg_start) / segment_percent;
                let fill_width = (seg_w as f64 * fraction) as i32;
                if fill_width > 0 {
                    let fill_rect = RECT {
                        left: seg_x,
                        top: y,
                        right: seg_x + fill_width,
                        bottom: y + seg_h,
                    };
                    let rgn = CreateRoundRectRgn(
                        seg_rect.left,
                        seg_rect.top,
                        seg_rect.right + 1,
                        seg_rect.bottom + 1,
                        corner_r * 2,
                        corner_r * 2,
                    );
                    let _ = SelectClipRgn(hdc, rgn);
                    let brush = CreateSolidBrush(COLORREF(fill_color.to_colorref()));
                    FillRect(hdc, &fill_rect, brush);
                    let _ = DeleteObject(brush);
                    let _ = SelectClipRgn(hdc, HRGN::default());
                    let _ = DeleteObject(rgn);
                }
            }
        }

        let text_x = bar_x + segment_count * (seg_w + seg_gap) - seg_gap + sc(BAR_RIGHT_MARGIN);
        let mut text_wide: Vec<u16> = value.text.encode_utf16().collect();
        let mut text_rect = RECT {
            left: text_x,
            top: y,
            right: text_x + sc(TEXT_WIDTH),
            bottom: y + seg_h,
        };
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));
        let _ = DrawTextW(
            hdc,
            &mut text_wide,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
    }
}

fn draw_rounded_rect(hdc: HDC, rect: &RECT, color: &Color, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let rgn = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        let _ = FillRgn(hdc, rgn, brush);
        let _ = DeleteObject(rgn);
        let _ = DeleteObject(brush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(color: Color) -> (u8, u8, u8) {
        (color.r, color.g, color.b)
    }

    #[test]
    fn codex_is_the_default_provider() {
        let settings = SettingsFile::default();
        assert!(!settings.show_claude_code);
        assert!(settings.show_codex);
        assert!(!settings.show_antigravity);
        assert_eq!(settings.theme, ThemeMode::System);
    }

    #[test]
    fn existing_settings_default_to_system_theme() {
        let settings: SettingsFile = serde_json::from_str(r#"{"tray_offset":12}"#).unwrap();
        assert_eq!(settings.theme, ThemeMode::System);
    }

    #[test]
    fn explicit_dark_theme_survives_settings_round_trip() {
        let settings: SettingsFile = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        let saved = serde_json::to_string(&settings).unwrap();
        let restored: SettingsFile = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.theme, ThemeMode::Dark);
    }

    #[test]
    fn interval_and_language_survive_settings_round_trip() {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"poll_interval_ms":300000,"language":"ru"}"#).unwrap();
        let saved = serde_json::to_string(&settings).unwrap();
        let restored: SettingsFile = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.poll_interval_ms, POLL_5_MIN);
        assert_eq!(restored.language.as_deref(), Some("ru"));
    }

    #[test]
    fn widget_offset_stays_before_notification_area() {
        assert_eq!(clamp_widget_offset(-1920, -200, 312, -20), 0);
        assert_eq!(clamp_widget_offset(-1920, -200, 312, 2_000), 1_408);
        assert_eq!(clamp_widget_offset(0, 250, 312, 100), 0);
    }

    #[test]
    fn taskbar_geometry_requires_space_before_the_tray() {
        let taskbar = RECT {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        let tray = RECT {
            left: 1608,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        assert!(usable_taskbar_space(taskbar, tray, 217, 46));
        assert!(!usable_taskbar_space(
            taskbar,
            RECT { left: 180, ..tray },
            217,
            46
        ));
        assert!(!usable_taskbar_space(
            taskbar,
            RECT {
                right: 1608,
                ..tray
            },
            217,
            46
        ));
    }

    #[test]
    fn model_toggle_keeps_at_least_one_provider_enabled() {
        let mut claude = false;
        let mut codex = true;
        let mut antigravity = false;

        assert!(!toggle_model_visibility(
            &mut claude,
            &mut codex,
            &mut antigravity,
            IDM_MODEL_CODEX,
        ));
        assert!(codex);

        assert!(toggle_model_visibility(
            &mut claude,
            &mut codex,
            &mut antigravity,
            IDM_MODEL_CLAUDE_CODE,
        ));
        assert!(toggle_model_visibility(
            &mut claude,
            &mut codex,
            &mut antigravity,
            IDM_MODEL_CODEX,
        ));
        assert!(claude);
        assert!(!codex);

        assert!(!toggle_model_visibility(
            &mut claude,
            &mut codex,
            &mut antigravity,
            IDM_MODEL_CLAUDE_CODE,
        ));
        assert!(claude);
    }

    #[test]
    fn providers_have_distinct_gradient_endpoints() {
        let claude = claude_palette(false);
        let codex = codex_palette(false);
        let antigravity = antigravity_palette(false);

        assert_eq!(rgb(claude.segment_color(0, 10)), (0xD9, 0x77, 0x57));
        assert_eq!(rgb(claude.segment_color(9, 10)), (0xC4, 0x50, 0x20));
        assert_eq!(rgb(codex.segment_color(0, 10)), (0xA9, 0x9B, 0xFF));
        assert_eq!(rgb(codex.segment_color(9, 10)), (0x30, 0x2F, 0xE5));
        assert_eq!(rgb(antigravity.segment_color(0, 10)), (0x42, 0x85, 0xF4));
        assert_eq!(rgb(antigravity.segment_color(9, 10)), (0xA1, 0x42, 0xF4));
    }

    #[test]
    fn tray_badge_falls_back_to_an_available_weekly_window() {
        let mut usage = UsageData::default();
        usage.weekly.available = true;
        usage.weekly.percentage = 37.5;
        assert_eq!(badge_percentage(&usage), Some(37.5));

        usage.session.available = true;
        usage.session.percentage = 12.0;
        assert_eq!(badge_percentage(&usage), Some(12.0));
    }

    #[test]
    fn authentication_notifications_are_deduplicated_until_recovery() {
        let mut notified = false;

        assert!(update_auth_notification_latch(
            &mut notified,
            true,
            false,
            Some(poller::PollError::NoCredentials),
            false,
        ));
        assert!(!update_auth_notification_latch(
            &mut notified,
            true,
            false,
            Some(poller::PollError::NoCredentials),
            false,
        ));
        assert!(update_auth_notification_latch(
            &mut notified,
            true,
            false,
            Some(poller::PollError::NoCredentials),
            true,
        ));

        assert!(!update_auth_notification_latch(
            &mut notified,
            true,
            true,
            None,
            false,
        ));
        assert!(!notified);
        assert!(update_auth_notification_latch(
            &mut notified,
            true,
            false,
            Some(poller::PollError::AuthRequired),
            false,
        ));
    }

    #[test]
    fn transient_request_failures_do_not_show_authentication_notifications() {
        let mut notified = false;
        assert!(!update_auth_notification_latch(
            &mut notified,
            true,
            false,
            Some(poller::PollError::RequestFailed),
            false,
        ));
        assert!(!notified);
    }
}
