use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetKeyState, IsWindowEnabled, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VK_BACK, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::localization::{LanguageId, Strings};
use crate::native_interop::{self, Color};
use crate::theme::ThemeMode;
use crate::tray_icon;
use crate::window::{
    self, IDM_FREQ_15MIN, IDM_FREQ_1HOUR, IDM_FREQ_1MIN, IDM_FREQ_5MIN, IDM_MODEL_ANTIGRAVITY,
    IDM_MODEL_CLAUDE_CODE, IDM_MODEL_CODEX, IDM_RESET_POSITION, IDM_START_WITH_WINDOWS,
    IDM_THEME_DARK, IDM_THEME_LIGHT, IDM_THEME_SYSTEM, IDM_VERSION_ACTION,
};

const BUTTON_BASE: usize = 1000;
const WIDTH_DIP: i32 = 312;
const ROW_DIP: i32 = 40;
const HEADER_DIP: i32 = 70;
const INSET_DIP: i32 = 8;
const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_POPUP_ACTION: u32 = native_interop::WM_APP + 110;
const WM_POPUP_REFRESH: u32 = native_interop::WM_APP + 111;

static CLASS_READY: OnceLock<bool> = OnceLock::new();
static ACTIVE_POPUP: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone)]
pub(crate) struct MenuSnapshot {
    pub strings: Strings,
    pub interval: u32,
    pub theme: ThemeMode,
    pub language_override: Option<LanguageId>,
    pub widget_visible: bool,
    pub claude_visible: bool,
    pub codex_visible: bool,
    pub antigravity_visible: bool,
    pub startup_enabled: bool,
    pub version_label: String,
    pub update_disabled: bool,
    pub dark: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Main,
    Models,
    Frequency,
    Settings,
    Theme,
    Language,
}

impl Page {
    fn back(self) -> Self {
        match self {
            Self::Theme | Self::Language => Self::Settings,
            _ => Self::Main,
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Back,
    Navigate(Page),
    Command(u16),
}

struct Row {
    label: String,
    icon: &'static str,
    action: Action,
    checked: bool,
    disabled: bool,
    divider: bool,
    top: i32,
}

impl Row {
    fn new(label: &str, icon: &'static str, action: Action) -> Self {
        Self {
            label: label.to_owned(),
            icon,
            action,
            checked: false,
            disabled: false,
            divider: false,
            top: 0,
        }
    }

    fn checked(mut self, value: bool) -> Self {
        self.checked = value;
        self
    }

    fn disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }

    fn divider(mut self) -> Self {
        self.divider = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskbarEdge {
    Top,
    Bottom,
    Left,
    Right,
    Unknown,
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

fn popup_origin(anchor: POINT, work: RECT, width: i32, height: i32, edge: TaskbarEdge) -> POINT {
    let width = width.min(work.right - work.left);
    let height = height.min(work.bottom - work.top);
    let x = match edge {
        TaskbarEdge::Left => work.left,
        TaskbarEdge::Right => work.right - width,
        _ => anchor.x - width / 2,
    };
    let y = match edge {
        TaskbarEdge::Top => work.top,
        TaskbarEdge::Bottom => work.bottom - height,
        _ => anchor.y - height,
    };
    POINT {
        x: x.clamp(work.left, work.right - width),
        y: y.clamp(work.top, work.bottom - height),
    }
}

fn taskbar_edge(anchor: POINT, monitor: RECT) -> TaskbarEdge {
    native_interop::find_taskbars()
        .into_iter()
        .find(|bar| {
            anchor.x >= bar.rect.left
                && anchor.x < bar.rect.right
                && anchor.y >= bar.rect.top
                && anchor.y < bar.rect.bottom
        })
        .map(|bar| {
            if bar.rect.right - bar.rect.left >= bar.rect.bottom - bar.rect.top {
                if bar.rect.top < (monitor.top + monitor.bottom) / 2 {
                    TaskbarEdge::Top
                } else {
                    TaskbarEdge::Bottom
                }
            } else if bar.rect.left < (monitor.left + monitor.right) / 2 {
                TaskbarEdge::Left
            } else {
                TaskbarEdge::Right
            }
        })
        .unwrap_or(TaskbarEdge::Unknown)
}

fn rows_for(page: Page, snapshot: &MenuSnapshot) -> Vec<Row> {
    let s = snapshot.strings;
    let mut rows = Vec::new();
    if page != Page::Main {
        rows.push(Row::new(s.back, "←", Action::Back));
    }
    match page {
        Page::Main => {
            rows.push(Row::new(s.refresh, "↻", Action::Command(1)));
            rows.push(
                Row::new(
                    s.show_widget,
                    "◉",
                    Action::Command(tray_icon::IDM_TOGGLE_WIDGET),
                )
                .checked(snapshot.widget_visible),
            );
            rows.push(Row::new(s.models, "▤", Action::Navigate(Page::Models)));
            rows.push(Row::new(
                s.update_frequency,
                "◷",
                Action::Navigate(Page::Frequency),
            ));
            rows.push(Row::new(s.settings, "⚙", Action::Navigate(Page::Settings)));
            rows.push(Row::new(s.exit, "⏻", Action::Command(2)).divider());
        }
        Page::Models => {
            rows.push(
                Row::new(s.codex_model, "", Action::Command(IDM_MODEL_CODEX))
                    .checked(snapshot.codex_visible),
            );
            rows.push(
                Row::new(
                    s.claude_code_model,
                    "",
                    Action::Command(IDM_MODEL_CLAUDE_CODE),
                )
                .checked(snapshot.claude_visible),
            );
            rows.push(
                Row::new(
                    s.antigravity_model,
                    "",
                    Action::Command(IDM_MODEL_ANTIGRAVITY),
                )
                .checked(snapshot.antigravity_visible),
            );
        }
        Page::Frequency => {
            for (label, id, interval) in [
                (s.one_minute, IDM_FREQ_1MIN, 60_000),
                (s.five_minutes, IDM_FREQ_5MIN, 300_000),
                (s.fifteen_minutes, IDM_FREQ_15MIN, 900_000),
                (s.one_hour, IDM_FREQ_1HOUR, 3_600_000),
            ] {
                rows.push(
                    Row::new(label, "", Action::Command(id)).checked(snapshot.interval == interval),
                );
            }
        }
        Page::Settings => {
            rows.push(Row::new(s.language, "文", Action::Navigate(Page::Language)));
            rows.push(Row::new(s.theme, "◐", Action::Navigate(Page::Theme)));
            rows.push(
                Row::new(
                    s.start_with_windows,
                    "",
                    Action::Command(IDM_START_WITH_WINDOWS),
                )
                .checked(snapshot.startup_enabled),
            );
            rows.push(Row::new(
                s.reset_position,
                "",
                Action::Command(IDM_RESET_POSITION),
            ));
            rows.push(
                Row::new(
                    &snapshot.version_label,
                    "",
                    Action::Command(IDM_VERSION_ACTION),
                )
                .disabled(snapshot.update_disabled)
                .divider(),
            );
        }
        Page::Theme => {
            for (label, id, mode) in [
                (s.system_default, IDM_THEME_SYSTEM, ThemeMode::System),
                (s.light_theme, IDM_THEME_LIGHT, ThemeMode::Light),
                (s.dark_theme, IDM_THEME_DARK, ThemeMode::Dark),
            ] {
                rows.push(Row::new(label, "", Action::Command(id)).checked(snapshot.theme == mode));
            }
        }
        Page::Language => {
            rows.push(
                Row::new(
                    s.system_default,
                    "",
                    Action::Command(window::IDM_LANG_SYSTEM),
                )
                .checked(snapshot.language_override.is_none()),
            );
            for language in LanguageId::ALL {
                rows.push(
                    Row::new(
                        language.native_name(),
                        "",
                        Action::Command(window::language_command_id(language)),
                    )
                    .checked(snapshot.language_override == Some(language)),
                );
            }
        }
    }
    rows
}

struct PopupState {
    widget: HWND,
    icon: HICON,
    anchor: POINT,
    work: RECT,
    edge: TaskbarEdge,
    snapshot: MenuSnapshot,
    page: Page,
    rows: Vec<Row>,
    buttons: Vec<HWND>,
    hot: Option<usize>,
    scroll: i32,
    content_height: i32,
    height: i32,
    dpi: u32,
    font: HFONT,
}

fn state(hwnd: HWND) -> Option<&'static RefCell<PopupState>> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut RefCell<PopupState>;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

fn menu_title(page: Page, s: Strings) -> &'static str {
    match page {
        Page::Main => s.window_title,
        Page::Models => s.models,
        Page::Frequency => s.update_frequency,
        Page::Settings => s.settings,
        Page::Theme => s.theme,
        Page::Language => s.language,
    }
}

fn color(dark: bool, kind: u8) -> Color {
    match (dark, kind) {
        (true, 0) => Color::new(32, 33, 39),
        (true, 1) => Color::new(44, 45, 53),
        (true, 2) => Color::new(244, 244, 245),
        (true, 3) => Color::new(180, 181, 193),
        (true, 4) => Color::new(139, 139, 255),
        (true, _) => Color::new(65, 66, 75),
        (false, 0) => Color::new(250, 250, 252),
        (false, 1) => Color::new(235, 237, 243),
        (false, 2) => Color::new(24, 24, 27),
        (false, 3) => Color::new(102, 106, 121),
        (false, 4) => Color::new(87, 87, 200),
        (false, _) => Color::new(211, 214, 224),
    }
}

unsafe fn fill(hdc: HDC, rect: &RECT, value: Color) {
    let brush = CreateSolidBrush(COLORREF(value.to_colorref()));
    FillRect(hdc, rect, brush);
    let _ = DeleteObject(brush);
}

unsafe fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, flags: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let _ = DrawTextW(hdc, &mut wide, rect, flags);
}

unsafe fn draw_button(hwnd: HWND, draw: &DRAWITEMSTRUCT) {
    let Some(cell) = state(hwnd) else { return };
    let Ok(s) = cell.try_borrow() else { return };
    let index = draw.CtlID as usize - BUTTON_BASE;
    let Some(row) = s.rows.get(index) else { return };
    let dpi = s.dpi;
    let dark = s.snapshot.dark;
    fill(draw.hDC, &draw.rcItem, color(dark, 0));
    let focused = (draw.itemState.0 & ODS_FOCUS.0) != 0;
    let pressed = (draw.itemState.0 & ODS_SELECTED.0) != 0;
    if !row.disabled && (s.hot == Some(index) || focused || pressed) {
        let region = CreateRoundRectRgn(
            scale(2, dpi),
            scale(2, dpi),
            draw.rcItem.right - scale(2, dpi),
            draw.rcItem.bottom - scale(2, dpi),
            scale(8, dpi),
            scale(8, dpi),
        );
        let brush = CreateSolidBrush(COLORREF(color(dark, 1).to_colorref()));
        let _ = FillRgn(draw.hDC, region, brush);
        if focused {
            let ring = CreateSolidBrush(COLORREF(color(dark, 4).to_colorref()));
            let _ = FrameRgn(draw.hDC, region, ring, scale(1, dpi), scale(1, dpi));
            let _ = DeleteObject(ring);
        }
        let _ = DeleteObject(brush);
        let _ = DeleteObject(region);
    }
    let old_font = SelectObject(draw.hDC, s.font);
    let old_mode = SetBkMode(draw.hDC, TRANSPARENT);
    let foreground = if row.disabled {
        color(dark, 3)
    } else {
        color(dark, 2)
    };
    let old_color = SetTextColor(draw.hDC, COLORREF(foreground.to_colorref()));
    if !row.icon.is_empty() {
        let mut icon_rect = RECT {
            left: scale(10, dpi),
            top: 0,
            right: scale(36, dpi),
            bottom: draw.rcItem.bottom,
        };
        let _ = SetTextColor(draw.hDC, COLORREF(color(dark, 3).to_colorref()));
        draw_text(
            draw.hDC,
            row.icon,
            &mut icon_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        let _ = SetTextColor(draw.hDC, COLORREF(foreground.to_colorref()));
    }
    let mut label_rect = RECT {
        left: scale(40, dpi),
        top: 0,
        right: draw.rcItem.right - scale(32, dpi),
        bottom: draw.rcItem.bottom,
    };
    draw_text(
        draw.hDC,
        &row.label,
        &mut label_rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    if row.checked || matches!(row.action, Action::Navigate(_)) {
        let symbol = if row.checked { "✓" } else { "›" };
        let mut mark_rect = RECT {
            left: draw.rcItem.right - scale(30, dpi),
            top: 0,
            right: draw.rcItem.right - scale(8, dpi),
            bottom: draw.rcItem.bottom,
        };
        let mark_color = if row.checked {
            color(dark, 4)
        } else {
            color(dark, 3)
        };
        let _ = SetTextColor(draw.hDC, COLORREF(mark_color.to_colorref()));
        draw_text(
            draw.hDC,
            symbol,
            &mut mark_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    let _ = SetTextColor(draw.hDC, old_color);
    let _ = SetBkMode(draw.hDC, BACKGROUND_MODE(old_mode as u32));
    SelectObject(draw.hDC, old_font);
}

unsafe fn paint(hwnd: HWND) {
    let Some(cell) = state(hwnd) else { return };
    let Ok(s) = cell.try_borrow() else { return };
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    fill(hdc, &client, color(s.snapshot.dark, 0));
    let old_font = SelectObject(hdc, s.font);
    let old_mode = SetBkMode(hdc, TRANSPARENT);
    let old_color = SetTextColor(hdc, COLORREF(color(s.snapshot.dark, 2).to_colorref()));
    if !s.icon.is_invalid() {
        let _ = DrawIconEx(
            hdc,
            scale(16, s.dpi),
            scale(18, s.dpi),
            s.icon,
            scale(32, s.dpi),
            scale(32, s.dpi),
            0,
            HBRUSH::default(),
            DI_NORMAL,
        );
    }
    let title_left = scale(if s.icon.is_invalid() { 16 } else { 56 }, s.dpi);
    let mut title = RECT {
        left: title_left,
        top: scale(15, s.dpi),
        right: client.right - scale(12, s.dpi),
        bottom: scale(40, s.dpi),
    };
    draw_text(
        hdc,
        menu_title(s.page, s.snapshot.strings),
        &mut title,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    let _ = SetTextColor(hdc, COLORREF(color(s.snapshot.dark, 3).to_colorref()));
    let mut subtitle = RECT {
        left: title_left,
        top: scale(39, s.dpi),
        right: client.right - scale(12, s.dpi),
        bottom: scale(56, s.dpi),
    };
    draw_text(
        hdc,
        concat!("v", env!("CARGO_PKG_VERSION")),
        &mut subtitle,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
    let divider = RECT {
        left: scale(8, s.dpi),
        top: scale(HEADER_DIP - 4, s.dpi),
        right: client.right - scale(8, s.dpi),
        bottom: scale(HEADER_DIP - 4, s.dpi) + 1,
    };
    fill(hdc, &divider, color(s.snapshot.dark, 5));
    for row in &s.rows {
        if row.divider {
            let y = row.top - s.scroll - scale(4, s.dpi);
            if y >= scale(HEADER_DIP, s.dpi) && y < client.bottom {
                let line = RECT {
                    left: scale(8, s.dpi),
                    top: y,
                    right: client.right - scale(8, s.dpi),
                    bottom: y + 1,
                };
                fill(hdc, &line, color(s.snapshot.dark, 5));
            }
        }
    }
    let outline = CreateRoundRectRgn(
        0,
        0,
        client.right,
        client.bottom,
        scale(12, s.dpi),
        scale(12, s.dpi),
    );
    let border = CreateSolidBrush(COLORREF(color(s.snapshot.dark, 5).to_colorref()));
    let _ = FrameRgn(hdc, outline, border, 1, 1);
    let _ = DeleteObject(border);
    let _ = DeleteObject(outline);
    let _ = SetTextColor(hdc, old_color);
    let _ = SetBkMode(hdc, BACKGROUND_MODE(old_mode as u32));
    SelectObject(hdc, old_font);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn work_area(anchor: POINT) -> Option<(RECT, RECT)> {
    let monitor = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
    if monitor.0.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
        return None;
    }
    Some((info.rcWork, info.rcMonitor))
}

unsafe fn make_font(dpi: u32) -> HFONT {
    CreateFontW(
        -scale(13, dpi),
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
        w!("Segoe UI"),
    )
}

unsafe fn layout_buttons(hwnd: HWND) {
    let Some(cell) = state(hwnd) else { return };
    let Ok(s) = cell.try_borrow() else { return };
    let inset = scale(INSET_DIP, s.dpi);
    let width = scale(WIDTH_DIP, s.dpi) - 2 * inset;
    let row_height = scale(ROW_DIP, s.dpi);
    for (index, button) in s.buttons.iter().enumerate() {
        let y = s.rows[index].top - s.scroll;
        let show = y + row_height > scale(HEADER_DIP, s.dpi) && y < s.height;
        let clip_top = (scale(HEADER_DIP, s.dpi) - y).clamp(0, row_height);
        let clip_bottom = (s.height - y).clamp(0, row_height);
        if show && (clip_top > 0 || clip_bottom < row_height) {
            let region = CreateRectRgn(0, clip_top, width, clip_bottom);
            if SetWindowRgn(*button, region, false) == 0 {
                let _ = DeleteObject(region);
            }
        } else {
            let _ = SetWindowRgn(*button, HRGN::default(), false);
        }
        let _ = SetWindowPos(
            *button,
            HWND::default(),
            inset,
            y,
            width,
            row_height,
            SWP_NOZORDER | SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW },
        );
        let _ = InvalidateRect(*button, None, false);
    }
    let _ = InvalidateRect(hwnd, None, false);
}

unsafe fn rebuild(hwnd: HWND, focus_index: usize) {
    let Some(cell) = state(hwnd) else { return };
    let Ok(mut s) = cell.try_borrow_mut() else {
        return;
    };
    let old_buttons = std::mem::take(&mut s.buttons);
    for button in old_buttons {
        let _ = RemoveWindowSubclass(button, Some(button_proc), 1);
        let _ = DestroyWindow(button);
    }
    s.rows = rows_for(s.page, &s.snapshot);
    s.hot = None;
    s.scroll = 0;
    let dpi = s.dpi;
    let mut y = scale(HEADER_DIP + 8, dpi);
    for row in &mut s.rows {
        if row.divider {
            y += scale(10, dpi);
        }
        row.top = y;
        y += scale(ROW_DIP, dpi);
    }
    s.content_height = y + scale(8, dpi);
    let available = s.work.bottom - s.work.top - scale(12, dpi);
    s.height = s.content_height.min(available.max(1));
    let width = scale(WIDTH_DIP, dpi).min(s.work.right - s.work.left);
    let origin = popup_origin(s.anchor, s.work, width, s.height, s.edge);
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        origin.x,
        origin.y,
        width,
        s.height,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    let region = CreateRoundRectRgn(
        0,
        0,
        width + 1,
        s.height + 1,
        scale(12, dpi),
        scale(12, dpi),
    );
    if SetWindowRgn(hwnd, region, true) == 0 {
        let _ = DeleteObject(region);
    }
    let button_width = width - 2 * scale(INSET_DIP, dpi);
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
    for index in 0..s.rows.len() {
        let row = &s.rows[index];
        let accessible_label = if row.checked {
            format!("{} ({})", row.label, s.snapshot.strings.selected)
        } else {
            row.label.clone()
        };
        let label = native_interop::wide_str(&accessible_label);
        let top = row.top;
        let disabled = row.disabled;
        let id = BUTTON_BASE + index;
        if let Ok(button) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            PCWSTR::from_raw(label.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
            scale(INSET_DIP, dpi),
            top,
            button_width,
            scale(ROW_DIP, dpi),
            hwnd,
            HMENU(id as *mut _),
            instance,
            None,
        ) {
            if disabled {
                let _ = EnableWindow(button, false);
            }
            let _ = SetWindowSubclass(button, Some(button_proc), 1, hwnd.0 as usize);
            s.buttons.push(button);
        }
    }
    let focus_button = s
        .buttons
        .get(focus_index)
        .copied()
        .or_else(|| s.buttons.first().copied());
    drop(s);
    layout_buttons(hwnd);
    if let Some(button) = focus_button {
        if IsWindowEnabled(button).as_bool() {
            let _ = SetFocus(button);
        }
    }
}

unsafe fn change_page(hwnd: HWND, page: Page) {
    if let Some(cell) = state(hwnd) {
        if let Ok(mut s) = cell.try_borrow_mut() {
            s.page = page;
        }
    }
    rebuild(hwnd, 0);
}

unsafe fn command(hwnd: HWND, index: usize) {
    let (action, widget, page) = {
        let Some(cell) = state(hwnd) else { return };
        let Ok(s) = cell.try_borrow() else { return };
        let Some(row) = s.rows.get(index) else { return };
        if row.disabled {
            return;
        }
        (row.action, s.widget, s.page)
    };
    match action {
        Action::Back => change_page(hwnd, page.back()),
        Action::Navigate(page) => change_page(hwnd, page),
        Action::Command(id) => {
            if matches!(id, 1 | 2 | IDM_VERSION_ACTION | IDM_RESET_POSITION) {
                let _ = DestroyWindow(hwnd);
                let _ = PostMessageW(widget, WM_COMMAND, WPARAM(id as usize), LPARAM(0));
                return;
            }
            let _ = SendMessageW(widget, WM_COMMAND, WPARAM(id as usize), LPARAM(0));
            let snapshot = window::menu_snapshot();
            if let Some(cell) = state(hwnd) {
                if let Ok(mut s) = cell.try_borrow_mut() {
                    s.snapshot = snapshot;
                }
            }
            rebuild(hwnd, index);
        }
    }
}

unsafe fn focus_next(parent: HWND, current: HWND, step: i32) {
    let Some(cell) = state(parent) else { return };
    let Ok(mut s) = cell.try_borrow_mut() else {
        return;
    };
    if s.buttons.is_empty() {
        return;
    }
    let current = s
        .buttons
        .iter()
        .position(|button| *button == current)
        .unwrap_or(0) as i32;
    let len = s.buttons.len() as i32;
    for offset in 1..=len {
        let next = (current + offset * step).rem_euclid(len) as usize;
        let button = s.buttons[next];
        if IsWindowEnabled(button).as_bool() {
            let top = s.rows[next].top;
            let view_top = scale(HEADER_DIP, s.dpi);
            let view_bottom = s.height - scale(ROW_DIP, s.dpi);
            if top - s.scroll < view_top {
                s.scroll = top - view_top;
            } else if top - s.scroll > view_bottom {
                s.scroll = top - view_bottom;
            }
            s.scroll = s.scroll.clamp(0, (s.content_height - s.height).max(0));
            drop(s);
            layout_buttons(parent);
            let _ = SetFocus(button);
            break;
        }
    }
}

unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    ref_data: usize,
) -> LRESULT {
    let parent = HWND(ref_data as *mut _);
    match msg {
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            if key == VK_ESCAPE.0 {
                let _ = DestroyWindow(parent);
                return LRESULT(0);
            }
            if key == VK_BACK.0 || key == VK_LEFT.0 {
                let back = state(parent)
                    .and_then(|cell| cell.try_borrow().ok().map(|s| s.page))
                    .filter(|page| *page != Page::Main)
                    .map(Page::back);
                if let Some(page) = back {
                    change_page(parent, page);
                    return LRESULT(0);
                }
            }
            if key == VK_DOWN.0 || key == VK_UP.0 || key == VK_TAB.0 {
                let backwards =
                    key == VK_UP.0 || (key == VK_TAB.0 && GetKeyState(VK_SHIFT.0 as i32) < 0);
                focus_next(parent, hwnd, if backwards { -1 } else { 1 });
                return LRESULT(0);
            }
            if key == VK_RETURN.0 {
                let _ = SendMessageW(hwnd, BM_CLICK, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
        WM_MOUSEMOVE => {
            let index = GetDlgCtrlID(hwnd) as usize - BUTTON_BASE;
            if let Some(cell) = state(parent) {
                if let Ok(mut s) = cell.try_borrow_mut() {
                    if s.hot == Some(index) {
                        return DefSubclassProc(hwnd, msg, wparam, lparam);
                    }
                    let old = s.hot.replace(index);
                    if let Some(old) = old.and_then(|old| s.buttons.get(old).copied()) {
                        let _ = InvalidateRect(old, None, false);
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
            }
        }
        WM_MOUSELEAVE => {
            if let Some(cell) = state(parent) {
                if let Ok(mut s) = cell.try_borrow_mut() {
                    s.hot = None;
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
        }
        WM_MOUSEWHEEL => {
            let _ = SendMessageW(parent, WM_MOUSEWHEEL, wparam, lparam);
            return LRESULT(0);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DRAWITEM if lparam.0 != 0 => {
            draw_button(hwnd, &*(lparam.0 as *const DRAWITEMSTRUCT));
            LRESULT(1)
        }
        WM_COMMAND if lparam.0 != 0 && ((wparam.0 >> 16) as u32) == BN_CLICKED => {
            let id = wparam.0 & 0xffff;
            if id >= BUTTON_BASE {
                let _ = PostMessageW(hwnd, WM_POPUP_ACTION, WPARAM(id - BUTTON_BASE), lparam);
            }
            LRESULT(0)
        }
        WM_POPUP_ACTION => {
            let index = wparam.0;
            let valid = state(hwnd)
                .and_then(|cell| {
                    cell.try_borrow()
                        .ok()
                        .map(|s| s.buttons.get(index).copied())
                })
                .flatten()
                .is_some_and(|button| button.0 as isize == lparam.0);
            if valid {
                command(hwnd, index);
            }
            LRESULT(0)
        }
        WM_POPUP_REFRESH => {
            let snapshot = window::refresh_menu_snapshot();
            if let Some(cell) = state(hwnd) {
                if let Ok(mut s) = cell.try_borrow_mut() {
                    s.snapshot = snapshot;
                }
            }
            rebuild(hwnd, 0);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(cell) = state(hwnd) {
                if let Ok(mut s) = cell.try_borrow_mut() {
                    let delta = ((wparam.0 >> 16) as u16 as i16) as i32;
                    let amount = scale(ROW_DIP * 3, s.dpi);
                    s.scroll = (s.scroll - delta.signum() * amount)
                        .clamp(0, (s.content_height - s.height).max(0));
                }
            }
            layout_buttons(hwnd);
            LRESULT(0)
        }
        WM_ACTIVATE if (wparam.0 & 0xffff) == WA_INACTIVE as usize => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DPICHANGED_MSG => {
            if let Some(cell) = state(hwnd) {
                if let Ok(mut s) = cell.try_borrow_mut() {
                    s.dpi = (wparam.0 & 0xffff) as u32;
                    if let Some((work, monitor)) = work_area(s.anchor) {
                        s.work = work;
                        s.edge = taskbar_edge(s.anchor, monitor);
                    }
                    if !s.font.is_invalid() {
                        let _ = DeleteObject(s.font);
                    }
                    s.font = make_font(s.dpi);
                }
            }
            rebuild(hwnd, 0);
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            let _ = PostMessageW(hwnd, WM_POPUP_REFRESH, WPARAM(0), LPARAM(0));
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DISPLAYCHANGE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            ACTIVE_POPUP
                .compare_exchange(hwnd.0 as isize, 0, Ordering::SeqCst, Ordering::SeqCst)
                .ok();
            let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut RefCell<PopupState>;
            if !ptr.is_null() {
                let cell = Box::from_raw(ptr);
                let s = cell.borrow();
                if !s.font.is_invalid() {
                    let _ = DeleteObject(s.font);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub(crate) fn close() {
    let hwnd = ACTIVE_POPUP.swap(0, Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = DestroyWindow(HWND(hwnd as *mut _));
        }
    }
}

pub(crate) fn refresh() {
    let hwnd = ACTIVE_POPUP.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(HWND(hwnd as *mut _), WM_POPUP_REFRESH, WPARAM(0), LPARAM(0));
        }
    }
}

pub(crate) fn show(widget: HWND) -> bool {
    close();
    let registered = *CLASS_READY.get_or_init(|| unsafe {
        let instance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(popup_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("CodexUsageMonitorMenu"),
            ..Default::default()
        };
        RegisterClassExW(&class) != 0
    });
    if !registered {
        return false;
    }
    unsafe {
        let mut anchor = POINT::default();
        if GetCursorPos(&mut anchor).is_err() {
            return false;
        }
        let Some((work, monitor)) = work_area(anchor) else {
            return false;
        };
        let edge = taskbar_edge(anchor, monitor);
        let instance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("CodexUsageMonitorMenu"),
            w!(""),
            WS_POPUP,
            anchor.x,
            anchor.y,
            1,
            1,
            HWND::default(),
            HMENU::default(),
            instance,
            None,
        ) else {
            return false;
        };
        let dpi = GetDpiForWindow(hwnd).max(96);
        let snapshot = window::menu_snapshot();
        let popup = Box::new(RefCell::new(PopupState {
            widget,
            icon: HICON(
                SendMessageW(widget, WM_GETICON, WPARAM(ICON_SMALL as usize), LPARAM(0)).0
                    as *mut _,
            ),
            anchor,
            work,
            edge,
            snapshot,
            page: Page::Main,
            rows: Vec::new(),
            buttons: Vec::new(),
            hot: None,
            scroll: 0,
            content_height: 0,
            height: 0,
            dpi,
            font: make_font(dpi),
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(popup) as isize);
        ACTIVE_POPUP.store(hwnd.0 as isize, Ordering::SeqCst);
        rebuild(hwnd, 0);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        let first = state(hwnd).and_then(|cell| {
            cell.try_borrow()
                .ok()
                .and_then(|s| s.buttons.first().copied())
        });
        if let Some(first) = first {
            let _ = SetFocus(first);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_clamps_negative_monitor_and_edges() {
        let work = RECT {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        };
        let anchor = POINT { x: -5, y: 1079 };
        assert_eq!(
            popup_origin(anchor, work, 312, 348, TaskbarEdge::Bottom).x,
            -312
        );
        assert_eq!(
            popup_origin(anchor, work, 312, 348, TaskbarEdge::Bottom).y,
            632
        );
        assert_eq!(
            popup_origin(anchor, work, 624, 696, TaskbarEdge::Right).x,
            -624
        );
        assert_eq!(
            popup_origin(anchor, work, 624, 696, TaskbarEdge::Top).y,
            -100
        );
    }

    #[test]
    fn placement_stays_inside_narrow_work_area() {
        let work = RECT {
            left: 100,
            top: 50,
            right: 400,
            bottom: 250,
        };
        let origin = popup_origin(
            POINT { x: 390, y: 240 },
            work,
            500,
            400,
            TaskbarEdge::Unknown,
        );
        assert_eq!(origin.x, 100);
        assert_eq!(origin.y, 50);
    }

    #[test]
    fn nested_pages_have_reliable_back_targets() {
        assert_eq!(Page::Models.back(), Page::Main);
        assert_eq!(Page::Frequency.back(), Page::Main);
        assert_eq!(Page::Settings.back(), Page::Main);
        assert_eq!(Page::Theme.back(), Page::Settings);
        assert_eq!(Page::Language.back(), Page::Settings);
    }

    #[test]
    fn dimensions_scale_across_supported_dpi_values() {
        for dpi in [96, 120, 144, 168, 192] {
            assert_eq!(scale(WIDTH_DIP, dpi), WIDTH_DIP * dpi as i32 / 96);
            assert!(scale(ROW_DIP, dpi) >= ROW_DIP);
        }
    }

    #[test]
    fn menu_pages_reflect_selected_state() {
        let snapshot = MenuSnapshot {
            strings: LanguageId::English.strings(),
            interval: 300_000,
            theme: ThemeMode::Dark,
            language_override: Some(LanguageId::Russian),
            widget_visible: true,
            claude_visible: false,
            codex_visible: true,
            antigravity_visible: false,
            startup_enabled: false,
            version_label: String::new(),
            update_disabled: false,
            dark: true,
        };
        assert!(rows_for(Page::Models, &snapshot)[1].checked);
        assert!(rows_for(Page::Frequency, &snapshot)[2].checked);
        assert!(rows_for(Page::Theme, &snapshot)[3].checked);
        assert!(rows_for(Page::Language, &snapshot)[11].checked);
    }
}
