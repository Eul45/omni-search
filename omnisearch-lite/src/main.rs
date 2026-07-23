// ponytail: `let _ =` and `.ok()` are heavily used here for Win32 API calls and async ops. Skipped adding global error propagation because it adds noise for non-fatal OS events. Add real error handling when a specific system call actually fails in a way we need to recover from.
#![windows_subsystem = "windows"]

mod ai;
mod applog;
mod browser_indexer;
mod circle_to_search;

mod indexer;
mod launcher;
mod markdown;
mod search;
mod settings;
mod settings_startup;
mod settings_ui;
pub mod sync_server;

use search::{SearchEngine, SearchResult};
use std::os::windows::process::CommandExt;
use std::process::Command;
pub mod hotkey;

use std::ptr::null_mut;
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

#[link(name = "kernel32")]
extern "system" {
    fn GlobalFree(memory: HGLOBAL) -> HGLOBAL;
}

// ── Layout ────────────────────────────────────────────────────────────────────
const DEFAULT_WIN_W: i32 = 840;
const MIN_WIN_W: i32 = 560;
const SCREEN_EDGE_GAP: i32 = 32;
const MAX_RESULTS: usize = 300;

const PAD_L: i32 = 24;
const BADGE_W: i32 = 54;
const BADGE_H: i32 = 18;
const SEARCH_ICON_SIZE: i32 = 28;
const SEARCH_LENS_BUTTON_SIZE: i32 = 36;
const SEARCH_LENS_ICON_SIZE: i32 = 30;
const SEARCH_WEB_BUTTON_W: i32 = 124;
const SEARCH_ACTION_GAP: i32 = 8;
const RESULT_ICON_SIZE: i32 = 32;
const MAX_OWNED_ICON_CACHE: usize = 256;
const MAX_THUMBNAIL_CACHE: usize = 64;
// Agent icons (homepage Agents/Agent History + the agents:/agentchats: scoped views) draw
// larger than other result icons so the logo reads clearly instead of looking tiny.
const AGENT_ICON_SIZE: i32 = 40;
const RESULT_TEXT_BLOCK_H: i32 = 40;
const RESULT_TEXT_GAP: i32 = 12;
const CONTENT_HEADER_H: i32 = 80;
// Compact header band for single-label modes (homepage + scoped prefixes like `clip:`),
// which draw one section label instead of the filter row + results header. The old code
// reused CONTENT_HEADER_H (80px) here, leaving ~54px of dead space before the first row.
// ponytail: one constant + a mode-aware header_h() keeps paint, hit-test and sizing in sync.
const LABEL_HEADER_H: i32 = 38;
const HEIGHT_ANIM_MS: u128 = 90;
const WM_MOUSELEAVE: u32 = 0x02A3;

fn launcher_width_for_work_area(work_w: i32, desired_w: i32) -> i32 {
    let desired_w = if work_w < 1400 {
        desired_w.min(600).max(MIN_WIN_W)
    } else {
        desired_w.clamp(MIN_WIN_W, DEFAULT_WIN_W)
    };
    let available_w = (work_w - SCREEN_EDGE_GAP).max(1);
    desired_w.min(available_w).max(240).min(work_w.max(1))
}

fn centered_in_result_row(row_y: i32, height: i32, item_h: i32) -> i32 {
    row_y + (item_h - height) / 2
}

// ── Win32 IDs ─────────────────────────────────────────────────────────────────
const HOTKEY_ID: i32 = 1;
const TIMER_DEBOUNCE: usize = 1;
const TIMER_CURSOR_BLINK: usize = 2;
const TIMER_AI_ANIM: usize = 5;
const TIMER_ICON_BATCH: usize = 6;
const TIMER_SEARCH_ANIM: usize = 7;
const TIMER_CLIPBOARD_MSG: usize = 8;
const CURSOR_BLINK_MS: u32 = 530;
const WM_ICON_LOADED: u32 = WM_USER + 1;
const WM_ENGINE_READY: u32 = WM_USER + 2;
const WM_SEARCH_RESULTS: u32 = WM_USER + 3;
const WM_START_EDITING: u32 = WM_USER + 4;
const WM_REFRESH_SEARCH: u32 = WM_USER + 5;
const WM_AI_RESULT: u32 = WM_USER + 6;
// AI streaming output progress (lparam = boxed String).
const WM_AI_PROGRESS: u32 = WM_USER + 8;
const WM_TRAYICON: u32 = WM_USER + 9;
pub(crate) const WM_RELOAD_SETTINGS: u32 = WM_USER + 10;
pub(crate) const WM_SET_HOTKEY_RECORDING: u32 = WM_USER + 11;
pub(crate) const WM_LAUNCH_AGENT: u32 = WM_USER + 12;
const WM_OCR_RESULT: u32 = WM_USER + 13;
// Deferred post-animation actions: tick_window_animation runs with the caller's &mut
// State still live, so it posts these instead of calling ShowWindow/force_foreground
// directly - both can synchronously dispatch WM_ACTIVATE etc. back into this wndproc,
// which would derive its own &mut State from the same GWLP_USERDATA pointer.
const WM_ANIM_FOREGROUND: u32 = WM_USER + 14;
const WM_ANIM_HIDE_WINDOW: u32 = WM_USER + 15;

fn clipboard_ocr_empty_message() -> &'static str {
    "No readable text found in the clipboard image"
}

fn run_clipboard_ocr<T>(job: impl FnOnce() -> T) -> T {
    // ponytail: one global OCR lane is enough for clipboard images; upgrade to a real worker
    // queue if we need progress/cancellation for many long-running image imports.
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let guard = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = job();
    drop(guard);
    result
}

unsafe fn setup_tray_icon(
    hwnd: windows::Win32::Foundation::HWND,
    _hinst: windows::Win32::Foundation::HMODULE,
) {
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NOTIFYICONDATAW,
    };

    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAYICON;
    let hicon =
        unsafe { load_png_to_hicon(include_bytes!("../icons/OmniSearchTrans_16.png"), 16) };
    nid.hIcon = hicon;
    let tip = "OmniSearch Lite".encode_utf16().collect::<Vec<u16>>();
    for (i, &c) in tip.iter().enumerate().take(127) {
        nid.szTip[i] = c;
    }
    let res = Shell_NotifyIconW(NIM_ADD, &nid);
    if !res.as_bool() {
        applog::log("setup_tray_icon: NIM_ADD failed");
    }
}

unsafe fn remove_tray_icon(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

// AI answer panel height (below the search bar) when showing an AI response.
const AI_PANEL_H: i32 = 360;

#[derive(Clone)]
struct SearchRequest {
    query: String,
    query_id: usize,
}
// ── Animation ─────────────────────────────────────────────────────────────────
// const ANIM_TICK_MS: u32 = 1;
const ANIM_DURATION_SEC: f32 = 0.100; // 220ms
                                      // const MAX_ALPHA: u8 = 255;

// ── Genie Morph Dimensions ────────────────────────────────────────────────────
// const PILL_H: i32 = 12; // Starting height at top center

// ── Colors (COLORREF = 0x00BBGGRR) ───────────────────────────────────────────
// const s.theme.palette().bg: COLORREF ...
// const s.theme.palette().bg_sel: COLORREF ...
// const s.theme.palette().clr_div: COLORREF ...
// const s.theme.palette().clr_white: COLORREF ...
// const s.theme.palette().clr_gray: COLORREF ...
// const s.theme.palette().clr_ph: COLORREF ...
// const s.theme.palette().clr_bdgbg: COLORREF ...
// const s.theme.palette().clr_bdgtx: COLORREF ...
// const s.theme.palette().clr_accent: COLORREF ...
const COLOR_KEY: COLORREF = COLORREF(0x00_12_34_56);

#[derive(Debug, Clone, PartialEq)]
enum FormState {
    None,
    CreateSnippetName,
    CreateSnippetContent { name: String },
    CreateSnippetKeyword { name: String, content: String },
    CreateQuicklinkName,
    CreateQuicklinkUrl { name: String },
    CreateQuicklinkKeyword { name: String, url: String },
    CreateFocusCategoryName,
    CreateFocusCategoryBlocked { name: String },
    CreateNoteName,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum FilterType {
    All,
    Files,
    Folders,
    Images,
    Code,
    Settings,
    Commands,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Theme {
    Omni,
    Darker,
    NordDarker,
    Light,
}

#[derive(Debug, Clone, Copy)]
struct ThemePalette {
    bg: COLORREF,
    bg_sel: COLORREF,
    bg_hover: COLORREF,
    clr_div: COLORREF,
    clr_white: COLORREF,
    clr_gray: COLORREF,
    clr_gray_sel: COLORREF,
    clr_ph: COLORREF,
    clr_bdgbg: COLORREF,
    clr_bdgtx: COLORREF,
    clr_accent: COLORREF,
    bg_footer: COLORREF,
    scrollbar_track: COLORREF,
    scrollbar_thumb: COLORREF,
}

impl Theme {
    fn palette(self) -> ThemePalette {
        match self {
            Theme::Omni => ThemePalette {
                bg: COLORREF(0x00_2C_24_1F),           // #1f242c
                bg_sel: COLORREF(0x00_3F_33_2B),       // #2b333f
                bg_hover: COLORREF(0x00_37_2E_27),     // #272e37
                clr_div: COLORREF(0x00_32_2B_26),      // #262b32
                clr_white: COLORREF(0x00_FB_F5_F1),    // #f1f5fb
                clr_gray: COLORREF(0x00_C6_B4_AA),     // #aab4c6
                clr_gray_sel: COLORREF(0x00_D2_C7_C0),
                clr_ph: COLORREF(0x00_C6_B4_AA),
                clr_bdgbg: COLORREF(0x00_3F_33_2B),
                clr_bdgtx: COLORREF(0x00_FB_F5_F1),
                clr_accent: COLORREF(0x00_FF_B3_7A),   // #7ab3ff (light blue accent)
                bg_footer: COLORREF(0x00_22_1B_17),    // #171b22
                scrollbar_track: COLORREF(0x00_27_20_1C),
                scrollbar_thumb: COLORREF(0x00_56_47_3F),
            },
            Theme::Darker => ThemePalette {
                bg: COLORREF(0x00_2F_2F_2F),
                bg_sel: COLORREF(0x00_4D_4D_4D),
                bg_hover: COLORREF(0x00_38_38_38),
                clr_div: COLORREF(0x00_3B_3B_3B),
                clr_white: COLORREF(0x00_FF_FF_FF),
                clr_gray: COLORREF(0x00_8F_8F_8F),
                clr_gray_sel: COLORREF(0x00_8F_8F_8F),
                clr_ph: COLORREF(0x00_8F_8F_8F),
                clr_bdgbg: COLORREF(0x00_3B_3B_3B),
                clr_bdgtx: COLORREF(0x00_FF_FF_FF),
                clr_accent: COLORREF(0x00_E5_99_4C),
                // Neutral footer shade matching the gray bg (#2f2f2f). Was #191d23, a
                // navy leftover from an older accent theme that clashed with the new
                // neutral dark theme — this is the "clipboard guide" mismatch.
                bg_footer: COLORREF(0x00_26_26_26),
                scrollbar_track: COLORREF(0x00_28_28_28),
                scrollbar_thumb: COLORREF(0x00_4D_4D_4D),
            },
            Theme::NordDarker => ThemePalette {
                bg: COLORREF(0x00_40_34_2E),           // 2e3440
                bg_sel: COLORREF(0x00_6B_58_4E),       // 4e586b
                bg_hover: COLORREF(0x00_52_42_3B),     // 3b4252
                clr_div: COLORREF(0x00_6A_56_4C),      // 4c566a
                clr_white: COLORREF(0x00_F0_E9_E5),    // e5e9f0
                clr_gray: COLORREF(0x00_83_6C_60),     // 606c83
                clr_gray_sel: COLORREF(0x00_AB_91_83), // 8391ab
                clr_ph: COLORREF(0x00_6B_58_4E),
                clr_bdgbg: COLORREF(0x00_52_42_3B),
                clr_bdgtx: COLORREF(0x00_F0_E9_E5),
                clr_accent: COLORREF(0x00_AB_91_83),
                bg_footer: COLORREF(0x00_33_29_24), // nord darker footer shade (#242933)
                scrollbar_track: COLORREF(0x00_2D_26_22), // original Nord scrollbar track (#22262d)
                scrollbar_thumb: COLORREF(0x00_70_62_58), // original Nord scrollbar thumb (#586270)
            },
            Theme::Light => ThemePalette {
                bg: COLORREF(0x00_FA_FA_FA),
                bg_sel: COLORREF(0x00_E5_E5_E5),
                bg_hover: COLORREF(0x00_F0_F0_F0),
                clr_div: COLORREF(0x00_E5_E5_E5),
                clr_white: COLORREF(0x00_1B_1B_1B),
                clr_gray: COLORREF(0x00_81_81_81),
                clr_gray_sel: COLORREF(0x00_72_76_7D),
                clr_ph: COLORREF(0x00_81_81_81),
                clr_bdgbg: COLORREF(0x00_E5_E5_E5),
                clr_bdgtx: COLORREF(0x00_1B_1B_1B),
                clr_accent: COLORREF(0x00_D7_78_00),
                bg_footer: COLORREF(0x00_F2_F2_F2),
                scrollbar_track: COLORREF(0x00_F0_F0_F0),
                scrollbar_thumb: COLORREF(0x00_C0_C0_C0),
            },
        }
    }
}

fn theme_from_setting(value: &str) -> Theme {
    match value {
        "Light" => Theme::Light,
        "NordDarker" => Theme::NordDarker,
        "Darker" => Theme::Darker,
        "Omni" => Theme::Omni,
        _ => Theme::Omni,
    }
}

// ── App state ─────────────────────────────────────────────────────────────────
struct State {
    app_settings: crate::settings::AppSettings,
    theme: Theme,
    hovered_item: Option<usize>,
    mouse_tracking: bool,
    search_tx: Option<std::sync::mpsc::Sender<SearchRequest>>,
    // Separate channel to the slow (content/FTS) worker thread so the fast file/folder worker is
    // never blocked behind a multi-second content search.
    search_tx_slow: Option<std::sync::mpsc::Sender<SearchRequest>>,
    icon_tx: Option<std::sync::mpsc::Sender<IconRequest>>,
    current_query_id: usize,
    db_path: std::path::PathBuf,
    query: String,
    cursor_pos: usize,
    // High half of a UTF-16 surrogate pair from WM_CHAR, waiting for its low half.
    // Windows delivers astral-plane chars (emoji) as two separate WM_CHAR messages.
    pending_surrogate: Option<u16>,
    search_input_active: bool,
    chat_input: String,
    chat_cursor_pos: usize,
    chat_input_active: bool,
    results: Vec<SearchResult>,
    results_stale: bool,
    selected: usize,
    anim: Anim,
    launcher_w: i32,
    cx: i32,
    cy: i32,
    font_q: HFONT,
    font_n: HFONT,
    font_c: HFONT,
    font_b: HFONT,
    font_code: HFONT, // monospace for inline code / code blocks
    font_h: HFONT,    // bold larger font for markdown headings
    icon_settings: HICON,
    icon_web: HICON,
    icon_web_action: HICON,
    icon_chatgpt: HICON,
    icon_claude: HICON,
    icon_gemini: HICON,
    icon_grok: HICON,
    icon_eyux: HICON,
    icon_calculator: HICON,
    icon_color_picker: HICON,
    icon_lens: HICON,
    icon_phone_connect: HICON,
    icon_bookmark: HICON,
    icon_folder: HICON,
    icon_file: HICON,
    icon_app: HICON,
    icon_commit: HICON,
    icon_todo: HICON,
    icon_agent: HICON,
    icon_agent_chat: HICON,
    icon_clipboard: HICON,
    icon_memory: HICON,
    icon_chrome: HICON,
    icon_firefox: HICON,
    icon_edge: HICON,
    icon_brave: HICON,

    icon_new_search: HICON,

    active_filter: FilterType,
    hovered_filter: Option<FilterType>,
    hovered_search_action: Option<SearchActionHover>,
    filter_counts: [usize; 7],
    filter_scroll_x: i32,
    sort_asc: bool,
    text_selected: bool,
    cursor_visible: bool,
    // Homepage highlight to restore when returning to the homepage (Escape / cleared query),
    // so we land back on the item the user last visited instead of a fixed default.
    homepage_sel: usize,
    // Rect of the search caret as last painted — lets the blink timer repaint just the caret
    // instead of the whole window (a full blit on the layered surface flickers the cursor).
    caret_rect: std::cell::Cell<RECT>,
    scroll_offset: usize,
    last_mouse_x: i32,
    last_mouse_y: i32,
    app_icons: std::collections::HashMap<String, HICON>,
    app_icons_owned: std::collections::HashSet<String>,
    pending_icons: std::collections::HashSet<String>,
    clipboard_thumbnails: std::cell::RefCell<std::collections::HashMap<String, HBITMAP>>,
    selected_clip_ids: std::collections::HashSet<String>,
    delete_confirm: bool,
    editing_item: Option<String>,
    submenu_active: bool,
    submenu_selected: usize,
    image_preview_active: bool,
    hwnd_preview: Option<HWND>,
    // In-app note editor (self-rendered — no child control, which broke on the layered window)
    note_editing: bool,
    note_text: String,
    note_path: Option<String>,
    note_scroll: i32,
    // Explain Results: launch_command -> "why it surfaced" tag (e.g. "2h ago"),
    // computed when results arrive so paint does no file I/O.
    result_reasons: std::collections::HashMap<String, String>,
    form_state: FormState, // Phase 2 Quicklinks & Snippets creation form state
    color_picker_active: bool,
    color_picker_mx: i32,
    color_picker_my: i32,
    prev_foreground: HWND, // Window that had focus before launcher appeared (for snippet auto-paste)
    taskbar_shown_by_app: bool,
    // AI answer panel
    ai_pending: bool,                        // true while waiting on the AI response
    ai_answer: Option<String>,               // the response text to render
    ai_title: String,                        // command label shown above the answer
    ai_scroll: i32,                          // vertical pixel scroll offset in the answer panel
    ai_follow_bottom: bool, // true = keep the latest message pinned to the bottom (auto-scroll)
    ai_content_height: std::cell::Cell<i32>, // cached total rendered AI height (for max_scroll in input handlers)
    ai_view_height: std::cell::Cell<i32>, // cached viewport height (content_bottom - content_top)
    ai_tick: u32,                         // lightweight activity indicator while AI is running
    active_chat_id: Option<i64>,          // persistent chat thread ID in ai_chats table

    unfiltered_results: Vec<SearchResult>,
    search_loading: bool,
    search_anim_tick: usize,
    shown_h: i32,
    target_h: i32,
    height_anim_from: i32,
    height_anim_started: std::time::Instant,
    placeholder_override: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchActionHover {
    Web,
    Lens,
    Settings,
}

#[derive(PartialEq)]
enum Anim {
    Hidden,
    Appearing {
        start_time: std::time::Instant,
        start_p: f32,
    },
    Visible,
    Hiding {
        start_time: std::time::Instant,
        start_p: f32,
    },
}

#[derive(Clone, Copy)]
struct SendHwnd(HWND);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

struct IconRequest {
    key: String,
    source: String,
}

impl State {
    fn search_h(&self) -> i32 {
        self.app_settings.search_bar_height as i32
    }
    fn item_h(&self) -> i32 {
        self.app_settings.item_height as i32
    }
    fn visible_results(&self) -> usize {
        let work_h = if self.cy * 2 < 400 {
            400
        } else if self.cy == unsafe { GetSystemMetrics(SM_CYSCREEN) } / 3 {
            self.cy * 3 - 48
        } else {
            self.cy * 2
        };
        let search_h = self.search_h();
        let item_h = self.item_h();
        let limit = work_h - 189 - search_h;
        if limit <= 0 {
            return 3;
        }
        let calculated = (limit / item_h) as usize;
        if work_h < 800 {
            calculated.clamp(3, 5)
        } else {
            calculated.clamp(3, 8)
        }
    }
    fn launcher_x(&self, client_w: i32) -> i32 {
        (client_w - self.launcher_w) / 2
    }
    fn has_prefix(&self) -> bool {
        let q = self.query.to_lowercase();
        q.starts_with("bookmarks:")
            || q.starts_with("history:")
            || q.starts_with("commits:")
            || q.starts_with("todos:")
            || q.starts_with("clip:")
            || q.starts_with("clipboard:")
            || q.starts_with("file:")
            || q.starts_with("folder:")
            || q.starts_with("code:")
            || q.starts_with("img:")
            || q.starts_with("agents:")
            || q.starts_with("agentchats:")
            || q.starts_with("chatgpt:")
            || q.starts_with("claude:")
            || q.starts_with("gemini:")
            || q.starts_with("grok:")
            || q.starts_with("eyux:")
    }

    fn shows_web_search_action(&self) -> bool {
        !self.query.trim().is_empty()
            && !self.has_prefix()
            && self.form_state == FormState::None
            && !self.ai_pending
            && self.ai_answer.is_none()
            && !self.chat_input_active
    }

    fn shows_lens_action(&self) -> bool {
        self.app_settings.plugin_circle_search
            && self.query.trim().is_empty()
            && self.form_state == FormState::None
            && !self.ai_pending
            && self.ai_answer.is_none()
            && !self.chat_input_active
    }

    fn reset_results(&mut self) {
        if self.query.is_empty() {
            self.active_filter = FilterType::All;
            self.results = default_homepage_results();
            self.results_stale = false;
            self.selected = self.homepage_sel.min(self.results.len().saturating_sub(1));
            self.scroll_offset = 0;
        } else {
            self.results.clear();
        }
    }

    fn shows_guidance_footer(&self) -> bool {
        false
    }

    fn target_win_h(&self) -> i32 {
        if self.note_editing {
            return self.search_h() + 1 + AI_PANEL_H;
        }
        if self.ai_pending || self.ai_answer.is_some() || self.chat_input_active {
            return self.search_h() + 1 + 600;
        }
        if self.form_state != FormState::None {
            return self.search_h() + 24;
        }
        let vr = self.visible_results();
        if self.query.is_empty() {
            return homepage_win_h(self.search_h(), self.item_h(), self.results.len(), vr);
        }
        if self.has_prefix() {
            scoped_results_win_h(self.search_h(), self.item_h(), self.results.len(), vr)
        } else {
            normal_search_win_h(self.search_h(), self.item_h(), self.results.len(), vr)
        }
    }
    fn paint_win_h(&self) -> i32 {
        match self.anim {
            Anim::Visible => self.shown_h.max(self.search_h()),
            _ => self.target_win_h(),
        }
    }
    fn launcher_top_y(&self) -> i32 {
        launcher_top_y(self.cy, self.paint_win_h())
    }
    // Height of the band between the search bar and the first result row. Homepage and
    // scoped prefixes draw a single compact label; normal search draws the taller filter
    // row + results header. Must match the paint sites and the *_win_h() helpers.
    fn header_h(&self) -> i32 {
        if self.query.is_empty() || self.has_prefix() {
            LABEL_HEADER_H
        } else {
            CONTENT_HEADER_H
        }
    }
    fn result_row_y(&self, i: usize) -> i32 {
        let end_y = self.launcher_top_y();
        let cur_y = end_y + self.search_h() + 1 + self.header_h();
        cur_y + i as i32 * self.item_h()
    }
    fn result_rect(&self, i: usize) -> RECT {
        let y = self.result_row_y(i);
        RECT {
            left: 0,
            top: y,
            right: self.launcher_w,
            bottom: y + self.item_h(),
        }
    }
    fn current_p(&self) -> f32 {
        match self.anim {
            Anim::Hidden => 0.0,
            Anim::Visible => 1.0,
            Anim::Appearing {
                start_time,
                start_p,
            } => {
                let elapsed = start_time.elapsed().as_secs_f32();
                (start_p + elapsed / ANIM_DURATION_SEC).min(1.0)
            }
            Anim::Hiding {
                start_time,
                start_p,
            } => {
                let elapsed = start_time.elapsed().as_secs_f32();
                (start_p - elapsed / ANIM_DURATION_SEC).max(0.0)
            }
        }
    }
}

/// A named mutex handle that calls CloseHandle on drop.
struct MutexHandle(windows::Win32::Foundation::HANDLE);
impl Drop for MutexHandle {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

fn enforce_single_instance() -> Option<MutexHandle> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
    use windows::Win32::Foundation::WPARAM;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_COMMAND};

    unsafe {
        let name: Vec<u16> = "Local\\OmniSearchLiteInstanceMutex\0"
            .encode_utf16()
            .collect();
        let handle = CreateMutexW(None, true, PCWSTR(name.as_ptr()));
        if let Ok(h) = handle {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let class_name: Vec<u16> = "omnisearch-lite\0".encode_utf16().collect();
                if let Ok(hwnd) = FindWindowW(PCWSTR(class_name.as_ptr()), None) {
                    if !hwnd.0.is_null() {
                        let _ = PostMessageW(hwnd, WM_COMMAND, WPARAM(1), LPARAM(0));
                    }
                }
                let _ = windows::Win32::Foundation::CloseHandle(h);
                return None;
            }
            return Some(MutexHandle(h));
        }
    }
    None
}

// ── Entry point ───────────────────────────────────────────────────────────────
fn main() {
    migrate_legacy_appdata();
    install_panic_logger();

    // Clean up .bak files from previous updates on startup
    if let Ok(current_exe) = std::env::current_exe() {
        let backup_exe = current_exe.with_extension("bak");
        if backup_exe.exists() {
            let _ = std::fs::remove_file(backup_exe);
        }
    }

    let args: Vec<String> = std::env::args().collect();
    // Out-of-process document extraction: run pdf_extract/docx_lite in a throwaway child so
    // a stack overflow on a malformed file (an abort that catch_unwind CANNOT catch) kills
    // only this child, never the launcher. See indexer::extract_via_subprocess.
    if let Some(pos) = args.iter().position(|a| a == "--extract-content") {
        if let Some(path) = args.get(pos + 1) {
            indexer::extract_content_subprocess(path);
        }
        return;
    }
    if args.iter().any(|arg| arg == "--settings") {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED
                    | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
            );
        }
        settings_ui::run_settings_window();
        return;
    }

    let _mutex = match enforce_single_instance() {
        Some(m) => m,
        None => return,
    };

    let first_settings_run = !crate::settings::AppSettings::get_settings_path().exists();
    let startup_settings = crate::settings::AppSettings::load();
    crate::settings_startup::sync_run_on_startup(startup_settings.run_on_startup);
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED
                | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
        );
    }

    unsafe {
        run(first_settings_run);
    }
}

/// Install a global panic hook that appends the panic message, source location and a
/// backtrace to %APPDATA%/omnisearch-lite/panic.log. This binary is a windowed app with no
/// console, so a panic otherwise produces only an opaque 0xC000041D exit with no message.
/// The hook itself never panics (every fallible call is ignored), so it can't recurse.
/// One-time migration for the OmniSearch -> OmniSearch Lite rebrand: if the old
/// %APPDATA%\omnisearch folder exists and the new one doesn't yet, move it so the search
/// index, settings, clipboard history, snippets, and AI config carry over instead of every
/// upgrading user starting fresh. Runs first, before anything else touches the data dir.
fn migrate_legacy_appdata() {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let base = std::path::Path::new(&appdata);
        let old_dir = base.join("omnisearch");
        let old_dir_proton = base.join("protonsearch");
        let new_dir = base.join("omnisearch-lite");
        if !new_dir.exists() {
            if old_dir_proton.exists() {
                let _ = std::fs::rename(&old_dir_proton, &new_dir);
            } else if old_dir.exists() {
                let _ = std::fs::rename(&old_dir, &new_dir);
            }
        }
    }
}

fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bt = std::backtrace::Backtrace::force_capture();
        let entry = format!(
            "\n==== PANIC (epoch {secs}) ====\nthread: {thread}\nlocation: {loc}\nmessage: {msg}\nbacktrace:\n{bt}\n"
        );
        if let Ok(appdata) = std::env::var("APPDATA") {
            let dir = std::path::Path::new(&appdata).join("omnisearch-lite");
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("panic.log"))
            {
                let _ = f.write_all(entry.as_bytes());
            }
        }
    }));
}

unsafe fn create_gdi_font(family: &str, size_px: i32, weight_str: &str) -> HFONT {
    let face: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
    let weight = match weight_str {
        "Bold" => 700,
        "Semi-Bold" => 600,
        "Medium" => 500,
        _ => 400,
    };
    CreateFontW(
        -size_px,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
        PCWSTR(face.as_ptr()),
    )
}

unsafe fn run(first_settings_run: bool) {
    let hinst = GetModuleHandleW(PCWSTR::null()).unwrap();
    let face: Vec<u16> = "Segoe UI Variable\0".encode_utf16().collect();
    let fp = PCWSTR(face.as_ptr());

    // CreateFontW takes u32 for the font attribute params in windows 0.58.
    let mk_font = |h, w| {
        CreateFontW(
            h,
            0,
            0,
            0,
            w,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
            fp,
        )
    };

    // Monospace + bold fonts for markdown code blocks and headings in the AI panel.
    let mono_face: Vec<u16> = "Consolas\0".encode_utf16().collect();
    let font_code = CreateFontW(
        -15,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (FIXED_PITCH.0 | FF_MODERN.0) as u32,
        PCWSTR(mono_face.as_ptr()),
    );
    // Heading font: slightly larger, bold, same Segoe UI Variable face.
    let font_h = mk_font(-22, 700);

    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);

    let db_path = match std::env::var("APPDATA") {
        Ok(d) => {
            let path = std::path::PathBuf::from(d).join("omnisearch-lite");
            let _ = std::fs::create_dir_all(&path);
            path.join("file_index.db")
        }
        Err(_) => std::path::PathBuf::from("file_index.db"),
    };

    // Ensure agents table exists and has a default AI agent
    ensure_default_agents(&db_path);

    // Load the custom Settings icon from PNG at compile time.
    let icon_settings = unsafe {
        load_png_to_hicon(
            include_bytes!("../icons/settings.png"),
            RESULT_ICON_SIZE as u32,
        )
    };
    let icon_web = load_png_to_hicon(
        include_bytes!("../icons/google.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_web_action = load_png_to_hicon(include_bytes!("../icons/google.png"), 20);
    let icon_chatgpt = load_png_to_hicon(
        include_bytes!("../icons/chatgptlogo.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_claude = load_png_to_hicon(
        include_bytes!("../icons/claude.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_gemini = load_png_to_hicon(
        include_bytes!("../icons/gemini.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_grok = load_png_to_hicon(
        include_bytes!("../icons/grok.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_eyux = load_png_to_hicon(
        include_bytes!("../icons/eyux.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_calculator = {
        let mut h = unsafe { get_file_icon("C:\\Windows\\System32\\calc.exe") };
        if h.0.is_null() {
            h = unsafe { get_app_icon("C:\\Windows\\System32\\calc.exe") };
        }
        if h.0.is_null() {
            h = load_icon_from_dll("shell32.dll", 23, 64);
        }
        h
    };
    let icon_color_picker = load_png_to_hicon(
        include_bytes!("../icons/color-picker.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_lens = load_png_to_hicon(
        include_bytes!("../icons/lens.png"),
        SEARCH_LENS_ICON_SIZE as u32,
    );
    let icon_phone_connect = load_png_to_hicon(
        include_bytes!("../icons/connect-to-phone.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_bookmark = load_png_to_hicon(
        include_bytes!("../icons/fav.png"),
        RESULT_ICON_SIZE as u32,
    );
    let icon_folder = load_icon_from_dll("shell32.dll", 3, 64);
    let icon_file = load_icon_from_dll("shell32.dll", 0, 64);
    let icon_app = load_icon_from_dll("shell32.dll", 2, 64);
    let icon_commit = load_icon_from_dll("shell32.dll", 22, 64);
    let icon_todo = load_icon_from_dll("shell32.dll", 270, 64);
    let icon_clipboard = load_icon_from_dll("shell32.dll", 260, 64);
    let icon_memory = load_icon_from_dll("shell32.dll", 238, 64);
    let icon_agent = load_png_to_hicon(
        include_bytes!("../icons/AgentLogo.png"),
        AGENT_ICON_SIZE as u32,
    );
    let icon_agent_chat = load_png_to_hicon(
        include_bytes!("../icons/AgentMessageIcon.png"),
        AGENT_ICON_SIZE as u32,
    );

    // Load at exactly the draw size (from 256² sources via Lanczos) so DrawIconEx never rescales
    // the HICON at paint time — a 36→32 GDI rescale is what made these look soft/low-res.
    let icon_new_search = load_png_to_hicon(
        include_bytes!("../launcher_source_icons/search.png"),
        SEARCH_ICON_SIZE as u32,
    );

    let icon_chrome = {
        let mut h = HICON(std::ptr::null_mut());
        if let Some(path) = get_registered_app_path("chrome.exe") {
            h = unsafe { get_file_icon(&path) };
            if h.0.is_null() {
                h = unsafe { get_app_icon(&path) };
            }
        }
        if h.0.is_null() {
            h = unsafe {
                get_file_icon("C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe")
            };
        }
        if h.0.is_null() {
            h = unsafe {
                get_file_icon("C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe")
            };
        }
        h
    };
    let icon_firefox = {
        let mut h = HICON(std::ptr::null_mut());
        if let Some(path) = get_registered_app_path("firefox.exe") {
            h = unsafe { get_file_icon(&path) };
            if h.0.is_null() {
                h = unsafe { get_app_icon(&path) };
            }
        }
        if h.0.is_null() {
            h = unsafe { get_file_icon("C:\\Program Files\\Mozilla Firefox\\firefox.exe") };
        }
        h
    };
    let icon_edge = {
        let mut h = HICON(std::ptr::null_mut());
        if let Some(path) = get_registered_app_path("msedge.exe") {
            h = unsafe { get_file_icon(&path) };
            if h.0.is_null() {
                h = unsafe { get_app_icon(&path) };
            }
        }
        if h.0.is_null() {
            h = unsafe {
                get_file_icon("C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe")
            };
        }
        h
    };
    let icon_brave = {
        let mut h = HICON(std::ptr::null_mut());
        if let Some(path) = get_registered_app_path("brave.exe") {
            h = unsafe { get_file_icon(&path) };
            if h.0.is_null() {
                h = unsafe { get_app_icon(&path) };
            }
        }
        if h.0.is_null() {
            h = unsafe {
                get_file_icon(
                    "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
                )
            };
        }
        if h.0.is_null() {
            h = unsafe {
                get_file_icon(
                    "C:\\Program Files (x86)\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
                )
            };
        }
        h
    };

    let (icon_tx, icon_rx) = std::sync::mpsc::channel::<IconRequest>();

    let app_settings = crate::settings::AppSettings::load();
    let theme = theme_from_setting(&app_settings.theme_mode);
    let font_q = create_gdi_font(
        &app_settings.query_font_family,
        app_settings.query_font_size as i32,
        &app_settings.query_font_weight,
    );
    let font_n = create_gdi_font(
        &app_settings.result_title_font_family,
        app_settings.result_title_font_size as i32,
        &app_settings.result_title_font_weight,
    );
    let font_c = create_gdi_font(
        &app_settings.result_subtitle_font_family,
        app_settings.result_subtitle_font_size as i32,
        &app_settings.result_subtitle_font_weight,
    );

    let initial_launcher_w = launcher_width_for_work_area(sw, app_settings.window_width as i32);
    let state = Box::new(State {
        app_settings,
        theme,
        hovered_item: None,
        mouse_tracking: false,
        search_tx: None,
        search_tx_slow: None,
        icon_tx: Some(icon_tx),
        current_query_id: 0,
        db_path: db_path.clone(),
        query: String::new(),
        cursor_pos: 0,
        pending_surrogate: None,
        search_input_active: true,
        chat_input: String::new(),
        chat_cursor_pos: 0,
        chat_input_active: false,
        results: default_homepage_results(),
        results_stale: false,
        selected: 2,
        anim: Anim::Hidden,
        launcher_w: initial_launcher_w,
        cx: sw / 2,
        cy: sh / 3,
        font_q,
        font_n,
        font_c,
        font_b: mk_font(-13, 600),
        font_code,
        font_h,
        icon_settings,
        icon_web,
        icon_web_action,
        icon_chatgpt,
        icon_claude,
        icon_gemini,
        icon_grok,
        icon_eyux,
        icon_calculator,
        icon_color_picker,
        icon_lens,
        icon_phone_connect,
        icon_bookmark,
        icon_folder,
        icon_file,
        icon_app,
        icon_commit,
        icon_todo,
        icon_agent,
        icon_agent_chat,
        icon_clipboard,
        icon_memory,
        icon_chrome,
        icon_firefox,
        icon_edge,
        icon_brave,
        icon_new_search,
        active_filter: FilterType::All,
        hovered_filter: None,
        hovered_search_action: None,
        filter_counts: [0; 7],
        filter_scroll_x: 0,
        sort_asc: false,
        text_selected: false,
        cursor_visible: true,
        homepage_sel: 2,
        caret_rect: std::cell::Cell::new(RECT::default()),
        scroll_offset: 0,
        last_mouse_x: -1,
        last_mouse_y: -1,
        app_icons: std::collections::HashMap::new(),
        app_icons_owned: std::collections::HashSet::new(),
        pending_icons: std::collections::HashSet::new(),
        clipboard_thumbnails: std::cell::RefCell::new(std::collections::HashMap::new()),
        selected_clip_ids: std::collections::HashSet::new(),
        delete_confirm: false,
        editing_item: None,
        submenu_active: false,
        submenu_selected: 0,
        image_preview_active: false,
        hwnd_preview: None,
        note_editing: false,
        note_text: String::new(),
        note_path: None,
        note_scroll: 0,
        result_reasons: std::collections::HashMap::new(),
        form_state: FormState::None,
        color_picker_active: false,
        color_picker_mx: 0,
        color_picker_my: 0,
        prev_foreground: HWND(null_mut()),
        taskbar_shown_by_app: false,
        ai_pending: false,
        ai_answer: None,
        ai_title: String::new(),
        ai_scroll: 0,
        ai_follow_bottom: true,
        ai_content_height: std::cell::Cell::new(0),
        ai_view_height: std::cell::Cell::new(0),
        ai_tick: 0,
        active_chat_id: None,

        unfiltered_results: default_homepage_results(),
        search_loading: false,
        search_anim_tick: 0,
        shown_h: homepage_win_h(60, 68, 8, 8),
        target_h: homepage_win_h(60, 68, 8, 8),
        height_anim_from: homepage_win_h(60, 68, 8, 8),
        height_anim_started: std::time::Instant::now(),
        placeholder_override: None,
    });



     let icon_main =
        unsafe { load_png_to_hicon(include_bytes!("../icons/OmniSearchTrans_32.png"), 32) };
    let icon_main_sm =
        unsafe { load_png_to_hicon(include_bytes!("../icons/OmniSearchTrans_16.png"), 16) };

    let class: Vec<u16> = "omnisearch-lite\0".encode_utf16().collect();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst.into(),
        hIcon: icon_main,
        hIconSm: icon_main_sm,
        hCursor: LoadCursorW(HINSTANCE(null_mut()), IDC_ARROW).unwrap(),
        hbrBackground: HBRUSH(null_mut()),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);

    let preview_class: Vec<u16> = "omnisearch-lite-preview\0".encode_utf16().collect();
    let wc_preview = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(preview_wnd_proc),
        hInstance: hinst.into(),
        hIcon: icon_main,
        hIconSm: icon_main_sm,
        hCursor: LoadCursorW(HINSTANCE(null_mut()), IDC_ARROW).unwrap(),
        hbrBackground: HBRUSH(null_mut()),
        lpszClassName: PCWSTR(preview_class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc_preview);

    let sw = GetSystemMetrics(SM_CXSCREEN);
    let win_x = (sw - initial_launcher_w) / 2;
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        win_x,
        0,
        initial_launcher_w,
        800,
        HWND(null_mut()),
        HMENU(null_mut()),
        hinst,
        Some(Box::into_raw(state) as _),
    )
    .unwrap();

    setup_tray_icon(hwnd, hinst);

    // Show the launcher window immediately on startup instead of starting hidden
    let s_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if !s_ptr.is_null() {
        do_show(hwnd, &mut *s_ptr);
    }

    let hwnd_icon = SendHwnd(hwnd);

    // Icon loading thread with auto-restart on panic.
    // If the COM-heavy icon loading code panics (common with shell extensions),
    // the thread would die silently and all future icons would stop loading.
    // This wrapper catches panics, logs them, and restarts the thread.
    fn spawn_icon_loader(hwnd: SendHwnd, icon_rx: std::sync::mpsc::Receiver<IconRequest>) {
        std::thread::spawn(move || {
            // Rebind the whole wrapper: edition-2021 disjoint capture would otherwise
            // capture only the raw (non-Send) HWND field out of `hwnd.0` uses below.
            let hwnd = hwnd;
            loop {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let hwnd_raw = SendHwnd(hwnd.0);
                    let _ = unsafe {
                        windows::Win32::System::Com::CoInitializeEx(
                            None,
                            windows::Win32::System::Com::COINIT_APARTMENTTHREADED
                                | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
                        )
                    };
                    while let Ok(req) = icon_rx.recv() {
                        unsafe {
                            let file_icon_path = icon_file_path(&req.source, &req.key);
                            let hicon = if let Some(path) = file_icon_path {
                                get_file_icon(&path)
                            } else if req.source == "ACTION" && req.key.starts_with("kill:") {
                                let pid_str = req.key.strip_prefix("kill:").unwrap_or("");
                                if let Ok(pid) = pid_str.parse::<u32>() {
                                    if let Some(path) = get_process_path(pid) {
                                        get_app_icon(&path)
                                    } else {
                                        get_app_icon("C:\\Windows\\System32\\cmd.exe")
                                    }
                                } else {
                                    HICON(std::ptr::null_mut())
                                }
                            } else {
                                get_app_icon(&req.key)
                            };
                            let key_ptr = Box::into_raw(Box::new(req.key));
                            if PostMessageW(
                                hwnd_raw.0,
                                WM_ICON_LOADED,
                                WPARAM(hicon.0 as usize),
                                LPARAM(key_ptr as isize),
                            )
                            .is_err()
                            {
                                let _ = Box::from_raw(key_ptr);
                                if !hicon.0.is_null() {
                                    let _ = DestroyIcon(hicon);
                                }
                            }
                        }
                    }
                }));

                if let Err(payload) = result {
                    // Thread panicked - log and restart after a short delay
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        format!("icon thread panicked: {}", s)
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        format!("icon thread panicked: {}", s)
                    } else {
                        "icon thread panicked with unknown payload".to_string()
                    };
                    applog::log(&msg);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    // Loop continues - thread restarts with same channel
                } else {
                    // Channel closed (sender dropped) - normal shutdown
                    break;
                }
            }
        });
    }

    spawn_icon_loader(hwnd_icon, icon_rx);



    SetLayeredWindowAttributes(hwnd, COLOR_KEY, 255, LWA_COLORKEY | LWA_ALPHA).unwrap();

    // DWM rounded corners (Windows 11) - Do not round the transparent box
    let corner = DWMWCP_DONOTROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const _ as _,
        4,
    );

    // Disable DWM Acrylic backdrop (make it solid)
    let backdrop = 1i32; // DWMSBT_NONE (None)
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE,
        &backdrop as *const _ as _,
        4,
    );

    // Load the search engine in a background thread so the window appears instantly.
    let hwnd_usize = hwnd.0 as usize;
    let db_path_for_thread = db_path.clone();
    std::thread::spawn(move || {
        let db_path = db_path_for_thread;
        indexer::start_indexer(db_path.clone());
        indexer::start_watcher(db_path.clone()); // instant indexing of new/changed files
        browser_indexer::start_browser_indexer(db_path.clone());


        let db_path_for_timeline = db_path.clone();
        let hwnd_for_timeline = SendHwnd(HWND(hwnd_usize as *mut std::ffi::c_void));
        std::thread::spawn(move || {
            let com_initialized = unsafe {
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                )
            }
            .is_ok();
            unsafe {
                start_timeline_tracker(db_path_for_timeline, hwnd_for_timeline);
            }
            if com_initialized {
                unsafe {
                    windows::Win32::System::Com::CoUninitialize();
                }
            }
        });

        let db_path_for_engine = db_path.clone();
        let result = SearchEngine::new(db_path_for_engine, true);
        let db_path_for_slow = db_path.clone();
        let result_slow = SearchEngine::new(db_path_for_slow, false);
        let hwnd_bg = HWND(hwnd_usize as *mut std::ffi::c_void);
        unsafe {
            match (result, result_slow) {
                (Ok(mut engine), Ok(mut slow_engine)) => {


                    // Two independent worker threads, each with its own engine and channel, so a
                    // multi-second content (FTS) search NEVER blocks instant file/folder results.
                    // Each thread coalesces to the newest queued query; stale results are dropped by
                    // the UI via query_id, so no explicit cancellation is needed.
                    let (tx_fast, rx_fast) = std::sync::mpsc::channel::<SearchRequest>();
                    let (tx_slow, rx_slow) = std::sync::mpsc::channel::<SearchRequest>();
                    let hwnd_worker_fast = SendHwnd(hwnd_bg);
                    let hwnd_worker_slow = SendHwnd(hwnd_bg);

                    // FAST worker: apps + recent + in-memory files/folders (is_final = false).
                    std::thread::spawn(move || {
                        let hwnd_target = hwnd_worker_fast;
                        loop {
                            let mut current_req = match rx_fast.recv() {
                                Ok(r) => r,
                                Err(_) => break,
                            };
                            while let Ok(next_req) = rx_fast.try_recv() {
                                current_req = next_req;
                            }
                            let results =
                                engine.search_with_fts(&current_req.query, MAX_RESULTS, false);
                            let ptr = Box::into_raw(Box::new(results)) as isize;
                            let wparam = (current_req.query_id & 0xFFFF_FFFF) as usize; // is_final = false
                            let _ = PostMessageW(
                                hwnd_target.0,
                                WM_SEARCH_RESULTS,
                                WPARAM(wparam),
                                LPARAM(ptr),
                            );
                        }
                    });

                    // SLOW worker: full search incl. content/OCR/settings FTS (is_final = true).
                    std::thread::spawn(move || {
                        let hwnd_target = hwnd_worker_slow;
                        loop {
                            let mut current_req = match rx_slow.recv() {
                                Ok(r) => r,
                                Err(_) => break,
                            };
                            while let Ok(next_req) = rx_slow.try_recv() {
                                current_req = next_req;
                            }
                            let results =
                                slow_engine.search_with_fts(&current_req.query, MAX_RESULTS, true);
                            let ptr = Box::into_raw(Box::new(results)) as isize;
                            let wparam =
                                ((current_req.query_id & 0xFFFF_FFFF) as usize) | (1 << 32); // is_final = true
                            let _ = PostMessageW(
                                hwnd_target.0,
                                WM_SEARCH_RESULTS,
                                WPARAM(wparam),
                                LPARAM(ptr),
                            );
                        }
                    });

                    let tx_ptr = Box::into_raw(Box::new((tx_fast, tx_slow))) as isize;
                    let _ = PostMessageW(hwnd_bg, WM_ENGINE_READY, WPARAM(1), LPARAM(tx_ptr));
                }
                (Err(e), _) | (_, Err(e)) => {
                    let msg = Box::into_raw(Box::new(e.to_string())) as isize;
                    let _ = PostMessageW(hwnd_bg, WM_ENGINE_READY, WPARAM(0), LPARAM(msg));
                }
            }
        }
    });

    // Flow Launcher uses a native registered hotkey for the launcher toggle; keep
    // the low-level hook only for recording a new shortcut in Settings.
    let settings = crate::settings::AppSettings::load();
    if !crate::hotkey::register_hotkey(hwnd, HOTKEY_ID, &settings.global_hotkey) {
        applog::log(&format!(
            "launcher hotkey {} registration FAILED (already in use?)",
            settings.global_hotkey
        ));
        if should_prompt_for_default_hotkey_conflict(
            first_settings_run,
            &settings.global_hotkey,
            false,
        ) {
            prompt_default_hotkey_conflict(hwnd, &settings.global_hotkey);
        }
    } else {
        applog::log(&format!(
            "launcher hotkey {} registered",
            settings.global_hotkey
        ));
    }

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND(null_mut()), 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
}

fn should_prompt_for_default_hotkey_conflict(
    first_settings_run: bool,
    hotkey: &str,
    registered: bool,
) -> bool {
    first_settings_run && !registered && crate::hotkey::same_hotkey(hotkey, "Alt+Space")
}

unsafe fn prompt_default_hotkey_conflict(hwnd: HWND, hotkey: &str) {
    let message = format!(
        "{hotkey} is already used by another app.\n\nOpen Settings to change the launcher hotkey now?\n\nChoose No to keep {hotkey}; it will start working after the other app releases it."
    );
    let mut msg: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let mut title: Vec<u16> = "Launcher Hotkey Conflict\0".encode_utf16().collect();
    let choice = MessageBoxW(
        hwnd,
        PCWSTR(msg.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_ICONWARNING | MB_YESNO,
    );
    let _ = (&mut msg, &mut title);
    if choice == IDYES {
        if let Ok(exe) = std::env::current_exe() {
            let _ = Command::new(exe).arg("--settings").spawn();
        }
    }
}

unsafe extern "system" fn preview_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        EndPaint, GetObjectW, SelectObject, BITMAP, PAINTSTRUCT, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, GWLP_USERDATA, WM_NCCREATE, WM_PAINT,
    };

    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return LRESULT(1);
    }

    let sp = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;

    match msg {
        WM_PAINT => {
            if sp.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let s = &*sp;
            let palette = s.theme.palette();

            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let win_w = rc.right - rc.left;
            let win_h = rc.bottom - rc.top;

            // Double-buffer
            let mdc = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, win_w, win_h);
            let old = SelectObject(mdc, bmp);

            // Draw background
            fill(mdc, 0, 0, win_w, win_h, palette.bg);

            // Draw a thin border
            fill(mdc, 0, 0, win_w, 1, palette.clr_div); // top
            fill(mdc, 0, win_h - 1, win_w, 1, palette.clr_div); // bottom
            fill(mdc, 0, 0, 1, win_h, palette.clr_div); // left
            fill(mdc, win_w - 1, 0, 1, win_h, palette.clr_div); // right

            if let Some((_result, path)) = s
                .results
                .get(s.selected)
                .and_then(|result| image_path_for_result(result).map(|path| (result, path)))
            {
                let cache = s.clipboard_thumbnails.borrow_mut();
                let hbitmap = cache.get(path).copied();

                if let Some(hbitmap) = hbitmap {
                    let mut bmp_info: BITMAP = std::mem::zeroed();
                    let size = std::mem::size_of::<BITMAP>() as i32;
                    if GetObjectW(hbitmap, size, Some(&mut bmp_info as *mut BITMAP as *mut _)) != 0
                    {
                        let img_w = bmp_info.bmWidth;
                        let img_h = bmp_info.bmHeight;

                        let max_w = win_w - 16;
                        let max_h = win_h - 16;
                        let scale = (max_w as f32 / img_w as f32)
                            .min(max_h as f32 / img_h as f32)
                            .min(1.0);
                        let draw_w = (img_w as f32 * scale).round() as i32;
                        let draw_h = (img_h as f32 * scale).round() as i32;

                        // Center the image within the window
                        let img_x = (win_w - draw_w) / 2;
                        let img_y = (win_h - draw_h) / 2;

                        draw_cached_bmp(mdc, img_x, img_y, draw_w, draw_h, hbitmap);
                    }
                }
            }

            let _ = BitBlt(hdc, 0, 0, win_w, win_h, mdc, 0, 0, SRCCOPY);
            let _ = SelectObject(mdc, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mdc);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn show_preview_window(hwnd_parent: HWND, s: *mut State) {
    use windows::Win32::Graphics::Gdi::{GetObjectW, InvalidateRect, BITMAP};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, GetWindowRect, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA,
        HWND_TOPMOST, SWP_NOACTIVATE, SW_SHOWNOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    let mut parent_rect = RECT::default();
    let _ = GetWindowRect(hwnd_parent, &mut parent_rect);

    let mut p_w = 260;
    let mut p_h = parent_rect.bottom - parent_rect.top;

    // Explicit shared borrows through the raw pointer (no implicit autoref, and no
    // aliasing &mut State — the caller still holds a borrow). All dropped before the
    // hwnd_preview write below.
    if let Some((_result, path)) = (&*s)
        .results
        .get((*s).selected)
        .and_then(|result| image_path_for_result(result).map(|path| (result, path)))
    {
        let mut cache = (*s).clipboard_thumbnails.borrow_mut();
        let hbitmap = cache.get(path).copied().or_else(|| {
            load_shell_thumbnail(path, 256).inspect(|h| {
                cache.insert(path.to_string(), *h);
                prune_thumbnail_cache(&mut cache, path);
            })
        });

        if let Some(hbitmap) = hbitmap {
            let mut bmp: BITMAP = std::mem::zeroed();
            let size = std::mem::size_of::<BITMAP>() as i32;
            if GetObjectW(hbitmap, size, Some(&mut bmp as *mut BITMAP as *mut _)) != 0 {
                let img_w = bmp.bmWidth;
                let img_h = bmp.bmHeight;
                let max_size = 320;
                let scale = (max_size as f32 / img_w as f32)
                    .min(max_size as f32 / img_h as f32)
                    .min(1.0);
                let draw_w = (img_w as f32 * scale).round() as i32;
                let draw_h = (img_h as f32 * scale).round() as i32;

                p_w = draw_w + 16;
                p_h = draw_h + 16;
            }
        }
    }

    let p_x = parent_rect.right + 8;

    let visual_idx = (*s).selected.saturating_sub((*s).scroll_offset);
    let item_y = (&*s).result_row_y(visual_idx);
    let item_center = item_y + ((*s).app_settings.item_height as i32) / 2;
    let mut p_y = parent_rect.top + item_center - (p_h / 2);

    if p_y + p_h > parent_rect.bottom {
        p_y = parent_rect.bottom - p_h;
    }
    if p_y < parent_rect.top {
        p_y = parent_rect.top;
    }

    if (&*s).hwnd_preview.is_none() {
        let preview_class: Vec<u16> = "omnisearch-lite-preview\0".encode_utf16().collect();
        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();

        let hwnd_preview = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(preview_class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            p_x,
            p_y,
            p_w,
            p_h,
            hwnd_parent,
            HMENU(null_mut()),
            hinst,
            None,
        );
        if let Ok(h) = hwnd_preview {
            SetWindowLongPtrW(h, GWLP_USERDATA, s as isize);
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            (*s).hwnd_preview = Some(h);
        }
    } else {
        let hwnd_preview = (*s).hwnd_preview.unwrap();
        let _ = SetWindowPos(
            hwnd_preview,
            HWND_TOPMOST,
            p_x,
            p_y,
            p_w,
            p_h,
            SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd_preview, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(hwnd_preview, None, FALSE);
    }
}

unsafe fn hide_preview_window(s: &State) {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if let Some(h) = s.hwnd_preview {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

// ── WndProc ───────────────────────────────────────────────────────────────────
const WM_NEXT_ANIM_FRAME: u32 = WM_USER + 50;

thread_local! {
    static ANIM_LOOP_ACTIVE: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Guards clipboard access across threads; only one thread may hold
/// the Windows clipboard open at a time.
static CLIPBOARD_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
pub fn clipboard_lock() -> &'static std::sync::Mutex<()> {
    CLIPBOARD_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

unsafe fn trigger_anim_loop(hwnd: HWND) {
    ANIM_LOOP_ACTIVE.with(|f| {
        if !f.get() {
            f.set(true);
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                hwnd,
                WM_NEXT_ANIM_FRAME,
                WPARAM(0),
                LPARAM(0),
            );
        }
    });
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // A panic must never unwind across this `extern "system"` boundary — that is UB and
    // Windows tears the process down with STATUS_FATAL_USER_CALLBACK_EXCEPTION (0xC000041D).
    // Catch any panic from a message handler (e.g. a bad paint), let the panic hook record
    // its location to panic.log, and fall back to default handling so the app survives.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wnd_proc_inner(hwnd, msg, wp, lp)
    })) {
        Ok(res) => res,
        Err(_) => {
            if msg == WM_PAINT {
                // The paint handler may have panicked after BeginPaint without validating
                // the update region; clear it so we don't spin re-painting + re-panicking.
                let _ = windows::Win32::Graphics::Gdi::ValidateRect(hwnd, None);
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

unsafe extern "system" fn wnd_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let sp = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    #[allow(non_snake_case)]
    let SEARCH_H = if !sp.is_null() {
        unsafe { (*sp).search_h() }
    } else {
        68
    };

    match msg {
        WM_NEXT_ANIM_FRAME => {
            if !sp.is_null() {
                let s = &mut *sp;
                s.search_anim_tick = (s.search_anim_tick + 1) % 8;
                let window_anim_active = tick_window_animation(hwnd, s);
                let next_h = animated_height(s);
                let height_changed = next_h != s.shown_h;
                if height_changed {
                    s.shown_h = next_h;
                }
                let height_done = s.shown_h == s.target_h;
                let animating = !height_done || window_anim_active;

                if window_anim_active || height_changed {
                    let _ = InvalidateRect(hwnd, None, FALSE);
                    unsafe {
                        let _ = windows::Win32::Graphics::Dwm::DwmFlush();
                        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
                    }
                } else if s.search_loading {
                    invalidate_search_row(hwnd, s);
                    unsafe {
                        let _ = windows::Win32::Graphics::Dwm::DwmFlush();
                        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
                    }
                }

                if animating {
                    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, WM_NEXT_ANIM_FRAME, WPARAM(0), LPARAM(0)) };
                } else {
                    ANIM_LOOP_ACTIVE.with(|f| f.set(false));
                }
            } else {
                ANIM_LOOP_ACTIVE.with(|f| f.set(false));
            }
            LRESULT(0)
        }

        // Deferred from tick_window_animation - see WM_ANIM_FOREGROUND/WM_ANIM_HIDE_WINDOW
        // constant comments. Neither touches State, so no aliasing risk here.
        WM_ANIM_FOREGROUND => {
            force_foreground(hwnd);
            LRESULT(0)
        }
        WM_ANIM_HIDE_WINDOW => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }

        WM_CREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as _);
            LRESULT(0)
        }

        WM_SETCURSOR => {
            unsafe {
                use windows::Win32::Foundation::HINSTANCE;
                use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_CROSS};
                if !sp.is_null() && (*sp).color_picker_active {
                    if let Ok(cursor) = LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_CROSS) {
                        SetCursor(cursor);
                        return LRESULT(1);
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => {
            let s = &mut *sp;
            match s.anim {
                Anim::Hidden | Anim::Hiding { .. } => do_show(hwnd, s),
                Anim::Visible => start_hide(hwnd, s),
                Anim::Appearing { .. } => {}
            }
            LRESULT(0)
        }



        WM_START_EDITING => {
            let ptr = lp.0 as *mut (String, String);
            let (id, content) = unsafe { *Box::from_raw(ptr) };
            if !sp.is_null() {
                let s = unsafe { &mut *sp };
                s.editing_item = Some(id);
                s.query = content;
                s.cursor_pos = s.query.len();
                s.selected = 0;
                s.scroll_offset = 0;
                let _ = unsafe { InvalidateRect(hwnd, None, FALSE) };
            }
            LRESULT(0)
        }

        WM_REFRESH_SEARCH => {
            if !sp.is_null() {
                let s = unsafe { &mut *sp };
                unsafe {
                    trigger_search(hwnd, s);
                }
            }
            LRESULT(0)
        }

        // Clicking a result must not re-activate/churn focus, or the launcher
        // (which holds focus via AttachThreadInput) dismisses itself before the
        // click executes. MA_NOACTIVATE=3 keeps focus stable; the click still fires.
        0x0021 /* WM_MOUSEACTIVATE */ => LRESULT(3),

        WM_KILLFOCUS => {
            if !sp.is_null() {
                let s = &mut *sp;
                if s.note_editing {
                    return LRESULT(0);
                }
                // The window receiving focus is wParam; if it's us (or none), don't hide.
                let next = HWND(wp.0 as *mut std::ffi::c_void);
                if next == hwnd || next.0.is_null() {
                    return LRESULT(0);
                }
                if s.app_settings.hide_on_lose_focus && matches!(s.anim, Anim::Visible) {
                    start_hide(hwnd, s);
                }
            }
            LRESULT(0)
        }

        WM_ACTIVATEAPP | WM_ACTIVATE => {
            if !sp.is_null() {
                let s = &mut *sp;
                let app_inactive = msg == WM_ACTIVATEAPP && wp.0 == 0;
                let window_inactive = msg == WM_ACTIVATE && (wp.0 & 0xffff) == 0;
                if s.app_settings.hide_on_lose_focus
                    && (app_inactive || window_inactive)
                    && !s.note_editing
                    && matches!(s.anim, Anim::Visible)
                {
                    start_hide(hwnd, s);
                }
            }
            LRESULT(0)
        }

        WM_ICON_LOADED => {
            if sp.is_null() {
                unsafe {
                    let _ = Box::from_raw(lp.0 as *mut String);
                }
                return LRESULT(0);
            }
            let s = &mut *sp;
            let hicon = HICON(wp.0 as *mut std::ffi::c_void);
            let key_box = unsafe { Box::from_raw(lp.0 as *mut String) };
            let key = *key_box;

            // Remove from pending set so the icon can be retried on next search
            s.pending_icons.remove(&key);

            if !hicon.0.is_null() {
                if let Some(old_hicon) = s.app_icons.insert(key.clone(), hicon) {
                    if s.app_icons_owned.remove(&key) && !old_hicon.0.is_null() && old_hicon != hicon {
                        unsafe {
                            let _ = DestroyIcon(old_hicon);
                        }
                    }
                }
                s.app_icons_owned.insert(key);
                prune_owned_icon_cache(s, "");
            }

            let _ = KillTimer(hwnd, TIMER_ICON_BATCH);
            let _ = SetTimer(hwnd, TIMER_ICON_BATCH, 40, None);
            LRESULT(0)
        }

        WM_CLIPBOARDUPDATE => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &*sp;

            // Check if foreground window is the launcher itself to prevent duplicates
            let hwnd_fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            if hwnd_fg == hwnd {
                return LRESULT(0);
            }

            let db_path = s.db_path.clone();
            let app_name = unsafe { get_active_app_name() };

            // paste_from_clipboard and copy_to_clipboard now acquire clipboard_lock()
            // internally, so no explicit lock is needed here.

            // Try text format. Some apps put both text/HTML and bitmap data on the
            // clipboard for copied images, so image capture is attempted independently
            // below instead of living in this branch's `else`.
            if let Some(text) = unsafe { paste_from_clipboard(hwnd) } {
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    let db_path_clone = db_path.clone();
                    let app_name_clone = app_name.clone();
                    std::thread::spawn(move || {
                        if let Ok(conn) = rusqlite::Connection::open(&db_path_clone) {
                            let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64;
                            let _ = conn.execute(
                                "INSERT INTO clipboard_history (content, timestamp, source_app, is_image, pinned) \
                                 VALUES (?, ?, ?, 0, 0) \
                                 ON CONFLICT(content) DO UPDATE SET \
                                     timestamp = excluded.timestamp, \
                                     source_app = excluded.source_app, \
                                     is_image = excluded.is_image;",
                                rusqlite::params![trimmed, now, app_name_clone],
                            );
                            search::insert_memory_event(
                                &conn,
                                now,
                                "Clipboard",
                                "Copied Text",
                                &format!("Copied text from {}", app_name_clone),
                                &trimmed,
                                &app_name_clone,
                                None,
                                None,
                            );
                            let _ = conn.execute(
                                "DELETE FROM clipboard_history WHERE pinned = 0 AND id NOT IN (SELECT id FROM clipboard_history ORDER BY pinned DESC, timestamp DESC LIMIT 500);",
                                [],
                            );
                        }
                    });
                }
            }
            let _ = unsafe { save_clipboard_image(hwnd, &db_path, &app_name) };
            LRESULT(0)
        }

        WM_ENGINE_READY => {
            if wp.0 == 1 {
                let (tx_fast, tx_slow) = unsafe {
                    *Box::from_raw(
                        lp.0 as *mut (
                            std::sync::mpsc::Sender<SearchRequest>,
                            std::sync::mpsc::Sender<SearchRequest>,
                        ),
                    )
                };
                if !sp.is_null() {
                    let s = &mut *sp;
                    s.search_tx = Some(tx_fast);
                    s.search_tx_slow = Some(tx_slow);
                    trigger_search(hwnd, s);
                }
            } else {
                let err = *Box::from_raw(lp.0 as *mut String);
                let mut msg: Vec<u16> = format!("Engine error:\n{err}\0").encode_utf16().collect();
                let mut title: Vec<u16> = "OmniSearch Lite\0".encode_utf16().collect();
                MessageBoxW(
                    HWND(null_mut()),
                    PCWSTR(msg.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_ICONERROR | MB_OK,
                );
                let _ = (&mut msg, &mut title);
            }
            LRESULT(0)
        }

        WM_SEARCH_RESULTS => {
            let packed = wp.0;
            let query_id = packed & 0xFFFF_FFFF;
            let is_final = (packed >> 32) != 0;
            let results_ptr = lp.0 as *mut Vec<SearchResult>;
            let results = unsafe { *Box::from_raw(results_ptr) };
            if !sp.is_null() {
                let s = &mut *sp;
                if s.query.is_empty() {
                    // Discard search results if user cleared the query
                } else if query_id == s.current_query_id {
                    if !is_final && results.is_empty() {
                        return LRESULT(0);
                    }
                    s.results_stale = false;
                    if is_final {
                        s.search_loading = false;
                    }
                    s.unfiltered_results = results;
                    s.filter_counts = filter_counts_for_results(&s.unfiltered_results);
                    let mut filtered = s.unfiltered_results.clone();
                    if !matches!(s.active_filter, FilterType::All) {
                        filtered.retain(|r| result_matches_filter(r, s.active_filter));
                    }
                    apply_sort(&mut filtered, s.sort_asc, &s.query);
                    s.results = filtered;
                    s.result_reasons = compute_result_reasons(&s.results);
                    if s.results.is_empty() {
                        s.selected = 0;
                        s.scroll_offset = 0;
                    } else {
                        if s.query.is_empty() {
                            // Restore the homepage item the user last visited (not a fixed default).
                            s.selected = s.homepage_sel.min(s.results.len() - 1);
                        } else {
                            s.selected = s.selected.min(s.results.len() - 1);
                        }
                        let vr = s.visible_results();
                        s.scroll_offset = s
                            .scroll_offset
                            .min(s.results.len().saturating_sub(vr));
                    }

                    // Clear stale WINDOW icon cache when new window results arrive
                    if s.results.iter().any(|r| r.entry.source == "WINDOW") {
                        s.app_icons.retain(|k, _| !k.starts_with("window:"));
                        s.app_icons_owned.retain(|k| !k.starts_with("window:"));
                    }
                    trigger_icon_loading(hwnd, s);
                    sync_height_animation(hwnd, s);
                    invalidate_results_area(hwnd, s);
                }
            }
            LRESULT(0)
        }

        WM_AI_RESULT => {
            let payload = unsafe { *Box::from_raw(lp.0 as *mut (bool, String)) };
            if !sp.is_null() {
                let s = &mut *sp;
                let target_chat = wp.0 as i64;
                let active_chat = s.active_chat_id.unwrap_or(0);
                if target_chat == active_chat && (active_chat != 0 || s.ai_pending) {
                    let (ok, text) = payload;
                    let _ = KillTimer(hwnd, TIMER_AI_ANIM);
                    s.ai_pending = false;
                    s.ai_tick = 0;
                    s.ai_scroll = 0;
                    s.ai_follow_bottom = true;
                    s.ai_answer = Some(if ok { text } else { format!("⚠ {}", text) });
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
            }
            LRESULT(0)
        }

        WM_AI_PROGRESS => {
            if lp.0 != 0 {
                let text = unsafe { *Box::from_raw(lp.0 as *mut String) };
                if !sp.is_null() {
                    let s = &mut *sp;
                    let target_chat = wp.0 as i64;
                    let active_chat = s.active_chat_id.unwrap_or(0);
                    if target_chat == active_chat && (active_chat != 0 || s.ai_pending) {
                        s.ai_answer = Some(text);
                        s.ai_follow_bottom = true;
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
            }
            LRESULT(0)
        }





        WM_TIMER => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;
            match wp.0 {
                TIMER_DEBOUNCE => {
                    let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
                    trigger_search(hwnd, s);
                }
                TIMER_CURSOR_BLINK => {
                    if text_caret_active(s) {
                        s.cursor_visible = !s.cursor_visible;
                        if search_input_caret_active(s) {
                            invalidate_search_row(hwnd, s);
                        } else {
                            let _ = InvalidateRect(hwnd, None, FALSE);
                        }
                    } else {
                        s.cursor_visible = false;
                        let _ = KillTimer(hwnd, TIMER_CURSOR_BLINK);
                    }
                }

                TIMER_AI_ANIM => {
                    s.ai_tick = (s.ai_tick + 1) % 60;
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
                TIMER_ICON_BATCH => {
                    let _ = KillTimer(hwnd, TIMER_ICON_BATCH);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }

                TIMER_SEARCH_ANIM => {
                    s.search_anim_tick = (s.search_anim_tick + 1) % 8;
                    let window_anim_active = tick_window_animation(hwnd, s);
                    let next_h = animated_height(s);
                    let height_changed = next_h != s.shown_h;
                    if height_changed {
                        s.shown_h = next_h;
                    }
                    let height_done = s.shown_h == s.target_h;
                    if height_done && !s.search_loading && !window_anim_active {
                        // Nothing left to animate.
                        let _ = KillTimer(hwnd, TIMER_SEARCH_ANIM);
                    } else if height_done && s.search_loading && !window_anim_active {
                        // Grow finished; only the loading spinner remains, so ease off to a
                        // gentle 80ms cadence instead of repainting the search row at 60fps.
                        let _ = SetTimer(hwnd, TIMER_SEARCH_ANIM, 80, None);
                    }
                    if window_anim_active || height_changed {
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else if s.search_loading {
                        invalidate_search_row(hwnd, s);
                    }
                }
                TIMER_CLIPBOARD_MSG => {
                    let _ = KillTimer(hwnd, TIMER_CLIPBOARD_MSG);
                    s.placeholder_override = None;
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_CHAR => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;
            let is_control_char = (wp.0 as u32) < 32 || (wp.0 as u32) == 127;
            if !is_control_char && s.placeholder_override.is_some() {
                s.placeholder_override = None;
                let _ = KillTimer(hwnd, TIMER_CLIPBOARD_MSG);
                let _ = InvalidateRect(hwnd, None, FALSE);
            }
            if s.form_state != FormState::None {
                s.search_input_active = true;
                if let Some(c) = wm_char_decode(&mut s.pending_surrogate, wp.0 as u32) {
                    if !c.is_control() {
                        if s.text_selected {
                            s.query.clear();
                            s.cursor_pos = 0;
                            s.text_selected = false;
                        }
                        s.query.insert(s.cursor_pos, c);
                        s.cursor_pos += c.len_utf8();
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
                return LRESULT(0);
            }
            if s.note_editing {
                if let Some(c) = wm_char_decode(&mut s.pending_surrogate, wp.0 as u32) {
                    if !c.is_control() {
                        s.note_text.push(c);
                        s.note_scroll = i32::MAX; // follow the caret to the bottom
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
                return LRESULT(0);
            }

            s.submenu_active = false;
            if let Some(c) = wm_char_decode(&mut s.pending_surrogate, wp.0 as u32) {
                if !c.is_control() {
                    if s.chat_input_active {
                        if s.text_selected {
                            s.chat_input.clear();
                            s.chat_cursor_pos = 0;
                            s.text_selected = false;
                        }
                        s.chat_input.insert(s.chat_cursor_pos, c);
                        s.chat_cursor_pos += c.len_utf8();
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    s.search_input_active = true;
                    if s.text_selected {
                        s.query.clear();
                        s.cursor_pos = 0;
                        s.text_selected = false;
                    }
                    s.query.insert(s.cursor_pos, c);
                    s.cursor_pos += c.len_utf8();
                    s.selected = 0;
                    s.scroll_offset = 0;
                    sync_height_animation(hwnd, s);
                    kick_debounce(hwnd, s);
                    reset_cursor_blink(hwnd, s);
                    invalidate_search_row(hwnd, s);
                }
            }
            LRESULT(0)
        }

        WM_KEYUP if wp.0 == VK_TAB.0 as usize => {
            if !sp.is_null() {
                let s = &mut *sp;
                if s.image_preview_active {
                    s.image_preview_active = false;
                    hide_preview_window(s);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
            }
            LRESULT(0)
        }

        WM_KEYDOWN => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;
            let vk = VIRTUAL_KEY(wp.0 as u16);
            let ctrl_down = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
            if vk == VK_BACK || vk == VK_DELETE || (ctrl_down && vk.0 as u32 == 0x56) {
                if s.placeholder_override.is_some() {
                    s.placeholder_override = None;
                    let _ = KillTimer(hwnd, TIMER_CLIPBOARD_MSG);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
            }
            // In-app note editor: text is appended in WM_CHAR; control keys here.
            if s.note_editing {
                if ctrl_down && vk.0 == 0x53 { // Ctrl+S — save, keep editing
                    save_note(s);
                    return LRESULT(0);
                }
                match vk {
                    VK_ESCAPE => { close_note_editor(hwnd, s); }       // save & close
                    VK_BACK => { s.note_text.pop(); s.note_scroll = i32::MAX; reset_cursor_blink(hwnd, s); let _ = InvalidateRect(hwnd, None, FALSE); }
                    VK_RETURN => { s.note_text.push('\n'); s.note_scroll = i32::MAX; reset_cursor_blink(hwnd, s); let _ = InvalidateRect(hwnd, None, FALSE); }
                    VK_UP => { s.note_scroll = (s.note_scroll - 24).max(0); let _ = InvalidateRect(hwnd, None, FALSE); }
                    VK_DOWN => { s.note_scroll += 24; let _ = InvalidateRect(hwnd, None, FALSE); }
                    _ => {}
                }
                return LRESULT(0);
            }



            if s.color_picker_active {
                if vk == VK_ESCAPE {
                    stop_color_picker(hwnd, s);
                }
                return LRESULT(0);
            }

            // AI answer panel captures keys: Esc/Backspace closes, Enter submits follow-up.
            if s.ai_pending {
                match vk {
                    VK_ESCAPE => {
                        close_ai_panel_to_agent_history(hwnd, s);
                        return LRESULT(0);
                    }
                    VK_DOWN => {
                        ai_scroll_down(s, 40);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                    VK_UP => {
                        ai_scroll_up(s, 40);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                    _ => {}
                }
            }

            if s.ai_answer.is_some() || s.chat_input_active {
                if s.chat_input_active {
                    if ctrl_down {
                        match vk.0 as u32 {
                            0x41 => {
                                if !s.chat_input.is_empty() {
                                    s.text_selected = true;
                                    s.chat_cursor_pos = s.chat_input.len();
                                    reset_cursor_blink(hwnd, s);
                                    let _ = InvalidateRect(hwnd, None, FALSE);
                                }
                                return LRESULT(0);
                            }
                            0x43 => {
                                if s.text_selected && !s.chat_input.is_empty() {
                                    copy_to_clipboard(hwnd, &s.chat_input);
                                }
                                return LRESULT(0);
                            }
                            0x56 => {
                                paste_clipboard_into_chat(hwnd, s);
                                return LRESULT(0);
                            }
                            _ => {}
                        }
                    }
                    match vk {
                        VK_ESCAPE => {
                            close_ai_panel_to_agent_history(hwnd, s);
                            return LRESULT(0);
                        }
                        VK_BACK | VK_DELETE => {
                            if s.text_selected {
                                s.chat_input.clear();
                                s.chat_cursor_pos = 0;
                                s.text_selected = false;
                                reset_cursor_blink(hwnd, s);
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            } else if vk == VK_BACK && s.chat_cursor_pos > 0 {
                                let mut p = s.chat_cursor_pos - 1;
                                while p > 0 && !s.chat_input.is_char_boundary(p) {
                                    p -= 1;
                                }
                                s.chat_input.remove(p);
                                s.chat_cursor_pos = p;
                                reset_cursor_blink(hwnd, s);
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            } else if vk == VK_DELETE && s.chat_cursor_pos < s.chat_input.len() {
                                let mut p = s.chat_cursor_pos + 1;
                                while p < s.chat_input.len() && !s.chat_input.is_char_boundary(p) {
                                    p += 1;
                                }
                                s.chat_input.replace_range(s.chat_cursor_pos..p, "");
                                reset_cursor_blink(hwnd, s);
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            }
                            return LRESULT(0);
                        }
                        VK_LEFT => {
                            s.text_selected = false;
                            if ctrl_down {
                                s.chat_cursor_pos = word_left(&s.chat_input, s.chat_cursor_pos);
                            } else if s.chat_cursor_pos > 0 {
                                let mut p = s.chat_cursor_pos - 1;
                                while p > 0 && !s.chat_input.is_char_boundary(p) {
                                    p -= 1;
                                }
                                s.chat_cursor_pos = p;
                            }
                            reset_cursor_blink(hwnd, s);
                            let _ = InvalidateRect(hwnd, None, FALSE);
                            return LRESULT(0);
                        }
                        VK_RIGHT => {
                            s.text_selected = false;
                            if ctrl_down {
                                s.chat_cursor_pos = word_right(&s.chat_input, s.chat_cursor_pos);
                            } else if s.chat_cursor_pos < s.chat_input.len() {
                                let mut p = s.chat_cursor_pos + 1;
                                while p < s.chat_input.len() && !s.chat_input.is_char_boundary(p) {
                                    p += 1;
                                }
                                s.chat_cursor_pos = p;
                            }
                            reset_cursor_blink(hwnd, s);
                            let _ = InvalidateRect(hwnd, None, FALSE);
                            return LRESULT(0);
                        }
                        VK_RETURN => {
                            let msg = s.chat_input.trim().to_string();
                            if !msg.is_empty() {
                                s.chat_input.clear();
                                s.chat_cursor_pos = 0;
                                start_follow_up_chat(hwnd, s, msg);
                            }
                            return LRESULT(0);
                        }
                        VK_DOWN => {
                            ai_scroll_down(s, 40);
                            let _ = InvalidateRect(hwnd, None, FALSE);
                            return LRESULT(0);
                        }
                        VK_UP => {
                            ai_scroll_up(s, 40);
                            let _ = InvalidateRect(hwnd, None, FALSE);
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                }
                if ctrl_down && vk.0 as u32 == 0x43 {
                    // Ctrl+C
                    if let Some(ans) = &s.ai_answer {
                        copy_to_clipboard(hwnd, ans);
                    }
                    return LRESULT(0);
                }
                match vk {
                    VK_ESCAPE => {
                        close_ai_panel_to_agent_history(hwnd, s);
                        return LRESULT(0);
                    }
                    VK_BACK => {
                        if s.query.is_empty() {
                            close_ai_panel(hwnd, s);
                            return LRESULT(0);
                        }
                        // Let it fall through to normal backspace handling if there is text to edit!
                    }
                    VK_RETURN => {
                        let q_trim = s.query.trim().to_string();
                        if q_trim.is_empty() {
                            if let Some(ans) = s.ai_answer.clone() {
                                copy_to_clipboard(hwnd, &ans);
                            }
                            close_ai_panel(hwnd, s);
                        } else {
                            start_follow_up_chat(hwnd, s, q_trim);
                        }
                        return LRESULT(0);
                    }
                    VK_DOWN => {
                        ai_scroll_down(s, 40);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    VK_UP => {
                        ai_scroll_up(s, 40);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    _ => {} // Let other keys fall through to let user type!
                }
            }

            if s.form_state != FormState::None {
                match vk {
                    VK_ESCAPE => {
                        s.form_state = FormState::None;
                        s.query.clear();
                        s.cursor_pos = 0;
                        s.reset_results();
                        s.selected = s.homepage_sel;
                        s.scroll_offset = 0;
                        reset_cursor_blink(hwnd, s);
                        trigger_search(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    VK_RETURN => {
                        handle_form_enter(hwnd, s);
                        return LRESULT(0);
                    }
                    VK_BACK => {
                        if ctrl_down {
                            if s.text_selected {
                                s.query.clear();
                                s.cursor_pos = 0;
                                s.text_selected = false;
                            } else {
                                delete_word_before(s);
                            }
                        } else if s.text_selected {
                            s.query.clear();
                            s.cursor_pos = 0;
                            s.text_selected = false;
                        } else if s.cursor_pos > 0 {
                            let mut p = s.cursor_pos - 1;
                            while p > 0 && !s.query.is_char_boundary(p) {
                                p -= 1;
                            }
                            s.query.remove(p);
                            s.cursor_pos = p;
                        }
                        if s.query.is_empty() {
                            s.selected = s.homepage_sel;
                            s.reset_results();
                            sync_height_animation(hwnd, s);
                        } else {
                            s.selected = 0;
                            s.results.clear();
                            sync_height_animation(hwnd, s);
                            kick_debounce(hwnd, s);
                        }
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    VK_LEFT => {
                        if ctrl_down {
                            s.cursor_pos = word_left(&s.query, s.cursor_pos);
                        } else if s.cursor_pos > 0 {
                            let mut p = s.cursor_pos - 1;
                            while p > 0 && !s.query.is_char_boundary(p) {
                                p -= 1;
                            }
                            s.cursor_pos = p;
                        }
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    VK_RIGHT => {
                        if ctrl_down {
                            s.cursor_pos = word_right(&s.query, s.cursor_pos);
                        } else if s.cursor_pos < s.query.len() {
                            let mut p = s.cursor_pos + 1;
                            while p < s.query.len() && !s.query.is_char_boundary(p) {
                                p += 1;
                            }
                            s.cursor_pos = p;
                        }
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    _ => {
                        if ctrl_down {
                            match vk.0 as u32 {
                                0x41 => {
                                    // Ctrl + A
                                    if !s.query.is_empty() {
                                        s.text_selected = true;
                                        let _ = InvalidateRect(hwnd, None, FALSE);
                                    }
                                    return LRESULT(0);
                                }
                                0x43 => {
                                    // Ctrl + C
                                    if !s.query.is_empty() {
                                        copy_to_clipboard(hwnd, &s.query);
                                    }
                                    return LRESULT(0);
                                }
                                0x56 => {
                                    // Ctrl + V
                                    paste_clipboard_into_query(hwnd, s, false);
                                    return LRESULT(0);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                return LRESULT(0);
            }

            let vk = VIRTUAL_KEY(wp.0 as u16);

            // Check if Ctrl is pressed
            let ctrl_down = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;

            if ctrl_down {
                match vk.0 as u32 {
                    0x41 => {
                        // Ctrl + A (Select All)
                        if !s.query.is_empty() {
                            s.text_selected = true;
                            let _ = InvalidateRect(hwnd, None, FALSE);
                        }
                        return LRESULT(0);
                    }
                    0x43 => {
                        // Ctrl + C (Copy)
                        if !s.query.is_empty() {
                            copy_to_clipboard(hwnd, &s.query);
                        }
                        return LRESULT(0);
                    }
                    0x56 => {
                        // Ctrl + V (Paste)
                        paste_clipboard_into_query(hwnd, s, true);
                        return LRESULT(0);
                    }
                    0x50 => {
                        // Ctrl + P (Pin/Unpin toggle)
                        if let Some(r) = s.results.get(s.selected) {
                            if r.entry.source == "CLIPBOARD" {
                                let current_id = r.entry.id.clone();
                                let timestamps = selected_clip_timestamps(&s.selected_clip_ids, Some(&current_id));
                                if !timestamps.is_empty() {
                                    let pin_target = !current_id.starts_with("clip.pinned.");
                                    let timestamp_set: std::collections::HashSet<i64> =
                                        timestamps.iter().copied().collect();
                                    if !s.selected_clip_ids.is_empty() {
                                        s.selected_clip_ids = timestamp_set
                                            .iter()
                                            .map(|ts| clip_id_for_pin_state(*ts, pin_target))
                                            .collect();
                                    }
                                    for result in &mut s.results {
                                        if result.entry.source == "CLIPBOARD" {
                                            if let Some(ts) = clip_timestamp_from_id(&result.entry.id) {
                                                if timestamp_set.contains(&ts) {
                                                    result.entry.id = clip_id_for_pin_state(ts, pin_target);
                                                }
                                            }
                                        }
                                    }
                                    let _ = InvalidateRect(hwnd, None, FALSE);

                                    let db_path = s.db_path.clone();
                                    let hwnd_notify = SendHwnd(hwnd);
                                    std::thread::spawn(move || {
                                        let hwnd_notify = hwnd_notify;
                                        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                                            let _ = conn.busy_timeout(
                                                std::time::Duration::from_secs(5),
                                            );
                                            let _ =
                                                conn.execute_batch("PRAGMA journal_mode=WAL;");
                                            let mut changed_any = false;
                                            for ts in timestamps {
                                                if conn.execute(
                                                    "UPDATE clipboard_history SET pinned = ? WHERE timestamp = ?;",
                                                    rusqlite::params![if pin_target { 1 } else { 0 }, ts],
                                                ).is_ok() {
                                                    changed_any = true;
                                                }
                                            }
                                            if changed_any {
                                                    let _ = PostMessageW(
                                                        hwnd_notify.0,
                                                        WM_REFRESH_SEARCH,
                                                        WPARAM(0),
                                                        LPARAM(0),
                                                    );
                                            }
                                        }
                                    });
                                }
                            }
                        }
                        return LRESULT(0);
                    }
                    0x45 => {
                        // Ctrl + E (Edit selected clipboard item)
                        if s.editing_item.is_some() {
                            s.editing_item = None;
                            s.query = "clip:".to_string();
                            s.cursor_pos = s.query.len();
                            trigger_search(hwnd, s);
                        } else if let Some(r) = s.results.get(s.selected) {
                            if r.entry.source == "CLIPBOARD"
                                && !r.entry.launch_command.starts_with("copy_image:")
                            {
                                let id = r.entry.id.clone();
                                if let Some(ts) = clip_timestamp_from_id(&id) {
                                        let db_path = s.db_path.clone();
                                        let hwnd_notify = SendHwnd(hwnd);
                                        std::thread::spawn(move || {
                                            let hwnd_notify = hwnd_notify;
                                            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                                                if let Ok(content) = conn.query_row(
                                                     "SELECT content FROM clipboard_history WHERE timestamp = ?;",
                                                     [ts],
                                                     |row| row.get::<_, String>(0),
                                                 ) {
                                                     let content_ptr = Box::into_raw(Box::new((id, content)));
                                                     let _ = PostMessageW(
                                                         hwnd_notify.0,
                                                         WM_START_EDITING,
                                                         WPARAM(0),
                                                         LPARAM(content_ptr as isize),
                                                     );
                                                 }
                                            }
                                        });
                                }
                            }
                        }
                        return LRESULT(0);
                    }
                    0x4F => {
                        // Ctrl + O (OCR selected clipboard image)
                        let maybe_path = s.results.get(s.selected)
                            .and_then(|r| r.entry.launch_command.strip_prefix("copy_image:"))
                            .map(|p| p.to_string());
                        if let Some(path_owned) = maybe_path {
                            s.query = "Scanning image…".to_string();
                            s.cursor_pos = s.query.len();
                            s.reset_results();
                            reset_cursor_blink(hwnd, s);
                            let _ = InvalidateRect(hwnd, None, FALSE);
                            let hwnd_ocr = hwnd.0 as usize;
                            let db_path_ocr = s.db_path.clone();
                            std::thread::spawn(move || {
                                let com_initialized = unsafe {
                                    windows::Win32::System::Com::CoInitializeEx(
                                        None,
                                        windows::Win32::System::Com::COINIT_MULTITHREADED,
                                    )
                                }
                                .is_ok();
                                let result = run_clipboard_ocr(|| {
                                    indexer::ocr_image_file(std::path::Path::new(&path_owned))
                                });
                                if let Some(text) = &result {
                                    if let Ok(conn) = rusqlite::Connection::open(&db_path_ocr) {
                                        let _ =
                                            conn.busy_timeout(std::time::Duration::from_secs(5));
                                        let _ = conn.execute(
                                            "UPDATE clipboard_history SET ocr_text = ? WHERE content = ?;",
                                            rusqlite::params![text, path_owned],
                                        );
                                    }
                                }
                                if com_initialized {
                                    unsafe {
                                        windows::Win32::System::Com::CoUninitialize();
                                    }
                                }
                                let ptr = Box::into_raw(Box::new(result));
                                let _ = unsafe {
                                    windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                        windows::Win32::Foundation::HWND(
                                            hwnd_ocr as *mut std::ffi::c_void,
                                        ),
                                        WM_OCR_RESULT,
                                        WPARAM(0),
                                        LPARAM(ptr as isize),
                                    )
                                };
                            });
                        }
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }

            match vk {
                VK_ESCAPE => {
                    if s.submenu_active {
                        s.submenu_active = false;
                        s.submenu_selected = 0;
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    if s.delete_confirm {
                        s.delete_confirm = false;
                        s.selected_clip_ids.clear();
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else if s.editing_item.is_some() {
                        s.editing_item = None;
                        s.query = "clip:".to_string();
                        s.cursor_pos = s.query.len();
                        trigger_search(hwnd, s);
                    } else if s.text_selected {
                        s.text_selected = false;
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else if !s.query.is_empty() {
                        s.query.clear();
                        s.cursor_pos = 0;
                        s.reset_results();
                        s.scroll_offset = 0;
                        trigger_search(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else {
                        start_hide(hwnd, s);
                    }
                }
                VK_LEFT => {
                    if s.submenu_active {
                        s.submenu_active = false;
                        s.submenu_selected = 0;
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    if ctrl_down {
                        s.cursor_pos = word_left(&s.query, s.cursor_pos);
                    } else if s.cursor_pos > 0 {
                        let mut p = s.cursor_pos - 1;
                        while p > 0 && !s.query.is_char_boundary(p) {
                            p -= 1;
                        }
                        s.cursor_pos = p;
                    }
                    reset_cursor_blink(hwnd, s);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
                VK_RIGHT => {
                    if s.submenu_active {
                        // ignore
                    } else if ctrl_down {
                        s.cursor_pos = word_right(&s.query, s.cursor_pos);
                    } else if s.cursor_pos < s.query.len() {
                        let mut p = s.cursor_pos + 1;
                        while p < s.query.len() && !s.query.is_char_boundary(p) {
                            p += 1;
                        }
                        s.cursor_pos = p;
                    } else {
                        if let Some(r) = s.results.get(s.selected) {
                            if r.entry.source == "app" {
                                s.submenu_active = true;
                                s.submenu_selected = 0;
                            }
                        }
                    }
                    reset_cursor_blink(hwnd, s);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
                VK_BACK => {
                    if ctrl_down {
                        if s.text_selected {
                            s.query.clear();
                            s.cursor_pos = 0;
                            s.text_selected = false;
                        } else {
                            delete_word_before(s);
                        }
                    } else if s.text_selected {
                        s.query.clear();
                        s.cursor_pos = 0;
                        s.text_selected = false;
                    } else if s.cursor_pos > 0 {
                        let mut p = s.cursor_pos - 1;
                        while p > 0 && !s.query.is_char_boundary(p) {
                            p -= 1;
                        }
                        s.query.remove(p);
                        s.cursor_pos = p;
                    }
                    if s.query.is_empty() {
                        s.selected = s.homepage_sel;
                        s.reset_results();
                    } else {
                        s.selected = 0;
                    }
                    s.scroll_offset = 0;
                    kick_debounce(hwnd, s);
                    reset_cursor_blink(hwnd, s);
                    if s.query.is_empty() {
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else {
                        invalidate_search_row(hwnd, s);
                    }
                }
                VK_TAB => {
                    let is_clip_view =
                        s.query.starts_with("clip:") || s.query.starts_with("clipboard:");
                    if is_clip_view {
                        if let Some(r) = s.results.get(s.selected) {
                            if r.entry.source == "CLIPBOARD" {
                                let id = r.entry.id.clone();
                                if selected_clip_ids_contain(&s.selected_clip_ids, &id) {
                                    if let Some(ts) = clip_timestamp_from_id(&id) {
                                        s.selected_clip_ids.retain(|selected_id| {
                                            clip_timestamp_from_id(selected_id) != Some(ts)
                                        });
                                    } else {
                                        s.selected_clip_ids.remove(&id);
                                    }
                                } else {
                                    s.selected_clip_ids.insert(id);
                                }
                                s.delete_confirm = false;
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            }
                        }
                    } else {
                        if let Some(r) = s.results.get(s.selected) {
                            if image_path_for_result(r).is_some() {
                                s.image_preview_active = true;
                                show_preview_window(hwnd, sp);
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            } else if r.entry.source == "app" {
                                s.submenu_active = !s.submenu_active;
                                s.submenu_selected = 0;
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            }
                        }
                    }
                }
                VK_DELETE => {
                    let is_clip_view =
                        s.query.starts_with("clip:") || s.query.starts_with("clipboard:");
                    if is_clip_view {
                        if s.delete_confirm {
                            // Second Delete confirms the deletion!
                            s.delete_confirm = false;
                            let db_path = s.db_path.clone();
                            let selected_ids: Vec<String> =
                                s.selected_clip_ids.iter().cloned().collect();
                            let selected_timestamps: std::collections::HashSet<i64> =
                                selected_ids.iter().filter_map(|id| clip_timestamp_from_id(id)).collect();
                            s.selected_clip_ids.clear();
                            let hwnd_notify = SendHwnd(hwnd);
                            std::thread::spawn(move || {
                                let hwnd_notify = hwnd_notify;
                                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                                    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                                    let mut deleted_any = false;
                                    for id in &selected_ids {
                                        if let Some(ts) = clip_timestamp_from_id(id) {
                                            if conn.execute("DELETE FROM clipboard_history WHERE timestamp = ?;", [ts]).is_ok() {
                                                deleted_any = true;
                                            }
                                        }
                                    }
                                    if deleted_any {
                                        let _ = PostMessageW(
                                            hwnd_notify.0,
                                            WM_REFRESH_SEARCH,
                                            WPARAM(0),
                                            LPARAM(0),
                                        );
                                    }
                                }
                            });
                            s.results.retain(|r| {
                                clip_timestamp_from_id(&r.entry.id)
                                    .map_or(true, |ts| !selected_timestamps.contains(&ts))
                            });
                            let _ = InvalidateRect(hwnd, None, FALSE);
                        } else {
                            if s.selected_clip_ids.is_empty() {
                                if let Some(r) = s.results.get(s.selected) {
                                    if r.entry.source == "CLIPBOARD" {
                                        s.selected_clip_ids.insert(r.entry.id.clone());
                                    }
                                }
                            }
                            if !s.selected_clip_ids.is_empty() {
                                s.delete_confirm = true;
                                let _ = InvalidateRect(hwnd, None, FALSE);
                            }
                        }
                        return LRESULT(0);
                    } else if s.cursor_pos < s.query.len() {
                        s.query.remove(s.cursor_pos);
                        s.selected = 0;
                        s.scroll_offset = 0;
                        kick_debounce(hwnd, s);
                        reset_cursor_blink(hwnd, s);
                        invalidate_search_row(hwnd, s);
                    }
                }
                VK_RETURN => {
                    if s.submenu_active {
                        execute_submenu_action(hwnd, s);
                        return LRESULT(0);
                    }
                    if s.delete_confirm {
                        s.delete_confirm = false;
                        let db_path = s.db_path.clone();
                        let selected_ids: Vec<String> =
                            s.selected_clip_ids.iter().cloned().collect();
                        let selected_timestamps: std::collections::HashSet<i64> =
                            selected_ids.iter().filter_map(|id| clip_timestamp_from_id(id)).collect();
                        s.selected_clip_ids.clear();
                        let hwnd_notify = SendHwnd(hwnd);
                        std::thread::spawn(move || {
                            let hwnd_notify = hwnd_notify;
                            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                                let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                                let mut deleted_any = false;
                                for id in &selected_ids {
                                    if let Some(ts) = clip_timestamp_from_id(id) {
                                        if conn.execute("DELETE FROM clipboard_history WHERE timestamp = ?;", [ts]).is_ok() {
                                            deleted_any = true;
                                        }
                                    }
                                }
                                if deleted_any {
                                    let _ = PostMessageW(
                                        hwnd_notify.0,
                                        WM_REFRESH_SEARCH,
                                        WPARAM(0),
                                        LPARAM(0),
                                    );
                                }
                            }
                        });
                        s.results.retain(|r| {
                            clip_timestamp_from_id(&r.entry.id)
                                .map_or(true, |ts| !selected_timestamps.contains(&ts))
                        });
                        let _ = InvalidateRect(hwnd, None, FALSE);
                        return LRESULT(0);
                    }
                    if let Some(ref id) = s.editing_item {
                        if let Some(ts) = clip_timestamp_from_id(id) {
                                let db_path = s.db_path.clone();
                                let new_content = s.query.clone();
                                let new_content_for_thread = new_content.clone();
                                let hwnd_notify = SendHwnd(hwnd);
                                std::thread::spawn(move || {
                                    let hwnd_notify = hwnd_notify;
                                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                                        let _ =
                                            conn.busy_timeout(std::time::Duration::from_secs(5));
                                        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                                        if conn.execute(
                                            "UPDATE clipboard_history SET content = ? WHERE timestamp = ?;",
                                            rusqlite::params![new_content_for_thread, ts],
                                        ).is_ok() {
                                            let _ = PostMessageW(
                                                hwnd_notify.0,
                                                WM_REFRESH_SEARCH,
                                                WPARAM(0),
                                                LPARAM(0),
                                            );
                                        }
                                    }
                                });
                                copy_to_clipboard(hwnd, &new_content);
                        }
                        s.editing_item = None;
                        s.query = "clip:".to_string();
                        s.cursor_pos = s.query.len();
                        trigger_search(hwnd, s);
                        return LRESULT(0);
                    }
                    if !s.selected_clip_ids.is_empty() {
                        let db_path = s.db_path.clone();
                        let selected_ids: Vec<String> =
                            s.selected_clip_ids.iter().cloned().collect();
                        s.selected_clip_ids.clear();
                        let hwnd_copy = SendHwnd(hwnd);
                        std::thread::spawn(move || {
                            let hwnd_copy = hwnd_copy;
                            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                                let mut timestamps = Vec::new();
                                for id in &selected_ids {
                                    if let Some(ts) = clip_timestamp_from_id(id) {
                                        timestamps.push(ts);
                                    }
                                }
                                timestamps.sort();
                                timestamps.dedup();
                                let mut rows = Vec::new();
                                for ts in timestamps {
                                    if let Ok((content, is_image)) = conn.query_row(
                                        "SELECT content, is_image FROM clipboard_history WHERE timestamp = ?;",
                                        [ts],
                                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?)),
                                    ) {
                                        rows.push((content, is_image == 1));
                                    }
                                }
                                if rows.len() == 1 && rows[0].1 {
                                    let _ = copy_image_to_clipboard(hwnd_copy.0, &rows[0].0);
                                } else {
                                    let contents: Vec<String> = rows
                                        .into_iter()
                                        .map(|(content, is_image)| {
                                            if is_image { format!("[Image] {}", content) } else { content }
                                        })
                                        .collect();
                                    if !contents.is_empty() {
                                        let combined = contents.join("\r\n");
                                        copy_to_clipboard(hwnd_copy.0, &combined);
                                    }
                                }
                            }
                        });
                        do_hide(hwnd, s);
                        return LRESULT(0);
                    }

                    let is_shift = (GetKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
                    let is_ctrl = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                    if (is_shift || is_ctrl) && !s.query.is_empty() {
                        open_query_on_web(hwnd, s);
                    } else {
                        execute_selected(hwnd, s);
                    }
                }
                VK_DOWN => {
                    if s.submenu_active {
                        s.submenu_selected = (s.submenu_selected + 1).min(2);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else if !s.results.is_empty() {
                        s.selected = (s.selected + 1).min(s.results.len() - 1);
                        let vr = s.visible_results();
                        if s.selected >= s.scroll_offset + vr {
                            s.scroll_offset = s.selected - (vr - 1);
                        }
                        if s.query.is_empty() {
                            s.homepage_sel = s.selected;
                        }
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
                VK_UP => {
                    if s.submenu_active {
                        s.submenu_selected = s.submenu_selected.saturating_sub(1);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    } else if s.selected > 0 {
                        s.selected -= 1;
                        if s.selected < s.scroll_offset {
                            s.scroll_offset = s.selected;
                        }
                        if s.query.is_empty() {
                            s.homepage_sel = s.selected;
                        }
                        reset_cursor_blink(hwnd, s);
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
            LRESULT(0)
        }

        WM_MOUSEWHEEL => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;
            let in_chat_history_list =
                s.query.starts_with("chats:") || s.query.starts_with("agentchats:");
            if s.ai_answer.is_some() && !in_chat_history_list {
                let delta = (wp.0 >> 16) as i16;
                let step = (delta as i32).abs().max(40);
                if delta > 0 {
                    ai_scroll_up(s, step);
                } else {
                    ai_scroll_down(s, step);
                }
                let _ = InvalidateRect(hwnd, None, FALSE);
                return LRESULT(0);
            }

            if !s.query.is_empty() {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let y_start = s.launcher_top_y();
                let list_y = y_start + SEARCH_H + 1;
                if pt.y >= list_y + 8 && pt.y < list_y + 40 {
                    let delta = (wp.0 >> 16) as i16;
                    if delta < 0 {
                        s.filter_scroll_x = (s.filter_scroll_x + 88).min(480);
                    } else {
                        s.filter_scroll_x = (s.filter_scroll_x - 88).max(0);
                    }
                    let _ = InvalidateRect(hwnd, None, FALSE);
                    return LRESULT(0);
                }
            }
            if !s.results.is_empty() {
                let delta = (wp.0 >> 16) as i16;
                if delta > 0 {
                    // Scroll up
                    if s.scroll_offset > 0 {
                        s.scroll_offset -= 1;
                        let vr = s.visible_results();
                        if s.selected >= s.scroll_offset + vr {
                            s.selected = s.scroll_offset + vr - 1;
                        }
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                } else {
                    // Scroll down
                    let vr = s.visible_results();
                    if s.scroll_offset + vr < s.results.len() {
                        s.scroll_offset += 1;
                        if s.selected < s.scroll_offset {
                            s.selected = s.scroll_offset;
                        }
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;



            if s.ai_answer.is_some() {
                let my = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
                let mx = (lp.0 & 0xFFFF) as i16 as i32;
                let y_start = s.launcher_top_y();
                let mut rc_client = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc_client);
                let win_w = rc_client.right - rc_client.left;
                let x_start = s.launcher_x(win_w);

                let body_top = y_start + SEARCH_H + 1;
                let footer_h = 62;
                let actual_panel_h = s.paint_win_h() - SEARCH_H - 1;
                let content_bottom = y_start + SEARCH_H + 1 + actual_panel_h - footer_h;
                let input_y = content_bottom + 8;
                let input_x = x_start + 24;
                let input_w = s.launcher_w - 48;

                if mx >= input_x && mx < input_x + input_w && my >= input_y && my < input_y + 34 {
                    s.chat_input_active = true;
                    s.search_input_active = false;
                    s.chat_cursor_pos = s.chat_input.len();
                    s.text_selected = false;
                    reset_cursor_blink(hwnd, s);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                    return LRESULT(0);
                }

                // Click inside the chat history area (above bottom input box) copies the chat text
                if my >= body_top && my < content_bottom {
                    s.search_input_active = false;
                    s.cursor_visible = false;
                    if let Some(ans) = &s.ai_answer {
                        copy_to_clipboard(hwnd, ans);
                    }
                    return LRESULT(0);
                }
            }

            if s.color_picker_active {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let screen_dc = GetDC(HWND(null_mut()));
                let pixel = GetPixel(screen_dc, pt.x, pt.y);
                let _ = ReleaseDC(HWND(null_mut()), screen_dc);

                let r = (pixel.0 & 0xFF) as u8;
                let g = ((pixel.0 >> 8) & 0xFF) as u8;
                let b = ((pixel.0 >> 16) & 0xFF) as u8;
                let hex = format!("#{:02X}{:02X}{:02X}", r, g, b);

                copy_to_clipboard(hwnd, &hex);
                stop_color_picker(hwnd, s);
                do_hide(hwnd, s);
                return LRESULT(0);
            }

            reset_cursor_blink(hwnd, s);
            let my = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let mx = (lp.0 & 0xFFFF) as i16 as i32;
            let mut rc_client = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc_client);
            let win_w = rc_client.right - rc_client.left;
            let x_start = s.launcher_x(win_w);
            let by = s.launcher_top_y();

            if my >= by && my < by + SEARCH_H {
                if s.shows_lens_action() {
                    let lens_rect = search_lens_action_rect(x_start, by, s.launcher_w, s.search_h());
                    if point_in_rect(mx, my, lens_rect) {
                        start_circle_to_search_from_launcher(hwnd, s);
                        return LRESULT(0);
                    }
                }
                if s.shows_web_search_action() {
                    let web_rect =
                        search_web_action_rect(x_start, by, s.launcher_w, s.search_h(), s.shows_lens_action());
                    if point_in_rect(mx, my, web_rect) {
                        open_query_on_web(hwnd, s);
                        return LRESULT(0);
                    }
                }
                let settings_rect = search_settings_action_rect(
                    x_start,
                    by,
                    s.launcher_w,
                    s.search_h(),
                    s.shows_lens_action(),
                    s.shows_web_search_action(),
                );
                if point_in_rect(mx, my, settings_rect) {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = Command::new(exe).arg("--settings").spawn();
                    }
                    return LRESULT(0);
                }
                s.search_input_active = true;
                if s.ai_answer.is_some() || s.ai_pending || s.chat_input_active {
                    s.chat_input_active = false;
                    close_ai_panel(hwnd, s);
                    return LRESULT(0);
                }
            }

            if !s.query.is_empty() && !s.has_prefix() {
                let list_y = by + SEARCH_H + 1;
                let rects = filter_pill_rects(s, x_start, list_y);
                for (ftype, r) in rects {
                    if mx >= r.left && mx < r.right && my >= r.top && my < r.bottom {
                        if s.active_filter != ftype {
                            s.active_filter = ftype;
                            let mut filtered = s.unfiltered_results.clone();
                            if !matches!(s.active_filter, FilterType::All) {
                                filtered.retain(|r| result_matches_filter(r, s.active_filter));
                            }
                            apply_sort(&mut filtered, s.sort_asc, &s.query);
                            s.results = filtered;
                            s.result_reasons = compute_result_reasons(&s.results);
                            if s.results.is_empty() {
                                s.selected = 0;
                                s.scroll_offset = 0;
                            } else {
                                let vr = s.visible_results();
                                s.scroll_offset = s.scroll_offset.min(s.results.len().saturating_sub(vr));
                            }
                            sync_height_animation(hwnd, s);
                            let _ = InvalidateRect(hwnd, None, FALSE);
                        }
                        return LRESULT(0);
                    }
                }

                // Click on "Best matches first" / "A-Z" chevron sort toggle.
                // on the row list_y + 48 .. list_y + 80.
                let sort_row_top = list_y + 48;
                let sort_row_bot = list_y + 80;
                let sort_x_left = x_start + s.launcher_w / 2;
                let sort_x_right = x_start + s.launcher_w - 12;
                if my >= sort_row_top && my < sort_row_bot && mx >= sort_x_left && mx < sort_x_right {
                    s.sort_asc = !s.sort_asc;
                    apply_sort(&mut s.results, s.sort_asc, &s.query);
                    s.selected = 0;
                    s.scroll_offset = 0;
                    sync_height_animation(hwnd, s);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                    return LRESULT(0);
                }
            }


            if s.submenu_active && mx >= x_start + (s.launcher_w - 240) {
                let start_y = s.launcher_top_y() + SEARCH_H + 16;
                let action_h = 44;
                for idx in 0..3 {
                    let ay = start_y + idx as i32 * (action_h + 8);
                    if my >= ay && my < ay + action_h {
                        s.submenu_selected = idx;
                        execute_submenu_action(hwnd, s);
                        return LRESULT(0);
                    }
                }
                return LRESULT(0);
            } else if s.submenu_active {
                s.submenu_active = false;
                let _ = InvalidateRect(hwnd, None, FALSE);
            }
            let vr = s.visible_results();
            let n = (s.results.len().saturating_sub(s.scroll_offset)).min(vr);
            for i in 0..n {
                let r = s.result_rect(i);
                if my >= r.top && my < r.bottom {
                    s.search_input_active = false;
                    s.cursor_visible = false;
                    s.selected = s.scroll_offset + i;
                    execute_selected(hwnd, s);
                    break;
                }
            }
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;

            if !s.mouse_tracking {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = unsafe { TrackMouseEvent(&mut tme) };
                s.mouse_tracking = true;
            }

            if s.color_picker_active {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut client_pt = pt;
                let _ = ScreenToClient(hwnd, &mut client_pt);
                s.color_picker_mx = client_pt.x;
                s.color_picker_my = client_pt.y;
                let _ = InvalidateRect(hwnd, None, FALSE);
                return LRESULT(0);
            }
            let _mx = (lp.0 & 0xFFFF) as i16 as i32;
            let my = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);

            if pt.x != s.last_mouse_x || pt.y != s.last_mouse_y {
                s.last_mouse_x = pt.x;
                s.last_mouse_y = pt.y;

                let mut rc_client = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc_client);
                let win_w = rc_client.right - rc_client.left;
                let x_start = s.launcher_x(win_w);
                let by = s.launcher_top_y();

                let mut new_search_action_hover = None;
                if my >= by && my < by + SEARCH_H {
                    if s.shows_lens_action() {
                        let r = search_lens_action_rect(x_start, by, s.launcher_w, s.search_h());
                        if point_in_rect(_mx, my, r) {
                            new_search_action_hover = Some(SearchActionHover::Lens);
                        }
                    }
                    if new_search_action_hover.is_none() && s.shows_web_search_action() {
                        let r = search_web_action_rect(
                            x_start,
                            by,
                            s.launcher_w,
                            s.search_h(),
                            s.shows_lens_action(),
                        );
                        if point_in_rect(_mx, my, r) {
                            new_search_action_hover = Some(SearchActionHover::Web);
                        }
                    }
                    if new_search_action_hover.is_none() {
                        let r = search_settings_action_rect(
                            x_start,
                            by,
                            s.launcher_w,
                            s.search_h(),
                            s.shows_lens_action(),
                            s.shows_web_search_action(),
                        );
                        if point_in_rect(_mx, my, r) {
                            new_search_action_hover = Some(SearchActionHover::Settings);
                        }
                    }
                }
                if s.hovered_search_action != new_search_action_hover {
                    s.hovered_search_action = new_search_action_hover;
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }

                if !s.query.is_empty() && !s.has_prefix() {
                    let list_y = by + SEARCH_H + 1;
                    let mut new_hover = None;
                    let rects = filter_pill_rects(s, x_start, list_y);
                    for (ftype, r) in rects {
                        if _mx >= r.left && _mx < r.right && my >= r.top && my < r.bottom {
                            new_hover = Some(ftype);
                            break;
                        }
                    }
                    if s.hovered_filter != new_hover {
                        s.hovered_filter = new_hover;
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }

                let mut new_hover_item = None;
                let vr = s.visible_results();
                let n = (s.results.len().saturating_sub(s.scroll_offset)).min(vr);
                for i in 0..n {
                    let r = s.result_rect(i);
                    if my >= r.top && my < r.bottom {
                        new_hover_item = Some(s.scroll_offset + i);
                        break;
                    }
                }
                if s.hovered_item != new_hover_item {
                    s.hovered_item = new_hover_item;
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
            }
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;
            s.mouse_tracking = false;
            s.hovered_item = None;
            s.hovered_filter = None;
            s.hovered_search_action = None;
            let _ = InvalidateRect(hwnd, None, FALSE);
            LRESULT(0)
        }

        WM_RBUTTONDOWN => {
            if sp.is_null() {
                return LRESULT(0);
            }
            let s = &mut *sp;
            if s.color_picker_active {
                stop_color_picker(hwnd, s);
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_ERASEBKGND => LRESULT(1),

        WM_PAINT => {
            if sp.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            paint(hwnd, &*sp);
            LRESULT(0)
        }

        WM_DESTROY => {
            let _ = unsafe {
                // Kill ALL active timers before freeing State to prevent use-after-free
                // from any pending WM_TIMER messages dispatched after Box::from_raw.
                let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
                let _ = KillTimer(hwnd, TIMER_CURSOR_BLINK);
                let _ = KillTimer(hwnd, TIMER_AI_ANIM);
                let _ = KillTimer(hwnd, TIMER_ICON_BATCH);
                let _ = KillTimer(hwnd, TIMER_SEARCH_ANIM);
            };
            // Clear GWLP_USERDATA before freeing State so any late message that still
            // re-enters wnd_proc_inner gets sp==null and falls through safely.
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !sp.is_null() {
                let s = Box::from_raw(sp);
                if !s.icon_clipboard.0.is_null() {
                    let _ = DestroyIcon(s.icon_clipboard);
                }
                if !s.icon_memory.0.is_null() {
                    let _ = DestroyIcon(s.icon_memory);
                }
                if !s.icon_new_search.0.is_null() {
                    let _ = DestroyIcon(s.icon_new_search);
                }
                let _ = DeleteObject(s.font_q);
                let _ = DeleteObject(s.font_n);
                let _ = DeleteObject(s.font_c);
                let _ = DeleteObject(s.font_b);
                let _ = DeleteObject(s.font_code);
                let _ = DeleteObject(s.font_h);
                if !s.icon_settings.0.is_null() {
                    let _ = DestroyIcon(s.icon_settings);
                }
                if !s.icon_web.0.is_null() {
                    let _ = DestroyIcon(s.icon_web);
                }
                if !s.icon_web_action.0.is_null() {
                    let _ = DestroyIcon(s.icon_web_action);
                }
                if !s.icon_chatgpt.0.is_null() {
                    let _ = DestroyIcon(s.icon_chatgpt);
                }
                if !s.icon_claude.0.is_null() {
                    let _ = DestroyIcon(s.icon_claude);
                }
                if !s.icon_gemini.0.is_null() {
                    let _ = DestroyIcon(s.icon_gemini);
                }
                if !s.icon_grok.0.is_null() {
                    let _ = DestroyIcon(s.icon_grok);
                }
                if !s.icon_eyux.0.is_null() {
                    let _ = DestroyIcon(s.icon_eyux);
                }
                if !s.icon_calculator.0.is_null() {
                    let _ = DestroyIcon(s.icon_calculator);
                }
                if !s.icon_color_picker.0.is_null() {
                    let _ = DestroyIcon(s.icon_color_picker);
                }
                if !s.icon_lens.0.is_null() {
                    let _ = DestroyIcon(s.icon_lens);
                }
                if !s.icon_bookmark.0.is_null() {
                    let _ = DestroyIcon(s.icon_bookmark);
                }
                if !s.icon_folder.0.is_null() {
                    let _ = DestroyIcon(s.icon_folder);
                }
                if !s.icon_file.0.is_null() {
                    let _ = DestroyIcon(s.icon_file);
                }
                if !s.icon_app.0.is_null() {
                    let _ = DestroyIcon(s.icon_app);
                }
                if !s.icon_commit.0.is_null() {
                    let _ = DestroyIcon(s.icon_commit);
                }
                if !s.icon_todo.0.is_null() {
                    let _ = DestroyIcon(s.icon_todo);
                }
                if !s.icon_chrome.0.is_null() {
                    let _ = DestroyIcon(s.icon_chrome);
                }
                if !s.icon_firefox.0.is_null() {
                    let _ = DestroyIcon(s.icon_firefox);
                }
                if !s.icon_edge.0.is_null() {
                    let _ = DestroyIcon(s.icon_edge);
                }
                if !s.icon_brave.0.is_null() {
                    let _ = DestroyIcon(s.icon_brave);
                }
                for (key, &hicon) in &s.app_icons {
                    if s.app_icons_owned.contains(key) && !hicon.0.is_null() {
                        let _ = DestroyIcon(hicon);
                    }
                }
                for &hbmp in s.clipboard_thumbnails.borrow().values() {
                    if !hbmp.0.is_null() {
                        let _ = DeleteObject(hbmp);
                    }
                }
            }
            remove_tray_icon(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }

        WM_TRAYICON => {
            let l_event = lp.0 as u32;
            use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONUP, WM_RBUTTONUP};
            if l_event == WM_LBUTTONUP {
                if !sp.is_null() {
                    let s = &mut *sp;
                    do_show(hwnd, s);
                }
            } else if l_event == WM_RBUTTONUP {
                use windows::Win32::UI::WindowsAndMessaging::{
                    CreatePopupMenu, AppendMenuW, TrackPopupMenu, DestroyMenu, MF_STRING,
                    TPM_RIGHTBUTTON, TPM_BOTTOMALIGN, SetForegroundWindow, WM_NULL
                };
                let hmenu = unsafe { CreatePopupMenu().unwrap() };
                let open_text: Vec<u16> = "Open".encode_utf16().chain(std::iter::once(0)).collect();
                let _ = unsafe { AppendMenuW(hmenu, MF_STRING, 1, PCWSTR(open_text.as_ptr())) };

                let settings_text: Vec<u16> = "Settings".encode_utf16().chain(std::iter::once(0)).collect();
                let _ = unsafe { AppendMenuW(hmenu, MF_STRING, 3, PCWSTR(settings_text.as_ptr())) };

                let exit_text: Vec<u16> = "Exit".encode_utf16().chain(std::iter::once(0)).collect();
                let _ = unsafe { AppendMenuW(hmenu, MF_STRING, 2, PCWSTR(exit_text.as_ptr())) };

                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };

                let _ = unsafe { SetForegroundWindow(hwnd) };

                let selection = unsafe { TrackPopupMenu(hmenu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | windows::Win32::UI::WindowsAndMessaging::TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, None) };
                let _ = unsafe { PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0)) };

                let _ = unsafe { DestroyMenu(hmenu) };

                if selection.0 == 1 {
                    if !sp.is_null() {
                        let s = &mut *sp;
                        do_show(hwnd, s);
                    }
                } else if selection.0 == 2 {
                    // Kill every other instance of this exe (settings windows are separate processes).
                    unsafe {
                        use std::os::windows::process::CommandExt;
                        use windows::Win32::System::Threading::GetCurrentProcessId;
                        if let Some(exe) = std::env::current_exe()
                            .ok()
                            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        {
                            let _ = std::process::Command::new("taskkill")
                                .args(["/F", "/IM", &exe, "/FI",
                                       &format!("PID ne {}", GetCurrentProcessId())])
                                .creation_flags(0x0800_0000)
                                .spawn();
                        }
                    }
                    let _ = unsafe { PostMessageW(hwnd, windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, WPARAM(0), LPARAM(0)) };
                } else if selection.0 == 3 {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe)
                            .arg("--settings")
                            .spawn();
                    }
                }
            }
            LRESULT(0)
        }

        WM_RELOAD_SETTINGS => {
            if !sp.is_null() {
                let s = &mut *sp;
                s.app_settings = crate::settings::AppSettings::load();
                s.theme = theme_from_setting(&s.app_settings.theme_mode);

                // Reload filesystem watcher with new folders list
                let db_path_watcher = match std::env::var("APPDATA") {
                    Ok(d) => std::path::PathBuf::from(d).join("omnisearch-lite").join("file_index.db"),
                    Err(_) => std::path::PathBuf::from("file_index.db"),
                };
                indexer::start_watcher(db_path_watcher);

                // Recreate fonts dynamically
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(s.font_q);
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(s.font_n);
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(s.font_c);
                    s.font_q = create_gdi_font(&s.app_settings.query_font_family, s.app_settings.query_font_size as i32, &s.app_settings.query_font_weight);
                    s.font_n = create_gdi_font(&s.app_settings.result_title_font_family, s.app_settings.result_title_font_size as i32, &s.app_settings.result_title_font_weight);
                    s.font_c = create_gdi_font(&s.app_settings.result_subtitle_font_family, s.app_settings.result_subtitle_font_size as i32, &s.app_settings.result_subtitle_font_weight);
                }

                if !crate::hotkey::register_hotkey(hwnd, HOTKEY_ID, &s.app_settings.global_hotkey) {
                    applog::log(&format!(
                        "launcher hotkey {} registration FAILED (already in use?)",
                        s.app_settings.global_hotkey
                    ));
                } else {
                    applog::log(&format!(
                        "launcher hotkey {} registered",
                        s.app_settings.global_hotkey
                    ));
                }



                if s.query.is_empty() {
                    s.results = default_homepage_results_for_settings();
                    s.selected = s.selected.min(s.results.len().saturating_sub(1));
                    s.homepage_sel = s.homepage_sel.min(s.results.len().saturating_sub(1));
                    s.scroll_offset = 0;
                    s.result_reasons.clear();
                    sync_height_animation(hwnd, s);
                }

                // Parse theme manually if needed, or trigger redraw
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
                }
            }
            LRESULT(0)
        }

        WM_SET_HOTKEY_RECORDING => {
            if !sp.is_null() {
                let s = &mut *sp;
                if wp.0 != 0 {
                    let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
                } else if !crate::hotkey::register_hotkey(hwnd, HOTKEY_ID, &s.app_settings.global_hotkey) {
                    applog::log(&format!(
                        "launcher hotkey {} registration FAILED (already in use?)",
                        s.app_settings.global_hotkey
                    ));
                }
            }
            LRESULT(0)
        }

        WM_OCR_RESULT => {
            // Background MTA thread has finished OCR; retrieve the boxed Option<String>.
            if lp.0 != 0 {
                let result = unsafe { *Box::from_raw(lp.0 as *mut Option<String>) };
                if !sp.is_null() {
                    let s = &mut *sp;
                    match result {
                        Some(text) => {
                            s.query = text;
                            s.cursor_pos = s.query.len();
                            s.reset_results();
                            trigger_search(hwnd, s);
                        }
                        None => {
                            s.query = clipboard_ocr_empty_message().to_string();
                            s.cursor_pos = s.query.len();
                            s.reset_results();
                        }
                    }
                    reset_cursor_blink(hwnd, s);
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
            }
            LRESULT(0)
        }

        WM_LAUNCH_AGENT => {
            let agent_id = wp.0 as i64;
            if !sp.is_null() {
                let s = &mut *sp;
                if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                    if let Ok(name) = conn.query_row(
                        "SELECT name FROM agents WHERE id = ?",
                        [agent_id],
                        |row| row.get::<_, String>(0)
                    ) {
                        let new_title = format!("@{}: [New Conversation]", name);
                        s.ai_pending = false;
                        s.ai_answer = None;
                        s.ai_title = new_title;
                        s.ai_scroll = 0;
                        s.ai_follow_bottom = true;
                        s.active_chat_id = None;
                        s.chat_input.clear();
                        s.chat_cursor_pos = 0;
                        s.chat_input_active = true;
                        s.reset_results();
                        s.selected = 0;
                        s.scroll_offset = 0;
                        sync_height_animation(hwnd, s);
                        do_show(hwnd, s);
                    }
                }
            }
            LRESULT(0)
        }

        windows::Win32::UI::WindowsAndMessaging::WM_COMMAND => {
            let cmd = wp.0 & 0xFFFF;
            if cmd == 1 {
                // Open App
                if !sp.is_null() {
                    let s = &mut *sp;
                    do_show(hwnd, s);
                }
            } else if cmd == 2 {
                // Exit App — close every instance (this launcher + any open settings windows, which
                // run as separate `--settings` processes of the same exe), not just this one.
                unsafe {
                    use std::os::windows::process::CommandExt;
                    use windows::Win32::System::Threading::GetCurrentProcessId;
                    if let Some(exe) = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    {
                        // Kill the other instances; this process then exits cleanly below so its
                        // tray icon is removed properly.
                        let _ = std::process::Command::new("taskkill")
                            .args([
                                "/F",
                                "/IM",
                                &exe,
                                "/FI",
                                &format!("PID ne {}", GetCurrentProcessId()),
                            ])
                            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                            .spawn();
                    }
                }
                let _ = unsafe { PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam.0 as *mut Vec<HMONITOR>);
    monitors.push(hmonitor);
    TRUE
}

unsafe fn get_custom_monitor() -> HMONITOR {
    use windows::Win32::Graphics::Gdi::EnumDisplayMonitors;
    let mut monitors = Vec::<HMONITOR>::new();
    let _ = EnumDisplayMonitors(
        HDC(null_mut()),
        None,
        Some(monitor_enum_proc),
        LPARAM(&mut monitors as *mut _ as isize),
    );
    if monitors.len() > 1 {
        monitors[1] // Use the second monitor
    } else if !monitors.is_empty() {
        monitors[0]
    } else {
        HMONITOR(null_mut())
    }
}

// ── Window lifecycle ──────────────────────────────────────────────────────────
// Takes `s` from the caller instead of re-deriving `&mut State` from GWLP_USERDATA:
// both call sites (do_show, start_hide) already hold a live `&mut State` that they
// use again after this returns, so a second independently-derived reference here
// would alias it — undefined behavior that the LTO/opt-level=3 build is free to
// exploit (e.g. caching a stale field value across the call).
unsafe fn animate_window(hwnd: HWND, s: &mut State, appearing: bool) {
    let start_time = std::time::Instant::now();
    let start_p = match (appearing, &s.anim) {
        (true, Anim::Hiding { .. }) | (false, Anim::Appearing { .. }) => s.current_p(),
        (true, _) => 0.0,
        (false, _) => 1.0,
    };

    if appearing {
        // Save the current foreground window so snippet auto-paste can restore focus to it
        s.prev_foreground = GetForegroundWindow();
        if !(s.ai_pending || s.ai_answer.is_some()) {
            s.query.clear();
            s.cursor_pos = 0;
            s.reset_results();
            s.scroll_offset = 0;
            s.ai_pending = false;
            s.ai_answer = None;
            s.ai_scroll = 0;
            s.ai_follow_bottom = true;
            trigger_search(hwnd, s);
        } else {
            s.selected = 0;
            s.scroll_offset = 0;
            if s.ai_pending {
                let _ = KillTimer(hwnd, TIMER_AI_ANIM);
                let _ = SetTimer(hwnd, TIMER_AI_ANIM, 180, None);
            }
        }

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        // Determine which monitor to place the window on based on Settings
        let hmonitor = match s.app_settings.window_location.as_str() {
            "Remember Last Position" => {
                if s.app_settings.last_win_x != 0 || s.app_settings.last_win_y != 0 {
                    let last_pt = POINT {
                        x: s.app_settings.last_win_x,
                        y: s.app_settings.last_win_y,
                    };
                    MonitorFromPoint(last_pt, MONITOR_DEFAULTTONEAREST)
                } else {
                    MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
                }
            }
            "Monitor with Focused Window" => {
                let fore = GetForegroundWindow();
                if !fore.0.is_null() {
                    use windows::Win32::Graphics::Gdi::MonitorFromWindow;
                    MonitorFromWindow(
                        fore,
                        windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
                    )
                } else {
                    MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
                }
            }
            "Primary Monitor" => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
            "Custom Monitor" => get_custom_monitor(),
            _ => {
                // "Monitor with Mouse Cursor" (default)
                MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
            }
        };

        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (work_w, work_h, work_left, work_top) = if GetMonitorInfoW(hmonitor, &mut mi).as_bool()
        {
            // Save the last opened monitor center coordinates for "Remember Last Position"
            let center_x = mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left) / 2;
            let center_y = mi.rcWork.top + (mi.rcWork.bottom - mi.rcWork.top) / 2;
            if s.app_settings.last_win_x != center_x || s.app_settings.last_win_y != center_y {
                s.app_settings.last_win_x = center_x;
                s.app_settings.last_win_y = center_y;
                s.app_settings.save();
            }

            (
                mi.rcWork.right - mi.rcWork.left,
                mi.rcWork.bottom - mi.rcWork.top,
                mi.rcWork.left,
                mi.rcWork.top,
            )
        } else {
            (
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                0,
                0,
            )
        };

        let launcher_w = launcher_width_for_work_area(work_w, s.app_settings.window_width as i32);
        let win_x = work_left + (work_w - launcher_w) / 2;
        let win_y = work_top;

        applog::log(&format!(
            "animate_window: location_mode='{}' last_x={} last_y={} resolved_x={} resolved_y={} work_w={} work_h={}",
            s.app_settings.window_location,
            s.app_settings.last_win_x,
            s.app_settings.last_win_y,
            win_x,
            win_y,
            work_w,
            work_h
        ));

        // Position and size the physical window to cover the entire work area vertically
        // while keeping the launcher horizontally consistent on every monitor that can fit it.
        s.launcher_w = launcher_w;
        let _ = SetWindowPos(
            hwnd,
            HWND(null_mut()),
            win_x,
            win_y,
            launcher_w,
            work_h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );

        s.cx = launcher_w / 2;
        s.cy = work_h / 2;
        s.last_mouse_x = pt.x;
        s.last_mouse_y = pt.y;

        s.anim = Anim::Appearing {
            start_time,
            start_p,
        };
        let _ = SetLayeredWindowAttributes(hwnd, COLOR_KEY, 0, LWA_COLORKEY | LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        force_foreground(hwnd);
    } else {
        let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
        s.anim = Anim::Hiding {
            start_time,
            start_p,
        };
    }

    let _ = InvalidateRect(hwnd, None, FALSE);
    let _ = SetTimer(hwnd, TIMER_SEARCH_ANIM, 3, None);
    unsafe {
        trigger_anim_loop(hwnd);
    }
}

// AttachThreadInput trick: allows SetForegroundWindow to succeed even from background context.
// Needed when the launcher is summoned by a global hotkey while another app holds focus.
unsafe fn force_foreground(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    let fore = GetForegroundWindow();
    let fore_tid = GetWindowThreadProcessId(fore, None);
    let my_tid = GetCurrentThreadId();
    if fore_tid != 0 && fore_tid != my_tid {
        let _ = AttachThreadInput(fore_tid, my_tid, TRUE);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
        let _ = AttachThreadInput(fore_tid, my_tid, FALSE);
    } else {
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
    }
}

unsafe fn reset_cursor_blink(hwnd: HWND, s: &mut State) {
    let _ = KillTimer(hwnd, TIMER_CURSOR_BLINK);
    if text_caret_active(s) {
        s.cursor_visible = true;
        let _ = SetTimer(hwnd, TIMER_CURSOR_BLINK, CURSOR_BLINK_MS, None);
    } else {
        s.cursor_visible = false;
    }
}

unsafe fn do_show(hwnd: HWND, s: &mut State) {
    raise_timer_res();
    reset_cursor_blink(hwnd, s);
    if s.app_settings.show_taskbar && !s.taskbar_shown_by_app {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
        let class_name: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
        if let Ok(h) = FindWindowW(PCWSTR(class_name.as_ptr()), None) {
            if !h.0.is_null() {
                let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
                let _ = PostMessageW(h, 0x05D1, WPARAM(1), LPARAM(mon.0 as isize));
                s.taskbar_shown_by_app = true;
            }
        }
    }
    animate_window(hwnd, s, true);
    trigger_search(hwnd, s);
}

unsafe fn reset_launcher_window_position(hwnd: HWND, s: &mut State) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmonitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (work_w, work_h, work_left, work_top) = if GetMonitorInfoW(hmonitor, &mut mi).as_bool() {
        (
            mi.rcWork.right - mi.rcWork.left,
            mi.rcWork.bottom - mi.rcWork.top,
            mi.rcWork.left,
            mi.rcWork.top,
        )
    } else {
        (
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
            0,
            0,
        )
    };

    // Written before SetWindowPos: it can synchronously dispatch WM_WINDOWPOSCHANGED etc.
    // back into this wndproc, which would derive its own &mut State from the same
    // GWLP_USERDATA pointer as this function's `s` — writing after would alias it.
    let launcher_w = launcher_width_for_work_area(work_w, s.app_settings.window_width as i32);
    s.launcher_w = launcher_w;
    s.cx = launcher_w / 2;
    s.cy = work_h / 2;
    let _ = SetWindowPos(
        hwnd,
        HWND(null_mut()),
        work_left + (work_w - launcher_w) / 2,
        work_top,
        launcher_w,
        work_h,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    let _ = InvalidateRect(hwnd, None, FALSE);
}

unsafe fn start_hide(hwnd: HWND, s: &mut State) {
    // Destroy the in-app note editor so it doesn't ghost over the launcher
    close_note_editor(hwnd, s);
    animate_window(hwnd, s, false);
    reset_visible_chat_view(hwnd, s);
}

unsafe fn reset_visible_chat_view(hwnd: HWND, s: &mut State) {
    // ponytail: hide only resets what is visible; workers keep updating ai_chats in the background.
    let _ = KillTimer(hwnd, TIMER_AI_ANIM);
    s.ai_pending = false;
    s.ai_answer = None;
    s.ai_title.clear();
    s.ai_scroll = 0;
    s.ai_follow_bottom = true;

    s.ai_tick = 0;
    s.active_chat_id = None;
    s.query.clear();
    s.cursor_pos = 0;
    s.search_input_active = true;
    s.chat_input.clear();
    s.chat_cursor_pos = 0;
    s.chat_input_active = false;
    s.reset_results();
    s.scroll_offset = 0;
    s.text_selected = false;
}

unsafe fn start_color_picker(hwnd: HWND, s: &mut State) {
    s.color_picker_active = true;

    // Get active monitor full bounds
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmonitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (monitor_w, monitor_h, monitor_left, monitor_top) =
        if GetMonitorInfoW(hmonitor, &mut mi).as_bool() {
            (
                mi.rcMonitor.right - mi.rcMonitor.left,
                mi.rcMonitor.bottom - mi.rcMonitor.top,
                mi.rcMonitor.left,
                mi.rcMonitor.top,
            )
        } else {
            (
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                0,
                0,
            )
        };

    // Computed before SetWindowPos (which can synchronously dispatch WM_WINDOWPOSCHANGED
    // etc. back into this wndproc, re-deriving &mut State from the same pointer) so no
    // write here depends on, or races, that reentrant path.
    s.color_picker_mx = pt.x - monitor_left;
    s.color_picker_my = pt.y - monitor_top;

    // Resize and position the window to cover the entire active monitor
    let _ = SetWindowPos(
        hwnd,
        HWND(null_mut()),
        monitor_left,
        monitor_top,
        monitor_w,
        monitor_h,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );

    // Set mouse capture
    let _ = SetCapture(hwnd);
    let _ = InvalidateRect(hwnd, None, FALSE);
}

unsafe fn stop_color_picker(hwnd: HWND, s: &mut State) {
    let _ = ReleaseCapture();
    s.color_picker_active = false;

    // Center the window on the active monitor's work area and reset size
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmonitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (work_w, work_h, work_left, work_top) = if GetMonitorInfoW(hmonitor, &mut mi).as_bool() {
        (
            mi.rcWork.right - mi.rcWork.left,
            mi.rcWork.bottom - mi.rcWork.top,
            mi.rcWork.left,
            mi.rcWork.top,
        )
    } else {
        (
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
            0,
            0,
        )
    };

    let launcher_w = launcher_width_for_work_area(work_w, s.app_settings.window_width as i32);
    let win_x = work_left + (work_w - launcher_w) / 2;
    let win_y = work_top;

    // Written before SetWindowPos for the same reason as start_color_picker above.
    s.launcher_w = launcher_w;
    s.cx = launcher_w / 2;
    s.cy = work_h / 2;

    let _ = SetWindowPos(
        hwnd,
        HWND(null_mut()),
        win_x,
        win_y,
        launcher_w,
        work_h,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );

    let _ = InvalidateRect(hwnd, None, FALSE);
}

unsafe fn do_hide(hwnd: HWND, s: &mut State) {
    let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
    let _ = KillTimer(hwnd, TIMER_CURSOR_BLINK);
    let _ = KillTimer(hwnd, TIMER_SEARCH_ANIM);
    lower_timer_res();
    s.search_loading = false;
    s.form_state = FormState::None;
    s.image_preview_active = false;
    s.cursor_visible = false;
    s.search_input_active = false;
    s.chat_input_active = false;
    hide_preview_window(s);

    if s.taskbar_shown_by_app {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
        let class_name: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
        if let Ok(h) = FindWindowW(PCWSTR(class_name.as_ptr()), None) {
            if !h.0.is_null() {
                let _ = PostMessageW(h, 0x05D1, WPARAM(0), LPARAM(0));
            }
        }
        s.taskbar_shown_by_app = false;
    }

    // Pre-reset query and results to homepage so the next show has data immediately.
    s.query.clear();
    s.cursor_pos = 0;
    s.text_selected = false;
    s.active_filter = FilterType::All;
    s.unfiltered_results = default_homepage_results();
    s.results = s.unfiltered_results.clone();
    s.results_stale = false;
    s.selected = s.homepage_sel.min(s.results.len().saturating_sub(1));
    s.scroll_offset = 0;

    // Write anim state before ShowWindow: hiding the active window can synchronously
    // dispatch WM_ACTIVATE back into this wndproc, which derives its own &mut State from
    // the same GWLP_USERDATA pointer — writing after the call would alias it.
    s.anim = Anim::Hidden;
    let _ = ShowWindow(hwnd, SW_HIDE);
}

fn format_conversation(prompt: &str, response: &str) -> String {
    let prompts: Vec<&str> = prompt
        .split("\n---\n")
        .map(|p| p.strip_prefix("User: ").unwrap_or(p).trim())
        .collect();
    let responses: Vec<&str> = response.split("\n\n---\n\n").collect();

    let mut conversation = String::new();
    for i in 0..prompts.len() {
        if i > 0 {
            conversation.push_str("\n\n---\n\n");
        }
        let p = prompts[i];
        if !p.is_empty() {
            conversation.push_str("User: ");
            conversation.push_str(p);
            conversation.push_str("\n\n");
        }
        if i < responses.len() {
            let r = responses[i].trim();
            if !r.is_empty() {
                conversation.push_str(r);
            }
        }
    }
    if responses.len() > prompts.len() {
        for i in prompts.len()..responses.len() {
            if i > 0 {
                conversation.push_str("\n\n---\n\n");
            }
            conversation.push_str(responses[i].trim());
        }
    }
    conversation
}

#[derive(Clone, Debug)]
enum LocalAgentAction {
    RunPowerShell { command: String },
}

fn local_agent_action(user: &str) -> Option<LocalAgentAction> {
    let text = user.trim();
    let lower = text.to_lowercase();

    for prefix in [
        "run command ",
        "execute command ",
        "run powershell ",
        "powershell ",
        "powershell: ",
        "cmd: ",
    ] {
        if lower.starts_with(prefix) {
            let command = text[prefix.len()..].trim();
            if !command.is_empty() {
                return Some(LocalAgentAction::RunPowerShell {
                    command: command.to_string(),
                });
            }
        }
    }

    // NOTE: the "youtube" / "google" / "search for" fast-paths were removed on purpose.
    // They short-circuited the reasoning agent with a dumb keyword match — e.g. ANY message
    // containing "youtube" opened the generic homepage in the default browser and never reached
    // the AI agent. Real requests like "open MrWhosTheBoss on Brave" now fall through to the
    // agent, which reasons about the intent and executes the actual action via its tools.
    None
}

fn run_local_agent_action(action: LocalAgentAction) -> (bool, String) {
    match action {
        LocalAgentAction::RunPowerShell { command } => {
            let output = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &command,
                ])
                .creation_flags(0x08000000)
                .output();
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let mut text = format!("Executed command:\n`{command}`");
                    if !stdout.is_empty() {
                        text.push_str("\n\nOutput:\n");
                        text.push_str(&stdout);
                    }
                    if !stderr.is_empty() {
                        text.push_str("\n\nError output:\n");
                        text.push_str(&stderr);
                    }
                    if stdout.is_empty() && stderr.is_empty() {
                        text.push_str("\n\nNo output.");
                    }
                    (out.status.success(), text)
                }
                Err(e) => (false, format!("Failed to execute command `{command}`: {e}")),
            }
        }
    }
}

fn ensure_agent_memory_table(conn: &rusqlite::Connection) {
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_memory (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_name TEXT NOT NULL,
            content TEXT NOT NULL,
            source TEXT,
            ts INTEGER NOT NULL,
            UNIQUE(agent_name, content)
        );",
        [],
    );
}

fn extract_agent_memory_items(input: &str) -> Vec<String> {
    let text = input.trim();
    let lower = text.to_lowercase();
    let mut out = Vec::new();

    for marker in ["remember that ", "remember "] {
        if let Some(rest) = lower
            .find(marker)
            .map(|idx| text[idx + marker.len()..].trim())
        {
            if !rest.is_empty() {
                out.push(rest.trim_matches(['.', '!', '?']).to_string());
                return out;
            }
        }
    }

    for marker in [
        "my name is ",
        "i am ",
        "i'm ",
        "i like ",
        "i love ",
        "i prefer ",
        "i use ",
    ] {
        if let Some(rest) = lower.find(marker).map(|idx| text[idx..].trim()) {
            if rest.len() <= 160 {
                out.push(format!("User said: {}", rest.trim_matches(['.', '!', '?'])));
                return out;
            }
        }
    }

    out
}

fn remember_agent_user_facts(db_path: &std::path::Path, agent_name: &str, input: &str) {
    if agent_name.trim().is_empty() {
        return;
    }
    let items = extract_agent_memory_items(input);
    if items.is_empty() {
        return;
    }
    if let Ok(conn) = rusqlite::Connection::open(db_path) {
        ensure_agent_memory_table(&conn);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        for item in items {
            let _ = conn.execute(
                "INSERT INTO agent_memory (agent_name, content, source, ts)
                 VALUES (?, ?, 'chat', ?)
                 ON CONFLICT(agent_name, content) DO UPDATE SET ts = excluded.ts;",
                rusqlite::params![agent_name, item, now],
            );
        }
    }
}

fn agent_memory_context(db_path: &std::path::Path, agent_name: &str) -> String {
    if agent_name.trim().is_empty() {
        return String::new();
    }
    let Ok(conn) = rusqlite::Connection::open(db_path) else {
        return String::new();
    };
    ensure_agent_memory_table(&conn);
    let Ok(mut stmt) = conn.prepare(
        "SELECT content FROM agent_memory
         WHERE lower(agent_name) = lower(?)
         ORDER BY ts DESC
         LIMIT 24;",
    ) else {
        return String::new();
    };
    let memories: Vec<String> = stmt
        .query_map([agent_name], |row| row.get::<_, String>(0))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    if memories.is_empty() {
        return String::new();
    }
    let mut text = String::from("\n\nKnown user memory for this agent. Use only when relevant:\n");
    for memory in memories.iter().rev() {
        text.push_str("- ");
        text.push_str(memory);
        text.push('\n');
    }
    text
}

fn system_prompt_with_agent_memory(
    db_path: &std::path::Path,
    agent_name: &str,
    system_prompt: &str,
) -> String {
    let memory = agent_memory_context(db_path, agent_name);
    if memory.is_empty() {
        system_prompt.to_string()
    } else {
        format!("{system_prompt}{memory}")
    }
}

/// Bucketed relative-age tag ("just now", "2h ago", "yesterday", "3w ago").
/// ponytail: buckets, no calendar lib — recency is the signal, exact dates aren't needed.
fn relative_age(age_secs: i64) -> String {
    match age_secs {
        s if s < 0 => String::new(),
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s if s < 172800 => "yesterday".to_string(),
        s if s < 604800 => format!("{}d ago", s / 86400),
        s if s < 2592000 => format!("{}w ago", s / 604800),
        s if s < 31536000 => format!("{}mo ago", s / 2592000),
        s => format!("{}y ago", s / 31536000),
    }
}

fn relative_time(modified: std::time::SystemTime) -> String {
    match std::time::SystemTime::now().duration_since(modified) {
        Ok(d) => relative_age(d.as_secs() as i64),
        Err(_) => String::new(), // mtime in the future — skip
    }
}

fn clip_timestamp_from_id(id: &str) -> Option<i64> {
    id.rsplit('.').next()?.parse::<i64>().ok()
}

fn clip_timestamp_to_unix_seconds(ts: i64) -> i64 {
    if ts > 10_000_000_000 {
        ts / 1000
    } else {
        ts
    }
}

fn clip_id_for_pin_state(ts: i64, pinned: bool) -> String {
    if pinned {
        format!("clip.pinned.{}", ts)
    } else {
        format!("clip.{}", ts)
    }
}

fn selected_clip_ids_contain(
    selected: &std::collections::HashSet<String>,
    candidate_id: &str,
) -> bool {
    selected.contains(candidate_id)
        || clip_timestamp_from_id(candidate_id).is_some_and(|candidate_ts| {
            selected
                .iter()
                .any(|id| clip_timestamp_from_id(id) == Some(candidate_ts))
        })
}

fn selected_clip_timestamps(
    selected: &std::collections::HashSet<String>,
    fallback_id: Option<&str>,
) -> Vec<i64> {
    let mut timestamps: Vec<i64> = if selected.is_empty() {
        fallback_id
            .into_iter()
            .filter_map(clip_timestamp_from_id)
            .collect()
    } else {
        selected
            .iter()
            .filter_map(|id| clip_timestamp_from_id(id))
            .collect()
    };
    timestamps.sort_unstable();
    timestamps.dedup();
    timestamps
}

/// "Explain results": map each result's launch_command to a "why it surfaced" recency tag.
/// Files use mtime; clipboard entries use the unix ts embedded in their id (e.g. clip.<ts>).
/// ponytail: stat/parse per result on arrival (≤30, debounced) — not in the paint loop.
fn compute_result_reasons(results: &[SearchResult]) -> std::collections::HashMap<String, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut map = std::collections::HashMap::new();
    for r in results {
        let cmd = &r.entry.launch_command;
        if map.contains_key(cmd) {
            continue;
        }
        let tag = if r.entry.source == "CLIPBOARD" {
            // id is clip.<ts> or clip.pinned.<ts>
            clip_timestamp_from_id(&r.entry.id)
                .map(clip_timestamp_to_unix_seconds)
                .map(|ts| relative_age(now - ts))
                .unwrap_or_default()
        } else {
            // Files use mtime. Notes wrap the path as open_note:<path>.
            let path = cmd.strip_prefix("open_note:").unwrap_or(cmd);
            std::fs::metadata(path)
                .and_then(|md| md.modified())
                .map(relative_time)
                .unwrap_or_default()
        };
        if !tag.is_empty() {
            map.insert(cmd.clone(), tag);
        }
    }
    map
}

fn store_ai_chat(
    db_path: &std::path::Path,
    command: &str,
    title: &str,
    prompt: &str,
    response: &str,
) -> Option<i64> {
    if let Ok(conn) = rusqlite::Connection::open(db_path) {
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS ai_chats (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER, \
                command TEXT, title TEXT, prompt TEXT, response TEXT);",
            [],
        );
        let _ = conn.execute("ALTER TABLE ai_chats ADD COLUMN run_id TEXT;", []);
        let _ = conn.execute("ALTER TABLE ai_chats ADD COLUMN pending_approval TEXT;", []);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let _ = conn.execute(
            "INSERT INTO ai_chats (ts, command, title, prompt, response) VALUES (?,?,?,?,?);",
            rusqlite::params![now, command, title, prompt, response],
        );
        let id = conn.last_insert_rowid();
        return Some(id);
    }
    None
}

fn ensure_default_agents(db_path: &std::path::Path) {
    if let Ok(conn) = rusqlite::Connection::open(db_path) {
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS agents (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, goal TEXT, \
                system_prompt TEXT, ts INTEGER);",
            [],
        );
        let _ = conn.execute(
            "DELETE FROM agents WHERE lower(name) = 'hermes';",
            [],
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE lower(name) = 'ai'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if count == 0 {
            let name = "AI";
            let goal = "Answer general queries and assist with coding, translation, and explanations";
            let system_prompt = default_agent_system_prompt();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let _ = conn.execute(
                "INSERT INTO agents (name, goal, system_prompt, ts) VALUES (?,?,?,?);",
                rusqlite::params![name, goal, system_prompt, now],
            );
        } else {
            let old_prompt = "You are a helpful AI assistant. Be concise and proactive.";
            let _ = conn.execute(
                "UPDATE agents SET goal = ?, system_prompt = ? \
                 WHERE lower(name) = 'ai' AND system_prompt = ?;",
                rusqlite::params![
                    "Answer general queries and assist with coding, translation, and explanations",
                    default_agent_system_prompt(),
                    old_prompt
                ],
            );
        }
    }
}

fn default_agent_system_prompt() -> &'static str {
    "You are a helpful AI assistant. Answer directly, concisely, and helpfully."
}

fn create_agent(db_path: &std::path::Path, name: &str, goal: &str) {
    if let Ok(conn) = rusqlite::Connection::open(db_path) {
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS agents (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, goal TEXT, \
                system_prompt TEXT, ts INTEGER);",
            [],
        );
        let system_prompt = if goal.is_empty() {
            format!("You are {name}, a helpful AI assistant. Be concise and proactive.")
        } else {
            format!("You are {name}, an AI assistant. Your goal: {goal}. Be concise, helpful, and proactive in pursuing this goal.")
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let _ = conn.execute(
            "INSERT INTO agents (name, goal, system_prompt, ts) VALUES (?,?,?,?);",
            rusqlite::params![name, goal, system_prompt, now],
        );
    }
}



fn start_follow_up_chat(hwnd: HWND, s: &mut State, follow_up: String) {
    start_ai_activity(hwnd, s);
    let prev_ans = s.ai_answer.clone().unwrap_or_default();
    let new_prompt_str = follow_up.clone();
    if !prev_ans.is_empty() {
        s.ai_answer = Some(format!(
            "{}\n\n---\n\nUser: {}\n\nExecuting...",
            prev_ans, new_prompt_str
        ));
    } else {
        s.ai_answer = Some(format!("User: {}\n\nExecuting...", new_prompt_str));
    }
    s.ai_scroll = 0;
    s.ai_follow_bottom = true;
    s.reset_results();
    s.selected = 0;
    s.chat_input.clear();
    s.chat_cursor_pos = 0;
    s.chat_input_active = true;
    let _ = unsafe { InvalidateRect(hwnd, None, FALSE) };

    let hwnd_raw = hwnd.0 as isize;
    let db_path = s.db_path.clone();

    if s.active_chat_id.is_none() {
        let (cmd_type, title_str) = if s.ai_title.starts_with("@") {
            ("agent", s.ai_title.clone())
        } else {
            ("ask", "Follow-up Chat".to_string())
        };
        s.active_chat_id = store_ai_chat(&db_path, cmd_type, &title_str, "", "");
    }
    let chat_id = s.active_chat_id;
    let new_prompt = follow_up;

    std::thread::spawn(move || {
        let mut original_prompt = String::new();
        let mut original_response = String::new();
        let mut command = "ask".to_string();
        let mut title = "Follow-up Chat".to_string();

        if let Some(id) = chat_id {
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                let _ = conn.query_row(
                    "SELECT command, title, prompt, response FROM ai_chats WHERE id = ?",
                    [id],
                    |row| {
                        let cmd_str: String = row.get(0)?;
                        let title_str: String = row.get(1)?;
                        let p_str: String = row.get(2)?;
                        let r_str: String = row.get(3)?;
                        command = cmd_str;
                        title = title_str;
                        original_prompt = p_str;
                        original_response = r_str;
                        Ok(())
                    },
                );
                // Immediately write the "Executing..." state to DB now that we have the original values.
                let updated_prompt = if original_prompt.is_empty() {
                    new_prompt.clone()
                } else {
                    format!("{}\n---\nUser: {}", original_prompt, new_prompt)
                };
                let updated_response = if original_response.is_empty() {
                    "Executing...".to_string()
                } else {
                    format!("{}\n\n---\n\nExecuting...", original_response)
                };
                let _ = conn.execute(
                    "UPDATE ai_chats SET prompt = ?, response = ? WHERE id = ?",
                    rusqlite::params![updated_prompt, updated_response, id],
                );
            }
        }

        let mut system_prompt = "You are a concise, helpful assistant. Answer directly in at most a few short paragraphs.".to_string();
        let mut agent_name = String::new();
        if command == "agent" {
            if let Some(name) = title
                .strip_prefix('@')
                .and_then(|t| t.split_once(':'))
                .map(|(n, _)| n.trim())
            {
                agent_name = name.to_string();
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    let _ = conn.query_row(
                        "SELECT system_prompt FROM agents WHERE lower(name) = lower(?)",
                        [name],
                        |row| {
                            let sp: String = row.get(0)?;
                            system_prompt = sp;
                            Ok(())
                        },
                    );
                }
            }
            remember_agent_user_facts(&db_path, &agent_name, &new_prompt);
            system_prompt = system_prompt_with_agent_memory(&db_path, &agent_name, &system_prompt);
            if let Some(action) = local_agent_action(&new_prompt) {
                let (ok, text) = run_local_agent_action(action);
                if let Some(id) = chat_id {
                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                        let updated_prompt = if original_prompt.is_empty() {
                            new_prompt.clone()
                        } else {
                            format!("{}\n---\nUser: {}", original_prompt, new_prompt)
                        };
                        let updated_response = if original_response.is_empty() {
                            text.clone()
                        } else {
                            format!("{}\n\n---\n\n{}", original_response, text)
                        };
                        let _ = conn.execute(
                            "UPDATE ai_chats SET prompt = ?, response = ?, pending_approval = NULL WHERE id = ?",
                            rusqlite::params![updated_prompt, updated_response, id],
                        );
                    }
                }
                let prev_history = format_conversation(&original_prompt, &original_response);
                let formatted = if prev_history.trim().is_empty() {
                    if ok {
                        format!("User: {}\n\n{}", new_prompt, text)
                    } else {
                        format!("User: {}\n\n⚠ {}", new_prompt, text)
                    }
                } else if ok {
                    format!(
                        "{}\n\n---\n\nUser: {}\n\n{}",
                        prev_history, new_prompt, text
                    )
                } else {
                    format!(
                        "{}\n\n---\n\nUser: {}\n\n⚠ {}",
                        prev_history, new_prompt, text
                    )
                };
                let ptr = Box::into_raw(Box::new((ok, formatted))) as isize;
                unsafe {
                    let wp_chat_id = chat_id.unwrap_or(0) as usize;
                    let _ = PostMessageW(
                        HWND(hwnd_raw as *mut _),
                        WM_AI_RESULT,
                        WPARAM(wp_chat_id),
                        LPARAM(ptr),
                    );
                }
                return;
            }
        } else {
            system_prompt = match command.as_str() {
                "explain" => "Explain the following clearly and simply for a general audience. Be concise.".to_string(),
                "grammar" => "Fix the spelling and grammar of the text. Output ONLY the corrected text, with no preamble or quotes.".to_string(),
                "translate" => "You are a translator. If the input names a target language (e.g. 'X to Spanish'), translate X into it; otherwise translate the text to English. Output ONLY the translation.".to_string(),
                "summarize" => "Summarize the following text concisely as a few short bullet points.".to_string(),
                "bugs" => "You are a code reviewer. List likely bugs and issues in the following code as short bullet points. Be specific.".to_string(),
                _ => system_prompt
            };
        }



        let result = if command == "agent" {
            ai::complete_chat_agent(
                &system_prompt,
                &original_prompt,
                &original_response,
                &new_prompt,
            )
        } else {
            ai::complete_chat(
                &system_prompt,
                &original_prompt,
                &original_response,
                &new_prompt,
            )
        };

        if let Ok(ref new_response) = result {
            if let Some(id) = chat_id {
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    let updated_prompt = if original_prompt.is_empty() {
                        new_prompt.clone()
                    } else {
                        format!(
                            "{}
---
User: {}",
                            original_prompt, new_prompt
                        )
                    };
                    let updated_response = if original_response.is_empty() {
                        new_response.clone()
                    } else {
                        format!(
                            "{}

---

{}",
                            original_response, new_response
                        )
                    };
                    let _ = conn.execute(
                        "UPDATE ai_chats SET prompt = ?, response = ? WHERE id = ?",
                        rusqlite::params![updated_prompt, updated_response, id],
                    );
                }
            }
        }

        let payload: (bool, String) = match result {
            Ok(ref new_response) => {
                let updated_prompt = if original_prompt.is_empty() {
                    new_prompt.clone()
                } else {
                    format!(
                        "{}
---
User: {}",
                        original_prompt, new_prompt
                    )
                };
                let updated_response = if original_response.is_empty() {
                    new_response.clone()
                } else {
                    format!(
                        "{}

---

{}",
                        original_response, new_response
                    )
                };
                let full_history_resp = format_conversation(&updated_prompt, &updated_response);
                (true, full_history_resp)
            }
            Err(e) => (false, e.to_string()),
        };
        let ptr = Box::into_raw(Box::new(payload)) as isize;
        unsafe {
            let wp_chat_id = chat_id.unwrap_or(0) as usize;
            let _ = PostMessageW(
                HWND(hwnd_raw as *mut _),
                WM_AI_RESULT,
                WPARAM(wp_chat_id),
                LPARAM(ptr),
            );
        }
    });
}

fn start_ai_activity(hwnd: HWND, s: &mut State) {
    s.ai_pending = true;
    s.ai_tick = 0;
    unsafe {
        let _ = KillTimer(hwnd, TIMER_AI_ANIM);
        let _ = SetTimer(hwnd, TIMER_AI_ANIM, 180, None);
    }
}

unsafe fn close_ai_panel(hwnd: HWND, s: &mut State) {
    let _ = KillTimer(hwnd, TIMER_AI_ANIM);
    s.ai_pending = false;
    s.ai_answer = None;
    s.ai_title.clear();
    s.ai_scroll = 0;
    s.ai_follow_bottom = true;
    s.ai_tick = 0;
    s.active_chat_id = None;
    s.chat_input.clear();
    s.chat_cursor_pos = 0;
    s.chat_input_active = false;
    s.reset_results();
    trigger_search(hwnd, s);
    let _ = InvalidateRect(hwnd, None, FALSE);
}

unsafe fn close_ai_panel_to_agent_history(hwnd: HWND, s: &mut State) {
    close_ai_panel(hwnd, s);
    s.query = "agentchats:".to_string();
    s.cursor_pos = s.query.len();
    s.selected = 0;
    s.scroll_offset = 0;
    s.active_filter = FilterType::All;
    s.filter_scroll_x = 0;
    trigger_search(hwnd, s);
}

// Scroll the AI panel up by `step` pixels. Scrolling up always leaves "follow the
// latest message" mode so the view stays where the user put it.
fn ai_scroll_up(s: &mut State, step: i32) {
    s.ai_follow_bottom = false;
    s.ai_scroll = (s.ai_scroll - step).max(0);
}

// Scroll the AI panel down by `step` pixels. If this lands at (or past) the bottom,
// re-enter follow-bottom mode so future messages auto-scroll again.
fn ai_scroll_down(s: &mut State, step: i32) {
    let total = s.ai_content_height.get();
    let view = s.ai_view_height.get();
    let max_scroll = (total - view).max(0);
    s.ai_scroll = (s.ai_scroll + step).min(max_scroll);
    if s.ai_scroll >= max_scroll {
        s.ai_follow_bottom = true;
    }
}

fn plugin_execution_allowed(source: &str, cmd: &str) -> bool {
    let settings = crate::settings::AppSettings::load();
    if source == "CALC" {
        return settings.plugin_calculator;
    }
    if source == "SNIPPET" || cmd.starts_with("copy_snippet:") {
        return settings.plugin_text_expansions;
    }
    if cmd == "action:color_picker" {
        return settings.plugin_color_picker;
    }
    if cmd == "action:circle_to_search" {
        return settings.plugin_circle_search;
    }
    true
}

unsafe fn execute_selected(hwnd: HWND, s: &mut State) {
    if s.results_stale {
        let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
        trigger_search(hwnd, s);
        return;
    }
    if let Some(r) = s.results.get(s.selected) {
        let cmd = r.entry.launch_command.clone();
        if cmd == "sync:phone" {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).args(["--settings", "--page=8"]).spawn();
            }
            do_hide(hwnd, s);
            return;
        }
        if !plugin_execution_allowed(&r.entry.source, &cmd) {
            s.query = "Plugin is disabled in Settings".to_string();
            s.cursor_pos = s.query.len();
            s.reset_results();
            let _ = InvalidateRect(hwnd, None, FALSE);
            return;
        }
        let ctrl_name = r.entry.control_name.clone();
        // A scope command navigates the launcher into that scope. These exact strings are
        // never real launch targets, so trigger on the command alone — not the result's
        // source (the "Search Notes" quick-action isn't a FOLDER but still uses "notes:").
        let is_action_folder = cmd == "bookmarks:"
            || cmd == "history:"
            || cmd == "commits:"
            || cmd == "todos:"
            || cmd == "clip:"
            || cmd == "file:"
            || cmd == "code:"
            || cmd == "switch:"
            || cmd == "window:"
            || cmd == "ql:"
            || cmd == "snip:"
            || cmd == "img:"
            || cmd == "memory:"
            || cmd == "chats:"
            || cmd == "agents:"
            || cmd == "agentchats:"
            || cmd == "notes:";
        if is_action_folder {
            s.query = cmd;
            s.cursor_pos = s.query.len();
            s.selected = 0;
            s.scroll_offset = 0;
            s.text_selected = false;
            reset_cursor_blink(hwnd, s);
            trigger_search(hwnd, s);
        } else if let Some(rest) = cmd.strip_prefix("ai:") {
            // Run an AI command on a worker thread; show the answer in the AI panel.
            let (aicmd, inline) = rest.split_once(':').unwrap_or((rest, ""));
            let aicmd = aicmd.to_string();
            let mut input = if inline.trim().is_empty() {
                paste_from_clipboard(hwnd).unwrap_or_default()
            } else {
                inline.to_string()
            };
            if input.len() > 30000 {
                input =
                    input.chars().take(30000).collect::<String>() + "\n\n[Truncated for length...]";
            }
            start_ai_activity(hwnd, s);
            s.ai_answer = Some(format!("User: {}\n\nExecuting...", input));
            s.ai_scroll = 0;
            s.ai_follow_bottom = true;
            s.ai_title = ctrl_name;
            s.reset_results();
            s.selected = 0;
            let _ = InvalidateRect(hwnd, None, FALSE);

            let hwnd_ai = SendHwnd(hwnd);
            let db_path = s.db_path.clone();
            let title = s.ai_title.clone();
            let aicmd_clone = aicmd.clone();
            let input_clone = input.clone();

            // Store chat in DB immediately to get a chat ID
            let chat_id =
                store_ai_chat(&db_path, &aicmd_clone, &title, &input_clone, "Executing...");
            s.active_chat_id = chat_id;
            s.chat_input.clear();
            s.chat_cursor_pos = 0;
            s.chat_input_active = true;

            std::thread::spawn(move || {
                let hwnd_ai = hwnd_ai;
                let result = ai::run(&aicmd_clone, &input_clone);
                if let Ok(ref text) = result {
                    if let Some(id) = chat_id {
                        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                            let _ = conn.execute(
                                "UPDATE ai_chats SET response = ? WHERE id = ?",
                                rusqlite::params![text, id],
                            );
                        }
                    }
                }
                let payload: (bool, String) = match result {
                    Ok(text) => (true, format_conversation(&input_clone, &text)),
                    Err(e) => (false, e.to_string()),
                };
                let ptr = Box::into_raw(Box::new(payload)) as isize;
                unsafe {
                    let wp_chat_id = chat_id.unwrap_or(0) as usize;
                    let _ = PostMessageW(hwnd_ai.0, WM_AI_RESULT, WPARAM(wp_chat_id), LPARAM(ptr));
                }
            });
            return;
        } else if let Some(id_str) = cmd.strip_prefix("aichat:") {
            // Reopen a stored chat in the panel, reconnecting to its run if still active.
            if let Ok(id) = id_str.parse::<i64>() {
                if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                    let _ = conn.execute("ALTER TABLE ai_chats ADD COLUMN run_id TEXT;", []);
                    let _ =
                        conn.execute("ALTER TABLE ai_chats ADD COLUMN pending_approval TEXT;", []);
                    if let Ok((title, prompt, response)) = conn.query_row(
                        "SELECT title, prompt, response FROM ai_chats WHERE id = ?",
                        [id],
                        |row| Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        )),
                    ) {
                        s.ai_title = title;
                        s.ai_scroll = 0;
                        s.ai_follow_bottom = true;
                        s.active_chat_id = Some(id);
                        s.chat_input.clear();
                        s.chat_cursor_pos = 0;
                        s.chat_input_active = true;
                        s.reset_results();
                        s.selected = 0;

                        s.ai_pending = false;
                        s.ai_answer = Some(format_conversation(&prompt, &response));
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
            }
            return;
        } else if let Some(rest) = cmd.strip_prefix("mkagent:") {
            // Create a new agent, then jump to the agents list.
            let mut it = rest.splitn(2, '\u{1f}');
            let name = it.next().unwrap_or("").to_string();
            let goal = it.next().unwrap_or("").to_string();
            if !name.is_empty() {
                create_agent(&s.db_path, &name, &goal);
            }
            s.query = "agents:".to_string();
            s.cursor_pos = s.query.len();
            s.selected = 0;
            s.scroll_offset = 0;
            reset_cursor_blink(hwnd, s);
            trigger_search(hwnd, s);
            return;
        } else if let Some(rest) = cmd.strip_prefix("openagent:") {
            let mut parts = rest.splitn(2, '\u{1f}');
            let _agent_id: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(-1);
            let name = parts.next().unwrap_or("").to_string();
            if !name.is_empty() {
                let new_title = format!("@{}: [New Conversation]", name);
                s.ai_pending = false;
                s.ai_answer = None;
                s.ai_title = new_title;
                s.ai_scroll = 0;
                s.ai_follow_bottom = true;
                s.active_chat_id = None;
                s.chat_input.clear();
                s.chat_cursor_pos = 0;
                s.chat_input_active = true;
                s.reset_results();
                s.selected = 0;
                s.scroll_offset = 0;
                sync_height_animation(hwnd, s);
                let _ = InvalidateRect(hwnd, None, FALSE);
            }
            return;
        } else if let Some(name) = cmd.strip_prefix("startnewagent:") {
            let new_title = format!("@{}: [New Conversation]", name);
            s.ai_pending = false;
            s.ai_answer = None;
            s.ai_title = new_title;
            s.ai_scroll = 0;
            s.ai_follow_bottom = true;
            s.active_chat_id = None;
            s.chat_input.clear();
            s.chat_cursor_pos = 0;
            s.chat_input_active = true;
            s.reset_results();
            s.selected = 0;
            s.scroll_offset = 0;
            s.text_selected = false;
            reset_cursor_blink(hwnd, s);
            sync_height_animation(hwnd, s);
            let _ = InvalidateRect(hwnd, None, FALSE);
            return;
        } else if let Some(rest) = cmd.strip_prefix("agent:") {
            // Message an agent: run the AI with the agent's persona, show in the panel.
            let mut it = rest.splitn(2, '\u{1f}');
            let id: i64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-1);
            let msg = it.next().unwrap_or("").to_string();
            let (mut aname, mut sys) = (String::new(), String::new());
            if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
                if let Ok((n, sp)) = conn.query_row(
                    "SELECT name, system_prompt FROM agents WHERE id = ?",
                    [id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                ) {
                    aname = n;
                    sys = sp;
                }
            }
            if sys.is_empty() || msg.is_empty() {
                return;
            }
            remember_agent_user_facts(&s.db_path, &aname, &msg);
            sys = system_prompt_with_agent_memory(&s.db_path, &aname, &sys);
            start_ai_activity(hwnd, s);
            s.ai_answer = Some(format!("User: {}\n\nExecuting...", msg));
            s.ai_scroll = 0;
            s.ai_follow_bottom = true;
            s.ai_title = format!("@{}: {}", aname, msg);
            s.reset_results();
            s.selected = 0;
            let _ = InvalidateRect(hwnd, None, FALSE);

            let hwnd_ai = SendHwnd(hwnd);
            let db_path = s.db_path.clone();
            let title = s.ai_title.clone();
            let msg_clone = msg.clone();

            // Store chat in DB immediately to get a chat ID
            let chat_id = store_ai_chat(&db_path, "agent", &title, &msg_clone, "Executing...");
            s.active_chat_id = chat_id;
            s.chat_input.clear();
            s.chat_cursor_pos = 0;
            s.chat_input_active = true;

            if let Some(action) = local_agent_action(&msg_clone) {
                let hwnd_raw = hwnd.0 as isize;
                let db_path_local = db_path.clone();
                let msg_local = msg_clone.clone();
                std::thread::spawn(move || {
                    let (ok, text) = run_local_agent_action(action);
                    if let Some(id) = chat_id {
                        if let Ok(conn) = rusqlite::Connection::open(&db_path_local) {
                            let _ = conn.execute(
                                "UPDATE ai_chats SET response = ?, pending_approval = NULL WHERE id = ?",
                                rusqlite::params![text, id],
                            );
                        }
                    }
                    let payload = if ok {
                        (true, format_conversation(&msg_local, &text))
                    } else {
                        (
                            false,
                            format_conversation(&msg_local, &format!("⚠ {}", text)),
                        )
                    };
                    let ptr = Box::into_raw(Box::new(payload)) as isize;
                    unsafe {
                        let wp_chat_id = chat_id.unwrap_or(0) as usize;
                        let _ = PostMessageW(
                            HWND(hwnd_raw as *mut _),
                            WM_AI_RESULT,
                            WPARAM(wp_chat_id),
                            LPARAM(ptr),
                        );
                    }
                });
                return;
            }



            std::thread::spawn(move || {
                let hwnd_ai = hwnd_ai;
                let result = ai::complete_agent(&sys, &msg_clone);
                if let Ok(ref text) = result {
                    if let Some(id) = chat_id {
                        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                            let _ = conn.execute(
                                "UPDATE ai_chats SET response = ? WHERE id = ?",
                                rusqlite::params![text, id],
                            );
                        }
                    }
                }
                let payload: (bool, String) = match result {
                    Ok(text) => (true, format_conversation(&msg_clone, &text)),
                    Err(e) => (false, e.to_string()),
                };
                let ptr = Box::into_raw(Box::new(payload)) as isize;
                unsafe {
                    let wp_chat_id = chat_id.unwrap_or(0) as usize;
                    let _ = PostMessageW(hwnd_ai.0, WM_AI_RESULT, WPARAM(wp_chat_id), LPARAM(ptr));
                }
            });
            return;
        } else {
            if let Some(new_query) = cmd.strip_prefix("query:") {
                s.query = new_query.to_string();
                s.cursor_pos = s.query.len();
                s.selected = 0;
                s.scroll_offset = 0;
                s.text_selected = false;
                reset_cursor_blink(hwnd, s);
                trigger_search(hwnd, s);
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if let Some(theme_name) = cmd.strip_prefix("action:switch_theme:") {
                match theme_name {
                    "darker" => {
                        s.theme = Theme::Darker;
                        s.app_settings.theme_mode = "Darker".to_string();
                        s.app_settings.save();
                    }
                    "nord" => {
                        s.theme = Theme::NordDarker;
                        s.app_settings.theme_mode = "NordDarker".to_string();
                        s.app_settings.save();
                    }
                    "light" => {
                        s.theme = Theme::Light;
                        s.app_settings.theme_mode = "Light".to_string();
                        s.app_settings.save();
                    }
                    "omni" => {
                        s.theme = Theme::Omni;
                        s.app_settings.theme_mode = "Omni".to_string();
                        s.app_settings.save();
                    }
                    _ => {}
                }
                s.query = format!("Theme: {}", theme_name.to_uppercase());
                s.cursor_pos = s.query.len();
                s.reset_results();
                s.selected = 0;
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if let Some(config_action) = cmd.strip_prefix("action:ai_config:") {
                let db_path = s.db_path.clone();
                if config_action == "reset" {
                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                        let _ = conn.execute("DELETE FROM ai_settings", []);
                    }
                    s.query = "AI Config Reset!".to_string();
                } else if let Some((k, v)) = config_action.split_once(':') {
                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                        let _ = conn.execute(
                            "CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT);",
                            [],
                        );
                        if k == "preset" {
                            if let Some(provider) = ai::find_provider(v) {
                                let _ = conn.execute("INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('endpoint', ?);", rusqlite::params![provider.endpoint]);
                                let _ = conn.execute("INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('model', ?);", rusqlite::params![provider.default_model]);
                                s.query = format!("AI Configured for {}!", provider.name);
                            }
                        } else {
                            let db_key = if k == "key" { "api_key" } else { k };
                            let _ = conn.execute(
                                "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?, ?);",
                                rusqlite::params![db_key, v],
                            );
                            if db_key == "api_key" {
                                if v.trim().starts_with("sk-oc-")
                                {
                                    let _ = conn.execute("INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('endpoint', 'https://opencode.ai/zen/v1/chat/completions');", []);
                                    let _ = conn.execute("INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('model', 'deepseek-v4-flash-free');", []);
                                }
                            }
                            s.query = format!("AI {} Saved!", k.to_uppercase());
                        }
                    }
                }

            } else if let Some(path) = cmd.strip_prefix("open_note:") {
                open_note_editor(hwnd, s, path.to_string());
                s.query.clear();
                s.cursor_pos = 0;
                s.reset_results();
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if let Some(text) = cmd.strip_prefix("copy:") {
                copy_to_clipboard(hwnd, text);
            } else if let Some(path) = cmd.strip_prefix("copy_image:") {
                let prev_hwnd = s.prev_foreground;
                if copy_image_to_clipboard(hwnd, path) {
                    do_hide(hwnd, s);
                    paste_into_window(prev_hwnd);
                } else {
                    s.query = "Could not copy image to clipboard".to_string();
                    s.cursor_pos = s.query.len();
                    s.reset_results();
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
                return;
            } else if let Some(path) = cmd.strip_prefix("ocr_image_file:") {
                // OCR a saved clipboard image file (e.g., a screenshot saved in clipboard_images/).
                // Runs on a background MTA thread to avoid deadlocking the STA window thread.
                let path_owned = path.to_string();
                let hwnd_ocr = hwnd.0 as usize;
                let db_path_ocr = s.db_path.clone();
                std::thread::spawn(move || {
                    let com_initialized = unsafe {
                        windows::Win32::System::Com::CoInitializeEx(
                            None,
                            windows::Win32::System::Com::COINIT_MULTITHREADED,
                        )
                    }
                    .is_ok();
                    let result = run_clipboard_ocr(|| {
                        indexer::ocr_image_file(std::path::Path::new(&path_owned))
                    });
                    if let Some(text) = &result {
                        if let Ok(conn) = rusqlite::Connection::open(&db_path_ocr) {
                            let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                            let _ = conn.execute(
                                "UPDATE clipboard_history SET ocr_text = ? WHERE content = ?;",
                                rusqlite::params![text, path_owned],
                            );
                        }
                    }
                    if com_initialized {
                        unsafe {
                            windows::Win32::System::Com::CoUninitialize();
                        }
                    }
                    let ptr = Box::into_raw(Box::new(result));
                    let _ = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            windows::Win32::Foundation::HWND(hwnd_ocr as *mut std::ffi::c_void),
                            WM_OCR_RESULT,
                            WPARAM(0),
                            LPARAM(ptr as isize),
                        )
                    };
                });
                s.query = "Scanning image…".to_string();
                s.cursor_pos = s.query.len();
                s.reset_results();
                reset_cursor_blink(hwnd, s);
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if cmd == "action:ocr_clipboard" {
                // Offload blocking WinRT OCR to a background MTA thread to avoid
                // deadlocking the STA main window thread.
                let hwnd_ocr = hwnd.0 as usize;
                std::thread::spawn(move || {
                    let com_initialized = unsafe {
                        windows::Win32::System::Com::CoInitializeEx(
                            None,
                            windows::Win32::System::Com::COINIT_MULTITHREADED,
                        )
                    }
                    .is_ok();
                    let result = run_clipboard_ocr(|| unsafe {
                        ocr_clipboard_image_with_win32_fallback(windows::Win32::Foundation::HWND(
                            hwnd_ocr as *mut std::ffi::c_void,
                        ))
                    });
                    if com_initialized {
                        unsafe {
                            windows::Win32::System::Com::CoUninitialize();
                        }
                    }
                    // Box the Option<String> and post it back to the window proc.
                    let ptr = Box::into_raw(Box::new(result));
                    let _ = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            windows::Win32::Foundation::HWND(hwnd_ocr as *mut std::ffi::c_void),
                            WM_OCR_RESULT,
                            WPARAM(0),
                            LPARAM(ptr as isize),
                        )
                    };
                });
                // Show a placeholder while OCR is in-flight.
                s.query = "Scanning image…".to_string();
                s.cursor_pos = s.query.len();
                s.reset_results();
                reset_cursor_blink(hwnd, s);
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if cmd == "action:create_snippet" {
                s.form_state = FormState::CreateSnippetName;
                s.query.clear();
                s.cursor_pos = 0;
                s.reset_results();
                s.selected = 0;
                reset_cursor_blink(hwnd, s);
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if cmd == "action:create_focus_category" {
                s.form_state = FormState::CreateFocusCategoryName;
                s.query.clear();
                s.cursor_pos = 0;
                s.reset_results();
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if cmd == "action:create_note" {
                s.form_state = FormState::CreateNoteName;
                s.query.clear();
                s.cursor_pos = 0;
                s.reset_results();
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if cmd == "action:create_quicklink" {
                s.form_state = FormState::CreateQuicklinkName;
                s.query.clear();
                s.cursor_pos = 0;
                s.reset_results();
                s.selected = 0;
                reset_cursor_blink(hwnd, s);
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if let Some(content) = cmd.strip_prefix("copy_snippet:") {
                copy_to_clipboard(hwnd, content);
                s.placeholder_override = Some("Copied to clipboard!".to_string());
                let _ = unsafe { SetTimer(hwnd, TIMER_CLIPBOARD_MSG, 2000, None) };
                let _ = InvalidateRect(hwnd, None, FALSE);
                return;
            } else if let Some(url) = cmd.strip_prefix("open_quicklink:") {
                let url_w = format!("{}\0", url).encode_utf16().collect::<Vec<u16>>();
                let open_w = "open\0".encode_utf16().collect::<Vec<u16>>();
                windows::Win32::UI::Shell::ShellExecuteW(
                    None,
                    windows::core::PCWSTR(open_w.as_ptr()),
                    windows::core::PCWSTR(url_w.as_ptr()),
                    None,
                    None,
                    windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
                );
                do_hide(hwnd, s);
                return;
            } else if cmd == "action:ask_clipboard" {
                if let Some(text) = paste_from_clipboard(hwnd) {
                    let t = text.trim();
                    if !t.is_empty() {
                        s.chat_input = t.to_string();
                        s.query = "".to_string();
                        s.cursor_pos = 0;
                        s.chat_input_active = true;
                        s.ai_answer = Some(
                            "Ready. Hit Enter to send or edit your clipboard text above."
                                .to_string(),
                        );
                        s.ai_title = "Ask Clipboard".to_string();
                        s.reset_results();
                        s.selected = 0;
                        s.ai_scroll = 0;
                        s.ai_follow_bottom = true;
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
                return;
            } else if cmd == "action:export_snippets" {
                export_snippets(hwnd, s);
                do_hide(hwnd, s);
                return;
            } else if cmd == "action:import_snippets" {
                import_snippets(hwnd, s);
                do_hide(hwnd, s);
                return;
            } else if cmd == "action:export_quicklinks" {
                export_quicklinks(hwnd, s);
                do_hide(hwnd, s);
                return;
            } else if cmd == "action:import_quicklinks" {
                import_quicklinks(hwnd, s);
                do_hide(hwnd, s);
                return;
            } else if cmd == "action:reload_script_commands" {
                s.query = "Script commands reloaded!".to_string();
                s.cursor_pos = s.query.len();
                trigger_search(hwnd, s);
                return;
            } else if cmd == "action:color_picker" {
                start_color_picker(hwnd, s);
                return;
            } else if cmd == "action:circle_to_search" {
                start_circle_to_search_from_launcher(hwnd, s);
                return;
            } else if cmd == "action:reset_window_position" {
                reset_launcher_window_position(hwnd, s);
                return;
            } else if cmd == "action:paste_latest_screenshot" {
                let prev_hwnd = s.prev_foreground;
                if let Some(path) = latest_clipboard_image_path(&s.db_path) {
                    if copy_image_to_clipboard(hwnd, &path) {
                        do_hide(hwnd, s);
                        paste_into_window(prev_hwnd);
                    } else {
                        s.query = "Could not copy latest screenshot".to_string();
                        s.cursor_pos = s.query.len();
                        s.reset_results();
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                } else {
                    s.query = "No screenshot found in Clipboard History".to_string();
                    s.cursor_pos = s.query.len();
                    s.reset_results();
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
                return;
            } else if cmd == "action:quit_active_app" {
                let prev_hwnd = s.prev_foreground;
                do_hide(hwnd, s);
                if !prev_hwnd.0.is_null() {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        prev_hwnd,
                        windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                return;
            } else {
                launcher::launch(&cmd);
            }
            do_hide(hwnd, s);
        }
    }
}

unsafe fn kick_debounce(hwnd: HWND, s: &mut State) {
    s.results_stale = true;
    let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
    let _ = SetTimer(hwnd, TIMER_DEBOUNCE, 20, None);
}

unsafe fn sync_height_animation(hwnd: HWND, s: &mut State) {
    let target = s.target_win_h();
    if s.target_h != target {
        s.height_anim_from = s.shown_h.max(s.search_h());
        s.target_h = target;
        s.height_anim_started = std::time::Instant::now();
        let _ = SetTimer(hwnd, TIMER_SEARCH_ANIM, 3, None);
        unsafe {
            trigger_anim_loop(hwnd);
        }
        let _ = InvalidateRect(hwnd, None, FALSE);
    } else if s.shown_h == 0 {
        s.shown_h = target;
    }
}

fn animated_height(s: &State) -> i32 {
    if s.shown_h <= 0 {
        return s.target_win_h();
    }
    if s.shown_h == s.target_h {
        return s.shown_h;
    }
    let elapsed = s.height_anim_started.elapsed().as_millis();
    let t = (elapsed as f32 / HEIGHT_ANIM_MS as f32).clamp(0.0, 1.0);
    let eased = ease_out(t);
    (s.height_anim_from as f32 + (s.target_h - s.height_anim_from) as f32 * eased) as i32
}

unsafe fn tick_window_animation(hwnd: HWND, s: &mut State) -> bool {
    let p = s.current_p();
    match s.anim {
        Anim::Appearing { .. } => {
            let alpha = (ease_out(p) * 255.0).round() as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLOR_KEY, alpha, LWA_COLORKEY | LWA_ALPHA);
            if p >= 1.0 {
                s.anim = Anim::Visible;
                s.target_h = s.target_win_h();
                s.shown_h = s.target_h;
                s.height_anim_from = s.target_h;
                s.height_anim_started = std::time::Instant::now();
                let _ = SetLayeredWindowAttributes(hwnd, COLOR_KEY, 255, LWA_COLORKEY | LWA_ALPHA);
                applog::log("animate_window: visible");
                let _ = PostMessageW(hwnd, WM_ANIM_FOREGROUND, WPARAM(0), LPARAM(0));
                false
            } else {
                true
            }
        }
        Anim::Hiding { .. } => {
            let alpha = (ease_out(p) * 255.0).round() as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLOR_KEY, alpha, LWA_COLORKEY | LWA_ALPHA);
            if p <= 0.0 {
                s.anim = Anim::Hidden;
                let _ = PostMessageW(hwnd, WM_ANIM_HIDE_WINDOW, WPARAM(0), LPARAM(0));
                lower_timer_res();
                applog::log("animate_window: hidden");
                false
            } else {
                true
            }
        }
        _ => false,
    }
}

fn search_row_invalidation_rect(
    client_w: i32,
    launcher_w: i32,
    cy: i32,
    current_h: i32,
    search_h: i32,
) -> RECT {
    let x = (client_w - launcher_w) / 2;
    let y = launcher_top_y(cy, current_h);
    RECT {
        left: x,
        top: y,
        right: x + launcher_w,
        bottom: y + search_h + 2,
    }
}

fn results_invalidation_rect(
    client_w: i32,
    launcher_w: i32,
    cy: i32,
    current_h: i32,
    search_h: i32,
) -> RECT {
    let x = (client_w - launcher_w) / 2;
    let y = launcher_top_y(cy, current_h);
    RECT {
        left: x,
        top: y + search_h + 1,
        right: x + launcher_w,
        bottom: y + current_h,
    }
}

fn visible_row_count(result_count: usize, max_results: usize) -> i32 {
    result_count.clamp(1, max_results) as i32
}

fn homepage_win_h(search_h: i32, item_h: i32, result_count: usize, max_results: usize) -> i32 {
    search_h + 1 + LABEL_HEADER_H + visible_row_count(result_count, max_results) * item_h + 8
}

fn launcher_top_y(cy: i32, current_h: i32) -> i32 {
    cy - current_h / 2
}

fn normal_search_win_h(search_h: i32, item_h: i32, result_count: usize, max_results: usize) -> i32 {
    search_h + 1 + CONTENT_HEADER_H + visible_row_count(result_count, max_results) * item_h + 8
}

fn scoped_results_win_h(search_h: i32, item_h: i32, result_count: usize, max_results: usize) -> i32 {
    search_h + 1 + LABEL_HEADER_H + visible_row_count(result_count, max_results) * item_h + 8
}

unsafe fn invalidate_search_row(hwnd: HWND, s: &State) {
    if s.anim != Anim::Visible {
        let _ = InvalidateRect(hwnd, None, FALSE);
        return;
    }
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_ok() {
        let rect = search_row_invalidation_rect(
            client.right - client.left,
            s.launcher_w,
            s.cy,
            s.paint_win_h(),
            s.search_h(),
        );
        let _ = InvalidateRect(hwnd, Some(&rect), FALSE);
    } else {
        let _ = InvalidateRect(hwnd, None, FALSE);
    }
}

unsafe fn invalidate_results_area(hwnd: HWND, s: &State) {
    if s.anim != Anim::Visible {
        let _ = InvalidateRect(hwnd, None, FALSE);
        return;
    }
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_ok() {
        let rect = results_invalidation_rect(
            client.right - client.left,
            s.launcher_w,
            s.cy,
            s.paint_win_h(),
            s.search_h(),
        );
        let _ = InvalidateRect(hwnd, Some(&rect), FALSE);
    } else {
        let _ = InvalidateRect(hwnd, None, FALSE);
    }
}

unsafe fn trigger_search(_hwnd: HWND, s: &mut State) {
    s.submenu_active = false;
    s.image_preview_active = false;
    hide_preview_window(s);
    if s.editing_item.is_some() {
        return;
    }
    if s.query.is_empty() {
        s.current_query_id += 1;
        s.results_stale = false;
        s.search_loading = false;
        s.active_filter = FilterType::All;
        s.unfiltered_results = default_homepage_results();
        s.results = s.unfiltered_results.clone();
        // Land on the homepage item the user last visited, not a fixed default.
        s.selected = s.homepage_sel.min(s.results.len().saturating_sub(1));
        s.scroll_offset = 0;
        sync_height_animation(_hwnd, s);
        let _ = InvalidateRect(_hwnd, None, FALSE);
        return;
    }

    // ── End bare-word aliases ───────────────────────────────────────────────

    s.results_stale = true;
    s.current_query_id += 1;
    s.search_loading = true;
    sync_height_animation(_hwnd, s);
    // Drive the window grow at 60fps (16ms). This used to be 80ms, which capped the
    // grow at ~12fps and made navigation feel choppy. The WM_TIMER handler drops back
    // to a gentle 80ms once the grow finishes and only the loading spinner remains.
    let _ = SetTimer(_hwnd, TIMER_SEARCH_ANIM, 8, None);
    unsafe {
        trigger_anim_loop(_hwnd);
    }
    let req = SearchRequest {
        query: s.query.clone(),
        query_id: s.current_query_id,
    };
    // Plain queries get instant files/folders from the fast worker AND the full set from the
    // slow worker. Prefix queries (bookmarks:, commits:, agentchats:, clip:, file:, …) are
    // understood ONLY by the slow worker — the fast path returns an empty set that would
    // overwrite the real page (the "appears then disappears" bug) — so those go to slow only.
    if !s.has_prefix() {
        if let Some(ref tx) = s.search_tx {
            let _ = tx.send(req.clone());
        }
    }
    if let Some(ref tx) = s.search_tx_slow {
        let _ = tx.send(req);
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}
// fn ease_in(t: f32) -> f32 { t.clamp(0.0, 1.0).powi(4) }

fn search_lens_action_rect(x: i32, y: i32, w: i32, search_h: i32) -> RECT {
    let right = x + w - PAD_L;
    RECT {
        left: right - SEARCH_LENS_BUTTON_SIZE,
        top: y + (search_h - SEARCH_LENS_BUTTON_SIZE) / 2,
        right,
        bottom: y + (search_h + SEARCH_LENS_BUTTON_SIZE) / 2,
    }
}

fn search_settings_action_rect(x: i32, y: i32, w: i32, search_h: i32, lens_visible: bool, web_visible: bool) -> RECT {
    let right = if lens_visible {
        search_lens_action_rect(x, y, w, search_h).left - SEARCH_ACTION_GAP
    } else if web_visible {
        search_web_action_rect(x, y, w, search_h, false).left - SEARCH_ACTION_GAP
    } else {
        x + w - PAD_L
    };
    RECT {
        left: right - SEARCH_LENS_BUTTON_SIZE,
        top: y + (search_h - SEARCH_LENS_BUTTON_SIZE) / 2,
        right,
        bottom: y + (search_h + SEARCH_LENS_BUTTON_SIZE) / 2,
    }
}

fn search_web_action_rect(x: i32, y: i32, w: i32, search_h: i32, lens_visible: bool) -> RECT {
    let action_h = 32;
    let right = if lens_visible {
        search_lens_action_rect(x, y, w, search_h).left - SEARCH_ACTION_GAP
    } else {
        x + w - PAD_L - 8
    };
    RECT {
        left: right - SEARCH_WEB_BUTTON_W,
        top: y + (search_h - action_h) / 2,
        right,
        bottom: y + (search_h + action_h) / 2,
    }
}

fn point_in_rect(x: i32, y: i32, r: RECT) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

unsafe fn open_query_on_web(hwnd: HWND, s: &mut State) {
    let query = s.query.trim();
    if query.is_empty() {
        return;
    }
    let encoded = search::url_encode(query);
    let url = format!("https://www.google.com/search?q={encoded}");
    launcher::launch(&url);
    do_hide(hwnd, s);
}

unsafe fn start_circle_to_search_from_launcher(hwnd: HWND, s: &mut State) {
    let _ = KillTimer(hwnd, TIMER_SEARCH_ANIM);
    let _ = KillTimer(hwnd, TIMER_DEBOUNCE);
    let _ = KillTimer(hwnd, TIMER_CURSOR_BLINK);
    s.anim = Anim::Hidden;
    s.shown_h = 0;
    s.target_h = 0;
    s.search_input_active = false;
    s.cursor_visible = false;
    let _ = ShowWindow(hwnd, SW_HIDE);
    let _ = DwmFlush();
    circle_to_search::start_lens_search_async();
}

unsafe fn fill_rounded(hdc: HDC, x: i32, y: i32, w: i32, h: i32, r: i32, c: COLORREF) {
    let br = CreateSolidBrush(c);
    let old_brush = SelectObject(hdc, br);
    let pen = CreatePen(PS_NULL, 0, COLORREF(0));
    let old_pen = SelectObject(hdc, pen);

    let _ = RoundRect(hdc, x, y, x + w + 1, y + h + 1, r, r);

    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen);
    let _ = SelectObject(hdc, old_brush);
    let _ = DeleteObject(br);
}

/// Inner text width (px) for a user chat bubble: the message's natural single-line
/// width, clamped to `max_inner` so short messages hug their content and long ones
/// wrap. Called from both the measure and paint passes so the two agree on height.
/// The caller must have selected the body font into `hdc` first.
unsafe fn bubble_inner_w(hdc: HDC, text_wide: &[u16], max_inner: i32) -> i32 {
    let mut nat = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut tmp = text_wide.to_vec();
    let _ = DrawTextW(
        hdc,
        &mut tmp,
        &mut nat,
        DT_LEFT | DT_SINGLELINE | DT_CALCRECT | DT_NOPREFIX,
    );
    (nat.right - nat.left).clamp(1, max_inner.max(1))
}

// ── Markdown rendering for the AI panel ───────────────────────────────────────
//
// Both `measure_response` and `paint_response` walk the same parsed blocks so the
// measured height (used for scroll math) always matches the painted height.
//
// Layout vocabulary (pixels):
//   PARA_GAP     — space after a paragraph / between non-adjacent blocks
//   CODE_PAD     — inner padding of a fenced code block
//   LIST_INDENT  — left indent for list markers
const MD_PARA_GAP: i32 = 10;
const MD_CODE_PAD: i32 = 12;
const MD_CODE_BG: COLORREF = COLORREF(0x00_28_26_25);
const MD_CODE_BORDER: COLORREF = COLORREF(0x00_3A_36_35);
const MD_INLINE_CODE_BG: COLORREF = COLORREF(0x00_2E_2C_2B);
const MD_LINK: COLORREF = COLORREF(0x00_7A_B8_F5);
const MD_CODE_FG: COLORREF = COLORREF(0x00_E6_C0_7A);

/// Color used for the body font of an assistant response.
const MD_BODY: COLORREF = COLORREF(0x00_E7_E3_E1);
const MD_MUTED: COLORREF = COLORREF(0x00_8F_89_86);

/// Measure the rendered height of a Markdown response at the given pixel width.
unsafe fn measure_response(hdc: HDC, text: &str, s: &State, width: i32) -> i32 {
    let blocks = markdown::parse(text);
    measure_blocks(hdc, &blocks, s, width)
}

/// Paint a Markdown response at vertical offset `top`. Returns the total height
/// consumed (so the caller can advance its cursor).
unsafe fn paint_response(hdc: HDC, text: &str, s: &State, x: i32, width: i32, top: i32) -> i32 {
    let blocks = markdown::parse(text);
    paint_blocks(hdc, &blocks, s, x, width, top)
}

unsafe fn measure_blocks(hdc: HDC, blocks: &[markdown::MdBlock], s: &State, width: i32) -> i32 {
    let mut total = 0i32;
    let prev_font = SelectObject(hdc, s.font_c); // remember to restore
    for (i, b) in blocks.iter().enumerate() {
        let prev_kind = block_kind(blocks.get(i.wrapping_sub(1)));
        total += block_top_gap(b, prev_kind);
        total += measure_one(hdc, b, s, width);
    }
    let _ = SelectObject(hdc, prev_font);
    total
}

fn block_kind(b: Option<&markdown::MdBlock>) -> Option<&'static str> {
    match b {
        Some(markdown::MdBlock::Heading { .. }) => Some("h"),
        Some(markdown::MdBlock::Paragraph { .. }) => Some("p"),
        Some(markdown::MdBlock::Code { .. }) => Some("code"),
        Some(markdown::MdBlock::ListItem { .. }) => Some("li"),
        Some(markdown::MdBlock::Spacer) => Some("spacer"),
        None => None,
    }
}

/// Vertical gap to insert *above* this block based on what came before it.
fn block_top_gap(cur: &markdown::MdBlock, prev: Option<&str>) -> i32 {
    use markdown::MdBlock;
    match (cur, prev) {
        (_, None) => 0,
        (MdBlock::Spacer, _) | (_, Some("spacer")) => 0,
        (MdBlock::Heading { .. }, _) => 14,
        (_, Some("h")) => 10,
        (MdBlock::ListItem { .. }, Some("li")) => 3,
        (MdBlock::ListItem { .. }, _) => 8,
        (_, Some("li")) => MD_PARA_GAP,
        (MdBlock::Code { .. }, _) | (_, Some("code")) => MD_PARA_GAP,
        (_, Some("p")) => MD_PARA_GAP,
        _ => MD_PARA_GAP,
    }
}

unsafe fn measure_one(hdc: HDC, b: &markdown::MdBlock, s: &State, width: i32) -> i32 {
    use markdown::MdBlock;
    match b {
        MdBlock::Spacer => 6,
        MdBlock::Heading { level, .. } => {
            let _ = SelectObject(hdc, s.font_h);
            let plain = strip_inline_text(b);
            let h = wrap_text_height(hdc, &plain, width);
            (*level as i32 - 1).max(0) * 4 + h + 4
        }
        MdBlock::Paragraph { runs } => {
            let _ = SelectObject(hdc, s.font_c);
            measure_runs(hdc, runs, s, width).1
        }
        MdBlock::ListItem { runs, .. } => {
            let _ = SelectObject(hdc, s.font_c);
            // 22px reserved for the marker ("•"/"1.") on the left.
            measure_runs(hdc, runs, s, (width - 22).max(40)).1
        }
        MdBlock::Code { text, .. } => {
            let _ = SelectObject(hdc, s.font_code);
            let inner_w = (width - MD_CODE_PAD * 2).max(40);
            let mut h = 0;
            for line in text.split('\n') {
                let lh = if line.is_empty() {
                    18
                } else {
                    let mut wide: Vec<u16> = line.encode_utf16().collect();
                    let mut rc = RECT {
                        left: 0,
                        top: 0,
                        right: inner_w,
                        bottom: 0,
                    };
                    let _ = DrawTextW(
                        hdc,
                        &mut wide,
                        &mut rc,
                        DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                    );
                    (rc.bottom - rc.top).max(18)
                };
                h += lh;
            }
            // Empty code block still shows one line.
            if h == 0 {
                h = 18;
            }
            h + MD_CODE_PAD * 2
        }
    }
}

/// Extract the joined plain text of a block (used for heading measurement).
fn strip_inline_text(b: &markdown::MdBlock) -> String {
    use markdown::MdBlock;
    match b {
        MdBlock::Heading { runs, .. }
        | MdBlock::Paragraph { runs }
        | MdBlock::ListItem { runs, .. } => runs
            .iter()
            .map(|r| match r {
                markdown::MdInline::Plain(t)
                | markdown::MdInline::Bold(t)
                | markdown::MdInline::Italic(t)
                | markdown::MdInline::Code(t) => t.as_str(),
                markdown::MdInline::Link { label, .. } => label.as_str(),
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

unsafe fn wrap_text_height(hdc: HDC, text: &str, width: i32) -> i32 {
    if text.is_empty() {
        return 16;
    }
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: 0,
    };
    let _ = DrawTextW(
        hdc,
        &mut wide,
        &mut rc,
        DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
    );
    (rc.bottom - rc.top).max(16)
}

/// Measure a run-list, returning `(max_inline_width_unused, total_height)`.
/// Height is what the caller actually needs; the width component is unused today
/// but kept to centralize run layout for future use.
unsafe fn measure_runs(hdc: HDC, runs: &[markdown::MdInline], s: &State, width: i32) -> (i32, i32) {
    // Render the runs into a single wrapped string is hard because of mixed
    // fonts; instead we lay runs out greedily into visual lines and sum heights.
    let layout = layout_runs(hdc, runs, s, width);
    let height = layout.line_heights.iter().sum::<i32>();
    (width, height)
}

/// Greedy line layout of inline runs. Returns positioned run fragments plus the
/// per-line heights. Both measure and paint use this so wrapping matches.
unsafe fn layout_runs(hdc: HDC, runs: &[markdown::MdInline], s: &State, width: i32) -> MdLayout {
    use markdown::MdInline;
    let mut lines: Vec<Vec<MdFrag>> = vec![vec![]];
    let mut cur_w = 0i32;
    let mut max_ascent = 0i32;
    let mut max_descent = 0i32;
    let mut line_heights: Vec<i32> = Vec::new();

    let space_w = text_width(hdc, " ", s.font_c);

    for run in runs {
        let (font, _color, text) = match run {
            MdInline::Plain(t) => (s.font_c, MD_BODY, t.as_str()),
            MdInline::Bold(t) => (s.font_n, MD_BODY, t.as_str()),
            MdInline::Italic(t) => (s.font_c, MD_BODY, t.as_str()),
            MdInline::Code(t) => (s.font_code, MD_CODE_FG, t.as_str()),
            MdInline::Link { label, .. } => (s.font_c, MD_LINK, label.as_str()),
        };
        let _ = _color;

        // Split into words and wrap greedily.
        let parts: Vec<&str> = text.split(' ').collect();
        for (wi, word) in parts.iter().enumerate() {
            if word.is_empty() && wi != 0 {
                // Consecutive spaces collapse to one; the space is handled below.
                continue;
            }
            let word_w = text_width(hdc, word, font);
            let need_space = !lines.last().map(|l| l.is_empty()).unwrap_or(true) && wi != 0;
            let add_w = word_w + if need_space { space_w } else { 0 };

            if cur_w + add_w > width && !lines.last().map(|l| l.is_empty()).unwrap_or(true) {
                // Wrap: flush the current line, then start a fresh line with this word.
                line_heights.push((max_ascent + max_descent).max(16));
                lines.push(vec![]);
                lines.last_mut().unwrap().push(MdFrag {
                    font,
                    text: word.to_string(),
                    leading_space: false,
                });
                cur_w = word_w;
                let metrics = font_metrics(hdc, font);
                max_ascent = metrics.0;
                max_descent = metrics.1;
            } else {
                let leading = need_space;
                lines.last_mut().unwrap().push(MdFrag {
                    font,
                    text: word.to_string(),
                    leading_space: leading,
                });
                cur_w += add_w;
                let metrics = font_metrics(hdc, font);
                max_ascent = max_ascent.max(metrics.0);
                max_descent = max_descent.max(metrics.1);
            }
        }
    }
    line_heights.push((max_ascent + max_descent).max(16));

    MdLayout {
        lines,
        line_heights,
    }
}

#[derive(Default)]
struct MdLayout {
    lines: Vec<Vec<MdFrag>>,
    line_heights: Vec<i32>,
}

struct MdFrag {
    font: HFONT,
    text: String,
    leading_space: bool,
}

unsafe fn text_width(hdc: HDC, text: &str, font: HFONT) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let _old = SelectObject(hdc, font);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
    let _ = SelectObject(hdc, _old);
    size.cx
}

/// Returns (ascent, descent) in pixels for the current font.
unsafe fn font_metrics(hdc: HDC, font: HFONT) -> (i32, i32) {
    let _old = SelectObject(hdc, font);
    let mut tm = TEXTMETRICW::default();
    let _ = GetTextMetricsW(hdc, &mut tm);
    let _ = SelectObject(hdc, _old);
    (tm.tmAscent, tm.tmDescent)
}

unsafe fn paint_blocks(
    hdc: HDC,
    blocks: &[markdown::MdBlock],
    s: &State,
    x: i32,
    width: i32,
    top: i32,
) -> i32 {
    use markdown::MdBlock;
    let prev_font = SelectObject(hdc, s.font_c);
    let mut y = top;
    let _ = SetBkMode(hdc, TRANSPARENT);

    for (i, b) in blocks.iter().enumerate() {
        let prev_kind = block_kind(blocks.get(i.wrapping_sub(1)));
        y += block_top_gap(b, prev_kind);

        match b {
            MdBlock::Spacer => {
                y += 6;
            }
            MdBlock::Heading { level, runs } => {
                let _ = SelectObject(hdc, s.font_h);
                SetTextColor(hdc, s.theme.palette().clr_white);
                y += (*level as i32 - 1).max(0) * 4;
                y += paint_run_lines(hdc, runs, s, x, y, width, None);
            }
            MdBlock::Paragraph { runs } => {
                let _ = SelectObject(hdc, s.font_c);
                y += paint_run_lines(hdc, runs, s, x, y, width, None);
            }
            MdBlock::ListItem {
                runs,
                ordered,
                index,
            } => {
                let _ = SelectObject(hdc, s.font_c);
                let marker = if *ordered {
                    format!("{}.", index)
                } else {
                    "•".to_string()
                };
                // Draw the marker.
                SetTextColor(hdc, MD_MUTED);
                let mut mwide: Vec<u16> = marker.encode_utf16().collect();
                let mut mrc = RECT {
                    left: x,
                    top: y,
                    right: x + 22,
                    bottom: y + 40,
                };
                let _ = DrawTextW(hdc, &mut mwide, &mut mrc, DT_LEFT | DT_TOP | DT_NOPREFIX);
                y += paint_run_lines(hdc, runs, s, x + 22, y, (width - 22).max(40), None);
            }
            MdBlock::Code { text, .. } => {
                let _ = SelectObject(hdc, s.font_code);
                let inner_w = (width - MD_CODE_PAD * 2).max(40);
                // Measure height to draw the background box.
                let mut h = 0;
                for line in text.split('\n') {
                    let lh = if line.is_empty() {
                        18
                    } else {
                        let mut wide: Vec<u16> = line.encode_utf16().collect();
                        let mut rc = RECT {
                            left: 0,
                            top: 0,
                            right: inner_w,
                            bottom: 0,
                        };
                        let _ = DrawTextW(
                            hdc,
                            &mut wide,
                            &mut rc,
                            DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                        );
                        (rc.bottom - rc.top).max(18)
                    };
                    h += lh;
                }
                if h == 0 {
                    h = 18;
                }
                let box_h = h + MD_CODE_PAD * 2;
                fill_rounded(hdc, x, y, width, box_h, 8, MD_CODE_BG);
                // Subtle left accent border.
                fill(hdc, x, y, 2, box_h, MD_CODE_BORDER);

                SetTextColor(hdc, MD_CODE_FG);
                let mut ly = y + MD_CODE_PAD;
                for line in text.split('\n') {
                    let lh = if line.is_empty() {
                        18
                    } else {
                        let mut wide: Vec<u16> = line.encode_utf16().collect();
                        let mut measure = RECT {
                            left: 0,
                            top: 0,
                            right: inner_w,
                            bottom: 0,
                        };
                        let _ = DrawTextW(
                            hdc,
                            &mut wide,
                            &mut measure,
                            DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                        );
                        let lh = (measure.bottom - measure.top).max(18);
                        let mut rc = RECT {
                            left: x + MD_CODE_PAD,
                            top: ly,
                            right: x + MD_CODE_PAD + inner_w,
                            bottom: ly + lh,
                        };
                        let _ = DrawTextW(
                            hdc,
                            &mut wide,
                            &mut rc,
                            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
                        );
                        lh
                    };
                    ly += lh;
                }
                y += box_h;
            }
        }
    }

    let _ = SelectObject(hdc, prev_font);
    y - top
}

/// Paint inline runs with word-wrap, advancing downward. Returns the height used.
/// `bg` optionally fills behind each line (used for inline-code chips below).
unsafe fn paint_run_lines(
    hdc: HDC,
    runs: &[markdown::MdInline],
    s: &State,
    x: i32,
    top: i32,
    width: i32,
    _bg: Option<COLORREF>,
) -> i32 {
    let layout = layout_runs(hdc, runs, s, width);
    let mut y = top;

    for (line_idx, line) in layout.lines.iter().enumerate() {
        let lh = layout.line_heights.get(line_idx).copied().unwrap_or(16);
        let mut cx = x;
        // Baseline = top + ascent of the dominant (body) font.
        let body_metrics = font_metrics(hdc, s.font_c);
        let baseline = y + body_metrics.0;

        for frag in line {
            if frag.leading_space {
                cx += text_width(hdc, " ", s.font_c);
            }
            let _ = SelectObject(hdc, frag.font);
            let color = if std::ptr::eq(frag.font.0, s.font_code.0) {
                MD_CODE_FG
            } else {
                MD_BODY
            };

            // Inline code chip: draw a rounded background behind the text.
            let is_code = std::ptr::eq(frag.font.0, s.font_code.0);
            if is_code && !frag.text.is_empty() {
                let tw = text_width(hdc, &frag.text, frag.font);
                let metrics = font_metrics(hdc, frag.font);
                let chip_h = metrics.0 + metrics.1 + 4;
                let chip_top = baseline - metrics.0 + 2;
                fill_rounded(hdc, cx, chip_top, tw + 10, chip_h, 4, MD_INLINE_CODE_BG);
            }

            if !frag.text.is_empty() {
                SetTextColor(hdc, color);
                let mut wide: Vec<u16> = frag.text.encode_utf16().collect();
                let mut rc = RECT {
                    left: cx,
                    top: baseline - (font_metrics(hdc, frag.font).0),
                    right: cx + width,
                    bottom: baseline + 40,
                };
                let _ = DrawTextW(
                    hdc,
                    &mut wide,
                    &mut rc,
                    DT_LEFT | DT_NOPREFIX | DT_SINGLELINE,
                );
                cx += text_width(hdc, &frag.text, frag.font);
            }
        }
        y += lh;
    }
    y - top
}

/// Decode a WM_CHAR wparam into a char, pairing UTF-16 surrogate halves that
/// Windows delivers as two separate WM_CHAR messages. Without pairing, emoji
/// and other astral-plane input were silently dropped (char::from_u32 rejects
/// lone surrogates).
fn wm_char_decode(pending: &mut Option<u16>, code: u32) -> Option<char> {
    if (0xD800..0xDC00).contains(&code) {
        *pending = Some(code as u16);
        return None;
    }
    if (0xDC00..0xE000).contains(&code) {
        let high = pending.take()?;
        let combined = 0x10000 + (((high as u32 - 0xD800) << 10) | (code - 0xDC00));
        return char::from_u32(combined);
    }
    *pending = None;
    char::from_u32(code)
}

fn word_left(s: &str, pos: usize) -> usize {
    let i = pos.min(s.len());
    if i == 0 {
        return 0;
    }

    let mut chars = s[..i].char_indices().rev();

    let mut next_char = chars.next();
    while let Some((_, c)) = next_char {
        if c.is_whitespace() {
            next_char = chars.next();
        } else {
            break;
        }
    }

    let (idx, c) = match next_char {
        Some(pair) => pair,
        None => return 0,
    };

    let start_class = if c.is_alphanumeric() || c == '_' {
        1 // Word
    } else {
        2 // Punctuation
    };

    let mut last_idx = idx;
    for (idx, c) in chars {
        let class = if c.is_alphanumeric() || c == '_' {
            1
        } else if c.is_whitespace() {
            break; // Stop at whitespace
        } else {
            2
        };

        if class == start_class {
            last_idx = idx;
        } else {
            break; // Stop when class changes
        }
    }

    last_idx
}

fn word_right(s: &str, pos: usize) -> usize {
    let len = s.len();
    let i = pos.min(len);
    if i >= len {
        return len;
    }

    let mut chars = s[i..].char_indices();
    let (_, first_char) = chars.next().unwrap();

    if first_char.is_whitespace() {
        for (idx, c) in chars {
            if !c.is_whitespace() {
                return i + idx;
            }
        }
        return len;
    }

    let start_class = if first_char.is_alphanumeric() || first_char == '_' {
        1
    } else {
        2
    };

    let mut next_pos = len;
    let mut chars_loop = s[i..].char_indices();
    let _ = chars_loop.next();

    for (idx, c) in chars_loop {
        let class = if c.is_alphanumeric() || c == '_' {
            1
        } else if c.is_whitespace() {
            next_pos = i + idx;
            break;
        } else {
            2
        };

        if class != start_class {
            next_pos = i + idx;
            break;
        }
    }

    if next_pos < len {
        let follow_chars = s[next_pos..].char_indices();
        for (idx, c) in follow_chars {
            if !c.is_whitespace() {
                return next_pos + idx;
            }
        }
    }

    len
}

fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn search_input_caret_active(s: &State) -> bool {
    search_input_caret_active_flags(
        s.search_input_active,
        s.note_editing,
        s.chat_input_active,
        true,
    )
}

fn search_input_caret_active_flags(
    search_input_active: bool,
    note_editing: bool,
    chat_input_active: bool,
    has_visible_input: bool,
) -> bool {
    search_input_active && !note_editing && !chat_input_active && has_visible_input
}

fn text_caret_active(s: &State) -> bool {
    s.note_editing || s.chat_input_active || search_input_caret_active(s)
}

fn delete_word_before(s: &mut State) {
    let cur = floor_char_boundary(&s.query, s.cursor_pos);
    let new_pos = word_left(&s.query, cur);
    let rest = s.query[cur..].to_string();
    s.query.truncate(new_pos);
    s.query.push_str(&rest);
    s.cursor_pos = new_pos;
}

// ── Painting ──────────────────────────────────────────────────────────────────
unsafe fn resolve_lnk(path: &str) -> Option<String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_UNCPRIORITY};

    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
    let persist: IPersistFile = link.cast().ok()?;
    let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    persist.Load(PCWSTR(path_wide.as_ptr()), STGM_READ).ok()?;
    let mut buffer = [0u16; 260];
    link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_UNCPRIORITY.0 as u32)
        .ok()?;
    let target = String::from_utf16_lossy(&buffer);
    let trimmed = target.trim_matches('\0').trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn get_registered_app_path(exe_name: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ, REG_SZ,
    };

    let subkey_str = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}\0",
        exe_name
    );
    let subkey_wide: Vec<u16> = subkey_str.encode_utf16().collect();

    for root in &[HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        unsafe {
            if RegOpenKeyExW(*root, PCWSTR(subkey_wide.as_ptr()), 0, KEY_READ, &mut hkey).is_ok() {
                let mut val_type = windows::Win32::System::Registry::REG_VALUE_TYPE::default();
                let mut buf_size = 512u32;
                let mut buf = vec![0u16; 256];
                if RegQueryValueExW(
                    hkey,
                    PCWSTR(std::ptr::null()),
                    None,
                    Some(&mut val_type),
                    Some(buf.as_mut_ptr() as *mut u8),
                    Some(&mut buf_size),
                )
                .is_ok()
                {
                    let _ = RegCloseKey(hkey);
                    if val_type == REG_SZ {
                        let len = (buf_size / 2) as usize;
                        let mut end = len;
                        while end > 0 && (buf[end - 1] == 0 || buf[end - 1] == 32) {
                            end -= 1;
                        }
                        let path = String::from_utf16_lossy(&buf[..end]);
                        let path_clean = path.trim_matches('"').to_string();
                        if std::path::Path::new(&path_clean).exists() {
                            return Some(path_clean);
                        }
                    }
                } else {
                    let _ = RegCloseKey(hkey);
                }
            }
        }
    }
    None
}

unsafe fn get_app_icon(path: &str) -> HICON {
    let mut hicon = HICON(null_mut());

    // Resolve shortcut if it ends in .lnk to bypass shortcut arrow overlay
    let mut target_path = path.to_string();
    if target_path.to_lowercase().ends_with(".lnk") {
        if let Some(resolved) = resolve_lnk(&target_path) {
            target_path = resolved;
        }
    }

    // Format virtual paths properly for SHCreateItemFromParsingName
    let parsing_path = if target_path.starts_with("shell:AppsFolder\\") {
        target_path.clone()
    } else if !target_path.contains(":\\") && !target_path.starts_with("\\\\") {
        format!("shell:AppsFolder\\{}", target_path)
    } else {
        target_path.clone()
    };

    let path_wide: Vec<u16> = parsing_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // Try parsing as a shell item to get icon from virtual Applications folder or normal file
    let shell_item: Option<windows::Win32::UI::Shell::IShellItem> =
        windows::Win32::UI::Shell::SHCreateItemFromParsingName(PCWSTR(path_wide.as_ptr()), None)
            .ok();

    if let Some(item) = &shell_item {
        // 1. Try modern IShellItemImageFactory first
        let factory: Option<windows::Win32::UI::Shell::IShellItemImageFactory> = item.cast().ok();
        if let Some(f) = factory {
            let res = f.GetImage(
                windows::Win32::Foundation::SIZE { cx: 32, cy: 32 },
                windows::Win32::UI::Shell::SIIGBF_ICONONLY,
            );
            if let Ok(hbitmap) = res {
                let hbm_mask = windows::Win32::Graphics::Gdi::CreateBitmap(32, 32, 1, 1, None);
                if !hbm_mask.is_invalid() {
                    let mut ii = windows::Win32::UI::WindowsAndMessaging::ICONINFO {
                        fIcon: windows::Win32::Foundation::TRUE,
                        xHotspot: 0,
                        yHotspot: 0,
                        hbmMask: hbm_mask,
                        hbmColor: hbitmap,
                    };
                    if let Ok(hi) =
                        windows::Win32::UI::WindowsAndMessaging::CreateIconIndirect(&mut ii)
                    {
                        hicon = hi;
                    }
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(hbm_mask);
                }
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(hbitmap);
            }
        }

        // 2. Fall back to legacy SHGetFileInfoW with PIDL
        if hicon.0.is_null() {
            if let Ok(pidl) = windows::Win32::UI::Shell::SHGetIDListFromObject(item) {
                let mut shfi = windows::Win32::UI::Shell::SHFILEINFOW::default();
                let flags = windows::Win32::UI::Shell::SHGFI_ICON
                    | windows::Win32::UI::Shell::SHGFI_LARGEICON
                    | windows::Win32::UI::Shell::SHGFI_PIDL;
                let _ = windows::Win32::UI::Shell::SHGetFileInfoW(
                    PCWSTR(pidl as *const u16),
                    windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                    Some(&mut shfi),
                    std::mem::size_of::<windows::Win32::UI::Shell::SHFILEINFOW>() as u32,
                    flags,
                );
                hicon = shfi.hIcon;
                windows::Win32::UI::Shell::ILFree(Some(pidl));
            }
        }
    }

    // Fallback directly using path
    if hicon.0.is_null() {
        let mut shfi = windows::Win32::UI::Shell::SHFILEINFOW::default();
        let flags =
            windows::Win32::UI::Shell::SHGFI_ICON | windows::Win32::UI::Shell::SHGFI_LARGEICON;
        let fallback_wide: Vec<u16> = target_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let _ = windows::Win32::UI::Shell::SHGetFileInfoW(
            PCWSTR(fallback_wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut shfi),
            std::mem::size_of::<windows::Win32::UI::Shell::SHFILEINFOW>() as u32,
            flags,
        );
        hicon = shfi.hIcon;
    }

    hicon
}

unsafe fn get_process_path(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let res = QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_WIN32,
        PWSTR(buffer.as_mut_ptr()),
        &mut size,
    );
    let _ = CloseHandle(handle);

    if res.is_ok() && size > 0 {
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    } else {
        None
    }
}

unsafe fn get_file_icon(path: &str) -> HICON {
    let mut shfi = windows::Win32::UI::Shell::SHFILEINFOW::default();
    let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let flags = windows::Win32::UI::Shell::SHGFI_ICON | windows::Win32::UI::Shell::SHGFI_LARGEICON;
    let res = windows::Win32::UI::Shell::SHGetFileInfoW(
        PCWSTR(path_wide.as_ptr()),
        windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut shfi),
        std::mem::size_of::<windows::Win32::UI::Shell::SHFILEINFOW>() as u32,
        flags,
    );
    if res != 0 {
        shfi.hIcon
    } else {
        HICON(null_mut())
    }
}

fn owned_icon_eviction_key(
    owned: &std::collections::HashSet<String>,
    keep: &str,
) -> Option<String> {
    owned.iter().find(|key| key.as_str() != keep).cloned()
}

unsafe fn prune_owned_icon_cache(s: &mut State, keep: &str) {
    while s.app_icons_owned.len() > MAX_OWNED_ICON_CACHE {
        let Some(key) = owned_icon_eviction_key(&s.app_icons_owned, keep) else {
            break;
        };
        s.app_icons_owned.remove(&key);
        if let Some(hicon) = s.app_icons.remove(&key) {
            if !hicon.0.is_null() {
                let _ = DestroyIcon(hicon);
            }
        }
    }
}

unsafe fn trigger_icon_loading(_hwnd: HWND, s: &mut State) {
    if s.icon_tx.is_none() {
        return;
    }
    let tx = s.icon_tx.as_ref().unwrap();
    for res in &s.results {
        let (source, key) = (res.entry.source.as_str(), res.entry.launch_command.clone());
        // For WINDOW source: fetch icon synchronously on the UI thread (fast, only called once
        // when results arrive — not on every paint frame) and cache it in app_icons.
        // Wrap in catch_unwind to prevent UI crash if target window handle is invalid
        // or shell extension panics during icon extraction.
        if source == "WINDOW" && !s.app_icons.contains_key(&key) {
            let hwnd_val = key
                .strip_prefix("window:")
                .and_then(|h| h.parse::<isize>().ok())
                .unwrap_or(0);
            let win_hwnd = HWND(hwnd_val as *mut std::ffi::c_void);
            let hicon = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                get_window_icon(win_hwnd)
            }))
            .unwrap_or(HICON(std::ptr::null_mut()));
            s.app_icons.insert(key.clone(), hicon);
            continue;
        }
        let is_kill_action = source == "ACTION" && key.starts_with("kill:");
        let is_settings = key.starts_with("ms-settings:")
            || key.starts_with("control")
            || key.contains(".cpl")
            || key.ends_with(".msc");
        let needs_icon =
            (source == "app" || icon_file_path(source, &key).is_some() || is_kill_action)
                && !is_settings
                && !s.app_icons.contains_key(&key)
                && !s.pending_icons.contains(&key);
        if needs_icon {
            // Track pending loads so we don't spawn multiple threads for same path
            s.pending_icons.insert(key.clone());
            let _ = tx.send(IconRequest {
                key,
                source: source.to_string(),
            });
        }
    }
}

fn known_folder_icon_path(key: &str) -> Option<String> {
    let folder_name = key
        .strip_prefix("folder:")
        .or_else(|| key.strip_prefix("action:folder:"))?;
    let folder_id = match folder_name {
        "downloads" => &windows::Win32::UI::Shell::FOLDERID_Downloads,
        "desktop" => &windows::Win32::UI::Shell::FOLDERID_Desktop,
        "documents" => &windows::Win32::UI::Shell::FOLDERID_Documents,
        "pictures" => &windows::Win32::UI::Shell::FOLDERID_Pictures,
        "music" => &windows::Win32::UI::Shell::FOLDERID_Music,
        "videos" => &windows::Win32::UI::Shell::FOLDERID_Videos,
        _ => return None,
    };
    crate::launcher::get_known_folder_path(folder_id)
}

fn icon_file_path(source: &str, key: &str) -> Option<String> {
    if source == "FOLDER" {
        if let Some(path) = known_folder_icon_path(key) {
            return Some(path);
        }
        if !key.ends_with(':') && std::path::Path::new(key).exists() {
            return Some(key.to_string());
        }
    } else if source == "ACTION" && key.starts_with("action:folder:") {
        return known_folder_icon_path(key);
    } else if source == "ACTION" && key.eq_ignore_ascii_case("taskmgr.exe") {
        return task_manager_icon_path();
    } else if is_file_result_source(source) {
        if std::path::Path::new(key).exists() {
            return Some(key.to_string());
        }
    } else if source == "PROJECT"
        && !key.is_empty()
        && !key.starts_with("http")
        && std::path::Path::new(key).exists()
    {
        return Some(key.to_string());
    }
    None
}

fn task_manager_icon_path() -> Option<String> {
    let path = std::path::PathBuf::from(std::env::var("SystemRoot").ok()?)
        .join("System32")
        .join("taskmgr.exe");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

fn is_file_result_source(source: &str) -> bool {
    matches!(
        source,
        "RECENT" | "FILE" | "FILE_CONTENT" | "CODE" | "CODE_CONTENT" | "OCR" | "PDF" | "DOCX"
    )
}

fn is_content_match_source(source: &str) -> bool {
    matches!(
        source,
        "FILE_CONTENT" | "CODE_CONTENT" | "OCR" | "PDF" | "DOCX"
    )
}

fn is_windows_settings_command(command: &str) -> bool {
    command.starts_with("ms-settings:")
        || command.starts_with("control")
        || command.contains(".cpl")
        || command.ends_with(".msc")
}

fn image_path_for_result(result: &SearchResult) -> Option<&str> {
    let path = result
        .entry
        .launch_command
        .strip_prefix("copy_image:")
        .unwrap_or(&result.entry.launch_command);
    let extension = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp"
    )
    .then_some(path)
}

unsafe fn draw_spinner(hdc: HDC, x: i32, y: i32, size: i32, tick: usize, color: COLORREF) {
    let center_x = x + size / 2;
    let center_y = y + size / 2;
    let r = size / 2 - 2;

    for i in 0..8 {
        let angle = (i as f32) * 2.0 * std::f32::consts::PI / 8.0;
        let dx = (angle.cos() * r as f32) as i32;
        let dy = (angle.sin() * r as f32) as i32;

        let diff = (i + 8 - tick) % 8;
        let dot_color = if diff == 0 {
            color
        } else {
            let r_val = ((color.0 & 0xFF) * (8 - diff) as u32 / 8) as u8;
            let g_val = (((color.0 >> 8) & 0xFF) * (8 - diff) as u32 / 8) as u8;
            let b_val = (((color.0 >> 16) & 0xFF) * (8 - diff) as u32 / 8) as u8;
            COLORREF(r_val as u32 | ((g_val as u32) << 8) | ((b_val as u32) << 16))
        };

        let br = CreateSolidBrush(dot_color);
        let rect = RECT {
            left: center_x + dx - 2,
            top: center_y + dy - 2,
            right: center_x + dx + 2,
            bottom: center_y + dy + 2,
        };
        let _ = FillRect(hdc, &rect, br);
        let _ = DeleteObject(br);
    }
}

// Draw a result-row icon at `x_base` (the row's normal icon origin) for row top `ry`.
// Agent icons (icon_agent / icon_agent_chat) render at AGENT_ICON_SIZE — larger than other
// result icons — while staying centered on the same point as a RESULT_ICON_SIZE icon, so
// row alignment is unchanged and the larger glyph never overlaps the result text.
unsafe fn draw_result_icon(mdc: HDC, s: &State, icon: HICON, x_base: i32, ry: i32) {
    if icon.0.is_null() {
        return;
    }
    let is_agent = icon.0 == s.icon_agent.0 || icon.0 == s.icon_agent_chat.0;
    let size = if is_agent {
        AGENT_ICON_SIZE
    } else {
        RESULT_ICON_SIZE
    };
    let ix = x_base - (size - RESULT_ICON_SIZE) / 2;
    let iy = centered_in_result_row(ry, size, s.app_settings.item_height as i32);
    let _ = DrawIconEx(
        mdc,
        ix,
        iy,
        icon,
        size,
        size,
        0,
        HBRUSH(null_mut()),
        DI_NORMAL,
    );
}

// ── Cached GDI back-buffer for the launcher paint loop ──────────────────────────
// Reused across paints so an animation frame doesn't allocate a full-window bitmap +
// DC every time (paint() runs on every TIMER_SEARCH_ANIM tick during the fade/grow).
// Sized to the largest window seen; grows as needed, never shrinks. Single UI thread.
// ponytail: leaks one DC+bitmap at process exit (reclaimed by the OS), and is only
// recreated on grow — a display-format change isn't handled (rare for this window).
struct BackBuffer {
    mdc: windows::Win32::Graphics::Gdi::HDC,
    bmp: windows::Win32::Graphics::Gdi::HBITMAP,
    w: i32,
    h: i32,
}
thread_local! {
    static BACK_BUFFER: std::cell::RefCell<BackBuffer> = std::cell::RefCell::new(BackBuffer {
        mdc: windows::Win32::Graphics::Gdi::HDC(null_mut()),
        bmp: windows::Win32::Graphics::Gdi::HBITMAP(null_mut()),
        w: 0,
        h: 0,
    });
}

/// A memory DC (with a compatible bitmap already selected) sized at least win_w×win_h,
/// reusing the cached buffer when it's big enough.
unsafe fn ensure_back_buffer(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    win_w: i32,
    win_h: i32,
) -> windows::Win32::Graphics::Gdi::HDC {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, SelectObject,
    };
    BACK_BUFFER.with(|bb| {
        let mut bb = bb.borrow_mut();
        if bb.mdc.0.is_null() || bb.w < win_w || bb.h < win_h {
            let new_w = win_w.max(bb.w).max(1);
            let new_h = win_h.max(bb.h).max(1);
            if !bb.mdc.0.is_null() {
                let _ = DeleteObject(bb.bmp);
                let _ = DeleteDC(bb.mdc);
            }
            let mdc = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, new_w, new_h);
            SelectObject(mdc, bmp);
            bb.mdc = mdc;
            bb.bmp = bmp;
            bb.w = new_w;
            bb.h = new_h;
        }
        bb.mdc
    })
}

// System timer resolution is raised to ~1ms while the launcher is visible so the 3–8ms
// SetTimer animation ticks fire on time (default ~15ms granularity makes the fade/height
// grow visibly steppy). raise on show, lower when fully hidden — balanced by the flag.
thread_local! {
    static HIRES_TIMER: std::cell::Cell<bool> = std::cell::Cell::new(false);
}
unsafe fn raise_timer_res() {
    HIRES_TIMER.with(|f| {
        if !f.get() {
            let _ = windows::Win32::Media::timeBeginPeriod(1);
            f.set(true);
        }
    });
}
unsafe fn lower_timer_res() {
    HIRES_TIMER.with(|f| {
        if f.get() {
            let _ = windows::Win32::Media::timeEndPeriod(1);
            f.set(false);
        }
    });
}

unsafe fn paint(hwnd: HWND, s: &State) {
    #[allow(non_snake_case)]
    let SEARCH_H = s.search_h();
    let palette = s.theme.palette();
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);

    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let win_w = rc.right - rc.left;
    let win_h = rc.bottom - rc.top;

    // Double-buffer — cached across paints (see ensure_back_buffer) so an animation
    // frame doesn't allocate a full-window bitmap + DC every time.
    let mdc = ensure_back_buffer(hdc, win_w, win_h);

    // Clear background with COLOR_KEY (completely transparent)
    fill(mdc, 0, 0, win_w, win_h, COLOR_KEY);

    if s.color_picker_active {
        // Draw the magnifier and picked color overlay under the cursor
        let screen_dc = GetDC(HWND(null_mut()));
        let mut pt_screen = POINT {
            x: s.color_picker_mx,
            y: s.color_picker_my,
        };
        let _ = ClientToScreen(hwnd, &mut pt_screen);
        let pixel = GetPixel(screen_dc, pt_screen.x, pt_screen.y);

        let zoom_w = 117;
        let zoom_h = 117;
        let mut draw_x = s.color_picker_mx + 25;
        let mut draw_y = s.color_picker_my + 25;

        if draw_x + zoom_w + 20 > win_w {
            draw_x = s.color_picker_mx - zoom_w - 25;
        }
        if draw_y + zoom_h + 80 > win_h {
            draw_y = s.color_picker_my - zoom_h - 25;
        }

        let src_x = pt_screen.x - 6;
        let src_y = pt_screen.y - 6;

        let _ = StretchBlt(
            mdc, draw_x, draw_y, zoom_w, zoom_h, screen_dc, src_x, src_y, 13, 13, SRCCOPY,
        );

        // Draw magnifier border using fill lines
        fill(
            mdc,
            draw_x - 2,
            draw_y - 2,
            zoom_w + 4,
            2,
            s.theme.palette().clr_white,
        );
        fill(
            mdc,
            draw_x - 2,
            draw_y + zoom_h,
            zoom_w + 4,
            2,
            s.theme.palette().clr_white,
        );
        fill(
            mdc,
            draw_x - 2,
            draw_y - 2,
            2,
            zoom_h + 4,
            s.theme.palette().clr_white,
        );
        fill(
            mdc,
            draw_x + zoom_w,
            draw_y - 2,
            2,
            zoom_h + 4,
            s.theme.palette().clr_white,
        );

        // Draw central pixel highlight box (9x9)
        let cx_box = draw_x + 54;
        let cy_box = draw_y + 54;
        fill(
            mdc,
            cx_box - 1,
            cy_box - 1,
            9 + 2,
            1,
            s.theme.palette().clr_white,
        );
        fill(
            mdc,
            cx_box - 1,
            cy_box + 9,
            9 + 2,
            1,
            s.theme.palette().clr_white,
        );
        fill(mdc, cx_box - 1, cy_box, 1, 9, s.theme.palette().clr_white);
        fill(mdc, cx_box + 9, cy_box, 1, 9, s.theme.palette().clr_white);

        // Draw color info box below magnifier
        let info_y = draw_y + zoom_h + 6;
        fill_rounded(
            mdc,
            draw_x - 1,
            info_y - 1,
            zoom_w + 2,
            44 + 2,
            6,
            s.theme.palette().clr_div,
        );
        fill_rounded(mdc, draw_x, info_y, zoom_w, 44, 6, s.theme.palette().bg);

        let r_comp = (pixel.0 & 0xFF) as u8;
        let g_comp = ((pixel.0 >> 8) & 0xFF) as u8;
        let b_comp = ((pixel.0 >> 16) & 0xFF) as u8;
        fill_rounded(mdc, draw_x + 8, info_y + 8, 28, 28, 14, pixel);

        SelectObject(mdc, s.font_b);
        SetTextColor(mdc, s.theme.palette().clr_white);
        SetBkMode(mdc, TRANSPARENT);
        let hex_str = format!("#{:02X}{:02X}{:02X}", r_comp, g_comp, b_comp);
        let mut hex_wide: Vec<u16> = hex_str.encode_utf16().collect();
        let mut text_rect = RECT {
            left: draw_x + 42,
            top: info_y + 6,
            right: draw_x + zoom_w - 4,
            bottom: info_y + 22,
        };
        let _ = DrawTextW(
            mdc,
            &mut hex_wide,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        SelectObject(mdc, s.font_c);
        SetTextColor(mdc, s.theme.palette().clr_gray);
        let rgb_str = format!("{},{},{}", r_comp, g_comp, b_comp);
        let mut rgb_wide: Vec<u16> = rgb_str.encode_utf16().collect();
        let mut rgb_rect = RECT {
            left: draw_x + 42,
            top: info_y + 22,
            right: draw_x + zoom_w - 4,
            bottom: info_y + 38,
        };
        let _ = DrawTextW(
            mdc,
            &mut rgb_wide,
            &mut rgb_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        let _ = ReleaseDC(HWND(null_mut()), screen_dc);

        let _ = BitBlt(hdc, 0, 0, win_w, win_h, mdc, 0, 0, SRCCOPY);
        // back-buffer is cached — don't delete mdc/bmp here
        let _ = EndPaint(hwnd, &ps);
        return;
    }

    // Calculate dynamic shape coordinates
    let p = s.current_p();
    let t = ease_out(p);

    let pill_w = 96;
    let pill_h = SEARCH_H;
    let pill_r = 32;

    let end_w = win_w;
    let end_h = s.paint_win_h();

    let w = (pill_w as f32 + (end_w - pill_w) as f32 * t) as i32;
    let h = (pill_h as f32 + (end_h - pill_h) as f32 * t) as i32;
    let x = (win_w - w) / 2;
    let start_y = s.cy - pill_h / 2;
    let end_y = s.launcher_top_y();
    let y = (start_y as f32 + (end_y - start_y) as f32 * t) as i32;
    let r = (pill_r as f32 + (8 - pill_r) as f32 * t) as i32;

    // Fill background / Draw Glowing Border around the morphing rounded rect
    draw_rounded_border_and_bg(mdc, x, y, w, h, r, palette.bg, palette.clr_div);

    // Create rounded clipping region matching the inner background area of the morphing shape
    let clip_rgn = CreateRoundRectRgn(x + 1, y + 1, x + w - 1, y + h - 1, r - 1, r - 1);
    let _ = SelectClipRgn(mdc, clip_rgn);

    // ── Search row ────────────────────────────────────────────────────────
    SetBkMode(mdc, TRANSPARENT);

    // Draw Search Icon
    if !s.icon_new_search.0.is_null() {
        let icon_y = y + (SEARCH_H - SEARCH_ICON_SIZE) / 2;
        let _ = DrawIconEx(
            mdc,
            x + PAD_L,
            icon_y,
            s.icon_new_search,
            SEARCH_ICON_SIZE,
            SEARCH_ICON_SIZE,
            0,
            HBRUSH(null_mut()),
            DI_NORMAL,
        );
    }

    // Text / placeholder
    let tx = x + PAD_L + SEARCH_ICON_SIZE + 12;
    let show_web_action = s.shows_web_search_action();
    let show_lens_action = s.shows_lens_action();
    let web_reserve = if show_web_action {
        SEARCH_WEB_BUTTON_W + SEARCH_ACTION_GAP + (if show_lens_action { 0 } else { 8 })
    } else {
        0
    };
    let lens_reserve = if show_lens_action {
        SEARCH_LENS_BUTTON_SIZE + 12
    } else {
        0
    };
    let loading_reserve = if s.search_loading { 166 } else { 0 };
    let settings_reserve = SEARCH_LENS_BUTTON_SIZE + 12;
    let right_reserve = PAD_L + lens_reserve + web_reserve + settings_reserve + loading_reserve;
    let tw = w - (PAD_L + SEARCH_ICON_SIZE + 12) - right_reserve;
    let mut tr = RECT {
        left: tx,
        top: y,
        right: tx + tw,
        bottom: y + SEARCH_H,
    };

    let web_rect = if show_web_action {
        Some(search_web_action_rect(
            x,
            y,
            w,
            s.search_h(),
            show_lens_action,
        ))
    } else {
        None
    };
    let lens_rect = if show_lens_action {
        Some(search_lens_action_rect(x, y, w, s.search_h()))
    } else {
        None
    };

    if s.search_loading {
        let spinner_size = 16;
        let spinner_right = web_rect
            .map(|r| r.left - 14)
            .or_else(|| lens_rect.map(|r| r.left - 14))
            .unwrap_or(x + w - PAD_L - 4);
        let spinner_x = spinner_right - spinner_size;
        let spinner_y = y + (SEARCH_H - spinner_size) / 2;
        draw_spinner(
            mdc,
            spinner_x,
            spinner_y,
            spinner_size,
            s.search_anim_tick,
            palette.clr_accent,
        );

        SelectObject(mdc, s.font_c);
        SetTextColor(mdc, palette.clr_ph);
        let mut load_text: Vec<u16> = "Searching content...".encode_utf16().collect();
        let mut load_rect = RECT {
            left: (spinner_x - 140).max(tx),
            top: y,
            right: spinner_x - 8,
            bottom: y + SEARCH_H,
        };
        let _ = DrawTextW(
            mdc,
            &mut load_text,
            &mut load_rect,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }

    if let Some(web_rect) = web_rect {
        let btn_w = web_rect.right - web_rect.left;
        let btn_h = web_rect.bottom - web_rect.top;
        fill_rounded(
            mdc,
            web_rect.left,
            web_rect.top,
            btn_w,
            btn_h,
            10,
            if s.hovered_search_action == Some(SearchActionHover::Web) {
                palette.bg_hover
            } else {
                palette.clr_bdgbg
            },
        );
        if !s.icon_web_action.0.is_null() {
            let icon_size = 18;
            let icon_x = web_rect.left + 10;
            let icon_y = web_rect.top + (btn_h - icon_size) / 2;
            let _ = DrawIconEx(
                mdc,
                icon_x,
                icon_y,
                s.icon_web_action,
                icon_size,
                icon_size,
                0,
                HBRUSH(null_mut()),
                DI_NORMAL,
            );
        }

        SelectObject(mdc, s.font_c);
        SetTextColor(
            mdc,
            if s.hovered_search_action == Some(SearchActionHover::Web) {
                palette.clr_white
            } else {
                palette.clr_bdgtx
            },
        );
        let mut web_label: Vec<u16> = "Search web".encode_utf16().collect();
        let mut web_label_rect = RECT {
            left: web_rect.left + 34,
            top: web_rect.top,
            right: web_rect.right - 6,
            bottom: web_rect.bottom,
        };
        let _ = DrawTextW(
            mdc,
            &mut web_label,
            &mut web_label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }

    if let Some(lens_rect) = lens_rect {
        let btn_w = lens_rect.right - lens_rect.left;
        let btn_h = lens_rect.bottom - lens_rect.top;
        fill_rounded(
            mdc,
            lens_rect.left,
            lens_rect.top,
            btn_w,
            btn_h,
            10,
            if s.hovered_search_action == Some(SearchActionHover::Lens) {
                palette.bg_hover
            } else {
                palette.clr_bdgbg
            },
        );
        if !s.icon_lens.0.is_null() {
            let icon_x = lens_rect.left + (btn_w - SEARCH_LENS_ICON_SIZE) / 2;
            let icon_y = lens_rect.top + (btn_h - SEARCH_LENS_ICON_SIZE) / 2;
            let _ = DrawIconEx(
                mdc,
                icon_x,
                icon_y,
                s.icon_lens,
                SEARCH_LENS_ICON_SIZE,
                SEARCH_LENS_ICON_SIZE,
                0,
                HBRUSH(null_mut()),
                DI_NORMAL,
            );
        }
    }

    let settings_rect = search_settings_action_rect(x, y, w, s.search_h(), show_lens_action, show_web_action);
    {
        let btn_w = settings_rect.right - settings_rect.left;
        let btn_h = settings_rect.bottom - settings_rect.top;
        fill_rounded(
            mdc,
            settings_rect.left,
            settings_rect.top,
            btn_w,
            btn_h,
            10,
            if s.hovered_search_action == Some(SearchActionHover::Settings) {
                palette.bg_hover
            } else {
                palette.clr_bdgbg
            },
        );
        if !s.icon_settings.0.is_null() {
            let icon_x = settings_rect.left + (btn_w - SEARCH_LENS_ICON_SIZE) / 2;
            let icon_y = settings_rect.top + (btn_h - SEARCH_LENS_ICON_SIZE) / 2;
            let _ = DrawIconEx(
                mdc,
                icon_x,
                icon_y,
                s.icon_settings,
                SEARCH_LENS_ICON_SIZE,
                SEARCH_LENS_ICON_SIZE,
                0,
                HBRUSH(null_mut()),
                DI_NORMAL,
            );
        }
    }

    if let Some(ref override_msg) = s.placeholder_override {
        let right_limit = settings_rect.left - 16;
        let mut msg_text: Vec<u16> = override_msg.encode_utf16().collect();
        let mut msg_rect = RECT {
            left: right_limit - 220,
            top: y,
            right: right_limit,
            bottom: y + SEARCH_H,
        };
        SelectObject(mdc, s.font_c);
        SetTextColor(mdc, palette.clr_accent);
        let _ = DrawTextW(
            mdc,
            &mut msg_text,
            &mut msg_rect,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SetTextColor(mdc, palette.clr_white);
    }

    SelectObject(mdc, s.font_q);
    SetTextColor(mdc, palette.clr_white);

    if s.query.is_empty()
        && (s.app_settings.show_placeholder || !matches!(s.form_state, FormState::None))
    {
        let ph_str = match &s.form_state {
            FormState::CreateSnippetName => "Create Snippet: Enter Name...",
            FormState::CreateSnippetContent { .. } => "Create Snippet: Enter Content...",
            FormState::CreateSnippetKeyword { .. } => "Create Snippet: Enter Keyword (optional)...",
            FormState::CreateQuicklinkName => "Create Quicklink: Enter Name...",
            FormState::CreateQuicklinkUrl { .. } => {
                "Create Quicklink: Enter URL (use {query} placeholder)..."
            }
            FormState::CreateQuicklinkKeyword { .. } => "Create Quicklink: Enter Keyword...",
            FormState::CreateFocusCategoryName => "Create Focus Category: Enter Name...",
            FormState::CreateFocusCategoryBlocked { .. } => "Focus Category: Enter blocked apps (comma separated, e.g. discord.exe, slack.exe)...",
            FormState::CreateNoteName => "Create Note: Enter Title...",
            FormState::None => "Search files, folders, code, settings...",
        };
        let mut ph: Vec<u16> = ph_str.encode_utf16().collect();
        SetTextColor(mdc, palette.clr_ph);
        let _ = DrawTextW(
            mdc,
            &mut ph,
            &mut tr,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SetTextColor(mdc, palette.clr_white);
    } else {
        if !s.query.is_empty() {
            let cur = floor_char_boundary(&s.query, s.cursor_pos);
            let before = &s.query[..cur];
            let dw_before: Vec<u16> = before.encode_utf16().collect();
            let mut size = SIZE::default();
            if !dw_before.is_empty() {
                let _ = GetTextExtentPoint32W(mdc, &dw_before, &mut size);
            }
            let max_w = w - 80;
            let mut scroll_x = 0;
            if size.cx > max_w {
                scroll_x = size.cx - max_w;
            }
            let mut dw_query: Vec<u16> = s.query.encode_utf16().collect();
            let mut text_rect = tr;
            text_rect.left -= scroll_x;
            text_rect.right += 2000; // prevent clipping the remaining text
            if s.text_selected && search_input_caret_active(s) {
                let mut all_size = SIZE::default();
                let _ = GetTextExtentPoint32W(mdc, &dw_query, &mut all_size);
                fill(
                    mdc,
                    tr.left - scroll_x,
                    tr.top + 12,
                    all_size.cx.min(tw),
                    (tr.bottom - tr.top - 24).max(20),
                    palette.clr_accent,
                );
            }
            let _ = DrawTextW(
                mdc,
                &mut dw_query,
                &mut text_rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }

    // Draw cursor
    if s.cursor_visible && search_input_caret_active(s) {
        let cur = floor_char_boundary(&s.query, s.cursor_pos);
        let before = &s.query[..cur];
        let dw_before: Vec<u16> = before.encode_utf16().collect();
        let mut size = SIZE::default();
        if !dw_before.is_empty() {
            let _ = GetTextExtentPoint32W(mdc, &dw_before, &mut size);
        }
        let max_w = w - 80;
        let mut scroll_x = 0;
        if size.cx > max_w {
            scroll_x = size.cx - max_w;
        }
        let cursor_x = tr.left - scroll_x + size.cx;

        let mut dummy_size = SIZE::default();
        let _ = GetTextExtentPoint32W(mdc, &['A' as u16], &mut dummy_size);
        let text_h = dummy_size.cy;
        let cursor_top = tr.top + (tr.bottom - tr.top - text_h) / 2;
        fill(
            mdc,
            cursor_x,
            cursor_top,
            2,
            text_h,
            s.theme.palette().clr_white,
        );
        // Remember where the caret is so the blink timer can repaint just this sliver.
        s.caret_rect.set(RECT {
            left: cursor_x - 1,
            top: cursor_top - 1,
            right: cursor_x + 3,
            bottom: cursor_top + text_h + 1,
        });
    }
    // ── Note editor panel (self-rendered) ──────────────────────────────────
    if s.note_editing {
        let pad = 24;
        let body_top = y + SEARCH_H + 1;
        fill(mdc, x, y + SEARCH_H, w, 1, s.theme.palette().clr_div);

        let title_str = s
            .note_path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_stem())
            .and_then(|n| n.to_str())
            .unwrap_or("Note")
            .to_string();
        SelectObject(mdc, s.font_n);
        SetTextColor(mdc, s.theme.palette().clr_white);
        let mut title: Vec<u16> = title_str.encode_utf16().collect();
        let mut title_rc = RECT {
            left: x + pad,
            top: body_top + 12,
            right: x + w - pad,
            bottom: body_top + 42,
        };
        let _ = DrawTextW(
            mdc,
            &mut title,
            &mut title_rc,
            DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );

        let content_top = body_top + 48;
        let footer_h = 30;
        let actual_panel_h = s.paint_win_h() - SEARCH_H - 1;
        let content_bottom = y + SEARCH_H + 1 + actual_panel_h - footer_h;

        // Body — append a caret block so the user sees the insertion point.
        SelectObject(mdc, s.font_c);
        SetTextColor(mdc, s.theme.palette().clr_white);
        let shown = if s.cursor_visible && s.note_editing {
            format!("{}\u{2588}", s.note_text)
        } else {
            s.note_text.clone()
        };
        let mut body: Vec<u16> = shown.encode_utf16().collect();
        let mut calc = RECT {
            left: x + pad,
            top: 0,
            right: x + w - pad,
            bottom: 0,
        };
        let _ = DrawTextW(
            mdc,
            &mut body.clone(),
            &mut calc,
            DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
        );
        let total_h = calc.bottom - calc.top;
        let view_h = content_bottom - content_top;
        let max_scroll = (total_h - view_h).max(0);
        let scroll = s.note_scroll.clamp(0, max_scroll);
        let mut body_rc = RECT {
            left: x + pad,
            top: content_top - scroll,
            right: x + w - pad,
            bottom: content_top - scroll + total_h.max(view_h),
        };
        let _ = DrawTextW(
            mdc,
            &mut body,
            &mut body_rc,
            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
        );

        fill(
            mdc,
            x,
            content_bottom,
            w,
            footer_h + 4,
            s.theme.palette().bg,
        );
        fill(mdc, x, content_bottom, w, 1, s.theme.palette().clr_div);
        SelectObject(mdc, s.font_b);
        SetTextColor(mdc, s.theme.palette().clr_gray);
        let mut hint: Vec<u16> = "Esc: save & close     ·     Ctrl+S: save     ·     ↑ ↓ scroll"
            .encode_utf16()
            .collect();
        let mut hint_rc = RECT {
            left: x + pad,
            top: content_bottom + 2,
            right: x + w - pad,
            bottom: content_bottom + footer_h,
        };
        let _ = DrawTextW(
            mdc,
            &mut hint,
            &mut hint_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(mdc, s.font_q);
    }

    // ── AI answer panel ────────────────────────────────────────────────────
    if s.ai_pending || s.ai_answer.is_some() || s.chat_input_active {
        let pad = 24;
        let body_top = y + SEARCH_H + 1;
        fill(mdc, x, y + SEARCH_H, w, 1, s.theme.palette().clr_div);

        // Title (the command label)
        SelectObject(mdc, s.font_b);
        SetTextColor(mdc, s.theme.palette().clr_white);
        let mut title: Vec<u16> = s.ai_title.encode_utf16().collect();
        let mut title_rc = RECT {
            left: x + pad,
            top: body_top + 11,
            right: x + w - pad - 116,
            bottom: body_top + 35,
        };
        let _ = DrawTextW(
            mdc,
            &mut title,
            &mut title_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );

        if s.ai_pending {
            let dots = match s.ai_tick % 4 {
                0 => "",
                1 => ".",
                2 => "..",
                _ => "...",
            };
            fill_rounded(
                mdc,
                x + w - pad - 104,
                body_top + 11,
                104,
                24,
                10,
                COLORREF(0x00_34_3C_32),
            );
            SelectObject(mdc, s.font_b);
            SetTextColor(mdc, COLORREF(0x00_B8_D6_B4));
            let mut status: Vec<u16> = format!("Executing{}", dots).encode_utf16().collect();
            let mut status_rc = RECT {
                left: x + w - pad - 96,
                top: body_top + 11,
                right: x + w - pad - 8,
                bottom: body_top + 35,
            };
            let _ = DrawTextW(
                mdc,
                &mut status,
                &mut status_rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(mdc, s.font_n);
        }

        let content_top = body_top + 48;
        let footer_h = if s.ai_pending {
            30
        } else {
            62
        };
        let actual_panel_h = s.paint_win_h() - SEARCH_H - 1;
        let content_bottom = y + SEARCH_H + 1 + actual_panel_h - footer_h;

        let has_answer = s.ai_answer.is_some();
        if s.ai_pending && !has_answer {
            SelectObject(mdc, s.font_q);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let mut th: Vec<u16> = "Thinking…".encode_utf16().collect();
            let mut th_rc = RECT {
                left: x + pad,
                top: content_top,
                right: x + w - pad,
                bottom: content_bottom,
            };
            let _ = DrawTextW(
                mdc,
                &mut th,
                &mut th_rc,
                DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
            );
        } else if s.chat_input_active && !has_answer && !s.ai_pending {
            // Empty agent chat - show a subtle welcome prompt
            SelectObject(mdc, s.font_q);
            SetTextColor(mdc, s.theme.palette().clr_ph);
            let agent_name = s
                .ai_title
                .strip_prefix('@')
                .and_then(|t| t.split_once(':').map(|(n, _)| n.trim()))
                .unwrap_or("Agent");
            let placeholder = format!("Ask {} anything…", agent_name);
            let mut ph: Vec<u16> = placeholder.encode_utf16().collect();
            let ph_mid = content_top + (content_bottom - content_top) / 2 - 20;
            let mut ph_rc = RECT {
                left: x + pad,
                top: ph_mid,
                right: x + w - pad,
                bottom: ph_mid + 40,
            };
            let _ = DrawTextW(
                mdc,
                &mut ph,
                &mut ph_rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        } else if let Some(ans) = &s.ai_answer {
            let parts: Vec<&str> = ans.split("\n\n---\n\n").collect();

            // 1. Measure Pass
            let mut total_h = 0;
            // User messages render as right-aligned bubbles that hug their content but
            // never span the whole panel (a short "hi" used to draw a full-width bar).
            let max_bubble_inner = (w - pad * 2) * 72 / 100 - 24;
            let resp_w = w - pad * 2;

            for part in &parts {
                let mut prompt = "";
                let mut response = "";
                if part.starts_with("User: ") {
                    let after_user = &part["User: ".len()..];
                    if let Some((p, r)) = after_user.split_once("\n\n") {
                        prompt = p.trim();
                        response = r.trim();
                    } else {
                        prompt = after_user.trim();
                    }
                } else {
                    response = part.trim();
                }

                if !prompt.is_empty() {
                    let mut p_wide: Vec<u16> = prompt.encode_utf16().collect();
                    SelectObject(mdc, s.font_c);
                    let inner_w = bubble_inner_w(mdc, &p_wide, max_bubble_inner);
                    let mut calc = RECT {
                        left: 0,
                        top: 0,
                        right: inner_w,
                        bottom: 0,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut p_wide,
                        &mut calc,
                        DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                    );
                    let prompt_h = calc.bottom - calc.top;
                    total_h += prompt_h + 16 + 16;
                }

                if !response.is_empty() {
                    let is_thinking = response == "Thinking..." || response == "Executing...";
                    let resp_h = if is_thinking {
                        // Plain single-line height for the animated status text.
                        let mut r_wide: Vec<u16> = response.encode_utf16().collect();
                        let mut calc = RECT {
                            left: 0,
                            top: 0,
                            right: resp_w,
                            bottom: 0,
                        };
                        SelectObject(mdc, s.font_c);
                        let _ = DrawTextW(
                            mdc,
                            &mut r_wide,
                            &mut calc,
                            DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                        );
                        calc.bottom - calc.top
                    } else {
                        // Markdown-rendered height (headings/code/lists/etc.).
                        measure_response(mdc, response, s, resp_w)
                    };
                    total_h += resp_h + 24;
                }
            }

            let view_h = content_bottom - content_top;
            let max_scroll = (total_h - view_h).max(0);
            // Cache these so input handlers (VK_DOWN/wheel) can decide whether the user
            // has scrolled back to the bottom without re-running the whole measure pass.
            s.ai_content_height.set(total_h);
            s.ai_view_height.set(view_h);
            // While pending OR when following the latest message, keep pinned to the bottom.
            let scroll = if s.ai_pending || s.ai_follow_bottom {
                max_scroll
            } else {
                s.ai_scroll.clamp(0, max_scroll)
            };

            // 2. Paint Pass
            let dc_state = SaveDC(mdc);
            let _ = IntersectClipRect(mdc, x + pad, content_top, x + w - pad, content_bottom);

            let mut current_y = content_top - scroll;
            let bg_user = COLORREF(0x00_2C_2B_2A);

            for part in &parts {
                let mut prompt = "";
                let mut response = "";
                if part.starts_with("User: ") {
                    let after_user = &part["User: ".len()..];
                    if let Some((p, r)) = after_user.split_once("\n\n") {
                        prompt = p.trim();
                        response = r.trim();
                    } else {
                        prompt = after_user.trim();
                    }
                } else {
                    response = part.trim();
                }

                if !prompt.is_empty() {
                    let mut p_wide: Vec<u16> = prompt.encode_utf16().collect();
                    SelectObject(mdc, s.font_c);
                    let inner_w = bubble_inner_w(mdc, &p_wide, max_bubble_inner);
                    let mut calc = RECT {
                        left: 0,
                        top: 0,
                        right: inner_w,
                        bottom: 0,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut p_wide.clone(),
                        &mut calc,
                        DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                    );
                    let prompt_h = calc.bottom - calc.top;
                    let bubble_h = prompt_h + 16;
                    let bubble_w = inner_w + 24;
                    // Right-aligned chat bubble (user side), sized to its content.
                    let bubble_x = x + w - pad - bubble_w;

                    fill_rounded(mdc, bubble_x, current_y, bubble_w, bubble_h, 8, bg_user);

                    let mut body_rc = RECT {
                        left: bubble_x + 12,
                        top: current_y + 8,
                        right: bubble_x + 12 + inner_w,
                        bottom: current_y + 8 + prompt_h,
                    };
                    SetTextColor(mdc, COLORREF(0x00_D0_D0_D0));
                    let _ = DrawTextW(
                        mdc,
                        &mut p_wide,
                        &mut body_rc,
                        DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
                    );

                    current_y += bubble_h + 16;
                }

                if !response.is_empty() {
                    let is_thinking = response == "Thinking..." || response == "Executing...";
                    let response_text = if is_thinking && s.ai_pending {
                        let dots = match s.ai_tick % 4 {
                            0 => "",
                            1 => ".",
                            2 => "..",
                            _ => "...",
                        };
                        format!("Executing task{}", dots)
                    } else {
                        response.to_string()
                    };
                    let mut r_wide: Vec<u16> = response_text.encode_utf16().collect();
                    let mut calc = RECT {
                        left: 0,
                        top: 0,
                        right: resp_w,
                        bottom: 0,
                    };
                    SelectObject(mdc, s.font_c);
                    let _ = DrawTextW(
                        mdc,
                        &mut r_wide.clone(),
                        &mut calc,
                        DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                    );
                    let resp_h = calc.bottom - calc.top;

                    let mut body_rc = RECT {
                        left: x + pad,
                        top: current_y,
                        right: x + w - pad,
                        bottom: current_y + resp_h,
                    };
                    if is_thinking {
                        SetTextColor(mdc, s.theme.palette().clr_gray);
                        let _ = DrawTextW(
                            mdc,
                            &mut r_wide,
                            &mut body_rc,
                            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
                        );
                        current_y += resp_h + 24;
                    } else {
                        // Markdown: paint using the shared renderer so heights match
                        // the measure pass exactly.
                        let used =
                            paint_response(mdc, &response_text, s, x + pad, resp_w, current_y);
                        current_y += used + 24;
                    }
                }
            }

            let _ = RestoreDC(mdc, dc_state);
        }

        // Footer / chat input (painted over any text overflow)
        fill(
            mdc,
            x,
            content_bottom,
            w,
            footer_h + 4,
            s.theme.palette().bg,
        );
        fill(mdc, x, content_bottom, w, 1, s.theme.palette().clr_div);

        if s.ai_pending {
            SelectObject(mdc, s.font_b);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let mut hint_w: Vec<u16> = "Esc: cancel".encode_utf16().collect();
            let mut hint_rc = RECT {
                left: x + pad,
                top: content_bottom + 2,
                right: x + w - pad,
                bottom: content_bottom + footer_h,
            };
            let _ = DrawTextW(
                mdc,
                &mut hint_w,
                &mut hint_rc,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        } else {
            let input_y = content_bottom + 8;
            fill_rounded(
                mdc,
                x + pad,
                input_y,
                w - pad * 2,
                34,
                10,
                COLORREF(0x00_2B_29_28),
            );
            SelectObject(mdc, s.font_c);
            let input_text = if s.chat_input.trim().is_empty() {
                SetTextColor(mdc, s.theme.palette().clr_ph);
                "Message this chat...".to_string()
            } else {
                SetTextColor(mdc, s.theme.palette().clr_white);
                s.chat_input.clone()
            };
            let mut input_w: Vec<u16> = input_text.encode_utf16().collect();
            let mut input_rc = RECT {
                left: x + pad + 12,
                top: input_y,
                right: x + w - pad - 118,
                bottom: input_y + 34,
            };
            if s.text_selected && s.chat_input_active && !s.chat_input.is_empty() {
                let mut selected_w = SIZE::default();
                let selected_text: Vec<u16> = s.chat_input.encode_utf16().collect();
                let _ = GetTextExtentPoint32W(mdc, &selected_text, &mut selected_w);
                fill(
                    mdc,
                    input_rc.left,
                    input_rc.top + 7,
                    selected_w.cx.min(input_rc.right - input_rc.left),
                    20,
                    s.theme.palette().clr_accent,
                );
            }
            let _ = DrawTextW(
                mdc,
                &mut input_w,
                &mut input_rc,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );

            if s.cursor_visible && s.chat_input_active {
                let cur = floor_char_boundary(&s.chat_input, s.chat_cursor_pos);
                let before = &s.chat_input[..cur];
                let dw_before: Vec<u16> = before.encode_utf16().collect();
                let mut size = SIZE::default();
                if !dw_before.is_empty() {
                    let _ = GetTextExtentPoint32W(mdc, &dw_before, &mut size);
                }
                let cursor_x = input_rc.left + size.cx;
                let mut dummy_size = SIZE::default();
                let _ = GetTextExtentPoint32W(mdc, &['A' as u16], &mut dummy_size);
                let text_h = dummy_size.cy;
                let cursor_top = input_rc.top + (input_rc.bottom - input_rc.top - text_h) / 2;
                fill(
                    mdc,
                    cursor_x,
                    cursor_top,
                    2,
                    text_h,
                    s.theme.palette().clr_white,
                );
            }

            SelectObject(mdc, s.font_b);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let mut hint_w: Vec<u16> = "Enter send".encode_utf16().collect();
            let mut hint_rc = RECT {
                left: x + w - pad - 104,
                top: input_y,
                right: x + w - pad - 12,
                bottom: input_y + 34,
            };
            let _ = DrawTextW(
                mdc,
                &mut hint_w,
                &mut hint_rc,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }

        SelectObject(mdc, s.font_q);
    }

    // ── Results ───────────────────────────────────────────────────────────
    let is_special_mode =
        s.ai_pending || s.ai_answer.is_some() || s.note_editing || s.chat_input_active;
    let n = if is_special_mode {
        0
    } else {
        let vr = s.visible_results();
        (s.results.len().saturating_sub(s.scroll_offset)).min(vr)
    };
    if !is_special_mode {
        let list_w = if s.submenu_active { w - 240 } else { w };

        // Draw top separator
        fill(mdc, x, y + SEARCH_H, list_w, 1, s.theme.palette().clr_div);

        let mut list_y = y + SEARCH_H + 1;

        if s.query.is_empty() {
            // Homepage empty state layout
            // Draw "Quick Search" and "8 sources" header
            SelectObject(mdc, s.font_c);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let mut qs_w: Vec<u16> = "Quick Search".encode_utf16().collect();
            let mut qs_rc = RECT {
                left: x + PAD_L,
                top: list_y + 10,
                right: x + w / 2,
                bottom: list_y + 26,
            };
            let _ = DrawTextW(
                mdc,
                &mut qs_w,
                &mut qs_rc,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            let mut src_w: Vec<u16> = "8 sources".encode_utf16().collect();
            let mut src_rc = RECT {
                left: x + w / 2,
                top: list_y + 10,
                right: x + w - PAD_L,
                bottom: list_y + 26,
            };
            let _ = DrawTextW(
                mdc,
                &mut src_w,
                &mut src_rc,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            list_y += LABEL_HEADER_H;
        } else if !s.has_prefix() {
            // Search state layout: Filter Row (hidden when inside a scope like clip:, agents:)
            let filters = [
                ("All", FilterType::All),
                ("Files", FilterType::Files),
                ("Folders", FilterType::Folders),
                ("Images", FilterType::Images),
                ("Code", FilterType::Code),
                ("Settings", FilterType::Settings),
                ("Commands", FilterType::Commands),
            ];

            let active_filter = if s.has_prefix() {
                filter_type_from_prefix(&s.query)
            } else {
                s.active_filter
            };

            let mut fx = x + PAD_L - s.filter_scroll_x;
            for &(label, ftype) in filters.iter() {
                let count = s.filter_counts[filter_index(ftype)];
                let full_label = format!("{} {}", label, count);
                let mut lw: Vec<u16> = full_label.encode_utf16().collect();

                SelectObject(mdc, s.font_c);
                let mut sz_lbl = SIZE::default();
                let _ = GetTextExtentPoint32W(mdc, &lw, &mut sz_lbl);

                let fw = sz_lbl.cx + 16;

                if ftype == active_filter {
                    fill_rounded(
                        mdc,
                        fx,
                        list_y + 8,
                        fw,
                        32,
                        16,
                        s.theme.palette().clr_accent,
                    );
                    fill_rounded(
                        mdc,
                        fx + 1,
                        list_y + 9,
                        fw - 2,
                        30,
                        15,
                        s.theme.palette().bg,
                    );
                } else if Some(ftype) == s.hovered_filter {
                    fill_rounded(mdc, fx, list_y + 8, fw, 32, 16, s.theme.palette().bg_hover);
                } else {
                    fill_rounded(mdc, fx, list_y + 8, fw, 32, 16, s.theme.palette().bg);
                }

                SetTextColor(
                    mdc,
                    if ftype == active_filter {
                        s.theme.palette().clr_white
                    } else {
                        s.theme.palette().clr_gray
                    },
                );
                let mut l_rc = RECT {
                    left: fx,
                    top: list_y + 8,
                    right: fx + fw,
                    bottom: list_y + 40,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut lw,
                    &mut l_rc,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );

                fx += fw + 8;
            }

            list_y += 48;

            if !s.has_prefix() {
                // Draw "Results" label and clickable "Best matches first" / "A–Z" sort toggle
                SelectObject(mdc, s.font_c);
                SetTextColor(mdc, s.theme.palette().clr_gray);
                let mut res_w: Vec<u16> = "Results".encode_utf16().collect();
                let mut res_rc = RECT {
                    left: x + PAD_L,
                    top: list_y + 8,
                    right: x + w / 2,
                    bottom: list_y + 24,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut res_w,
                    &mut res_rc,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );

                let sort_label = if s.sort_asc {
                    "A\u{2013}Z"
                } else {
                    "Best matches first"
                };
                let mut bm_text: Vec<u16> = sort_label.encode_utf16().collect();
                let mut sz_bm = SIZE::default();
                let _ = GetTextExtentPoint32W(mdc, &bm_text, &mut sz_bm);
                let bm_x = x + list_w - PAD_L - sz_bm.cx - 16;
                let mut bm_rc = RECT {
                    left: bm_x,
                    top: list_y + 8,
                    right: bm_x + sz_bm.cx + 2,
                    bottom: list_y + 24,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut bm_text,
                    &mut bm_rc,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );

                // Draw directional chevron
                let chev_x = bm_x + sz_bm.cx + 6;
                let chev_y = list_y + 8 + sz_bm.cy / 2;
                let pen = CreatePen(PS_SOLID, 1, s.theme.palette().clr_gray);
                let old_pen = SelectObject(mdc, pen);
                if s.sort_asc {
                    let _ = MoveToEx(mdc, chev_x, chev_y + 1, None);
                    let _ = LineTo(mdc, chev_x + 3, chev_y - 2);
                    let _ = LineTo(mdc, chev_x + 6, chev_y + 1);
                } else {
                    let _ = MoveToEx(mdc, chev_x, chev_y - 2, None);
                    let _ = LineTo(mdc, chev_x + 3, chev_y + 1);
                    let _ = LineTo(mdc, chev_x + 6, chev_y - 2);
                }
                SelectObject(mdc, old_pen);
                let _ = DeleteObject(pen);

                list_y += 32;
            }
        } else {
            SelectObject(mdc, s.font_c);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let section = s
                .results
                .first()
                .map(source_section_label_res)
                .unwrap_or("Results");
            let mut label: Vec<u16> = section.encode_utf16().collect();
            let mut label_rect = RECT {
                left: x + PAD_L,
                top: list_y + 10,
                right: x + list_w / 2,
                bottom: list_y + 26,
            };
            let _ = DrawTextW(
                mdc,
                &mut label,
                &mut label_rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            let count_text = if s.results.len() == 1 {
                "1 result".to_string()
            } else {
                format!("{} results", s.results.len())
            };
            let mut count: Vec<u16> = count_text.encode_utf16().collect();
            let mut count_rect = RECT {
                left: x + list_w / 2,
                top: list_y + 10,
                right: x + list_w - PAD_L,
                bottom: list_y + 26,
            };
            let _ = DrawTextW(
                mdc,
                &mut count,
                &mut count_rect,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            list_y += LABEL_HEADER_H;
        }

        for i in 0..n {
            let res_idx = s.scroll_offset + i;
            let res = &s.results[res_idx];

            if s.query.is_empty() {
                // Homepage (Image 3) flat list items
                let ry = list_y + i as i32 * s.item_h();
                let is_selected = res_idx == s.selected;
                let is_hovered = Some(res_idx) == s.hovered_item;
                if is_selected {
                    let border_y = ry + 2;
                    let border_h = s.item_h() - 4;
                    fill_rounded(
                        mdc,
                        x + 8,
                        border_y,
                        list_w - 16,
                        border_h,
                        4,
                        palette.bg_sel,
                    );
                } else if is_hovered {
                    let border_y = ry + 2;
                    let border_h = s.item_h() - 4;
                    fill_rounded(
                        mdc,
                        x + 8,
                        border_y,
                        list_w - 16,
                        border_h,
                        4,
                        palette.bg_hover,
                    );
                }

                let icon_to_draw = match res.entry.source.as_str() {
                    "HOMEPAGE_PHONE_CONNECT" => s.icon_phone_connect,
                    "HOMEPAGE_BROWSER" => {
                        if res.entry.id == "home_1" {
                            s.icon_bookmark
                        } else {
                            s.icon_web
                        }
                    }
                    "HOMEPAGE_GIT" => s.icon_commit,
                    "HOMEPAGE_CLIPBOARD" => s.icon_clipboard,
                    "HOMEPAGE_LOCAL" => s.icon_folder,
                    "HOMEPAGE_CODE" | "HOMEPAGE_OCR" => s.icon_file,
                    "HOMEPAGE_AI" | "AI" => s.icon_agent,
                    "HOMEPAGE_AI_CHAT" | "AI_CHAT" => s.icon_agent_chat,
                    _ => s.icon_app,
                };

                draw_result_icon(mdc, s, icon_to_draw, x + PAD_L, ry);

                let tx = x + PAD_L + RESULT_ICON_SIZE + RESULT_TEXT_GAP;
                let text_top = centered_in_result_row(
                    ry,
                    RESULT_TEXT_BLOCK_H,
                    s.app_settings.item_height as i32,
                );
                SelectObject(mdc, s.font_n);
                SetTextColor(mdc, palette.clr_white);
                let display_name = res.entry.control_name.clone();
                let mut name: Vec<u16> = display_name.encode_utf16().collect();
                let mut r = RECT {
                    left: tx,
                    top: text_top,
                    right: x + list_w - PAD_L - 80,
                    bottom: text_top + 22,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut name,
                    &mut r,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );

                SelectObject(mdc, s.font_c);
                SetTextColor(
                    mdc,
                    if is_selected {
                        palette.clr_gray_sel
                    } else {
                        palette.clr_gray
                    },
                );
                let mut crumb: Vec<u16> = res.entry.breadcrumb_path.encode_utf16().collect();
                let mut r2 = RECT {
                    left: tx,
                    top: text_top + 22,
                    right: x + list_w - PAD_L - 80,
                    bottom: text_top + RESULT_TEXT_BLOCK_H,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut crumb,
                    &mut r2,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );

                let cat_str = match res.entry.source.as_str() {
                    "HOMEPAGE_BROWSER" => "Browser",
                    "HOMEPAGE_GIT" => "Git",
                    "HOMEPAGE_CLIPBOARD" => "Clipboard",
                    "HOMEPAGE_LOCAL" => "Local",
                    "HOMEPAGE_CODE" => "Code",
                    "HOMEPAGE_OCR" => "OCR",
                    "HOMEPAGE_AI" => "AI",
                    "HOMEPAGE_AI_CHAT" => "AI",
                    _ => "",
                };
                // DrawTextW must never be called with an empty slice: an empty &mut [u16]
                // yields a dangling pointer that faults inside user32.dll (this was the
                // startup crash — HOMEPAGE_AI_CHAT fell through to "" above).
                if !cat_str.is_empty() {
                    let mut cat: Vec<u16> = cat_str.encode_utf16().collect();
                    let mut rc_cat = RECT {
                        left: x + list_w / 2,
                        top: ry,
                        right: x + list_w - PAD_L,
                        bottom: ry + s.item_h(),
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut cat,
                        &mut rc_cat,
                        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
            } else if s.has_prefix() {
                // ── Flat results layout with headers ───────────────────
                let starts_section = res_idx == 0
                    || source_section_label_res(&s.results[res_idx - 1])
                        != source_section_label_res(res);
                if false && starts_section {
                    SelectObject(mdc, s.font_b);
                    SetTextColor(mdc, palette.clr_gray);
                    let section = source_section_label_res(res);
                    let section_total = s
                        .results
                        .iter()
                        .filter(|candidate| source_section_label_res(candidate) == section)
                        .count();
                    let mut label: Vec<u16> = section.encode_utf16().collect();
                    let mut label_rect = RECT {
                        left: x + PAD_L,
                        top: list_y + 4,
                        right: x + list_w / 2,
                        bottom: list_y + 20,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut label,
                        &mut label_rect,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );

                    let count_text = if section_total == 1 {
                        "1 result".to_string()
                    } else {
                        format!("{} results", section_total)
                    };
                    let mut count: Vec<u16> = count_text.encode_utf16().collect();
                    let mut count_rect = RECT {
                        left: x + list_w / 2,
                        top: list_y + 4,
                        right: x + list_w - PAD_L,
                        bottom: list_y + 20,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut count,
                        &mut count_rect,
                        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );

                    // Separator line
                    let sep_y = list_y + 23;
                    fill(
                        mdc,
                        x + PAD_L,
                        sep_y,
                        list_w - 2 * PAD_L,
                        1,
                        palette.clr_div,
                    );

                    list_y += 24;
                }

                let ry = list_y;
                let is_selected = res_idx == s.selected;
                let is_hovered = Some(res_idx) == s.hovered_item;
                let is_checked = selected_clip_ids_contain(&s.selected_clip_ids, &res.entry.id);

                if is_selected {
                    fill_rounded(
                        mdc,
                        x + 8,
                        ry + 2,
                        list_w - 16,
                        s.item_h() - 4,
                        4,
                        palette.bg_sel,
                    );
                } else if is_hovered {
                    fill_rounded(
                        mdc,
                        x + 8,
                        ry + 2,
                        list_w - 16,
                        s.item_h() - 4,
                        4,
                        palette.bg_hover,
                    );
                } else if is_checked {
                    fill_rounded(
                        mdc,
                        x + 8,
                        ry + 2,
                        list_w - 16,
                        s.item_h() - 4,
                        4,
                        palette.bg_hover,
                    );
                }

                let icon_y =
                    centered_in_result_row(ry, RESULT_ICON_SIZE, s.app_settings.item_height as i32);
                let mut drew_thumbnail = false;
                if let Some(path) = image_path_for_result(res) {
                    let mut cache = s.clipboard_thumbnails.borrow_mut();
                    if let Some(&hbitmap) = cache.get(path) {
                        draw_cached_bmp(
                            mdc,
                            x + PAD_L,
                            icon_y,
                            RESULT_ICON_SIZE,
                            RESULT_ICON_SIZE,
                            hbitmap,
                        );
                        drew_thumbnail = true;
                    } else if let Some(hbitmap) = load_shell_thumbnail(path, 256) {
                        draw_cached_bmp(
                            mdc,
                            x + PAD_L,
                            icon_y,
                            RESULT_ICON_SIZE,
                            RESULT_ICON_SIZE,
                            hbitmap,
                        );
                        cache.insert(path.to_string(), hbitmap);
                        prune_thumbnail_cache(&mut cache, path);
                        drew_thumbnail = true;
                    }
                }
                if !drew_thumbnail {
                    let cached_icon = s
                        .app_icons
                        .get(&res.entry.launch_command)
                        .copied()
                        .filter(|h| !h.0.is_null());
                    let icon_to_draw =
                        if search::is_native_settings_command(&res.entry.launch_command)
                            || res.entry.source == "CONTROL"
                            || res.entry.source == "SETTINGS"
                        {
                            s.icon_settings
                        } else if let Some(hicon) = cached_icon {
                            hicon
                        } else if res.entry.source == "WINDOW" {
                            s.app_icons
                                .get(&res.entry.launch_command)
                                .copied()
                                .filter(|h| !h.0.is_null())
                                .unwrap_or(s.icon_app)
                        } else if res.entry.source == "app"
                            || is_file_result_source(&res.entry.source)
                            || (res.entry.source == "ACTION"
                                && res.entry.launch_command.starts_with("kill:"))
                        {
                            s.app_icons
                                .get(&res.entry.launch_command)
                                .copied()
                                .filter(|h| !h.0.is_null())
                                .unwrap_or_else(|| {
                                    if res.entry.source == "app" {
                                        s.icon_app
                                    } else if res.entry.source == "FOLDER" {
                                        s.icon_folder
                                    } else {
                                        s.icon_file
                                    }
                                })
                        } else if res.entry.launch_command.starts_with("ms-settings:") {
                            s.icon_settings
                        } else if res
                            .entry
                            .launch_command
                            .starts_with("https://chatgpt.com/")
                        {
                            s.icon_chatgpt
                        } else if res
                            .entry
                            .launch_command
                            .starts_with("https://claude.ai/")
                        {
                            s.icon_claude
                        } else if res
                            .entry
                            .launch_command
                            .starts_with("https://gemini.google.com/")
                        {
                            s.icon_gemini
                        } else if res
                            .entry
                            .launch_command
                            .starts_with("https://grok.com/")
                        {
                            s.icon_grok
                        } else if res
                            .entry
                            .launch_command
                            .starts_with("https://eyux.com.et")
                        {
                            s.icon_eyux
                        } else if res.entry.source == "CALC" {
                            s.icon_calculator
                        } else if res.entry.launch_command == "action:circle_to_search" {
                            s.icon_lens
                        } else if res.entry.launch_command == "action:color_picker" {
                            s.icon_color_picker
                        } else if res.entry.source == "web"
                            || res.entry.source == "HISTORY"
                            || res.entry.source == "QUICKLINK"
                            || res.entry.launch_command.starts_with("https://")
                        {
                            s.icon_web
                        } else if res.entry.source == "BOOKMARK" {
                            s.icon_bookmark
                        } else if res.entry.source == "FOLDER" {
                            s.icon_folder
                        } else if res.entry.source == "COMMIT" {
                            s.icon_commit
                        } else if res.entry.source == "TODO"
                            || res.entry.source == "SNIPPET"
                            || res
                                .entry
                                .launch_command
                                .starts_with("action:create_snippet")
                        {
                            s.icon_file
                        } else if res.entry.source == "CLIPBOARD"
                            || res.entry.launch_command.starts_with("action:ask_clipboard")
                        {
                            s.icon_clipboard
                        } else if res.entry.launch_command.starts_with("openagent:") {
                            // Agents in the `agents:` view — same icon as homepage Agents.
                            s.icon_agent
                        } else if res.entry.source == "AI_CHAT" {
                            // Agent runs / chats in the `agentchats:` view — same icon as
                            // homepage Agent History.
                            s.icon_agent_chat
                        } else if res.entry.source == "AI"
                            || res.entry.source == "MEMORY"
                            || res
                                .entry
                                .launch_command
                                .starts_with("action:reload_script_commands")
                        {
                            s.icon_app
                        } else if res.entry.launch_command.starts_with("start_focus_session:")
                            || res
                                .entry
                                .launch_command
                                .starts_with("action:toggle_focus_session")
                            || res
                                .entry
                                .launch_command
                                .starts_with("action:create_focus_category")
                        {
                            s.icon_app
                        } else if res
                            .entry
                            .launch_command
                            .starts_with("action:create_quicklink")
                        {
                            s.icon_bookmark
                        } else {
                            s.icon_app
                        };

                    draw_result_icon(mdc, s, icon_to_draw, x + PAD_L, ry);
                }

                let tx = x + PAD_L + RESULT_ICON_SIZE + RESULT_TEXT_GAP;
                let text_top = centered_in_result_row(
                    ry,
                    RESULT_TEXT_BLOCK_H,
                    s.app_settings.item_height as i32,
                );
                SelectObject(mdc, s.font_n);
                SetTextColor(mdc, palette.clr_white);
                let display_name = if selected_clip_ids_contain(&s.selected_clip_ids, &res.entry.id)
                {
                    format!("[✓] {}", res.entry.control_name)
                } else {
                    res.entry.control_name.clone()
                };
                let mut name: Vec<u16> = display_name.encode_utf16().collect();

                let mut sz_name = SIZE::default();
                unsafe {
                    SelectObject(mdc, s.font_n);
                    let _ = GetTextExtentPoint32W(mdc, &name, &mut sz_name);
                }

                let badge_left = x + list_w - PAD_L - BADGE_W;
                let mut r = RECT {
                    left: tx,
                    top: text_top,
                    right: (tx + sz_name.cx + 2).min(badge_left - 14),
                    bottom: text_top + 22,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut name,
                    &mut r,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );

                let default_gray = if is_selected {
                    palette.clr_gray_sel
                } else {
                    palette.clr_gray
                };

                let reason = s
                    .result_reasons
                    .get(&res.entry.launch_command)
                    .filter(|r| !r.is_empty());
                let reason_slot = if reason.is_some() { 96 } else { 0 };

                if !res.entry.description.is_empty() {
                    // Draw breadcrumb path on Line 1 next to control_name
                    let separator = "  —  ".to_string();
                    let full_path = separator + &res.entry.breadcrumb_path;
                    let mut path_w: Vec<u16> = full_path.encode_utf16().collect();
                    SelectObject(mdc, s.font_c);
                    SetTextColor(mdc, default_gray);

                    let mut r_path = RECT {
                        left: tx + sz_name.cx + 4,
                        top: text_top + 2,
                        right: badge_left - 14,
                        bottom: text_top + 22,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut path_w,
                        &mut r_path,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );

                    // Draw description on Line 2
                    draw_highlighted_text(
                        mdc,
                        &res.entry.description,
                        clean_query_prefix(&s.query),
                        s.font_c,
                        default_gray,
                        palette.clr_accent,
                        tx,
                        text_top + 22,
                    );
                } else {
                    SelectObject(mdc, s.font_c);
                    SetTextColor(mdc, default_gray);
                    let mut crumb: Vec<u16> = res.entry.breadcrumb_path.encode_utf16().collect();
                    let mut r2 = RECT {
                        left: tx,
                        top: text_top + 22,
                        right: badge_left - 14 - reason_slot,
                        bottom: text_top + RESULT_TEXT_BLOCK_H,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut crumb,
                        &mut r2,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                }

                if let Some(reason) = reason {
                    SetTextColor(mdc, palette.clr_ph);
                    let mut rtxt: Vec<u16> = reason.encode_utf16().collect();
                    let mut rr = RECT {
                        left: badge_left - 14 - reason_slot,
                        top: text_top + 22,
                        right: badge_left - 14,
                        bottom: text_top + RESULT_TEXT_BLOCK_H,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut rtxt,
                        &mut rr,
                        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }

                let badge_source = if res.entry.id.starts_with("clip.pinned.") {
                    "pinned_clip"
                } else {
                    &res.entry.source
                };
                badge(
                    mdc,
                    s,
                    badge_source,
                    badge_left,
                    ry + (s.item_h() - BADGE_H) / 2,
                );
                list_y += s.item_h();
            } else {
                // ── Search Page (Image 4) flat list items ──────────────────────────
                let ry = list_y + i as i32 * s.item_h();
                let is_selected = res_idx == s.selected;
                let is_hovered = Some(res_idx) == s.hovered_item;
                if is_selected {
                    let border_y = ry + 2;
                    let border_h = s.item_h() - 4;
                    fill_rounded(
                        mdc,
                        x + 8,
                        border_y,
                        list_w - 16,
                        border_h,
                        4,
                        palette.bg_sel,
                    );
                } else if is_hovered {
                    let border_y = ry + 2;
                    let border_h = s.item_h() - 4;
                    fill_rounded(
                        mdc,
                        x + 8,
                        border_y,
                        list_w - 16,
                        border_h,
                        4,
                        palette.bg_hover,
                    );
                }

                let mut drew_thumbnail = false;
                if let Some(path) = image_path_for_result(res) {
                    let mut cache = s.clipboard_thumbnails.borrow_mut();
                    let hbitmap = cache.get(path).copied().or_else(|| {
                        load_shell_thumbnail(path, 256).inspect(|h| {
                            cache.insert(path.to_string(), *h);
                            prune_thumbnail_cache(&mut cache, path);
                        })
                    });
                    if let Some(hbitmap) = hbitmap {
                        let icon_y = centered_in_result_row(
                            ry,
                            RESULT_ICON_SIZE,
                            s.app_settings.item_height as i32,
                        );
                        draw_cached_bmp(
                            mdc,
                            x + PAD_L,
                            icon_y,
                            RESULT_ICON_SIZE,
                            RESULT_ICON_SIZE,
                            hbitmap,
                        );
                        drew_thumbnail = true;
                    }
                }

                let icon_to_draw = if is_windows_settings_command(&res.entry.launch_command) {
                    s.icon_settings
                } else {
                    s.app_icons
                        .get(&res.entry.launch_command)
                        .copied()
                        .filter(|icon| !icon.0.is_null())
                        .unwrap_or_else(|| match res.entry.source.as_str() {
                            "app" => s.icon_app,
                            "FOLDER" => s.icon_folder,
                            "FILE" | "FILE_CONTENT" | "RECENT" | "CODE" | "CODE_CONTENT"
                            | "OCR" => s.icon_file,
                            "ACTION" if res.entry.launch_command == "action:color_picker" => {
                                s.icon_color_picker
                            }
                            "ACTION" if res.entry.launch_command == "action:circle_to_search" => {
                                s.icon_lens
                            }
                            "ACTION" | "SYSTEM" | "WINDOW" => s.icon_app,
                            "BOOKMARK" | "QUICKLINK" => {
                                let desc = res.entry.description.to_lowercase();
                                if desc.contains("chrome") && !s.icon_chrome.0.is_null() {
                                    s.icon_chrome
                                } else if desc.contains("firefox") && !s.icon_firefox.0.is_null() {
                                    s.icon_firefox
                                } else if desc.contains("edge") && !s.icon_edge.0.is_null() {
                                    s.icon_edge
                                } else if desc.contains("brave") && !s.icon_brave.0.is_null() {
                                    s.icon_brave
                                } else {
                                    s.icon_bookmark
                                }
                            }
                            "CLIPBOARD" => s.icon_clipboard,
                            "COMMIT" => s.icon_commit,
                            "HISTORY" | "web" => {
                                let desc = res.entry.description.to_lowercase();
                                if desc.contains("chrome") && !s.icon_chrome.0.is_null() {
                                    s.icon_chrome
                                } else if desc.contains("firefox") && !s.icon_firefox.0.is_null() {
                                    s.icon_firefox
                                } else if desc.contains("edge") && !s.icon_edge.0.is_null() {
                                    s.icon_edge
                                } else if desc.contains("brave") && !s.icon_brave.0.is_null() {
                                    s.icon_brave
                                } else {
                                    s.icon_web
                                }
                            }
                            "MEMORY" | "AI" => s.icon_app,
                            "PDF" => s.icon_file,
                            "Settings" | "SETTINGS" | "CONTROL" => s.icon_settings,
                            "CALC" => s.icon_calculator,
                            "SNIPPET" | "TODO" => s.icon_file,
                            _ => s.icon_app,
                        })
                };

                if !drew_thumbnail && !icon_to_draw.0.is_null() {
                    let icon_y = centered_in_result_row(
                        ry,
                        RESULT_ICON_SIZE,
                        s.app_settings.item_height as i32,
                    );
                    let _ = unsafe {
                        DrawIconEx(
                            mdc,
                            x + PAD_L,
                            icon_y,
                            icon_to_draw,
                            RESULT_ICON_SIZE,
                            RESULT_ICON_SIZE,
                            0,
                            HBRUSH(null_mut()),
                            DI_NORMAL,
                        )
                    };
                }

                let tx = x + PAD_L + RESULT_ICON_SIZE + RESULT_TEXT_GAP;
                let text_top = centered_in_result_row(
                    ry,
                    RESULT_TEXT_BLOCK_H,
                    s.app_settings.item_height as i32,
                );

                let badge_limit = x + list_w - PAD_L - 100;
                let default_gray = if is_selected {
                    palette.clr_gray_sel
                } else {
                    palette.clr_gray
                };

                if is_content_match_source(&res.entry.source) && !res.entry.description.is_empty() {
                    let content_top = centered_in_result_row(ry, 54, s.item_h());

                    SelectObject(mdc, s.font_n);
                    SetTextColor(mdc, palette.clr_white);
                    let mut title: Vec<u16> = res.entry.control_name.encode_utf16().collect();
                    let mut title_rect = RECT {
                        left: tx,
                        top: content_top,
                        right: badge_limit,
                        bottom: content_top + 20,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut title,
                        &mut title_rect,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );

                    SelectObject(mdc, s.font_c);
                    SetTextColor(mdc, palette.clr_white);
                    let mut snippet: Vec<u16> = res.entry.description.encode_utf16().collect();
                    let mut snippet_rect = RECT {
                        left: tx,
                        top: content_top + 20,
                        right: badge_limit,
                        bottom: content_top + 38,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut snippet,
                        &mut snippet_rect,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );

                    SelectObject(mdc, s.font_c);
                    SetTextColor(mdc, default_gray);
                    let meta = format!("{}  >  {}", res.entry.source, res.entry.breadcrumb_path);
                    let mut meta_w: Vec<u16> = meta.encode_utf16().collect();
                    let mut meta_rect = RECT {
                        left: tx,
                        top: content_top + 38,
                        right: badge_limit,
                        bottom: content_top + 54,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut meta_w,
                        &mut meta_rect,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                } else {
                    SelectObject(mdc, s.font_n);
                    SetTextColor(mdc, palette.clr_white);
                    let display_name = res.entry.control_name.clone();
                    let mut name: Vec<u16> = display_name.encode_utf16().collect();

                    let mut sz_name = SIZE::default();
                    unsafe {
                        SelectObject(mdc, s.font_n);
                        let _ = GetTextExtentPoint32W(mdc, &name, &mut sz_name);
                    }

                    let mut r = RECT {
                        left: tx,
                        top: text_top,
                        right: (tx + sz_name.cx + 2).min(badge_limit),
                        bottom: text_top + 22,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut name,
                        &mut r,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );

                    if !res.entry.description.is_empty() {
                        // Draw breadcrumb path on Line 1 next to control_name
                        let separator = "  —  ".to_string();
                        let full_path = separator + &res.entry.breadcrumb_path;
                        let mut path_w: Vec<u16> = full_path.encode_utf16().collect();
                        SelectObject(mdc, s.font_c);
                        SetTextColor(mdc, default_gray);

                        let mut r_path = RECT {
                            left: tx + sz_name.cx + 4,
                            top: text_top + 2,
                            right: badge_limit,
                            bottom: text_top + 22,
                        };
                        let _ = DrawTextW(
                            mdc,
                            &mut path_w,
                            &mut r_path,
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                        );

                        // Draw description on Line 2
                        draw_highlighted_text(
                            mdc,
                            &res.entry.description,
                            clean_query_prefix(&s.query),
                            s.font_c,
                            default_gray,
                            palette.clr_accent,
                            tx,
                            text_top + 22,
                        );
                    } else {
                        SelectObject(mdc, s.font_c);
                        SetTextColor(mdc, default_gray);
                        let mut crumb: Vec<u16> =
                            res.entry.breadcrumb_path.encode_utf16().collect();
                        let mut r2 = RECT {
                            left: tx,
                            top: text_top + 22,
                            right: badge_limit,
                            bottom: text_top + RESULT_TEXT_BLOCK_H,
                        };
                        let _ = DrawTextW(
                            mdc,
                            &mut crumb,
                            &mut r2,
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                        );
                    }
                }

                let badge_source = &res.entry.source;
                let label = match badge_source.as_str() {
                    "CODE" => "Code",
                    "CODE_CONTENT" => "Code Content",
                    "PDF" => "PDF Content",
                    "OCR" => "Image OCR",
                    "FILE" => "File",
                    "FILE_CONTENT" => "Content",
                    "Settings" => "Settings",
                    "SYSTEM" => "Command",
                    _ => badge_source,
                };

                if !label.is_empty() && badge_source != "WINDOW" {
                    let t: Vec<u16> = label.encode_utf16().collect();
                    let mut sz = SIZE::default();
                    SelectObject(mdc, s.font_b);
                    let _ = GetTextExtentPoint32W(mdc, &t, &mut sz);
                    let badge_w = (sz.cx + 16).max(40);
                    let badge_x = x + list_w - PAD_L - badge_w;
                    badge_custom(
                        mdc,
                        s,
                        label,
                        badge_x,
                        ry + (s.item_h() - BADGE_H) / 2,
                        badge_w,
                    );
                }
            }
        }

        // Draw scrollbar if there are more results than visible
        let total_results = s.results.len();
        let vr = s.visible_results();
        if total_results > vr {
            let track_top = list_y + 8;
            let track_bottom = list_y + n as i32 * s.item_h() - 8;
            let track_h = track_bottom - track_top;

            let thumb_h = ((vr as f32 / total_results as f32) * track_h as f32) as i32;
            let thumb_h = thumb_h.max(24);

            let max_offset = total_results - vr;
            let thumb_y = track_top
                + (s.scroll_offset as f32 / max_offset as f32 * (track_h - thumb_h) as f32) as i32;

            let sb_x = x + list_w - 10;
            let sb_w = 4;
            fill(mdc, sb_x, track_top, sb_w, track_h, palette.scrollbar_track);
            fill(mdc, sb_x, thumb_y, sb_w, thumb_h, palette.scrollbar_thumb);
        }

        if s.image_preview_active {
            // Draw nothing in main window, handled by popup preview
        } else if s.submenu_active {
            fill(
                mdc,
                x + list_w,
                y + SEARCH_H,
                1,
                h - SEARCH_H,
                s.theme.palette().clr_div,
            );
            fill(
                mdc,
                x + list_w + 1,
                y + SEARCH_H + 1,
                238,
                h - SEARCH_H - 1,
                palette.bg_footer,
            );
            let actions = ["Run as Administrator", "Open File Location", "Copy Path"];
            let action_h = 44;
            let start_y = y + SEARCH_H + 16;
            for idx in 0..3 {
                let ay = start_y + idx as i32 * (action_h + 8);
                if s.submenu_selected == idx {
                    fill_rounded(
                        mdc,
                        x + list_w + 8,
                        ay,
                        224,
                        action_h,
                        8,
                        s.theme.palette().bg_sel,
                    );
                }

                SelectObject(mdc, s.font_n);
                SetTextColor(
                    mdc,
                    if s.submenu_selected == idx {
                        s.theme.palette().clr_white
                    } else {
                        s.theme.palette().clr_gray
                    },
                );
                let mut text_wide: Vec<u16> = actions[idx].encode_utf16().collect();
                let mut r_action = RECT {
                    left: x + list_w + 16,
                    top: ay,
                    right: x + w - 16,
                    bottom: ay + action_h,
                };
                let _ = DrawTextW(
                    mdc,
                    &mut text_wide,
                    &mut r_action,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );

                if s.submenu_selected == idx {
                    SelectObject(mdc, s.font_c);
                    SetTextColor(mdc, s.theme.palette().clr_gray);
                    let mut hint_wide: Vec<u16> = "Enter".encode_utf16().collect();
                    let mut r_hint = RECT {
                        left: x + w - 60,
                        top: ay,
                        right: x + w - 16,
                        bottom: ay + action_h,
                    };
                    let _ = DrawTextW(
                        mdc,
                        &mut hint_wide,
                        &mut r_hint,
                        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
            }
        }
    }

    // Draw footer instructions if showing clipboard
    if s.query.starts_with("clip:") || s.query.starts_with("clipboard:") {
        let footer_y = y + h - 24;
        fill(mdc, x, footer_y, w, 24, palette.bg_footer);
        fill(mdc, x, footer_y, w, 1, s.theme.palette().clr_div);

        if s.delete_confirm {
            badge(mdc, s, "confirm", x + PAD_L, footer_y + 2);
            SelectObject(mdc, s.font_c);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let inst_text = format!(
                " Press Delete again to delete {} selected items, Escape to cancel",
                s.selected_clip_ids.len()
            );
            let mut inst_wide: Vec<u16> = inst_text.encode_utf16().collect();
            let mut r_inst = RECT {
                left: x + PAD_L + 68,
                top: footer_y,
                right: x + w - PAD_L,
                bottom: y + h,
            };
            let _ = DrawTextW(
                mdc,
                &mut inst_wide,
                &mut r_inst,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        } else {
            SelectObject(mdc, s.font_c);
            SetTextColor(mdc, s.theme.palette().clr_gray);
            let inst_text = if s.editing_item.is_some() {
                " 📝 Editing snippet: Press Enter to save to database & clipboard, Escape to cancel"
                    .to_string()
            } else {
                let sel_count = s.selected_clip_ids.len();
                if sel_count > 0 {
                    format!(" Tab: Deselect  |  Enter: Paste combined ({})  |  Delete: Bulk Delete  |  Ctrl+P: Pin/Unpin", sel_count)
                } else {
                    " Tab: Select  |  Enter: Copy & Paste  |  Ctrl+P: Pin/Unpin  |  Ctrl+E: Edit  |  Delete: Delete".to_string()
                }
            };
            let mut inst_wide: Vec<u16> = inst_text.encode_utf16().collect();
            let mut r_inst = RECT {
                left: x + PAD_L,
                top: footer_y,
                right: x + w - PAD_L,
                bottom: y + h,
            };
            let _ = DrawTextW(
                mdc,
                &mut inst_wide,
                &mut r_inst,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }

    // Draw footer instructions if showing snippet/quicklink creation form
    if s.form_state != FormState::None {
        let footer_y = y + h - 24;
        fill(mdc, x, footer_y, w, 24, palette.bg_footer);
        fill(mdc, x, footer_y, w, 1, s.theme.palette().clr_div);

        SelectObject(mdc, s.font_c);
        SetTextColor(mdc, s.theme.palette().clr_gray);
        let inst_text = " Enter: Next / Save  |  Escape: Cancel creation".to_string();
        let mut inst_wide: Vec<u16> = inst_text.encode_utf16().collect();
        let mut r_inst = RECT {
            left: x + PAD_L,
            top: footer_y,
            right: x + w - PAD_L,
            bottom: y + h,
        };
        let _ = DrawTextW(
            mdc,
            &mut inst_wide,
            &mut r_inst,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }

    if n > 0
        && s.form_state == FormState::None
        && !s.query.starts_with("clip:")
        && !s.query.starts_with("clipboard:")
        && s.shows_guidance_footer()
    {
        let footer_y = y + h - 28;
        fill(mdc, x, footer_y, w, 28, palette.bg_footer);
        fill(mdc, x, footer_y, w, 1, s.theme.palette().clr_div);
        let mut hint_x = x + PAD_L;
        hint_x = key_hint(mdc, s, hint_x, footer_y + 6, "↑|↓", "Navigate");
        hint_x = key_hint(mdc, s, hint_x + 12, footer_y + 6, "Enter", "Open");
        hint_x = key_hint(mdc, s, hint_x + 12, footer_y + 6, "Tab", "Preview");
        let _ = key_hint(mdc, s, hint_x + 12, footer_y + 6, "Esc", "Close");

        // Draw "● Index ready" on the far right
        let status_text = "Index ready";
        let mut st_wide: Vec<u16> = status_text.encode_utf16().collect();
        SelectObject(mdc, s.font_c);
        let mut sz_st = SIZE::default();
        let _ = GetTextExtentPoint32W(mdc, &st_wide, &mut sz_st);
        let st_x = x + w - PAD_L - sz_st.cx;
        let mut st_rc = RECT {
            left: st_x,
            top: footer_y + 6,
            right: x + w - PAD_L,
            bottom: footer_y + 22,
        };
        SetTextColor(mdc, s.theme.palette().clr_gray);
        let _ = DrawTextW(
            mdc,
            &mut st_wide,
            &mut st_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        // Draw green circle
        let dot_x = st_x - 14;
        let dot_y = footer_y + 10;
        let br = CreateSolidBrush(COLORREF(0x00_50_DF_50)); // nice green
        let old_brush = SelectObject(mdc, br);
        let pen = CreatePen(PS_NULL, 0, COLORREF(0));
        let old_pen = SelectObject(mdc, pen);
        let _ = Ellipse(mdc, dot_x, dot_y, dot_x + 8, dot_y + 8);
        SelectObject(mdc, old_brush);
        let _ = DeleteObject(br);
        SelectObject(mdc, old_pen);
        let _ = DeleteObject(pen);
    }

    // Restore clipping
    let _ = SelectClipRgn(mdc, HRGN(null_mut()));
    let _ = DeleteObject(clip_rgn);

    let bx = ps.rcPaint.left;
    let by = ps.rcPaint.top;
    let bw = (ps.rcPaint.right - ps.rcPaint.left).max(0);
    let bh = (ps.rcPaint.bottom - ps.rcPaint.top).max(0);
    let _ = BitBlt(hdc, bx, by, bw, bh, mdc, bx, by, SRCCOPY);
    // back-buffer is cached — don't delete mdc/bmp here

    if s.image_preview_active {
        if let Some(h) = s.hwnd_preview {
            let _ = unsafe { windows::Win32::Graphics::Gdi::InvalidateRect(h, None, FALSE) };
        }
    }

    let _ = EndPaint(hwnd, &ps);
}

fn default_homepage_results() -> Vec<crate::search::SearchResult> {
    default_homepage_results_for_settings()
}

fn default_homepage_results_for_settings() -> Vec<crate::search::SearchResult> {
    let items = [
        (
            "Connect Phone",
            "Sync > Phone Connection",
            "sync:phone",
            "HOMEPAGE_PHONE_CONNECT",
        ),
        (
            "Browser Bookmarks",
            "Browser > Bookmarks",
            "bookmarks:",
            "HOMEPAGE_BROWSER",
        ),
        (
            "Browser History",
            "Browser > History",
            "history:",
            "HOMEPAGE_BROWSER",
        ),
        ("Local Files", "Local > Files", "file:", "HOMEPAGE_LOCAL"),
        ("Agents", "AI > Agents", "agents:", "HOMEPAGE_AI"),
        (
            "Agent History",
            "AI > Agent History",
            "agentchats:",
            "HOMEPAGE_AI_CHAT",
        ),
    ];

    items
        .into_iter()
        .enumerate()
        .map(|(i, (name, path, cmd, src))| crate::search::SearchResult {
            score: 1.0 - (i as f32 * 0.01),
            entry: crate::search::CatalogEntry {
                id: format!("home_{}", i),
                control_name: name.to_string(),
                breadcrumb_path: path.to_string(),
                launch_command: cmd.to_string(),
                source: src.to_string(),
                description: "".to_string(),
                synonyms: "".to_string(),
            },
        })
        .collect()
}

fn clean_query_prefix(query: &str) -> &str {
    let prefixes = [
        "bookmarks:",
        "history:",
        "commits:",
        "todos:",
        "clip:",
        "clipboard:",
        "file:",
        "folder:",
        "code:",
        "img:",
        "image:",
        "screenshots:",
        "agents:",
        "agentchats:",
    ];
    let q_lower = query.to_lowercase();
    for p in &prefixes {
        if q_lower.starts_with(p) {
            return query[p.len()..].trim();
        }
    }
    query
}

fn highlighted_text_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let query_words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.to_string())
        .filter(|w| !w.is_empty())
        .collect();
    if query_words.is_empty() {
        return Vec::new();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut matches = Vec::new();

    for word in &query_words {
        let word_len = word.chars().count();
        if word_len == 0 || word_len > chars.len() {
            continue;
        }

        let mut start = 0;
        while start + word_len <= chars.len() {
            let segment: String = chars[start..start + word_len]
                .iter()
                .collect::<String>()
                .to_lowercase();
            if segment == *word {
                matches.push((start, start + word_len));
                start += word_len;
            } else {
                start += 1;
            }
        }
    }

    matches.sort_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in matches {
        if let Some(last) = merged.last_mut() {
            if r.0 <= last.1 {
                last.1 = last.1.max(r.1);
            } else {
                merged.push(r);
            }
        } else {
            merged.push(r);
        }
    }
    merged
}

fn filter_type_from_prefix(query: &str) -> FilterType {
    let q = query.to_lowercase();
    if q.starts_with("file:") {
        FilterType::Files
    } else if q.starts_with("folder:") {
        FilterType::Folders
    } else if q.starts_with("code:") || q.starts_with("commits:") || q.starts_with("todos:") {
        FilterType::Code
    } else if q.starts_with("img:") || q.starts_with("image:") || q.starts_with("screenshots:") {
        FilterType::Images
    } else {
        FilterType::All
    }
}

fn filter_index(ftype: FilterType) -> usize {
    match ftype {
        FilterType::All => 0,
        FilterType::Files => 1,
        FilterType::Folders => 2,
        FilterType::Images => 3,
        FilterType::Code => 4,
        FilterType::Settings => 5,
        FilterType::Commands => 6,
    }
}

fn apply_sort(results: &mut Vec<SearchResult>, sort_asc: bool, query: &str) {
    let q_trimmed = query.trim().to_lowercase();
    if q_trimmed.is_empty() {
        return;
    }
    if q_trimmed.starts_with("history:")
        || q_trimmed.starts_with("clipboard:")
        || q_trimmed.starts_with("clip:")
    {
        return;
    }
    if sort_asc {
        results.sort_by(|a, b| {
            a.entry
                .control_name
                .to_lowercase()
                .cmp(&b.entry.control_name.to_lowercase())
        });
    } else {
        let q = clean_query_prefix(query).trim().to_lowercase();
        results.sort_by(|a, b| best_match_cmp(a, b, &q));
    }
}

fn best_match_cmp(a: &SearchResult, b: &SearchResult, query: &str) -> std::cmp::Ordering {
    let score_a = calculate_relevance_score(a, query);
    let score_b = calculate_relevance_score(b, query);
    score_b
        .partial_cmp(&score_a)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            a.entry
                .control_name
                .to_lowercase()
                .cmp(&b.entry.control_name.to_lowercase())
        })
}

fn calculate_relevance_score(result: &SearchResult, query: &str) -> f32 {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return result.score;
    }

    let title = result.entry.control_name.to_lowercase();
    let stem = title.rsplit_once('.').map(|(s, _)| s).unwrap_or(&title);

    let mut score = 0.0;

    // 1. Title matching
    if title == q || stem == q {
        score += 10000.0;
    } else if title.starts_with(&q) || stem.starts_with(&q) {
        score += 8000.0;
    } else {
        let mut word_prefix_match = false;
        for word in title.split(|c: char| !c.is_alphanumeric()) {
            if word.starts_with(&q) && !word.is_empty() {
                word_prefix_match = true;
                break;
            }
        }
        if word_prefix_match {
            score += 6000.0;
        } else if title.contains(&q) || stem.contains(&q) {
            score += 4000.0;
        } else {
            let words: Vec<&str> = stem
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            let q_words: Vec<&str> = q.split_whitespace().collect();
            if !q_words.is_empty() {
                let matched = q_words.iter().filter(|qw| words.contains(qw)).count();
                if matched > 0 {
                    score += 2000.0 * (matched as f32 / q_words.len() as f32);
                }
            }
        }
    }

    // Acronym / abbreviation match
    if q.len() >= 2 {
        let first_letters: String = title
            .split(|c: char| !c.is_alphanumeric())
            .filter_map(|w| w.chars().next())
            .collect();
        if first_letters.starts_with(&q) {
            score += 3000.0;
        }
    }

    // 2. Path / Command matching
    let path = result.entry.launch_command.to_lowercase();
    if path.contains(&q) {
        score += 100.0;
    }

    // 3. Source priority weight
    let src_priority = source_priority(&result.entry.source, &result.entry.launch_command);
    score += src_priority * 2.0;

    // 4. Base score
    score += result.score.clamp(0.0, 500.0);

    score
}

fn source_priority(source: &str, command: &str) -> f32 {
    if source == "app" {
        60.0
    } else if matches!(source, "FILE" | "FOLDER" | "RECENT" | "CODE") {
        50.0
    } else if source.eq_ignore_ascii_case("settings")
        || source.eq_ignore_ascii_case("control")
        || command.starts_with("ms-settings:")
        || command.starts_with("control")
        || command.contains(".cpl")
        || command.ends_with(".msc")
    {
        45.0
    } else if matches!(source, "BOOKMARK" | "HISTORY" | "QUICKLINK") {
        40.0
    } else if source == "web" {
        30.0
    } else if matches!(
        source,
        "FILE_CONTENT" | "CODE_CONTENT" | "PDF" | "DOCX" | "OCR"
    ) {
        20.0
    } else {
        10.0
    }
}

fn is_image_file_path(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".png")
        || p.ends_with(".jpg")
        || p.ends_with(".jpeg")
        || p.ends_with(".gif")
        || p.ends_with(".bmp")
        || p.ends_with(".webp")
        || p.ends_with(".ico")
        || p.ends_with(".tiff")
        || p.ends_with(".svg")
}

fn is_code_file_path(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".rs")
        || p.ends_with(".py")
        || p.ends_with(".js")
        || p.ends_with(".ts")
        || p.ends_with(".jsx")
        || p.ends_with(".tsx")
        || p.ends_with(".cpp")
        || p.ends_with(".h")
        || p.ends_with(".c")
        || p.ends_with(".cs")
        || p.ends_with(".go")
        || p.ends_with(".java")
        || p.ends_with(".html")
        || p.ends_with(".css")
        || p.ends_with(".json")
        || p.ends_with(".toml")
        || p.ends_with(".yaml")
        || p.ends_with(".yml")
        || p.ends_with(".sh")
        || p.ends_with(".bat")
        || p.ends_with(".ps1")
        || p.ends_with(".sln")
        || p.ends_with(".iss")
}

fn result_matches_filter(r: &SearchResult, ftype: FilterType) -> bool {
    let src = r.entry.source.as_str();
    let cmd = r.entry.launch_command.as_str();
    match ftype {
        FilterType::All => true,
        FilterType::Files => src == "FILE" || src == "RECENT",
        FilterType::Folders => src == "FOLDER",
        FilterType::Images => {
            src == "IMAGE"
                || cmd.starts_with("copy_image:")
                || ((src == "FILE" || src == "RECENT") && is_image_file_path(cmd))
        }
        FilterType::Code => {
            src == "CODE"
                || src == "CODE_CONTENT"
                || src == "COMMIT"
                || src == "TODO"
                || src == "SNIPPET"
                || ((src == "FILE" || src == "RECENT") && is_code_file_path(cmd))
        }
        FilterType::Settings => {
            src.eq_ignore_ascii_case("settings")
                || src.eq_ignore_ascii_case("control")
                || cmd.starts_with("ms-settings:")
                || cmd.starts_with("control")
                || cmd.contains(".cpl")
                || cmd.ends_with(".msc")
        }
        FilterType::Commands => {
            (src == "SYSTEM"
                || src == "WINDOW"
                || src == "ACTION"
                || src == "AI"
                || src.eq_ignore_ascii_case("app"))
                && !(cmd.starts_with("ms-settings:")
                    || cmd.starts_with("control")
                    || cmd.contains(".cpl")
                    || cmd.ends_with(".msc")
                    || src.eq_ignore_ascii_case("settings")
                    || src.eq_ignore_ascii_case("control"))
        }
    }
}

fn filter_counts_for_results(results: &[SearchResult]) -> [usize; 7] {
    let mut counts = [0; 7];
    counts[0] = results.len();
    for r in results {
        for ftype in [
            FilterType::Files,
            FilterType::Folders,
            FilterType::Images,
            FilterType::Code,
            FilterType::Settings,
            FilterType::Commands,
        ] {
            if result_matches_filter(r, ftype) {
                counts[filter_index(ftype)] += 1;
            }
        }
    }
    counts
}

fn filter_pill_rects(s: &State, x_start: i32, list_y: i32) -> Vec<(FilterType, RECT)> {
    let filters = [
        ("All", FilterType::All),
        ("Files", FilterType::Files),
        ("Folders", FilterType::Folders),
        ("Images", FilterType::Images),
        ("Code", FilterType::Code),
        ("Settings", FilterType::Settings),
        ("Commands", FilterType::Commands),
    ];

    let mut res = Vec::new();
    unsafe {
        let hdc = GetDC(HWND(std::ptr::null_mut()));
        let old_font = SelectObject(hdc, s.font_c);
        let mut fx = x_start + PAD_L - s.filter_scroll_x;
        for &(label, ftype) in &filters {
            let count = s.filter_counts[filter_index(ftype)];
            let full_label = format!("{} {}", label, count);
            let lw: Vec<u16> = full_label.encode_utf16().collect();
            let mut sz_lbl = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &lw, &mut sz_lbl);

            let fw = sz_lbl.cx + 16;

            res.push((
                ftype,
                RECT {
                    left: fx,
                    top: list_y + 8,
                    right: fx + fw,
                    bottom: list_y + 40,
                },
            ));

            fx += fw + 8;
        }
        SelectObject(hdc, old_font);
        let _ = ReleaseDC(HWND(std::ptr::null_mut()), hdc);
    }
    res
}

unsafe fn badge_custom(hdc: HDC, s: &State, label: &str, x: i32, y: i32, w: i32) {
    let palette = s.theme.palette();
    let bg_color = palette.clr_bdgbg;
    let tx_color = palette.clr_bdgtx;
    fill_rounded(hdc, x, y, w, BADGE_H, 5, bg_color);
    SelectObject(hdc, s.font_b);
    SetTextColor(hdc, tx_color);
    SetBkMode(hdc, TRANSPARENT);
    let mut t: Vec<u16> = label.encode_utf16().collect();
    let mut r = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + BADGE_H,
    };
    let _ = DrawTextW(
        hdc,
        &mut t,
        &mut r,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

unsafe fn draw_highlighted_text(
    hdc: HDC,
    text: &str,
    query: &str,
    font: HFONT,
    default_color: COLORREF,
    highlight_color: COLORREF,
    x: i32,
    y: i32,
) {
    let old_font = SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);

    let merged = highlighted_text_ranges(text, query);

    if merged.is_empty() {
        SetTextColor(hdc, default_color);
        let mut t: Vec<u16> = text.encode_utf16().collect();
        let mut r = RECT {
            left: x,
            top: y,
            right: x + 2000,
            bottom: y + 20,
        };
        let _ = DrawTextW(
            hdc,
            &mut t,
            &mut r,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old_font);
        return;
    }

    let mut current_idx = 0;
    let mut cur_x = x;
    let chars: Vec<char> = text.chars().collect();

    for (start, end) in merged {
        if start > current_idx {
            let segment: String = chars[current_idx..start].iter().collect();
            let mut w_seg: Vec<u16> = segment.encode_utf16().collect();
            SetTextColor(hdc, default_color);
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &w_seg, &mut size);
            let mut r = RECT {
                left: cur_x,
                top: y,
                right: cur_x + size.cx + 10,
                bottom: y + 20,
            };
            let _ = DrawTextW(
                hdc,
                &mut w_seg,
                &mut r,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            cur_x += size.cx;
        }

        let segment: String = chars[start..end].iter().collect();
        let mut w_seg: Vec<u16> = segment.encode_utf16().collect();
        SetTextColor(hdc, highlight_color);
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &w_seg, &mut size);
        let mut r = RECT {
            left: cur_x,
            top: y,
            right: cur_x + size.cx + 10,
            bottom: y + 20,
        };
        let _ = DrawTextW(
            hdc,
            &mut w_seg,
            &mut r,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        cur_x += size.cx;

        current_idx = end;
    }

    if current_idx < chars.len() {
        let segment: String = chars[current_idx..].iter().collect();
        let mut w_seg: Vec<u16> = segment.encode_utf16().collect();
        SetTextColor(hdc, default_color);
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &w_seg, &mut size);
        let mut r = RECT {
            left: cur_x,
            top: y,
            right: cur_x + size.cx + 10,
            bottom: y + 20,
        };
        let _ = DrawTextW(
            hdc,
            &mut w_seg,
            &mut r,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }

    SelectObject(hdc, old_font);
}

unsafe fn fill(hdc: HDC, x: i32, y: i32, w: i32, h: i32, c: COLORREF) {
    let br = CreateSolidBrush(c);
    let _ = FillRect(
        hdc,
        &RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        br,
    );
    let _ = DeleteObject(br);
}

unsafe fn draw_rounded_border_and_bg(
    hdc: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    bg: COLORREF,
    border: COLORREF,
) {
    fill_rounded(hdc, x, y, w, h, r, border);
    fill_rounded(hdc, x + 1, y + 1, w - 2, h - 2, r - 1, bg);
}

fn source_section_label_res(res: &SearchResult) -> &'static str {
    let source = res.entry.source.as_str();
    let cmd = res.entry.launch_command.as_str();
    if source.eq_ignore_ascii_case("settings")
        || source.eq_ignore_ascii_case("control")
        || cmd.starts_with("ms-settings:")
        || cmd.starts_with("control")
        || cmd.contains(".cpl")
        || cmd.ends_with(".msc")
    {
        "SETTINGS"
    } else {
        source_section_label(source)
    }
}

fn source_section_label(source: &str) -> &'static str {
    if source.eq_ignore_ascii_case("app") || source.eq_ignore_ascii_case("window") {
        "APPS"
    } else if source.eq_ignore_ascii_case("file")
        || source.eq_ignore_ascii_case("recent")
        || source.eq_ignore_ascii_case("folder")
    {
        "FILES"
    } else if source.eq_ignore_ascii_case("code")
        || source.eq_ignore_ascii_case("todo")
        || source.eq_ignore_ascii_case("snippet")
    {
        "CODE MATCHES"
    } else if source.eq_ignore_ascii_case("memory") {
        "MEMORY"
    } else if source.eq_ignore_ascii_case("clipboard") || source.eq_ignore_ascii_case("pinned_clip")
    {
        "CLIPBOARD"
    } else if source.eq_ignore_ascii_case("bookmark")
        || source.eq_ignore_ascii_case("browser")
        || source.eq_ignore_ascii_case("history")
        || source.eq_ignore_ascii_case("quicklink")
        || source.eq_ignore_ascii_case("web")
    {
        "WEB"
    } else if source.eq_ignore_ascii_case("commit") {
        "GIT"
    } else if source.eq_ignore_ascii_case("ai")
        || source.eq_ignore_ascii_case("action")
        || source.eq_ignore_ascii_case("calc")
        || source.eq_ignore_ascii_case("confirm")
        || source.eq_ignore_ascii_case("live")
        || source.eq_ignore_ascii_case("project")
        || source.eq_ignore_ascii_case("translated")
    {
        "COMMANDS"
    } else {
        "RESULTS"
    }
}

unsafe fn key_hint(hdc: HDC, s: &State, x: i32, y: i32, key: &str, label: &str) -> i32 {
    let key_w = (key.chars().count() as i32 * 7 + 14).max(24);
    fill_rounded(hdc, x, y, key_w, 16, 4, s.theme.palette().clr_bdgbg);
    SelectObject(hdc, s.font_b);
    SetTextColor(hdc, s.theme.palette().clr_bdgtx);
    SetBkMode(hdc, TRANSPARENT);
    let mut key_text: Vec<u16> = key.encode_utf16().collect();
    let mut key_rect = RECT {
        left: x,
        top: y,
        right: x + key_w,
        bottom: y + 16,
    };
    let _ = DrawTextW(
        hdc,
        &mut key_text,
        &mut key_rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    SelectObject(hdc, s.font_c);
    SetTextColor(hdc, s.theme.palette().clr_gray);
    let label_x = x + key_w + 5;
    let label_w = label.chars().count() as i32 * 7 + 4;
    let mut label_text: Vec<u16> = label.encode_utf16().collect();
    let mut label_rect = RECT {
        left: label_x,
        top: y,
        right: label_x + label_w,
        bottom: y + 16,
    };
    let _ = DrawTextW(
        hdc,
        &mut label_text,
        &mut label_rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    label_x + label_w
}

unsafe fn badge(hdc: HDC, s: &State, source: &str, x: i32, y: i32) {
    let palette = s.theme.palette();
    let src_lc = source.to_lowercase();
    let (label, bg_color, tx_color) = if src_lc == "window" {
        ("WIN", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "live" {
        ("LIVE", COLORREF(0x00_31_46_35), COLORREF(0x00_A8_DF_A0))
    } else if src_lc == "project" {
        ("PROJ", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "action" {
        ("ACT", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "translated" {
        ("OK", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "web" {
        ("WEB", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "app" {
        ("APP", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "ai" {
        ("AI", COLORREF(0x00_46_37_3A), COLORREF(0x00_F0_D0_D6))
    } else if src_lc == "quicklink" {
        ("LINK", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "snippet" {
        ("SNIP", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "calc" {
        ("CALC", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "recent" {
        ("REC", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "file" {
        ("FILE", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "code" {
        ("CODE", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "clipboard" {
        ("CLIP", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "pinned_clip" {
        ("PIN", COLORREF(0x00_46_43_31), COLORREF(0x00_F0_D6_AA))
    } else if src_lc == "confirm" {
        ("DEL", COLORREF(0x00_30_30_55), COLORREF(0x00_D6_D6_FF))
    } else if src_lc == "bookmark" {
        ("MARK", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "history" {
        ("HIST", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "folder" {
        ("DIR", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "commit" {
        ("GIT", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "todo" {
        ("TODO", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "memory" {
        ("MEM", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc == "browser" {
        ("BROW", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc.contains("legacy") {
        ("OLD", palette.clr_bdgbg, palette.clr_bdgtx)
    } else if src_lc.contains("native") {
        ("SYS", palette.clr_bdgbg, palette.clr_bdgtx)
    } else {
        ("SET", palette.clr_bdgbg, palette.clr_bdgtx)
    };
    fill_rounded(hdc, x, y, BADGE_W, BADGE_H, 5, bg_color);
    SelectObject(hdc, s.font_b);
    SetTextColor(hdc, tx_color);
    SetBkMode(hdc, TRANSPARENT);
    let mut t: Vec<u16> = label.encode_utf16().collect();
    let mut r = RECT {
        left: x,
        top: y,
        right: x + BADGE_W,
        bottom: y + BADGE_H,
    };
    DrawTextW(
        hdc,
        &mut t,
        &mut r,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

unsafe fn load_icon_from_dll(dll_name: &str, index: i32, size: i32) -> HICON {
    let mut filename_buf = [0u16; 260];
    let dll_wide: Vec<u16> = dll_name.encode_utf16().collect();
    for (dest, src) in filename_buf.iter_mut().zip(dll_wide.iter()) {
        *dest = *src;
    }

    let mut phicon = [HICON(std::ptr::null_mut())];
    let mut piconid_val = 0u32;

    let num = PrivateExtractIconsW(
        &filename_buf,
        index,
        size,
        size,
        Some(&mut phicon),
        Some(&mut piconid_val as *mut u32),
        1,
    );
    if num > 0 && !phicon[0].0.is_null() {
        phicon[0]
    } else {
        HICON(std::ptr::null_mut())
    }
}

unsafe fn load_png_to_hicon(bytes: &[u8], size: u32) -> HICON {
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, GetDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    if size == 0 {
        return HICON(null_mut());
    }

    let img = match image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
        Ok(img) => {
            let mut rgba = img.into_rgba8();
            // Premultiply alpha before resizing to prevent white fringe/halo artifacts
            for pixel in rgba.chunks_exact_mut(4) {
                let a = pixel[3] as u32;
                pixel[0] = ((pixel[0] as u32 * a) / 255) as u8;
                pixel[1] = ((pixel[1] as u32 * a) / 255) as u8;
                pixel[2] = ((pixel[2] as u32 * a) / 255) as u8;
            }
            image::imageops::resize(&rgba, size, size, image::imageops::FilterType::Lanczos3)
        }
        Err(_) => return HICON(null_mut()),
    };

    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return HICON(null_mut());
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let hdc = GetDC(HWND(null_mut()));
    if hdc.is_invalid() {
        return HICON(null_mut());
    }
    let mut bits: *mut u8 = null_mut();
    let h_color = CreateDIBSection(
        hdc,
        &bmi as *const _,
        DIB_RGB_COLORS,
        &mut bits as *mut _ as *mut *mut core::ffi::c_void,
        HANDLE(null_mut()),
        0,
    );

    let mut hicon = HICON(null_mut());
    if let Ok(h) = h_color {
        // Guard the color DIB handle and its backing bits before touching memory.
        let expected = (width as usize) * (height as usize) * 4;
        let src = img.as_raw();
        if !h.0.is_null() && !bits.is_null() && src.len() >= expected {
            let slice = std::slice::from_raw_parts_mut(bits, expected);
            for i in 0..(width as usize * height as usize) {
                // The image is already premultiplied, just copy with BGR order swap for Windows DIB
                slice[i * 4] = src[i * 4 + 2]; // B
                slice[i * 4 + 1] = src[i * 4 + 1]; // G
                slice[i * 4 + 2] = src[i * 4]; // R
                slice[i * 4 + 3] = src[i * 4 + 3]; // A
            }
        }

        if !h.0.is_null() {
            // Monochrome AND-mask, EXPLICITLY zero-initialized. The old code passed a
            // NULL bits pointer (Some(null)) so the mask was uninitialised; on some
            // systems CreateIconIndirect then read past / dereferenced a bad mask and
            // faulted inside user32.dll (the 0xC0000005 startup crash). A real zeroed
            // buffer (over-allocated, always large enough) makes the alpha channel drive
            // transparency and keeps the call well-defined.
            let mask_bits = vec![0u8; (width as usize) * (height as usize) + 4];
            let h_mask = CreateBitmap(
                width as i32,
                height as i32,
                1,
                1,
                Some(mask_bits.as_ptr() as *const core::ffi::c_void),
            );
            if !h_mask.0.is_null() {
                let mut ii = ICONINFO {
                    fIcon: TRUE,
                    xHotspot: 0,
                    yHotspot: 0,
                    hbmMask: h_mask,
                    hbmColor: h,
                };
                hicon = CreateIconIndirect(&mut ii).unwrap_or(HICON(null_mut()));
                let _ = DeleteObject(h_mask);
            }
        }

        let _ = DeleteObject(h);
    }

    let _ = ReleaseDC(HWND(null_mut()), hdc);
    hicon
}

#[cfg(test)]
fn alpha_bounds(img: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (mut left, mut top) = (img.width(), img.height());
    let (mut right, mut bottom) = (0, 0);
    let mut found = false;
    for (x, y, pixel) in img.enumerate_pixels() {
        if pixel[3] != 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
            found = true;
        }
    }
    found.then_some((left, top, right - left + 1, bottom - top + 1))
}

unsafe fn get_active_app_name() -> String {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, BOOL};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    let hwnd_fg = GetForegroundWindow();
    if hwnd_fg.0.is_null() {
        return "Unknown".to_string();
    }

    let mut process_id: u32 = 0;
    GetWindowThreadProcessId(hwnd_fg, Some(&mut process_id));
    if process_id == 0 {
        return "Unknown".to_string();
    }

    if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), process_id) {
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);

        if res.is_ok() && size > 0 {
            return String::from_utf16_lossy(&buffer[..size as usize]);
        }
    }

    "Unknown".to_string()
}

unsafe fn paste_into_window(target: HWND) {
    if target.0.is_null() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    let _ = SetForegroundWindow(target);
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VK_CONTROL,
    };
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x56),
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x56),
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn copy_image_to_clipboard(hwnd: HWND, file_path: &str) -> bool {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    let dib = match image::open(file_path).ok().and_then(image_to_dib_bytes) {
        Some(dib) => dib,
        None => return false,
    };
    let h_mem = match GlobalAlloc(GMEM_MOVEABLE, dib.len()) {
        Ok(handle) => handle,
        Err(_) => return false,
    };
    let ptr = GlobalLock(h_mem);
    if ptr.is_null() {
        let _ = GlobalFree(h_mem);
        return false;
    }
    std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr as *mut u8, dib.len());
    let _ = GlobalUnlock(h_mem);

    // Serialize against other clipboard operations (e.g. paste_sequentially on a background thread).
    let _clip_guard = clipboard_lock().lock().unwrap_or_else(|e| e.into_inner());
    if OpenClipboard(hwnd).is_err() {
        let _ = GlobalFree(h_mem);
        return false;
    }
    let _ = EmptyClipboard();
    let copied = SetClipboardData(8, HANDLE(h_mem.0)).is_ok();
    let _ = CloseClipboard();
    if !copied {
        let _ = GlobalFree(h_mem);
    }
    copied
}

fn image_to_dib_bytes(image: image::DynamicImage) -> Option<Vec<u8>> {
    let rgba = image.into_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let pixel_bytes = width.checked_mul(height)?.checked_mul(4)? as usize;
    let mut dib = Vec::with_capacity(40 + pixel_bytes);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(width as i32).to_le_bytes());
    dib.extend_from_slice(&(height as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    for row in rgba.rows().rev() {
        for pixel in row {
            dib.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
    }
    Some(dib)
}

unsafe fn paste_clipboard_into_chat(hwnd: HWND, s: &mut State) {
    if let Some(text) = paste_from_clipboard(hwnd) {
        let clean_text: String = text.chars().filter(|c| !c.is_control()).collect();
        if s.text_selected {
            s.chat_input.clear();
            s.chat_cursor_pos = 0;
            s.text_selected = false;
        }
        s.chat_input.insert_str(s.chat_cursor_pos, &clean_text);
        s.chat_cursor_pos += clean_text.len();
    } else if save_clipboard_image(hwnd, &s.db_path, "Pasted Image").is_some() {
        let marker = "[Pasted image saved to Clipboard History]";
        if s.text_selected {
            s.chat_input.clear();
            s.chat_cursor_pos = 0;
            s.text_selected = false;
        }
        s.chat_input.insert_str(s.chat_cursor_pos, marker);
        s.chat_cursor_pos += marker.len();
    }
    reset_cursor_blink(hwnd, s);
    let _ = InvalidateRect(hwnd, None, FALSE);
}

unsafe fn paste_clipboard_into_query(hwnd: HWND, s: &mut State, search_now: bool) {
    if let Some(text) = paste_from_clipboard(hwnd) {
        let clean_text: String = text.chars().filter(|c| !c.is_control()).collect();
        if s.text_selected {
            s.query = clean_text;
            s.cursor_pos = s.query.len();
            s.text_selected = false;
        } else {
            s.query.insert_str(s.cursor_pos, &clean_text);
            s.cursor_pos += clean_text.len();
        }
        if search_now {
            s.selected = 0;
            s.scroll_offset = 0;
            s.results.clear();
            kick_debounce(hwnd, s);
        }
    } else if save_clipboard_image(hwnd, &s.db_path, "Pasted Image").is_some() {
        s.query = "clip:".to_string();
        s.cursor_pos = s.query.len();
        s.text_selected = false;
        s.selected = 0;
        s.scroll_offset = 0;
        trigger_search(hwnd, s);
    }
    reset_cursor_blink(hwnd, s);
    let _ = InvalidateRect(hwnd, None, FALSE);
}

fn clipboard_image_path(
    db_path: &std::path::Path,
    now_ms: u128,
) -> Option<(std::path::PathBuf, String)> {
    let img_dir = db_path.parent()?.join("clipboard_images");
    let filename = format!("image_{}.bmp", now_ms);
    let img_path = img_dir.join(&filename);
    Some((img_path.clone(), img_path.to_string_lossy().to_string()))
}

unsafe fn save_clipboard_image(
    hwnd: HWND,
    db_path: &std::path::Path,
    source_app: &str,
) -> Option<String> {
    let Some(bmp_bytes) = capture_clipboard_bmp_bytes(hwnd) else {
        applog::log("clipboard OCR: no supported clipboard image format found");
        return None;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let timestamp = now.as_millis() as i64;
    let memory_timestamp = now.as_secs() as i64;
    let (img_path, img_path_str) = clipboard_image_path(db_path, now.as_millis())?;
    std::fs::create_dir_all(img_path.parent()?).ok()?;
    std::fs::write(&img_path, bmp_bytes).ok()?;

    let conn = rusqlite::Connection::open(db_path).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
    conn.execute(
        "INSERT INTO clipboard_history (content, timestamp, source_app, is_image, pinned) \
         VALUES (?, ?, ?, 1, 0) \
         ON CONFLICT(content) DO UPDATE SET \
             timestamp = excluded.timestamp, \
             source_app = excluded.source_app, \
             is_image = excluded.is_image;",
        rusqlite::params![img_path_str, timestamp, source_app],
    )
    .ok()?;
    search::insert_memory_event(
        &conn,
        memory_timestamp,
        "Clipboard",
        "Copied Image",
        &format!("Copied image from {}", source_app),
        &img_path_str,
        source_app,
        Some(&img_path_str),
        None,
    );
    let _ = conn.execute(
        "DELETE FROM clipboard_history WHERE pinned = 0 AND id NOT IN (SELECT id FROM clipboard_history ORDER BY pinned DESC, timestamp DESC LIMIT 500);",
        [],
    );

    // Kick off background OCR so the image text becomes searchable.
    let ocr_img_path = img_path_str.clone();
    let ocr_db_path = db_path.to_path_buf();
    let hwnd_refresh = hwnd.0 as isize;
    std::thread::spawn(move || {
        let com_initialized = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        }
        .is_ok();
        if let Some(text) =
            run_clipboard_ocr(|| indexer::ocr_image_file(std::path::Path::new(&ocr_img_path)))
        {
            if let Ok(conn) = rusqlite::Connection::open(&ocr_db_path) {
                let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                let _ = conn.execute(
                    "UPDATE clipboard_history SET ocr_text = ? WHERE content = ?;",
                    rusqlite::params![text, ocr_img_path],
                );
            }
        }
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                windows::Win32::Foundation::HWND(hwnd_refresh as *mut std::ffi::c_void),
                WM_REFRESH_SEARCH,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if com_initialized {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    });

    Some(img_path.to_string_lossy().to_string())
}

unsafe fn ocr_clipboard_image_with_win32_fallback(hwnd: HWND) -> Option<String> {
    if let Some(text) = indexer::ocr_clipboard_image() {
        return Some(text);
    }

    let bmp_bytes = capture_clipboard_bmp_bytes(hwnd)?;
    let path = std::env::temp_dir().join(format!(
        "omnisearch_lite_clipboard_ocr_{}.bmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    if let Err(err) = std::fs::write(&path, bmp_bytes) {
        applog::log(&format!("clipboard OCR: temp image write failed: {}", err));
        return None;
    }
    let result = indexer::ocr_image_file(&path);
    let _ = std::fs::remove_file(path);
    if result.is_none() {
        applog::log("clipboard OCR: OCR completed without readable text");
    }
    result
}

fn latest_clipboard_image_path(db_path: &std::path::Path) -> Option<String> {
    let conn = rusqlite::Connection::open(db_path).ok()?;
    conn.query_row(
        "SELECT content FROM clipboard_history WHERE is_image = 1 ORDER BY pinned DESC, timestamp DESC LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    ).ok()
}

unsafe fn capture_clipboard_bmp_bytes(hwnd: HWND) -> Option<Vec<u8>> {
    capture_clipboard_dib_bmp_bytes(hwnd).or_else(|| {
        let (buf, bih) = capture_clipboard_image_data(hwnd)?;
        bmp_file_bytes(&buf, bih)
    })
}

unsafe fn capture_clipboard_dib_bmp_bytes(hwnd: HWND) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;
    let format = if IsClipboardFormatAvailable(CF_DIBV5).is_ok() {
        CF_DIBV5
    } else if IsClipboardFormatAvailable(CF_DIB).is_ok() {
        CF_DIB
    } else {
        return None;
    };

    if OpenClipboard(hwnd).is_err() {
        return None;
    }

    let result = (|| {
        let handle = GetClipboardData(format).ok()?;
        if handle.0.is_null() {
            return None;
        }
        let hglobal = HGLOBAL(handle.0);
        let size = GlobalSize(hglobal);
        if size == 0 {
            return None;
        }
        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            return None;
        }
        let dib = std::slice::from_raw_parts(ptr as *const u8, size);
        let bytes = dib_to_bmp_file_bytes(dib);
        let _ = GlobalUnlock(hglobal);
        bytes
    })();

    let _ = CloseClipboard();
    result
}

unsafe fn capture_clipboard_image_data(
    hwnd: HWND,
) -> Option<(Vec<u8>, windows::Win32::Graphics::Gdi::BITMAPINFOHEADER)> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };

    // CF_BITMAP is 2
    if IsClipboardFormatAvailable(2).is_err() {
        return None;
    }

    if OpenClipboard(hwnd).is_err() {
        return None;
    }

    let mut result = None;
    if let Ok(h_mem) = GetClipboardData(2) {
        if !h_mem.0.is_null() {
            let hbitmap = HBITMAP(h_mem.0);
            let hdc_screen = GetDC(HWND(std::ptr::null_mut()));
            if !hdc_screen.is_invalid() {
                let mut bmp: BITMAP = std::mem::zeroed();
                let size = std::mem::size_of::<BITMAP>() as i32;
                if GetObjectW(hbitmap, size, Some(&mut bmp as *mut BITMAP as *mut _)) != 0 {
                    let Some(byte_len) = clipboard_bitmap_byte_len(bmp.bmWidth, bmp.bmHeight)
                    else {
                        let _ = ReleaseDC(HWND(std::ptr::null_mut()), hdc_screen);
                        let _ = CloseClipboard();
                        return None;
                    };
                    let bih = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: bmp.bmWidth,
                        biHeight: bmp.bmHeight,
                        biPlanes: 1,
                        biBitCount: 32,   // Convert to 32-bit BGRA
                        biCompression: 0, // BI_RGB
                        biSizeImage: byte_len as u32,
                        biXPelsPerMeter: 0,
                        biYPelsPerMeter: 0,
                        biClrUsed: 0,
                        biClrImportant: 0,
                    };

                    let mut bi = BITMAPINFO {
                        bmiHeader: bih,
                        bmiColors: [std::mem::zeroed(); 1],
                    };

                    let mut buf = vec![0u8; byte_len];

                    let res = GetDIBits(
                        hdc_screen,
                        hbitmap,
                        0,
                        bmp.bmHeight as u32,
                        Some(buf.as_mut_ptr() as *mut _),
                        &mut bi,
                        DIB_RGB_COLORS,
                    );

                    if res != 0 {
                        result = Some((buf, bih));
                    }
                }
                let _ = ReleaseDC(HWND(std::ptr::null_mut()), hdc_screen);
            }
        }
    }
    let _ = CloseClipboard();
    result
}

fn bmp_file_bytes(
    buf: &[u8],
    bih: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER,
) -> Option<Vec<u8>> {
    let mut file_header = [0u8; 14];
    let file_size = 54usize.checked_add(buf.len())?;
    file_header[0] = b'B';
    file_header[1] = b'M';
    file_header[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
    file_header[10..14].copy_from_slice(&54u32.to_le_bytes());

    let mut info_header = [0u8; 40];
    info_header[0..4].copy_from_slice(&bih.biSize.to_le_bytes());
    info_header[4..8].copy_from_slice(&bih.biWidth.to_le_bytes());
    info_header[8..12].copy_from_slice(&bih.biHeight.to_le_bytes());
    info_header[12..14].copy_from_slice(&bih.biPlanes.to_le_bytes());
    info_header[14..16].copy_from_slice(&bih.biBitCount.to_le_bytes());
    info_header[16..20].copy_from_slice(&bih.biCompression.to_le_bytes());
    info_header[20..24].copy_from_slice(&bih.biSizeImage.to_le_bytes());

    let mut bytes = Vec::with_capacity(file_size);
    bytes.extend_from_slice(&file_header);
    bytes.extend_from_slice(&info_header);
    bytes.extend_from_slice(buf);
    Some(bytes)
}

fn clipboard_bitmap_byte_len(width: i32, height: i32) -> Option<usize> {
    if width <= 0 || height <= 0 || width > 6000 || height > 6000 {
        return None;
    }
    let bytes = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if bytes > 50 * 1024 * 1024 {
        return None;
    }
    Some(bytes)
}

fn dib_to_bmp_file_bytes(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    if header_size < 40 || header_size > dib.len() {
        return None;
    }
    let bit_count = u16::from_le_bytes(dib[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let clr_used = u32::from_le_bytes(dib[32..36].try_into().ok()?) as usize;
    let color_count = if clr_used > 0 {
        clr_used
    } else if bit_count <= 8 {
        1usize.checked_shl(bit_count as u32).unwrap_or(0)
    } else {
        0
    };
    let masks_size = if header_size == 40 && (compression == 3 || compression == 6) {
        12
    } else {
        0
    };
    let pixel_offset = 14usize
        .checked_add(header_size)?
        .checked_add(masks_size)?
        .checked_add(color_count.checked_mul(4)?)?;
    let file_size = 14usize.checked_add(dib.len())?;

    let mut file_header = [0u8; 14];
    file_header[0] = b'B';
    file_header[1] = b'M';
    file_header[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
    file_header[10..14].copy_from_slice(&(pixel_offset as u32).to_le_bytes());

    let mut bytes = Vec::with_capacity(file_size);
    bytes.extend_from_slice(&file_header);
    bytes.extend_from_slice(dib);
    Some(bytes)
}

unsafe fn copy_to_clipboard(hwnd: HWND, text: &str) {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    // Serialize all clipboard access through the shared lock.
    let _clip_guard = clipboard_lock().lock().unwrap_or_else(|e| e.into_inner());
    if OpenClipboard(hwnd).is_ok() {
        let _ = EmptyClipboard();
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let size = utf16.len() * 2;
        if let Ok(h_mem) = GlobalAlloc(GMEM_MOVEABLE, size) {
            let ptr = GlobalLock(h_mem);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, size);
                let _ = GlobalUnlock(h_mem);
                let _ = SetClipboardData(13, HANDLE(h_mem.0));
            }
        }
        let _ = CloseClipboard();
    }
}

unsafe fn paste_from_clipboard(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    // Serialize clipboard access. The WM_CLIPBOARDUPDATE handler already holds
    // the lock before calling this function; callers that are not inside that guard
    // will acquire it here so clipboard reads are always serialized.
    let _clip_guard = clipboard_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut result = None;
    if OpenClipboard(hwnd).is_ok() {
        if let Ok(h_mem) = GetClipboardData(13) {
            if !h_mem.0.is_null() {
                let h_global = HGLOBAL(h_mem.0);
                let size = GlobalSize(h_global);
                let max_len = size / 2;
                let ptr = GlobalLock(h_global);
                if !ptr.is_null() {
                    let mut len = 0;
                    let ptr_u16 = ptr as *const u16;
                    while len < max_len && *ptr_u16.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(ptr_u16, len);
                    if let Ok(s) = String::from_utf16(slice) {
                        result = Some(s);
                    }
                    let _ = GlobalUnlock(h_global);
                }
            }
        }
        let _ = CloseClipboard();
    }
    result
}

unsafe fn start_timeline_tracker(db_path: std::path::PathBuf, launcher_hwnd: SendHwnd) {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, BOOL, HWND};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    let launcher_hwnd = launcher_hwnd.0;

    // Open a single persistent SQLite connection for the lifetime of this tracker thread.
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

    let mut last_hwnd = HWND::default();
    let mut last_title = String::new();
    let mut last_app = String::new();
    let mut focus_start = std::time::Instant::now();
    let mut focus_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            continue;
        }

        // Skip our own launcher window
        if fg == launcher_hwnd {
            continue;
        }

        // Get window title
        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(fg, &mut title_buf);
        let title = if len > 0 {
            String::from_utf16_lossy(&title_buf[..len as usize])
        } else {
            String::new()
        };

        // Get app name (process filename)
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));

        let app = if pid != 0 {
            if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid) {
                let mut buffer = [0u16; 512];
                let mut size = buffer.len() as u32;
                let res = QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_WIN32,
                    PWSTR(buffer.as_mut_ptr()),
                    &mut size,
                );
                let _ = CloseHandle(handle);
                if res.is_ok() && size > 0 {
                    String::from_utf16_lossy(&buffer[..size as usize])
                } else {
                    "Unknown".to_string()
                }
            } else {
                "Unknown".to_string()
            }
        } else {
            "Unknown".to_string()
        };

        // Check if focus changed
        if fg != last_hwnd || title != last_title || app != last_app {
            let duration = focus_start.elapsed().as_secs() as i64;
            if (!last_title.is_empty() || !last_app.is_empty()) && duration >= 1 {
                log_timeline_event(&conn, focus_timestamp, duration, &last_app, &last_title);
            }
            last_hwnd = fg;
            last_title = title;
            last_app = app;
            focus_start = std::time::Instant::now();
            focus_timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
        }
    }
}

fn log_timeline_event(
    conn: &rusqlite::Connection,
    timestamp: i64,
    duration: i64,
    app_name: &str,
    window_title: &str,
) {
    let _ = conn.execute(
        "INSERT INTO timeline_events (timestamp, duration, app_name, window_title) VALUES (?, ?, ?, ?);",
        rusqlite::params![timestamp, duration, app_name, window_title],
    );
    search::insert_memory_event(
        conn,
        timestamp,
        "Timeline",
        "Active Window",
        window_title,
        &format!("Used {} for {} seconds", app_name, duration),
        app_name,
        None,
        None,
    );
    // Keep only the latest 10000 timeline events to bound DB size
    let _ = conn.execute(
        "DELETE FROM timeline_events WHERE id NOT IN (SELECT id FROM timeline_events ORDER BY timestamp DESC LIMIT 10000);",
        [],
    );
}

unsafe fn load_shell_thumbnail(path: &str, size: i32) -> Option<HBITMAP> {
    let wide_path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let item: windows::Win32::UI::Shell::IShellItem =
        windows::Win32::UI::Shell::SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None)
            .ok()?;
    let factory: windows::Win32::UI::Shell::IShellItemImageFactory = item.cast().ok()?;
    factory
        .GetImage(
            SIZE { cx: size, cy: size },
            windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY
                | windows::Win32::UI::Shell::SIIGBF_BIGGERSIZEOK,
        )
        .ok()
}

fn thumbnail_eviction_key(
    cache: &std::collections::HashMap<String, HBITMAP>,
    keep: &str,
) -> Option<String> {
    cache.keys().find(|key| key.as_str() != keep).cloned()
}

unsafe fn prune_thumbnail_cache(
    cache: &mut std::collections::HashMap<String, HBITMAP>,
    keep: &str,
) {
    while cache.len() > MAX_THUMBNAIL_CACHE {
        let Some(key) = thumbnail_eviction_key(cache, keep) else {
            break;
        };
        if let Some(hbitmap) = cache.remove(&key) {
            if !hbitmap.0.is_null() {
                let _ = DeleteObject(hbitmap);
            }
        }
    }
}

unsafe fn draw_cached_bmp(hdc: HDC, x: i32, y: i32, w: i32, h: i32, hbitmap: HBITMAP) {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetObjectW, SelectObject, SetStretchBltMode, StretchBlt,
        BITMAP, COLORONCOLOR,
    };

    let mem_dc = CreateCompatibleDC(hdc);
    if !mem_dc.is_invalid() {
        let mut bmp: BITMAP = std::mem::zeroed();
        let size = std::mem::size_of::<BITMAP>() as i32;
        if GetObjectW(hbitmap, size, Some(&mut bmp as *mut BITMAP as *mut _)) != 0 {
            let old_obj = SelectObject(mem_dc, hbitmap);
            let old_mode = SetStretchBltMode(hdc, COLORONCOLOR);
            let scale = (w as f32 / bmp.bmWidth as f32).min(h as f32 / bmp.bmHeight as f32);
            let draw_w = (bmp.bmWidth as f32 * scale).round() as i32;
            let draw_h = (bmp.bmHeight as f32 * scale).round() as i32;
            let _ = StretchBlt(
                hdc,
                x + (w - draw_w) / 2,
                y + (h - draw_h) / 2,
                draw_w,
                draw_h,
                mem_dc,
                0,
                0,
                bmp.bmWidth,
                bmp.bmHeight,
                SRCCOPY,
            );
            let _ = SetStretchBltMode(hdc, STRETCH_BLT_MODE(old_mode));
            let _ = SelectObject(mem_dc, old_obj);
        }
        let _ = DeleteDC(mem_dc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_width_honors_setting_and_small_screens() {
        assert_eq!(launcher_width_for_work_area(1920, 720), 720);
        assert_eq!(launcher_width_for_work_area(1920, 1000), DEFAULT_WIN_W);
        assert_eq!(launcher_width_for_work_area(640, 720), 600);
        assert_eq!(launcher_width_for_work_area(300, 720), 268);
    }

    #[test]
    fn test_registered_app_paths() {
        let chrome = get_registered_app_path("chrome.exe");
        let firefox = get_registered_app_path("firefox.exe");
        let edge = get_registered_app_path("msedge.exe");
        let brave = get_registered_app_path("brave.exe");

        println!("Chrome path: {:?}", chrome);
        println!("Firefox path: {:?}", firefox);
        println!("Edge path: {:?}", edge);
        println!("Brave path: {:?}", brave);

        unsafe {
            if let Some(p) = &chrome {
                let h1 = get_app_icon(p);
                let h2 = get_file_icon(p);
                println!(
                    "Chrome HICON: get_app_icon={:?}, get_file_icon={:?}",
                    h1.0, h2.0
                );
            }
            if let Some(p) = &firefox {
                let h1 = get_app_icon(p);
                let h2 = get_file_icon(p);
                println!(
                    "Firefox HICON: get_app_icon={:?}, get_file_icon={:?}",
                    h1.0, h2.0
                );
            }
            if let Some(p) = &brave {
                let h1 = get_app_icon(p);
                let h2 = get_file_icon(p);
                println!(
                    "Brave HICON: get_app_icon={:?}, get_file_icon={:?}",
                    h1.0, h2.0
                );
            }
        }
    }

    #[test]
    fn test_relative_time() {
        use std::time::{Duration, SystemTime};
        let ago = |s| SystemTime::now() - Duration::from_secs(s);
        assert_eq!(relative_time(ago(10)), "just now");
        assert_eq!(relative_time(ago(120)), "2m ago");
        assert_eq!(relative_time(ago(7200)), "2h ago");
        assert_eq!(relative_time(ago(90000)), "yesterday");
        assert_eq!(relative_time(ago(3 * 86400)), "3d ago");
        assert_eq!(relative_time(ago(14 * 86400)), "2w ago");
    }

    #[test]
    fn test_source_section_labels_for_launcher_groups() {
        assert_eq!(source_section_label("FILE"), "FILES");
        assert_eq!(source_section_label("recent"), "FILES");
        assert_eq!(source_section_label("CODE"), "CODE MATCHES");
        assert_eq!(source_section_label("ACTION"), "COMMANDS");
        assert_eq!(source_section_label("MEMORY"), "MEMORY");
        assert_eq!(source_section_label("unknown"), "RESULTS");
    }

    #[test]
    fn filter_counts_use_actual_search_results() {
        let mk = |source: &str, cmd: &str| SearchResult {
            score: 1.0,
            entry: search::CatalogEntry {
                id: format!("{}.{}", source, cmd),
                control_name: source.to_string(),
                breadcrumb_path: String::new(),
                launch_command: cmd.to_string(),
                source: source.to_string(),
                description: String::new(),
                synonyms: String::new(),
            },
        };
        let results = vec![
            mk("FILE", "C:\\readme.md"),
            mk("FOLDER", "C:\\Users"),
            mk("CODE", "C:\\main.rs"),
            mk("Settings", "ms-settings:display"),
            mk("ACTION", "control.exe /name Microsoft.WindowsUpdate"),
            mk("ACTION", "action:open_settings"),
            mk("OCR", "C:\\screen.png"),
        ];
        let counts = filter_counts_for_results(&results);
        assert_eq!(counts[filter_index(FilterType::All)], 6);
        assert_eq!(counts[filter_index(FilterType::Files)], 1);
        assert_eq!(counts[filter_index(FilterType::Folders)], 1);
        assert_eq!(counts[filter_index(FilterType::Code)], 1);
        assert_eq!(counts[filter_index(FilterType::Settings)], 2);
        assert_eq!(counts[filter_index(FilterType::Commands)], 1);
        assert_eq!(counts[filter_index(FilterType::Images)], 0);
    }

    #[test]
    fn clipboard_ocr_image_matches_ocr_filter() {
        let result = SearchResult {
            score: 1.0,
            entry: search::CatalogEntry {
                id: "clip.image.1".to_string(),
                control_name: "[Image] Copied from Snipping Tool".to_string(),
                breadcrumb_path: "Clipboard > Snipping Tool".to_string(),
                launch_command: "copy_image:C:\\clip.bmp".to_string(),
                source: "CLIPBOARD".to_string(),
                description: "🔤 invoice total due".to_string(),
                synonyms: "image clipboard invoice total due".to_string(),
            },
        };

        assert!(result_matches_filter(&result, FilterType::Images));
    }

    #[test]
    fn homepage_uses_current_default_actions() {
        let results = default_homepage_results_for_settings();

        assert!(results
            .iter()
            .any(|result| result.entry.launch_command == "sync:phone"));
        assert!(!results
            .iter()
            .any(|result| result.entry.launch_command == "action:ocr_clipboard"));
    }

    #[test]
    fn row_geometry_centers_icons_and_text() {
        assert_eq!(centered_in_result_row(100, RESULT_ICON_SIZE, 68), 118);
        assert_eq!(centered_in_result_row(100, RESULT_TEXT_BLOCK_H, 68), 114);
    }

    #[test]
    fn highlighted_text_ranges_use_char_indices_not_byte_offsets() {
        let text = "🔥 Windows Terminal";
        let ranges = highlighted_text_ranges(text, "te");
        let char_len = text.chars().count();

        assert_eq!(ranges, vec![(10, 12)]);
        assert!(ranges.iter().all(|(_, end)| *end <= char_len));
    }

    #[test]
    fn search_row_invalidation_only_covers_search_header() {
        let rect = search_row_invalidation_rect(900, DEFAULT_WIN_W, 300, 581, 60);
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (30, 10, 870, 72)
        );
    }

    #[test]
    fn results_invalidation_starts_below_search_header() {
        let rect = results_invalidation_rect(900, DEFAULT_WIN_W, 300, 581, 60);
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (30, 71, 870, 591)
        );
    }

    #[test]
    fn normal_search_height_does_not_depend_on_result_count() {
        assert_eq!(normal_search_win_h(60, 54, 1, 8), 203);
        assert_eq!(normal_search_win_h(60, 54, 8, 8), 581);
    }

    #[test]
    fn scoped_results_height_matches_homepage_height() {
        assert_eq!(scoped_results_win_h(60, 54, 2, 8), homepage_win_h(60, 54, 2, 8));
        // Homepage/scoped use the compact label header; normal search uses the taller
        // filter + results header, so it is strictly taller for the same row count.
        assert!(scoped_results_win_h(60, 54, 2, 8) < normal_search_win_h(60, 54, 2, 8));
    }

    #[test]
    fn launcher_top_is_anchored_to_homepage_height() {
        // 60 + 1 + LABEL_HEADER_H(38) + 8*54 + 8 = 539
        assert_eq!(homepage_win_h(60, 54, 8, 8), 539);
        assert_eq!(launcher_top_y(300, 539), 31);
    }

    #[test]
    fn homepage_exposes_agents_page() {
        let results = default_homepage_results();
        assert!(results
            .iter()
            .any(|r| { r.entry.control_name == "Agents" && r.entry.launch_command == "agents:" }));
        assert_eq!(clean_query_prefix("agents: AI"), "AI");
    }

    #[test]
    fn default_agent_prompt_includes_pc_control_skills() {
        let prompt = default_agent_system_prompt();
        assert!(prompt.contains("open browser pages"));
        assert!(prompt.contains("PowerShell"));
        assert!(prompt.contains("After running any command"));
    }

    #[test]
    fn local_agent_action_routes_reasoning_requests_to_agent() {
        // "youtube"/"search" requests must NOT be intercepted anymore — they fall through to
        // the AI agent so it can reason (e.g. open a specific channel in a specific browser).
        assert!(local_agent_action("open mrwhostheboss youtube channel on brave").is_none());
        assert!(local_agent_action("open youtube and search for rust ownership").is_none());
        // Explicit shell commands still take the deterministic fast path.
        match local_agent_action("run powershell Get-Date") {
            Some(LocalAgentAction::RunPowerShell { command }) => assert_eq!(command, "Get-Date"),
            _ => panic!("expected RunPowerShell fast path"),
        }
    }

    #[test]
    fn agent_memory_extracts_explicit_remember_fact() {
        assert_eq!(
            extract_agent_memory_items("remember that I prefer concise answers."),
            vec!["I prefer concise answers".to_string()]
        );
    }

    #[test]
    fn best_match_prefers_title_match_over_raw_score() {
        let mk = |source: &str, name: &str, score: f32| SearchResult {
            score,
            entry: search::CatalogEntry {
                id: format!("{source}.{name}"),
                control_name: name.to_string(),
                breadcrumb_path: String::new(),
                launch_command: String::new(),
                source: source.to_string(),
                description: String::new(),
                synonyms: String::new(),
            },
        };
        let mut results = vec![
            mk("web", "Search Google for \"task\"", 500.0),
            mk("app", "Task Manager", 10.0),
            mk("FILE_CONTENT", "notes.txt", 600.0),
        ];
        apply_sort(&mut results, false, "task");
        assert_eq!(results[0].entry.control_name, "Task Manager");
    }

    #[test]
    fn best_match_keeps_content_below_file_title_match() {
        let mk = |source: &str, name: &str, score: f32| SearchResult {
            score,
            entry: search::CatalogEntry {
                id: format!("{source}.{name}"),
                control_name: name.to_string(),
                breadcrumb_path: String::new(),
                launch_command: String::new(),
                source: source.to_string(),
                description: String::new(),
                synonyms: String::new(),
            },
        };
        let mut results = vec![
            mk("OCR", "random-screenshot.png", 900.0),
            mk("FILE", "project-plan.txt", 20.0),
        ];
        apply_sort(&mut results, false, "project");
        assert_eq!(results[0].entry.control_name, "project-plan.txt");
    }

    #[test]
    fn source_icon_padding_is_trimmed() {
        let mut image = image::RgbaImage::new(8, 8);
        image.put_pixel(2, 1, image::Rgba([255, 255, 255, 255]));
        image.put_pixel(5, 6, image::Rgba([255, 255, 255, 255]));
        assert_eq!(alpha_bounds(&image), Some((2, 1, 4, 6)));
    }

    #[test]
    fn icon_eviction_keeps_requested_key() {
        let owned = ["current".to_string(), "older".to_string()]
            .into_iter()
            .collect();

        assert_eq!(
            owned_icon_eviction_key(&owned, "current"),
            Some("older".to_string())
        );
    }

    #[test]
    fn thumbnail_eviction_keeps_requested_path() {
        let mut cache = std::collections::HashMap::new();
        cache.insert("current".to_string(), HBITMAP(std::ptr::null_mut()));
        cache.insert("older".to_string(), HBITMAP(std::ptr::null_mut()));

        assert_eq!(
            thumbnail_eviction_key(&cache, "current"),
            Some("older".to_string())
        );
    }

    #[test]
    fn image_results_accept_files_and_clipboard_commands() {
        let result = |command: &str| SearchResult {
            score: 1.0,
            entry: search::CatalogEntry {
                id: String::new(),
                control_name: String::new(),
                breadcrumb_path: String::new(),
                launch_command: command.to_string(),
                source: "FILE".to_string(),
                description: String::new(),
                synonyms: String::new(),
            },
        };
        assert_eq!(
            image_path_for_result(&result("C:\\Pictures\\shot.PNG")),
            Some("C:\\Pictures\\shot.PNG")
        );
        assert_eq!(
            image_path_for_result(&result("copy_image:C:\\Pictures\\shot.jpg")),
            Some("C:\\Pictures\\shot.jpg")
        );
        assert_eq!(image_path_for_result(&result("C:\\notes.txt")), None);
    }

    #[test]
    fn native_shell_icons_cover_all_file_result_types() {
        for source in [
            "RECENT",
            "FILE",
            "FILE_CONTENT",
            "CODE",
            "CODE_CONTENT",
            "OCR",
        ] {
            assert!(is_file_result_source(source));
        }
        assert!(!is_file_result_source("app"));
    }

    #[test]
    fn windows_settings_commands_use_settings_icon() {
        assert!(is_windows_settings_command("ms-settings:display"));
        assert!(is_windows_settings_command("control.exe inetcpl.cpl"));
        assert!(is_windows_settings_command("services.msc"));
        assert!(!is_windows_settings_command(
            "C:\\Windows\\System32\\taskmgr.exe"
        ));
    }

    #[test]
    fn content_sources_use_content_match_layout() {
        for source in ["FILE_CONTENT", "CODE_CONTENT", "OCR", "PDF", "DOCX"] {
            assert!(is_content_match_source(source));
        }
        for source in ["FILE", "CODE", "Settings", "SYSTEM"] {
            assert!(!is_content_match_source(source));
        }
    }

    #[test]
    fn first_install_prompts_only_for_default_hotkey_conflict() {
        assert!(should_prompt_for_default_hotkey_conflict(
            true,
            "Alt+Space",
            false
        ));
        assert!(!should_prompt_for_default_hotkey_conflict(
            false,
            "Alt+Space",
            false
        ));
        assert!(!should_prompt_for_default_hotkey_conflict(
            true,
            "Ctrl+Alt+K",
            false
        ));
        assert!(!should_prompt_for_default_hotkey_conflict(
            true,
            "Alt+Space",
            true
        ));
    }

    #[test]
    fn task_manager_uses_its_system_executable_icon() {
        let path = task_manager_icon_path().expect("taskmgr.exe should exist on Windows");
        assert!(path
            .to_ascii_lowercase()
            .ends_with("\\system32\\taskmgr.exe"));
    }

    #[test]
    fn image_clipboard_dib_is_bottom_up_bgra() {
        let mut image = image::RgbaImage::new(1, 2);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 128]));
        image.put_pixel(0, 1, image::Rgba([0, 0, 255, 128]));
        let dib = image_to_dib_bytes(image::DynamicImage::ImageRgba8(image)).unwrap();
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 1);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(&dib[40..44], &[255, 0, 0, 255]);
        assert_eq!(&dib[44..48], &[0, 0, 255, 255]);
    }

    #[test]
    fn clipboard_selection_matches_pinned_and_unpinned_ids() {
        let mut selected = std::collections::HashSet::new();
        selected.insert("clip.1710000000123".to_string());

        assert!(selected_clip_ids_contain(&selected, "clip.1710000000123"));
        assert!(selected_clip_ids_contain(
            &selected,
            "clip.pinned.1710000000123"
        ));
        assert!(!selected_clip_ids_contain(&selected, "clip.1710000000456"));
        assert_eq!(
            selected_clip_timestamps(&selected, Some("clip.1")),
            vec![1710000000123]
        );
        assert_eq!(
            selected_clip_timestamps(&std::collections::HashSet::new(), Some("clip.pinned.42")),
            vec![42]
        );
    }

    #[test]
    fn clipboard_timestamp_normalizes_millis_for_age() {
        assert_eq!(clip_timestamp_to_unix_seconds(1_710_000_000), 1_710_000_000);
        assert_eq!(
            clip_timestamp_to_unix_seconds(1_710_000_000_123),
            1_710_000_000
        );
        assert_eq!(
            clip_id_for_pin_state(1_710_000_000_123, true),
            "clip.pinned.1710000000123"
        );
    }

    #[test]
    fn test_winrt_clipboard() {
        unsafe {
            let res = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
            if res.is_ok() {
                use windows::ApplicationModel::DataTransfer::Clipboard;
                if let Ok(enabled) = Clipboard::IsHistoryEnabled() {
                    println!("Clipboard history enabled status: {}", enabled);
                }
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }

    #[test]
    fn test_antigravity_icons() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED
                    | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
            );
            let h1 = get_app_icon("electron.app.Antigravity");
            let h2 = get_app_icon("Google.Antigravity");
            let h3 = get_app_icon("Google.AntigravityIDE");
            println!("electron.app.Antigravity HICON: {:?}", h1.0);
            println!("Google.Antigravity HICON: {:?}", h2.0);
            println!("Google.AntigravityIDE HICON: {:?}", h3.0);
            assert!(!h1.0.is_null(), "electron.app.Antigravity icon was null");
            assert!(!h2.0.is_null(), "Google.Antigravity icon was null");
            assert!(!h3.0.is_null(), "Google.AntigravityIDE icon was null");
        }
    }

    #[test]
    fn test_image_factory_cast() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED
                    | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
            );

            unsafe fn get_icon_via_image_factory(parsing_path: &str) -> Option<HICON> {
                use windows::core::{Interface, PCWSTR};
                use windows::Win32::Foundation::SIZE;
                use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
                use windows::Win32::UI::Shell::IShellItemImageFactory;
                use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
                use windows::Win32::UI::Shell::SIIGBF_ICONONLY;
                use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

                let path_wide: Vec<u16> = parsing_path
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let item: windows::Win32::UI::Shell::IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(path_wide.as_ptr()), None).ok()?;

                let factory: IShellItemImageFactory = item.cast().ok()?;
                let hbitmap = factory
                    .GetImage(SIZE { cx: 32, cy: 32 }, SIIGBF_ICONONLY)
                    .ok()?;

                let hbm_mask = CreateBitmap(32, 32, 1, 1, None);
                if hbm_mask.is_invalid() {
                    let _ = DeleteObject(hbitmap);
                    return None;
                }

                let mut ii = ICONINFO {
                    fIcon: windows::Win32::Foundation::TRUE,
                    xHotspot: 0,
                    yHotspot: 0,
                    hbmMask: hbm_mask,
                    hbmColor: hbitmap,
                };

                let hicon = CreateIconIndirect(&mut ii).ok();

                let _ = DeleteObject(hbitmap);
                let _ = DeleteObject(hbm_mask);

                hicon
            }

            for app_id in &[
                "electron.app.Antigravity",
                "Google.Antigravity",
                "Google.AntigravityIDE",
            ] {
                let parsing_path = format!("shell:AppsFolder\\{}", app_id);
                let hicon = get_icon_via_image_factory(&parsing_path);
                println!(
                    "App: {}, ImageFactory HICON: {:?}",
                    app_id,
                    hicon.map(|h| h.0)
                );
                if let Some(h) = hicon {
                    let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(h);
                }
            }
        }
    }

    #[test]
    fn clipboard_image_path_uses_clipboard_images_dir() {
        let db_path =
            std::path::PathBuf::from(r"C:\Users\Test\AppData\Roaming\protonsearch\app.db");
        let (_, path_str) = clipboard_image_path(&db_path, 123).unwrap();
        assert!(path_str.ends_with(r"protonsearch\clipboard_images\image_123.bmp"));
    }

    #[test]
    fn dib_clipboard_bytes_are_wrapped_as_bmp() {
        let mut dib = vec![0u8; 44];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&4u32.to_le_bytes());
        let bmp = dib_to_bmp_file_bytes(&dib).unwrap();
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        assert_eq!(bmp.len(), 58);
    }

    #[test]
    fn clipboard_bitmap_byte_len_rejects_invalid_or_huge_images() {
        assert_eq!(clipboard_bitmap_byte_len(10, 20), Some(800));
        assert_eq!(clipboard_bitmap_byte_len(0, 20), None);
        assert_eq!(clipboard_bitmap_byte_len(20, -1), None);
        assert_eq!(clipboard_bitmap_byte_len(7000, 20), None);
        assert_eq!(clipboard_bitmap_byte_len(6000, 6000), None);
    }

    #[test]
    fn clipboard_ocr_empty_message_mentions_text_not_missing_image() {
        assert!(clipboard_ocr_empty_message().contains("readable text"));
        assert!(!clipboard_ocr_empty_message().contains("copy a picture"));
    }

    #[test]
    fn run_clipboard_ocr_returns_job_result() {
        assert_eq!(run_clipboard_ocr(|| 42), 42);
    }

    #[test]
    fn search_cursor_only_belongs_to_search_input() {
        assert!(search_input_caret_active_flags(true, false, false, true));
        assert!(!search_input_caret_active_flags(false, false, false, true));
        assert!(!search_input_caret_active_flags(true, true, false, true));
        assert!(!search_input_caret_active_flags(true, false, true, true));
        assert!(!search_input_caret_active_flags(true, false, false, false));
    }
}

fn resolve_known_folder_path(path: &str) -> String {
    if path.starts_with('{') && path.contains('}') {
        if let Some(close_brace_idx) = path.find('}') {
            let guid_str = &path[0..=close_brace_idx];
            let guid_str_wide: Vec<u16> =
                guid_str.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                use windows::core::PCWSTR;
                use windows::Win32::Foundation::HANDLE;
                use windows::Win32::System::Com::CLSIDFromString;
                use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
                if let Ok(guid) = CLSIDFromString(PCWSTR(guid_str_wide.as_ptr())) {
                    if let Ok(result) =
                        SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, HANDLE::default())
                    {
                        let mut len = 0;
                        while *result.0.add(len) != 0 {
                            len += 1;
                        }
                        let base_path =
                            String::from_utf16_lossy(std::slice::from_raw_parts(result.0, len));
                        windows::Win32::System::Com::CoTaskMemFree(Some(result.0 as *const _));

                        let remaining = &path[close_brace_idx + 1..];
                        let remaining = remaining.trim_start_matches('\\');
                        return format!("{}\\{}", base_path, remaining);
                    }
                }
            }
        }
    }
    path.to_string()
}

fn get_app_path(launch_command: &str) -> String {
    let clean = if let Some(rest) = launch_command.strip_prefix("shell:AppsFolder\\") {
        rest
    } else {
        launch_command
    };
    resolve_known_folder_path(clean)
}

unsafe fn get_window_icon(hwnd: HWND) -> HICON {
    use windows::Win32::Foundation::WPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassLongPtrW, IsWindow, SendMessageTimeoutW, GCLP_HICON, GCLP_HICONSM, ICON_BIG,
        ICON_SMALL, SMTO_ABORTIFHUNG, SMTO_NORMAL, WM_GETICON,
    };

    // Validate window handle before attempting to get icon.
    // Stale handles from closed windows may return garbage or cause crashes.
    if !IsWindow(hwnd).as_bool() {
        return HICON(std::ptr::null_mut());
    }

    // This runs on the UI thread: a plain SendMessageW to a hung window would
    // freeze the launcher, so query with a short timeout and abort if hung.
    let query = |kind: u32| -> HICON {
        let mut result: usize = 0;
        let ok = SendMessageTimeoutW(
            hwnd,
            WM_GETICON,
            WPARAM(kind as usize),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_NORMAL,
            150,
            Some(&mut result),
        );
        if ok.0 != 0 {
            HICON(result as *mut std::ffi::c_void)
        } else {
            HICON(std::ptr::null_mut())
        }
    };

    let mut hicon = query(ICON_BIG);
    if hicon.0.is_null() {
        hicon = query(ICON_SMALL);
    }
    if hicon.0.is_null() {
        hicon = HICON(GetClassLongPtrW(hwnd, GCLP_HICON) as *mut std::ffi::c_void);
    }
    if hicon.0.is_null() {
        hicon = HICON(GetClassLongPtrW(hwnd, GCLP_HICONSM) as *mut std::ffi::c_void);
    }
    hicon
}

unsafe fn execute_submenu_action(hwnd: HWND, s: &mut State) {
    if let Some(r) = s.results.get(s.selected) {
        if r.entry.source == "app" {
            let launch_cmd = &r.entry.launch_command;
            let parsing_name = get_app_path(launch_cmd);

            let lnk_path = std::path::Path::new(&parsing_name);
            let resolved_path = if parsing_name.to_lowercase().ends_with(".lnk") {
                crate::search::resolve_lnk_path(lnk_path)
            } else {
                None
            };

            match s.submenu_selected {
                0 => {
                    // Run as Administrator
                    let run_cmd = if parsing_name.to_lowercase().ends_with(".lnk") {
                        resolved_path.unwrap_or_else(|| parsing_name.clone())
                    } else {
                        if parsing_name.contains('\\') {
                            parsing_name.clone()
                        } else {
                            format!("shell:AppsFolder\\{}", parsing_name)
                        }
                    };

                    let run_cmd_wide: Vec<u16> =
                        run_cmd.encode_utf16().chain(std::iter::once(0)).collect();
                    use windows::core::{w, PCWSTR};
                    use windows::Win32::UI::Shell::ShellExecuteW;
                    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
                    let _ = ShellExecuteW(
                        HWND::default(),
                        w!("runas"),
                        PCWSTR(run_cmd_wide.as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );

                    s.submenu_active = false;
                    do_hide(hwnd, s);
                }
                1 => {
                    // Open File Location
                    let select_path = if parsing_name.to_lowercase().ends_with(".lnk") {
                        if let Some(ref res) = resolved_path {
                            res.clone()
                        } else {
                            parsing_name.clone()
                        }
                    } else {
                        parsing_name.clone()
                    };

                    if std::path::Path::new(&select_path).exists() {
                        use std::os::windows::process::CommandExt;
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(format!(r#"/select,"{}""#, select_path))
                            .creation_flags(0x08000000) // CREATE_NO_WINDOW
                            .spawn();
                    }

                    s.submenu_active = false;
                    do_hide(hwnd, s);
                }
                2 => {
                    // Copy Path
                    let copy_val = if parsing_name.to_lowercase().ends_with(".lnk") {
                        resolved_path.unwrap_or_else(|| parsing_name.clone())
                    } else {
                        parsing_name.clone()
                    };

                    copy_to_clipboard(hwnd, &copy_val);
                    s.submenu_active = false;
                    do_hide(hwnd, s);
                }
                _ => {}
            }
        }
    }
}

unsafe fn handle_form_enter(hwnd: HWND, s: &mut State) {
    let input = s.query.trim().to_string();
    match &s.form_state {
        FormState::CreateSnippetName => {
            if !input.is_empty() {
                s.form_state = FormState::CreateSnippetContent { name: input };
                s.query.clear();
                s.cursor_pos = 0;
            }
        }
        FormState::CreateSnippetContent { name } => {
            if !input.is_empty() {
                s.form_state = FormState::CreateSnippetKeyword {
                    name: name.clone(),
                    content: input,
                };
                s.query.clear();
                s.cursor_pos = 0;
            }
        }
        FormState::CreateSnippetKeyword { name, content } => {
            let keyword = if input.is_empty() { None } else { Some(input) };
            let db_path = s.db_path.clone();
            let name = name.clone();
            let content = content.clone();
            std::thread::spawn(move || {
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                    let _ = conn.execute(
                        "INSERT OR REPLACE INTO snippets (name, content, keyword) VALUES (?, ?, ?);",
                        rusqlite::params![name, content, keyword],
                    );
                }
            });
            s.form_state = FormState::None;
            s.query.clear();
            s.cursor_pos = 0;
            trigger_search(hwnd, s);
        }
        FormState::CreateQuicklinkName => {
            if !input.is_empty() {
                s.form_state = FormState::CreateQuicklinkUrl { name: input };
                s.query.clear();
                s.cursor_pos = 0;
            }
        }
        FormState::CreateQuicklinkUrl { name } => {
            if !input.is_empty() {
                s.form_state = FormState::CreateQuicklinkKeyword {
                    name: name.clone(),
                    url: input,
                };
                s.query.clear();
                s.cursor_pos = 0;
            }
        }
        FormState::CreateQuicklinkKeyword { name, url } => {
            if !input.is_empty() {
                let db_path = s.db_path.clone();
                let name = name.clone();
                let url = url.clone();
                let keyword = input;
                std::thread::spawn(move || {
                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                        let _ = conn.execute(
                            "INSERT OR REPLACE INTO quicklinks (name, url, keyword) VALUES (?, ?, ?);",
                            rusqlite::params![name, url, keyword],
                        );
                    }
                });
                s.form_state = FormState::None;
                s.query.clear();
                s.cursor_pos = 0;
                trigger_search(hwnd, s);
            }
        }
        FormState::CreateFocusCategoryName => {
            if !input.is_empty() {
                s.form_state = FormState::CreateFocusCategoryBlocked { name: input };
                s.query.clear();
                s.cursor_pos = 0;
                trigger_search(hwnd, s);
            }
        }
        FormState::CreateFocusCategoryBlocked { name } => {
            if !input.is_empty() {
                let db_path = s.db_path.clone();
                let name = name.clone();
                let blocked = input;
                std::thread::spawn(move || {
                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                        let _ = conn.execute(
                            "INSERT OR REPLACE INTO focus_categories (name, blocked_apps) VALUES (?, ?);",
                            rusqlite::params![name, blocked],
                        );
                    }
                });
                s.query.clear();
                s.form_state = FormState::None;
                s.cursor_pos = 0;
                trigger_search(hwnd, s);
            }
        }
        FormState::CreateNoteName => {
            if !input.is_empty() {
                if let Ok(appdata) = std::env::var("APPDATA") {
                    let notes_dir = std::path::PathBuf::from(appdata)
                        .join("omnisearch-lite")
                        .join("notes");
                    let _ = std::fs::create_dir_all(&notes_dir);
                    let safe_name = input.replace(|c: char| !c.is_alphanumeric() && c != ' ', "_");
                    let note_path = notes_dir.join(format!("{}.txt", safe_name));
                    if !note_path.exists() {
                        let _ = std::fs::write(&note_path, "");
                    }
                    open_note_editor(hwnd, s, note_path.to_string_lossy().to_string());
                }
                s.query.clear();
                s.form_state = FormState::None;
                s.cursor_pos = 0;
                s.reset_results();
                // Don't trigger_search — note editor is now covering the results area
            }
        }
        FormState::None => {}
    }
    let _ = InvalidateRect(hwnd, None, FALSE);
}
// Self-rendered note editor: the text lives in State and is painted directly, so it
// works on the layered window (a child EDIT control rendered as a black box and stole
// focus, hiding the launcher). Type to append, Backspace/Enter edit, Esc saves & closes.
unsafe fn open_note_editor(hwnd: HWND, s: &mut State, path: String) {
    s.note_text = std::fs::read_to_string(&path).unwrap_or_default();
    s.note_path = Some(path);
    s.note_editing = true;
    s.note_scroll = 0;
    s.ai_pending = false;
    s.ai_answer = None;
    s.ai_title.clear();
    s.chat_input.clear();
    s.chat_cursor_pos = 0;
    s.chat_input_active = false;
    s.search_input_active = false;
    s.form_state = FormState::None;
    s.text_selected = false;
    s.reset_results();
    s.selected = 0;
    reset_cursor_blink(hwnd, s);
    let _ = InvalidateRect(hwnd, None, FALSE);
}

unsafe fn save_note(s: &State) {
    if let Some(path) = &s.note_path {
        let _ = std::fs::write(path, s.note_text.as_bytes());
    }
}

unsafe fn close_note_editor(hwnd: HWND, s: &mut State) {
    if s.note_editing {
        save_note(s);
        s.note_editing = false;
        s.note_text.clear();
        s.note_path = None;
        s.note_scroll = 0;
        s.search_input_active = true;
        trigger_search(hwnd, s);
        let _ = InvalidateRect(hwnd, None, FALSE);
    }
}

unsafe fn export_snippets(hwnd: HWND, s: &State) {
    if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
        let mut stmt = match conn.prepare("SELECT name, content, keyword FROM snippets") {
            Ok(st) => st,
            Err(_) => return,
        };
        let iter = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        }) {
            Ok(it) => it,
            Err(_) => return,
        };
        let mut list = Vec::new();
        for item in iter {
            if let Ok((name, content, keyword)) = item {
                list.push(serde_json::json!({
                    "name": name,
                    "content": content,
                    "keyword": keyword,
                }));
            }
        }
        let json_data = serde_json::to_string_pretty(&list).unwrap_or_default();
        if let Some(desktop) =
            launcher::get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Desktop)
        {
            let path = std::path::PathBuf::from(desktop).join("snippets_export.json");
            if std::fs::write(&path, json_data).is_ok() {
                copy_to_clipboard(hwnd, &path.to_string_lossy().to_string());
                let msg = format!(
                    "Snippets exported successfully to:\n{:?}\n\nPath copied to clipboard.",
                    path
                );
                let title = "Export Snippets\0".encode_utf16().collect::<Vec<u16>>();
                let msg_w = format!("{}\0", msg).encode_utf16().collect::<Vec<u16>>();
                windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    hwnd,
                    windows::core::PCWSTR(msg_w.as_ptr()),
                    windows::core::PCWSTR(title.as_ptr()),
                    windows::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                );
            }
        }
    }
}

unsafe fn import_snippets(hwnd: HWND, s: &State) {
    if let Some(desktop) =
        launcher::get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Desktop)
    {
        let path = std::path::PathBuf::from(desktop).join("snippets_import.json");
        if !path.exists() {
            let msg = format!("Import file not found!\n\nPlease place a file named 'snippets_import.json' on your Desktop and try again.\n\nTemplate format:\n[\n  {{\n    \"name\": \"example\",\n    \"content\": \"text\",\n    \"keyword\": \"optional\"\n  }}\n]");
            let title = "Import Snippets Error\0"
                .encode_utf16()
                .collect::<Vec<u16>>();
            let msg_w = format!("{}\0", msg).encode_utf16().collect::<Vec<u16>>();
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                hwnd,
                windows::core::PCWSTR(msg_w.as_ptr()),
                windows::core::PCWSTR(title.as_ptr()),
                windows::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
            );
            return;
        }

        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&data) {
                if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                    let mut count = 0;
                    for val in list {
                        let name = val.get("name").and_then(|v| v.as_str());
                        let content = val.get("content").and_then(|v| v.as_str());
                        let keyword = val.get("keyword").and_then(|v| v.as_str());
                        if let (Some(n), Some(c)) = (name, content) {
                            if conn.execute(
                                "INSERT OR REPLACE INTO snippets (name, content, keyword) VALUES (?, ?, ?);",
                                rusqlite::params![n, c, keyword],
                            ).is_ok() {
                                count += 1;
                            }
                        }
                    }
                    let msg = format!(
                        "Successfully imported {} snippets from snippets_import.json!",
                        count
                    );
                    let title = "Import Snippets\0".encode_utf16().collect::<Vec<u16>>();
                    let msg_w = format!("{}\0", msg).encode_utf16().collect::<Vec<u16>>();
                    windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                        hwnd,
                        windows::core::PCWSTR(msg_w.as_ptr()),
                        windows::core::PCWSTR(title.as_ptr()),
                        windows::Win32::UI::WindowsAndMessaging::MB_OK
                            | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                    );
                }
            }
        }
    }
}

unsafe fn export_quicklinks(hwnd: HWND, s: &State) {
    if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
        let mut stmt = match conn.prepare("SELECT name, url, keyword FROM quicklinks") {
            Ok(st) => st,
            Err(_) => return,
        };
        let iter = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            Ok(it) => it,
            Err(_) => return,
        };
        let mut list = Vec::new();
        for item in iter {
            if let Ok((name, url, keyword)) = item {
                list.push(serde_json::json!({
                    "name": name,
                    "url": url,
                    "keyword": keyword,
                }));
            }
        }
        let json_data = serde_json::to_string_pretty(&list).unwrap_or_default();
        if let Some(desktop) =
            launcher::get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Desktop)
        {
            let path = std::path::PathBuf::from(desktop).join("quicklinks_export.json");
            if std::fs::write(&path, json_data).is_ok() {
                copy_to_clipboard(hwnd, &path.to_string_lossy().to_string());
                let msg = format!(
                    "Quicklinks exported successfully to:\n{:?}\n\nPath copied to clipboard.",
                    path
                );
                let title = "Export Quicklinks\0".encode_utf16().collect::<Vec<u16>>();
                let msg_w = format!("{}\0", msg).encode_utf16().collect::<Vec<u16>>();
                windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    hwnd,
                    windows::core::PCWSTR(msg_w.as_ptr()),
                    windows::core::PCWSTR(title.as_ptr()),
                    windows::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                );
            }
        }
    }
}

unsafe fn import_quicklinks(hwnd: HWND, s: &State) {
    if let Some(desktop) =
        launcher::get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Desktop)
    {
        let path = std::path::PathBuf::from(desktop).join("quicklinks_import.json");
        if !path.exists() {
            let msg = format!("Import file not found!\n\nPlease place a file named 'quicklinks_import.json' on your Desktop and try again.\n\nTemplate format:\n[\n  {{\n    \"name\": \"example\",\n    \"url\": \"https://example.com/?q={{query}}\",\n    \"keyword\": \"ex\"\n  }}\n]");
            let title = "Import Quicklinks Error\0"
                .encode_utf16()
                .collect::<Vec<u16>>();
            let msg_w = format!("{}\0", msg).encode_utf16().collect::<Vec<u16>>();
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                hwnd,
                windows::core::PCWSTR(msg_w.as_ptr()),
                windows::core::PCWSTR(title.as_ptr()),
                windows::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
            );
            return;
        }

        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&data) {
                if let Ok(conn) = rusqlite::Connection::open(&s.db_path) {
                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
                    let mut count = 0;
                    for val in list {
                        let name = val.get("name").and_then(|v| v.as_str());
                        let url = val.get("url").and_then(|v| v.as_str());
                        let keyword = val.get("keyword").and_then(|v| v.as_str());
                        if let (Some(n), Some(u), Some(kw)) = (name, url, keyword) {
                            if conn.execute(
                                "INSERT OR REPLACE INTO quicklinks (name, url, keyword) VALUES (?, ?, ?);",
                                rusqlite::params![n, u, kw],
                            ).is_ok() {
                                count += 1;
                            }
                        }
                    }
                    let msg = format!(
                        "Successfully imported {} quicklinks from quicklinks_import.json!",
                        count
                    );
                    let title = "Import Quicklinks\0".encode_utf16().collect::<Vec<u16>>();
                    let msg_w = format!("{}\0", msg).encode_utf16().collect::<Vec<u16>>();
                    windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                        hwnd,
                        windows::core::PCWSTR(msg_w.as_ptr()),
                        windows::core::PCWSTR(title.as_ptr()),
                        windows::Win32::UI::WindowsAndMessaging::MB_OK
                            | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                    );
                }
            }
        }
    }
}
