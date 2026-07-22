#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use music_link_finder::search::{SearchMode, VideoResult, search_youtube};
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::thread;
use windows_sys::Win32::Foundation::{GlobalFree, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::*;
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
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::Ime::{
    GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK,
    VK_CONTROL, VK_RETURN,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const APP_NAME: &str = "음악 한글발음 링크 찾기";
const CLASS_NAME: &str = "MusicLinkFinderNativeWindow";
const WM_SEARCH_DONE: u32 = WM_APP + 1;
const WM_TRIGGER_SEARCH: u32 = WM_APP + 2;
#[cfg(debug_assertions)]
const WM_DEBUG_COPY_QUERY: u32 = WM_APP + 99;
const ID_QUERY: i32 = 101;

const fn rgb(red: u32, green: u32, blue: u32) -> u32 {
    red | (green << 8) | (blue << 16)
}

const BG: u32 = rgb(248, 248, 246);
const SURFACE: u32 = rgb(255, 255, 255);
const SURFACE_HOVER: u32 = rgb(241, 241, 238);
const SURFACE_SELECTED: u32 = rgb(247, 247, 244);
const BORDER: u32 = rgb(217, 217, 213);
const TEXT: u32 = rgb(17, 17, 17);
const TEXT_MUTED: u32 = rgb(105, 105, 100);
const ACCENT: u32 = rgb(17, 17, 17);
const ACCENT_HOVER: u32 = rgb(43, 43, 43);
const SUCCESS: u32 = rgb(22, 122, 74);
const ERROR: u32 = rgb(194, 48, 48);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HitZone {
    None,
    Search,
    Pronunciation,
    Karaoke,
    Copy,
    Open,
    Result(usize),
}

#[derive(Clone, Copy)]
enum StatusTone {
    Normal,
    Success,
    Error,
}

struct AppState {
    query: HWND,
    font_body: HFONT,
    font_button: HFONT,
    font_small: HFONT,
    font_title: HFONT,
    edit_brush: HBRUSH,
    results: Vec<VideoResult>,
    selected: usize,
    scroll: usize,
    mode: SearchMode,
    loading: bool,
    hover: HitZone,
    tracking_mouse: bool,
    status: String,
    status_tone: StatusTone,
}

impl AppState {
    fn new() -> Self {
        Self {
            query: null_mut(),
            font_body: null_mut(),
            font_button: null_mut(),
            font_small: null_mut(),
            font_title: null_mut(),
            edit_brush: null_mut(),
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            mode: SearchMode::Pronunciation,
            loading: false,
            hover: HitZone::None,
            tracking_mouse: false,
            status: "곡 제목을 입력하고 Enter를 누르세요.".to_owned(),
            status_tone: StatusTone::Normal,
        }
    }
}

struct UiLayout {
    query_shell: RECT,
    search: RECT,
    pronunciation: RECT,
    karaoke: RECT,
    results: Vec<RECT>,
    copy: RECT,
    open: RECT,
    status: RECT,
}

enum SearchOutcome {
    Success(Vec<VideoResult>),
    Error(String),
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

#[cfg(test)]
mod input_tests {
    use super::previous_word_start;

    #[test]
    fn ctrl_backspace_finds_korean_and_ascii_word_boundaries() {
        let korean: Vec<u16> = "두근어질 검색".encode_utf16().collect();
        assert_eq!(previous_word_start(&korean, korean.len()), 5);
        assert_eq!(previous_word_start(&korean, 4), 0);

        let spaced: Vec<u16> = "song title   ".encode_utf16().collect();
        assert_eq!(previous_word_start(&spaced, spaced.len()), 5);
    }
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
    if message == WM_CHAR && wparam == 0x7f {
        unsafe { delete_previous_word(hwnd) };
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

unsafe fn set_status(hwnd: HWND, state: &mut AppState, text: impl Into<String>, tone: StatusTone) {
    state.status = text.into();
    state.status_tone = tone;
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

unsafe fn create_font(size: i32, weight: i32, dpi: i32) -> HFONT {
    let face = wide("Pretendard JP Variable");
    unsafe {
        CreateFontW(
            -(size * dpi / 96),
            0,
            0,
            0,
            weight,
            0,
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
        delete_font(state.font_small);
        delete_font(state.font_title);
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    state.font_body = unsafe { create_font(17, FW_NORMAL as i32, dpi) };
    state.font_button = unsafe { create_font(15, FW_BOLD as i32, dpi) };
    state.font_small = unsafe { create_font(13, FW_NORMAL as i32, dpi) };
    state.font_title = unsafe { create_font(30, FW_BOLD as i32, dpi) };
    if !state.query.is_null() {
        unsafe { SendMessageW(state.query, WM_SETFONT, state.font_body as usize, 1) };
    }
}

fn make_layout(hwnd: HWND) -> UiLayout {
    let mut client: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    let width = client.right;
    let height = client.bottom;
    let m = scale(38, dpi);
    let gap = scale(12, dpi);
    let search_w = scale(190, dpi);
    let input_top = scale(148, dpi);
    let input_h = scale(58, dpi);
    let segment_top = input_top + input_h + scale(14, dpi);
    let segment_h = scale(44, dpi);
    let segment_w = scale(205, dpi);
    let results_top = segment_top + segment_h + scale(58, dpi);
    let action_h = scale(48, dpi);
    let action_w = scale(174, dpi);
    let action_top = height - m - action_h;
    let status_h = scale(28, dpi);
    let status_top = action_top - status_h - scale(16, dpi);
    let result_gap = scale(9, dpi);
    let result_area = (status_top - scale(12, dpi) - results_top).max(scale(240, dpi));
    let row_h = ((result_area - result_gap * 4) / 5).max(scale(52, dpi));
    let mut results = Vec::with_capacity(5);
    for index in 0..5 {
        let top = results_top + index * (row_h + result_gap);
        results.push(RECT {
            left: m,
            top,
            right: width - m,
            bottom: top + row_h,
        });
    }
    UiLayout {
        query_shell: RECT {
            left: m,
            top: input_top,
            right: width - m - search_w - gap,
            bottom: input_top + input_h,
        },
        search: RECT {
            left: width - m - search_w,
            top: input_top,
            right: width - m,
            bottom: input_top + input_h,
        },
        pronunciation: RECT {
            left: m,
            top: segment_top,
            right: m + segment_w,
            bottom: segment_top + segment_h,
        },
        karaoke: RECT {
            left: m + segment_w + gap,
            top: segment_top,
            right: m + segment_w * 2 + gap,
            bottom: segment_top + segment_h,
        },
        results,
        copy: RECT {
            left: m,
            top: action_top,
            right: m + action_w,
            bottom: action_top + action_h,
        },
        open: RECT {
            left: m + action_w + gap,
            top: action_top,
            right: m + action_w * 2 + gap,
            bottom: action_top + action_h,
        },
        status: RECT {
            left: m,
            top: status_top,
            right: width - m,
            bottom: status_top + status_h,
        },
    }
}

unsafe fn layout_edit(hwnd: HWND, state: &AppState) {
    let layout = make_layout(hwnd);
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    let px = scale(18, dpi);
    let py = scale(9, dpi);
    unsafe {
        MoveWindow(
            state.query,
            layout.query_shell.left + px,
            layout.query_shell.top + py,
            layout.query_shell.right - layout.query_shell.left - px * 2,
            layout.query_shell.bottom - layout.query_shell.top - py * 2,
            1,
        )
    };
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
    unsafe {
        SetWindowSubclass(state.query, Some(edit_subclass_proc), 1, 0);
        SendMessageW(
            state.query,
            EM_SETCUEBANNER,
            1,
            wide("곡 제목을 입력하세요  예: 누덕누덕 스타카토").as_ptr() as isize,
        );
        replace_fonts(hwnd, state);
        layout_edit(hwnd, state);
        SetFocus(state.query);
    }
}

unsafe fn select_font(hdc: HDC, font: HFONT) -> HGDIOBJ {
    unsafe { SelectObject(hdc, font as _) }
}

unsafe fn draw_text(hdc: HDC, text: &str, rect: RECT, color: u32, font: HFONT, format: u32) {
    let value: Vec<u16> = text.encode_utf16().collect();
    let mut target = rect;
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);
        let previous = select_font(hdc, font);
        DrawTextW(hdc, value.as_ptr(), value.len() as i32, &mut target, format);
        SelectObject(hdc, previous);
    }
}

unsafe fn rounded_box(hdc: HDC, rect: RECT, fill: u32, outline: u32, radius: i32) {
    let brush = unsafe { CreateSolidBrush(fill) };
    let pen = unsafe { CreatePen(PS_SOLID, 1, outline) };
    unsafe {
        let old_brush = SelectObject(hdc, brush as _);
        let old_pen = SelectObject(hdc, pen as _);
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(pen as _);
        DeleteObject(brush as _);
    }
}

unsafe fn draw_button(
    hdc: HDC,
    state: &AppState,
    rect: RECT,
    zone: HitZone,
    label: &str,
    primary: bool,
    enabled: bool,
    dpi: i32,
) {
    let hovered = state.hover == zone && enabled;
    let fill = if !enabled {
        SURFACE
    } else if primary && hovered {
        ACCENT_HOVER
    } else if primary {
        ACCENT
    } else if hovered {
        SURFACE_HOVER
    } else {
        SURFACE
    };
    let outline = if primary { fill } else { BORDER };
    unsafe { rounded_box(hdc, rect, fill, outline, scale(12, dpi)) };
    let color = if enabled {
        if primary { BG } else { TEXT }
    } else {
        TEXT_MUTED
    };
    unsafe {
        draw_text(
            hdc,
            label,
            rect,
            color,
            state.font_button,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        )
    };
}

unsafe fn paint_ui(hwnd: HWND, state: &AppState) {
    let mut paint: PAINTSTRUCT = unsafe { zeroed() };
    let window_dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let width = client.right.max(1);
    let height = client.bottom.max(1);
    let memory_dc = unsafe { CreateCompatibleDC(window_dc) };
    let bitmap = unsafe { CreateCompatibleBitmap(window_dc, width, height) };
    let old_bitmap = unsafe { SelectObject(memory_dc, bitmap as _) };
    let background = unsafe { CreateSolidBrush(BG) };
    unsafe { FillRect(memory_dc, &client, background) };
    unsafe { DeleteObject(background as _) };

    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    let layout = make_layout(hwnd);
    let m = scale(38, dpi);
    let brand_rect = RECT {
        left: m,
        top: scale(26, dpi),
        right: client.right - m,
        bottom: scale(48, dpi),
    };
    unsafe {
        draw_text(
            memory_dc,
            "LINKFINDER",
            brand_rect,
            ACCENT,
            state.font_small,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        )
    };
    let title_rect = RECT {
        left: m,
        top: scale(55, dpi),
        right: client.right - m,
        bottom: scale(100, dpi),
    };
    unsafe {
        draw_text(
            memory_dc,
            "원하는 노래, 바로 복사.",
            title_rect,
            TEXT,
            state.font_title,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        )
    };
    let subtitle_rect = RECT {
        left: m,
        top: scale(104, dpi),
        right: client.right - m,
        bottom: scale(132, dpi),
    };
    unsafe {
        draw_text(
            memory_dc,
            "한글발음 영상과 노래방 반주를 비교해 가장 정확한 링크를 찾습니다.",
            subtitle_rect,
            TEXT_MUTED,
            state.font_small,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        rounded_box(
            memory_dc,
            layout.query_shell,
            SURFACE,
            if GetFocus() == state.query {
                ACCENT
            } else {
                BORDER
            },
            scale(14, dpi),
        );
        draw_button(
            memory_dc,
            state,
            layout.search,
            HitZone::Search,
            if state.loading {
                "검색 중…"
            } else {
                "검색하고 자동복사"
            },
            true,
            !state.loading,
            dpi,
        );
        draw_button(
            memory_dc,
            state,
            layout.pronunciation,
            HitZone::Pronunciation,
            "한글발음 영상",
            state.mode == SearchMode::Pronunciation,
            true,
            dpi,
        );
        draw_button(
            memory_dc,
            state,
            layout.karaoke,
            HitZone::Karaoke,
            "노래방 · 카라오케",
            state.mode == SearchMode::Karaoke,
            true,
            dpi,
        );
    }

    let result_label = RECT {
        left: m,
        top: layout.results[0].top - scale(40, dpi),
        right: client.right - m,
        bottom: layout.results[0].top - scale(10, dpi),
    };
    unsafe {
        draw_text(
            memory_dc,
            if state.results.is_empty() {
                "검색 결과"
            } else {
                "검색 결과  ·  아래로 스크롤하면 다른 추천곡이 이어집니다"
            },
            result_label,
            TEXT_MUTED,
            state.font_small,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        )
    };

    if state.results.is_empty() {
        let empty_rect = RECT {
            left: layout.results[0].left,
            top: layout.results[0].top,
            right: layout.results[4].right,
            bottom: layout.results[4].bottom,
        };
        unsafe { rounded_box(memory_dc, empty_rect, SURFACE, BORDER, scale(12, dpi)) };
        unsafe {
            draw_text(
                memory_dc,
                if state.loading {
                    "검색 결과를 정리하고 있습니다…"
                } else {
                    "검색 결과가 여기에 표시됩니다."
                },
                empty_rect,
                TEXT_MUTED,
                state.font_body,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            )
        };
    }

    for (slot, rect) in layout.results.iter().copied().enumerate() {
        if state.results.is_empty() {
            break;
        }
        let index = state.scroll + slot;
        let selected = index == state.selected && index < state.results.len();
        let hovered = state.hover == HitZone::Result(index);
        let fill = if selected {
            SURFACE_SELECTED
        } else if hovered {
            SURFACE_HOVER
        } else {
            SURFACE
        };
        let outline = if selected { ACCENT } else { BORDER };
        unsafe { rounded_box(memory_dc, rect, fill, outline, scale(12, dpi)) };
        if let Some(result) = state.results.get(index) {
            let number = if result.is_recommendation {
                "추천".to_owned()
            } else {
                format!("{:02}", index + 1)
            };
            let number_rect = RECT {
                left: rect.left + scale(18, dpi),
                top: rect.top,
                right: rect.left + scale(55, dpi),
                bottom: rect.bottom,
            };
            unsafe {
                draw_text(
                    memory_dc,
                    &number,
                    number_rect,
                    if result.is_recommendation {
                        SUCCESS
                    } else if selected {
                        ACCENT
                    } else {
                        TEXT_MUTED
                    },
                    state.font_button,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                )
            };
            let title_rect = RECT {
                left: rect.left + scale(62, dpi),
                top: rect.top + scale(10, dpi),
                right: rect.right - scale(20, dpi),
                bottom: rect.top + (rect.bottom - rect.top) * 58 / 100,
            };
            unsafe {
                draw_text(
                    memory_dc,
                    &result.title,
                    title_rect,
                    TEXT,
                    state.font_button,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                )
            };
            let channel_rect = RECT {
                left: rect.left + scale(62, dpi),
                top: rect.top + (rect.bottom - rect.top) * 52 / 100,
                right: rect.right - scale(20, dpi),
                bottom: rect.bottom - scale(6, dpi),
            };
            unsafe {
                draw_text(
                    memory_dc,
                    &result.channel,
                    channel_rect,
                    TEXT_MUTED,
                    state.font_small,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                )
            };
        }
    }

    let status_color = match state.status_tone {
        StatusTone::Normal => TEXT_MUTED,
        StatusTone::Success => SUCCESS,
        StatusTone::Error => ERROR,
    };
    unsafe {
        let mut status_left = layout.status;
        status_left.right -= scale(190, dpi);
        draw_text(
            memory_dc,
            &state.status,
            status_left,
            status_color,
            state.font_small,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        if state.results.len() > layout.results.len() {
            let range = format!(
                "마우스 휠  ·  {}–{} / {}",
                state.scroll + 1,
                (state.scroll + layout.results.len()).min(state.results.len()),
                state.results.len()
            );
            draw_text(
                memory_dc,
                &range,
                layout.status,
                TEXT_MUTED,
                state.font_small,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        draw_button(
            memory_dc,
            state,
            layout.copy,
            HitZone::Copy,
            "선택 링크 복사",
            false,
            !state.results.is_empty(),
            dpi,
        );
        draw_button(
            memory_dc,
            state,
            layout.open,
            HitZone::Open,
            "유튜브에서 열기",
            false,
            !state.results.is_empty(),
            dpi,
        );
        BitBlt(window_dc, 0, 0, width, height, memory_dc, 0, 0, SRCCOPY);
        SelectObject(memory_dc, old_bitmap);
        DeleteObject(bitmap as _);
        DeleteDC(memory_dc);
        EndPaint(hwnd, &paint);
    }
}

fn hit_test(hwnd: HWND, state: &AppState, x: i32, y: i32) -> HitZone {
    let layout = make_layout(hwnd);
    if point_in(&layout.search, x, y) {
        HitZone::Search
    } else if point_in(&layout.pronunciation, x, y) {
        HitZone::Pronunciation
    } else if point_in(&layout.karaoke, x, y) {
        HitZone::Karaoke
    } else if point_in(&layout.copy, x, y) {
        HitZone::Copy
    } else if point_in(&layout.open, x, y) {
        HitZone::Open
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
                "선택한 링크를 복사했습니다.",
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

unsafe fn open_selected(state: &AppState) {
    let Some(result) = state.results.get(state.selected) else {
        return;
    };
    let operation = wide("open");
    let url = wide(&result.url);
    unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
}

unsafe fn start_search(hwnd: HWND, state: &mut AppState) {
    if state.loading {
        return;
    }
    let query = unsafe { get_text(state.query) }.trim().to_owned();
    if query.is_empty() {
        unsafe {
            set_status(hwnd, state, "먼저 곡 제목을 입력하세요.", StatusTone::Error);
            SetFocus(state.query);
        }
        return;
    }
    state.loading = true;
    state.results.clear();
    state.selected = 0;
    state.scroll = 0;
    unsafe {
        set_status(
            hwnd,
            state,
            "유튜브 후보를 비교하고 있습니다…",
            StatusTone::Normal,
        )
    };
    let mode = state.mode;
    let hwnd_value = hwnd as isize;
    thread::spawn(move || {
        let outcome = match search_youtube(&query, mode, 30) {
            Ok(results) => SearchOutcome::Success(results),
            Err(error) => SearchOutcome::Error(error.to_string()),
        };
        let raw = Box::into_raw(Box::new(outcome));
        let posted = unsafe { PostMessageW(hwnd_value as HWND, WM_SEARCH_DONE, 0, raw as isize) };
        if posted == 0 {
            unsafe { drop(Box::from_raw(raw)) };
        }
    });
}

unsafe fn finish_search(hwnd: HWND, state: &mut AppState, outcome: SearchOutcome) {
    state.loading = false;
    match outcome {
        SearchOutcome::Error(error) => unsafe {
            set_status(
                hwnd,
                state,
                format!("검색 실패 · {error}"),
                StatusTone::Error,
            )
        },
        SearchOutcome::Success(results) if results.is_empty() => unsafe {
            set_status(
                hwnd,
                state,
                "조건에 맞는 영상을 찾지 못했습니다.",
                StatusTone::Error,
            )
        },
        SearchOutcome::Success(results) => {
            state.results = results;
            state.selected = 0;
            state.scroll = 0;
            let exact_count = state
                .results
                .iter()
                .filter(|result| !result.is_recommendation)
                .count();
            let recommendation_count = state.results.len() - exact_count;
            let copied =
                exact_count > 0 && unsafe { copy_to_clipboard(hwnd, &state.results[0].url) };
            if exact_count == 0 {
                let kind = if state.mode == SearchMode::Karaoke {
                    "정확한 노래방 영상"
                } else {
                    "정확한 한글발음 영상"
                };
                unsafe {
                    set_status(
                        hwnd,
                        state,
                        format!("{kind}을 찾지 못했습니다  ·  추천곡 {recommendation_count}개"),
                        StatusTone::Error,
                    )
                };
            } else if copied {
                unsafe {
                    set_status(
                        hwnd,
                        state,
                        format!(
                            "1순위 자동복사 완료  ·  정확한 결과 {exact_count}개  ·  추천곡 {recommendation_count}개",
                        ),
                        StatusTone::Success,
                    )
                };
            } else {
                unsafe {
                    set_status(
                        hwnd,
                        state,
                        format!(
                            "검색 완료  ·  정확한 결과 {exact_count}개  ·  추천곡 {recommendation_count}개"
                        ),
                        StatusTone::Normal,
                    )
                };
            }
        }
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
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
        HitZone::Search => unsafe { start_search(hwnd, state) },
        HitZone::Pronunciation => {
            state.mode = SearchMode::Pronunciation;
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
        HitZone::Karaoke => {
            state.mode = SearchMode::Karaoke;
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
        HitZone::Copy => unsafe { copy_selected(hwnd, state) },
        HitZone::Open => unsafe { open_selected(state) },
        HitZone::Result(index) if index < state.results.len() => {
            state.selected = index;
            unsafe { InvalidateRect(hwnd, null(), 0) };
            if double_click {
                unsafe { copy_selected(hwnd, state) };
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
                unsafe { create_ui(hwnd, &mut *state) };
            }
            0
        }
        WM_GETMINMAXINFO => {
            let info = lparam as *mut MINMAXINFO;
            if !info.is_null() {
                unsafe {
                    (*info).ptMinTrackSize.x = 900;
                    (*info).ptMinTrackSize.y = 760;
                }
            }
            0
        }
        WM_SIZE => {
            if !state.is_null() {
                unsafe { layout_edit(hwnd, &*state) };
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
        WM_CTLCOLOREDIT => {
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
                    unsafe { (*state).hover = next };
                    unsafe { InvalidateRect(hwnd, null(), 0) };
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
                    (*state).tracking_mouse = false;
                }
                unsafe { InvalidateRect(hwnd, null(), 0) };
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !state.is_null() {
                let wheel_delta = ((wparam >> 16) as i16) as i32;
                let visible = make_layout(hwnd).results.len();
                let max_scroll = unsafe { (*state).results.len().saturating_sub(visible) };
                let old_scroll = unsafe { (*state).scroll };
                let new_scroll = if wheel_delta < 0 {
                    old_scroll.saturating_add(1).min(max_scroll)
                } else {
                    old_scroll.saturating_sub(1)
                };
                if new_scroll != old_scroll {
                    unsafe {
                        (*state).scroll = new_scroll;
                        (*state).hover = HitZone::None;
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
            if !state.is_null() {
                let (x, y) = mouse_point(lparam);
                let zone = unsafe { hit_test(hwnd, &*state, x, y) };
                unsafe { activate_zone(hwnd, &mut *state, zone, message == WM_LBUTTONDBLCLK) };
            }
            0
        }
        WM_TRIGGER_SEARCH => {
            if !state.is_null() {
                unsafe { start_search(hwnd, &mut *state) };
            }
            0
        }
        WM_SEARCH_DONE => {
            if !state.is_null() && lparam != 0 {
                let outcome = unsafe { *Box::from_raw(lparam as *mut SearchOutcome) };
                unsafe { finish_search(hwnd, &mut *state, outcome) };
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
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_NCDESTROY => {
            if !state.is_null() {
                let owned = unsafe { Box::from_raw(state) };
                unsafe {
                    delete_font(owned.font_body);
                    delete_font(owned.font_button);
                    delete_font(owned.font_small);
                    delete_font(owned.font_title);
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
        let instance = GetModuleHandleW(null());
        let class_name = wide(CLASS_NAME);
        let title = wide(APP_NAME);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hIcon: LoadIconW(instance, 1usize as *const u16),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: GetStockObject(WHITE_BRUSH) as HBRUSH,
            lpszClassName: class_name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return;
        }
        let state = Box::into_raw(Box::new(AppState::new()));
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1040,
            820,
            null_mut(),
            null_mut(),
            instance,
            state as *const _,
        );
        if hwnd.is_null() {
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
            } else if message.message == WM_KEYUP && message.wParam == VK_CONTROL as usize {
                control_down = false;
            } else if message.message == WM_KILLFOCUS {
                control_down = false;
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
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
