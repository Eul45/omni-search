use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    io::ErrorKind,
    mem::size_of,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, LogicalSize, Manager, Runtime, WebviewWindow, Window, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
#[cfg(windows)]
use windows::Win32::{
    Foundation::COLORREF,
    Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR},
    System::{ProcessStatus::K32EmptyWorkingSet, Threading::GetCurrentProcess},
};

const MAIN_WINDOW_LABEL: &str = "main";
const TRAY_ICON_ID: &str = "omni-search-tray";
const TRAY_MENU_OPEN_QUICK_ID: &str = "open-quick-window";
const TRAY_MENU_OPEN_FULL_ID: &str = "open-full-window";
const TRAY_MENU_HIDE_ID: &str = "hide-window";
const TRAY_MENU_QUIT_ID: &str = "quit-app";
const DESKTOP_SETTINGS_FILE_NAME: &str = "desktop-settings.json";
const LEGACY_APP_SHORTCUT: &str = "Alt+Space";
const DEFAULT_APP_SHORTCUT: &str = "Alt+Shift+S";
const FULL_WINDOW_WIDTH: f64 = 800.0;
const FULL_WINDOW_HEIGHT: f64 = 700.0;
const FULL_WINDOW_MIN_WIDTH: f64 = 600.0;
const FULL_WINDOW_MIN_HEIGHT: f64 = 400.0;
const QUICK_WINDOW_WIDTH: f64 = 1140.0;
const QUICK_WINDOW_HEIGHT: f64 = 730.0;
const QUICK_WINDOW_MIN_WIDTH: f64 = 960.0;
const QUICK_WINDOW_MIN_HEIGHT: f64 = 620.0;
pub const WINDOW_MODE_EVENT: &str = "omni-search://window-mode";

#[derive(Clone, Copy)]
enum WindowMode {
    Full,
    Quick,
}

impl WindowMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Quick => "quick",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettings {
    pub background_mode_enabled: bool,
    pub shortcut_enabled: bool,
    pub shortcut: String,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            background_mode_enabled: true,
            shortcut_enabled: true,
            shortcut: DEFAULT_APP_SHORTCUT.to_string(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowModePayload {
    mode: &'static str,
}

pub struct DesktopRuntimeState {
    window_mode: Mutex<WindowMode>,
    settings: Mutex<DesktopSettings>,
    active_shortcut: Mutex<Option<String>>,
    is_quitting: AtomicBool,
}

impl Default for DesktopRuntimeState {
    fn default() -> Self {
        Self {
            window_mode: Mutex::new(WindowMode::Full),
            settings: Mutex::new(DesktopSettings::default()),
            active_shortcut: Mutex::new(None),
            is_quitting: AtomicBool::new(false),
        }
    }
}

fn desktop_state<R: Runtime>(app: &AppHandle<R>) -> &DesktopRuntimeState {
    app.state::<DesktopRuntimeState>().inner()
}

fn main_window<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    app.get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is not available".to_string())
}

fn emit_window_mode<R: Runtime>(app: &AppHandle<R>, mode: WindowMode) -> Result<(), String> {
    app.emit(
        WINDOW_MODE_EVENT,
        WindowModePayload {
            mode: mode.as_str(),
        },
    )
    .map_err(|err| err.to_string())
}

fn normalize_shortcut(shortcut: &str) -> Result<String, String> {
    let trimmed = shortcut.trim();
    if trimmed.is_empty() {
        return Err("Shortcut cannot be empty.".to_string());
    }

    let normalized_shortcut = trimmed
        .parse::<Shortcut>()
        .map(|shortcut| shortcut.to_string())
        .map_err(|err| format!("Invalid shortcut '{trimmed}': {err}"))?;

    if normalized_shortcut.eq_ignore_ascii_case(LEGACY_APP_SHORTCUT) {
        return Err(
            "Alt+Space conflicts with the Windows system menu. Choose another shortcut."
                .to_string(),
        );
    }

    Ok(normalized_shortcut)
}

fn sanitized_shortcut_or_default(shortcut: &str) -> String {
    let trimmed = shortcut.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(LEGACY_APP_SHORTCUT) {
        return DEFAULT_APP_SHORTCUT.to_string();
    }

    normalize_shortcut(trimmed).unwrap_or_else(|_| DEFAULT_APP_SHORTCUT.to_string())
}

fn sanitize_desktop_settings(settings: DesktopSettings) -> DesktopSettings {
    if !settings.background_mode_enabled
        && !settings.shortcut_enabled
        && settings
            .shortcut
            .trim()
            .eq_ignore_ascii_case(LEGACY_APP_SHORTCUT)
    {
        return DesktopSettings::default();
    }

    DesktopSettings {
        background_mode_enabled: settings.background_mode_enabled,
        shortcut_enabled: settings.shortcut_enabled,
        shortcut: sanitized_shortcut_or_default(&settings.shortcut),
    }
}

fn desktop_settings_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|err| err.to_string())?
        .join(DESKTOP_SETTINGS_FILE_NAME))
}

fn load_desktop_settings<R: Runtime>(app: &AppHandle<R>) -> DesktopSettings {
    let path = match desktop_settings_path(app) {
        Ok(path) => path,
        Err(err) => {
            log_desktop_error("resolve the desktop settings file", &err);
            return DesktopSettings::default();
        }
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<DesktopSettings>(&contents) {
            Ok(settings) => sanitize_desktop_settings(settings),
            Err(err) => {
                log_desktop_error("parse the desktop settings file", &err.to_string());
                DesktopSettings::default()
            }
        },
        Err(err) if err.kind() == ErrorKind::NotFound => DesktopSettings::default(),
        Err(err) => {
            log_desktop_error("read the desktop settings file", &err.to_string());
            DesktopSettings::default()
        }
    }
}

fn save_desktop_settings_file<R: Runtime>(
    app: &AppHandle<R>,
    settings: &DesktopSettings,
) -> Result<(), String> {
    let path = desktop_settings_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }

    let payload = serde_json::to_vec_pretty(settings).map_err(|err| err.to_string())?;
    std::fs::write(path, payload).map_err(|err| err.to_string())
}

fn current_desktop_settings<R: Runtime>(app: &AppHandle<R>) -> Result<DesktopSettings, String> {
    desktop_state(app)
        .settings
        .lock()
        .map_err(|_| "Failed to lock desktop settings".to_string())
        .map(|settings| settings.clone())
}

fn set_desktop_settings<R: Runtime>(
    app: &AppHandle<R>,
    settings: &DesktopSettings,
) -> Result<(), String> {
    *desktop_state(app)
        .settings
        .lock()
        .map_err(|_| "Failed to lock desktop settings".to_string())? = settings.clone();
    Ok(())
}

fn is_background_mode_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    desktop_state(app)
        .settings
        .lock()
        .map(|settings| settings.background_mode_enabled)
        .unwrap_or(false)
}

fn current_window_mode<R: Runtime>(app: &AppHandle<R>) -> WindowMode {
    desktop_state(app)
        .window_mode
        .lock()
        .map(|mode| *mode)
        .unwrap_or(WindowMode::Full)
}

fn set_window_mode<R: Runtime>(app: &AppHandle<R>, mode: WindowMode) -> Result<(), String> {
    *desktop_state(app)
        .window_mode
        .lock()
        .map_err(|_| "Failed to lock the window mode".to_string())? = mode;
    Ok(())
}

fn apply_main_window_layout<R: Runtime>(
    window: &WebviewWindow<R>,
    mode: WindowMode,
) -> Result<(), String> {
    let (width, height, min_width, min_height) = match mode {
        WindowMode::Full => (
            FULL_WINDOW_WIDTH,
            FULL_WINDOW_HEIGHT,
            FULL_WINDOW_MIN_WIDTH,
            FULL_WINDOW_MIN_HEIGHT,
        ),
        WindowMode::Quick => (
            QUICK_WINDOW_WIDTH,
            QUICK_WINDOW_HEIGHT,
            QUICK_WINDOW_MIN_WIDTH,
            QUICK_WINDOW_MIN_HEIGHT,
        ),
    };

    window
        .set_min_size(Some(LogicalSize::new(min_width, min_height)))
        .map_err(|err| err.to_string())?;
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|err| err.to_string())?;
    window.center().map_err(|err| err.to_string())
}

fn show_window_in_mode<R: Runtime>(app: &AppHandle<R>, mode: WindowMode) -> Result<(), String> {
    let window = main_window(app)?;
    let is_visible = window.is_visible().map_err(|err| err.to_string())?;
    let was_minimized = window.is_minimized().map_err(|err| err.to_string())?;

    set_window_mode(app, mode)?;

    if is_visible && !was_minimized {
        window.hide().map_err(|err| err.to_string())?;
    }

    if was_minimized {
        window.unminimize().map_err(|err| err.to_string())?;
    }

    emit_window_mode(app, mode)?;
    apply_main_window_layout(&window, mode)?;

    window
        .set_always_on_top(false)
        .map_err(|err| err.to_string())?;
    window.show().map_err(|err| err.to_string())?;

    if was_minimized {
        apply_main_window_layout(&window, mode)?;
    }

    window.set_focus().map_err(|err| err.to_string())
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = main_window(app)?;
    apply_main_window_layout(&window, current_window_mode(app))?;

    if window.is_minimized().map_err(|err| err.to_string())? {
        window.unminimize().map_err(|err| err.to_string())?;
    }

    window
        .set_always_on_top(false)
        .map_err(|err| err.to_string())?;
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())
}

fn hide_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = main_window(app)?;
    window.hide().map_err(|err| err.to_string())?;
    trim_process_working_set();
    Ok(())
}

#[cfg(windows)]
fn trim_process_working_set() {
    unsafe {
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}

#[cfg(not(windows))]
fn trim_process_working_set() {}

fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = main_window(app)?;
    let is_visible = window.is_visible().map_err(|err| err.to_string())?;
    let is_minimized = window.is_minimized().map_err(|err| err.to_string())?;

    if is_visible && !is_minimized {
        hide_main_window(app)
    } else {
        show_main_window(app)
    }
}

fn open_full_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    show_window_in_mode(app, WindowMode::Full)
}

fn open_quick_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    show_window_in_mode(app, WindowMode::Quick)
}

fn toggle_quick_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = main_window(app)?;
    let is_visible = window.is_visible().map_err(|err| err.to_string())?;
    let is_minimized = window.is_minimized().map_err(|err| err.to_string())?;

    if is_visible && !is_minimized && matches!(current_window_mode(app), WindowMode::Quick) {
        hide_main_window(app)
    } else {
        open_quick_window(app)
    }
}

fn apply_native_window_theme<R: Runtime>(
    app: &AppHandle<R>,
    theme_mode: &str,
    background_color: Option<&str>,
    title_bar_color: Option<&str>,
    title_bar_text_color: Option<&str>,
) -> Result<(), String> {
    let window = main_window(app)?;
    let native_theme = match theme_mode.trim().to_ascii_lowercase().as_str() {
        "dark" => tauri::Theme::Dark,
        "light" => tauri::Theme::Light,
        other => return Err(format!("Unsupported theme mode '{other}'.")),
    };

    window
        .set_theme(Some(native_theme))
        .map_err(|err| err.to_string())?;

    if let Some(color_value) = background_color.map(str::trim).filter(|value| !value.is_empty()) {
        let color = color_value
            .parse::<tauri::window::Color>()
            .map_err(|err| format!("Invalid background color '{color_value}': {err}"))?;
        window
            .set_background_color(Some(color))
            .map_err(|err| err.to_string())?;
    }

    apply_native_title_bar_colors(&window, title_bar_color, title_bar_text_color);

    Ok(())
}

#[cfg(windows)]
fn apply_native_title_bar_colors<R: Runtime>(
    window: &WebviewWindow<R>,
    title_bar_color: Option<&str>,
    title_bar_text_color: Option<&str>,
) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };

    if let Some(caption_color) = title_bar_color.and_then(parse_css_colorref) {
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR,
                &caption_color as *const COLORREF as *const c_void,
                size_of::<COLORREF>() as u32,
            );
        }
    }

    if let Some(text_color) = title_bar_text_color.and_then(parse_css_colorref) {
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR,
                &text_color as *const COLORREF as *const c_void,
                size_of::<COLORREF>() as u32,
            );
        }
    }
}

#[cfg(not(windows))]
fn apply_native_title_bar_colors<R: Runtime>(
    _window: &WebviewWindow<R>,
    _title_bar_color: Option<&str>,
    _title_bar_text_color: Option<&str>,
) {
}

#[cfg(windows)]
fn parse_css_colorref(value: &str) -> Option<COLORREF> {
    let tauri::window::Color(red, green, blue, _) = value.trim().parse().ok()?;
    Some(COLORREF(
        u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16),
    ))
}

fn unregister_active_shortcut<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = desktop_state(app);
    let mut active_shortcut = state
        .active_shortcut
        .lock()
        .map_err(|_| "Failed to lock the active shortcut".to_string())?;

    if let Some(previous_shortcut) = active_shortcut.clone() {
        if let Ok(previous_parsed_shortcut) = previous_shortcut.parse::<Shortcut>() {
            if app
                .global_shortcut()
                .is_registered(previous_parsed_shortcut.clone())
            {
                app.global_shortcut()
                    .unregister(previous_parsed_shortcut)
                    .map_err(|err| err.to_string())?;
            }
        }
    }

    *active_shortcut = None;
    Ok(())
}

fn register_app_shortcut<R: Runtime>(
    app: &AppHandle<R>,
    settings: &DesktopSettings,
) -> Result<String, String> {
    let normalized_shortcut = normalize_shortcut(&settings.shortcut)?;

    if !settings.shortcut_enabled {
        unregister_active_shortcut(app)?;
        return Ok(normalized_shortcut);
    }

    let parsed_shortcut = normalized_shortcut
        .parse::<Shortcut>()
        .map_err(|err| err.to_string())?;
    let state = desktop_state(app);
    let mut active_shortcut = state
        .active_shortcut
        .lock()
        .map_err(|_| "Failed to lock the active shortcut".to_string())?;

    if active_shortcut
        .as_deref()
        .map(|registered| registered.eq_ignore_ascii_case(&normalized_shortcut))
        .unwrap_or(false)
        && app.global_shortcut().is_registered(parsed_shortcut.clone())
    {
        return Ok(normalized_shortcut);
    }

    if let Some(previous_shortcut) = active_shortcut.clone() {
        if !previous_shortcut.eq_ignore_ascii_case(&normalized_shortcut) {
            if let Ok(previous_parsed_shortcut) = previous_shortcut.parse::<Shortcut>() {
                if app
                    .global_shortcut()
                    .is_registered(previous_parsed_shortcut.clone())
                {
                    app.global_shortcut()
                        .unregister(previous_parsed_shortcut)
                        .map_err(|err| err.to_string())?;
                }
            }
        }
    }

    if !app.global_shortcut().is_registered(parsed_shortcut.clone()) {
        let shortcut_for_logs = normalized_shortcut.clone();
        app.global_shortcut()
            .on_shortcut(parsed_shortcut, move |app, _, event| {
                if event.state == ShortcutState::Pressed {
                    if let Err(err) = toggle_quick_window(app) {
                        log_desktop_error(
                            &format!("toggle the quick window with {shortcut_for_logs}"),
                            &err,
                        );
                    }
                }
            })
            .map_err(|err| err.to_string())?;
    }

    *active_shortcut = Some(normalized_shortcut.clone());
    Ok(normalized_shortcut)
}

fn quit_background_app<R: Runtime>(app: &AppHandle<R>) {
    desktop_state(app).is_quitting.store(true, Ordering::SeqCst);
    app.exit(0);
}

fn log_desktop_error(action: &str, err: &str) {
    eprintln!("OmniSearch desktop action failed while trying to {action}: {err}");
}

fn setup_system_tray<R: Runtime>(app: &mut App<R>) -> tauri::Result<()> {
    let open_quick_item = MenuItem::with_id(
        app,
        TRAY_MENU_OPEN_QUICK_ID,
        "Open Quick Window",
        true,
        None::<&str>,
    )?;
    let open_full_item = MenuItem::with_id(
        app,
        TRAY_MENU_OPEN_FULL_ID,
        "Open Main App",
        true,
        None::<&str>,
    )?;
    let hide_item = MenuItem::with_id(app, TRAY_MENU_HIDE_ID, "Hide", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT_ID, "Quit", true, None::<&str>)?;
    let tray_menu = Menu::with_items(
        app,
        &[&open_quick_item, &open_full_item, &hide_item, &quit_item],
    )?;

    let mut tray_builder = TrayIconBuilder::with_id(TRAY_ICON_ID)
        .menu(&tray_menu)
        .tooltip("OmniSearch")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_MENU_OPEN_QUICK_ID => {
                if let Err(err) = open_quick_window(app) {
                    log_desktop_error("open the quick window from the tray", &err);
                }
            }
            TRAY_MENU_OPEN_FULL_ID => {
                if let Err(err) = open_full_window(app) {
                    log_desktop_error("open the app from the tray", &err);
                }
            }
            TRAY_MENU_HIDE_ID => {
                if let Err(err) = hide_main_window(app) {
                    log_desktop_error("hide the app from the tray", &err);
                }
            }
            TRAY_MENU_QUIT_ID => quit_background_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Err(err) = toggle_main_window(app) {
                    log_desktop_error("toggle the app from the tray icon", &err);
                }
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }

    let _tray = tray_builder.build(app)?;
    Ok(())
}

#[tauri::command]
pub fn get_desktop_settings(app: AppHandle<tauri::Wry>) -> Result<DesktopSettings, String> {
    current_desktop_settings(&app)
}

#[tauri::command]
pub fn open_full_window_command(app: AppHandle<tauri::Wry>) -> Result<(), String> {
    open_full_window(&app)
}

#[tauri::command]
pub fn open_quick_window_command(app: AppHandle<tauri::Wry>) -> Result<(), String> {
    open_quick_window(&app)
}

#[tauri::command]
pub fn sync_window_theme_command(
    app: AppHandle<tauri::Wry>,
    theme_mode: Option<String>,
    #[allow(non_snake_case)] themeMode: Option<String>,
    background_color: Option<String>,
    #[allow(non_snake_case)] backgroundColor: Option<String>,
    title_bar_color: Option<String>,
    #[allow(non_snake_case)] titleBarColor: Option<String>,
    title_bar_text_color: Option<String>,
    #[allow(non_snake_case)] titleBarTextColor: Option<String>,
) -> Result<(), String> {
    let resolved_mode = theme_mode
        .or(themeMode)
        .unwrap_or_else(|| "dark".to_string());
    let resolved_background_color = background_color.or(backgroundColor);
    let resolved_title_bar_color = title_bar_color.or(titleBarColor);
    let resolved_title_bar_text_color = title_bar_text_color.or(titleBarTextColor);

    apply_native_window_theme(
        &app,
        &resolved_mode,
        resolved_background_color.as_deref(),
        resolved_title_bar_color.as_deref(),
        resolved_title_bar_text_color.as_deref(),
    )
}

#[tauri::command]
pub fn update_desktop_settings(
    app: AppHandle<tauri::Wry>,
    background_mode_enabled: Option<bool>,
    #[allow(non_snake_case)] backgroundModeEnabled: Option<bool>,
    shortcut_enabled: Option<bool>,
    #[allow(non_snake_case)] shortcutEnabled: Option<bool>,
    shortcut: String,
) -> Result<DesktopSettings, String> {
    let defaults = DesktopSettings::default();
    let settings = DesktopSettings {
        background_mode_enabled: background_mode_enabled
            .or(backgroundModeEnabled)
            .unwrap_or(defaults.background_mode_enabled),
        shortcut_enabled: shortcut_enabled
            .or(shortcutEnabled)
            .unwrap_or(defaults.shortcut_enabled),
        shortcut,
    };

    let normalized_shortcut = register_app_shortcut(&app, &settings)?;
    let next_settings = DesktopSettings {
        shortcut: normalized_shortcut,
        ..settings
    };

    set_desktop_settings(&app, &next_settings)?;
    save_desktop_settings_file(&app, &next_settings)?;
    Ok(next_settings)
}

pub fn setup<R: Runtime>(app: &mut App<R>) -> tauri::Result<()> {
    setup_system_tray(app)?;

    let loaded_settings = load_desktop_settings(app.handle());
    let resolved_shortcut = match register_app_shortcut(app.handle(), &loaded_settings) {
        Ok(shortcut) => shortcut,
        Err(err) => {
            log_desktop_error("register the global shortcut", &err);
            loaded_settings.shortcut.clone()
        }
    };
    let resolved_settings = DesktopSettings {
        shortcut: resolved_shortcut,
        ..loaded_settings
    };

    if let Err(err) = set_desktop_settings(app.handle(), &resolved_settings) {
        log_desktop_error("sync desktop settings in memory", &err);
    }
    if let Err(err) = set_window_mode(app.handle(), WindowMode::Full) {
        log_desktop_error("sync the initial window mode", &err);
    }
    if let Err(err) = save_desktop_settings_file(app.handle(), &resolved_settings) {
        log_desktop_error("persist desktop settings", &err);
    }
    if let Err(err) = emit_window_mode(app.handle(), WindowMode::Full) {
        log_desktop_error("emit the initial window mode", &err);
    }

    Ok(())
}

pub fn focus_existing_instance<R: Runtime>(app: &AppHandle<R>) {
    if let Err(err) = open_full_window(app) {
        log_desktop_error("focus the existing app instance", &err);
    }
}

pub fn handle_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    if let WindowEvent::CloseRequested { api, .. } = event {
        let app = window.app_handle();
        if !desktop_state(&app).is_quitting.load(Ordering::SeqCst)
            && is_background_mode_enabled(&app)
        {
            api.prevent_close();
            if let Err(err) = hide_main_window(&app) {
                log_desktop_error("hide the app instead of closing", &err);
            }
        }
    }
}

pub fn desktop_state_for_builder() -> DesktopRuntimeState {
    DesktopRuntimeState::default()
}
