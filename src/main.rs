#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use music_link_finder::relay::{HostingEvent, RelayHost};
use music_link_finder::search::{SearchMode, VideoResult, search_youtube};
use std::fs;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
mod thumbnails;
use thumbnails::Thumbnails;
use windows_sys::Win32::Foundation::{GlobalFree, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::{
    FillModeAlternate, GdipAddPathArc, GdipClosePathFigure, GdipCreateFromHDC, GdipCreatePath,
    GdipCreatePen1, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath,
    GdipDeletePen, GdipDrawPath, GdipFillPath, GdipSetInterpolationMode, GdipSetSmoothingMode,
    GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GpGraphics, GpPath,
    InterpolationModeHighQualityBicubic, SmoothingModeAntiAlias, UnitPixel,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Controls::{
    EM_GETSEL, EM_REPLACESEL, EM_SETCUEBANNER, EM_SETSEL, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::Ime::{
    GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK,
    VK_CONTROL, VK_DOWN, VK_RETURN, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static FONT_READY: AtomicBool = AtomicBool::new(false);

struct BundledFonts(Vec<windows_sys::Win32::Foundation::HANDLE>);
impl BundledFonts {
    fn load() -> Self {
        let mut handles = Vec::new();
        for bytes in [
            include_bytes!("../assets/fonts/PretendardJP-Regular.otf").as_slice(),
            include_bytes!("../assets/fonts/PretendardJP-SemiBold.otf").as_slice(),
        ] {
            let mut count = 0;
            let handle = unsafe {
                AddFontMemResourceEx(
                    bytes.as_ptr().cast(),
                    bytes.len() as u32,
                    null(),
                    &raw mut count,
                )
            };
            if !handle.is_null() {
                handles.push(handle);
            }
        }
        FONT_READY.store(handles.len() == 2, Ordering::Relaxed);
        Self(handles)
    }
}
impl Drop for BundledFonts {
    fn drop(&mut self) {
        for handle in self.0.drain(..) {
            unsafe {
                RemoveFontMemResourceEx(handle);
            }
        }
    }
}

fn client_animations_enabled() -> bool {
    let mut enabled = 1i32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        );
    }
    enabled != 0
}

unsafe fn start_feedback(hwnd: HWND, state: &mut AppState) {
    if state.reduce_motion {
        for (index, value) in state.hover_progress.iter_mut().enumerate() {
            *value = f32::from(zone_index(state.hover, state.scroll) == Some(index));
        }
        for (slot, value) in state.selection_progress.iter_mut().enumerate() {
            *value = f32::from(state.selected == state.scroll + slot);
        }
        unsafe {
            KillTimer(hwnd, TIMER_ANIM);
            InvalidateRect(hwnd, null(), 0);
        }
        state.animating = false;
    } else if !state.animating {
        unsafe {
            SetTimer(hwnd, TIMER_ANIM, 16, None);
        }
        state.animating = true;
    }
}

const APP_NAME: &str = "UTALINK";
const CLASS_NAME: &str = "UtaLinkNativeWindow";

fn config_file_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("window-state.txt")
}

fn load_window_size() -> (i32, i32) {
    let path = config_file_path();
    let dpi = unsafe { GetDpiForSystem() }.max(96) as i32;
    let mut width = scale(1280, dpi);
    let mut height = scale(900, dpi);
    if let Ok(content) =
        fs::read_to_string(&path).or_else(|_| fs::read_to_string(path.with_file_name("config.txt")))
    {
        // Apply the roomier layout once, then remember the user's later resizing.
        if !content
            .lines()
            .any(|line| line.trim() == "LAYOUT_VERSION=4")
        {
            return (width, height);
        }
        for line in content.lines() {
            let line = line.trim();
            if let Some((k, v)) = line.split_once('=') {
                match k.trim().to_ascii_uppercase().as_str() {
                    "WINDOW_WIDTH" => {
                        if let Ok(w) = v.trim().parse::<i32>() {
                            width = w.clamp(720, 3840);
                        }
                    }
                    "WINDOW_HEIGHT" => {
                        if let Ok(h) = v.trim().parse::<i32>() {
                            height = h.clamp(620, 2160);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (width, height)
}

fn save_window_size(width: i32, height: i32) {
    let path = config_file_path();
    let mut lines = Vec::new();
    let mut found_w = false;
    let mut found_h = false;
    if let Ok(content) = fs::read_to_string(&path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some((k, _)) = trimmed.split_once('=') {
                match k.trim().to_ascii_uppercase().as_str() {
                    "WINDOW_WIDTH" => {
                        lines.push(format!("WINDOW_WIDTH={width}"));
                        found_w = true;
                        continue;
                    }
                    "WINDOW_HEIGHT" => {
                        lines.push(format!("WINDOW_HEIGHT={height}"));
                        found_h = true;
                        continue;
                    }
                    _ => {}
                }
            }
            lines.push(line.to_string());
        }
    }
    if !found_w {
        lines.push(format!("WINDOW_WIDTH={width}"));
    }
    if !found_h {
        lines.push(format!("WINDOW_HEIGHT={height}"));
    }
    if !lines.iter().any(|line| line.trim() == "LAYOUT_VERSION=4") {
        lines.push("LAYOUT_VERSION=4".into());
    }
    let _ = fs::write(&path, lines.join("\r\n") + "\r\n");
}

const WM_SEARCH_DONE: u32 = WM_APP + 1;
const WM_TRIGGER_SEARCH: u32 = WM_APP + 2;
const WM_HOSTING_EVENT: u32 = WM_APP + 3;
const WM_THUMBNAIL: u32 = WM_APP + 4;
#[cfg(debug_assertions)]
const WM_DEBUG_COPY_QUERY: u32 = WM_APP + 99;
const ID_QUERY: i32 = 101;
const ID_AUTO_COPY: usize = 102;
const TIMER_ANIM: usize = 1;

const fn rgb(red: u32, green: u32, blue: u32) -> u32 {
    red | (green << 8) | (blue << 16)
}

const BG: u32 = rgb(255, 255, 255);
const SURFACE: u32 = rgb(255, 255, 255);
const SURFACE_HOVER: u32 = rgb(244, 247, 252);
const SURFACE_SELECTED: u32 = rgb(235, 242, 255);
const BORDER: u32 = rgb(223, 230, 240);
const SEARCH_FOCUS: u32 = rgb(52, 91, 221);
const TEXT: u32 = rgb(26, 40, 61);
const TEXT_MUTED: u32 = rgb(75, 91, 113);
const ACCENT: u32 = rgb(49, 92, 229);
const SUCCESS: u32 = rgb(51, 108, 77);
const ERROR: u32 = rgb(177, 58, 40);

const fn to_argb(rgb: u32, alpha: u8) -> u32 {
    let r = rgb & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = (rgb >> 16) & 0xFF;
    ((alpha as u32) << 24) | (r << 16) | (g << 8) | b
}

fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let r1 = (a & 0xFF) as f32;
    let g1 = ((a >> 8) & 0xFF) as f32;
    let b1 = ((a >> 16) & 0xFF) as f32;

    let r2 = (b & 0xFF) as f32;
    let g2 = ((b >> 8) & 0xFF) as f32;
    let b2 = ((b >> 16) & 0xFF) as f32;

    let r = (r1 + (r2 - r1) * t).round() as u32;
    let g = (g1 + (g2 - g1) * t).round() as u32;
    let b = (b1 + (b2 - b1) * t).round() as u32;

    r | (g << 8) | (b << 16)
}

const MAX_VISIBLE_RESULTS: usize = 8;
const NUM_ZONES: usize = 9 + MAX_VISIBLE_RESULTS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HitZone {
    None,
    Query,
    Search,
    Pronunciation,
    Karaoke,
    HostSelected,
    StopHosting,
    Copy,
    Open,
    CopyHosted,
    ToggleRelay,
    Result(usize),
}

fn zone_index(zone: HitZone, scroll: usize) -> Option<usize> {
    match zone {
        HitZone::Search => Some(0),
        HitZone::Pronunciation => Some(1),
        HitZone::Karaoke => Some(2),
        HitZone::HostSelected => Some(3),
        HitZone::StopHosting => Some(4),
        HitZone::Copy => Some(5),
        HitZone::Open => Some(6),
        HitZone::CopyHosted => Some(7),
        HitZone::ToggleRelay => Some(NUM_ZONES - 1),
        HitZone::Result(i) if i >= scroll && i - scroll < MAX_VISIBLE_RESULTS => {
            Some(8 + i - scroll)
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum StatusTone {
    Normal,
    Success,
    Error,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ButtonStyle {
    Primary,
    Secondary,
    SegmentTab(bool),
    DangerSubtle,
}

const APP_ICON_RESOURCE: u16 = 101;

struct WindowIcons {
    small: HICON,
    large: HICON,
}
impl WindowIcons {
    unsafe fn load(dpi: u32) -> Option<Self> {
        unsafe {
            let instance = GetModuleHandleW(null());
            let name = APP_ICON_RESOURCE as usize as *const u16;
            let icons = Self {
                small: LoadImageW(
                    instance,
                    name,
                    IMAGE_ICON,
                    GetSystemMetricsForDpi(SM_CXSMICON, dpi),
                    GetSystemMetricsForDpi(SM_CYSMICON, dpi),
                    LR_DEFAULTCOLOR,
                )
                .cast(),
                large: LoadImageW(
                    instance,
                    name,
                    IMAGE_ICON,
                    GetSystemMetricsForDpi(SM_CXICON, dpi),
                    GetSystemMetricsForDpi(SM_CYICON, dpi),
                    LR_DEFAULTCOLOR,
                )
                .cast(),
            };
            if icons.small.is_null() || icons.large.is_null() {
                None
            } else {
                Some(icons)
            }
        }
    }
}
impl Drop for WindowIcons {
    fn drop(&mut self) {
        unsafe {
            if !self.small.is_null() {
                DestroyIcon(self.small);
            }
            if !self.large.is_null() {
                DestroyIcon(self.large);
            }
        }
    }
}

unsafe fn update_window_icons(hwnd: HWND, state: &mut AppState) {
    if let Some(icons) = unsafe { WindowIcons::load(GetDpiForWindow(hwnd).max(96)) } {
        unsafe {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icons.small as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icons.large as isize);
        }
        // Drop the previous owned icons only after the window points to the new pair.
        state.icons = Some(icons);
    }
}

struct AppState {
    icons: Option<WindowIcons>,
    query: HWND,
    copy_toggle: HWND,
    hosted_field: HWND,
    auto_copy: bool,
    relay_expanded: bool,
    thumbnails: Thumbnails,
    relay: Option<RelayHost>,
    hosting_generation: u64,
    hosting: bool,
    hosting_source: Option<String>,
    hosted: Option<(String, String)>,
    font_body: HFONT,
    font_button: HFONT,
    font_card_title: HFONT,
    font_small: HFONT,
    font_badge: HFONT,
    font_title: HFONT,
    font_credit: HFONT,
    edit_brush: HBRUSH,
    results: Vec<VideoResult>,
    selected: usize,
    scroll: usize,
    wheel_remainder: i32,
    mode: SearchMode,
    loading: bool,
    search_generation: u64,
    empty_title: &'static str,
    empty_detail: &'static str,
    hover: HitZone,
    pressed: HitZone,
    hover_progress: [f32; NUM_ZONES],
    animating: bool,
    reduce_motion: bool,
    selection_progress: [f32; MAX_VISIBLE_RESULTS],
    tracking_mouse: bool,
    status: String,
    status_tone: StatusTone,
}

impl AppState {
    fn new() -> Self {
        Self {
            icons: None,
            query: null_mut(),
            copy_toggle: null_mut(),
            hosted_field: null_mut(),
            auto_copy: false,
            relay_expanded: false,
            thumbnails: Thumbnails::default(),
            relay: None,
            hosting_generation: 0,
            hosting: false,
            hosting_source: None,
            hosted: None,
            font_body: null_mut(),
            font_button: null_mut(),
            font_card_title: null_mut(),
            font_small: null_mut(),
            font_badge: null_mut(),
            font_title: null_mut(),
            font_credit: null_mut(),
            edit_brush: null_mut(),
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            wheel_remainder: 0,
            mode: SearchMode::Pronunciation,
            loading: false,
            search_generation: 0,
            empty_title: "",
            empty_detail: "",
            hover: HitZone::None,
            pressed: HitZone::None,
            hover_progress: [0.0; NUM_ZONES],
            animating: false,
            reduce_motion: !client_animations_enabled(),
            selection_progress: [0.0; MAX_VISIBLE_RESULTS],
            tracking_mouse: false,
            status: String::new(),
            status_tone: StatusTone::Normal,
        }
    }
}

struct UiLayout {
    sidebar: Option<RECT>,
    selection_card: Option<RECT>,
    detail_title: Option<RECT>,
    detail_channel: Option<RECT>,
    query_shell: RECT,
    search: RECT,
    pronunciation: RECT,
    karaoke: RECT,
    copy_toggle: RECT,
    results_label: RECT,
    results: Vec<RECT>,
    empty_results: RECT,
    host_button: RECT,
    copy: RECT,
    open: RECT,
    relay_card: RECT,
    relay_header: RECT,
    stop_hosting: RECT,
    hosted_shell: RECT,
    hosted_field: RECT,
    copy_hosted: RECT,
    status: RECT,
}

enum SearchOutcome {
    Success(Vec<VideoResult>),
    Error(String),
}

struct SearchMessage {
    generation: u64,
    outcome: SearchOutcome,
}

struct HostingMessage {
    generation: u64,
    event: HostingEvent,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn scale(value: i32, dpi: i32) -> i32 {
    value * dpi / 96
}

fn point_in(rect: &RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn mouse_point(lparam: LPARAM) -> (i32, i32) {
    ((lparam as i16) as i32, ((lparam >> 16) as i16) as i32)
}

unsafe fn state_ptr(hwnd: HWND) -> *mut AppState {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState }
}

unsafe fn get_text(hwnd: HWND) -> String {
    let length = unsafe { SendMessageW(hwnd, WM_GETTEXTLENGTH, 0, 0) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    let read = unsafe { SendMessageW(hwnd, WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize) }
        .max(0) as usize;
    String::from_utf16_lossy(&buffer[..read.min(length)])
}

fn previous_word_start(text: &[u16], cursor: usize) -> usize {
    let mut position = cursor.min(text.len());
    while position > 0 {
        let character = char::from_u32(text[position - 1] as u32).unwrap_or('\0');
        if !character.is_whitespace() {
            break;
        }
        position -= 1;
    }
    while position > 0 {
        let character = char::from_u32(text[position - 1] as u32).unwrap_or('\0');
        if character.is_whitespace() {
            break;
        }
        position -= 1;
    }
    position
}

unsafe fn delete_previous_word(edit: HWND) {
    let mut selection_start = 0u32;
    let mut selection_end = 0u32;
    unsafe {
        SendMessageW(
            edit,
            EM_GETSEL,
            &mut selection_start as *mut u32 as usize,
            &mut selection_end as *mut u32 as isize,
        )
    };
    if selection_start == selection_end && selection_start > 0 {
        let length = unsafe { SendMessageW(edit, WM_GETTEXTLENGTH, 0, 0) }.max(0) as usize;
        let mut buffer = vec![0u16; length + 1];
        unsafe { SendMessageW(edit, WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize) };
        selection_start = previous_word_start(&buffer[..length], selection_start as usize) as u32;
    }
    if selection_start != selection_end {
        let empty = [0u16];
        unsafe {
            SendMessageW(
                edit,
                EM_SETSEL,
                selection_start as usize,
                selection_end as isize,
            );
            SendMessageW(edit, EM_REPLACESEL, 1, empty.as_ptr() as isize);
        }
    }
}

unsafe extern "system" fn edit_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if message == WM_CHAR && wparam == 0x01 {
        unsafe { SendMessageW(hwnd, EM_SETSEL, 0, -1) };
        return 0;
    }
    if message == WM_CHAR && wparam == 0x7f {
        unsafe { delete_previous_word(hwnd) };
        return 0;
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if matches!(message, WM_SETFOCUS | WM_KILLFOCUS) {
        unsafe {
            InvalidateRect(GetParent(hwnd), null(), 0);
        }
    }
    result
}

// Custom modern toggle switch for the auto-copy preference with anti-aliasing
unsafe extern "system" fn toggle_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if message == WM_PAINT {
        let mut paint: PAINTSTRUCT = unsafe { zeroed() };
        let dc = unsafe { BeginPaint(hwnd, &mut paint) };
        let mut rect: RECT = unsafe { zeroed() };
        unsafe {
            GetClientRect(hwnd, &mut rect);
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
        let checked = unsafe { SendMessageW(hwnd, BM_GETCHECK, 0, 0) } != 0;
        let brush = unsafe { CreateSolidBrush(BG) };
        unsafe {
            FillRect(dc, &rect, brush);
            DeleteObject(brush as _);
        }
        let track_w = scale(34, dpi);
        let track_h = scale(18, dpi);
        let track_top = (rect.bottom - track_h) / 2;
        let track = RECT {
            left: 0,
            top: track_top,
            right: track_w,
            bottom: track_top + track_h,
        };
        let knob_size = scale(14, dpi);
        let knob_left = if checked {
            track_w - knob_size - scale(2, dpi)
        } else {
            scale(2, dpi)
        };
        let knob = RECT {
            left: knob_left,
            top: track_top + scale(2, dpi),
            right: knob_left + knob_size,
            bottom: track_top + scale(2, dpi) + knob_size,
        };
        let label = RECT {
            left: track_w + scale(10, dpi),
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        let font = unsafe { SendMessageW(hwnd, WM_GETFONT, 0, 0) } as HFONT;
        unsafe {
            let mut graphics = null_mut();
            GdipCreateFromHDC(dc, &mut graphics);
            GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
            rounded_box(
                graphics,
                track,
                if checked { TEXT } else { BORDER },
                if checked { TEXT } else { BORDER },
                track_h / 2,
            );
            rounded_box(graphics, knob, SURFACE, SURFACE, knob_size / 2);
            GdipDeleteGraphics(graphics);

            draw_text(
                dc,
                &get_text(hwnd),
                label,
                TEXT,
                font,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            if GetFocus() == hwnd {
                DrawFocusRect(dc, &rect);
            }
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if matches!(message, WM_SETFOCUS | WM_KILLFOCUS | BM_SETCHECK) {
        unsafe {
            InvalidateRect(hwnd, null(), 0);
        }
    }
    result
}

unsafe fn set_status(hwnd: HWND, state: &mut AppState, text: impl Into<String>, tone: StatusTone) {
    state.status = text.into();
    state.status_tone = tone;
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

unsafe fn create_font(size: i32, weight: i32, dpi: i32) -> HFONT {
    unsafe { create_font_style(size, weight, dpi, false) }
}

unsafe fn create_font_style(size: i32, weight: i32, dpi: i32, italic: bool) -> HFONT {
    let face = wide(if italic {
        "Georgia"
    } else if FONT_READY.load(Ordering::Relaxed) {
        if weight >= FW_SEMIBOLD as i32 {
            "Pretendard JP SemiBold"
        } else {
            "Pretendard JP"
        }
    } else {
        "Malgun Gothic"
    });
    unsafe {
        CreateFontW(
            -(size * dpi / 96),
            0,
            0,
            0,
            weight,
            u32::from(italic),
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        )
    }
}

unsafe fn delete_font(font: HFONT) {
    if !font.is_null() {
        unsafe { DeleteObject(font as _) };
    }
}

unsafe fn replace_fonts(hwnd: HWND, state: &mut AppState) {
    unsafe {
        delete_font(state.font_body);
        delete_font(state.font_button);
        delete_font(state.font_card_title);
        delete_font(state.font_small);
        delete_font(state.font_badge);
        delete_font(state.font_title);
        delete_font(state.font_credit);
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    state.font_title = unsafe { create_font(25, FW_SEMIBOLD as i32, dpi) };
    state.font_card_title = unsafe { create_font(18, FW_SEMIBOLD as i32, dpi) };
    state.font_body = unsafe { create_font(16, FW_NORMAL as i32, dpi) };
    state.font_button = unsafe { create_font(15, FW_SEMIBOLD as i32, dpi) };
    state.font_small = unsafe { create_font(15, FW_NORMAL as i32, dpi) };
    state.font_badge = unsafe { create_font(13, FW_SEMIBOLD as i32, dpi) };
    state.font_credit = unsafe { create_font_style(14, FW_NORMAL as i32, dpi, true) };

    if !state.query.is_null() {
        unsafe { SendMessageW(state.query, WM_SETFONT, state.font_body as usize, 1) };
    }
    if !state.copy_toggle.is_null() {
        unsafe { SendMessageW(state.copy_toggle, WM_SETFONT, state.font_small as usize, 1) };
    }
    if !state.hosted_field.is_null() {
        unsafe { SendMessageW(state.hosted_field, WM_SETFONT, state.font_small as usize, 1) };
    }
}

fn make_layout(hwnd: HWND) -> UiLayout {
    let mut client: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    let state = unsafe { state_ptr(hwnd) };
    let expanded = !state.is_null() && unsafe { (*state).relay_expanded };
    layout_for_state(client.right, client.bottom, dpi, expanded)
}

#[cfg(test)]
fn layout_for_size(width: i32, height: i32, dpi: i32) -> UiLayout {
    layout_for_state(width, height, dpi, false)
}

fn layout_for_state(width: i32, height: i32, dpi: i32, expanded: bool) -> UiLayout {
    let s = |v| scale(v, dpi);
    let r = |left, top, right, bottom| RECT {
        left,
        top,
        right,
        bottom,
    };
    let m = s(24).max((width - s(1440)) / 2);
    let right = width - m;
    let bottom = height - s(44);
    let is_wide = width >= s(1040) && height >= s(700);
    let sidebar = is_wide.then(|| {
        r(
            right - s(if width >= s(1440) { 384 } else { 344 }),
            s(184),
            right,
            bottom,
        )
    });
    let list_right = sidebar.map_or(right, |side| side.left - s(24));
    let results_top = s(if is_wide { 184 } else { 218 });
    let (
        selection_card,
        detail_title,
        detail_channel,
        copy,
        open,
        relay_card,
        relay_header,
        host_button,
        hosted_shell,
        hosted_field,
        copy_hosted,
        stop_hosting,
        results_bottom,
    ) = if let Some(side) = sidebar {
        let left = side.left + s(20);
        let edge = side.right - s(20);
        let available = bottom - side.top;
        let image_h = ((edge - left) * 9 / 16)
            .min((available - s(if expanded { 440 } else { 284 })).max(s(50)));
        let image_w = image_h * 16 / 9;
        let image_left = left + (edge - left - image_w) / 2;
        let image = r(
            image_left,
            side.top + s(20),
            image_left + image_w,
            side.top + s(20) + image_h,
        );
        let title = r(left, image.bottom + s(16), edge, image.bottom + s(88));
        let channel = r(left, title.bottom + s(8), edge, title.bottom + s(28));
        let actions = channel.bottom + s(16);
        let copy = r(left, actions, edge - s(144), actions + s(42));
        let open = r(edge - s(136), actions, edge, actions + s(42));
        let relay_top = copy.bottom + s(24);
        let header = r(left, relay_top, edge, relay_top + s(40));
        let host = r(left, header.bottom + s(10), edge, header.bottom + s(46));
        let shell = r(left, host.bottom + s(8), edge, host.bottom + s(40));
        let field = r(
            shell.left + s(10),
            shell.top + s(6),
            shell.right - s(10),
            shell.bottom - s(6),
        );
        let copy_hosted = r(
            left,
            shell.bottom + s(8),
            edge - s(76),
            shell.bottom + s(42),
        );
        let stop = r(edge - s(68), copy_hosted.top, edge, copy_hosted.bottom);
        let relay_bottom = if expanded {
            copy_hosted.bottom + s(12)
        } else {
            header.bottom
        };
        (
            Some(image),
            Some(title),
            Some(channel),
            copy,
            open,
            r(left, relay_top, edge, relay_bottom),
            header,
            host,
            shell,
            field,
            copy_hosted,
            stop,
            bottom,
        )
    } else {
        let relay_top = bottom - s(if expanded { 124 } else { 36 });
        let actions = relay_top - s(56);
        let copy = r(m, actions, m + (right - m) * 2 / 3 - s(5), actions + s(42));
        let open = r(copy.right + s(10), actions, right, actions + s(42));
        let header = r(m, relay_top, right, relay_top + s(36));
        let host = r(m, header.bottom + s(8), m + s(144), header.bottom + s(44));
        let shell = r(host.right + s(10), host.top, right, host.bottom);
        let field = r(
            shell.left + s(10),
            shell.top + s(8),
            shell.right - s(10),
            shell.bottom - s(8),
        );
        let copy_hosted = r(shell.left, host.bottom + s(8), right, host.bottom + s(40));
        let stop = r(m, copy_hosted.top, host.right, copy_hosted.bottom);
        (
            None,
            None,
            None,
            copy,
            open,
            r(m, relay_top, right, bottom),
            header,
            host,
            shell,
            field,
            copy_hosted,
            stop,
            actions - s(12),
        )
    };
    let available = (results_bottom - results_top).max(s(56));
    let count = (available / s(96)).clamp(1, MAX_VISIBLE_RESULTS as i32);
    let row_h = (available / count).min(s(106));
    UiLayout {
        sidebar,
        selection_card,
        detail_title,
        detail_channel,
        query_shell: r(
            if is_wide { m + s(156) } else { m },
            if is_wide { s(24) } else { s(70) },
            right - s(100),
            if is_wide { s(78) } else { s(120) },
        ),
        search: r(
            right - s(90),
            if is_wide { s(24) } else { s(70) },
            right,
            if is_wide { s(78) } else { s(120) },
        ),
        pronunciation: r(
            m,
            if is_wide { s(104) } else { s(134) },
            m + s(126),
            if is_wide { s(144) } else { s(174) },
        ),
        karaoke: r(
            m + s(136),
            if is_wide { s(104) } else { s(134) },
            m + s(272),
            if is_wide { s(144) } else { s(174) },
        ),
        copy_toggle: r(
            right - s(208),
            if is_wide { s(110) } else { s(140) },
            right,
            if is_wide { s(136) } else { s(166) },
        ),
        results_label: r(m, results_top - s(24), list_right, results_top - s(2)),
        results: (0..count)
            .map(|i| {
                r(
                    m,
                    results_top + i * row_h,
                    list_right,
                    results_top + (i + 1) * row_h,
                )
            })
            .collect(),
        empty_results: r(m, results_top, list_right, results_bottom),
        host_button,
        copy,
        open,
        relay_card,
        relay_header,
        stop_hosting,
        hosted_shell,
        hosted_field,
        copy_hosted,
        status: r(m, height - s(30), right, height - s(8)),
    }
}

unsafe fn layout_edit(hwnd: HWND, state: &AppState) {
    let layout = make_layout(hwnd);
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    let px = scale(16, dpi);
    // A single-line EDIT paints at the top of its client area. Center a
    // font-sized control inside the search shell instead of using fixed padding.
    let mut line_height = scale(20, dpi);
    unsafe {
        let dc = GetDC(state.query);
        if !dc.is_null() {
            let previous = SelectObject(dc, state.font_body.cast());
            let mut metrics: TEXTMETRICW = zeroed();
            if GetTextMetricsW(dc, &mut metrics) != 0 {
                line_height = metrics.tmHeight;
            }
            SelectObject(dc, previous);
            ReleaseDC(state.query, dc);
        }
    }
    let shell_height = layout.query_shell.bottom - layout.query_shell.top;
    let edit_height = (line_height + scale(2, dpi)).min(shell_height);
    unsafe {
        MoveWindow(
            state.query,
            layout.query_shell.left + px,
            layout.query_shell.top + (shell_height - edit_height) / 2,
            layout.query_shell.right - layout.query_shell.left - px * 2,
            edit_height,
            1,
        );
        MoveWindow(
            state.copy_toggle,
            layout.copy_toggle.left,
            layout.copy_toggle.top,
            layout.copy_toggle.right - layout.copy_toggle.left,
            layout.copy_toggle.bottom - layout.copy_toggle.top,
            1,
        );
        ShowWindow(
            state.hosted_field,
            if state.relay_expanded {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        MoveWindow(
            state.hosted_field,
            layout.hosted_field.left,
            layout.hosted_field.top,
            layout.hosted_field.right - layout.hosted_field.left,
            layout.hosted_field.bottom - layout.hosted_field.top,
            1,
        );
    }
}

unsafe fn create_ui(hwnd: HWND, state: &mut AppState) {
    let class_name = wide("EDIT");
    let empty = wide("");
    state.query = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            empty.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            0,
            0,
            10,
            10,
            hwnd,
            ID_QUERY as isize as HMENU,
            GetModuleHandleW(null()),
            null(),
        )
    };
    state.edit_brush = unsafe { CreateSolidBrush(SURFACE) };

    state.copy_toggle = unsafe {
        CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("첫 번째 링크 자동 복사").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            0,
            10,
            10,
            hwnd,
            ID_AUTO_COPY as HMENU,
            GetModuleHandleW(null()),
            null(),
        )
    };
    unsafe {
        SetWindowSubclass(state.copy_toggle, Some(toggle_subclass_proc), 1, 0);
    }

    state.hosted_field = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            empty.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_READONLY as u32,
            0,
            0,
            10,
            10,
            hwnd,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        )
    };

    let hwnd_value = hwnd as isize;
    state.relay = Some(RelayHost::new(move |generation, event| {
        let raw = Box::into_raw(Box::new(HostingMessage { generation, event }));
        if unsafe { PostMessageW(hwnd_value as HWND, WM_HOSTING_EVENT, 0, raw as isize) } == 0 {
            unsafe {
                drop(Box::from_raw(raw));
            }
        }
    }));

    unsafe {
        replace_fonts(hwnd, state);
        SetWindowSubclass(state.query, Some(edit_subclass_proc), 1, 0);
        SendMessageW(
            state.query,
            EM_SETCUEBANNER,
            1,
            wide("곡 제목 또는 별칭").as_ptr() as isize,
        );
        SendMessageW(
            state.hosted_field,
            EM_SETCUEBANNER,
            1,
            wide("중계 주소").as_ptr() as isize,
        );
        layout_edit(hwnd, state);
        SetFocus(state.query);
    }
}

unsafe fn select_font(hdc: HDC, font: HFONT) -> HGDIOBJ {
    unsafe { SelectObject(hdc, font as _) }
}

unsafe fn draw_text(hdc: HDC, text: &str, rect: RECT, color: u32, font: HFONT, format: u32) {
    if text.is_empty() {
        return;
    }
    let clean = text.replace(['\u{30fb}', '\u{00b7}'], " ");
    let value: Vec<u16> = clean.encode_utf16().collect();
    let mut target = rect;
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);
        let previous = select_font(hdc, font);
        DrawTextW(hdc, value.as_ptr(), value.len() as i32, &mut target, format);
        SelectObject(hdc, previous);
    }
}

unsafe fn create_rounded_path(rect: RECT, radius: i32) -> *mut GpPath {
    let mut path = null_mut();
    unsafe {
        GdipCreatePath(FillModeAlternate, &mut path);
        let r = (radius.max(1) as f32)
            .min(((rect.right - rect.left).max(2) as f32) / 2.0)
            .min(((rect.bottom - rect.top).max(2) as f32) / 2.0);
        let d = r * 2.0;
        let x = rect.left as f32;
        let y = rect.top as f32;
        let w = ((rect.right - rect.left) as f32 - 1.0).max(d);
        let h = ((rect.bottom - rect.top) as f32 - 1.0).max(d);

        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(path);
    }
    path
}

unsafe fn outlined_box(
    graphics: *mut GpGraphics,
    rect: RECT,
    fill: u32,
    outline: u32,
    radius: i32,
    border_width: i32,
) {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    unsafe {
        let path = create_rounded_path(rect, radius);
        let mut brush = null_mut();
        GdipCreateSolidFill(to_argb(fill, 255), &mut brush);
        GdipFillPath(graphics, brush as _, path);
        GdipDeleteBrush(brush as _);

        if border_width > 0 {
            let mut pen = null_mut();
            GdipCreatePen1(
                to_argb(outline, 255),
                border_width as f32,
                UnitPixel,
                &mut pen,
            );
            GdipDrawPath(graphics, pen as _, path);
            GdipDeletePen(pen as _);
        }

        GdipDeletePath(path);
    }
}

unsafe fn rounded_box(graphics: *mut GpGraphics, rect: RECT, fill: u32, outline: u32, radius: i32) {
    unsafe {
        outlined_box(graphics, rect, fill, outline, radius, 1);
    }
}

unsafe fn fill_rect(dc: HDC, rect: RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &rect, brush);
        DeleteObject(brush as _);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_custom_button(
    graphics: *mut GpGraphics,
    hdc: HDC,
    state: &AppState,
    rect: RECT,
    zone: HitZone,
    label: &str,
    style: ButtonStyle,
    enabled: bool,
    dpi: i32,
) {
    let s = |v| scale(v, dpi);
    let hover = zone_index(zone, state.scroll).map_or(0.0, |i| state.hover_progress[i]);
    let pressed = state.pressed == zone && enabled;
    unsafe {
        if let ButtonStyle::SegmentTab(active) = style {
            if hover > 0.01 && enabled {
                outlined_box(
                    graphics,
                    rect,
                    lerp_color(BG, SURFACE_HOVER, hover),
                    BG,
                    s(4),
                    0,
                );
            }
            draw_text(
                hdc,
                label,
                rect,
                if active { ACCENT } else { TEXT_MUTED },
                state.font_button,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            if active {
                fill_rect(
                    hdc,
                    RECT {
                        top: rect.bottom - s(3),
                        bottom: rect.bottom,
                        ..rect
                    },
                    ACCENT,
                );
            }
            return;
        }
        let (base, highlight, outline, color) = if !enabled {
            (
                rgb(237, 241, 246),
                rgb(237, 241, 246),
                rgb(237, 241, 246),
                rgb(147, 157, 172),
            )
        } else {
            match style {
                ButtonStyle::Primary => (ACCENT, rgb(34, 74, 190), ACCENT, SURFACE),
                ButtonStyle::Secondary => (SURFACE, SURFACE_HOVER, BORDER, TEXT),
                ButtonStyle::DangerSubtle => (BG, rgb(245, 228, 221), BG, ERROR),
                ButtonStyle::SegmentTab(_) => unreachable!(),
            }
        };
        let fill = if pressed {
            highlight
        } else {
            lerp_color(base, highlight, hover)
        };
        outlined_box(
            graphics,
            rect,
            fill,
            outline,
            s(6),
            i32::from(enabled && style == ButtonStyle::Secondary),
        );
        draw_text(
            hdc,
            label,
            rect,
            color,
            state.font_button,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
}

unsafe fn paint_ui(hwnd: HWND, state: &AppState) {
    unsafe {
        let mut paint: PAINTSTRUCT = zeroed();
        let window_dc = BeginPaint(hwnd, &mut paint);
        let mut client: RECT = zeroed();
        GetClientRect(hwnd, &mut client);
        let width = client.right.max(1);
        let height = client.bottom.max(1);
        let dc = CreateCompatibleDC(window_dc);
        let bitmap = CreateCompatibleBitmap(window_dc, width, height);
        let old_bitmap = SelectObject(dc, bitmap as _);
        fill_rect(dc, client, BG);
        let mut graphics = null_mut();
        GdipCreateFromHDC(dc, &mut graphics);
        GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        GdipSetInterpolationMode(graphics, InterpolationModeHighQualityBicubic);
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let s = |v| scale(v, dpi);
        let r = |left, top, right, bottom| RECT {
            left,
            top,
            right,
            bottom,
        };
        let layout = make_layout(hwnd);
        let m = layout.empty_results.left;
        let single = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX;
        let centered = DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX;
        let multiline = DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX;
        draw_text(
            dc,
            "UtaLink",
            r(m, s(18), m + s(148), s(51)),
            TEXT,
            state.font_title,
            single,
        );
        draw_text(
            dc,
            "음악 링크 검색",
            r(m, s(51), m + s(148), s(69)),
            TEXT_MUTED,
            state.font_small,
            single,
        );
        outlined_box(
            graphics,
            layout.query_shell,
            SURFACE,
            if GetFocus() == state.query {
                SEARCH_FOCUS
            } else {
                BORDER
            },
            s(6),
            1,
        );
        draw_custom_button(
            graphics,
            dc,
            state,
            layout.search,
            HitZone::Search,
            if state.loading {
                "검색 중"
            } else {
                "검색"
            },
            ButtonStyle::Primary,
            !state.loading,
            dpi,
        );
        draw_custom_button(
            graphics,
            dc,
            state,
            layout.pronunciation,
            HitZone::Pronunciation,
            "한글 발음",
            ButtonStyle::SegmentTab(state.mode == SearchMode::Pronunciation),
            true,
            dpi,
        );
        draw_custom_button(
            graphics,
            dc,
            state,
            layout.karaoke,
            HitZone::Karaoke,
            "노래방 반주",
            ButtonStyle::SegmentTab(state.mode == SearchMode::Karaoke),
            true,
            dpi,
        );
        let general = state.results.first().is_some_and(|item| item.is_fallback);
        draw_text(
            dc,
            if general {
                "일반 YouTube 결과"
            } else {
                "검색 결과"
            },
            layout.results_label,
            TEXT_MUTED,
            state.font_badge,
            single,
        );
        if !state.results.is_empty() {
            draw_text(
                dc,
                &format!("{}개 영상", state.results.len()),
                layout.results_label,
                TEXT_MUTED,
                state.font_badge,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        let thumbnail = |rect: RECT, url: Option<&str>| {
            fill_rect(dc, rect, rgb(240, 243, 248));
            if let Some(image) = url.and_then(|url| state.thumbnails.get(url)) {
                image.paint(
                    graphics,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                );
            } else {
                draw_text(
                    dc,
                    if url.is_some_and(|url| !state.thumbnails.failed(url)) {
                        "이미지 준비 중"
                    } else {
                        "미리보기 없음"
                    },
                    rect,
                    TEXT_MUTED,
                    state.font_badge,
                    centered,
                );
            }
        };
        if state.results.is_empty() {
            let area = layout.empty_results;
            if state.loading {
                for (i, row) in layout.results.iter().take(4).enumerate() {
                    let top = row.top + s(14);
                    fill_rect(
                        dc,
                        r(row.left + s(10), top, row.left + s(130), top + s(68)),
                        rgb(239, 243, 248),
                    );
                    fill_rect(
                        dc,
                        r(
                            row.left + s(146),
                            top + s(4),
                            row.right - s(28 + i as i32 * 12),
                            top + s(18),
                        ),
                        rgb(239, 243, 248),
                    );
                    fill_rect(
                        dc,
                        r(
                            row.left + s(146),
                            top + s(30),
                            row.right - s(80),
                            top + s(42),
                        ),
                        rgb(246, 248, 251),
                    );
                    fill_rect(
                        dc,
                        r(
                            row.left + s(146),
                            top + s(54),
                            row.left + s(260),
                            top + s(64),
                        ),
                        rgb(246, 248, 251),
                    );
                }
            } else {
                let cy = (area.top + area.bottom) / 2;
                draw_text(
                    dc,
                    state.empty_title,
                    r(area.left, cy - s(24), area.right, cy + s(8)),
                    TEXT,
                    state.font_body,
                    centered,
                );
                draw_text(
                    dc,
                    state.empty_detail,
                    r(area.left, cy + s(14), area.right, cy + s(38)),
                    TEXT_MUTED,
                    state.font_small,
                    centered,
                );
            }
        }
        for (slot, rect) in layout.results.iter().copied().enumerate() {
            let index = state.scroll + slot;
            let Some(item) = state.results.get(index) else {
                break;
            };
            let selected = index == state.selected;
            let base = lerp_color(SURFACE, SURFACE_HOVER, state.hover_progress[8 + slot]);
            let fill = lerp_color(base, SURFACE_SELECTED, state.selection_progress[slot]);
            let row = r(rect.left, rect.top + s(2), rect.right, rect.bottom - s(3));
            outlined_box(
                graphics,
                row,
                fill,
                if selected { rgb(170, 197, 245) } else { fill },
                s(6),
                i32::from(selected),
            );
            let image_h = s(68).min(rect.bottom - rect.top - s(18));
            let image_w = image_h * 16 / 9;
            let image_top = rect.top + (rect.bottom - rect.top - image_h) / 2;
            thumbnail(
                r(
                    rect.left + s(10),
                    image_top,
                    rect.left + s(10) + image_w,
                    image_top + image_h,
                ),
                Some(&item.thumbnail_url),
            );
            let left = rect.left + s(10) + image_w + s(16);
            draw_text(
                dc,
                &item.title,
                r(left, rect.top + s(12), rect.right - s(14), rect.top + s(59)),
                TEXT,
                state.font_card_title,
                multiline,
            );
            let kind = item.kind_label();
            draw_text(
                dc,
                &format!("{}  |  {}", kind, item.channel),
                r(
                    left,
                    rect.bottom - s(30),
                    rect.right - s(14),
                    rect.bottom - s(9),
                ),
                TEXT_MUTED,
                state.font_small,
                single,
            );
        }
        if let Some(side) = layout.sidebar {
            fill_rect(
                dc,
                r(side.left, side.top, side.left + 1, side.bottom),
                BORDER,
            );
        }
        let selected = state.results.get(state.selected);
        if let Some(image) = layout.selection_card {
            thumbnail(image, selected.map(|item| item.thumbnail_url.as_str()));
        }
        if let Some(title) = layout.detail_title {
            draw_text(
                dc,
                selected.map_or("선택한 영상", |item| item.title.as_str()),
                title,
                TEXT,
                state.font_card_title,
                multiline,
            );
        }
        if let Some(channel) = layout.detail_channel {
            draw_text(
                dc,
                selected.map_or("", |item| item.channel.as_str()),
                channel,
                TEXT_MUTED,
                state.font_small,
                single,
            );
        }
        let has_selection = selected.is_some() && !state.loading;
        draw_custom_button(
            graphics,
            dc,
            state,
            layout.copy,
            HitZone::Copy,
            "링크 복사",
            ButtonStyle::Primary,
            has_selection,
            dpi,
        );
        draw_custom_button(
            graphics,
            dc,
            state,
            layout.open,
            HitZone::Open,
            "YouTube 열기 ↗",
            ButtonStyle::Secondary,
            has_selection,
            dpi,
        );
        fill_rect(
            dc,
            r(
                layout.relay_card.left,
                layout.relay_card.top,
                layout.relay_card.right,
                layout.relay_card.top + 1,
            ),
            BORDER,
        );
        draw_custom_button(
            graphics,
            dc,
            state,
            layout.relay_header,
            HitZone::ToggleRelay,
            if state.relay_expanded {
                "VRChat 중계 접기"
            } else if state.hosting {
                "VRChat 중계 준비 중"
            } else if state.hosted.is_some() {
                "VRChat 중계 연결됨"
            } else {
                "VRChat 중계 펼치기"
            },
            ButtonStyle::Secondary,
            true,
            dpi,
        );
        if state.relay_expanded {
            let same_source =
                state.hosting_source.as_deref() == selected.map(|item| item.url.as_str());
            let host_label = if state.hosting && same_source {
                "중계 준비 중"
            } else if state.hosting {
                "이 영상으로 변경"
            } else if state.hosted.is_some() && same_source {
                "다시 중계하기"
            } else {
                "중계 시작"
            };
            draw_custom_button(
                graphics,
                dc,
                state,
                layout.host_button,
                HitZone::HostSelected,
                host_label,
                ButtonStyle::Secondary,
                has_selection && !(state.hosting && same_source),
                dpi,
            );
            outlined_box(graphics, layout.hosted_shell, SURFACE, BORDER, s(4), 1);
            draw_custom_button(
                graphics,
                dc,
                state,
                layout.copy_hosted,
                HitZone::CopyHosted,
                "중계 주소 복사",
                ButtonStyle::Secondary,
                state.hosted.is_some() && !state.hosting,
                dpi,
            );
            draw_custom_button(
                graphics,
                dc,
                state,
                layout.stop_hosting,
                HitZone::StopHosting,
                "중지",
                ButtonStyle::DangerSubtle,
                state.hosting || state.hosting_source.is_some(),
                dpi,
            );
        }
        let color = match state.status_tone {
            StatusTone::Normal => TEXT_MUTED,
            StatusTone::Success => SUCCESS,
            StatusTone::Error => ERROR,
        };
        fill_rect(
            dc,
            r(
                m,
                layout.status.top - s(8),
                width - m,
                layout.status.top - s(8) + 1,
            ),
            BORDER,
        );
        draw_text(
            dc,
            &state.status,
            RECT {
                right: layout.status.right - s(230),
                ..layout.status
            },
            color,
            state.font_small,
            single,
        );
        let position = if state.results.is_empty() {
            String::new()
        } else {
            format!(
                "{}–{} / {}",
                state.scroll + 1,
                (state.scroll + layout.results.len()).min(state.results.len()),
                state.results.len()
            )
        };
        draw_text(
            dc,
            &position,
            RECT {
                right: layout.status.right - s(104),
                ..layout.status
            },
            TEXT_MUTED,
            state.font_small,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        draw_text(
            dc,
            "by hanbi.",
            RECT {
                left: layout.status.right - s(90),
                ..layout.status
            },
            TEXT_MUTED,
            state.font_credit,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        GdipDeleteGraphics(graphics);
        BitBlt(window_dc, 0, 0, width, height, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old_bitmap);
        DeleteObject(bitmap as _);
        DeleteDC(dc);
        EndPaint(hwnd, &paint);
    }
}

fn hit_test(hwnd: HWND, state: &AppState, x: i32, y: i32) -> HitZone {
    let layout = make_layout(hwnd);
    if point_in(&layout.query_shell, x, y) {
        HitZone::Query
    } else if point_in(&layout.search, x, y) {
        HitZone::Search
    } else if point_in(&layout.pronunciation, x, y) {
        HitZone::Pronunciation
    } else if point_in(&layout.karaoke, x, y) {
        HitZone::Karaoke
    } else if point_in(&layout.relay_header, x, y) {
        HitZone::ToggleRelay
    } else if state.relay_expanded && point_in(&layout.host_button, x, y) {
        HitZone::HostSelected
    } else if point_in(&layout.copy, x, y) {
        HitZone::Copy
    } else if point_in(&layout.open, x, y) {
        HitZone::Open
    } else if state.relay_expanded && point_in(&layout.copy_hosted, x, y) {
        HitZone::CopyHosted
    } else if state.relay_expanded && point_in(&layout.stop_hosting, x, y) {
        HitZone::StopHosting
    } else {
        layout
            .results
            .iter()
            .position(|rect| point_in(rect, x, y))
            .map(|slot| HitZone::Result(state.scroll + slot))
            .unwrap_or(HitZone::None)
    }
}

unsafe fn copy_to_clipboard(owner: HWND, text: &str) -> bool {
    let data = wide(text);
    if unsafe { OpenClipboard(owner) } == 0 {
        return false;
    }
    unsafe { EmptyClipboard() };
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, data.len() * size_of::<u16>()) };
    if memory.is_null() {
        unsafe { CloseClipboard() };
        return false;
    }
    let locked = unsafe { GlobalLock(memory) } as *mut u16;
    if locked.is_null() {
        unsafe {
            GlobalFree(memory);
            CloseClipboard();
        }
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), locked, data.len());
        GlobalUnlock(memory);
    }
    let accepted = unsafe { SetClipboardData(CF_UNICODETEXT as u32, memory as _) };
    if accepted.is_null() {
        unsafe { GlobalFree(memory) };
    }
    unsafe { CloseClipboard() };
    !accepted.is_null()
}

unsafe fn copy_selected(hwnd: HWND, state: &mut AppState) {
    let Some(result) = state.results.get(state.selected) else {
        return;
    };
    if unsafe { copy_to_clipboard(hwnd, &result.url) } {
        unsafe {
            set_status(
                hwnd,
                state,
                "선택한 YouTube 링크를 복사했습니다.",
                StatusTone::Success,
            )
        };
    } else {
        unsafe {
            set_status(
                hwnd,
                state,
                "클립보드를 열 수 없습니다. 잠시 후 다시 시도하세요.",
                StatusTone::Error,
            )
        };
    }
}

unsafe fn open_selected(hwnd: HWND, state: &mut AppState) {
    if state.loading {
        return;
    }
    let Some(result) = state.results.get(state.selected) else {
        return;
    };
    let operation = wide("open");
    let url = wide(&result.url);
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    unsafe {
        set_status(
            hwnd,
            state,
            if result as isize > 32 {
                "브라우저에서 YouTube를 여는 중입니다."
            } else {
                "YouTube를 열지 못했습니다. 기본 브라우저 설정을 확인하세요."
            },
            if result as isize > 32 {
                StatusTone::Success
            } else {
                StatusTone::Error
            },
        );
    }
}

unsafe fn start_search(hwnd: HWND, state: &mut AppState) {
    if state.loading {
        return;
    }
    let query = unsafe { get_text(state.query) }.trim().to_owned();
    if query.is_empty() {
        state.empty_title = "";
        state.empty_detail = "";
        unsafe {
            set_status(hwnd, state, "곡명을 입력하세요.", StatusTone::Error);
            SetFocus(state.query);
        }
        return;
    }
    state.search_generation = state.search_generation.wrapping_add(1);
    let generation = state.search_generation;
    state.loading = true;
    state.thumbnails.clear();
    state.selection_progress.fill(0.0);
    state.results.clear();
    state.selected = 0;
    state.scroll = 0;
    unsafe { set_status(hwnd, state, "", StatusTone::Normal) };
    let mode = state.mode;
    let hwnd_value = hwnd as isize;
    thread::spawn(move || {
        let outcome = match search_youtube(&query, mode, 30) {
            Ok(results) => SearchOutcome::Success(results),
            Err(error) => SearchOutcome::Error(error.to_string()),
        };
        let raw = Box::into_raw(Box::new(SearchMessage {
            generation,
            outcome,
        }));
        let posted = unsafe { PostMessageW(hwnd_value as HWND, WM_SEARCH_DONE, 0, raw as isize) };
        if posted == 0 {
            unsafe { drop(Box::from_raw(raw)) };
        }
    });
}

unsafe fn finish_search(hwnd: HWND, state: &mut AppState, message: SearchMessage) {
    if message.generation != state.search_generation {
        return;
    }
    state.loading = false;
    match message.outcome {
        SearchOutcome::Error(error) => unsafe {
            state.empty_title = "검색에 연결할 수 없습니다";
            state.empty_detail = "연결 상태를 확인하고 다시 검색하세요.";
            set_status(
                hwnd,
                state,
                format!("검색 실패: {error}"),
                StatusTone::Error,
            )
        },
        SearchOutcome::Success(results) if results.is_empty() => unsafe {
            state.empty_title = "검색 결과가 없습니다";
            state.empty_detail = "";
            set_status(hwnd, state, "", StatusTone::Normal)
        },
        SearchOutcome::Success(results) => {
            state.results = results;
            let window = hwnd as isize;
            state.thumbnails.load(
                state
                    .results
                    .iter()
                    .map(|item| item.thumbnail_url.clone())
                    .collect(),
                move |message| {
                    let raw = Box::into_raw(Box::new(message));
                    if unsafe { PostMessageW(window as HWND, WM_THUMBNAIL, 0, raw as isize) } == 0 {
                        unsafe {
                            drop(Box::from_raw(raw));
                        }
                    }
                },
            );
            state.selected = 0;
            state.scroll = 0;
            let exact_count = state
                .results
                .iter()
                .filter(|result| !result.is_recommendation && !result.is_fallback)
                .count();
            let general_count = state
                .results
                .iter()
                .filter(|result| result.is_fallback)
                .count();
            let recommendation_count = state.results.len() - exact_count - general_count;
            let copied =
                state.auto_copy && unsafe { copy_to_clipboard(hwnd, &state.results[0].url) };
            let mut message = if general_count > 0 {
                format!("조건에 맞는 영상이 없어 일반 YouTube 결과 {general_count}개를 표시합니다.")
            } else {
                format!("곡명 일치 {exact_count}개, 관련 추천 {recommendation_count}개")
            };
            let tone = if state.auto_copy {
                if copied {
                    message.push_str(", 첫 링크 복사 완료");
                    StatusTone::Success
                } else {
                    message.push_str(", 자동 복사 실패, 복사 버튼으로 다시 시도하세요.");
                    StatusTone::Error
                }
            } else {
                StatusTone::Normal
            };
            unsafe {
                set_status(hwnd, state, message, tone);
            }
        }
    }
    unsafe {
        start_feedback(hwnd, state);
        InvalidateRect(hwnd, null(), 0);
    }
}

unsafe fn start_hosting(hwnd: HWND, state: &mut AppState) {
    if state.loading {
        return;
    }
    let Some(result) = state.results.get(state.selected) else {
        return;
    };
    if state.hosting && state.hosting_source.as_deref() == Some(result.url.as_str()) {
        unsafe {
            set_status(
                hwnd,
                state,
                "선택한 영상을 이미 준비하고 있습니다. 다른 영상을 선택하거나 중계 중지를 누르세요.",
                StatusTone::Normal,
            );
        }
        return;
    }
    let Some(relay) = &state.relay else {
        return;
    };
    let message = format!("선택 영상 중계 준비 중: {}", result.title);
    state.hosting_generation = relay.prepare(result.url.clone());
    state.hosting_source = Some(result.url.clone());
    state.hosting = true;
    state.hosted = None;
    unsafe {
        SetWindowTextW(state.hosted_field, wide("").as_ptr());
        set_status(hwnd, state, message, StatusTone::Normal);
        InvalidateRect(hwnd, null(), 0);
    }
}

unsafe fn stop_hosting(hwnd: HWND, state: &mut AppState) {
    if let Some(relay) = &state.relay {
        relay.stop();
    }
    state.hosting_generation = 0;
    state.hosting = false;
    state.hosting_source = None;
    state.hosted = None;
    unsafe {
        SetWindowTextW(state.hosted_field, wide("").as_ptr());
        set_status(hwnd, state, "중계를 중지했습니다.", StatusTone::Normal);
        InvalidateRect(hwnd, null(), 0);
    }
}

unsafe fn toggle_auto_copy(hwnd: HWND, state: &mut AppState) {
    state.auto_copy = !state.auto_copy;
    unsafe {
        SendMessageW(
            state.copy_toggle,
            BM_SETCHECK,
            usize::from(state.auto_copy),
            0,
        );
    }
    let message = if state.auto_copy {
        "자동 복사 켜짐: 다음 검색부터 첫 번째 YouTube 링크를 복사합니다."
    } else {
        "자동 복사 꺼짐: 원하는 영상을 선택해 직접 복사하거나 프록싱할 수 있습니다."
    };
    unsafe {
        set_status(hwnd, state, message, StatusTone::Normal);
    }
}

unsafe fn hosting_event(hwnd: HWND, state: &mut AppState, message: HostingMessage) {
    if message.generation == 0 || message.generation != state.hosting_generation {
        return;
    }
    match message.event {
        HostingEvent::Progress(text) => {
            if !state.loading && state.status != text {
                unsafe {
                    set_status(hwnd, state, text, StatusTone::Normal);
                }
            }
        }
        HostingEvent::Ready {
            source,
            url,
            streaming,
        } => {
            state.hosting = false;
            unsafe {
                SetWindowTextW(state.hosted_field, wide(&url).as_ptr());
            }
            let copied = unsafe { copy_to_clipboard(hwnd, &url) };
            state.hosted = Some((source, url));
            let ready_msg = if copied {
                if streaming {
                    "실시간 중계 준비 완료! 링크가 자동 복사되었습니다. (VRChat 붙여넣기)"
                } else {
                    "파일 중계 준비 완료! 링크가 자동 복사되었습니다. (VRChat 붙여넣기)"
                }
            } else if streaming {
                "중계 준비 완료: 링크를 복사해 VRChat에 넣으세요."
            } else {
                "파일 중계 준비 완료: 링크를 복사해 VRChat에 넣으세요."
            };
            unsafe {
                set_status(hwnd, state, ready_msg, StatusTone::Success);
                InvalidateRect(hwnd, null(), 0);
            }
        }
        HostingEvent::Failed(error) => {
            state.hosting = false;
            state.hosting_source = None;
            state.hosted = None;
            unsafe {
                SetWindowTextW(state.hosted_field, wide("").as_ptr());
                set_status(
                    hwnd,
                    state,
                    format!("프록싱 실패: {error}"),
                    StatusTone::Error,
                );
                InvalidateRect(hwnd, null(), 0);
            }
        }
    }
}

unsafe fn ime_composing(edit: HWND) -> bool {
    let context = unsafe { ImmGetContext(edit) };
    if context.is_null() {
        return false;
    }
    let length = unsafe { ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0) };
    unsafe { ImmReleaseContext(edit, context) };
    length > 0
}

unsafe fn activate_zone(hwnd: HWND, state: &mut AppState, zone: HitZone, double_click: bool) {
    match zone {
        HitZone::Query => unsafe {
            SetFocus(state.query);
        },
        HitZone::Search => unsafe { start_search(hwnd, state) },
        HitZone::Pronunciation | HitZone::Karaoke => {
            let mode = if zone == HitZone::Pronunciation {
                SearchMode::Pronunciation
            } else {
                SearchMode::Karaoke
            };
            if state.mode != mode {
                state.mode = mode;
                state.search_generation = state.search_generation.wrapping_add(1);
                state.loading = false;
                state.empty_title = "";
                state.empty_detail = "";
                state.thumbnails.clear();
                state.results.clear();
                state.selected = 0;
                state.scroll = 0;
                unsafe {
                    set_status(hwnd, state, "", StatusTone::Normal);
                    if !get_text(state.query).trim().is_empty() {
                        PostMessageW(hwnd, WM_TRIGGER_SEARCH, 0, 0);
                    }
                };
            }
            unsafe {
                SetFocus(state.query);
                InvalidateRect(hwnd, null(), 0)
            };
        }
        HitZone::ToggleRelay => {
            state.relay_expanded = !state.relay_expanded;
            unsafe {
                layout_edit(hwnd, state);
                InvalidateRect(hwnd, null(), 0);
            }
        }
        HitZone::HostSelected => {
            if !state.loading && state.results.get(state.selected).is_some() {
                unsafe { start_hosting(hwnd, state) };
            }
        }
        HitZone::StopHosting => {
            if state.hosting || state.hosting_source.is_some() {
                unsafe { stop_hosting(hwnd, state) };
            }
        }
        HitZone::Copy => unsafe { copy_selected(hwnd, state) },
        HitZone::Open => unsafe { open_selected(hwnd, state) },
        HitZone::CopyHosted => {
            if let Some((_, url)) = &state.hosted {
                let copied = unsafe { copy_to_clipboard(hwnd, url) };
                unsafe {
                    set_status(
                        hwnd,
                        state,
                        if copied {
                            "중계 링크를 복사했습니다. VRChat에 바로 붙여넣기 하세요."
                        } else {
                            "복사 실패: 중계 주소창에서 직접 복사하세요."
                        },
                        if copied {
                            StatusTone::Success
                        } else {
                            StatusTone::Error
                        },
                    );
                }
            }
        }
        HitZone::Result(index) if index < state.results.len() => {
            state.selected = index;
            unsafe {
                start_feedback(hwnd, state);
                SetFocus(hwnd);
                if double_click {
                    copy_selected(hwnd, state);
                }
                InvalidateRect(hwnd, null(), 0);
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize) };
    }
    let state = unsafe { state_ptr(hwnd) };
    match message {
        WM_CREATE => {
            if !state.is_null() {
                unsafe {
                    create_ui(hwnd, &mut *state);
                    update_window_icons(hwnd, &mut *state);
                };
            }
            0
        }
        WM_GETMINMAXINFO => {
            let info = lparam as *mut MINMAXINFO;
            if !info.is_null() {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
                unsafe {
                    (*info).ptMinTrackSize.x = scale(720, dpi);
                    (*info).ptMinTrackSize.y = scale(620, dpi);
                }
            }
            0
        }
        WM_SIZE => {
            if !state.is_null() {
                let visible = make_layout(hwnd).results.len();
                unsafe {
                    (*state).scroll = (*state)
                        .scroll
                        .min((*state).results.len().saturating_sub(visible));
                    layout_edit(hwnd, &*state);
                }
            }
            0
        }
        WM_DPICHANGED => {
            let suggested = lparam as *const RECT;
            if !suggested.is_null() {
                let rect = unsafe { *suggested };
                unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                };
            }
            if !state.is_null() {
                unsafe {
                    replace_fonts(hwnd, &mut *state);
                    update_window_icons(hwnd, &mut *state);
                    layout_edit(hwnd, &*state);
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            0
        }
        WM_PAINT => {
            if !state.is_null() {
                unsafe { paint_ui(hwnd, &*state) };
                0
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            if state.is_null() {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            let hdc = wparam as HDC;
            unsafe {
                SetTextColor(hdc, TEXT);
                SetBkColor(hdc, SURFACE);
                SetBkMode(hdc, OPAQUE as i32);
                (*state).edit_brush as isize
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            unsafe { InvalidateRect(hwnd, null(), 0) };
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            if !state.is_null() {
                let (x, y) = mouse_point(lparam);
                let next = unsafe { hit_test(hwnd, &*state, x, y) };
                if unsafe { (*state).hover } != next {
                    unsafe {
                        (*state).hover = next;
                        start_feedback(hwnd, &mut *state);
                    }
                }
                if !unsafe { (*state).tracking_mouse } {
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut tracking) };
                    unsafe { (*state).tracking_mouse = true };
                }
            }
            0
        }
        WM_MOUSELEAVE => {
            if !state.is_null() {
                unsafe {
                    (*state).hover = HitZone::None;
                    (*state).pressed = HitZone::None;
                    (*state).tracking_mouse = false;
                    start_feedback(hwnd, &mut *state);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !state.is_null() {
                let (x, y) = mouse_point(lparam);
                let zone = unsafe { hit_test(hwnd, &*state, x, y) };
                unsafe {
                    (*state).pressed = zone;
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if !state.is_null() {
                let (x, y) = mouse_point(lparam);
                let zone = unsafe { hit_test(hwnd, &*state, x, y) };
                let was_pressed = unsafe { (*state).pressed };
                unsafe {
                    (*state).pressed = HitZone::None;
                    InvalidateRect(hwnd, null(), 0);
                    if was_pressed != HitZone::None && was_pressed == zone {
                        activate_zone(hwnd, &mut *state, zone, false);
                    }
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            if !state.is_null() {
                let (x, y) = mouse_point(lparam);
                let zone = unsafe { hit_test(hwnd, &*state, x, y) };
                unsafe {
                    activate_zone(hwnd, &mut *state, zone, true);
                }
            }
            0
        }
        WM_SETTINGCHANGE => {
            if !state.is_null() {
                unsafe {
                    (*state).reduce_motion = !client_animations_enabled();
                    start_feedback(hwnd, &mut *state);
                }
            }
            0
        }
        WM_TIMER => {
            if wparam == TIMER_ANIM && !state.is_null() {
                let mut still_animating = false;
                let active_idx = zone_index(unsafe { (*state).hover }, unsafe { (*state).scroll });
                for i in 0..NUM_ZONES {
                    let target = if active_idx == Some(i) {
                        1.0f32
                    } else {
                        0.0f32
                    };
                    let cur = unsafe { (*state).hover_progress[i] };
                    let diff = target - cur;
                    if diff.abs() > 0.02 {
                        still_animating = true;
                        let speed = if target > cur { 0.35 } else { 0.22 };
                        unsafe {
                            (*state).hover_progress[i] = cur + diff * speed;
                        }
                    } else {
                        unsafe {
                            (*state).hover_progress[i] = target;
                        }
                    }
                }
                for slot in 0..MAX_VISIBLE_RESULTS {
                    let target = f32::from(unsafe { (*state).selected == (*state).scroll + slot });
                    let value = unsafe { &mut (*state).selection_progress[slot] };
                    let diff = target - *value;
                    if diff.abs() > 0.02 {
                        *value += diff * 0.4;
                        still_animating = true;
                    } else {
                        *value = target;
                    }
                }
                unsafe {
                    InvalidateRect(hwnd, null(), 0);
                    if !still_animating {
                        KillTimer(hwnd, TIMER_ANIM);
                        (*state).animating = false;
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !state.is_null() {
                let (x, y) = mouse_point(lparam);
                let mut point = windows_sys::Win32::Foundation::POINT { x, y };
                unsafe { ScreenToClient(hwnd, &mut point) };
                if !point_in(&make_layout(hwnd).empty_results, point.x, point.y) {
                    return 0;
                }
                let wheel_delta = ((wparam >> 16) as i16) as i32;
                let delta = unsafe { (*state).wheel_remainder } + wheel_delta;
                let steps = delta / WHEEL_DELTA as i32;
                unsafe {
                    (*state).wheel_remainder = delta % WHEEL_DELTA as i32;
                }
                if steps == 0 {
                    return 0;
                }
                let visible = make_layout(hwnd).results.len();
                let max_scroll = unsafe { (*state).results.len().saturating_sub(visible) };
                let old_scroll = unsafe { (*state).scroll };
                let new_scroll = if steps < 0 {
                    old_scroll
                        .saturating_add(steps.unsigned_abs() as usize)
                        .min(max_scroll)
                } else {
                    old_scroll.saturating_sub(steps as usize)
                };
                if new_scroll != old_scroll {
                    unsafe {
                        (*state).scroll = new_scroll;
                        (*state).hover = HitZone::None;
                        (&mut (*state).hover_progress)[8..].fill(0.0);
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
            }
            0
        }
        WM_TRIGGER_SEARCH => {
            if !state.is_null() {
                unsafe { start_search(hwnd, &mut *state) };
            }
            0
        }
        WM_THUMBNAIL => {
            if lparam != 0 {
                let message = unsafe { *Box::from_raw(lparam as *mut thumbnails::Message) };
                if !state.is_null() {
                    unsafe {
                        (*state).thumbnails.apply(message);
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
            }
            0
        }
        WM_SEARCH_DONE => {
            if lparam != 0 {
                let outcome = unsafe { *Box::from_raw(lparam as *mut SearchMessage) };
                if !state.is_null() {
                    unsafe { finish_search(hwnd, &mut *state, outcome) };
                }
            }
            0
        }
        WM_COMMAND => {
            if !state.is_null()
                && (wparam >> 16) == BN_CLICKED as usize
                && (wparam & 0xffff) == ID_AUTO_COPY
            {
                unsafe {
                    toggle_auto_copy(hwnd, &mut *state);
                }
            }
            0
        }
        WM_HOSTING_EVENT => {
            if lparam != 0 {
                let message = unsafe { *Box::from_raw(lparam as *mut HostingMessage) };
                if !state.is_null() {
                    unsafe {
                        hosting_event(hwnd, &mut *state, message);
                    }
                }
            }
            0
        }
        #[cfg(debug_assertions)]
        WM_DEBUG_COPY_QUERY => {
            if !state.is_null() {
                let text = unsafe { get_text((*state).query) };
                unsafe { copy_to_clipboard(hwnd, &text) };
            }
            0
        }
        WM_DESTROY => {
            let mut placement: WINDOWPLACEMENT = unsafe { zeroed() };
            placement.length = size_of::<WINDOWPLACEMENT>() as u32;
            unsafe {
                GetWindowPlacement(hwnd, &mut placement);
                let rect = placement.rcNormalPosition;
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;
                if w >= 700 && h >= 580 {
                    save_window_size(w, h);
                }
                PostQuitMessage(0);
            }
            0
        }
        WM_NCDESTROY => {
            if !state.is_null() {
                let owned = unsafe { Box::from_raw(state) };
                unsafe {
                    delete_font(owned.font_body);
                    delete_font(owned.font_button);
                    delete_font(owned.font_card_title);
                    delete_font(owned.font_small);
                    delete_font(owned.font_badge);
                    delete_font(owned.font_title);
                    delete_font(owned.font_credit);
                    if !owned.edit_brush.is_null() {
                        DeleteObject(owned.edit_brush as _);
                    }
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let _fonts = BundledFonts::load();

        // Initialize GDI+ with anti-aliasing support
        let mut gdiplus_token = 0usize;
        let gdiplus_input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };
        GdiplusStartup(&mut gdiplus_token, &gdiplus_input, null_mut());

        let instance = GetModuleHandleW(null());
        let class_name = wide(CLASS_NAME);
        let title = wide(APP_NAME);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hIcon: LoadIconW(instance, APP_ICON_RESOURCE as usize as *const u16),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: GetStockObject(WHITE_BRUSH) as HBRUSH,
            lpszClassName: class_name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            GdiplusShutdown(gdiplus_token);
            return;
        }
        let state = Box::into_raw(Box::new(AppState::new()));
        let (saved_width, saved_height) = load_window_size();
        let mut work_area: RECT = zeroed();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work_area as *mut _ as _, 0) == 0 {
            work_area.right = GetSystemMetrics(SM_CXSCREEN);
            work_area.bottom = GetSystemMetrics(SM_CYSCREEN);
        }
        let screen_w = work_area.right - work_area.left;
        let screen_h = work_area.bottom - work_area.top;
        let win_w = saved_width.min(screen_w);
        let win_h = saved_height.min(screen_h);
        let x = work_area.left + ((screen_w - win_w) / 2).max(0);
        let y = work_area.top + ((screen_h - win_h) / 2).max(0);

        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            win_w,
            win_h,
            null_mut(),
            null_mut(),
            instance,
            state as *const _,
        );
        if hwnd.is_null() {
            GdiplusShutdown(gdiplus_token);
            return;
        }
        let dark: i32 = 0;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const _ as _,
            size_of::<i32>() as u32,
        );
        let corner: i32 = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as _,
            size_of::<i32>() as u32,
        );
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut message: MSG = zeroed();
        let mut control_down = false;
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            let current = state_ptr(hwnd);
            if message.message == WM_KEYDOWN && message.wParam == VK_CONTROL as usize {
                control_down = true;
            } else if (message.message == WM_KEYUP && message.wParam == VK_CONTROL as usize)
                || message.message == WM_KILLFOCUS
            {
                control_down = false;
            }

            // Keyboard navigation for search results
            if !current.is_null() && message.message == WM_KEYDOWN && GetFocus() == hwnd {
                if message.wParam == VK_RETURN as usize
                    || (message.wParam == b'C' as usize && GetKeyState(VK_CONTROL as i32) < 0)
                {
                    copy_selected(hwnd, &mut *current);
                    continue;
                } else if message.wParam == VK_DOWN as usize {
                    let cur = &mut *current;
                    if !cur.results.is_empty() && cur.selected + 1 < cur.results.len() {
                        cur.selected += 1;
                        let visible = make_layout(hwnd).results.len();
                        if cur.selected >= cur.scroll + visible {
                            cur.scroll = cur.selected + 1 - visible;
                        }
                        start_feedback(hwnd, cur);
                        InvalidateRect(hwnd, null(), 0);
                        continue;
                    }
                } else if message.wParam == VK_UP as usize {
                    let cur = &mut *current;
                    if !cur.results.is_empty() && cur.selected > 0 {
                        cur.selected -= 1;
                        if cur.selected < cur.scroll {
                            cur.scroll = cur.selected;
                        }
                        start_feedback(hwnd, cur);
                        InvalidateRect(hwnd, null(), 0);
                        continue;
                    }
                }
            }

            // Classic Win32 EDIT controls need an explicit Ctrl+A handler.
            if !current.is_null()
                && message.message == WM_KEYDOWN
                && message.wParam == b'A' as usize
                && GetFocus() == (*current).query
                && (control_down || GetKeyState(VK_CONTROL as i32) < 0)
            {
                SendMessageW((*current).query, EM_SETSEL, 0, -1);
                continue;
            }

            if !current.is_null()
                && message.message == WM_KEYDOWN
                && message.wParam == VK_BACK as usize
                && GetFocus() == (*current).query
                && (control_down || GetKeyState(VK_CONTROL as i32) < 0)
                && !ime_composing((*current).query)
            {
                delete_previous_word((*current).query);
                continue;
            }
            if !current.is_null()
                && message.message == WM_KEYDOWN
                && message.wParam == VK_RETURN as usize
                && GetFocus() == (*current).query
                && !ime_composing((*current).query)
            {
                PostMessageW(hwnd, WM_TRIGGER_SEARCH, 0, 0);
                continue;
            }
            if IsDialogMessageW(hwnd, &message) != 0 {
                continue;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        GdiplusShutdown(gdiplus_token);
    }
}

#[cfg(test)]
mod input_tests {
    use super::{
        HitZone, layout_for_size, layout_for_state, previous_word_start, scale, zone_index,
    };

    #[test]
    fn embedded_window_icons_load_at_each_display_scale() {
        unsafe {
            for dpi in [96, 120, 144, 192] {
                let icons = super::WindowIcons::load(dpi).expect("Miku icon resource is missing");
                for (icon, metric) in [
                    (icons.small, super::SM_CXSMICON),
                    (icons.large, super::SM_CXICON),
                ] {
                    let mut info: super::ICONINFO = super::zeroed();
                    assert_ne!(super::GetIconInfo(icon, &mut info), 0);
                    let mut bitmap: super::BITMAP = super::zeroed();
                    assert_ne!(
                        super::GetObjectW(
                            info.hbmColor.cast(),
                            super::size_of::<super::BITMAP>() as i32,
                            (&mut bitmap as *mut super::BITMAP).cast()
                        ),
                        0
                    );
                    let expected = super::GetSystemMetricsForDpi(metric, dpi);
                    assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (expected, expected));
                    super::DeleteObject(info.hbmColor.cast());
                    super::DeleteObject(info.hbmMask.cast());
                }
            }
        }
    }

    #[test]
    fn bundled_fonts_resolve_and_cover_korean_and_japanese() {
        let _fonts = super::BundledFonts::load();
        assert!(super::FONT_READY.load(super::Ordering::Relaxed));
        unsafe {
            let dc = super::CreateCompatibleDC(super::null_mut());
            for weight in [400, 600] {
                let font = super::create_font(16, weight, 96);
                let previous = super::SelectObject(dc, font.cast());
                let mut face = [0u16; 128];
                let count = super::GetTextFaceW(dc, face.len() as i32, face.as_mut_ptr());
                let name = String::from_utf16_lossy(&face[..(count - 1) as usize]);
                assert!(name.starts_with("Pretendard JP"), "Unexpected font: {name}");
                let text: Vec<u16> = "두근어질きゅうくらりん歌詞可不".encode_utf16().collect();
                let mut glyphs = vec![0u16; text.len()];
                super::GetGlyphIndicesW(
                    dc,
                    text.as_ptr(),
                    text.len() as i32,
                    glyphs.as_mut_ptr(),
                    super::GGI_MARK_NONEXISTING_GLYPHS,
                );
                assert!(!glyphs.contains(&0xffff), "Missing glyphs in {name}");
                super::SelectObject(dc, previous);
                super::DeleteObject(font.cast());
            }
            super::DeleteDC(dc);
        }
    }

    struct HiddenSearchWindow(super::HWND);
    impl Drop for HiddenSearchWindow {
        fn drop(&mut self) {
            unsafe {
                super::DestroyWindow(self.0);
            }
        }
    }
    unsafe fn hidden_search_window(text: &str) -> (HiddenSearchWindow, super::AppState) {
        unsafe {
            let host = super::CreateWindowExW(
                0,
                super::wide("STATIC").as_ptr(),
                super::wide("").as_ptr(),
                super::WS_POPUP,
                0,
                0,
                0,
                0,
                super::null_mut(),
                super::null_mut(),
                super::GetModuleHandleW(super::null()),
                super::null(),
            );
            assert!(!host.is_null());
            let window = HiddenSearchWindow(host);
            let query = super::CreateWindowExW(
                0,
                super::wide("EDIT").as_ptr(),
                super::wide(text).as_ptr(),
                super::WS_CHILD | super::ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                host,
                super::null_mut(),
                super::GetModuleHandleW(super::null()),
                super::null(),
            );
            assert!(!query.is_null());
            let mut state = super::AppState::new();
            state.query = query;
            state.reduce_motion = true;
            (window, state)
        }
    }

    #[test]
    fn search_shell_padding_remains_an_input_target() {
        unsafe {
            let (window, mut state) = hidden_search_window("두근어질");
            for (width, height) in [(800, 650), (1280, 900)] {
                super::SetWindowPos(
                    window.0,
                    super::null_mut(),
                    0,
                    0,
                    width,
                    height,
                    super::SWP_NOZORDER | super::SWP_NOACTIVATE,
                );
                let layout = super::make_layout(window.0);
                let zone = super::hit_test(
                    window.0,
                    &state,
                    layout.query_shell.left + 2,
                    layout.query_shell.top + 2,
                );
                assert_ne!(zone, super::HitZone::None);
                super::activate_zone(window.0, &mut state, zone, false);
                assert_eq!(super::GetFocus(), state.query);
                assert_eq!(super::get_text(state.query), "두근어질");
            }
        }
    }

    #[test]
    fn late_search_results_and_errors_cannot_replace_the_active_tab() {
        unsafe {
            let (window, mut state) = hidden_search_window("두근어질");
            state.mode = super::SearchMode::Karaoke;
            state.search_generation = 2;
            state.loading = true;
            state.status = "현재 검색".into();
            let old_result = super::VideoResult {
                title: "이전 발음 영상".into(),
                url: String::new(),
                channel: String::new(),
                thumbnail_url: String::new(),
                score: 0,
                matched: Vec::new(),
                kind: music_link_finder::search::VideoKind::Pronunciation,
                is_recommendation: false,
                is_fallback: false,
            };
            super::finish_search(
                window.0,
                &mut state,
                super::SearchMessage {
                    generation: 1,
                    outcome: super::SearchOutcome::Success(vec![old_result]),
                },
            );
            assert!(state.loading);
            assert!(state.results.is_empty());
            assert_eq!(state.status, "현재 검색");
            super::finish_search(
                window.0,
                &mut state,
                super::SearchMessage {
                    generation: 2,
                    outcome: super::SearchOutcome::Success(Vec::new()),
                },
            );
            assert!(!state.loading);
            assert_eq!(state.empty_title, "검색 결과가 없습니다");
            super::finish_search(
                window.0,
                &mut state,
                super::SearchMessage {
                    generation: 1,
                    outcome: super::SearchOutcome::Error("늦은 오류".into()),
                },
            );
            assert_eq!(state.empty_title, "검색 결과가 없습니다");
            assert!(state.status.is_empty());
            state.search_generation = 3;
            state.loading = true;
            super::finish_search(
                window.0,
                &mut state,
                super::SearchMessage {
                    generation: 2,
                    outcome: super::SearchOutcome::Success(Vec::new()),
                },
            );
            assert!(state.loading);
            super::finish_search(
                window.0,
                &mut state,
                super::SearchMessage {
                    generation: 3,
                    outcome: super::SearchOutcome::Error("현재 오류".into()),
                },
            );
            assert!(!state.loading);
            assert_eq!(state.status, "검색 실패: 현재 오류");
        }
    }

    #[test]
    fn changing_tabs_during_search_schedules_the_new_mode() {
        unsafe {
            let (window, mut state) = hidden_search_window("두근어질");
            state.loading = true;
            super::activate_zone(window.0, &mut state, super::HitZone::Karaoke, false);
            assert_eq!(state.mode, super::SearchMode::Karaoke);
            assert!(!state.loading);
            assert_eq!(super::get_text(state.query), "두근어질");
            let mut message: super::MSG = super::zeroed();
            assert_ne!(
                super::PeekMessageW(
                    &mut message,
                    window.0,
                    super::WM_TRIGGER_SEARCH,
                    super::WM_TRIGGER_SEARCH,
                    super::PM_REMOVE
                ),
                0
            );
            super::activate_zone(window.0, &mut state, super::HitZone::Karaoke, false);
            assert_eq!(
                super::PeekMessageW(
                    &mut message,
                    window.0,
                    super::WM_TRIGGER_SEARCH,
                    super::WM_TRIGGER_SEARCH,
                    super::PM_REMOVE
                ),
                0
            );
            super::SetWindowTextW(state.query, super::wide("").as_ptr());
            state.loading = true;
            super::activate_zone(window.0, &mut state, super::HitZone::Pronunciation, false);
            assert_eq!(state.mode, super::SearchMode::Pronunciation);
            assert!(!state.loading);
            assert_eq!(
                super::PeekMessageW(
                    &mut message,
                    window.0,
                    super::WM_TRIGGER_SEARCH,
                    super::WM_TRIGGER_SEARCH,
                    super::PM_REMOVE
                ),
                0
            );
        }
    }

    #[test]
    fn ctrl_a_selects_and_replaces_all_text_in_the_search_edit() {
        unsafe {
            // Exercise the native edit and its subclass without showing a window.
            let edit = super::CreateWindowExW(
                0,
                super::wide("EDIT").as_ptr(),
                super::wide("").as_ptr(),
                super::WS_POPUP | super::ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                super::null_mut(),
                super::null_mut(),
                super::GetModuleHandleW(super::null()),
                super::null(),
            );
            assert!(!edit.is_null());
            assert_ne!(
                super::SetWindowSubclass(edit, Some(super::edit_subclass_proc), 1, 0),
                0
            );
            for text in ["", "song title", "두근어질 きゅうくらりん 🎵"] {
                super::SetWindowTextW(edit, super::wide(text).as_ptr());
                super::SendMessageW(edit, super::EM_SETSEL, 1, 1);
                super::SendMessageW(edit, super::WM_CHAR, 0x01, 0);
                let mut start = 0u32;
                let mut end = 0u32;
                super::SendMessageW(
                    edit,
                    super::EM_GETSEL,
                    (&mut start as *mut u32) as usize,
                    (&mut end as *mut u32) as isize,
                );
                assert_eq!((start, end), (0, text.encode_utf16().count() as u32));
                assert_eq!(super::get_text(edit), text);
                super::SendMessageW(edit, super::WM_CHAR, b'x' as usize, 0);
                assert_eq!(super::get_text(edit), "x");
            }
            super::DestroyWindow(edit);
        }
    }

    #[test]
    fn ctrl_backspace_finds_korean_and_ascii_word_boundaries() {
        let korean: Vec<u16> = "두근어질 검색".encode_utf16().collect();
        assert_eq!(previous_word_start(&korean, korean.len()), 5);
        assert_eq!(previous_word_start(&korean, 4), 0);

        let spaced: Vec<u16> = "song title   ".encode_utf16().collect();
        assert_eq!(previous_word_start(&spaced, spaced.len()), 5);
    }

    #[test]
    fn result_rows_and_actions_never_overlap_at_supported_sizes_and_dpi() {
        for dpi in [96, 120, 144, 192] {
            for width in [704, 1000, 1040, 1240, 1600] {
                for height in [580, 620, 700, 760, 821, 900, 1200] {
                    let layout = layout_for_size(scale(width, dpi), scale(height, dpi), dpi);
                    assert!(!layout.results.is_empty());
                    assert!(layout.results.len() <= super::MAX_VISIBLE_RESULTS);
                    for rect in &layout.results {
                        assert!(rect.bottom - rect.top >= scale(52, dpi));
                        assert!(rect.top >= layout.results_label.bottom);
                        if let Some(sidebar) = layout.sidebar {
                            assert!(rect.right < sidebar.left);
                            assert!(rect.bottom <= sidebar.bottom);
                            assert!(layout.relay_card.bottom <= sidebar.bottom);
                            assert!(layout.host_button.left >= sidebar.left);
                            assert!(layout.copy.right < layout.open.left);
                            assert_eq!(layout.copy.top, layout.open.top);
                            assert!(layout.open.bottom < layout.relay_card.top);
                        } else {
                            assert!(rect.bottom < layout.host_button.top);
                        }
                    }
                    if layout.sidebar.is_none() {
                        assert!(layout.empty_results.bottom < layout.host_button.top);
                    }
                }
            }
        }
    }

    #[test]
    fn expanded_relay_and_long_title_space_fit_at_all_supported_dpi() {
        for dpi in [96, 120, 144, 192] {
            for width in [704, 1000, 1040, 1240, 1600] {
                for height in [580, 620, 700, 760, 821, 900, 1200] {
                    let layout = layout_for_state(scale(width, dpi), scale(height, dpi), dpi, true);
                    assert!(layout.copy.right < layout.open.left);
                    assert!(
                        layout.results.last().unwrap().bottom < layout.copy.top
                            || layout.sidebar.is_some()
                    );
                    assert!(layout.results[0].top >= layout.results_label.bottom);
                    assert!(layout.stop_hosting.bottom <= layout.relay_card.bottom);
                    assert!(layout.copy_hosted.bottom <= layout.relay_card.bottom);
                    assert!(layout.relay_card.bottom < layout.status.top);
                    if let Some(title) = layout.detail_title {
                        assert!(title.bottom - title.top >= scale(70, dpi));
                    }
                }
            }
        }
    }

    #[test]
    fn hover_animation_tracks_visible_slots_after_scrolling() {
        assert_eq!(zone_index(HitZone::Result(12), 10), Some(10));
        assert_eq!(zone_index(HitZone::Result(9), 10), None);
        assert_eq!(zone_index(HitZone::Result(17), 10), Some(15));
        assert_eq!(zone_index(HitZone::Result(18), 10), None);
    }
}
