use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

fn settings_mutex() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_true")]
    pub run_on_startup: bool,

    #[serde(default = "default_true")]
    pub hide_on_lose_focus: bool,

    #[serde(default = "default_theme")]
    pub theme_mode: String,

    #[serde(default = "default_global_hotkey")]
    pub global_hotkey: String,

    #[serde(default = "default_window_width")]
    pub window_width: u32,

    #[serde(default = "default_item_height")]
    pub item_height: u32,

    #[serde(default = "default_false")]
    pub show_taskbar: bool,

    #[serde(default = "default_window_location")]
    pub window_location: String,

    #[serde(default = "default_zero_i32")]
    pub last_win_x: i32,

    #[serde(default = "default_zero_i32")]
    pub last_win_y: i32,

    #[serde(default = "default_scan_folders")]
    pub scan_folders: Vec<String>,

    #[serde(default = "default_search_bar_height")]
    pub search_bar_height: u32,

    #[serde(default = "default_query_font_family")]
    pub query_font_family: String,

    #[serde(default = "default_query_font_weight")]
    pub query_font_weight: String,

    #[serde(default = "default_query_font_size")]
    pub query_font_size: u32,

    #[serde(default = "default_result_title_font_family")]
    pub result_title_font_family: String,

    #[serde(default = "default_result_title_font_weight")]
    pub result_title_font_weight: String,

    #[serde(default = "default_result_title_font_size")]
    pub result_title_font_size: u32,

    #[serde(default = "default_result_subtitle_font_family")]
    pub result_subtitle_font_family: String,

    #[serde(default = "default_result_subtitle_font_weight")]
    pub result_subtitle_font_weight: String,

    #[serde(default = "default_result_subtitle_font_size")]
    pub result_subtitle_font_size: u32,

    #[serde(default = "default_true")]
    pub show_placeholder: bool,

    #[serde(default = "default_true")]
    pub plugin_circle_search: bool,

    #[serde(default = "default_true")]
    pub plugin_text_expansions: bool,

    #[serde(default = "default_true")]
    pub plugin_color_picker: bool,

    #[serde(default = "default_true")]
    pub plugin_calculator: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            run_on_startup: default_true(),
            hide_on_lose_focus: default_true(),
            theme_mode: default_theme(),
            global_hotkey: default_global_hotkey(),
            window_width: default_window_width(),
            item_height: default_item_height(),
            show_taskbar: default_false(),
            window_location: default_window_location(),
            last_win_x: default_zero_i32(),
            last_win_y: default_zero_i32(),
            scan_folders: default_scan_folders(),
            search_bar_height: default_search_bar_height(),
            query_font_family: default_query_font_family(),
            query_font_weight: default_query_font_weight(),
            query_font_size: default_query_font_size(),
            result_title_font_family: default_result_title_font_family(),
            result_title_font_weight: default_result_title_font_weight(),
            result_title_font_size: default_result_title_font_size(),
            result_subtitle_font_family: default_result_subtitle_font_family(),
            result_subtitle_font_weight: default_result_subtitle_font_weight(),
            result_subtitle_font_size: default_result_subtitle_font_size(),
            show_placeholder: default_true(),
            plugin_circle_search: default_true(),
            plugin_text_expansions: default_true(),
            plugin_color_picker: default_true(),
            plugin_calculator: default_true(),
        }
    }
}

fn default_false() -> bool {
    false
}
fn default_window_location() -> String {
    "Monitor with Mouse Cursor".to_string()
}
fn default_zero_i32() -> i32 {
    0
}

fn default_true() -> bool {
    true
}
fn default_theme() -> String {
    "Dark".to_string()
}
fn default_global_hotkey() -> String {
    "Alt+Space".to_string()
}
fn default_window_width() -> u32 {
    720
}
fn default_item_height() -> u32 {
    76
}
fn default_scan_folders() -> Vec<String> {
    Vec::new()
}
fn default_search_bar_height() -> u32 {
    60
}
fn default_query_font_family() -> String {
    "Segoe UI Variable".to_string()
}
fn default_query_font_weight() -> String {
    "Regular".to_string()
}
fn default_query_font_size() -> u32 {
    20
}
fn default_result_title_font_family() -> String {
    "Segoe UI Variable".to_string()
}
fn default_result_title_font_weight() -> String {
    "Bold".to_string()
}
fn default_result_title_font_size() -> u32 {
    18
}
fn default_result_subtitle_font_family() -> String {
    "Segoe UI Variable".to_string()
}
fn default_result_subtitle_font_weight() -> String {
    "Regular".to_string()
}
fn default_result_subtitle_font_size() -> u32 {
    13
}

fn exe_dir_settings_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("settings.json"))
}

fn appdata_settings_path() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(
        PathBuf::from(appdata)
            .join("omnisearch-lite")
            .join("settings.json"),
    )
}

impl AppSettings {
    /// Prefer whichever settings file already exists (exe dir first for the
    /// default/portable install), falling back to the exe dir for new files.
    /// The exe dir isn't writable under Program Files — save() handles that.
    pub fn get_settings_path() -> PathBuf {
        if let Some(p) = exe_dir_settings_path() {
            if p.exists() {
                return p;
            }
        }
        if let Some(p) = appdata_settings_path() {
            if p.exists() {
                return p;
            }
        }
        exe_dir_settings_path()
            .or_else(appdata_settings_path)
            .unwrap_or_else(|| PathBuf::from("settings.json"))
    }

    pub fn load() -> Self {
        // Scope the read lock so it is released before the fallback save() below —
        // save() locks the same (non-reentrant) mutex and would otherwise deadlock
        // on first run when no settings file exists yet.
        {
            let _guard = settings_mutex().lock().unwrap();
            let path = Self::get_settings_path();
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    match serde_json::from_str(&content) {
                        Ok(settings) => return settings,
                        Err(_) => {
                            // Keep the corrupt file for inspection instead of silently
                            // overwriting the user's settings with defaults.
                            let _ = fs::rename(&path, path.with_extension("json.bak"));
                        }
                    }
                }
            }
        }
        let default_settings = Self::default();
        default_settings.save();
        default_settings
    }

    pub fn save(&self) {
        // Serialize file I/O across threads (settings window + launcher).
        let _guard = settings_mutex().lock().unwrap();
        let Ok(content) = serde_json::to_string_pretty(self) else {
            return;
        };
        let path = Self::get_settings_path();
        if fs::write(&path, &content).is_ok() {
            return;
        }
        // Exe dir not writable (e.g. Program Files install) — fall back to %APPDATA%.
        if let Some(fallback) = appdata_settings_path() {
            if let Some(dir) = fallback.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(fallback, &content);
        }
    }

    pub fn normalized_theme_mode(&self) -> String {
        match self.theme_mode.as_str() {
            "Light" => "Light",
            "NordDarker" => "NordDarker",
            "Darker" => "Darker",
            "Omni" => "Omni",
            _ => "Omni",
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;


}
