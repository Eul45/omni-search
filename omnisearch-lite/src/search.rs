use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

pub static DISABLE_LIVE_RESULTS: AtomicBool = AtomicBool::new(false);

const CATALOG: &[u8] = include_bytes!("../assets/catalog.bin");

pub fn ensure_memory_events_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memory_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            source TEXT NOT NULL,
            event_type TEXT NOT NULL,
            title TEXT NOT NULL,
            detail TEXT,
            app_name TEXT,
            path TEXT,
            url TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_memory_events_timestamp
            ON memory_events(timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_memory_events_source_type
            ON memory_events(source, event_type);
        CREATE VIRTUAL TABLE IF NOT EXISTS memory_events_fts USING fts5(
            title,
            detail,
            source,
            event_type,
            app_name,
            path,
            url,
            content='memory_events',
            content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS memory_events_ai AFTER INSERT ON memory_events BEGIN
            INSERT INTO memory_events_fts(rowid, title, detail, source, event_type, app_name, path, url)
            VALUES (new.id, new.title, coalesce(new.detail, ''), new.source, new.event_type, coalesce(new.app_name, ''), coalesce(new.path, ''), coalesce(new.url, ''));
        END;
        CREATE TRIGGER IF NOT EXISTS memory_events_ad AFTER DELETE ON memory_events BEGIN
            INSERT INTO memory_events_fts(memory_events_fts, rowid, title, detail, source, event_type, app_name, path, url)
            VALUES ('delete', old.id, old.title, coalesce(old.detail, ''), old.source, old.event_type, coalesce(old.app_name, ''), coalesce(old.path, ''), coalesce(old.url, ''));
        END;
        CREATE TRIGGER IF NOT EXISTS memory_events_au AFTER UPDATE ON memory_events BEGIN
            INSERT INTO memory_events_fts(memory_events_fts, rowid, title, detail, source, event_type, app_name, path, url)
            VALUES ('delete', old.id, old.title, coalesce(old.detail, ''), old.source, old.event_type, coalesce(old.app_name, ''), coalesce(old.path, ''), coalesce(old.url, ''));
            INSERT INTO memory_events_fts(rowid, title, detail, source, event_type, app_name, path, url)
            VALUES (new.id, new.title, coalesce(new.detail, ''), new.source, new.event_type, coalesce(new.app_name, ''), coalesce(new.path, ''), coalesce(new.url, ''));
        END;
        ",
    )
}

pub fn insert_memory_event(
    conn: &Connection,
    timestamp: i64,
    source: &str,
    event_type: &str,
    title: &str,
    detail: &str,
    app_name: &str,
    path: Option<&str>,
    url: Option<&str>,
) {
    if title.trim().is_empty() {
        return;
    }
    if ensure_memory_events_schema(conn).is_err() {
        return;
    }
    let timestamp = normalize_event_timestamp(timestamp);
    let exists = conn
        .query_row(
            "SELECT COUNT(*) FROM memory_events
             WHERE timestamp = ?
               AND source = ?
               AND event_type = ?
               AND title = ?
               AND coalesce(path, '') = coalesce(?, '')
               AND coalesce(url, '') = coalesce(?, '')",
            params![timestamp, source, event_type, title, path, url],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    if exists > 0 {
        return;
    }
    let _ = conn.execute(
        "INSERT INTO memory_events
         (timestamp, source, event_type, title, detail, app_name, path, url)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
        params![timestamp, source, event_type, title, detail, app_name, path, url],
    );
    // Trim to the 50K newest rows, but not on every insert — the subquery scans the
    // whole table and the indexers call this thousands of times per rescan cycle.
    static INSERTS_SINCE_TRIM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if INSERTS_SINCE_TRIM.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 128 == 0 {
        let _ = conn.execute(
            "DELETE FROM memory_events
             WHERE id NOT IN (SELECT id FROM memory_events ORDER BY timestamp DESC LIMIT 50000);",
            [],
        );
    }
}

fn ensure_settings_catalog_fts(conn: &Connection, meta: &[CatalogEntry]) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS settings_catalog_fts USING fts5(
            name,
            breadcrumb,
            description,
            synonyms,
            launch_command UNINDEXED,
            source UNINDEXED
        );
        DELETE FROM settings_catalog_fts;
        ",
    )?;

    let mut seen = std::collections::HashSet::new();
    let mut insert_entry = |entry: &CatalogEntry| -> rusqlite::Result<()> {
        if !is_native_settings_command(&entry.launch_command) {
            return Ok(());
        }
        if !seen.insert(entry.launch_command.clone()) {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO settings_catalog_fts
             (name, breadcrumb, description, synonyms, launch_command, source)
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                entry.control_name,
                entry.breadcrumb_path,
                entry.description,
                entry.synonyms,
                entry.launch_command,
                native_settings_source(&entry.launch_command),
            ],
        )?;
        Ok(())
    };

    for entry in meta {
        insert_entry(entry)?;
    }

    for action in QUICK_ACTIONS {
        if !is_native_settings_command(action.launch_command) {
            continue;
        }
        let entry = CatalogEntry {
            id: format!(
                "settings.quick.{}",
                action.name.to_lowercase().replace(' ', "_")
            ),
            control_name: action.name.to_string(),
            breadcrumb_path: action.breadcrumb.to_string(),
            launch_command: action.launch_command.to_string(),
            source: native_settings_source(action.launch_command).to_string(),
            description: action.description.to_string(),
            synonyms: action.triggers.join("|"),
        };
        insert_entry(&entry)?;
    }

    Ok(())
}

#[derive(Clone)]
pub struct CatalogEntry {
    pub id: String,
    pub control_name: String,
    pub breadcrumb_path: String,
    pub launch_command: String,
    pub source: String,
    pub description: String,
    pub synonyms: String,
}

#[derive(Clone)]
pub struct SearchResult {
    pub entry: CatalogEntry,
    pub score: f32,
}

const MIN_SEARCH_CANDIDATES: usize = 2_000;
const MAX_SEARCH_CANDIDATES: usize = 5_000;

fn candidate_limit(requested: usize) -> usize {
    requested
        .max(MIN_SEARCH_CANDIDATES)
        .min(MAX_SEARCH_CANDIDATES)
}

fn content_match_source(extension: &str, only_code: bool) -> &'static str {
    if matches!(extension, "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp") {
        "OCR"
    } else if only_code
        || matches!(
            extension,
            "rs" | "py"
                | "js"
                | "ts"
                | "jsx"
                | "tsx"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "dart"
                | "rb"
                | "php"
        )
    {
        "CODE_CONTENT"
    } else {
        "FILE_CONTENT"
    }
}

fn normalize_ocr_search_text(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

fn ocr_text_matches_query(ocr_text: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let ocr_lower = ocr_text.to_lowercase();
    if ocr_lower.contains(&q) {
        return true;
    }

    let normalized_ocr = normalize_ocr_search_text(ocr_text);
    let normalized_q = normalize_ocr_search_text(&q);
    if normalized_q.len() >= 2 && normalized_ocr.contains(&normalized_q) {
        return true;
    }

    let words = q
        .split_whitespace()
        .map(normalize_ocr_search_text)
        .filter(|word| word.len() >= 2)
        .collect::<Vec<_>>();
    !words.is_empty() && words.iter().all(|word| normalized_ocr.contains(word))
}

fn image_path_dedupe_key(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp"
    ) {
        return None;
    }
    Some(format!(
        "image:{}",
        path.trim().replace('/', "\\").to_ascii_lowercase()
    ))
}

fn result_launch_dedupe_key(entry: &CatalogEntry) -> Option<String> {
    if entry.launch_command.is_empty() {
        return None;
    }
    if let Some(path) = entry.launch_command.strip_prefix("copy_image:") {
        return image_path_dedupe_key(path);
    }
    image_path_dedupe_key(&entry.launch_command)
        .or_else(|| Some(format!("cmd:{}", entry.launch_command)))
}

fn is_ocr_filter_visible_result(result: &SearchResult) -> bool {
    result.entry.source == "OCR"
        || (result.entry.source == "CLIPBOARD"
            && result.entry.launch_command.starts_with("copy_image:")
            && result.entry.description.starts_with("🔤 "))
}

fn search_result_identity(result: &SearchResult) -> String {
    result_launch_dedupe_key(&result.entry).unwrap_or_else(|| format!("id:{}", result.entry.id))
}

fn truncate_preserving_ocr_results(results: &mut Vec<SearchResult>, top_k: usize) {
    if top_k == 0 {
        results.clear();
        return;
    }
    if results.len() <= top_k {
        return;
    }

    let total_ocr = results
        .iter()
        .filter(|result| is_ocr_filter_visible_result(result))
        .count();
    let target_ocr = total_ocr.min(top_k.min(20));
    if target_ocr == 0 {
        results.truncate(top_k);
        return;
    }

    let mut kept = results.iter().take(top_k).cloned().collect::<Vec<_>>();
    let mut kept_ocr = kept
        .iter()
        .filter(|result| is_ocr_filter_visible_result(result))
        .count();
    if kept_ocr >= target_ocr {
        *results = kept;
        return;
    }

    let mut seen = kept
        .iter()
        .map(search_result_identity)
        .collect::<std::collections::HashSet<_>>();
    for candidate in results
        .iter()
        .skip(top_k)
        .filter(|result| is_ocr_filter_visible_result(result))
    {
        if kept_ocr >= target_ocr {
            break;
        }
        let key = search_result_identity(candidate);
        if !seen.insert(key) {
            continue;
        }
        let replace_at = kept
            .iter()
            .rposition(|result| !is_ocr_filter_visible_result(result))
            .unwrap_or_else(|| kept.len().saturating_sub(1));
        kept[replace_at] = candidate.clone();
        kept_ocr += 1;
    }

    *results = kept;
}

fn empty_scope_result(query: &str) -> Option<SearchResult> {
    let q = query.trim().to_ascii_lowercase();
    let (prefix, title, detail) = [
        (
            "bookmarks:",
            "No bookmarks found",
            "Browser bookmarks are indexed in the background.",
        ),
        (
            "history:",
            "No browser history found",
            "Browser history is indexed in the background.",
        ),
        (
            "commits:",
            "No git commits found",
            "Git repositories may still be indexing.",
        ),
        (
            "agentchats:",
            "No agent history found",
            "Agent runs will appear here after they are created.",
        ),
        (
            "clip:",
            "No clipboard history found",
            "Copied items will appear here after capture.",
        ),
        (
            "clipboard:",
            "No clipboard history found",
            "Copied items will appear here after capture.",
        ),
        (
            "file:",
            "No files found",
            "Indexed files will appear here when they match this page.",
        ),
        (
            "code:",
            "No code files found",
            "Indexed source files will appear here when they match this page.",
        ),
        (
            "img:",
            "No images found",
            "Indexed images and OCR matches will appear here.",
        ),
        (
            "image:",
            "No images found",
            "Indexed images and OCR matches will appear here.",
        ),
        (
            "screenshots:",
            "No screenshots found",
            "Indexed screenshots and OCR matches will appear here.",
        ),
    ]
    .into_iter()
    .find(|(prefix, _, _)| q.starts_with(prefix))?;

    Some(SearchResult {
        entry: CatalogEntry {
            id: format!("empty.{}", prefix.trim_end_matches(':')),
            control_name: title.to_string(),
            breadcrumb_path: "Search > Empty page".to_string(),
            launch_command: prefix.to_string(),
            source: "FOLDER".to_string(),
            description: detail.to_string(),
            synonyms: String::new(),
        },
        score: 0.0,
    })
}

pub(crate) fn is_native_settings_command(command: &str) -> bool {
    let cmd = command.to_ascii_lowercase();
    cmd.starts_with("ms-settings:")
        || cmd.starts_with("control")
        || cmd.contains(".cpl")
        || cmd.ends_with(".msc")
        || cmd.contains("shell:::{")
        || cmd.starts_with("optionalfeatures.exe")
        || cmd.starts_with("useraccountcontrolsettings.exe")
        || cmd.starts_with("dfrgui.exe")
        || cmd.starts_with("cleanmgr.exe")
        || cmd.starts_with("regedit.exe")
        || cmd.starts_with("msconfig.exe")
        || cmd.starts_with("resmon.exe")
        || cmd.starts_with("sndvol.exe")
        || cmd.starts_with("mblctr.exe")
        || cmd.starts_with("systemproperties")
        || cmd.starts_with("inetmgr.exe")
        || cmd.starts_with("odbcad32.exe")
        || cmd.starts_with("mstsc.exe")
        || cmd.starts_with("dxdiag.exe")
        || cmd.starts_with("msinfo32.exe")
        || cmd.starts_with("wt.exe")
        || cmd.starts_with("powershell.exe")
        || cmd.starts_with("windowssandbox.exe")
        || cmd.starts_with("cmd.exe")
}

fn native_settings_source(command: &str) -> &'static str {
    let cmd = command.to_ascii_lowercase();
    if cmd.starts_with("control") || cmd.contains(".cpl") || cmd.contains("shell:::{") {
        "CONTROL"
    } else {
        "SETTINGS"
    }
}

fn is_native_settings_result(result: &SearchResult) -> bool {
    is_native_settings_command(&result.entry.launch_command)
        || result.entry.source.eq_ignore_ascii_case("settings")
        || result.entry.source.eq_ignore_ascii_case("control")
}

/// lean-build allowlist: keep only the curated feature set. See SearchEngine::search.
fn lean_allowed(r: &SearchResult) -> bool {
    let s = r.entry.source.as_str();
    let cmd = r.entry.launch_command.as_str();

    // (4) Control panel + modern settings — matched by how Windows opens them.
    if is_native_settings_result(r) || cmd.starts_with("action:") || s == "ACTION" || s == "SYSTEM"
    {
        return true;
    }

    // (9) AI agents and their history.
    if cmd.starts_with("agent:")
        || cmd.starts_with("openagent:")
        || cmd.starts_with("mkagent:")
        || cmd.starts_with("aichat:")
        || is_ai_provider_url(cmd)
    {
        return true;
    }

    if s.eq_ignore_ascii_case("web") {
        return true;
    }

    if s == "FOLDER" {
        // Real directory results carry a filesystem path — keep them (folder search).
        if cmd.contains('\\') || cmd.contains('/') {
            return true;
        }
        // Otherwise it's a bare "scope:" entry-point; keep only the allowed categories.
        return matches!(
            cmd,
            "file:"
                | "code:"
                | "history:"
                | "bookmarks:"
                | "agents:"
                | "agentchats:"
        );
    }

    // (1) files, (2) apps, (3) history, (4) bookmarks, (5) calculator, (6) snippets.
    matches!(
        s,
        "FILE"
            | "RECENT"
            | "CODE"
            | "app"
            | "HISTORY"
            | "BOOKMARK"
            | "CALC"
            | "SNIPPET"
    )
}

fn is_ai_provider_url(cmd: &str) -> bool {
    cmd.starts_with("https://chatgpt.com/")
        || cmd.starts_with("https://claude.ai/")
        || cmd.starts_with("https://gemini.google.com/")
        || cmd.starts_with("https://grok.com/")
        || cmd.starts_with("https://eyux.com.et")
}

fn plugin_allowed(r: &SearchResult, settings: &crate::settings::AppSettings) -> bool {
    let cmd = r.entry.launch_command.as_str();
    let source = r.entry.source.as_str();
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

struct CatalogEntryIndex {
    name: String,
    name_chars: usize,
    breadcrumb: String,
    description: String,
    source: String,
    synonyms: String,
}

impl CatalogEntryIndex {
    fn from_entry(entry: &CatalogEntry) -> Self {
        let name = entry.control_name.to_lowercase();
        Self {
            name_chars: name.chars().count(),
            name,
            breadcrumb: entry.breadcrumb_path.to_lowercase(),
            description: entry.description.to_lowercase(),
            source: entry.source.to_lowercase(),
            synonyms: entry.synonyms.to_lowercase(),
        }
    }
}

#[derive(Deserialize)]
struct MetaJson {
    control_name: String,
    breadcrumb_path: String,
    launch_command: String,
    source: String,
    id: String,
    description: String,
    synonyms: String,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct AnchorCategory {
    pub _name: &'static str,
    pub target_id: &'static str,
    pub translation_tip: &'static str,
    pub phrases: &'static [&'static str],
    pub vecs: Vec<Vec<f32>>,
}

#[derive(Clone)]
pub struct AppInfo {
    pub name: String,
    pub path: String,
}

#[derive(Clone)]
pub struct RecentFileInfo {
    pub name: String,
    pub path: String, // resolved target path
}

/// One row of the in-memory filename index used for instant fast-phase file/folder search.
struct FileRow {
    name: String,
    name_lower: String,
    path: String,
    ext: String,
    path_modifier: f32,
}

#[allow(dead_code)]
pub struct SearchEngine {
    vecs: Vec<f32>,
    meta: Vec<CatalogEntry>,
    meta_index: Vec<CatalogEntryIndex>,
    n: usize,
    dim: usize,
    anchor_categories: Vec<AnchorCategory>,
    apps: Vec<AppInfo>,
    recent_files: Vec<RecentFileInfo>,
    // In-memory filename index — lets the fast phase scan files/folders in RAM (Everything-style)
    // instead of a per-keystroke `LIKE '%q%'` full-table scan of `files`.
    file_index: Vec<FileRow>,
    was_indexing: bool,
    _db_path: std::path::PathBuf,
    conn: Connection,
}

impl SearchEngine {
    pub fn new(db_path: std::path::PathBuf, build_file_index: bool) -> Result<Self> {
        if CATALOG.len() < 8 {
            bail!("catalog.bin too small");
        }
        let n = u32::from_le_bytes(CATALOG[0..4].try_into()?) as usize;
        let dim = u32::from_le_bytes(CATALOG[4..8].try_into()?) as usize;

        let mut off = 8usize;
        let mut vecs = Vec::with_capacity(n * dim);
        let mut meta = Vec::with_capacity(n);

        for _ in 0..n {
            let vb = dim * 4;
            for chunk in CATALOG[off..off + vb].chunks_exact(4) {
                vecs.push(f32::from_le_bytes(chunk.try_into()?));
            }
            off += vb;
            let ml = u16::from_le_bytes(CATALOG[off..off + 2].try_into()?) as usize;
            off += 2;
            let m: MetaJson = serde_json::from_slice(&CATALOG[off..off + ml])?;
            off += ml;
            meta.push(CatalogEntry {
                id: m.id,
                control_name: m.control_name,
                breadcrumb_path: m.breadcrumb_path,
                launch_command: m.launch_command,
                source: m.source,
                description: m.description,
                synonyms: m.synonyms,
            });
        }

        let anchor_categories = vec![
            AnchorCategory {
                _name: "eyes_hurt",
                target_id: "system.night_light",
                translation_tip: "Translation Tip: Eye strain? Filter blue light or adjust display brightness. | System > Display > Brightness & color > Night light",
                phrases: &["my eyes hurt", "eye strain", "screen too bright", "reduce blue light", "eyes hurt", "blue light filter"],
                vecs: vec![],
            },
            AnchorCategory {
                _name: "internet_slow",
                target_id: "system.troubleshoot.network-internet-troubleshooter",
                translation_tip: "Translation Tip: Slow connection? Diagnose network adapter and DNS. | System > Troubleshoot > Other troubleshooters > Network and Internet",
                phrases: &["internet is slow", "wi-fi is slow", "wifi is slow", "slow network", "connection speed", "slow internet", "slow wifi"],
                vecs: vec![],
            },
            AnchorCategory {
                _name: "mouse_flying",
                target_id: "bluetooth-devices.mouse.enhance-pointer-precision",
                translation_tip: "Translation Tip: Erratic mouse? Adjust cursor speed and pointer precision. | Bluetooth & devices > Mouse > Enhance pointer precision",
                phrases: &["mouse is flying", "cursor moving too fast", "erratic mouse speed", "mouse speed too high", "pointer speed is fast"],
                vecs: vec![],
            },
            AnchorCategory {
                _name: "battery_dying",
                target_id: "system.power.energy_saver",
                translation_tip: "Translation Tip: Battery low? Enable Energy Saver to extend power. | System > Power & battery > Energy saver",
                phrases: &["battery is dying", "battery low", "running out of power", "extend battery life", "battery saver", "laptop dying"],
                vecs: vec![],
            },
            AnchorCategory {
                _name: "cant_see_text",
                target_id: "text_size.text_size",
                translation_tip: "Translation Tip: Text too small? Adjust scale or make text bigger. | Text size > Text size",
                phrases: &["can't see text", "text is too small", "make font size bigger", "increase UI scale", "font is tiny", "screen is too small"],
                vecs: vec![],
            },
        ];

        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&db_path)?;
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_files_name ON files(name);",
            [],
        );


        conn.execute(
            "CREATE TABLE IF NOT EXISTS timeline_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                duration INTEGER NOT NULL,
                app_name TEXT NOT NULL,
                window_title TEXT NOT NULL
            );",
            [],
        )?;
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timeline_timestamp ON timeline_events(timestamp);",
            [],
        );
        ensure_memory_events_schema(&conn)?;

        // Create quicklinks table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS quicklinks (
                name TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                keyword TEXT NOT NULL
            );",
            [],
        )?;

        // Pre-populate quicklinks if empty
        let ql_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM quicklinks", [], |row| row.get(0))
            .unwrap_or(0);
        if ql_count == 0 {
            let defaults = &[
                ("Google", "https://google.com/search?q={query}", "g"),
                (
                    "YouTube",
                    "https://youtube.com/results?search_query={query}",
                    "yt",
                ),
                ("GitHub", "https://github.com/search?q={query}", "gh"),
                (
                    "Rust Docs",
                    "https://docs.rs/releases/search?query={query}",
                    "rs",
                ),
            ];
            for &(name, url, keyword) in defaults {
                let _ = conn.execute(
                    "INSERT INTO quicklinks (name, url, keyword) VALUES (?, ?, ?);",
                    rusqlite::params![name, url, keyword],
                );
            }
        }

        // Create snippets table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS snippets (
                name TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                keyword TEXT
            );",
            [],
        )?;

        // Create focus categories table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS focus_categories (
                name TEXT PRIMARY KEY,
                blocked_apps TEXT NOT NULL
            );",
            [],
        )?;

        let fc_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM focus_categories", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        if fc_count == 0 {
            let _ = conn.execute(
                "INSERT INTO focus_categories (name, blocked_apps) VALUES (?, ?);",
                rusqlite::params!["Deep Work", "Discord.exe, slack.exe"],
            );
        }

        // Create AI settings table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
            [],
        )?;

        // Pre-populate AI endpoint/model if empty. Do not seed an API key:
        // users configure their own key via settings/env/AppData.
        let ai_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_settings", [], |row| row.get(0))
            .unwrap_or(0);
        if ai_count == 0 {
            let _ = conn.execute(
                "INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'https://opencode.ai/zen/v1/chat/completions');",
                [],
            );
            let _ = conn.execute(
                "INSERT INTO ai_settings (key, value) VALUES ('model', 'deepseek-v4-flash-free');",
                [],
            );
        }

        // Pre-populate snippets if empty
        let sn_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snippets", [], |row| row.get(0))
            .unwrap_or(0);
        if sn_count == 0 {
            let _ = conn.execute(
                "INSERT INTO snippets (name, content, keyword) VALUES (?, ?, ?);",
                rusqlite::params![
                    "Example Snippet",
                    "Hello, this is a reusable snippet! Type '!demo' to trigger it or search for it.",
                    "!demo"
                ],
            );
        }
        ensure_settings_catalog_fts(&conn, &meta)?;

        let meta_index = meta.iter().map(CatalogEntryIndex::from_entry).collect();
        let mut engine = Self {
            vecs,
            meta,
            meta_index,
            n,
            dim,
            anchor_categories,
            apps: vec![],
            recent_files: vec![],
            file_index: Vec::new(),
            was_indexing: false,
            _db_path: db_path,
            conn,
        };
        engine.apps = scan_apps();
        engine.recent_files = scan_recent_files();
        // Only the fast engine needs the in-memory index; the slow (content) engine searches
        // files via SQL/FTS and runs on its own thread, so it skips this to save RAM + startup.
        if build_file_index {
            engine.file_index = Self::build_file_index(&engine.conn);
        }

        let _ = engine.search("settings", 1);
        Ok(engine)
    }

    fn search_settings_catalog_fts(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let fts_query = make_fts_prefix_query(query);
        if fts_query.is_empty() {
            return Vec::new();
        }

        let mut stmt = match self.conn.prepare(
            "SELECT name, breadcrumb, description, synonyms, launch_command, source
             FROM settings_catalog_fts
             WHERE settings_catalog_fts MATCH ?
             ORDER BY bm25(settings_catalog_fts)
             LIMIT ?",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map(
            rusqlite::params![fts_query, limit as i64],
            |row| -> rusqlite::Result<CatalogEntry> {
                let launch_command: String = row.get(4)?;
                Ok(CatalogEntry {
                    id: format!("settings.{}", launch_command),
                    control_name: row.get(0)?,
                    breadcrumb_path: row.get(1)?,
                    description: row.get(2)?,
                    synonyms: row.get(3)?,
                    source: row
                        .get::<_, String>(5)
                        .unwrap_or_else(|_| native_settings_source(&launch_command).to_string()),
                    launch_command,
                })
            },
        ) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(|row| row.ok())
            .enumerate()
            .map(|(idx, entry)| SearchResult {
                entry,
                score: 130.0 - idx as f32,
            })
            .collect()
    }

    fn get_path_score_modifier(full_path: &str) -> f32 {
        let path_lower = full_path.to_lowercase();

        // Penalize system/tool/hidden directories
        if path_lower.contains("\\node_modules\\")
            || path_lower.contains("\\target\\")
            || path_lower.contains("\\.git\\")
            || path_lower.contains("\\appdata\\")
            || path_lower.contains("\\.cargo\\")
            || path_lower.contains("\\.rustup\\")
            || path_lower.contains("\\.npm\\")
            || path_lower.contains("\\.antigravity")
            || path_lower.contains("\\.cursor\\")
            || path_lower.contains("\\venv\\")
            || path_lower.contains("\\.venv\\")
            || path_lower.contains("\\__macosx\\")
            || path_lower.contains("\\bin\\")
            || path_lower.contains("\\obj\\")
            || path_lower.contains("\\temp\\")
            || path_lower.contains("\\tmp\\")
        {
            return -2.0; // Excluded from results
        }

        // Boost user's active/primary directories
        if path_lower.contains("\\desktop\\")
            || path_lower.contains("\\documents\\")
            || path_lower.contains("\\downloads\\")
            || path_lower.contains("\\pictures\\")
        {
            return 1.5;
        }

        0.0
    }

    /// Load the whole `files` table into RAM once, precomputing the lowercase name and the
    /// path score so each search is a pure in-memory scan.
    fn build_file_index(conn: &Connection) -> Vec<FileRow> {
        let mut rows = Vec::new();
        if let Ok(mut stmt) = conn.prepare("SELECT path, name, extension FROM files") {
            if let Ok(it) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            }) {
                for (path, name, ext) in it.filter_map(|r| r.ok()) {
                    let name_lower = name.to_lowercase();
                    let path_modifier = Self::get_path_score_modifier(&path);
                    rows.push(FileRow {
                        name,
                        name_lower,
                        path,
                        ext,
                        path_modifier,
                    });
                }
            }
        }
        rows
    }

    /// Instant in-memory equivalent of the filename portion of `search_files_generic` (no SQL,
    /// no content FTS). Scores every row and returns the best `max_results`. O(n) over the index;
    /// fine for the user-folder-sized index, revisit with a prefix trie if it ever grows huge.
    fn search_files_in_memory(
        &self,
        query: &str,
        only_code: bool,
        max_results: usize,
    ) -> Vec<SearchResult> {
        let q_lower = query.trim().to_lowercase();
        let q_words: Vec<&str> = q_lower.split_whitespace().collect();
        if q_words.is_empty() {
            return Vec::new();
        }
        let code_exts = [
            "rs", "py", "js", "ts", "json", "html", "css", "c", "cpp", "h", "hpp", "cs", "go",
            "java", "kt", "sh", "bat", "ps1", "yaml", "yml", "toml", "ini", "sql", "xml",
        ];
        let mut results = Vec::new();
        for row in &self.file_index {
            if row.path_modifier < -1.0 {
                continue;
            }
            let name_no_ext_lower = match row.name_lower.rfind('.') {
                Some(d) => &row.name_lower[..d],
                None => row.name_lower.as_str(),
            };
            let mut score = if row.name_lower == q_lower || name_no_ext_lower == q_lower {
                3.0
            } else if row.name_lower.starts_with(&q_lower)
                || name_no_ext_lower.starts_with(&q_lower)
            {
                2.5
            } else if row.name_lower.contains(&q_lower) {
                1.8
            } else {
                let words: Vec<&str> = name_no_ext_lower
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .collect();
                let matched = q_words.iter().filter(|w| words.contains(w)).count();
                if matched > 0 {
                    0.8 + 0.4 * (matched as f32 / q_words.len() as f32)
                } else {
                    0.0
                }
            };
            if score <= 0.0 {
                continue;
            }
            let is_code = code_exts.contains(&row.ext.as_str());
            if only_code && !is_code {
                continue;
            }
            score += row.path_modifier;
            let source = if row.ext == "folder" {
                "FOLDER"
            } else if only_code || is_code {
                "CODE"
            } else {
                "FILE"
            };
            let breadcrumb = if source == "FOLDER" {
                format!("Folder > {}", row.path)
            } else {
                format!(
                    "{} > {}",
                    if source == "CODE" { "Code" } else { "File" },
                    row.path
                )
            };
            let description = if source == "FOLDER" {
                "Local folder".to_string()
            } else {
                format!("Local {} file", row.ext.to_uppercase())
            };
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("{}.{}", source.to_lowercase(), row.path),
                    control_name: row.name.clone(),
                    breadcrumb_path: breadcrumb,
                    launch_command: row.path.clone(),
                    source: source.to_string(),
                    description,
                    synonyms: row.name_lower.clone(),
                },
                score,
            });
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(max_results);
        results
    }

    // with_fts_content: if false (general search), skips content-only matches — only filename hits shown.
    //                   if true  (file:/code: prefix), full content search is included.
    fn search_files_generic(
        &self,
        query: &str,
        only_code: bool,
        max_results: usize,
        with_fts_content: bool,
    ) -> Vec<SearchResult> {
        let conn = &self.conn;

        let q_lower = query.to_lowercase();
        let q_words: Vec<&str> = q_lower.split_whitespace().collect();

        // ── Empty query: show most recently modified files from DB ──────────
        if q_words.is_empty() {
            let image_exts = ["png", "jpg", "jpeg", "gif", "bmp", "webp"];
            let code_exts_local = [
                "rs", "py", "js", "ts", "json", "html", "css", "c", "cpp", "h", "hpp", "cs", "go",
                "java", "kt", "sh", "bat", "ps1", "yaml", "yml", "toml", "ini", "sql", "xml",
            ];
            let mut results = Vec::new();
            // Pull recent files sorted by modification time descending
            let query_str = if only_code {
                let placeholders: Vec<String> =
                    code_exts_local.iter().map(|_| "?".to_string()).collect();
                format!(
                    "SELECT path, name, extension, modified FROM files WHERE is_dir=0 AND extension IN ({}) ORDER BY modified DESC LIMIT ?",
                    placeholders.join(",")
                )
            } else {
                "SELECT path, name, extension, modified FROM files WHERE is_dir=0 ORDER BY modified DESC LIMIT ?".to_string()
            };
            if let Ok(mut stmt) = conn.prepare(&query_str) {
                let mut params_vec: Vec<rusqlite::types::Value> = vec![];
                if only_code {
                    for ext in &code_exts_local {
                        params_vec.push(rusqlite::types::Value::Text(ext.to_string()));
                    }
                }
                params_vec.push(rusqlite::types::Value::Integer(max_results as i64));
                let params_ref = rusqlite::params_from_iter(params_vec.iter());
                if let Ok(rows) = stmt.query_map(params_ref, |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                }) {
                    for row in rows.filter_map(|r| r.ok()) {
                        let (path, name, ext, _modified) = row;
                        let path_modifier = Self::get_path_score_modifier(&path);
                        if path_modifier < -1.0 {
                            continue;
                        }
                        // Score: recent + boosted path + image bonus
                        let image_bonus = if image_exts.contains(&ext.as_str()) {
                            0.5
                        } else {
                            0.0
                        };
                        let recency_score = 1.0 + path_modifier + image_bonus;
                        let source = if only_code || code_exts_local.contains(&ext.as_str()) {
                            "CODE"
                        } else if image_exts.contains(&ext.as_str()) {
                            "FILE"
                        } else {
                            "FILE"
                        };
                        let breadcrumb = format!(
                            "{} > {}",
                            if source == "CODE" { "Code" } else { "File" },
                            path
                        );
                        let description = format!("Local {} file", ext.to_uppercase());
                        results.push(SearchResult {
                            entry: CatalogEntry {
                                id: format!("{}.{}", source.to_lowercase(), path),
                                control_name: name.clone(),
                                breadcrumb_path: breadcrumb,
                                launch_command: path,
                                source: source.to_string(),
                                description,
                                synonyms: name.to_lowercase(),
                            },
                            score: recency_score,
                        });
                    }
                }
            }
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            return results;
        }

        let code_exts = [
            "rs", "py", "js", "ts", "json", "html", "css", "c", "cpp", "h", "hpp", "cs", "go",
            "java", "kt", "sh", "bat", "ps1", "yaml", "yml", "toml", "ini", "sql", "xml",
        ];

        // Helper: score a filename against query
        let score_name = |name: &str| -> f32 {
            let name_lower = name.to_lowercase();
            let name_no_ext = name_lower
                .rfind('.')
                .map(|d| &name_lower[..d])
                .unwrap_or(&name_lower);
            if name_lower == q_lower || name_no_ext == q_lower {
                return 3.0;
            }
            if name_lower.starts_with(&q_lower) || name_no_ext.starts_with(&q_lower) {
                return 2.5;
            }
            if name_lower.contains(&q_lower) {
                return 1.8;
            }
            let words: Vec<&str> = name_no_ext
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            let matched = q_words.iter().filter(|w| words.contains(w)).count();
            if matched > 0 {
                0.8 + 0.4 * (matched as f32 / q_words.len() as f32)
            } else {
                0.0
            }
        };

        let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results = Vec::new();

        let name_query = format!("%{}%", q_lower);
        let query_str = if only_code {
            let placeholders: Vec<String> = code_exts.iter().map(|_| "?".to_string()).collect();
            format!(
                "SELECT path, name, extension FROM files WHERE name LIKE ? AND extension IN ({}) LIMIT ?",
                placeholders.join(",")
            )
        } else {
            "SELECT path, name, extension FROM files WHERE name LIKE ? LIMIT ?".to_string()
        };
        if let Ok(mut stmt) = conn.prepare(&query_str) {
            let mut params_vec: Vec<rusqlite::types::Value> =
                vec![rusqlite::types::Value::Text(name_query)];
            if only_code {
                for ext in &code_exts {
                    params_vec.push(rusqlite::types::Value::Text(ext.to_string()));
                }
            }
            params_vec.push(rusqlite::types::Value::Integer(max_results as i64));
            let params_ref = rusqlite::params_from_iter(params_vec.iter());
            if let Ok(rows) = stmt.query_map(params_ref, |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            }) {
                for row in rows.filter_map(|r| r.ok()) {
                    let (path, name, ext) = row;
                    let path_modifier = Self::get_path_score_modifier(&path);
                    if path_modifier < -1.0 {
                        continue;
                    }
                    let mut score = score_name(&name);
                    if score <= 0.0 {
                        continue;
                    }
                    score += path_modifier;
                    let source = if ext == "folder" {
                        "FOLDER"
                    } else if only_code || code_exts.contains(&ext.as_str()) {
                        "CODE"
                    } else {
                        "FILE"
                    };
                    let breadcrumb = if source == "FOLDER" {
                        format!("Folder > {}", path)
                    } else {
                        format!(
                            "{} > {}",
                            if source == "CODE" { "Code" } else { "File" },
                            path
                        )
                    };
                    let description = if source == "FOLDER" {
                        "Local folder".to_string()
                    } else {
                        format!("Local {} file", ext.to_uppercase())
                    };
                    seen_paths.insert(path.clone());
                    results.push(SearchResult {
                        entry: CatalogEntry {
                            id: format!("{}.{}", source.to_lowercase(), path),
                            control_name: name.clone(),
                            breadcrumb_path: breadcrumb,
                            launch_command: path,
                            source: source.to_string(),
                            description,
                            synonyms: name.to_lowercase(),
                        },
                        score,
                    });
                }
            }
        }

        // ── 3. FTS5 content search — only in dedicated prefix searches (file:/code:)
        // In general search (with_fts_content=false), we skip content-only matches to prevent UI stutters.
        if !with_fts_content {
            return results;
        }

        // Build prefix-based FTS matching query: each word matches as prefix (e.g. "gene*" or "hello* world*")
        let clean_fts_query = q_words
            .iter()
            .map(|w| {
                let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
                format!("{}*", clean)
            })
            .filter(|w| w.len() > 1) // exclude empty or single "*"
            .collect::<Vec<String>>()
            .join(" ");

        if clean_fts_query.is_empty() {
            return results;
        }

        let fts_query_str = if only_code {
            let placeholders: Vec<String> = code_exts.iter().map(|_| "?".to_string()).collect();
            format!(
                "SELECT f.path, f.name, f.extension, snippet(files_fts, 1, '', '', '...', 15) \
                 FROM files f \
                 JOIN files_fts fts ON f.path = fts.path \
                 WHERE files_fts MATCH ? AND f.extension IN ({}) ORDER BY rank LIMIT ?",
                placeholders.join(",")
            )
        } else {
            "SELECT f.path, f.name, f.extension, snippet(files_fts, 1, '', '', '...', 15) \
             FROM files f \
             JOIN files_fts fts ON f.path = fts.path \
             WHERE files_fts MATCH ? AND f.extension NOT IN ('png', 'jpg', 'jpeg', 'bmp', 'gif', 'webp') ORDER BY rank LIMIT ?"
                .to_string()
        };
        if let Ok(mut stmt_fts) = conn.prepare(&fts_query_str) {
            let mut fts_params_vec: Vec<rusqlite::types::Value> =
                vec![rusqlite::types::Value::Text(clean_fts_query)];
            if only_code {
                for ext in &code_exts {
                    fts_params_vec.push(rusqlite::types::Value::Text(ext.to_string()));
                }
            }
            // Order by bm25 relevance and keep the same broad candidate budget as file names,
            // so content/OCR matches remain available to filters instead of being cut early.
            let fts_limit = max_results.max(30);
            fts_params_vec.push(rusqlite::types::Value::Integer(fts_limit as i64));
            let fts_params_ref = rusqlite::params_from_iter(fts_params_vec.iter());
            if let Ok(rows) = stmt_fts.query_map(fts_params_ref, |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            }) {
                for row in rows.filter_map(|r| r.ok()) {
                    let (path, name, ext, snippet_raw) = row;
                    // Already shown via metadata — upgrade score if name also matches
                    if seen_paths.contains(&path) {
                        // Boost existing entry's score for content match
                        if let Some(existing) =
                            results.iter_mut().find(|r| r.entry.launch_command == path)
                        {
                            existing.score += 0.5; // content match bonus
                            existing.entry.source =
                                content_match_source(&ext, only_code).to_string();
                        }
                        continue;
                    }
                    let path_modifier = Self::get_path_score_modifier(&path);
                    if path_modifier < -1.0 {
                        continue;
                    }
                    let snippet = snippet_raw
                        .replace('\n', " ")
                        .replace('\r', " ")
                        .replace('\t', " ")
                        .split_whitespace()
                        .collect::<Vec<&str>>()
                        .join(" ");
                    let source = content_match_source(&ext, only_code);
                    // Score: content-only matches intentionally score lower than filename matches.
                    // Base 0.8 + up to 1.5 name bonus keeps content matches below pure filename hits (score 1.8+).
                    let name_bonus = score_name(&name).min(1.5);
                    let score = 0.8 + name_bonus + path_modifier;
                    if score <= 0.0 {
                        continue;
                    }
                    let parent_dir = std::path::Path::new(&path)
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    let breadcrumb = if parent_dir.is_empty() {
                        format!(
                            "{} | {}",
                            if source == "CODE" { "Code" } else { "File" },
                            snippet
                        )
                    } else {
                        format!(
                            "{} > {} | {}",
                            if source == "CODE" { "Code" } else { "File" },
                            parent_dir,
                            snippet
                        )
                    };
                    seen_paths.insert(path.clone());
                    results.push(SearchResult {
                        entry: CatalogEntry {
                            id: format!("{}.{}", source.to_lowercase(), path),
                            control_name: name.clone(),
                            breadcrumb_path: breadcrumb,
                            launch_command: path,
                            source: source.to_string(),
                            description: format!(
                                "Local {} file (content match)",
                                ext.to_uppercase()
                            ),
                            synonyms: name.to_lowercase(),
                        },
                        score,
                    });
                }
            }
        }

        results
    }

    #[allow(dead_code)]
    fn search_local_files(&self, query: &str) -> Vec<SearchResult> {
        self.search_files_generic(query, false, 300, false)
    }

    fn search_local_files_with_fts(&self, query: &str, with_fts: bool) -> Vec<SearchResult> {
        let limit = candidate_limit(300);
        if !with_fts {
            // Fast phase: instant in-memory filename/folder scan, no SQL full-table LIKE.
            return self.search_files_in_memory(query, false, limit);
        }
        self.search_files_generic(query, false, limit, with_fts)
    }

    /*
    pub fn db_path(&self) -> std::path::PathBuf {
        self._db_path.clone()
    }
    */

    pub fn search_timeline(
        &self,
        start_time: i64,
        end_time: i64,
        keyword: &str,
    ) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;

        let select_query = if keyword.is_empty() {
            "SELECT timestamp, duration, app_name, window_title FROM timeline_events \
             WHERE timestamp >= ? AND timestamp <= ? \
             ORDER BY timestamp DESC LIMIT 50"
                .to_string()
        } else {
            "SELECT timestamp, duration, app_name, window_title FROM timeline_events \
             WHERE timestamp >= ? AND timestamp <= ? AND (window_title LIKE ? OR app_name LIKE ?) \
             ORDER BY timestamp DESC LIMIT 50"
                .to_string()
        };

        let mut stmt = match conn.prepare(&select_query) {
            Ok(s) => s,
            Err(_) => return results,
        };

        let rows: Vec<(i64, i64, String, String)> = if keyword.is_empty() {
            stmt.query_map([start_time, end_time], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        } else {
            let like_pattern = format!("%{}%", keyword.to_lowercase());
            stmt.query_map(
                rusqlite::params![start_time, end_time, like_pattern, like_pattern],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        for (timestamp, duration, app_name, window_title) in rows {
            let time_str = format_timestamp_local(timestamp);
            let dur_str = if duration < 60 {
                format!("{}s", duration)
            } else {
                format!("{}m {}s", duration / 60, duration % 60)
            };

            // Determine launch command: prefer a URL or path extracted from the window title
            let title_has_url = window_title.contains(":\\")
                || window_title.contains(":/")
                || window_title.contains("http://")
                || window_title.contains("https://");
            let title_has_meeting = [
                "meet.google.com",
                "zoom.us",
                "teams.microsoft.com",
                "teams.live.com",
                "webex.com",
                "gotomeeting.com",
            ]
            .iter()
            .any(|d| window_title.to_lowercase().contains(d));
            let launch_command = if title_has_url || title_has_meeting {
                extract_path_or_url(&window_title).unwrap_or_else(|| app_name.clone())
            } else {
                app_name.clone()
            };

            let display_app = std::path::Path::new(&app_name)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| app_name.clone());

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("timeline.{}", timestamp),
                    control_name: format!("{} ({})", window_title, display_app),
                    breadcrumb_path: format!("Timeline > {} ({})", time_str, dur_str),
                    launch_command,
                    source: "MEMORY".to_string(),
                    description: format!("Active app: {} at {}", display_app, time_str),
                    synonyms: format!(
                        "{} {} timeline memory",
                        display_app.to_lowercase(),
                        window_title.to_lowercase()
                    ),
                },
                score: 4.0,
            });
        }

        results
    }

    pub fn search_timeline_sequential(
        &self,
        anchor_app: &str,
        direction: &str,
        start_time: i64,
        end_time: i64,
    ) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;

        let anchor_pattern = format!("%{}%", anchor_app.to_lowercase());

        let anchor_query = "SELECT timestamp FROM timeline_events \
             WHERE (LOWER(app_name) LIKE ? OR LOWER(window_title) LIKE ?) \
             AND timestamp >= ? AND timestamp <= ? \
             ORDER BY timestamp DESC LIMIT 1";

        let anchor_ts: Option<i64> = conn
            .query_row(
                anchor_query,
                rusqlite::params![anchor_pattern, anchor_pattern, start_time, end_time],
                |row| row.get::<_, i64>(0),
            )
            .ok();

        let Some(anchor_ts) = anchor_ts else {
            return results;
        };

        let window_secs: i64 = 600;

        let seq_query = if direction == "after" {
            "SELECT timestamp, duration, app_name, window_title FROM timeline_events \
             WHERE timestamp > ? AND timestamp <= ? \
             AND (LOWER(app_name) NOT LIKE ? AND LOWER(window_title) NOT LIKE ?) \
             ORDER BY timestamp ASC LIMIT 20"
        } else {
            "SELECT timestamp, duration, app_name, window_title FROM timeline_events \
             WHERE timestamp >= ? AND timestamp < ? \
             AND (LOWER(app_name) NOT LIKE ? AND LOWER(window_title) NOT LIKE ?) \
             ORDER BY timestamp DESC LIMIT 20"
        };

        let (range_start, range_end) = if direction == "after" {
            (anchor_ts, anchor_ts + window_secs)
        } else {
            (anchor_ts - window_secs, anchor_ts)
        };

        let mut stmt = match conn.prepare(seq_query) {
            Ok(s) => s,
            Err(_) => return results,
        };

        let rows: Vec<(i64, i64, String, String)> = stmt
            .query_map(
                rusqlite::params![range_start, range_end, anchor_pattern, anchor_pattern],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        let dir_label = if direction == "after" {
            "After"
        } else {
            "Before"
        };

        for (i, (timestamp, duration, app_name, window_title)) in rows.into_iter().enumerate() {
            let time_str = format_timestamp_local(timestamp);
            let dur_str = if duration < 60 {
                format!("{}s", duration)
            } else {
                format!("{}m {}s", duration / 60, duration % 60)
            };

            let display_app = std::path::Path::new(&app_name)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| app_name.clone());

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("timeline.seq.{}", timestamp),
                    control_name: format!("{} ({})", window_title, display_app),
                    breadcrumb_path: format!(
                        "Timeline > {} {}: {} ({})",
                        dir_label, anchor_app, time_str, dur_str
                    ),
                    launch_command: app_name.clone(),
                    source: "MEMORY".to_string(),
                    description: format!(
                        "{} {}: {} at {}",
                        dir_label.to_lowercase(),
                        anchor_app,
                        display_app,
                        time_str
                    ),
                    synonyms: format!(
                        "{} {} {} timeline sequential",
                        display_app.to_lowercase(),
                        window_title.to_lowercase(),
                        direction
                    ),
                },
                score: 4.5 - (i as f32 * 0.1),
            });
        }

        results
    }

    pub fn search_memory_events(&self, query: &str) -> Vec<SearchResult> {
        let q = query.trim();
        let mut results = Vec::new();
        let conn = &self.conn;
        let rows: Vec<(
            i64,
            i64,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        )> = if q.is_empty() {
            let mut stmt = match conn.prepare(
                    "SELECT id, timestamp, source, event_type, title, coalesce(detail, ''), coalesce(app_name, ''), path, url
                     FROM memory_events
                     ORDER BY timestamp DESC LIMIT 50",
                ) {
                    Ok(s) => s,
                    Err(_) => return results,
                };
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        } else {
            let fts_query = make_fts_prefix_query(q);
            if fts_query.is_empty() {
                return results;
            }
            let mut stmt = match conn.prepare(
                "SELECT e.id, e.timestamp, e.source, e.event_type, e.title,
                            snippet(memory_events_fts, 1, '', '', '...', 14),
                            coalesce(e.app_name, ''), e.path, e.url
                     FROM memory_events_fts
                     JOIN memory_events e ON e.id = memory_events_fts.rowid
                     WHERE memory_events_fts MATCH ?
                     ORDER BY e.timestamp DESC LIMIT 50",
            ) {
                Ok(s) => s,
                Err(_) => return results,
            };
            stmt.query_map([fts_query], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        for (idx, (id, timestamp, source, event_type, title, detail, app_name, path, url)) in
            rows.into_iter().enumerate()
        {
            let time_str = format_timestamp_local(timestamp);
            let launch_command = url
                .clone()
                .or_else(|| path.clone())
                .unwrap_or_else(|| app_name.clone());
            let clean_detail = detail.replace("\r\n", " ").replace('\n', " ");
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("memory.{}", id),
                    control_name: title,
                    breadcrumb_path: format!("Memory > {} > {} > {}", source, event_type, time_str),
                    launch_command,
                    source: "MEMORY".to_string(),
                    description: ellipsize_chars(&clean_detail, 96),
                    synonyms: format!(
                        "{} {} {} {}",
                        source.to_lowercase(),
                        event_type.to_lowercase(),
                        app_name.to_lowercase(),
                        path.unwrap_or_default().to_lowercase()
                    ),
                },
                score: 7.0 - (idx as f32 * 0.03),
            });
        }

        results
    }

    pub fn search_memory_home(&self) -> Vec<SearchResult> {
        let (today_start, tomorrow_start) = local_day_bounds(0);
        let (yesterday_start, today_again) = local_day_bounds(1);
        let today_count = count_memory_events(&self.conn, today_start, tomorrow_start);
        let yesterday_count = count_memory_events(&self.conn, yesterday_start, today_again);
        let total_count = count_memory_events(&self.conn, 0, i64::MAX);
        let today_sources = memory_source_summary(&self.conn, today_start, tomorrow_start);
        let yesterday_sources = memory_source_summary(&self.conn, yesterday_start, today_again);

        vec![
            SearchResult {
                entry: CatalogEntry {
                    id: "memory.home".to_string(),
                    control_name: "Your Windows PC doesn't forget".to_string(),
                    breadcrumb_path: "Memory > Local on this PC".to_string(),
                    launch_command: "memory:".to_string(),
                    source: "MEMORY".to_string(),
                    description: memory_home_description(today_count, yesterday_count, total_count),
                    synonyms: "memory remembers local private pc windows timeline".to_string(),
                },
                score: 12.0,
            },
            SearchResult {
                entry: CatalogEntry {
                    id: "memory.today".to_string(),
                    control_name: "Today".to_string(),
                    breadcrumb_path: "Memory > Today".to_string(),
                    launch_command: "memory:".to_string(),
                    source: "MEMORY".to_string(),
                    description: format!(
                        "{} remembered events today. {}",
                        today_count, today_sources
                    ),
                    synonyms: "memory today activity timeline events".to_string(),
                },
                score: 11.0,
            },
            SearchResult {
                entry: CatalogEntry {
                    id: "memory.yesterday".to_string(),
                    control_name: "Yesterday".to_string(),
                    breadcrumb_path: "Memory > Yesterday".to_string(),
                    launch_command: "memory:".to_string(),
                    source: "MEMORY".to_string(),
                    description: format!(
                        "{} remembered events yesterday. {}",
                        yesterday_count, yesterday_sources
                    ),
                    synonyms: "memory yesterday activity rewind timeline events".to_string(),
                },
                score: 10.5,
            },
            SearchResult {
                entry: CatalogEntry {
                    id: "memory.privacy".to_string(),
                    control_name: "Stored locally on this PC".to_string(),
                    breadcrumb_path: "Memory > Privacy".to_string(),
                    launch_command: "memory:".to_string(),
                    source: "MEMORY".to_string(),
                    description:
                        "MemoryOS uses local SQLite storage. Pause/exclude/delete controls come next."
                            .to_string(),
                    synonyms: "memory privacy local storage sqlite pause exclude delete".to_string(),
                },
                score: 10.0,
            },
        ]
    }

    pub fn search_workday_memory_summary(&self, days_ago: i64) -> Vec<SearchResult> {
        let (start, end) = local_day_bounds(days_ago);
        let label = if days_ago == 0 { "Today" } else { "Yesterday" };
        let count = count_memory_events(&self.conn, start, end);
        let sources = memory_source_summary(&self.conn, start, end);
        let mut results = vec![SearchResult {
            entry: CatalogEntry {
                id: format!("memory.summary.{}", days_ago),
                control_name: format!("{}: what your PC remembers", label),
                breadcrumb_path: format!("Memory > {} > Summary", label),
                launch_command: "memory:".to_string(),
                source: "MEMORY".to_string(),
                description: if count == 0 {
                    "No captured memory for this day yet. MemoryOS is collecting new activity now."
                        .to_string()
                } else {
                    format!("{} remembered events. Evidence: {}", count, sources)
                },
                synonyms: "memory workday summary yesterday today evidence".to_string(),
            },
            score: 13.0,
        }];

        let mut stmt = match self.conn.prepare(
            "SELECT id, timestamp, source, event_type, title, coalesce(detail, ''), coalesce(app_name, ''), path, url
             FROM memory_events
             WHERE timestamp >= ? AND timestamp < ?
             ORDER BY timestamp DESC LIMIT 8",
        ) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let rows = stmt
            .query_map(params![start, end], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default();

        for (idx, (id, timestamp, source, event_type, title, detail, app_name, path, url)) in
            rows.into_iter().enumerate()
        {
            let launch_command = url
                .clone()
                .or_else(|| path.clone())
                .unwrap_or_else(|| app_name.clone());
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("memory.summary.evidence.{}", id),
                    control_name: title,
                    breadcrumb_path: format!(
                        "Memory > {} > {} > {}",
                        label,
                        source,
                        format_timestamp_local(timestamp)
                    ),
                    launch_command,
                    source: "MEMORY".to_string(),
                    description: format!(
                        "{}: {}",
                        event_type,
                        ellipsize_chars(&detail.replace("\r\n", " ").replace('\n', " "), 90)
                    ),
                    synonyms: format!(
                        "{} {} {} {}",
                        source.to_lowercase(),
                        event_type.to_lowercase(),
                        app_name.to_lowercase(),
                        path.unwrap_or_default().to_lowercase()
                    ),
                },
                score: 12.0 - (idx as f32 * 0.1),
            });
        }

        results
    }

    pub fn search_last_memory_session(&self) -> Vec<SearchResult> {
        let events = latest_memory_session_events(&self.conn);
        if events.is_empty() {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "memory.session.empty".to_string(),
                    control_name: "No session captured yet".to_string(),
                    breadcrumb_path: "Memory > Sessions".to_string(),
                    launch_command: "memory:".to_string(),
                    source: "MEMORY".to_string(),
                    description:
                        "MemoryOS is collecting activity now. Try again after using the PC."
                            .to_string(),
                    synonyms: "memory session continue last".to_string(),
                },
                score: 12.0,
            }];
        }

        let newest = events.first().map(|e| e.1).unwrap_or(0);
        let oldest = events.last().map(|e| e.1).unwrap_or(newest);
        let source_summary = session_source_summary(&events);
        let mut results = vec![SearchResult {
            entry: CatalogEntry {
                id: "memory.session.last".to_string(),
                control_name: "Continue last session".to_string(),
                breadcrumb_path: format!(
                    "Memory > Sessions > {} to {}",
                    format_timestamp_local(oldest),
                    format_timestamp_local(newest)
                ),
                launch_command: "memory:".to_string(),
                source: "MEMORY".to_string(),
                description: format!(
                    "{} remembered events. Evidence: {}",
                    events.len(),
                    source_summary
                ),
                synonyms: "continue last session coding work resume memory".to_string(),
            },
            score: 13.0,
        }];

        for (idx, (id, timestamp, source, event_type, title, detail, app_name, path, url)) in
            events.into_iter().take(8).enumerate()
        {
            let launch_command = url
                .clone()
                .or_else(|| path.clone())
                .unwrap_or_else(|| app_name.clone());
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("memory.session.evidence.{}", id),
                    control_name: title,
                    breadcrumb_path: format!(
                        "Memory > Last Session > {} > {}",
                        source,
                        format_timestamp_local(timestamp)
                    ),
                    launch_command,
                    source: "MEMORY".to_string(),
                    description: format!(
                        "{}: {}",
                        event_type,
                        ellipsize_chars(&detail.replace("\r\n", " ").replace('\n', " "), 90)
                    ),
                    synonyms: format!(
                        "{} {} {} {}",
                        source.to_lowercase(),
                        event_type.to_lowercase(),
                        app_name.to_lowercase(),
                        path.unwrap_or_default().to_lowercase()
                    ),
                },
                score: 12.0 - (idx as f32 * 0.1),
            });
        }

        results
    }

    pub fn search_project(&self, project_keyword: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;

        let kw_lower = project_keyword.to_lowercase();
        let kw_pattern = format!("%{}%", kw_lower);

        // 1. Find the Git repositories matching the keyword
        let mut stmt = match conn.prepare(
            "SELECT name, path, head_branch FROM git_repos WHERE name LIKE ? OR path LIKE ?",
        ) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let repos: Vec<(String, String, String)> = stmt
            .query_map([&kw_pattern, &kw_pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        let mut project_paths = Vec::new();
        for (name, path, head) in &repos {
            project_paths.push(path.clone());

            // Add git repo itself as a result
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("project.repo.{}", name),
                    control_name: format!("📁 Code Directory: {} (Git Repo)", name),
                    breadcrumb_path: format!("Project > Code > {} [branch: {}]", name, head),
                    launch_command: path.clone(),
                    source: "PROJECT".to_string(),
                    description: format!("Project repository located at {}", path),
                    synonyms: format!("{} project repo git code", name.to_lowercase()),
                },
                score: 9.5,
            });
        }

        // 2. Query matching recent files or folders
        let mut stmt =
            match conn.prepare("SELECT path, size, is_dir FROM files WHERE path LIKE ? LIMIT 20") {
                Ok(s) => s,
                Err(_) => return results,
            };
        let files: Vec<(String, i64, i32)> = stmt
            .query_map([&kw_pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i32>(2)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (path, size, is_dir) in files {
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());

            let is_code = path.contains("\\target\\") || path.contains("\\node_modules\\");
            if is_code {
                continue;
            } // skip build targets

            let control_name = if is_dir == 1 {
                format!("📁 Folder: {}", name)
            } else {
                format!("📄 Document: {}", name)
            };

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("project.file.{}", path),
                    control_name,
                    breadcrumb_path: format!("Project > Files > {}", path),
                    launch_command: path.clone(),
                    source: "PROJECT".to_string(),
                    description: format!("Related file ({} bytes)", size),
                    synonyms: format!("{} project file", name.to_lowercase()),
                },
                score: 9.0,
            });
        }

        // 3. Query browser history / bookmarks matching the keyword
        let mut stmt = match conn.prepare("SELECT url, title, source FROM browser_items WHERE title LIKE ? OR url LIKE ? LIMIT 20") {
            Ok(s) => s,
            Err(_) => return results,
        };
        let urls: Vec<(String, String, String)> = stmt
            .query_map([&kw_pattern, &kw_pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (url, title, src) in urls {
            let is_figma = url.contains("figma.com");
            let control_name = if is_figma {
                format!("🎨 Figma Design: {}", title)
            } else {
                format!("🔗 Link: {}", title)
            };

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("project.url.{}", url),
                    control_name,
                    breadcrumb_path: format!("Project > {} > {}", src.to_uppercase(), url),
                    launch_command: url,
                    source: "PROJECT".to_string(),
                    description: "Related browser link".to_string(),
                    synonyms: format!("{} project url link figma design", title.to_lowercase()),
                },
                score: 8.8,
            });
        }

        // 4. Query recent Git commits matching the keyword
        let mut stmt = match conn.prepare("SELECT repo_path, hash, message, timestamp FROM git_commits WHERE message LIKE ? ORDER BY timestamp DESC LIMIT 5") {
            Ok(s) => s,
            Err(_) => return results,
        };
        let commits: Vec<(String, String, String, i64)> = stmt
            .query_map([&kw_pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (repo_path, hash, message, ts) in commits {
            let repo_name = std::path::Path::new(&repo_path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "repo".to_string());
            let short_hash = take_chars(&hash, 7);

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("project.commit.{}", hash),
                    control_name: format!("💻 Commit: {} (in {})", message, repo_name),
                    breadcrumb_path: format!(
                        "Project > Commit > {} [hash: {}]",
                        repo_name, short_hash
                    ),
                    launch_command: repo_path.clone(),
                    source: "PROJECT".to_string(),
                    description: format!("Git commit at {}", format_timestamp_local(ts)),
                    synonyms: format!("{} project commit git", message.to_lowercase()),
                },
                score: 8.5,
            });
        }

        // 5. Query clipboard items matching the keyword
        let mut stmt = match conn.prepare("SELECT content, source_app, is_image FROM clipboard_history WHERE content LIKE ? LIMIT 5") {
            Ok(s) => s,
            Err(_) => return results,
        };
        let clips: Vec<(String, String, i32)> = stmt
            .query_map([&kw_pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                ))
            })
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for (content, app, is_image) in clips {
            let display_app = std::path::Path::new(&app)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| app.clone());

            if is_image == 1 {
                let filename = std::path::Path::new(&content)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "image.bmp".to_string());
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("project.clip.{}", content),
                        control_name: format!("🎨 Screenshot (Copied from {})", display_app),
                        breadcrumb_path: format!("Project > Clipboard > Screenshot"),
                        launch_command: format!("copy_image:{}", content),
                        source: "PROJECT".to_string(),
                        description: format!("Project screenshot: {}", filename),
                        synonyms: "project image figma figma_screenshot clipboard".to_string(),
                    },
                    score: 8.0,
                });
            } else {
                let preview =
                    ellipsize_chars(&content.replace("\r\n", " ").replace('\n', " "), 100);
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("project.clip.{}", content),
                        control_name: format!("📋 Clipboard: {}", preview),
                        breadcrumb_path: format!("Project > Clipboard > Text"),
                        launch_command: format!("copy:{}", content),
                        source: "PROJECT".to_string(),
                        description: format!("Text copied from {}", display_app),
                        synonyms: format!("{} project clip", content.to_lowercase()),
                    },
                    score: 7.8,
                });
            }
        }

        // 6. Temporal proximity connections (Worked on simultaneously)
        let mut stmt = match conn.prepare(
            "SELECT timestamp FROM timeline_events WHERE window_title LIKE ? OR app_name LIKE ?",
        ) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let activity_timestamps: Vec<i64> = stmt
            .query_map([&kw_pattern, &kw_pattern], |row| row.get::<_, i64>(0))
            .map(|m| m.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        if !activity_timestamps.is_empty() {
            let mut min_ts = i64::MAX;
            let mut max_ts = i64::MIN;
            for ts in activity_timestamps {
                if ts < min_ts {
                    min_ts = ts;
                }
                if ts > max_ts {
                    max_ts = ts;
                }
            }

            if max_ts >= min_ts {
                let mut stmt = match conn.prepare(
                    "SELECT url, title, source FROM browser_items \
                     WHERE ( \
                         CASE \
                             WHEN last_visit_time > 10000000000000000 THEN (last_visit_time / 1000000) - 11644473600 \
                             WHEN last_visit_time > 10000000000000 THEN last_visit_time / 1000000 \
                             WHEN last_visit_time > 10000000000 THEN last_visit_time / 1000 \
                             ELSE last_visit_time \
                         END \
                     ) >= ? - 600 AND ( \
                         CASE \
                             WHEN last_visit_time > 10000000000000000 THEN (last_visit_time / 1000000) - 11644473600 \
                             WHEN last_visit_time > 10000000000000 THEN last_visit_time / 1000000 \
                             WHEN last_visit_time > 10000000000 THEN last_visit_time / 1000 \
                             ELSE last_visit_time \
                         END \
                     ) <= ? + 600 \
                     AND url NOT IN (SELECT url FROM browser_items WHERE title LIKE ? OR url LIKE ?) \
                     LIMIT 5"
                ) {
                    Ok(s) => s,
                    Err(_) => return results,
                };
                let temp_urls: Vec<(String, String, String)> = stmt
                    .query_map(
                        rusqlite::params![min_ts, max_ts, kw_pattern, kw_pattern],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .map(|m| m.filter_map(|r| r.ok()).collect())
                    .unwrap_or_default();

                for (url, title, _src) in temp_urls {
                    results.push(SearchResult {
                        entry: CatalogEntry {
                            id: format!("project.temp_url.{}", url),
                            control_name: format!("🔗 Context Link: {}", title),
                            breadcrumb_path: format!("Project > Context > Browser > {}", url),
                            launch_command: url,
                            source: "PROJECT".to_string(),
                            description: "Opened during project work window".to_string(),
                            synonyms: format!("{} project context url link", title.to_lowercase()),
                        },
                        score: 7.5,
                    });
                }
            }
        }

        results
    }

    fn clipboard_image_result(
        content: String,
        source_app: String,
        timestamp: i64,
        pinned: i32,
        ocr_text: String,
        score: f32,
    ) -> SearchResult {
        let display_app = std::path::Path::new(&source_app)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| source_app.clone());

        let id = if pinned == 1 {
            format!("clip.pinned.{}", timestamp)
        } else {
            format!("clip.{}", timestamp)
        };

        let filename = std::path::Path::new(&content)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image.bmp".to_string());

        let dims = get_bmp_dimensions(&content);
        let control_name = if let Some((w, h)) = dims {
            format!("[Image] {}x{} (Copied from {})", w, h, display_app)
        } else {
            format!("[Image] Copied from {}", display_app)
        };

        let description = if !ocr_text.is_empty() {
            format!("🔤 {}", ellipsize_chars(&ocr_text, 80))
        } else {
            format!("Image history (Saved as {})", filename)
        };

        SearchResult {
            entry: CatalogEntry {
                id,
                control_name,
                breadcrumb_path: format!("Clipboard > {}", display_app),
                launch_command: format!("copy_image:{}", content),
                source: "CLIPBOARD".to_string(),
                description,
                synonyms: format!(
                    "image {} clipboard copy {}",
                    display_app.to_lowercase(),
                    ocr_text.to_lowercase()
                ),
            },
            score,
        }
    }

    fn search_clipboard_image_ocr_matches(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let q_lower = query.trim().to_lowercase();
        if q_lower.len() < 2 {
            return Vec::new();
        }

        let sql_limit = limit.max(10).min(500) as i64;
        let mut stmt = match self.conn.prepare(
            "SELECT content, source_app, timestamp, pinned, COALESCE(ocr_text, '')
             FROM clipboard_history
             WHERE is_image = 1
               AND COALESCE(ocr_text, '') <> ''
             ORDER BY pinned DESC, timestamp DESC
             LIMIT 500",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map(|rows| {
            rows.filter_map(|row| row.ok())
                .filter(|(_, _, _, _, ocr_text)| ocr_text_matches_query(ocr_text, &q_lower))
                .map(|(content, source_app, timestamp, pinned, ocr_text)| {
                    Self::clipboard_image_result(
                        content, source_app, timestamp, pinned, ocr_text, 42.0,
                    )
                })
                .take(sql_limit as usize)
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn search_clipboard_history(&self, _query: &str) -> Vec<SearchResult> {
        Vec::new()
    }

    pub fn search_files_only(&self, query: &str) -> Vec<SearchResult> {
        // Dedicated file: prefix — include full content search
        self.search_files_generic(query, false, 50, true)
    }

    pub fn search_folders_only(&self, query: &str) -> Vec<SearchResult> {
        // Dedicated folder: prefix — title match only (no FTS content)
        self.search_files_generic(query, false, 50, false)
    }

    pub fn search_code_only(&self, query: &str) -> Vec<SearchResult> {
        // Dedicated code: prefix — include full content search
        self.search_files_generic(query, true, 50, true)
    }

    /// img: / screenshots: prefix — search images by OCR'd text content and filename.
    pub fn search_images_only(&self, query: &str) -> Vec<SearchResult> {
        const IMG_FILTER: &str = "f.extension IN ('png','jpg','jpeg','bmp','gif','webp')";
        let conn = &self.conn;
        let q_lower = query.trim().to_lowercase();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results = Vec::new();

        let mut push_img =
            |path: String, name: String, ext: String, snippet: String, score: f32| {
                let parent = std::path::Path::new(&path)
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let breadcrumb = if snippet.is_empty() {
                    format!("Image > {}", path)
                } else {
                    format!("Image > {} | {}", parent, snippet)
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("file.{}", path),
                        control_name: name.clone(),
                        breadcrumb_path: breadcrumb,
                        launch_command: path,
                        source: "FILE".to_string(),
                        description: format!("{} image — Enter to open", ext.to_uppercase()),
                        synonyms: name.to_lowercase(),
                    },
                    score,
                });
            };

        // Empty query → most recent images
        if q_lower.is_empty() {
            if let Ok(mut stmt) = conn.prepare(
                &format!("SELECT f.path, f.name, f.extension FROM files f WHERE f.is_dir=0 AND {IMG_FILTER} ORDER BY f.modified DESC LIMIT 50")
            ) {
                if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?))) {
                    for (path, name, ext) in rows.filter_map(|r| r.ok()) {
                        if seen.insert(path.clone()) { push_img(path, name, ext, String::new(), 1.0); }
                    }
                }
            }
            return results;
        }

        // Filename match
        let like = format!("%{}%", q_lower);
        if let Ok(mut stmt) = conn.prepare(
            &format!("SELECT f.path, f.name, f.extension FROM files f WHERE f.is_dir=0 AND {IMG_FILTER} AND f.name LIKE ? LIMIT 50")
        ) {
            if let Ok(rows) = stmt.query_map([&like], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?))) {
                for (path, name, ext) in rows.filter_map(|r| r.ok()) {
                    if seen.insert(path.clone()) { push_img(path, name, ext, String::new(), 2.0); }
                }
            }
        }

        // OCR content match via FTS5 (each word as a prefix term)
        let fts_q = q_lower
            .split_whitespace()
            .map(|w| {
                let c: String = w.chars().filter(|ch| ch.is_alphanumeric()).collect();
                format!("{}*", c)
            })
            .filter(|w| w.len() > 1)
            .collect::<Vec<_>>()
            .join(" ");
        if !fts_q.is_empty() {
            if let Ok(mut stmt) = conn.prepare(&format!(
                "SELECT f.path, f.name, f.extension, snippet(files_fts, 1, '', '', '...', 12) \
                          FROM files f JOIN files_fts fts ON f.path = fts.path \
                          WHERE files_fts MATCH ? AND {IMG_FILTER} ORDER BY rank LIMIT 300"
            )) {
                if let Ok(rows) = stmt.query_map([&fts_q], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                }) {
                    for (path, name, ext, snip) in rows.filter_map(|r| r.ok()) {
                        if seen.insert(path.clone()) {
                            let snippet = snip
                                .replace('\n', " ")
                                .replace('\r', " ")
                                .replace('\t', " ")
                                .split_whitespace()
                                .collect::<Vec<&str>>()
                                .join(" ");
                            push_img(path, name, ext, snippet, 1.5);
                        }
                    }
                }
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    fn format_relative_time(ts: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let diff = now - ts;
        if diff < 0 {
            "Just now".to_string()
        } else if diff < 60 {
            format!("{}s ago", diff)
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else if diff < 86400 {
            format!("{}h ago", diff / 3600)
        } else {
            format!("{}d ago", diff / 86400)
        }
    }

    /// chats: prefix — browse stored AI chat history (newest first).
    pub fn search_ai_chats_only(&self, query: &str) -> Vec<SearchResult> {
        let conn = &self.conn;
        let q = query.trim().to_lowercase();
        let mut results = Vec::new();
        let sql = if q.is_empty() {
            "SELECT id, title, prompt, response, ts, command FROM ai_chats WHERE command != 'agent' ORDER BY ts DESC".to_string()
        } else {
            "SELECT id, title, prompt, response, ts, command FROM ai_chats \
             WHERE command != 'agent' AND (lower(title) LIKE ?1 OR lower(prompt) LIKE ?1 OR lower(response) LIKE ?1) \
             ORDER BY ts DESC".to_string()
        };
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let like = format!("%{}%", q);
        let map =
            |row: &rusqlite::Row| -> rusqlite::Result<(i64, String, String, String, i64, String)> {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            };
        let rows = if q.is_empty() {
            stmt.query_map([], map)
        } else {
            stmt.query_map([&like], map)
        };
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return results,
        };
        let mut score = 100.0f32;
        for row in rows.filter_map(|r| r.ok()) {
            let (id, title, prompt, response, ts, command) = row;

            let clean_prompt = prompt.replace('\n', " ").replace('\r', " ");
            let clean_prompt = clean_prompt
                .split_whitespace()
                .collect::<Vec<&str>>()
                .join(" ");
            let prompt_display = if clean_prompt.len() > 65 {
                format!("{}...", clean_prompt.chars().take(65).collect::<String>())
            } else if clean_prompt.is_empty() {
                if title.is_empty() {
                    "Untitled Chat".to_string()
                } else {
                    title
                }
            } else {
                clean_prompt
            };

            let clean_response = response.replace('\n', " ").replace('\r', " ");
            let clean_response = clean_response
                .split_whitespace()
                .collect::<Vec<&str>>()
                .join(" ");
            let response_snippet = if clean_response.len() > 80 {
                format!("{}...", clean_response.chars().take(80).collect::<String>())
            } else if clean_response.is_empty() {
                "(Thinking or empty response)".to_string()
            } else {
                clean_response
            };

            let cmd_upper = command.to_uppercase();
            let breadcrumb = format!("AI Chat [{}] > {}", cmd_upper, response_snippet);
            let time_str = Self::format_relative_time(ts);

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("aichat.{}", id),
                    control_name: prompt_display,
                    breadcrumb_path: breadcrumb,
                    launch_command: format!("aichat:{}", id),
                    source: "AI".to_string(),
                    description: format!("{} | Enter to select & continue talking", time_str),
                    synonyms: "ai chat history conversation".to_string(),
                },
                score,
            });
            score -= 0.01;
        }
        results
    }

    /// agentchats: prefix — browse stored Agent chat history (newest first).
    pub fn search_agent_chats_only(&self, query: &str) -> Vec<SearchResult> {
        let conn = &self.conn;
        let q = query.trim().to_lowercase();
        let mut results = Vec::new();

        // Check if query is targeting a specific agent, to prepend "New Conversation"
        let agent_name_to_check = if q.starts_with('@') {
            q.strip_prefix('@').unwrap_or(&q).trim()
        } else {
            &q
        };
        if !agent_name_to_check.is_empty() {
            if let Some((_id, actual_name)) = self.find_agent_by_name(agent_name_to_check) {
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("newagent.{}", actual_name),
                        control_name: format!("💬 New Conversation with @{}", actual_name),
                        breadcrumb_path: format!(
                            "Agent > Start fresh conversation with {}",
                            actual_name
                        ),
                        launch_command: format!("startnewagent:{}", actual_name),
                        source: "AI_CHAT".to_string(),
                        description: format!("Create a new chat thread with @{}", actual_name),
                        synonyms: format!(
                            "new conversation chat agent {}",
                            actual_name.to_lowercase()
                        ),
                    },
                    score: 110.0,
                });
            }
        }

        let sql = if q.is_empty() {
            "SELECT id, title, prompt, response, ts, command FROM ai_chats ORDER BY ts DESC"
                .to_string()
        } else {
            "SELECT id, title, prompt, response, ts, command FROM ai_chats \
             WHERE (lower(title) LIKE ?1 OR lower(prompt) LIKE ?1 OR lower(response) LIKE ?1) \
             ORDER BY ts DESC"
                .to_string()
        };
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let like = format!("%{}%", q);
        let map =
            |row: &rusqlite::Row| -> rusqlite::Result<(i64, String, String, String, i64, String)> {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            };
        let rows = if q.is_empty() {
            stmt.query_map([], map)
        } else {
            stmt.query_map([&like], map)
        };
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return results,
        };
        let mut score = 100.0f32;
        for row in rows.filter_map(|r| r.ok()) {
            let (id, title, prompt, response, ts, command) = row;

            let clean_prompt = prompt.replace('\n', " ").replace('\r', " ");
            let clean_prompt = clean_prompt
                .split_whitespace()
                .collect::<Vec<&str>>()
                .join(" ");
            let is_agent = command == "agent";
            let default_title = if is_agent {
                "Untitled Agent Run"
            } else {
                "Untitled Chat"
            };
            let prompt_display = if clean_prompt.len() > 65 {
                format!("{}...", clean_prompt.chars().take(65).collect::<String>())
            } else if clean_prompt.is_empty() {
                if title.is_empty() {
                    default_title.to_string()
                } else {
                    title
                }
            } else {
                clean_prompt
            };

            let clean_response = response.replace('\n', " ").replace('\r', " ");
            let clean_response = clean_response
                .split_whitespace()
                .collect::<Vec<&str>>()
                .join(" ");
            let response_snippet = if clean_response.len() > 80 {
                format!("{}...", clean_response.chars().take(80).collect::<String>())
            } else if clean_response.is_empty() {
                "(Thinking or empty response)".to_string()
            } else {
                clean_response
            };

            let cmd_upper = command.to_uppercase();
            let label = if is_agent { "Agent Run" } else { "AI Chat" };
            let breadcrumb = format!("{} [{}] > {}", label, cmd_upper, response_snippet);
            let time_str = Self::format_relative_time(ts);

            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("aichat.{}", id),
                    control_name: prompt_display,
                    breadcrumb_path: breadcrumb,
                    launch_command: format!("aichat:{}", id),
                    source: "AI_CHAT".to_string(),
                    description: format!("{} | Enter to select & continue", time_str),
                    synonyms: "agent chat history run conversation".to_string(),
                },
                score,
            });
            score -= 0.01;
        }
        results
    }

    fn find_agent_by_name(&self, name: &str) -> Option<(i64, String)> {
        self.conn
            .query_row(
                "SELECT id, name FROM agents WHERE lower(name) = lower(?) LIMIT 1",
                [name],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
    }

    /// agents: prefix — browse AI agents. Enter on one starts a message to it.
    pub fn search_agents(&self, query: &str) -> Vec<SearchResult> {
        let conn = &self.conn;
        let q = query.trim().to_lowercase();
        let mut results = Vec::new();
        let sql = if q.is_empty() {
            "SELECT id, name, goal FROM agents ORDER BY ts DESC LIMIT 50".to_string()
        } else {
            "SELECT id, name, goal FROM agents WHERE lower(name) LIKE ?1 OR lower(goal) LIKE ?1 ORDER BY ts DESC LIMIT 50".to_string()
        };
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let like = format!("%{}%", q);
        let map = |row: &rusqlite::Row| -> rusqlite::Result<(i64, String, String)> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        };
        let rows = if q.is_empty() {
            stmt.query_map([], map)
        } else {
            stmt.query_map([&like], map)
        };
        let rows = match rows {
            Ok(r) => r,
            Err(_) => return results,
        };
        let mut score = 100.0f32;
        for row in rows.filter_map(|r| r.ok()) {
            let (id, name, goal) = row;
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("agent.{}", id),
                    control_name: name.clone(),
                    breadcrumb_path: if goal.is_empty() {
                        "Agent > Enter to message".into()
                    } else {
                        format!("Agent > {}", goal)
                    },
                    launch_command: format!("openagent:{}\u{1f}{}", id, name),
                    source: "AI".into(),
                    description: "AI agent — Enter to message".into(),
                    synonyms: format!("agent {}", name.to_lowercase()),
                },
                score,
            });
            score -= 0.01;
        }
        results
    }

    pub fn search_bookmarks_only(&self, sub_query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;

        let q_lower = sub_query.trim().to_lowercase();
        let q_words: Vec<&str> = q_lower.split_whitespace().collect();

        let rows = if q_lower.is_empty() {
            let mut stmt = match conn.prepare(
                "SELECT url, title, source, visit_count FROM browser_items
                 WHERE source LIKE '%bookmark%'
                 ORDER BY visit_count DESC LIMIT 100",
            ) {
                Ok(s) => s,
                Err(_) => return results,
            };
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            });
            match mapped {
                Ok(r) => r.collect::<Vec<_>>(),
                Err(_) => return results,
            }
        } else {
            let name_query = format!("%{}%", q_lower);
            let mut stmt = match conn.prepare(
                "SELECT url, title, source, visit_count FROM browser_items
                 WHERE source LIKE '%bookmark%' AND (title LIKE ?1 OR url LIKE ?1)
                 ORDER BY visit_count DESC LIMIT 100",
            ) {
                Ok(s) => s,
                Err(_) => return results,
            };
            let mapped = stmt.query_map([&name_query], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            });
            match mapped {
                Ok(r) => r.collect::<Vec<_>>(),
                Err(_) => return results,
            }
        };

        for row in rows.into_iter().filter_map(|r| r.ok()) {
            let (url, title, source, visit_count) = row;
            let title_lower = title.to_lowercase();
            let url_lower = url.to_lowercase();

            let score;
            if !q_lower.is_empty() {
                if title_lower == q_lower || url_lower == q_lower {
                    score = 2.0;
                } else if title_lower.starts_with(&q_lower) || url_lower.starts_with(&q_lower) {
                    score = 1.6;
                } else if title_lower.contains(&q_lower) || url_lower.contains(&q_lower) {
                    score = 1.2;
                } else {
                    let matched = q_words
                        .iter()
                        .filter(|w| title_lower.contains(*w) || url_lower.contains(*w))
                        .count();
                    if matched > 0 {
                        score = 0.5 + 0.5 * (matched as f32 / q_words.len() as f32);
                    } else {
                        score = 0.0;
                    }
                }
            } else {
                score = 1.0 + (visit_count as f32).min(100.0) / 100.0;
            }

            if score > 0.0 {
                let browser_name = if source.contains("chrome") {
                    "Chrome"
                } else if source.contains("edge") {
                    "Edge"
                } else if source.contains("brave") {
                    "Brave"
                } else if source.contains("firefox") {
                    "Firefox"
                } else {
                    "Browser"
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("browser.{}", url),
                        control_name: title.clone(),
                        breadcrumb_path: format!("Browser > {}", url),
                        launch_command: url.clone(),
                        source: "BOOKMARK".to_string(),
                        description: format!("Bookmark from {}", browser_name),
                        synonyms: title.to_lowercase(),
                    },
                    score,
                });
            }
        }

        results.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    pub fn search_history_only(&self, sub_query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;

        let q_lower = sub_query.trim().to_lowercase();
        let q_words: Vec<&str> = q_lower.split_whitespace().collect();

        let rows = if q_lower.is_empty() {
            let mut stmt = match conn.prepare(
                "SELECT url, title, source, last_visit_time FROM browser_items
                 WHERE source LIKE '%history%'
                 ORDER BY last_visit_time DESC LIMIT 100",
            ) {
                Ok(s) => s,
                Err(_) => return results,
            };
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            });
            match mapped {
                Ok(r) => r.collect::<Vec<_>>(),
                Err(_) => return results,
            }
        } else {
            let name_query = format!("%{}%", q_lower);
            let mut stmt = match conn.prepare(
                "SELECT url, title, source, last_visit_time FROM browser_items
                 WHERE source LIKE '%history%' AND (title LIKE ?1 OR url LIKE ?1)
                 ORDER BY last_visit_time DESC LIMIT 100",
            ) {
                Ok(s) => s,
                Err(_) => return results,
            };
            let mapped = stmt.query_map([&name_query], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            });
            match mapped {
                Ok(r) => r.collect::<Vec<_>>(),
                Err(_) => return results,
            }
        };

        for row in rows.into_iter().filter_map(|r| r.ok()) {
            let (url, title, source, _last_visit_time) = row;
            let title_lower = title.to_lowercase();
            let url_lower = url.to_lowercase();

            let score;
            if !q_lower.is_empty() {
                if title_lower == q_lower || url_lower == q_lower {
                    score = 2.0;
                } else if title_lower.starts_with(&q_lower) || url_lower.starts_with(&q_lower) {
                    score = 1.6;
                } else if title_lower.contains(&q_lower) || url_lower.contains(&q_lower) {
                    score = 1.2;
                } else {
                    let matched = q_words
                        .iter()
                        .filter(|w| title_lower.contains(*w) || url_lower.contains(*w))
                        .count();
                    if matched > 0 {
                        score = 0.5 + 0.5 * (matched as f32 / q_words.len() as f32);
                    } else {
                        score = 0.0;
                    }
                }
            } else {
                score = 1.0;
            }

            if score > 0.0 {
                let browser_name = if source.contains("chrome") {
                    "Chrome"
                } else if source.contains("edge") {
                    "Edge"
                } else if source.contains("brave") {
                    "Brave"
                } else if source.contains("firefox") {
                    "Firefox"
                } else {
                    "Browser"
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("browser.{}", url),
                        control_name: title.clone(),
                        breadcrumb_path: format!("Browser > {}", url),
                        launch_command: url.clone(),
                        source: "HISTORY".to_string(),
                        description: format!("History from {}", browser_name),
                        synonyms: title.to_lowercase(),
                    },
                    score,
                });
            }
        }

        results
    }

    pub fn search_commits_only(&self, _sub_query: &str) -> Vec<SearchResult> {
        Vec::new()
    }

    pub fn search_todos_only(&self, _sub_query: &str) -> Vec<SearchResult> {
        Vec::new()
    }

    // ponytail: this 1350-line method is a God object. Skipped decomposition because it works fine and splitting it adds overhead and indirection. Decompose when a new search source actually breaks it.
    /// lean-build: only surface the curated feature set (files/OCR/content, settings &
    /// control panel, apps, git commits, browser history & bookmarks, AI agents & history).
    /// Everything else (clipboard, notes, todos, snippets, quicklinks, window mgmt, system
    /// actions, calculator, web search, AI text commands) is produced by search_raw but
    /// filtered out here. ponytail: one gate beats deleting thousands of lines.
    pub fn search(&mut self, query: &str, top_k: usize) -> Vec<SearchResult> {
        self.search_with_fts(query, top_k, true)
    }

    pub fn search_with_fts(
        &mut self,
        query: &str,
        top_k: usize,
        with_fts: bool,
    ) -> Vec<SearchResult> {
        let mut results = self.search_raw_with_fts(query, top_k, with_fts);
        results.retain(lean_allowed);
        let plugin_settings = crate::settings::AppSettings::load();
        results.retain(|r| plugin_allowed(r, &plugin_settings));
        if results.is_empty() {
            if let Some(empty) = empty_scope_result(query) {
                return vec![empty];
            }
        }
        results
    }

    fn search_raw_with_fts(
        &mut self,
        query: &str,
        top_k: usize,
        with_fts: bool,
    ) -> Vec<SearchResult> {
        self.search_raw_with_fts_inner(query, top_k, with_fts)
    }

    fn search_raw_with_fts_inner(
        &mut self,
        query: &str,
        top_k: usize,
        with_fts: bool,
    ) -> Vec<SearchResult> {
        let candidate_k = candidate_limit(top_k);
        // Rebuild the in-memory file index once a background indexing pass finishes, so newly
        // indexed files become searchable without restarting the app.
        let now_indexing = crate::indexer::IS_INDEXING.load(std::sync::atomic::Ordering::Relaxed);
        if self.was_indexing && !now_indexing {
            self.file_index = Self::build_file_index(&self.conn);
        }
        self.was_indexing = now_indexing;

        let q = query.trim();
        let q_lower_trimmed = q.to_lowercase();

        if !with_fts {
            // FAST SEARCH PATH: Only Apps, Recent Files, and Files/Folders by title
            let _q_clean = q.to_string();
            let q_lower = q_lower_trimmed.clone();
            let q_words: Vec<&str> = q_lower.split_whitespace().collect();

            // 1. Native Settings / Control Panel matches from FTS5.
            let mut settings_matches =
                self.search_settings_catalog_fts(&q_lower, candidate_k.max(30));

            // 2. App matches
            let mut app_matches = Vec::new();
            for app in &self.apps {
                let app_lower = app.name.to_lowercase();
                let mut score = 0.0f32;
                if app_lower == q_lower {
                    score = 120.0;
                } else if app_lower.starts_with(&q_lower) && q_lower.chars().count() >= 2 {
                    score = 118.0;
                } else if app_lower.starts_with(&q_lower) {
                    score = 116.0;
                } else if q_lower.starts_with(&app_lower) {
                    score = 114.0;
                } else if app_lower.contains(&q_lower) {
                    score = 112.0;
                } else if q_lower.contains(&app_lower) {
                    score = 110.0;
                } else {
                    let app_words: Vec<&str> = app_lower.split_whitespace().collect();
                    let mut matched = 0;
                    for qw in &q_words {
                        if app_words.contains(qw) {
                            matched += 1;
                        }
                    }
                    if matched > 0 && !q_words.is_empty() {
                        let ratio = matched as f32 / q_words.len() as f32;
                        if ratio >= 0.5 {
                            score = 100.0 + ratio;
                        }
                    }
                }
                if score > 0.0 {
                    app_matches.push(SearchResult {
                        entry: CatalogEntry {
                            id: format!("app.{}", app.name),
                            control_name: app.name.clone(),
                            breadcrumb_path: format!("Applications > {}", app.name),
                            launch_command: format!("shell:AppsFolder\\{}", app.path),
                            source: "app".to_string(),
                            description: format!("Launch {}", app.name),
                            synonyms: app.name.to_lowercase(),
                        },
                        score,
                    });
                }
            }
            app_matches.sort_unstable_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // 3. Recent matches
            let mut recent_matches = Vec::new();
            for rf in &self.recent_files {
                let name_lower = rf.name.to_lowercase();
                let name_no_ext = if let Some(dot) = name_lower.rfind('.') {
                    &name_lower[..dot]
                } else {
                    &name_lower
                };
                let mut score = 0.0f32;
                if name_lower == q_lower || name_no_ext == q_lower {
                    score = 89.0;
                } else if name_lower.starts_with(&q_lower) || name_no_ext.starts_with(&q_lower) {
                    score = 86.0;
                } else if name_lower.contains(&q_lower) {
                    score = 82.0;
                } else {
                    let name_words: Vec<&str> = name_no_ext
                        .split(|c: char| !c.is_alphanumeric())
                        .filter(|w| !w.is_empty())
                        .collect();
                    let mut matched = 0;
                    for qw in &q_words {
                        if name_words.contains(qw) {
                            matched += 1;
                        }
                    }
                    if matched > 0 && !q_words.is_empty() {
                        let ratio = matched as f32 / q_words.len() as f32;
                        if ratio >= 0.5 {
                            score = 78.0 + ratio;
                        }
                    }
                }
                if score > 0.0 {
                    let ext = rf.name.rsplit('.').next().unwrap_or("").to_uppercase();
                    recent_matches.push(SearchResult {
                        entry: CatalogEntry {
                            id: format!("recent.{}", rf.path),
                            control_name: rf.name.clone(),
                            breadcrumb_path: format!("Recent > {}", rf.path),
                            launch_command: rf.path.clone(),
                            source: "RECENT".to_string(),
                            description: format!("Recently opened {} file", ext),
                            synonyms: rf.name.to_lowercase(),
                        },
                        score,
                    });
                }
            }
            recent_matches.sort_unstable_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // 4. File matches by title
            let mut file_matches = self.search_local_files_with_fts(&q_lower, false);
            file_matches.sort_unstable_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for m in &mut file_matches {
                m.score += 70.0;
            }

            // Merge and return
            let mut merged = Vec::new();
            merged.append(&mut settings_matches);
            merged.append(&mut app_matches);
            merged.append(&mut recent_matches);
            merged.append(&mut file_matches);
            merged.sort_unstable_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // Deduplicate
            let mut unique_results = Vec::new();
            let mut seen_ids = std::collections::HashSet::new();
            let mut seen_launches = std::collections::HashSet::new();
            for r in merged {
                let launch_key = result_launch_dedupe_key(&r.entry);
                let is_duplicate = seen_ids.contains(&r.entry.id)
                    || launch_key
                        .as_ref()
                        .is_some_and(|key| seen_launches.contains(key));
                if !is_duplicate {
                    seen_ids.insert(r.entry.id.clone());
                    if let Some(key) = launch_key {
                        seen_launches.insert(key);
                    }
                    unique_results.push(r);
                }
            }
            unique_results.truncate(candidate_k);
            return unique_results;
        }

        // ── Phase 3: System Toggles & Audio Controls ─────────────────────────
        if q_lower_trimmed.starts_with("volume ") || q_lower_trimmed.starts_with("vol ") {
            let num_str = if q_lower_trimmed.starts_with("volume ") {
                q_lower_trimmed.strip_prefix("volume ").unwrap().trim()
            } else {
                q_lower_trimmed.strip_prefix("vol ").unwrap().trim()
            };
            if let Ok(pct) = num_str.parse::<u32>() {
                if pct <= 100 {
                    return vec![SearchResult {
                        entry: CatalogEntry {
                            id: format!("action.volume.{}", pct),
                            control_name: format!("Set Master Volume to {}%", pct),
                            breadcrumb_path: "System > Audio > Set master volume".to_string(),
                            launch_command: format!("action:volume:{}", pct),
                            source: "ACTION".to_string(),
                            description: format!("Set system volume level to {} percent.", pct),
                            synonyms: format!("volume vol volume{} vol{}", pct, pct),
                        },
                        score: 11.5,
                    }];
                }
            }
        }
        if q_lower_trimmed == "mute" {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.audio.mute".to_string(),
                    control_name: "Mute Audio".to_string(),
                    breadcrumb_path: "System > Audio > Mute sound output".to_string(),
                    launch_command: "action:mute".to_string(),
                    source: "ACTION".to_string(),
                    description: "Mute master sound session.".to_string(),
                    synonyms: "mute volume vol audio silence quiet".to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "unmute" {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.audio.unmute".to_string(),
                    control_name: "Unmute Audio".to_string(),
                    breadcrumb_path: "System > Audio > Unmute sound output".to_string(),
                    launch_command: "action:unmute".to_string(),
                    source: "ACTION".to_string(),
                    description: "Restore master sound session volume.".to_string(),
                    synonyms: "unmute volume vol audio sound speak loud".to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "toggle hidden files"
            || q_lower_trimmed == "show hidden files"
            || q_lower_trimmed == "hide hidden files"
            || q_lower_trimmed == "hidden files"
        {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.explorer.hidden_files".to_string(),
                    control_name: "Toggle Hidden Files".to_string(),
                    breadcrumb_path: "System > Explorer > Show or hide hidden files".to_string(),
                    launch_command: "action:toggle_hidden_files".to_string(),
                    source: "ACTION".to_string(),
                    description:
                        "Toggle visibility of hidden files and folders in Windows Explorer."
                            .to_string(),
                    synonyms: "hidden files folders show hidden toggle registry explorer"
                        .to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "play"
            || q_lower_trimmed == "pause"
            || q_lower_trimmed == "play/pause"
            || q_lower_trimmed == "play pause"
        {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.media.play_pause".to_string(),
                    control_name: "Play/Pause Media".to_string(),
                    breadcrumb_path: "System > Media > Media playback control".to_string(),
                    launch_command: "action:media:play_pause".to_string(),
                    source: "ACTION".to_string(),
                    description: "Toggle playback state of media players.".to_string(),
                    synonyms: "play pause music video media track toggle".to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "next"
            || q_lower_trimmed == "next track"
            || q_lower_trimmed == "skip track"
        {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.media.next".to_string(),
                    control_name: "Next Track".to_string(),
                    breadcrumb_path: "System > Media > Media playback control".to_string(),
                    launch_command: "action:media:next".to_string(),
                    source: "ACTION".to_string(),
                    description: "Skip to the next media track.".to_string(),
                    synonyms: "next track skip song music media forward".to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "prev"
            || q_lower_trimmed == "previous"
            || q_lower_trimmed == "prev track"
            || q_lower_trimmed == "previous track"
        {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.media.prev".to_string(),
                    control_name: "Previous Track".to_string(),
                    breadcrumb_path: "System > Media > Media playback control".to_string(),
                    launch_command: "action:media:prev".to_string(),
                    source: "ACTION".to_string(),
                    description: "Return to the previous media track.".to_string(),
                    synonyms: "prev previous track song music media backward rewind".to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "stop"
            || q_lower_trimmed == "stop track"
            || q_lower_trimmed == "stop music"
        {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.media.stop".to_string(),
                    control_name: "Stop Playback".to_string(),
                    breadcrumb_path: "System > Media > Media playback control".to_string(),
                    launch_command: "action:media:stop".to_string(),
                    source: "ACTION".to_string(),
                    description: "Stop media playback.".to_string(),
                    synonyms: "stop track song music media halt".to_string(),
                },
                score: 11.5,
            }];
        }
        if q_lower_trimmed == "night light"
            || q_lower_trimmed == "nightlight"
            || q_lower_trimmed == "toggle night light"
        {
            return vec![SearchResult {
                entry: CatalogEntry {
                    id: "action.display.night_light".to_string(),
                    control_name: "Night Light Settings".to_string(),
                    breadcrumb_path: "System > Display > Night Light settings".to_string(),
                    launch_command: "ms-settings:nightlight".to_string(),
                    source: "ACTION".to_string(),
                    description: "Open the display settings page to toggle or adjust Night Light."
                        .to_string(),
                    synonyms:
                        "night light settings display blue reduction color screen temp warmth"
                            .to_string(),
                },
                score: 11.5,
            }];
        }

        if matches!(
            q_lower_trimmed.as_str(),
            "memory"
                | "memories"
                | "what do you remember"
                | "what does my pc remember"
                | "what does windows remember"
        ) {
            return self.search_memory_home();
        }

        if let Some(days_ago) = workday_memory_query_days(&q_lower_trimmed) {
            return self.search_workday_memory_summary(days_ago);
        }

        if matches!(
            q_lower_trimmed.as_str(),
            "continue last session"
                | "continue my last session"
                | "continue my last coding session"
                | "last session"
        ) {
            return self.search_last_memory_session();
        }

        // Intercept temporal context queries (e.g. yesterday before lunch)
        if let Some((start_time, end_time, clean_q)) = parse_time_range(q) {
            return self.search_timeline(start_time, end_time, &clean_q);
        }

        // Intercept sequential queries (e.g. "after chrome yesterday", "before vscode today")
        if let Some((anchor, direction, start_time, end_time)) = parse_sequential_query(q) {
            let time_range = if start_time > 0 && end_time > 0 {
                (start_time, end_time)
            } else {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let local_time =
                    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
                let seconds_since_midnight = (local_time.wHour as i64 * 3600)
                    + (local_time.wMinute as i64 * 60)
                    + local_time.wSecond as i64;
                let today_start = now - seconds_since_midnight;
                (today_start - 7 * 86400, now)
            };
            return self.search_timeline_sequential(
                &anchor,
                &direction,
                time_range.0,
                time_range.1,
            );
        }

        if q.is_empty() {
            let mut results = Vec::new();
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.memory".to_string(),
                    control_name: "Your Windows PC doesn't forget".to_string(),
                    breadcrumb_path: "Memory > Local on this PC".to_string(),
                    launch_command: "memory:".to_string(),
                    source: "MEMORY".to_string(),
                    description: "Open everything MemoryOS has captured locally.".to_string(),
                    synonyms: "memory remembers timeline today yesterday computer pc".to_string(),
                },
                score: 6.0,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.bookmarks".to_string(),
                    control_name: "Browser Bookmarks".to_string(),
                    breadcrumb_path: "Browser > Bookmarks".to_string(),
                    launch_command: "bookmarks:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing all browser bookmarks".to_string(),
                    synonyms: "bookmarks folders browser favs favorites stars".to_string(),
                },
                score: 5.0,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.history".to_string(),
                    control_name: "Browser History".to_string(),
                    breadcrumb_path: "Browser > History".to_string(),
                    launch_command: "history:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing all browser history".to_string(),
                    synonyms: "history folders browser recent urls websites web".to_string(),
                },
                score: 4.5,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.commits".to_string(),
                    control_name: "Git Commits".to_string(),
                    breadcrumb_path: "Git > Commits".to_string(),
                    launch_command: "commits:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing recent git commits".to_string(),
                    synonyms: "git commits codes changes hashes log repo history".to_string(),
                },
                score: 4.0,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.todos".to_string(),
                    control_name: "Git TODOs".to_string(),
                    breadcrumb_path: "Git > TODOs".to_string(),
                    launch_command: "todos:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing active TODO and FIXME comments".to_string(),
                    synonyms: "git todos tasks comment code fixme bug".to_string(),
                },
                score: 3.5,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.clipboard".to_string(),
                    control_name: "Clipboard History".to_string(),
                    breadcrumb_path: "Clipboard > History".to_string(),
                    launch_command: "clip:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing clipboard history".to_string(),
                    synonyms: "clipboard history copy paste clip".to_string(),
                },
                score: 3.4,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.focus".to_string(),
                    control_name: "Focus Modes".to_string(),
                    breadcrumb_path: "Focus > Modes".to_string(),
                    launch_command: "focus:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing your Focus Modes".to_string(),
                    synonyms: "focus modes categories pomodoro dnd".to_string(),
                },
                score: 3.45,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.games".to_string(),
                    control_name: "Steam Games".to_string(),
                    breadcrumb_path: "Games > Steam".to_string(),
                    launch_command: "games:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing installed Steam games".to_string(),
                    synonyms: "games steam play".to_string(),
                },
                score: 3.42,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.notes".to_string(),
                    control_name: "Notes".to_string(),
                    breadcrumb_path: "Notes > Browse".to_string(),
                    launch_command: "notes:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing your saved text notes".to_string(),
                    synonyms: "notes text files read edit browse".to_string(),
                },
                score: 3.43,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.files".to_string(),
                    control_name: "Local Files".to_string(),
                    breadcrumb_path: "Local > Files".to_string(),
                    launch_command: "file:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing indexed documents and files".to_string(),
                    synonyms: "files documents local downloads desktop search pdf docx".to_string(),
                },
                score: 3.3,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.code".to_string(),
                    control_name: "Source Code".to_string(),
                    breadcrumb_path: "Local > Source Code".to_string(),
                    launch_command: "code:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing indexed source code files".to_string(),
                    synonyms: "code source rust python js cpp java develop program".to_string(),
                },
                score: 3.2,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.images".to_string(),
                    control_name: "Search Screenshots".to_string(),
                    breadcrumb_path: "Local > Image Text (OCR)".to_string(),
                    launch_command: "img:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Search text inside images and screenshots (OCR)".to_string(),
                    synonyms: "image images screenshot screenshots ocr photo picture text inside"
                        .to_string(),
                },
                score: 3.15,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.agentchats".to_string(),
                    control_name: "History".to_string(),
                    breadcrumb_path: "AI > History".to_string(),
                    launch_command: "agentchats:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Browse and reopen your past AI chats and agent runs".to_string(),
                    synonyms: "chats history runs conversations past previous saved"
                        .to_string(),
                },
                score: 3.08,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.agents".to_string(),
                    control_name: "AI Agents".to_string(),
                    breadcrumb_path: "AI > Agents".to_string(),
                    launch_command: "agents:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Create and message persistent AI agents".to_string(),
                    synonyms: "ai agents agent bot assistant create".to_string(),
                },
                score: 3.05,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.switch".to_string(),
                    control_name: "Window Switcher".to_string(),
                    breadcrumb_path: "Window > Switcher".to_string(),
                    launch_command: "switch:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Switch to any running application window".to_string(),
                    synonyms: "switch windows alt tab task active running switcher".to_string(),
                },
                score: 3.1,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.quicklinks".to_string(),
                    control_name: "Quicklinks".to_string(),
                    breadcrumb_path: "Quicklinks > Web shortcuts".to_string(),
                    launch_command: "ql:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing all custom web search shortcuts".to_string(),
                    synonyms: "quicklinks ql web shortcuts links".to_string(),
                },
                score: 3.05,
            });
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "folder.snippets".to_string(),
                    control_name: "Snippets".to_string(),
                    breadcrumb_path: "Snippets > Text templates".to_string(),
                    launch_command: "snip:".to_string(),
                    source: "FOLDER".to_string(),
                    description: "Folder containing all text snippet templates".to_string(),
                    synonyms: "snippets snip templates text patterns".to_string(),
                },
                score: 3.01,
            });
            for rf in self.recent_files.iter().take(top_k.min(20)) {
                let ext = rf.name.rsplit('.').next().unwrap_or("").to_uppercase();
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("recent.{}", rf.path),
                        control_name: rf.name.clone(),
                        breadcrumb_path: format!("Recent > {}", rf.path),
                        launch_command: rf.path.clone(),
                        source: "RECENT".to_string(),
                        description: format!("Recently opened {} file", ext),
                        synonyms: rf.name.to_lowercase(),
                    },
                    score: 3.0,
                });
            }
            return results;
        }

        if q_lower_trimmed.starts_with("quicklink:") {
            let sub_query = q_lower_trimmed.strip_prefix("quicklink:").unwrap().trim();
            return self.search_quicklinks_only(sub_query);
        }
        if q_lower_trimmed.starts_with("ql:") {
            let sub_query = q_lower_trimmed.strip_prefix("ql:").unwrap().trim();
            return self.search_quicklinks_only(sub_query);
        }
        if q_lower_trimmed.starts_with("snippet:") {
            let sub_query = q_lower_trimmed.strip_prefix("snippet:").unwrap().trim();
            return self.search_snippets_only(sub_query);
        }
        if q_lower_trimmed.starts_with("snip:") {
            let sub_query = q_lower_trimmed.strip_prefix("snip:").unwrap().trim();
            return self.search_snippets_only(sub_query);
        }
        // Quicklink keyword detection in general search
        let query_trimmed = q.trim();
        if !query_trimmed.is_empty() {
            let mut parts = query_trimmed.splitn(2, |c: char| c.is_whitespace());
            if let Some(first_word) = parts.next() {
                let rest_query = parts.next().unwrap_or("").trim();
                let has_space = query_trimmed.contains(|c: char| c.is_whitespace());
                if has_space {
                    if let Some((ql_name, ql_url)) = self.check_quicklink_keyword(first_word) {
                        let encoded = url_encode(rest_query);
                        let final_url = ql_url.replace("{query}", &encoded);
                        let display_rest = if rest_query.is_empty() {
                            "..."
                        } else {
                            rest_query
                        };
                        return vec![SearchResult {
                            entry: CatalogEntry {
                                id: format!(
                                    "quicklink_trigger.{}",
                                    ql_name.to_lowercase().replace(' ', "_")
                                ),
                                control_name: format!(
                                    "Search {} for \"{}\"",
                                    ql_name, display_rest
                                ),
                                breadcrumb_path: format!("Quicklink > Open in default browser"),
                                launch_command: format!("open_quicklink:{}", final_url),
                                source: "QUICKLINK".to_string(),
                                description: format!("Perform quick search on {}", ql_name),
                                synonyms: first_word.to_string(),
                            },
                            score: 12.0,
                        }];
                    }
                } else if let Some((name, content, keyword)) =
                    self.check_snippet_keyword(first_word)
                {
                    return vec![SearchResult {
                        entry: CatalogEntry {
                            id: format!(
                                "snippet_trigger.{}",
                                name.to_lowercase().replace(' ', "_")
                            ),
                            control_name: format!("Expand {}", name),
                            breadcrumb_path: format!("Snippet [{}] > Copy", keyword),
                            launch_command: format!("copy_snippet:{}", content),
                            source: "SNIPPET".to_string(),
                            description: ellipsize_chars(&content, 63),
                            synonyms: keyword,
                        },
                        score: 12.0,
                    }];
                }
            }
        }

        if q_lower_trimmed == "recent" || q_lower_trimmed == "recents" {
            let mut results = Vec::new();
            for rf in self.recent_files.iter().take(top_k.min(20)) {
                let ext = rf.name.rsplit('.').next().unwrap_or("").to_uppercase();
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("recent.{}", rf.path),
                        control_name: rf.name.clone(),
                        breadcrumb_path: format!("Recent > {}", rf.path),
                        launch_command: rf.path.clone(),
                        source: "RECENT".to_string(),
                        description: format!("Recently opened {} file", ext),
                        synonyms: rf.name.to_lowercase(),
                    },
                    score: 3.0,
                });
            }
            return results;
        }
        if q_lower_trimmed.starts_with("switch:") {
            let sub_query = q_lower_trimmed.strip_prefix("switch:").unwrap().trim();
            return self.search_windows(sub_query);
        }

        if q_lower_trimmed.starts_with("window:") {
            let sub_query = q_lower_trimmed.strip_prefix("window:").unwrap().trim();
            return self.search_windows(sub_query);
        }

        if q_lower_trimmed.starts_with("clipboard:") {
            let sub_query = q_lower_trimmed.strip_prefix("clipboard:").unwrap().trim();
            return self.search_clipboard_history(sub_query);
        }

        if q_lower_trimmed.starts_with("clip:") {
            let sub_query = q_lower_trimmed.strip_prefix("clip:").unwrap().trim();
            return self.search_clipboard_history(sub_query);
        }

        if q_lower_trimmed.starts_with("bookmarks:") {
            let sub_query = q_lower_trimmed.strip_prefix("bookmarks:").unwrap().trim();
            return self.search_bookmarks_only(sub_query);
        }

        if q_lower_trimmed.starts_with("history:") {
            let sub_query = q_lower_trimmed.strip_prefix("history:").unwrap().trim();
            return self.search_history_only(sub_query);
        }

        if q_lower_trimmed.starts_with("commits:") {
            let sub_query = q_lower_trimmed.strip_prefix("commits:").unwrap().trim();
            return self.search_commits_only(sub_query);
        }

        if q_lower_trimmed.starts_with("focus:") {
            let sub_query = q_lower_trimmed.strip_prefix("focus:").unwrap().trim();
            return self.search_focus_categories(sub_query);
        }
        if q_lower_trimmed.starts_with("notes:") {
            let sub_query = q_lower_trimmed.strip_prefix("notes:").unwrap().trim();
            return self.search_notes(sub_query);
        }
        if q_lower_trimmed.starts_with("memory:") {
            let sub_query = q_lower_trimmed.strip_prefix("memory:").unwrap().trim();
            return self.search_memory_events(sub_query);
        }
        if q_lower_trimmed.starts_with("games:") {
            let sub_query = q_lower_trimmed.strip_prefix("games:").unwrap().trim();
            return self.search_games(sub_query);
        }
        if q_lower_trimmed.starts_with("todos:") {
            let sub_query = q_lower_trimmed.strip_prefix("todos:").unwrap().trim();
            return self.search_todos_only(sub_query);
        }

        if q_lower_trimmed.starts_with("file:") {
            let sub_query = q_lower_trimmed.strip_prefix("file:").unwrap().trim();
            return self.search_files_only(sub_query);
        }

        if q_lower_trimmed.starts_with("folder:") {
            let sub_query = q_lower_trimmed.strip_prefix("folder:").unwrap().trim();
            return self.search_folders_only(sub_query);
        }

        if q_lower_trimmed.starts_with("code:") {
            let sub_query = q_lower_trimmed.strip_prefix("code:").unwrap().trim();
            return self.search_code_only(sub_query);
        }

        for p in ["img:", "image:", "screenshots:", "screenshot:", "ocr:"] {
            if let Some(sub) = q_lower_trimmed.strip_prefix(p) {
                return self.search_images_only(sub.trim());
            }
        }

        if let Some(sub) = q_lower_trimmed.strip_prefix("chats:") {
            return self.search_ai_chats_only(sub.trim());
        }

        if let Some(sub) = q_lower_trimmed.strip_prefix("agentchats:") {
            return self.search_agent_chats_only(sub.trim());
        }

        if let Some(sub) = q_lower_trimmed.strip_prefix("agents:") {
            return self.search_agents(sub.trim());
        }

        // ── AI browser prefixes: "<provider> <prompt>" opens the AI with the ──
        // prompt prefilled via query parameter. (prefix, label, url-before-encoded-prompt)
        const AI_PREFIXES: &[(&str, &str, &str)] = &[
            ("chatgpt", "ChatGPT", "https://chatgpt.com/?q="),
            ("claude", "Claude AI", "https://claude.ai/new?q="),
            ("grok", "Grok", "https://grok.com/?q="),
            ("eyux", "EyuX AI", "https://eyux.com.et?prompt="),
            ("gemini", "Gemini", "https://gemini.google.com/"),
        ];
        for (prefix, label, url) in AI_PREFIXES {
            let lead = [format!("{} ", prefix), format!("{}:", prefix)]
                .into_iter()
                .find(|lead| q_lower_trimmed.starts_with(lead));
            if let Some(lead) = lead {
                let prompt = q.trim()[lead.len()..].trim();
                let launch_command = if prompt.is_empty() {
                    if let Some(base) = url.strip_suffix("?q=") {
                        base.to_string()
                    } else if let Some(base) = url.strip_suffix("?prompt=") {
                        base.to_string()
                    } else {
                        url.to_string()
                    }
                } else {
                    format!("{}{}", url, url_encode(prompt))
                };
                return vec![SearchResult {
                    entry: CatalogEntry {
                        id: format!("{}_search", prefix),
                        control_name: if prompt.is_empty() {
                            format!("Open {}", label)
                        } else {
                            format!("{}: {}", label, prompt)
                        },
                        breadcrumb_path: format!("{} > Ask AI > Opens in default browser", label),
                        launch_command,
                        source: "LIVE".to_string(),
                        description: if prompt.is_empty() {
                            format!("Open {} in your default browser", label)
                        } else {
                            format!("Send '{}' to {}", prompt, label)
                        },
                        synonyms: format!("{} ai chat ask", prefix),
                    },
                    score: 10.0,
                }];
            }
        }



        // ── AI config: Settings and credentials ──────────────────────────────────────
        {
            let lt = q_lower_trimmed.as_str();
            let qt = q.trim();
            if lt == "ai config" {
                let mut key_masked = "Not set".to_string();
                let mut endpoint_val = "Default (DeepSeek)".to_string();
                let mut model_val = "Default (auto from provider)".to_string();
                let conn = &self.conn;
                if let Ok(val) = conn.query_row(
                    "SELECT value FROM ai_settings WHERE key = 'api_key'",
                    [],
                    |row| row.get::<_, String>(0),
                ) {
                    if val.chars().count() > 8 {
                        key_masked =
                            format!("{}...{}", take_chars(&val, 4), take_last_chars(&val, 4));
                    } else if !val.is_empty() {
                        key_masked = "****".to_string();
                    }
                }
                if let Ok(val) = conn.query_row(
                    "SELECT value FROM ai_settings WHERE key = 'endpoint'",
                    [],
                    |row| row.get::<_, String>(0),
                ) {
                    if !val.is_empty() {
                        endpoint_val = val;
                    }
                }
                if let Ok(val) = conn.query_row(
                    "SELECT value FROM ai_settings WHERE key = 'model'",
                    [],
                    |row| row.get::<_, String>(0),
                ) {
                    if !val.is_empty() {
                        model_val = val;
                    }
                }
                return vec![
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.preset.gemini".to_string(),
                            control_name: "Apply Google Gemini Preset".to_string(),
                            breadcrumb_path: "AI Config > Set endpoint and model for Google Gemini".to_string(),
                            launch_command: "action:ai_config:preset:gemini".to_string(),
                            source: "AI".to_string(),
                            description: "Free key at aistudio.google.com/apikey".to_string(),
                            synonyms: "ai config preset gemini google".to_string(),
                        },
                        score: 101.0,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.preset.openai".to_string(),
                            control_name: "Apply OpenAI Preset".to_string(),
                            breadcrumb_path: "AI Config > Set endpoint and model for OpenAI".to_string(),
                            launch_command: "action:ai_config:preset:openai".to_string(),
                            source: "AI".to_string(),
                            description: "Create key at platform.openai.com/api-keys".to_string(),
                            synonyms: "ai config preset openai gpt chatgpt".to_string(),
                        },
                        score: 100.9,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.preset.claude".to_string(),
                            control_name: "Apply Anthropic Claude Preset".to_string(),
                            breadcrumb_path: "AI Config > Set endpoint and model for Claude".to_string(),
                            launch_command: "action:ai_config:preset:claude".to_string(),
                            source: "AI".to_string(),
                            description: "Get key at console.anthropic.com/settings/keys".to_string(),
                            synonyms: "ai config preset claude anthropic".to_string(),
                        },
                        score: 100.8,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.preset.grok".to_string(),
                            control_name: "Apply xAI Grok Preset".to_string(),
                            breadcrumb_path: "AI Config > Set endpoint and model for Grok".to_string(),
                            launch_command: "action:ai_config:preset:grok".to_string(),
                            source: "AI".to_string(),
                            description: "Get key at console.x.ai".to_string(),
                            synonyms: "ai config preset grok xai".to_string(),
                        },
                        score: 100.7,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.preset.deepseek".to_string(),
                            control_name: "Apply DeepSeek Preset".to_string(),
                            breadcrumb_path: "AI Config > Set endpoint and model for DeepSeek".to_string(),
                            launch_command: "action:ai_config:preset:deepseek".to_string(),
                            source: "AI".to_string(),
                            description: "Press Enter to configure endpoint and model for DeepSeek".to_string(),
                            synonyms: "ai config preset deepseek".to_string(),
                        },
                        score: 100.5,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.key".to_string(),
                            control_name: "Set AI API Key".to_string(),
                            breadcrumb_path: format!("AI Config > Key: {}", key_masked),
                            launch_command: "query:ai config key ".to_string(),
                            source: "AI".to_string(),
                            description: "Type 'ai config key <API_KEY>' and press Enter to save".to_string(),
                            synonyms: "ai config setting key apikey gemini openai claude grok deepseek".to_string(),
                        },
                        score: 100.0,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.endpoint".to_string(),
                            control_name: "Set AI Endpoint URL".to_string(),
                            breadcrumb_path: format!("AI Config > Endpoint: {}", endpoint_val),
                            launch_command: "query:ai config endpoint ".to_string(),
                            source: "AI".to_string(),
                            description: "Type 'ai config endpoint <URL>' and press Enter to save".to_string(),
                            synonyms: "ai config endpoint url baseurl".to_string(),
                        },
                        score: 99.0,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.model".to_string(),
                            control_name: "Set AI Model Name".to_string(),
                            breadcrumb_path: format!("AI Config > Model: {}", model_val),
                            launch_command: "query:ai config model ".to_string(),
                            source: "AI".to_string(),
                            description: "Type 'ai config model <MODEL_NAME>' and press Enter to save".to_string(),
                            synonyms: "ai config model name".to_string(),
                        },
                        score: 98.0,
                    },
                    SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.reset".to_string(),
                            control_name: "Reset AI Configuration".to_string(),
                            breadcrumb_path: "AI Config > Clear all custom values".to_string(),
                            launch_command: "action:ai_config:reset".to_string(),
                            source: "AI".to_string(),
                            description: "Press Enter to delete SQLite configurations".to_string(),
                            synonyms: "ai config reset clear delete".to_string(),
                        },
                        score: 97.0,
                    },
                ];
            }

            if let Some(rest) = lt.strip_prefix("ai config ") {
                let rest_trimmed = rest.trim();
                if let Some((subcmd, val)) = rest_trimmed.split_once(' ') {
                    let subcmd = subcmd.trim();
                    let val = val.trim();
                    if subcmd == "key" || subcmd == "endpoint" || subcmd == "model" {
                        let label = format!(
                            "Save AI {}: {}",
                            subcmd,
                            if subcmd == "key" { "****" } else { val }
                        );
                        let mut raw_val = val.to_string();
                        if let Some(idx) = qt.to_lowercase().find(subcmd) {
                            let start_idx = idx + subcmd.len();
                            if start_idx < qt.len() {
                                raw_val = qt[start_idx..].trim().to_string();
                            }
                        }
                        return vec![SearchResult {
                            entry: CatalogEntry {
                                id: format!("ai.config.save.{}", subcmd),
                                control_name: label,
                                breadcrumb_path: format!("AI Config > Save {}", subcmd),
                                launch_command: format!("action:ai_config:{}:{}", subcmd, raw_val),
                                source: "AI".to_string(),
                                description: format!(
                                    "Press Enter to save this {} to settings",
                                    subcmd
                                ),
                                synonyms: format!("ai config save {}", subcmd),
                            },
                            score: 100.0,
                        }];
                    } else if subcmd == "preset" {
                        // Match any known provider id
                        if let Some(provider) = crate::ai::find_provider(val) {
                            let label = format!("Apply {} Preset", provider.name);
                            return vec![SearchResult {
                                entry: CatalogEntry {
                                    id: format!("ai.config.preset.{}", val),
                                    control_name: label,
                                    breadcrumb_path: "AI Config > Apply Preset".to_string(),
                                    launch_command: format!("action:ai_config:preset:{}", val),
                                    source: "AI".to_string(),
                                    description: format!(
                                        "Press Enter to configure endpoint and model for {}",
                                        provider.name
                                    ),
                                    synonyms: format!("ai config preset {}", val),
                                },
                                score: 100.0,
                            }];
                        }
                    }
                } else if rest_trimmed == "reset" {
                    return vec![SearchResult {
                        entry: CatalogEntry {
                            id: "ai.config.reset".to_string(),
                            control_name: "Reset AI Configuration".to_string(),
                            breadcrumb_path: "AI Config > Clear all custom values".to_string(),
                            launch_command: "action:ai_config:reset".to_string(),
                            source: "AI".to_string(),
                            description: "Press Enter to delete SQLite configurations".to_string(),
                            synonyms: "ai config reset clear delete".to_string(),
                        },
                        score: 100.0,
                    }];
                }
            }
        }

        // ── AI commands: Enter sends to DeepSeek (handled via "ai:" launch cmd) ─
        {
            let qt = q.trim();
            let lt = q_lower_trimmed.as_str();
            let mk = |label: String, cmd: &str, input: &str| -> Vec<SearchResult> {
                vec![SearchResult {
                    entry: CatalogEntry {
                        id: format!("ai.{cmd}"),
                        control_name: label,
                        breadcrumb_path: "AI > Press Enter to ask DeepSeek".to_string(),
                        launch_command: format!("ai:{}:{}", cmd, input),
                        source: "AI".to_string(),
                        description: "Runs on DeepSeek (free)".to_string(),
                        synonyms: "ai ask chat assistant deepseek".to_string(),
                    },
                    score: 12.0,
                }]
            };
            if let Some(r) = lt.strip_prefix("ask ") {
                return mk(format!("Ask AI: {}", r.trim()), "ask", qt[4..].trim());
            }
            if let Some(r) = lt.strip_prefix("chat ") {
                return mk(format!("Ask AI: {}", r.trim()), "ask", qt[5..].trim());
            }
            if let Some(r) = lt.strip_prefix("translate ") {
                return mk(
                    format!("Translate: {}", r.trim()),
                    "translate",
                    qt[10..].trim(),
                );
            }
            if let Some(r) = lt.strip_prefix("explain ") {
                return mk(format!("Explain: {}", r.trim()), "explain", qt[8..].trim());
            }
            if let Some(r) = lt.strip_prefix("summarize ") {
                return mk(
                    format!("Summarize: {}", r.trim()),
                    "summarize",
                    qt[10..].trim(),
                );
            }
            if lt == "explain" {
                return mk("Explain clipboard".into(), "explain", "");
            }
            if lt == "fix grammar" || lt == "grammar" || lt == "fix spelling" {
                return mk("Fix grammar of clipboard".into(), "grammar", "");
            }
            if lt == "find bugs" || lt == "bugs" {
                return mk("Find bugs in clipboard code".into(), "bugs", "");
            }
            if lt == "summarize" {
                return mk("Summarize clipboard".into(), "summarize", "");
            }
        }

        // ── AI Agents: "create agent <name>: <goal>" and "@<name>: <message>" ─
        {
            let qt = q.trim();
            let lt = q_lower_trimmed.as_str();
            if lt.starts_with("create agent ") {
                let raw = qt["create agent ".len()..].trim();
                let mut parts = raw.splitn(2, |c| c == ':' || c == '|');
                let name = parts.next().unwrap_or("").trim();
                let goal = parts.next().unwrap_or("").trim();
                if !name.is_empty() {
                    return vec![SearchResult {
                        entry: CatalogEntry {
                            id: "agent.create".into(),
                            control_name: format!("Create agent: {}", name),
                            breadcrumb_path: if goal.is_empty() {
                                "Agent > Press Enter to create".into()
                            } else {
                                format!("Agent > Goal: {}", goal)
                            },
                            launch_command: format!("mkagent:{}\u{1f}{}", name, goal),
                            source: "AI".into(),
                            description: "Create a persistent AI agent".into(),
                            synonyms: "agent create new bot assistant".into(),
                        },
                        score: 12.0,
                    }];
                }
            }
            if let Some(after) = qt.strip_prefix('@') {
                if let Some((name, msg)) = after.split_once(':') {
                    let (name, msg) = (name.trim(), msg.trim());
                    if let Some((id, real_name)) = self.find_agent_by_name(name) {
                        if !msg.is_empty() {
                            return vec![SearchResult {
                                entry: CatalogEntry {
                                    id: "agent.msg".into(),
                                    control_name: format!("Ask {}: {}", real_name, msg),
                                    breadcrumb_path: "Agent > Press Enter".into(),
                                    launch_command: format!("agent:{}\u{1f}{}", id, msg),
                                    source: "AI".into(),
                                    description: format!("Message agent {}", real_name),
                                    synonyms: "agent message ask".into(),
                                },
                                score: 12.0,
                            }];
                        }
                    }
                }
            }
        }

        // Clean conversational filler + detect command intent (typed or dictated).
        let (intent, q_clean) = clean_prompt(q);

        // ── Calculator: try the raw query, then the cleaned one ("what is 2+2") ─
        let calc_hit = try_calc(q).map(|v| (q.to_string(), v)).or_else(|| {
            if !q_clean.is_empty() && q_clean != q.to_lowercase() {
                try_calc(&q_clean).map(|v| (q_clean.clone(), v))
            } else {
                None
            }
        });
        let calc_result: Option<SearchResult> = calc_hit.map(|(expr, val)| {
            let display = if val.fract() == 0.0 && val.abs() < 1e15 {
                format!("{}", val as i64)
            } else {
                // Up to 10 significant digits, strip trailing zeros
                let s = format!("{:.10}", val);
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            };
            SearchResult {
                entry: CatalogEntry {
                    id: "calc".to_string(),
                    control_name: format!("{} = {}", expr, display),
                    breadcrumb_path: format!("Calculator > Press Enter to copy  {}", display),
                    launch_command: format!("copy:{}", display),
                    source: "CALC".to_string(),
                    description: format!("Math result: {}", display),
                    synonyms: String::new(),
                },
                score: 10.0,
            }
        });

        // ── Unit Converter: runs alongside calc ─────────────────────────────
        let unit_result: Option<SearchResult> = if calc_result.is_none() {
            try_unit_convert(q).map(|(label, value)| SearchResult {
                entry: CatalogEntry {
                    id: "unit_convert".to_string(),
                    control_name: label.clone(),
                    breadcrumb_path: format!("Converter > Press Enter to copy  {}", value),
                    launch_command: format!("copy:{}", value),
                    source: "CALC".to_string(),
                    description: "Unit conversion — press Enter to copy result".to_string(),
                    synonyms: String::new(),
                },
                score: 10.0,
            })
        } else {
            None
        };

        // ── Process Kill: triggered by 'kill' or 'kill <name>' prefix ───────
        let q_lower = q.to_lowercase();
        let kill_results: Vec<SearchResult> = if q_lower == "kill" || q_lower.starts_with("kill ") {
            let proc_query = if q_lower == "kill" {
                ""
            } else {
                q_lower.strip_prefix("kill ").unwrap_or("").trim()
            };
            search_processes(proc_query)
        } else {
            vec![]
        };
        if !kill_results.is_empty() {
            return kill_results;
        }

        // Match on the cleaned, intent-stripped query so filler ("can you open …")
        // doesn't dilute the name/word matching below. Falls back to raw if cleaning
        // emptied it.
        let q_lower = if q_clean.is_empty() {
            q_lower
        } else {
            q_clean.clone()
        };

        let stop_words = [
            "what", "is", "a", "the", "to", "for", "in", "of", "and", "or", "with", "on", "at",
            "by", "from", "about", "how", "this", "it", "my", "your",
        ];
        let q_words: Vec<&str> = q_lower
            .split_whitespace()
            .filter(|w| !stop_words.contains(w))
            .collect();
        let q_char_count = q_lower.chars().count();

        // ponytail: embeddings removed — settings/catalog ranked by lexical signals only
        // (name/synonym/breadcrumb/description/fuzzy). Faster, no model, no per-query ONNX.
        let mut scores: Vec<(usize, f32)> = (0..self.n)
            .map(|i| {
                let entry_idx = &self.meta_index[i];

                // Lexical score
                let mut lex_score = 0.0f32;
                let name_lower = entry_idx.name.as_str();

                // Title-level matching
                if name_lower == q_lower {
                    lex_score += 1.0;
                } else if name_lower.starts_with(&q_lower) {
                    lex_score += 0.7;
                } else if name_lower.contains(&q_lower) {
                    lex_score += 0.4;
                }

                // Position-based boost in Title
                if let Some(idx) = name_lower.find(&q_lower) {
                    let char_idx = name_lower[..idx].chars().count();
                    let total_chars = name_lower.chars().count();
                    if total_chars > 0 {
                        lex_score += 0.5 * (1.0 - (char_idx as f32 / total_chars as f32));
                    }
                }

                // Word overlap in title
                if !q_words.is_empty() {
                    let name_words: Vec<&str> = name_lower.split_whitespace().collect();
                    let mut matched_words = 0;
                    for qw in &q_words {
                        if name_words.contains(qw) {
                            matched_words += 1;
                        }
                    }
                    lex_score += 0.5 * (matched_words as f32 / q_words.len() as f32);
                }

                // Synonyms matching (split by pipe '|')
                let mut syn_boost = 0.0f32;
                for syn in entry_idx.synonyms.split('|') {
                    let s_trimmed = syn.trim();
                    if s_trimmed == q_lower {
                        syn_boost = syn_boost.max(0.8);
                    } else if s_trimmed.starts_with(&q_lower) {
                        syn_boost = syn_boost.max(0.6);
                    } else if s_trimmed.contains(&q_lower) {
                        syn_boost = syn_boost.max(0.4);
                    }
                }

                // Word match in synonyms
                if !q_words.is_empty() {
                    let mut matched_words_in_syn = 0;
                    for qw in &q_words {
                        for syn in entry_idx.synonyms.split('|') {
                            let syn_words: Vec<&str> = syn.split_whitespace().collect();
                            if syn_words.contains(qw) {
                                matched_words_in_syn += 1;
                                break;
                            }
                        }
                    }
                    syn_boost += 0.3 * (matched_words_in_syn as f32 / q_words.len() as f32);
                }
                lex_score += syn_boost;

                // Breadcrumb matching
                let breadcrumb_lower = entry_idx.breadcrumb.as_str();
                if breadcrumb_lower.contains(&q_lower) {
                    lex_score += 0.2;
                }

                // Parent category matching (boost if search words match the parent category path, excluding the item itself)
                if let Some(last_arrow_idx) = breadcrumb_lower.rfind('>') {
                    let parent_categories = &breadcrumb_lower[..last_arrow_idx];
                    let mut matched_words_in_parent = 0;
                    for qw in &q_words {
                        if parent_categories.contains(qw) {
                            matched_words_in_parent += 1;
                        }
                    }
                    if !q_words.is_empty() {
                        lex_score += 0.3 * (matched_words_in_parent as f32 / q_words.len() as f32);
                    }
                }

                // Description matching
                if entry_idx.description.contains(&q_lower) {
                    lex_score += 0.1;
                }

                if lex_score == 0.0 && q_char_count >= 3 {
                    let name_len = entry_idx.name_chars;
                    if name_len > 0 && name_len.abs_diff(q_char_count) <= 8 {
                        let dist = levenshtein_distance(&q_lower, &name_lower);
                        let max_len = name_len.max(q_char_count);
                        let similarity = 1.0 - (dist as f32 / max_len as f32);
                        if similarity >= 0.7 {
                            lex_score += 0.3 * similarity;
                        }
                    }
                }

                // Boost legacy control panel options if they have any match
                if entry_idx.source.contains("legacy") && lex_score > 0.0 {
                    lex_score += 0.25;
                }

                (i, lex_score)
            })
            .collect();

        scores.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // ponytail: conversational/anchor matching was embedding-based — gone now.
        let mut conv_results: Vec<SearchResult> = Vec::new();

        let mut final_results = get_live_results(q);
        let mut settings_matches = self.search_settings_catalog_fts(&q_lower, candidate_k.max(50));
        let mut vec_results: Vec<SearchResult> = scores
            .into_iter()
            .filter(|(_, s)| *s > 0.35)
            .map(|(i, score)| SearchResult {
                entry: self.meta[i].clone(),
                score,
            })
            .collect();

        if !conv_results.is_empty() {
            vec_results.retain(|vr| !conv_results.iter().any(|cr| cr.entry.id == vr.entry.id));
        }

        if !final_results.is_empty() {
            vec_results.retain(|vr| {
                !final_results.iter().any(|fr| {
                    let fr_name = fr.entry.control_name.to_lowercase();
                    let vr_name = vr.entry.control_name.to_lowercase();
                    fr_name == vr_name
                })
            });
        }

        let mut app_matches = Vec::new();
        for app in &self.apps {
            let app_lower = app.name.to_lowercase();
            let mut score = 0.0f32;

            if app_lower == q_lower {
                score = 120.0; // Exact app name wins the launcher, like Raycast/Flow Launcher.
            } else if app_lower.starts_with(&q_lower) && q_lower.chars().count() >= 2 {
                score = 118.0;
            } else if app_lower.starts_with(&q_lower) {
                score = 116.0; // 1-char prefix should still beat 'contains' matches
            } else if q_lower.starts_with(&app_lower) {
                score = 114.0;
            } else if app_lower.contains(&q_lower) {
                score = 112.0;
            } else if q_lower.contains(&app_lower) {
                score = 110.0;
            } else {
                let app_words: Vec<&str> = app_lower.split_whitespace().collect();
                let mut matched = 0;
                for qw in &q_words {
                    if app_words.contains(qw) {
                        matched += 1;
                    }
                }
                if matched > 0 && !q_words.is_empty() {
                    let ratio = matched as f32 / q_words.len() as f32;
                    if ratio >= 0.5 {
                        score = 100.0 + ratio;
                    }
                }
            }

            if score == 0.0 {
                let app_len = app_lower.chars().count();
                let q_len = q_lower.chars().count();
                if app_len > 0 && q_len > 0 {
                    let dist = levenshtein_distance(&q_lower, &app_lower);
                    let max_len = app_len.max(q_len);
                    let similarity = 1.0 - (dist as f32 / max_len as f32);
                    if similarity >= 0.7 {
                        score = 95.0 + similarity;
                    }
                }
            }

            if score > 0.0 {
                app_matches.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("app.{}", app.name),
                        control_name: app.name.clone(),
                        breadcrumb_path: format!("Applications > {}", app.name),
                        launch_command: format!("shell:AppsFolder\\{}", app.path),
                        source: "app".to_string(),
                        description: format!("Launch {}", app.name),
                        synonyms: app.name.to_lowercase(),
                    },
                    score,
                });
            }
        }

        app_matches.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if intent == Intent::LaunchApp {
            for m in &mut app_matches {
                m.score += 1.5;
            }
        }

        // Recent files matching
        let mut recent_matches = Vec::new();
        for rf in &self.recent_files {
            let name_lower = rf.name.to_lowercase();
            // Strip extension for matching (e.g. "report.pdf" → "report")
            let name_no_ext = if let Some(dot) = name_lower.rfind('.') {
                &name_lower[..dot]
            } else {
                &name_lower
            };
            let mut score = 0.0f32;
            if name_lower == q_lower || name_no_ext == q_lower {
                score = 89.0;
            } else if name_lower.starts_with(&q_lower) || name_no_ext.starts_with(&q_lower) {
                score = 86.0;
            } else if name_lower.contains(&q_lower) {
                score = 82.0;
            } else {
                let name_words: Vec<&str> = name_no_ext
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .collect();
                let mut matched = 0;
                for qw in &q_words {
                    if name_words.contains(qw) {
                        matched += 1;
                    }
                }
                if matched > 0 && !q_words.is_empty() {
                    let ratio = matched as f32 / q_words.len() as f32;
                    if ratio >= 0.5 {
                        score = 78.0 + ratio;
                    }
                }
            }
            if score > 0.0 {
                let ext = rf.name.rsplit('.').next().unwrap_or("").to_uppercase();
                recent_matches.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("recent.{}", rf.path),
                        control_name: rf.name.clone(),
                        breadcrumb_path: format!("Recent > {}", rf.path),
                        launch_command: rf.path.clone(),
                        source: "RECENT".to_string(),
                        description: format!("Recently opened {} file", ext),
                        synonyms: rf.name.to_lowercase(),
                    },
                    score,
                });
            }
        }
        recent_matches.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let web_query = if intent == Intent::WebSearch && !q_clean.is_empty() {
            q_clean.as_str()
        } else {
            q
        };
        let encoded_query = url_encode(web_query);
        let web_search = SearchResult {
            entry: CatalogEntry {
                id: "web_search".to_string(),
                control_name: format!("Search Google for \"{}\"", web_query),
                breadcrumb_path: "Web > Google Search > Open in default browser".to_string(),
                launch_command: format!("https://www.google.com/search?q={}", encoded_query),
                source: "web".to_string(),
                description: format!(
                    "Opens default browser and searches Google for '{}'.",
                    web_query
                ),
                synonyms: "google search web internet online".to_string(),
            },
            score: if intent == Intent::WebSearch {
                92.0
            } else {
                50.0
            },
        };

        let mut clipboard_ocr_matches = if with_fts {
            self.search_clipboard_image_ocr_matches(q, candidate_k)
        } else {
            Vec::new()
        };

        let mut file_matches = self.search_local_files_with_fts(&q_lower, with_fts);
        file_matches.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for m in &mut file_matches {
            m.score += if matches!(
                m.entry.source.as_str(),
                "FILE_CONTENT" | "CODE_CONTENT" | "OCR"
            ) {
                40.0
            } else {
                70.0
            };
        }

        if intent == Intent::FindFile {
            for m in &mut file_matches {
                m.score += 1.5;
            }
            for m in &mut recent_matches {
                m.score += 1.5;
            }
        }

        // ── Cross-Source Entity Linker ("Project Auto-Entity") ──────────────
        let mut matched_project_name = None;
        if q.len() >= 3 {
            {
                let conn = &self.conn;

                // Check git repos first
                if let Ok(mut s) = conn.prepare("SELECT name FROM git_repos") {
                    let names: Vec<String> = s
                        .query_map([], |row| row.get::<_, String>(0))
                        .map(|m| m.filter_map(|r| r.ok()).collect())
                        .unwrap_or_default();
                    for name in names {
                        let name_lc = name.to_lowercase();
                        if q_lower_trimmed == name_lc
                            || q_lower_trimmed.contains(&name_lc)
                            || name_lc.contains(&q_lower_trimmed)
                        {
                            matched_project_name = Some(name);
                            break;
                        }
                    }
                }

                // If not found in git repos, check folder names in indexed files using SQLite query to avoid loading all paths
                if matched_project_name.is_none() {
                    let folder_like = format!("%{}%", q_lower_trimmed);
                    if let Ok(mut s) = conn.prepare(
                        "SELECT name FROM files WHERE is_dir = 1 AND (name LIKE ?1 OR ?2 LIKE '%' || name || '%') LIMIT 1"
                    ) {
                        if let Ok(name) = s.query_row([&folder_like, &q_lower_trimmed], |row| row.get::<_, String>(0)) {
                            matched_project_name = Some(name);
                        }
                    }
                }
            }
        }

        let mut project_results = Vec::new();
        if let Some(ref project_name) = matched_project_name {
            let mut repo_path = String::new();
            {
                let conn = &self.conn;
                if let Ok(mut s) = conn.prepare("SELECT path FROM git_repos WHERE name = ? LIMIT 1")
                {
                    if let Ok(p) = s.query_row([project_name], |row| row.get::<_, String>(0)) {
                        repo_path = p;
                    }
                }
            }

            let workspace_card = SearchResult {
                entry: CatalogEntry {
                    id: format!("project.workspace.{}", project_name),
                    control_name: format!("📁 PROJECT WORKSPACE: {}", project_name),
                    breadcrumb_path: format!("Project > Dashboard > {}", project_name),
                    launch_command: repo_path,
                    source: "PROJECT".to_string(),
                    description: format!("Active workspace for project '{}'", project_name),
                    synonyms: format!(
                        "{} project workspace dashboard card",
                        project_name.to_lowercase()
                    ),
                },
                score: 10.0,
            };

            project_results.push(workspace_card);
            project_results.append(&mut self.search_project(project_name));
        }

        let mut merged = Vec::new();
        merged.append(&mut settings_matches);
        merged.append(&mut app_matches);
        merged.append(&mut recent_matches);
        merged.append(&mut clipboard_ocr_matches);
        merged.append(&mut file_matches);
        merged.append(&mut vec_results);
        merged.append(&mut project_results);
        merged.append(&mut self.search_quicklinks_name_matches(q));
        merged.append(&mut self.search_snippets_name_matches(q));
        merged.append(&mut self.search_focus_categories(q));
        merged.append(&mut self.search_games(q));
        merged.append(&mut self.search_notes(q));
        merged.push(web_search.clone());
        merged.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        conv_results.append(&mut final_results);
        final_results = conv_results;
        final_results.append(&mut merged);

        // Deduplicate final_results by id or non-empty launch_command
        let mut unique_results = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_launches = std::collections::HashSet::new();

        for r in final_results {
            let launch_key = result_launch_dedupe_key(&r.entry);
            let is_duplicate = seen_ids.contains(&r.entry.id)
                || launch_key
                    .as_ref()
                    .is_some_and(|key| seen_launches.contains(key));
            if !is_duplicate {
                seen_ids.insert(r.entry.id.clone());
                if let Some(key) = launch_key {
                    seen_launches.insert(key);
                }
                unique_results.push(r);
            }
        }
        final_results = unique_results;

        truncate_preserving_ocr_results(&mut final_results, candidate_k);

        // Quick system actions: match against query
        let mut action_matches = get_quick_actions(q);
        for am in &action_matches {
            final_results.retain(|r| {
                r.entry.control_name.to_lowercase() != am.entry.control_name.to_lowercase()
            });
        }
        action_matches.append(&mut final_results);
        final_results = action_matches;

        if let Some(calc) = calc_result {
            final_results.insert(0, calc);
        } else if let Some(unit) = unit_result {
            final_results.insert(0, unit);
        }

        // Ensure web_search is always in the list as a fallback
        if !final_results.iter().any(|r| r.entry.id == "web_search") {
            final_results.push(web_search);
        }

        if looks_like_agent_task(q) && !final_results.iter().any(|r| r.entry.id == "agent.nlp_task")
        {
            let launch_command = if let Some((id, _)) = self.find_agent_by_name("AI") {
                format!("agent:{}\u{1f}{}", id, q)
            } else {
                format!("query:@AI: {}", q)
            };
            let task_result = SearchResult {
                entry: CatalogEntry {
                    id: "agent.nlp_task".to_string(),
                    control_name: format!("Execute with AI: {}", q),
                    breadcrumb_path: "Agent > Natural language task".to_string(),
                    launch_command,
                    source: "AI".to_string(),
                    description: "Run this as an AI task".to_string(),
                    synonyms: "agent ai execute task command action".to_string(),
                },
                score: 1.09,
            };
            if let Some(web_idx) = final_results
                .iter()
                .position(|r| r.entry.id == "web_search")
            {
                final_results.insert(web_idx + 1, task_result);
            } else {
                final_results.push(task_result);
            }
            truncate_preserving_ocr_results(&mut final_results, candidate_k);
        }

        final_results
    }

    #[allow(dead_code)]
    fn get_conversational_results(&self, qvec: &[f32]) -> Vec<SearchResult> {
        if DISABLE_LIVE_RESULTS.load(Ordering::Relaxed) {
            return vec![];
        }
        let mut best_category: Option<&AnchorCategory> = None;
        let mut best_similarity = 0.0f32;

        for cat in &self.anchor_categories {
            let mut max_similarity = 0.0f32;
            for v in &cat.vecs {
                let sim: f32 = v.iter().zip(qvec).map(|(a, b)| a * b).sum();
                if sim > max_similarity {
                    max_similarity = sim;
                }
            }
            if max_similarity > best_similarity {
                best_similarity = max_similarity;
                best_category = Some(cat);
            }
        }

        if let Some(cat) = best_category {
            if best_similarity >= 0.80 {
                if let Some(entry) = self.meta.iter().find(|e| e.id == cat.target_id) {
                    let mut modified_entry = entry.clone();
                    modified_entry.breadcrumb_path = cat.translation_tip.to_string();
                    modified_entry.source = "translated".to_string();
                    return vec![SearchResult {
                        entry: modified_entry,
                        score: 5.0,
                    }];
                }
            }
        }
        vec![]
    }
}

// ── Prompt NLP: strip conversational filler + detect command intent ─────────────
// Deterministic, offline preprocessing. The leading politeness/verb and trailing
// fluff are stripped so the core phrase survives ("visual studio code"); the detected
// intent biases ranking in `search`. Embeddings still do the semantic matching.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    General,
    LaunchApp,
    FindFile,
    WebSearch,
}

pub fn clean_prompt(raw: &str) -> (Intent, String) {
    let mut q = raw.trim().to_lowercase();
    q = q
        .trim_matches(|c: char| matches!(c, '.' | '?' | '!' | ','))
        .trim()
        .to_string();

    // 1. Peel leading politeness / framing / question words (no source intent).
    const LEAD_FILLER: &[&str] = &[
        "please ",
        "can you ",
        "could you ",
        "would you ",
        "will you ",
        "can u ",
        "i want to ",
        "i wanna ",
        "i want ",
        "i need to ",
        "i need ",
        "i'd like to ",
        "i would like to ",
        "let's ",
        "lets ",
        "go ahead and ",
        "just ",
        "um ",
        "uh ",
        "hey ",
        "ok ",
        "okay ",
        "yeah ",
        "so ",
        "well ",
        "help me ",
        "what is the ",
        "what's the ",
        "whats the ",
        "what is ",
        "what's ",
        "whats ",
        "what are ",
        "how much is ",
        "how many ",
        "calculate ",
        "compute ",
        "tell me ",
    ];
    q = peel_prefixes(q, LEAD_FILLER);

    // 2. Detect + strip a leading intent verb (most specific first: web > file > app).
    const WEB_CUES: &[&str] = &[
        "search the web for ",
        "search the web ",
        "search online for ",
        "search the internet for ",
        "google for ",
        "look up ",
        "web search ",
    ];
    const FILE_CUES: &[&str] = &[
        "open the file ",
        "find the file ",
        "show me the file ",
        "where is the file ",
        "i'm looking for ",
        "im looking for ",
        "looking for ",
        "find my ",
        "find me ",
        "find ",
        "where is ",
        "where's ",
        "wheres ",
        "locate ",
    ];
    const APP_CUES: &[&str] = &[
        "open up ", "open ", "launch ", "fire up ", "boot up ", "run ", "start ", "go to ",
    ];

    let mut intent = Intent::General;
    if let Some(rest) = strip_any_prefix(&q, WEB_CUES) {
        intent = Intent::WebSearch;
        q = rest;
    } else if let Some(rest) = strip_any_prefix(&q, FILE_CUES) {
        intent = Intent::FindFile;
        q = rest;
    } else if let Some(rest) = strip_any_prefix(&q, APP_CUES) {
        intent = Intent::LaunchApp;
        q = rest;
    }

    // 3. Peel generic verbs with no source intent, then any re-exposed filler.
    const LEAD_GENERIC: &[&str] = &[
        "show me ",
        "show ",
        "give me ",
        "get me ",
        "bring up ",
        "pull up ",
        "take me to ",
    ];
    q = peel_prefixes(q, LEAD_GENERIC);
    q = peel_prefixes(q, LEAD_FILLER);

    // 4. Trailing fluff.
    const TRAIL_FILLER: &[&str] = &[
        " right now",
        " for me",
        " please",
        " now",
        " thanks",
        " thank you",
        " on my computer",
        " on my pc",
        " quickly",
        " real quick",
    ];
    loop {
        let before = q.clone();
        for s in TRAIL_FILLER {
            if let Some(stripped) = q.strip_suffix(s) {
                q = stripped.trim_end().to_string();
            }
        }
        if q == before {
            break;
        }
    }

    q = q
        .trim()
        .trim_end_matches(|c: char| matches!(c, '.' | '?' | '!' | ','))
        .trim()
        .to_string();
    (intent, q)
}

fn peel_prefixes(mut q: String, prefixes: &[&str]) -> String {
    loop {
        let before = q.clone();
        for p in prefixes {
            if let Some(rest) = q.strip_prefix(p) {
                q = rest.trim_start().to_string();
            }
        }
        if q == before {
            break;
        }
    }
    q
}

fn strip_any_prefix(q: &str, prefixes: &[&str]) -> Option<String> {
    for p in prefixes {
        if let Some(rest) = q.strip_prefix(p) {
            return Some(rest.trim_start().to_string());
        }
    }
    None
}

fn looks_like_agent_task(raw: &str) -> bool {
    let q = raw.trim().to_lowercase();
    if q.len() < 4 || q.starts_with('@') || q.starts_with("ai ") || q.starts_with("ask ") {
        return false;
    }
    // ponytail: simple verb heuristic; upgrade to an intent model only if this misclassifies real usage.
    const TASK_WORDS: &[&str] = &[
        "clear",
        "flush",
        "restart",
        "stop",
        "kill",
        "delete",
        "remove",
        "clean",
        "enable",
        "disable",
        "install",
        "uninstall",
        "update",
        "fix",
        "repair",
        "configure",
        "set",
        "create",
        "make",
        "move",
        "rename",
        "run",
        "execute",
    ];
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .any(|w| TASK_WORDS.contains(&w))
}

#[cfg(test)]
mod nlp_tests {
    use super::{clean_prompt, looks_like_agent_task, Intent};
    #[test]
    fn cleans_and_classifies() {
        assert_eq!(
            clean_prompt("Open Chrome, please"),
            (Intent::LaunchApp, "chrome".to_string())
        );
        assert_eq!(
            clean_prompt("can you launch spotify"),
            (Intent::LaunchApp, "spotify".to_string())
        );
        assert_eq!(
            clean_prompt("can you open brave for me"),
            (Intent::LaunchApp, "brave".to_string())
        );
        assert_eq!(
            clean_prompt("find my budget spreadsheet"),
            (Intent::FindFile, "budget spreadsheet".to_string())
        );
        assert_eq!(
            clean_prompt("look up rust lifetimes"),
            (Intent::WebSearch, "rust lifetimes".to_string())
        );
        assert_eq!(
            clean_prompt("show me my downloads right now"),
            (Intent::General, "my downloads".to_string())
        );
        assert_eq!(
            clean_prompt("settings"),
            (Intent::General, "settings".to_string())
        );
        // "google chrome" must stay an app query, not a web search.
        assert_eq!(
            clean_prompt("google chrome"),
            (Intent::General, "google chrome".to_string())
        );
        // question-word framing exposes a math expression for the calc fallback.
        assert_eq!(
            clean_prompt("what is 2+2"),
            (Intent::General, "2+2".to_string())
        );
    }

    #[test]
    fn detects_agent_tasks_without_stealing_plain_searches() {
        assert!(looks_like_agent_task("ipconfig flush dns"));
        assert!(looks_like_agent_task("can you clear my dns cache"));
        assert!(!looks_like_agent_task("google chrome"));
        assert!(!looks_like_agent_task("ask why dns is slow"));
    }
}

pub fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            b' ' => {
                encoded.push('+');
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_allowlist_keeps_only_curated() {
        let mk = |source: &str, cmd: &str| SearchResult {
            entry: CatalogEntry {
                id: String::new(),
                control_name: String::new(),
                breadcrumb_path: String::new(),
                launch_command: cmd.into(),
                source: source.into(),
                description: String::new(),
                synonyms: String::new(),
            },
            score: 0.0,
        };
        // kept
        for (s, c) in [
            ("FILE", "C:/x.pdf"),
            ("FILE_CONTENT", "C:/x.pdf"),
            ("RECENT", "C:/x"),
            ("CODE", "C:/x.rs"),
            ("CODE_CONTENT", "C:/x.rs"),
            ("OCR", "C:/x.png"),
            ("app", "shell:AppsFolder\\X"),
            ("COMMIT", "x"),
            ("HISTORY", "https://x"),
            ("BOOKMARK", "https://x"),
            ("X", "ms-settings:display"),
            ("X", "control.exe /name X"),
            ("X", "appwiz.cpl"),
            ("AI", "agent:1\u{1f}hi"),
            ("AI", "openagent:1\u{1f}n"),
            ("AI", "aichat:5"),
            ("", "https://chatgpt.com/?q=hello"),
            ("", "https://chatgpt.com/"),
            ("web", "https://www.google.com/search?q=hello"),
            ("CALC", "copy:4"),
            ("SNIPPET", "copy_snippet:hello"),
            ("FOLDER", "commits:"),
            ("FOLDER", "bookmarks:"),
            ("FOLDER", "agents:"),
            ("FOLDER", "C:\\Users\\me\\Documents"),
            ("FOLDER", "/home/x/docs"),
            ("CLIPBOARD", "copy:hi"),
            ("FOLDER", "clip:"),
        ] {
            assert!(lean_allowed(&mk(s, c)), "should keep {s} {c}");
        }
        // dropped
        for (s, c) in [
            ("AI", "ai:ask:hi"),
            ("AI", "aichats:"),
            ("FOLDER", "notes:"),
            ("TODO", "x"),
            ("QUICKLINK", "https://x"),
            ("LIVE", "https://example.com/?q=hello"),
            ("LIVE", "https://claude.ai/new?q=hello"),
        ] {
            assert!(!lean_allowed(&mk(s, c)), "should drop {s} {c}");
        }
    }

    #[test]
    fn truncation_preserves_ocr_results_for_filter_visibility() {
        let mut results = (0..10)
            .map(|idx| SearchResult {
                entry: CatalogEntry {
                    id: format!("file.{idx}"),
                    control_name: format!("file {idx}"),
                    breadcrumb_path: "File".to_string(),
                    launch_command: format!("C:\\files\\{idx}.txt"),
                    source: "FILE".to_string(),
                    description: "Local file".to_string(),
                    synonyms: String::new(),
                },
                score: 100.0 - idx as f32,
            })
            .collect::<Vec<_>>();
        results.push(SearchResult {
            entry: CatalogEntry {
                id: "clip.1".to_string(),
                control_name: "[Image] Copied from SnippingTool.exe".to_string(),
                breadcrumb_path: "Clipboard > SnippingTool.exe".to_string(),
                launch_command: "copy_image:C:\\clip.bmp".to_string(),
                source: "CLIPBOARD".to_string(),
                description: "🔤 ProtonSearch.exe".to_string(),
                synonyms: "ProtonSearch.exe".to_string(),
            },
            score: 1.0,
        });

        truncate_preserving_ocr_results(&mut results, 5);

        assert_eq!(results.len(), 5);
        assert!(
            results.iter().any(is_ocr_filter_visible_result),
            "OCR filter needs at least one OCR result after top-k truncation"
        );
    }

    #[test]
    fn ai_prefixes_work_without_space_or_prompt() {
        let mut engine = SearchEngine::new(std::path::PathBuf::from("test_ai_prefixes.db"), false)
            .expect("engine");

        let chatgpt = engine.search("chatgpt:", 5);
        assert_eq!(chatgpt[0].entry.control_name, "Open ChatGPT");
        assert_eq!(chatgpt[0].entry.launch_command, "https://chatgpt.com/");

        let claude = engine.search("claude:explain this", 5);
        assert!(
            claude
                .iter()
                .all(|r| !r.entry.launch_command.starts_with("https://claude.ai/")),
            "non-ChatGPT AI prefixes should not be exposed"
        );
    }

    #[test]
    fn catalog_index_caches_lowercase_fields() {
        let entry = CatalogEntry {
            id: "id".to_string(),
            control_name: "Display Settings".to_string(),
            breadcrumb_path: "System > Display".to_string(),
            launch_command: "ms-settings:display".to_string(),
            source: "MODERN".to_string(),
            description: "Adjust Screen".to_string(),
            synonyms: "Monitor|Brightness".to_string(),
        };

        let index = CatalogEntryIndex::from_entry(&entry);

        assert_eq!(index.name, "display settings");
        assert_eq!(index.name_chars, 16);
        assert_eq!(index.breadcrumb, "system > display");
        assert_eq!(index.description, "adjust screen");
        assert_eq!(index.source, "modern");
        assert_eq!(index.synonyms, "monitor|brightness");
    }

    #[test]
    fn content_matches_are_classified_for_filters() {
        assert_eq!(content_match_source("pdf", false), "FILE_CONTENT");
        assert_eq!(content_match_source("rs", false), "CODE_CONTENT");
        assert_eq!(content_match_source("png", false), "OCR");
    }

    #[test]
    fn scoped_pages_get_empty_state_instead_of_blank_results() {
        let empty = empty_scope_result("bookmarks: missing").unwrap();
        assert_eq!(empty.entry.control_name, "No bookmarks found");
        assert_eq!(empty.entry.launch_command, "bookmarks:");
        assert!(lean_allowed(&empty));
        assert!(empty_scope_result("plain search").is_none());
    }

    #[test]
    fn quick_actions_expose_native_windows_shortcuts() {
        for command in [
            "action:sleep_displays",
            "action:show_desktop",
            "action:open_run",
            "action:quit_active_app",
            "action:open_recycle_bin",
            "action:hibernate",
            "action:logout",
            "action:show_screensaver",
            "query:clip:",
            "action:reset_window_position",
            "action:paste_latest_screenshot",
            "action:reveal_logs",
            "action:window:restore",
            "action:window:maximize_height",
            "action:window:maximize_width",
            "action:window:move_left",
            "action:window:move_right",
            "action:window:move_top",
            "action:window:move_bottom",
            "action:window:bottom_center_sixth",
            "action:window:top_center_sixth",
            "action:window:bottom_left_sixth",
            "action:window:bottom_right_sixth",
            "action:window:top_left_sixth",
            "action:window:top_right_sixth",
            "action:window:bottom_center_two_thirds",
            "action:window:top_center_two_thirds",
            "action:window:bottom_third",
            "action:window:top_third",
            "action:window:bottom_three_fourths",
            "action:window:top_three_fourths",
            "action:window:bottom_two_thirds",
            "action:window:top_two_thirds",
            "action:window:first_fourth",
            "action:window:second_fourth",
            "action:window:third_fourth",
            "action:window:last_fourth",
            "action:window:top_first_fourth",
            "action:window:top_second_fourth",
            "action:window:top_third_fourth",
            "action:window:top_last_fourth",
            "action:window:bottom_first_fourth",
            "action:window:bottom_second_fourth",
            "action:window:bottom_third_fourth",
            "action:window:bottom_last_fourth",
            "action:window:last_third",
            "action:window:last_three_fourths",
            "action:window:last_two_thirds",
            "action:window:first_third",
            "action:window:first_three_fourths",
            "action:window:first_two_thirds",
            "action:window:center_half",
            "action:window:center_three_fourths",
            "action:window:center_two_thirds",
        ] {
            assert!(QUICK_ACTIONS
                .iter()
                .any(|action| action.launch_command == command));
        }
    }

    #[test]
    fn settings_fts_returns_search_settings_in_fast_path() {
        let mut engine = SearchEngine::new(std::path::PathBuf::from("test_db.db"), false)
            .expect("Failed to initialize engine");

        let results = engine.search_with_fts("Search", 10, false);
        assert!(
            results.iter().any(is_native_settings_result),
            "Search should return native settings/control results, got {:?}",
            results
                .iter()
                .map(|r| (&r.entry.control_name, &r.entry.launch_command))
                .collect::<Vec<_>>()
        );
        assert!(
            results.iter().any(|r| {
                let name = r.entry.control_name.to_lowercase();
                let breadcrumb = r.entry.breadcrumb_path.to_lowercase();
                is_native_settings_result(r)
                    && (name.contains("search")
                        || breadcrumb.contains("search")
                        || breadcrumb.contains("indexing"))
            }),
            "Search should directly match settings catalog rows, got {:?}",
            results
                .iter()
                .map(|r| (&r.entry.control_name, &r.entry.breadcrumb_path))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn string_shortening_is_utf8_safe() {
        assert_eq!(ellipsize_chars("abcdefg", 6), "abc...");
        assert_eq!(ellipsize_chars("😀😀😀😀", 5), "😀😀😀😀");
        assert_eq!(take_chars("😀abcdef", 2), "😀a");
        assert_eq!(take_last_chars("abcdef😀", 2), "f😀");
    }

    #[test]
    fn memory_home_description_keeps_local_trust_line() {
        assert_eq!(
            memory_home_description(3, 2, 9),
            "3 events today, 2 yesterday, 9 total. Stored locally on this PC."
        );
    }

    #[test]
    fn workday_memory_query_detects_mvp_phrasing() {
        assert_eq!(
            workday_memory_query_days("what was i working on yesterday"),
            Some(1)
        );
        assert_eq!(workday_memory_query_days("what did i do today"), Some(0));
        assert_eq!(workday_memory_query_days("open chrome yesterday"), None);
    }

    fn unique_test_db(name: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("omnisearch_lite_{name}_{stamp}.db"))
    }

    #[test]
    fn fresh_clipboard_schema_includes_ocr_text() {
        let db_path = unique_test_db("fresh_clipboard_schema");
        let _engine = SearchEngine::new(db_path.clone(), false).expect("engine");
        let conn = Connection::open(&db_path).expect("test db");
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(clipboard_history)")
            .expect("pragma")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("columns")
            .filter_map(|row| row.ok())
            .collect();
        let _ = std::fs::remove_file(&db_path);

        assert!(
            columns.iter().any(|column| column == "ocr_text"),
            "fresh clipboard_history schema should include ocr_text, got {columns:?}"
        );
    }

    #[test]
    fn normal_search_finds_clipboard_image_ocr_text() {
        let db_path = unique_test_db("normal_clipboard_ocr_search");
        let mut engine = SearchEngine::new(db_path.clone(), false).expect("engine");
        engine
            .conn
            .execute(
                "INSERT INTO clipboard_history
                 (content, timestamp, source_app, is_image, pinned, ocr_text)
                 VALUES (?1, ?2, ?3, 1, 0, ?4)",
                rusqlite::params![
                    "C:\\Users\\tester\\AppData\\Roaming\\protonsearch\\clipboard_images\\clip.bmp",
                    1_i64,
                    "SnippingTool.exe",
                    "quarterly revenue target"
                ],
            )
            .expect("insert clipboard image");

        let results = engine.search_with_fts("quarterly revenue", 20, true);
        let _ = std::fs::remove_file(&db_path);

        assert!(
            results.iter().any(|result| {
                result.entry.launch_command.starts_with("copy_image:")
                    && result.entry.source == "CLIPBOARD"
            }),
            "normal search should include OCR-matched clipboard image, got {:?}",
            results
                .iter()
                .map(|result| (
                    &result.entry.control_name,
                    &result.entry.source,
                    &result.entry.launch_command
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn clipboard_ocr_search_matches_compacted_text() {
        let db_path = unique_test_db("compact_clipboard_ocr_search");
        let mut engine = SearchEngine::new(db_path.clone(), false).expect("engine");
        engine
            .conn
            .execute(
                "INSERT INTO clipboard_history
                 (content, timestamp, source_app, is_image, pinned, ocr_text)
                 VALUES (?1, ?2, ?3, 1, 0, ?4)",
                rusqlite::params![
                    "C:\\Users\\tester\\AppData\\Roaming\\protonsearch\\clipboard_images\\compact.bmp",
                    2_i64,
                    "SnippingTool.exe",
                    "Proton Search.exe"
                ],
            )
            .expect("insert clipboard image");

        let results = engine.search_with_fts("protonsearch", 20, true);
        let _ = std::fs::remove_file(&db_path);

        assert!(
            results.iter().any(|result| {
                result.entry.launch_command.starts_with("copy_image:")
                    && result.entry.source == "CLIPBOARD"
            }),
            "clipboard OCR should match across OCR spacing/punctuation, got {:?}",
            results
                .iter()
                .map(|result| (
                    &result.entry.control_name,
                    &result.entry.source,
                    &result.entry.launch_command
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn saved_clipboard_image_ocr_is_not_returned_twice() {
        let db_path = unique_test_db("dedupe_clipboard_ocr_file_twin");
        let mut engine = SearchEngine::new(db_path.clone(), false).expect("engine");
        let image_path =
            "C:\\Users\\tester\\AppData\\Roaming\\protonsearch\\clipboard_images\\clip.bmp";
        engine
            .conn
            .execute(
                "CREATE TABLE IF NOT EXISTS files (
                    path TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    extension TEXT NOT NULL,
                    modified INTEGER NOT NULL,
                    size INTEGER NOT NULL DEFAULT 0,
                    is_dir INTEGER NOT NULL DEFAULT 0
                );",
                [],
            )
            .expect("files table");
        engine
            .conn
            .execute(
                "CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(path UNINDEXED, content);",
                [],
            )
            .expect("files fts");
        engine
            .conn
            .execute(
                "INSERT INTO files (path, name, extension, modified, size, is_dir)
                 VALUES (?1, 'clip.bmp', 'bmp', 1, 1, 0)",
                [image_path],
            )
            .expect("insert file");
        engine
            .conn
            .execute(
                "INSERT INTO files_fts (path, content) VALUES (?1, ?2)",
                rusqlite::params![image_path, "ProtonSearch.exe"],
            )
            .expect("insert fts");
        engine
            .conn
            .execute(
                "INSERT INTO clipboard_history
                 (content, timestamp, source_app, is_image, pinned, ocr_text)
                 VALUES (?1, ?2, ?3, 1, 0, ?4)",
                rusqlite::params![image_path, 3_i64, "SnippingTool.exe", "ProtonSearch.exe"],
            )
            .expect("insert clipboard image");

        let results = engine.search_with_fts("protonsearch", 20, true);
        let _ = std::fs::remove_file(&db_path);

        let image_hits = results
            .iter()
            .filter(|result| {
                result_launch_dedupe_key(&result.entry)
                    == Some(format!("image:{}", image_path.to_ascii_lowercase()))
            })
            .count();
        assert_eq!(
            image_hits,
            1,
            "same saved clipboard image should appear once, got {:?}",
            results
                .iter()
                .map(|result| (
                    &result.entry.control_name,
                    &result.entry.source,
                    &result.entry.launch_command
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_keeps_low_ranked_ocr_candidates_beyond_requested_top_k() {
        let db_path = unique_test_db("broad_search_keeps_ocr_candidates");
        let mut engine = SearchEngine::new(db_path.clone(), false).expect("engine");
        engine
            .conn
            .execute(
                "CREATE TABLE IF NOT EXISTS files (
                    path TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    extension TEXT NOT NULL,
                    modified INTEGER NOT NULL,
                    size INTEGER NOT NULL DEFAULT 0,
                    is_dir INTEGER NOT NULL DEFAULT 0
                );",
                [],
            )
            .expect("files table");
        for idx in 0..20 {
            engine
                .conn
                .execute(
                    "INSERT INTO files (path, name, extension, modified, size, is_dir)
                     VALUES (?1, ?2, 'exe', ?3, 1, 0)",
                    rusqlite::params![
                        format!("C:\\Users\\tester\\Documents\\protonsearch-{idx}.exe"),
                        format!("protonsearch-{idx}.exe"),
                        idx as i64
                    ],
                )
                .expect("insert file");
        }
        engine
            .conn
            .execute(
                "INSERT INTO clipboard_history
                 (content, timestamp, source_app, is_image, pinned, ocr_text)
                 VALUES (?1, ?2, ?3, 1, 0, ?4)",
                rusqlite::params![
                    "C:\\Users\\tester\\AppData\\Roaming\\protonsearch\\clipboard_images\\late.bmp",
                    4_i64,
                    "SnippingTool.exe",
                    "ProtonSearch.exe"
                ],
            )
            .expect("insert clipboard image");

        let results = engine.search_with_fts("protonsearch", 5, true);
        let _ = std::fs::remove_file(&db_path);

        assert!(
            results.len() > 5,
            "search should keep a broad matched candidate set, got {} results",
            results.len()
        );
        assert!(
            results.iter().any(|result| {
                result.entry.source == "CLIPBOARD"
                    && result.entry.launch_command.starts_with("copy_image:")
            }),
            "OCR filter needs OCR candidates even when file matches rank higher, got {:?}",
            results
                .iter()
                .map(|result| (
                    &result.entry.control_name,
                    &result.entry.source,
                    &result.entry.launch_command
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn memory_timestamps_are_normalized_to_unix_seconds() {
        assert_eq!(normalize_event_timestamp(1_700_000_000), 1_700_000_000);
        assert_eq!(normalize_event_timestamp(1_700_000_000_123), 1_700_000_000);
        assert_eq!(
            normalize_event_timestamp(1_700_000_000_123_456),
            1_700_000_000
        );
        assert_eq!(
            normalize_event_timestamp(11_644_473_600_000_000 + 1_700_000_000_000_000),
            1_700_000_000
        );
    }

    #[test]
    fn session_source_summary_counts_events() {
        let event = |source: &str| {
            (
                1,
                1_700_000_000,
                source.to_string(),
                "Event".to_string(),
                "Title".to_string(),
                String::new(),
                String::new(),
                None,
                None,
            )
        };
        assert_eq!(
            session_source_summary(&[event("Browser"), event("Git"), event("Browser")]),
            "Browser 2, Git 1"
        );
    }

    #[test]
    fn unix_date_uses_real_calendar() {
        assert_eq!(format_unix_date(0), "1970-01-01");
        assert_eq!(format_unix_date(1_704_067_200), "2024-01-01");
    }

    #[test]
    fn test_hybrid_search_accuracy() {
        DISABLE_LIVE_RESULTS.store(true, Ordering::Relaxed);
        let exe = std::env::current_exe().expect("failed to get current exe");
        let parent = exe.parent().expect("failed to get parent");
        let mut model_path = parent.join("model_int8.onnx");
        if !model_path.exists() {
            model_path = parent
                .parent()
                .expect("failed to get grandparent")
                .join("model_int8.onnx");
        }
        let _ = &model_path;
        let mut engine = SearchEngine::new(std::path::PathBuf::from("test_db.db"), true)
            .expect("Failed to initialize engine");

        let queries = vec![
            (
                "stop mouse from jumping",
                vec!["pointer precision", "pointer speed", "mouse"],
            ),
            ("disable startup programs", vec!["startup", "autostart"]),
            ("change time zone", vec!["time zone", "timezone"]),
            ("turn off notifications", vec!["notification"]),
            (
                "fix blurry text",
                vec!["ccd cleartype", "cleartype", "dpi", "blurry", "scale"],
            ),
            ("allow apps through firewall", vec!["firewall"]),
            (
                "make text bigger",
                vec!["text size", "font size", "scale", "display"],
            ),
            ("change display brightness", vec!["brightness"]),
            ("connect to wifi", vec!["wi-fi", "wifi", "wireless"]),
            ("remove a printer", vec!["printer", "print"]),
            (
                "enable dark mode",
                vec!["dark", "color mode", "theme", "appearance"],
            ),
            ("change screen resolution", vec!["resolution", "display"]),
            (
                "set default browser",
                vec!["default app", "default browser", "browser"],
            ),
            ("disable auto updates", vec!["update", "windows update"]),
            ("sleep settings", vec!["sleep", "power"]),
            (
                "change wallpaper",
                vec!["wallpaper", "background", "desktop background"],
            ),
            ("enable bluetooth", vec!["bluetooth"]),
            ("disable touchpad", vec!["touchpad", "trackpad"]),
            ("configure microphone", vec!["microphone", "input device"]),
            ("change language", vec!["language", "region"]),
            (
                "set up fingerprint login",
                vec!["fingerprint", "biometric", "windows hello"],
            ),
            (
                "clear storage space",
                vec!["storage", "disk cleanup", "disk space"],
            ),
            (
                "rename this computer",
                vec!["computer name", "rename pc", "device name"],
            ),
            (
                "change sound output device",
                vec!["sound output", "audio output", "speaker", "playback"],
            ),
            (
                "reduce eye strain at night",
                vec!["night light", "blue light", "color temperature"],
            ),
            (
                "stop apps from running in background",
                vec!["background app"],
            ),
            (
                "speed up animations",
                vec!["animation", "visual effect", "transition"],
            ),
            (
                "uninstall a program",
                vec!["uninstall", "remove app", "apps & features"],
            ),
            ("disable cortana", vec!["cortana", "search"]),
            ("set proxy settings", vec!["proxy"]),
            (
                "change mouse speed",
                vec!["pointer speed", "mouse speed", "cursor speed"],
            ),
            (
                "flip screen upside down",
                vec!["rotation", "orientation", "display"],
            ),
            ("enable remote desktop", vec!["remote desktop", "rdp"]),
            ("set up vpn", vec!["vpn", "virtual private"]),
            (
                "configure parental controls",
                vec!["parental", "family safety", "child"],
            ),
            ("map network drive", vec!["network drive", "map drive"]),
            (
                "change power plan",
                vec!["power plan", "battery saver", "performance"],
            ),
            ("set up email account", vec!["email", "mail", "account"]),
            ("configure taskbar", vec!["taskbar"]),
            ("disable location services", vec!["location"]),
            (
                "change keyboard layout",
                vec!["keyboard layout", "input method", "language"],
            ),
            ("enable magnifier", vec!["magnifier"]),
            (
                "set up multiple monitors",
                vec!["multiple display", "second screen", "extend"],
            ),
            (
                "change user account picture",
                vec!["account picture", "profile picture", "user photo"],
            ),
            (
                "disable password requirement",
                vec!["password", "sign-in", "sign in option"],
            ),
            ("configure storage sense", vec!["storage sense"]),
            ("enable developer mode", vec!["developer mode"]),
            (
                "sync settings between devices",
                vec!["sync", "backup", "cloud"],
            ),
            (
                "change default search engine",
                vec!["search", "default search"],
            ),
        ];

        let mut hits = 0;
        let mut misses = vec![];

        for (q, keywords) in &queries {
            let results = engine.search(q, 3);
            let mut hit = false;
            for r in &results {
                let haystack = format!(
                    "{} {} {}",
                    r.entry.control_name, r.entry.breadcrumb_path, r.entry.synonyms
                )
                .to_lowercase();
                if keywords
                    .iter()
                    .any(|kw| haystack.contains(&kw.to_lowercase()))
                {
                    hit = true;
                    break;
                }
            }
            if hit {
                hits += 1;
            } else {
                let got = if results.is_empty() {
                    "None".to_string()
                } else {
                    format!(
                        "{} ({})",
                        results[0].entry.control_name, results[0].entry.breadcrumb_path
                    )
                };
                misses.push((q, got));
            }
        }

        let hit_rate = (hits as f32 / queries.len() as f32) * 100.0;
        println!(
            "Rust Hit@3 rate: {}/{} = {:.1}%",
            hits,
            queries.len(),
            hit_rate
        );
        if !misses.is_empty() {
            println!("Misses:");
            for (q, got) in misses {
                println!("  Query '{}' -> got: {}", q, got);
            }
        }

        assert!(
            hit_rate >= 70.0,
            "Hit rate was only {:.1}% (target: >= 70.0%)",
            hit_rate
        );
    }

    #[test]
    fn test_enumerate_apps_folder() {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Com::{
            CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        use windows::Win32::UI::Shell::{
            BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, SHGetKnownFolderItem,
            KF_FLAG_DEFAULT, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
        };

        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let apps_folder: IShellItem =
                SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, HANDLE::default())
                    .unwrap();
            let enum_items: IEnumShellItems =
                apps_folder.BindToHandler(None, &BHID_EnumItems).unwrap();
            let mut items = [None];
            let mut fetched = 0;
            let mut count = 0;
            while enum_items.Next(&mut items, Some(&mut fetched)).is_ok() && fetched == 1 {
                if let Some(item) = &items[0] {
                    let display_name_ptr = item.GetDisplayName(SIGDN_NORMALDISPLAY).unwrap();
                    let parsing_name_ptr =
                        item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).unwrap();

                    let mut len = 0;
                    while *display_name_ptr.0.add(len) != 0 {
                        len += 1;
                    }
                    let display_name = String::from_utf16_lossy(std::slice::from_raw_parts(
                        display_name_ptr.0,
                        len,
                    ));

                    let mut len = 0;
                    while *parsing_name_ptr.0.add(len) != 0 {
                        len += 1;
                    }
                    let parsing_name = String::from_utf16_lossy(std::slice::from_raw_parts(
                        parsing_name_ptr.0,
                        len,
                    ));

                    println!("App: {} -> {}", display_name, parsing_name);
                    windows::Win32::System::Com::CoTaskMemFree(Some(
                        display_name_ptr.0 as *const _,
                    ));
                    windows::Win32::System::Com::CoTaskMemFree(Some(
                        parsing_name_ptr.0 as *const _,
                    ));
                    count += 1;
                    if count >= 10 {
                        break;
                    }
                }
            }
        }
    }

    #[test]
    fn test_search_file() {
        let db_path = match std::env::var("APPDATA") {
            Ok(d) => {
                let path = std::path::PathBuf::from(d).join("omnisearch-lite");
                path.join("file_index.db")
            }
            Err(_) => std::path::PathBuf::from("file_index.db"),
        };
        let exe = std::env::current_exe().expect("failed to get current exe");
        let parent = exe.parent().expect("failed to get parent");
        let mut model_path = parent.join("model_int8.onnx");
        if !model_path.exists() {
            model_path = parent
                .parent()
                .expect("failed to get grandparent")
                .join("model_int8.onnx");
        }
        let _ = &model_path;
        let mut engine = SearchEngine::new(db_path, true).expect("Failed to initialize engine");

        println!("--- DIAGNOSTIC SEARCH TEST ---");
        let results = engine.search("resume", 10);
        println!("Combined search results for 'resume':");
        for (idx, r) in results.iter().enumerate() {
            println!(
                "  [{}] ID: {}, Name: {}, Source: {}, Breadcrumb: {}, Score: {}",
                idx,
                r.entry.id,
                r.entry.control_name,
                r.entry.source,
                r.entry.breadcrumb_path,
                r.score
            );
        }
    }
}

#[repr(C)]
struct SYSTEM_POWER_STATUS {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[repr(C)]
struct MEMORYSTATUSEX {
    dw_length: u32,
    dw_memory_load: u32,
    ull_total_phys: u64,
    ull_avail_phys: u64,
    ull_total_page_file: u64,
    ull_avail_page_file: u64,
    ull_total_virtual: u64,
    ull_avail_virtual: u64,
    ull_avail_extended_virtual: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemPowerStatus(lpSystemPowerStatus: *mut SYSTEM_POWER_STATUS) -> i32;
    fn GetDiskFreeSpaceExW(
        lpDirectoryName: *const u16,
        lpFreeBytesAvailableToCaller: *mut u64,
        lpTotalNumberOfBytes: *mut u64,
        lpTotalNumberOfFreeBytes: *mut u64,
    ) -> i32;
    fn GlobalMemoryStatusEx(lpBuffer: *mut MEMORYSTATUSEX) -> i32;
}

fn get_local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip().to_string())
}

fn get_live_results(query: &str) -> Vec<SearchResult> {
    if DISABLE_LIVE_RESULTS.load(Ordering::Relaxed) {
        return vec![];
    }
    let q = query.to_lowercase();
    let mut results = vec![];

    // 1. Battery Status
    if q.contains("battery") || q.contains("power") || q.contains("charge") {
        let mut status = SYSTEM_POWER_STATUS {
            ac_line_status: 0,
            battery_flag: 0,
            battery_life_percent: 0,
            system_status_flag: 0,
            battery_life_time: 0,
            battery_full_life_time: 0,
        };
        unsafe {
            if GetSystemPowerStatus(&mut status) != 0 {
                let percent = status.battery_life_percent;
                if percent != 255 {
                    let state = if status.ac_line_status == 1 {
                        "Charging"
                    } else if status.ac_line_status == 0 {
                        "Discharging"
                    } else {
                        "Unknown State"
                    };

                    results.push(SearchResult {
                        entry: CatalogEntry {
                            id: "live.battery".to_string(),
                            control_name: "Battery Status".to_string(),
                            breadcrumb_path: format!(
                                "System > Power & battery > Currently {}% ({})",
                                percent, state
                            ),
                            launch_command: "ms-settings:powersleep".to_string(),
                            source: "LIVE".to_string(),
                            description: "Shows the current battery level and power state."
                                .to_string(),
                            synonyms: "battery percentage power life status".to_string(),
                        },
                        score: 2.0,
                    });
                }
            }
        }
    }

    // 2. Local IP Address
    if q.contains("ip")
        || q.contains("network")
        || q.contains("address")
        || q.contains("wifi")
        || q.contains("ethernet")
    {
        if let Some(ip) = get_local_ip() {
            results.push(SearchResult {
                entry: CatalogEntry {
                    id: "live.ip".to_string(),
                    control_name: "Copy Local IP Address".to_string(),
                    breadcrumb_path: format!("Network > Connection > {} (Press Enter to copy)", ip),
                    launch_command: format!("copy:{}", ip),
                    source: "ACTION".to_string(),
                    description: "Copies your current local IP address to the clipboard."
                        .to_string(),
                    synonyms: "ip address local network connection".to_string(),
                },
                score: 2.0,
            });
        }
    }

    // 3. System RAM Usage
    if q.contains("ram") || q.contains("memory") || q.contains("perf") || q.contains("speed") {
        let mut mem = MEMORYSTATUSEX {
            dw_length: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            dw_memory_load: 0,
            ull_total_phys: 0,
            ull_avail_phys: 0,
            ull_total_page_file: 0,
            ull_avail_page_file: 0,
            ull_total_virtual: 0,
            ull_avail_virtual: 0,
            ull_avail_extended_virtual: 0,
        };
        unsafe {
            if GlobalMemoryStatusEx(&mut mem) != 0 {
                let avail_gb = mem.ull_avail_phys as f64 / 1024.0 / 1024.0 / 1024.0;
                let total_gb = mem.ull_total_phys as f64 / 1024.0 / 1024.0 / 1024.0;
                let load = mem.dw_memory_load;
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: "live.ram".to_string(),
                        control_name: "System Memory".to_string(),
                        breadcrumb_path: format!(
                            "System > Performance > {:.1} GB free / {:.1} GB total ({}% used)",
                            avail_gb, total_gb, load
                        ),
                        launch_command: "taskmgr.exe".to_string(),
                        source: "LIVE".to_string(),
                        description:
                            "Shows currently free physical RAM and memory load percentage."
                                .to_string(),
                        synonyms: "ram memory physical usage performance".to_string(),
                    },
                    score: 2.0,
                });
            }
        }
    }

    // 4. Disk Space
    if q.contains("disk")
        || q.contains("storage")
        || q.contains("space")
        || q.contains("drive")
        || q.contains("free")
    {
        let mut free = 0u64;
        let mut total = 0u64;
        unsafe {
            if GetDiskFreeSpaceExW(
                std::ptr::null(),
                &mut free,
                &mut total,
                std::ptr::null_mut(),
            ) != 0
            {
                let free_gb = free as f64 / 1024.0 / 1024.0 / 1024.0;
                let total_gb = total as f64 / 1024.0 / 1024.0 / 1024.0;
                let free_percent = if total > 0 {
                    (free as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: "live.disk".to_string(),
                        control_name: "Disk Space (C:)".to_string(),
                        breadcrumb_path: format!(
                            "System > Storage > {:.1} GB free of {:.1} GB ({:.1}% free)",
                            free_gb, total_gb, free_percent
                        ),
                        launch_command: "ms-settings:storagesense".to_string(),
                        source: "LIVE".to_string(),
                        description: "Shows free space on your system partition (C: drive)."
                            .to_string(),
                        synonyms: "disk storage space free hard drive c".to_string(),
                    },
                    score: 2.0,
                });
            }
        }
    }

    results
}

// ── Recent Files ────────────────────────────────────────────────────────────
fn scan_recent_files() -> Vec<RecentFileInfo> {
    let mut results = Vec::new();
    unsafe {
        use windows::Win32::System::Com::{
            CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }

    // %APPDATA%\Microsoft\Windows\Recent
    let recent_dir = match std::env::var("APPDATA") {
        Ok(d) => std::path::PathBuf::from(d).join("Microsoft\\Windows\\Recent"),
        Err(_) => return results,
    };

    let entries = match std::fs::read_dir(&recent_dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    let mut file_entries: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("lnk"))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();

    // Sort by modification time, newest first
    file_entries.sort_by(|a, b| b.1.cmp(&a.1));

    // Take at most 200 recent items (the list can be large)
    file_entries.truncate(200);

    for (lnk_path, _) in file_entries {
        // Resolve the .lnk target
        if let Some(target) = resolve_lnk_path(&lnk_path) {
            // Skip system folders, executables, etc. — keep documents
            let path_lower = target.to_lowercase();
            let is_useful = path_lower.ends_with(".pdf")
                || path_lower.ends_with(".docx")
                || path_lower.ends_with(".doc")
                || path_lower.ends_with(".xlsx")
                || path_lower.ends_with(".xls")
                || path_lower.ends_with(".pptx")
                || path_lower.ends_with(".ppt")
                || path_lower.ends_with(".txt")
                || path_lower.ends_with(".md")
                || path_lower.ends_with(".png")
                || path_lower.ends_with(".jpg")
                || path_lower.ends_with(".jpeg")
                || path_lower.ends_with(".mp4")
                || path_lower.ends_with(".mp3")
                || path_lower.ends_with(".zip")
                || path_lower.ends_with(".rs")
                || path_lower.ends_with(".py")
                || path_lower.ends_with(".js")
                || path_lower.ends_with(".ts")
                || path_lower.ends_with(".json")
                || path_lower.ends_with(".html")
                || path_lower.ends_with(".css");

            if !is_useful {
                continue;
            }

            // Get the file name from the target path
            let name = std::path::Path::new(&target)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&target)
                .to_string();

            results.push(RecentFileInfo { name, path: target });
        }
    }

    results
}

fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let len1 = s1.chars().count();
    let len2 = s2.chars().count();
    if len1 == 0 {
        return len2;
    }
    if len2 == 0 {
        return len1;
    }

    let mut row: Vec<usize> = (0..=len2).collect();
    let s1_chars: Vec<char> = s1.chars().collect();
    let s2_chars: Vec<char> = s2.chars().collect();

    for i in 0..len1 {
        let mut prev = i + 1;
        for j in 0..len2 {
            let cost = if s1_chars[i] == s2_chars[j] { 0 } else { 1 };
            let val = std::cmp::min(std::cmp::min(row[j + 1] + 1, prev + 1), row[j] + cost);
            row[j] = prev;
            prev = val;
        }
        row[len2] = prev;
    }
    row[len2]
}

pub fn resolve_lnk_path(lnk_path: &std::path::Path) -> Option<String> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_UNCPRIORITY};

    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        let path_wide: Vec<u16> = lnk_path
            .to_str()?
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
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
}

fn scan_apps() -> Vec<AppInfo> {
    let mut apps = Vec::new();
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Com::{
            CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        use windows::Win32::UI::Shell::{
            BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, SHGetKnownFolderItem,
            KF_FLAG_DEFAULT, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
        };

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);

        let apps_folder: IShellItem =
            match SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, HANDLE::default()) {
                Ok(folder) => folder,
                Err(_) => return apps,
            };

        let enum_items: IEnumShellItems = match apps_folder.BindToHandler(None, &BHID_EnumItems) {
            Ok(e) => e,
            Err(_) => return apps,
        };

        let mut items = [None];
        let mut fetched = 0;
        while enum_items.Next(&mut items, Some(&mut fetched)).is_ok() && fetched == 1 {
            if let Some(item) = &items[0] {
                let display_name_ptr = match item.GetDisplayName(SIGDN_NORMALDISPLAY) {
                    Ok(ptr) => ptr,
                    Err(_) => continue,
                };
                let parsing_name_ptr = match item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) {
                    Ok(ptr) => ptr,
                    Err(_) => {
                        windows::Win32::System::Com::CoTaskMemFree(Some(
                            display_name_ptr.0 as *const _,
                        ));
                        continue;
                    }
                };

                let mut len = 0;
                while *display_name_ptr.0.add(len) != 0 {
                    len += 1;
                }
                let display_name =
                    String::from_utf16_lossy(std::slice::from_raw_parts(display_name_ptr.0, len));

                let mut len = 0;
                while *parsing_name_ptr.0.add(len) != 0 {
                    len += 1;
                }
                let parsing_name =
                    String::from_utf16_lossy(std::slice::from_raw_parts(parsing_name_ptr.0, len));

                windows::Win32::System::Com::CoTaskMemFree(Some(display_name_ptr.0 as *const _));
                windows::Win32::System::Com::CoTaskMemFree(Some(parsing_name_ptr.0 as *const _));

                let display_name_lower = display_name.to_lowercase();
                if display_name_lower.contains("uninstall")
                    || display_name_lower.contains("help")
                    || display_name_lower.contains("documentation")
                    || display_name_lower.contains("readme")
                    || display_name_lower.contains("read me")
                    || display_name_lower.contains("release notes")
                    || display_name_lower.contains("whats new")
                    || display_name_lower.contains("what's new")
                    || display_name_lower.contains("license")
                    || display_name_lower.contains("changelog")
                    || display_name_lower.contains("website")
                    || display_name_lower.contains("manual")
                    || display_name_lower.contains("visit")
                    || display_name_lower.contains("about")
                {
                    continue;
                }

                let path_lower = parsing_name.to_lowercase();
                let is_document = path_lower.ends_with(".txt")
                    || path_lower.ends_with(".url")
                    || path_lower.ends_with(".chm")
                    || path_lower.ends_with(".hlp")
                    || path_lower.ends_with(".pdf")
                    || path_lower.ends_with(".html")
                    || path_lower.ends_with(".htm")
                    || path_lower.ends_with(".png")
                    || path_lower.ends_with(".jpg")
                    || path_lower.ends_with(".jpeg")
                    || path_lower.ends_with(".gif")
                    || path_lower.ends_with(".ico")
                    || path_lower.ends_with(".ini")
                    || path_lower.ends_with(".cfg")
                    || path_lower.ends_with(".xml")
                    || path_lower.ends_with(".json")
                    || path_lower.ends_with(".md")
                    || path_lower.ends_with(".rtf")
                    || path_lower.ends_with(".log")
                    || path_lower.ends_with(".doc")
                    || path_lower.ends_with(".docx")
                    || path_lower.ends_with(".xls")
                    || path_lower.ends_with(".xlsx")
                    || path_lower.ends_with(".ppt")
                    || path_lower.ends_with(".pptx")
                    || path_lower.ends_with(".zip")
                    || path_lower.ends_with(".rar")
                    || path_lower.ends_with(".7z");

                if is_document {
                    continue;
                }

                apps.push(AppInfo {
                    name: display_name,
                    path: parsing_name,
                });
            }
        }
    }

    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps.dedup_by(|a, b| a.name == b.name);
    apps
}

// ── Quick System Actions ────────────────────────────────────────────────────
// Each entry: (trigger_phrases, name, breadcrumb, launch_command, description)
struct QuickAction {
    triggers: &'static [&'static str],
    name: &'static str,
    breadcrumb: &'static str,
    launch_command: &'static str,
    description: &'static str,
}

static QUICK_ACTIONS: &[QuickAction] = &[
    QuickAction {
        triggers: &[
            "theme darker",
            "switch to darker",
            "darker theme",
            "dark mode",
        ],
        name: "Switch Theme: Darker",
        breadcrumb: "Theme Switcher > Darker",
        launch_command: "action:switch_theme:darker",
        description: "Switch launcher theme to Darker.",
    },
    QuickAction {
        triggers: &["theme nord", "switch to nord", "nord theme", "nord darker"],
        name: "Switch Theme: Nord Darker",
        breadcrumb: "Theme Switcher > Nord Darker",
        launch_command: "action:switch_theme:nord",
        description: "Switch launcher theme to Nord Darker.",
    },
    QuickAction {
        triggers: &[
            "theme light",
            "switch to light",
            "light theme",
            "light mode",
        ],
        name: "Switch Theme: Light",
        breadcrumb: "Theme Switcher > Light",
        launch_command: "action:switch_theme:light",
        description: "Switch launcher theme to Light.",
    },
    QuickAction {
        triggers: &[
            "theme omni",
            "switch to omni",
            "omni theme",
            "omni dark",
        ],
        name: "Switch Theme: Omni Dark",
        breadcrumb: "Theme Switcher > Omni Dark",
        launch_command: "action:switch_theme:omni",
        description: "Switch launcher theme to Omni Dark.",
    },
    QuickAction {
        triggers: &[
            "settings",
            "preferences",
            "options",
            "general",
            "advanced",
            "shortcuts",
            "launcher settings",
        ],
        name: "Open Settings",
        breadcrumb: "Settings & Management > Open Settings",
        launch_command: "action:open_settings",
        description: "Open settings folder to edit index.db or configs manually.",
    },
    QuickAction {
        triggers: &["about", "copy version"],
        name: "About / Copy Version",
        breadcrumb: "Settings & Management > About",
        launch_command: "action:copy_version",
        description: "Copy app version to clipboard.",
    },
    QuickAction {
        triggers: &[
            "account",
            "organizations",
            "cloud sync",
            "affiliate dashboard",
        ],
        name: "Account & Sync",
        breadcrumb: "Settings & Management > Account",
        launch_command: "https://github.com/PranshulSoni/Project-Raycast",
        description: "Manage account and sync settings online.",
    },
    QuickAction {
        triggers: &["changelog", "what's new"],
        name: "Changelog",
        breadcrumb: "Settings & Management > Changelog",
        launch_command: "https://github.com/PranshulSoni/Project-Raycast/commits/main",
        description: "View recent changes on GitHub.",
    },
    QuickAction {
        triggers: &[
            "check for app updates",
            "update app",
            "check for extension updates",
        ],
        name: "Check for App Updates",
        breadcrumb: "Settings & Management > Check for Updates",
        launch_command: "https://github.com/PranshulSoni/Project-Raycast",
        description: "Check GitHub repository for new launcher releases.",
    },
    QuickAction {
        triggers: &["check for updates", "windows update"],
        name: "Windows Update",
        breadcrumb: "System > Settings > Windows Update",
        launch_command: "ms-settings:windowsupdate",
        description: "Open Windows Update settings.",
    },
    QuickAction {
        triggers: &[
            "search",
            "search settings",
            "windows search",
            "search permissions",
            "safe search",
            "cloud content search",
            "search history",
        ],
        name: "Windows Search Settings",
        breadcrumb: "Settings > Privacy & Security > Search Permissions",
        launch_command: "ms-settings:search-permissions",
        description:
            "Configure Windows Search permissions, SafeSearch, cloud content, and history.",
    },
    QuickAction {
        triggers: &[
            "search indexing",
            "indexing",
            "indexing options",
            "indexed locations",
            "windows indexing",
            "search index",
            "rebuild search index",
        ],
        name: "Indexing Options",
        breadcrumb: "Control Panel > Indexing Options",
        launch_command: "control.exe /name Microsoft.IndexingOptions",
        description: "Configure indexed locations and rebuild the Windows Search index.",
    },
    QuickAction {
        triggers: &[
            "find my files",
            "search files settings",
            "enhanced search",
            "classic search",
            "file search settings",
        ],
        name: "Searching Windows",
        breadcrumb: "Settings > Privacy & Security > Searching Windows",
        launch_command: "ms-settings:cortana-windowssearch",
        description: "Control classic/enhanced Windows file search and excluded folders.",
    },
    QuickAction {
        triggers: &["copy logs", "show logs", "reveal logs"],
        name: "Copy / Reveal Logs",
        breadcrumb: "Settings & Management > Copy Logs",
        launch_command: "action:copy_logs",
        description: "Copy log file contents to clipboard.",
    },
    QuickAction {
        triggers: &[
            "export settings",
            "import settings",
            "export data",
            "import data",
        ],
        name: "Export / Import Data",
        breadcrumb: "Settings & Management > Data",
        launch_command: "action:open_settings",
        description: "Open settings folder to backup or restore index.db manually.",
    },
    QuickAction {
        triggers: &["open manual", "help", "documentation", "show onboarding"],
        name: "Open Manual",
        breadcrumb: "Settings & Management > Manual",
        launch_command: "https://github.com/PranshulSoni/Project-Raycast#readme",
        description: "Open documentation and onboarding guide.",
    },
    QuickAction {
        triggers: &["store", "share"],
        name: "Store & Share",
        breadcrumb: "Settings & Management > Store",
        launch_command: "https://github.com/PranshulSoni/Project-Raycast",
        description: "Visit the extension store or share the app.",
    },
    QuickAction {
        triggers: &["quick look"],
        name: "Quick Look",
        breadcrumb: "Settings & Management > Quick Look",
        launch_command: "action:quick_look",
        description: "Preview selected file (simulated).",
    },
    QuickAction {
        triggers: &["set volume to 0%", "mute"],
        name: "Set Volume to 0%",
        breadcrumb: "System > Volume > 0%",
        launch_command: "action:volume:0",
        description: "Mute volume.",
    },
    QuickAction {
        triggers: &["set volume to 25%"],
        name: "Set Volume to 25%",
        breadcrumb: "System > Volume > 25%",
        launch_command: "action:volume:25",
        description: "Set volume to 25%.",
    },
    QuickAction {
        triggers: &["set volume to 50%"],
        name: "Set Volume to 50%",
        breadcrumb: "System > Volume > 50%",
        launch_command: "action:volume:50",
        description: "Set volume to 50%.",
    },
    QuickAction {
        triggers: &["set volume to 75%"],
        name: "Set Volume to 75%",
        breadcrumb: "System > Volume > 75%",
        launch_command: "action:volume:75",
        description: "Set volume to 75%.",
    },
    QuickAction {
        triggers: &["set volume to 100%", "max volume"],
        name: "Set Volume to 100%",
        breadcrumb: "System > Volume > 100%",
        launch_command: "action:volume:100",
        description: "Set volume to 100%.",
    },
    QuickAction {
        triggers: &[
            "toggle theme",
            "dark mode",
            "light mode",
            "toggle system appearance",
        ],
        name: "Toggle System Appearance",
        breadcrumb: "System > Personalization > Toggle Theme",
        launch_command: "action:toggle_theme",
        description: "Toggle Windows between Dark and Light mode.",
    },
    QuickAction {
        triggers: &["quit all apps"],
        name: "Quit All Apps",
        breadcrumb: "System > Quit All Apps",
        launch_command: "action:quit_all_apps",
        description: "Close all running applications.",
    },
    QuickAction {
        triggers: &["quit all apps except frontmost", "quit other apps"],
        name: "Quit All Apps Except Frontmost",
        breadcrumb: "System > Quit Other Apps",
        launch_command: "action:quit_other_apps",
        description: "Close all applications except the active one.",
    },
    QuickAction {
        triggers: &["hide all apps except frontmost", "hide other apps"],
        name: "Hide All Apps Except Frontmost",
        breadcrumb: "System > Hide Other Apps",
        launch_command: "action:hide_other_apps",
        description: "Minimize all windows except the active one.",
    },
    QuickAction {
        triggers: &["toggle hdr"],
        name: "Toggle HDR",
        breadcrumb: "System > Display > Toggle HDR",
        launch_command: "action:toggle_hdr",
        description: "Toggle HDR on/off via Win+Alt+B.",
    },
    QuickAction {
        triggers: &["move to desktop 1", "move to virtual desktop 1"],
        name: "Move to Desktop 1",
        breadcrumb: "Window Management > Move to Desktop 1",
        launch_command: "action:window:move_desktop_1",
        description: "Move the active window to virtual desktop 1.",
    },
    QuickAction {
        triggers: &["move to desktop 2", "move to virtual desktop 2"],
        name: "Move to Desktop 2",
        breadcrumb: "Window Management > Move to Desktop 2",
        launch_command: "action:window:move_desktop_2",
        description: "Move the active window to virtual desktop 2.",
    },
    QuickAction {
        triggers: &["move to desktop 3", "move to virtual desktop 3"],
        name: "Move to Desktop 3",
        breadcrumb: "Window Management > Move to Desktop 3",
        launch_command: "action:window:move_desktop_3",
        description: "Move the active window to virtual desktop 3.",
    },
    QuickAction {
        triggers: &["move to desktop 4", "move to virtual desktop 4"],
        name: "Move to Desktop 4",
        breadcrumb: "Window Management > Move to Desktop 4",
        launch_command: "action:window:move_desktop_4",
        description: "Move the active window to virtual desktop 4.",
    },
    QuickAction {
        triggers: &["move to desktop 5", "move to virtual desktop 5"],
        name: "Move to Desktop 5",
        breadcrumb: "Window Management > Move to Desktop 5",
        launch_command: "action:window:move_desktop_5",
        description: "Move the active window to virtual desktop 5.",
    },
    QuickAction {
        triggers: &["move to desktop 6", "move to virtual desktop 6"],
        name: "Move to Desktop 6",
        breadcrumb: "Window Management > Move to Desktop 6",
        launch_command: "action:window:move_desktop_6",
        description: "Move the active window to virtual desktop 6.",
    },
    QuickAction {
        triggers: &["move to desktop 7", "move to virtual desktop 7"],
        name: "Move to Desktop 7",
        breadcrumb: "Window Management > Move to Desktop 7",
        launch_command: "action:window:move_desktop_7",
        description: "Move the active window to virtual desktop 7.",
    },
    QuickAction {
        triggers: &["move to desktop 8", "move to virtual desktop 8"],
        name: "Move to Desktop 8",
        breadcrumb: "Window Management > Move to Desktop 8",
        launch_command: "action:window:move_desktop_8",
        description: "Move the active window to virtual desktop 8.",
    },
    QuickAction {
        triggers: &["move to desktop 9", "move to virtual desktop 9"],
        name: "Move to Desktop 9",
        breadcrumb: "Window Management > Move to Desktop 9",
        launch_command: "action:window:move_desktop_9",
        description: "Move the active window to virtual desktop 9.",
    },
    QuickAction {
        triggers: &["move to desktop 10", "move to virtual desktop 10"],
        name: "Move to Desktop 10",
        breadcrumb: "Window Management > Move to Desktop 10",
        launch_command: "action:window:move_desktop_10",
        description: "Move the active window to virtual desktop 10.",
    },
    QuickAction {
        triggers: &["open desktop 1", "switch to desktop 1", "go to desktop 1"],
        name: "Open Desktop 1",
        breadcrumb: "Window Management > Open Desktop 1",
        launch_command: "action:window:open_desktop_1",
        description: "Switch to virtual desktop 1.",
    },
    QuickAction {
        triggers: &["open desktop 2", "switch to desktop 2", "go to desktop 2"],
        name: "Open Desktop 2",
        breadcrumb: "Window Management > Open Desktop 2",
        launch_command: "action:window:open_desktop_2",
        description: "Switch to virtual desktop 2.",
    },
    QuickAction {
        triggers: &["open desktop 3", "switch to desktop 3", "go to desktop 3"],
        name: "Open Desktop 3",
        breadcrumb: "Window Management > Open Desktop 3",
        launch_command: "action:window:open_desktop_3",
        description: "Switch to virtual desktop 3.",
    },
    QuickAction {
        triggers: &["open desktop 4", "switch to desktop 4", "go to desktop 4"],
        name: "Open Desktop 4",
        breadcrumb: "Window Management > Open Desktop 4",
        launch_command: "action:window:open_desktop_4",
        description: "Switch to virtual desktop 4.",
    },
    QuickAction {
        triggers: &["open desktop 5", "switch to desktop 5", "go to desktop 5"],
        name: "Open Desktop 5",
        breadcrumb: "Window Management > Open Desktop 5",
        launch_command: "action:window:open_desktop_5",
        description: "Switch to virtual desktop 5.",
    },
    QuickAction {
        triggers: &["open desktop 6", "switch to desktop 6", "go to desktop 6"],
        name: "Open Desktop 6",
        breadcrumb: "Window Management > Open Desktop 6",
        launch_command: "action:window:open_desktop_6",
        description: "Switch to virtual desktop 6.",
    },
    QuickAction {
        triggers: &["open desktop 7", "switch to desktop 7", "go to desktop 7"],
        name: "Open Desktop 7",
        breadcrumb: "Window Management > Open Desktop 7",
        launch_command: "action:window:open_desktop_7",
        description: "Switch to virtual desktop 7.",
    },
    QuickAction {
        triggers: &["open desktop 8", "switch to desktop 8", "go to desktop 8"],
        name: "Open Desktop 8",
        breadcrumb: "Window Management > Open Desktop 8",
        launch_command: "action:window:open_desktop_8",
        description: "Switch to virtual desktop 8.",
    },
    QuickAction {
        triggers: &["open desktop 9", "switch to desktop 9", "go to desktop 9"],
        name: "Open Desktop 9",
        breadcrumb: "Window Management > Open Desktop 9",
        launch_command: "action:window:open_desktop_9",
        description: "Switch to virtual desktop 9.",
    },
    QuickAction {
        triggers: &[
            "open desktop 10",
            "switch to desktop 10",
            "go to desktop 10",
        ],
        name: "Open Desktop 10",
        breadcrumb: "Window Management > Open Desktop 10",
        launch_command: "action:window:open_desktop_10",
        description: "Switch to virtual desktop 10.",
    },
    QuickAction {
        triggers: &["close desktop", "close virtual desktop"],
        name: "Close Desktop",
        breadcrumb: "Window Management > Close Desktop",
        launch_command: "action:window:close_desktop",
        description: "Close the current virtual desktop.",
    },
    QuickAction {
        triggers: &["close desktop active", "close active desktop"],
        name: "Close Desktop Active",
        breadcrumb: "Window Management > Close Desktop Active",
        launch_command: "action:window:close_desktop_active",
        description: "Close the current virtual desktop and move windows.",
    },
    QuickAction {
        triggers: &["rename desktop", "rename virtual desktop"],
        name: "Rename Desktop",
        breadcrumb: "Window Management > Rename Desktop",
        launch_command: "action:window:rename_desktop",
        description: "Rename the current virtual desktop.",
    },
    QuickAction {
        triggers: &["move to next desktop", "move to next virtual desktop"],
        name: "Move to Next Desktop",
        breadcrumb: "Window Management > Move to Next Desktop",
        launch_command: "action:window:move_next_desktop",
        description: "Move the active window to the next virtual desktop.",
    },
    QuickAction {
        triggers: &[
            "move to previous desktop",
            "move to previous virtual desktop",
        ],
        name: "Move to Previous Desktop",
        breadcrumb: "Window Management > Move to Previous Desktop",
        launch_command: "action:window:move_previous_desktop",
        description: "Move the active window to the previous virtual desktop.",
    },
    QuickAction {
        triggers: &["move to next display", "move to next monitor"],
        name: "Move to Next Display",
        breadcrumb: "Window Management > Move to Next Display",
        launch_command: "action:window:move_next_display",
        description: "Move the active window to the next display (monitor).",
    },
    QuickAction {
        triggers: &["move to previous display", "move to previous monitor"],
        name: "Move to Previous Display",
        breadcrumb: "Window Management > Move to Previous Display",
        launch_command: "action:window:move_previous_display",
        description: "Move the active window to the previous display (monitor).",
    },
    QuickAction {
        triggers: &["paste sequentially", "paste sequence"],
        name: "Paste Sequentially",
        breadcrumb: "Clipboard > Paste Sequentially",
        launch_command: "action:clipboard:paste_sequentially",
        description: "Paste multiple items from clipboard history in sequence.",
    },
    QuickAction {
        triggers: &["lock", "lock screen", "lock pc", "lock computer"],
        name: "Lock Screen",
        breadcrumb: "System > Security > Lock this PC immediately",
        launch_command: "action:lock",
        description: "Lock the screen immediately.",
    },
    QuickAction {
        triggers: &[
            "shutdown",
            "shut down",
            "power off",
            "turn off computer",
            "turn off pc",
        ],
        name: "Shut Down",
        breadcrumb: "System > Power > Shut down this computer",
        launch_command: "action:shutdown",
        description: "Shut down the computer.",
    },
    QuickAction {
        triggers: &["restart", "reboot", "restart computer", "restart pc"],
        name: "Restart",
        breadcrumb: "System > Power > Restart this computer",
        launch_command: "action:restart",
        description: "Restart the computer.",
    },
    QuickAction {
        triggers: &["sleep", "sleep computer", "sleep pc"],
        name: "Sleep",
        breadcrumb: "System > Power > Put computer to sleep",
        launch_command: "action:sleep",
        description: "Put the computer to sleep.",
    },
    QuickAction {
        triggers: &["hibernate", "hibernate computer", "hibernate pc"],
        name: "Hibernate",
        breadcrumb: "System > Power > Hibernate this computer",
        launch_command: "action:hibernate",
        description: "Hibernate the computer.",
    },
    QuickAction {
        triggers: &["log out", "logout", "sign out", "sign out user"],
        name: "Log Out",
        breadcrumb: "System > Account > Sign out",
        launch_command: "action:logout",
        description: "Sign out of Windows.",
    },
    QuickAction {
        triggers: &[
            "sleep displays",
            "turn off displays",
            "turn off monitor",
            "screen off",
        ],
        name: "Sleep Displays",
        breadcrumb: "System > Power > Turn displays off",
        launch_command: "action:sleep_displays",
        description: "Turn off the connected displays without sleeping the PC.",
    },
    QuickAction {
        triggers: &["show screen saver", "screensaver", "start screensaver"],
        name: "Show Screen Saver",
        breadcrumb: "System > Display > Screen saver",
        launch_command: "action:show_screensaver",
        description: "Start the Windows screen saver.",
    },
    QuickAction {
        triggers: &["clipboard history", "show clipboard", "clipboard"],
        name: "Clipboard History",
        breadcrumb: "Clipboard > History",
        launch_command: "query:clip:",
        description: "Open clipboard history inside the launcher.",
    },
    QuickAction {
        triggers: &[
            "paste latest screenshot",
            "paste screenshot",
            "latest screenshot",
        ],
        name: "Paste Latest Screenshot",
        breadcrumb: "Screenshots > Paste latest screenshot",
        launch_command: "action:paste_latest_screenshot",
        description: "Paste the newest screenshot from Clipboard History.",
    },
    QuickAction {
        triggers: &[
            "reset window position",
            "reset launcher window",
            "center launcher",
        ],
        name: "Reset Window Position",
        breadcrumb: "Settings > Launcher > Reset window position",
        launch_command: "action:reset_window_position",
        description: "Reset the launcher to the active monitor.",
    },
    QuickAction {
        triggers: &["reveal logs", "open logs", "logs folder"],
        name: "Reveal Logs",
        breadcrumb: "Settings > Logs > Reveal logs",
        launch_command: "action:reveal_logs",
        description: "Open the app data folder containing logs.",
    },
    QuickAction {
        triggers: &["show desktop", "peek desktop", "minimize all", "desktop"],
        name: "Show Desktop",
        breadcrumb: "Windows > Desktop > Show desktop",
        launch_command: "action:show_desktop",
        description: "Toggle the desktop view.",
    },
    QuickAction {
        triggers: &["run", "open run", "run dialog", "windows run"],
        name: "Open Run",
        breadcrumb: "Windows > Run dialog",
        launch_command: "action:open_run",
        description: "Open the Windows Run dialog.",
    },
    QuickAction {
        triggers: &[
            "quit active app",
            "close active app",
            "close current window",
            "close foreground window",
        ],
        name: "Quit Active App",
        breadcrumb: "Windows > Active app > Close",
        launch_command: "action:quit_active_app",
        description: "Ask the previously active app window to close.",
    },
    QuickAction {
        triggers: &[
            "empty recycle bin",
            "clear recycle bin",
            "empty trash",
            "recycle bin",
        ],
        name: "Empty Recycle Bin",
        breadcrumb: "System > Storage > Empty the Recycle Bin",
        launch_command: "action:recycle",
        description: "Permanently delete all items in the Recycle Bin.",
    },
    QuickAction {
        triggers: &[
            "open recycle bin",
            "recycle bin folder",
            "open trash",
            "trash folder",
        ],
        name: "Open Recycle Bin",
        breadcrumb: "File System > Recycle Bin",
        launch_command: "action:open_recycle_bin",
        description: "Open the Recycle Bin folder.",
    },
    QuickAction {
        triggers: &["open downloads", "downloads folder", "my downloads"],
        name: "Open Downloads",
        breadcrumb: "File System > User > Downloads",
        launch_command: "action:folder:downloads",
        description: "Open the Downloads folder.",
    },
    QuickAction {
        triggers: &["open desktop", "desktop folder", "go to desktop"],
        name: "Open Desktop",
        breadcrumb: "File System > User > Desktop",
        launch_command: "action:folder:desktop",
        description: "Open the Desktop folder.",
    },
    QuickAction {
        triggers: &["open documents", "documents folder", "my documents"],
        name: "Open Documents",
        breadcrumb: "File System > User > Documents",
        launch_command: "action:folder:documents",
        description: "Open the Documents folder.",
    },
    QuickAction {
        triggers: &[
            "open pictures",
            "pictures folder",
            "my pictures",
            "photos folder",
        ],
        name: "Open Pictures",
        breadcrumb: "File System > User > Pictures",
        launch_command: "action:folder:pictures",
        description: "Open the Pictures folder.",
    },
    QuickAction {
        triggers: &["open music", "music folder", "my music"],
        name: "Open Music",
        breadcrumb: "File System > User > Music",
        launch_command: "action:folder:music",
        description: "Open the Music folder.",
    },
    QuickAction {
        triggers: &["open videos", "videos folder", "my videos"],
        name: "Open Videos",
        breadcrumb: "File System > User > Videos",
        launch_command: "action:folder:videos",
        description: "Open the Videos folder.",
    },
    QuickAction {
        triggers: &["open temp", "temp folder", "temporary files", "open tmp"],
        name: "Open Temp Folder",
        breadcrumb: "System > Temp > %TEMP% folder",
        launch_command: "action:folder:temp",
        description: "Open the Windows temporary files folder.",
    },
    QuickAction {
        triggers: &["open startup", "startup folder", "startup programs folder"],
        name: "Open Startup Folder",
        breadcrumb: "System > Startup > Shell startup programs",
        launch_command: "shell:startup",
        description: "Open the user startup programs folder.",
    },
    QuickAction {
        triggers: &["flush dns", "clear dns", "reset dns", "dns cache"],
        name: "Flush DNS Cache",
        breadcrumb: "Network > DNS > Flush resolver cache",
        launch_command: "action:flushdns",
        description: "Clear the DNS resolver cache.",
    },
    QuickAction {
        triggers: &["open task manager", "task manager", "taskmgr"],
        name: "Open Task Manager",
        breadcrumb: "System > Performance > Task Manager",
        launch_command: "taskmgr.exe",
        description: "Open Task Manager.",
    },
    QuickAction {
        triggers: &["open registry", "registry editor", "regedit"],
        name: "Open Registry Editor",
        breadcrumb: "System > Advanced > Registry Editor",
        launch_command: "regedit.exe",
        description: "Open the Windows Registry Editor.",
    },
    QuickAction {
        triggers: &[
            "open environment variables",
            "environment variables",
            "env variables",
            "path variable",
        ],
        name: "Environment Variables",
        breadcrumb: "System > Advanced System Settings > Environment Variables",
        launch_command: "action:envvars",
        description: "Open Environment Variables settings.",
    },
    QuickAction {
        triggers: &["clear clipboard", "empty clipboard", "reset clipboard"],
        name: "Clear Clipboard",
        breadcrumb: "System > Clipboard > Clear clipboard contents",
        launch_command: "action:clearclip",
        description: "Clear all contents from the clipboard.",
    },
    QuickAction {
        triggers: &["open hosts file", "hosts file", "edit hosts"],
        name: "Open Hosts File",
        breadcrumb: "Network > DNS > Edit hosts file",
        launch_command: "action:hosts",
        description: "Open the system hosts file in Notepad.",
    },
    // ── Windows Settings ────────────────────────────────────────────────────
    QuickAction {
        triggers: &[
            "wifi",
            "wifi settings",
            "wireless",
            "network settings",
            "connect wifi",
        ],
        name: "Wi-Fi Settings",
        breadcrumb: "Settings > Network & Internet > Wi-Fi",
        launch_command: "ms-settings:network-wifi",
        description: "Manage Wi-Fi connections.",
    },
    QuickAction {
        triggers: &[
            "bluetooth",
            "bluetooth settings",
            "pair device",
            "bluetooth devices",
        ],
        name: "Bluetooth Settings",
        breadcrumb: "Settings > Bluetooth & Devices",
        launch_command: "ms-settings:bluetooth",
        description: "Pair and manage Bluetooth devices.",
    },
    QuickAction {
        triggers: &[
            "display settings",
            "screen resolution",
            "resolution",
            "display",
            "monitor",
            "brightness",
        ],
        name: "Display Settings",
        breadcrumb: "Settings > System > Display",
        launch_command: "ms-settings:display",
        description: "Change display resolution, brightness and orientation.",
    },
    QuickAction {
        triggers: &["night light", "blue light", "night mode", "warm colors"],
        name: "Night Light",
        breadcrumb: "Settings > System > Display > Night Light",
        launch_command: "ms-settings:nightlight",
        description: "Reduce blue light with Night Light.",
    },
    QuickAction {
        triggers: &[
            "sound settings",
            "audio settings",
            "volume settings",
            "sound output",
            "speaker",
            "microphone",
        ],
        name: "Sound Settings",
        breadcrumb: "Settings > System > Sound",
        launch_command: "ms-settings:sound",
        description: "Manage audio output and input devices.",
    },
    QuickAction {
        triggers: &[
            "notifications",
            "notification settings",
            "do not disturb",
            "focus assist",
            "app notifications",
        ],
        name: "Notification Settings",
        breadcrumb: "Settings > System > Notifications",
        launch_command: "ms-settings:notifications",
        description: "Control app notifications and Do Not Disturb.",
    },
    QuickAction {
        triggers: &[
            "power settings",
            "battery settings",
            "sleep settings",
            "power plan",
            "battery saver",
        ],
        name: "Power & Battery Settings",
        breadcrumb: "Settings > System > Power & Battery",
        launch_command: "ms-settings:powersleep",
        description: "Configure power plan and battery saver.",
    },
    QuickAction {
        triggers: &[
            "apps settings",
            "installed apps",
            "uninstall app",
            "remove app",
            "add remove programs",
        ],
        name: "Installed Apps",
        breadcrumb: "Settings > Apps > Installed Apps",
        launch_command: "ms-settings:appsfeatures",
        description: "View, modify or uninstall installed apps.",
    },
    QuickAction {
        triggers: &[
            "default apps",
            "default browser",
            "set default",
            "default programs",
        ],
        name: "Default Apps",
        breadcrumb: "Settings > Apps > Default Apps",
        launch_command: "ms-settings:defaultapps",
        description: "Set default apps for file types and protocols.",
    },
    QuickAction {
        triggers: &[
            "startup apps",
            "startup programs",
            "autostart",
            "apps on startup",
        ],
        name: "Startup Apps",
        breadcrumb: "Settings > Apps > Startup",
        launch_command: "ms-settings:startupapps",
        description: "Control which apps launch on startup.",
    },
    QuickAction {
        triggers: &[
            "accounts",
            "account settings",
            "your info",
            "microsoft account",
            "user account",
        ],
        name: "Your Account",
        breadcrumb: "Settings > Accounts > Your Info",
        launch_command: "ms-settings:yourinfo",
        description: "View your Microsoft account info.",
    },
    QuickAction {
        triggers: &[
            "sign in options",
            "pin",
            "windows hello",
            "fingerprint",
            "face recognition",
            "password settings",
        ],
        name: "Sign-In Options",
        breadcrumb: "Settings > Accounts > Sign-In Options",
        launch_command: "ms-settings:signinoptions",
        description: "Manage PIN, password and Windows Hello.",
    },
    QuickAction {
        triggers: &["vpn", "vpn settings", "virtual private network"],
        name: "VPN Settings",
        breadcrumb: "Settings > Network & Internet > VPN",
        launch_command: "ms-settings:network-vpn",
        description: "Configure VPN connections.",
    },
    QuickAction {
        triggers: &["proxy settings", "proxy", "network proxy"],
        name: "Proxy Settings",
        breadcrumb: "Settings > Network & Internet > Proxy",
        launch_command: "ms-settings:network-proxy",
        description: "Configure proxy for network connections.",
    },
    QuickAction {
        triggers: &[
            "windows update",
            "check updates",
            "update windows",
            "updates",
        ],
        name: "Windows Update",
        breadcrumb: "Settings > Windows Update",
        launch_command: "ms-settings:windowsupdate",
        description: "Check for and install Windows updates.",
    },
    QuickAction {
        triggers: &[
            "storage settings",
            "disk space",
            "storage",
            "storage sense",
            "free up space",
        ],
        name: "Storage Settings",
        breadcrumb: "Settings > System > Storage",
        launch_command: "ms-settings:storagesense",
        description: "Manage disk storage and Storage Sense.",
    },
    QuickAction {
        triggers: &[
            "privacy settings",
            "privacy",
            "location",
            "camera access",
            "microphone access",
            "app permissions",
        ],
        name: "Privacy Settings",
        breadcrumb: "Settings > Privacy & Security",
        launch_command: "ms-settings:privacy",
        description: "Control privacy and app permissions.",
    },
    QuickAction {
        triggers: &["location settings", "location access", "location privacy"],
        name: "Location Settings",
        breadcrumb: "Settings > Privacy & Security > Location",
        launch_command: "ms-settings:privacy-location",
        description: "Control which apps can access your location.",
    },
    QuickAction {
        triggers: &["camera settings", "camera access", "camera privacy"],
        name: "Camera Settings",
        breadcrumb: "Settings > Privacy & Security > Camera",
        launch_command: "ms-settings:privacy-webcam",
        description: "Control which apps can access your camera.",
    },
    QuickAction {
        triggers: &[
            "date time settings",
            "date and time",
            "clock settings",
            "time zone",
            "set time",
        ],
        name: "Date & Time Settings",
        breadcrumb: "Settings > Time & Language > Date & Time",
        launch_command: "ms-settings:dateandtime",
        description: "Set date, time and time zone.",
    },
    QuickAction {
        triggers: &[
            "language settings",
            "region settings",
            "keyboard language",
            "input language",
            "add language",
        ],
        name: "Language & Region",
        breadcrumb: "Settings > Time & Language > Language & Region",
        launch_command: "ms-settings:regionlanguage",
        description: "Add or change display and input language.",
    },
    QuickAction {
        triggers: &[
            "mouse settings",
            "pointer speed",
            "scroll speed",
            "mouse buttons",
        ],
        name: "Mouse Settings",
        breadcrumb: "Settings > Bluetooth & Devices > Mouse",
        launch_command: "ms-settings:mousetouchpad",
        description: "Configure mouse pointer and scroll settings.",
    },
    QuickAction {
        triggers: &[
            "touchpad settings",
            "trackpad",
            "gestures",
            "touchpad sensitivity",
        ],
        name: "Touchpad Settings",
        breadcrumb: "Settings > Bluetooth & Devices > Touchpad",
        launch_command: "ms-settings:devices-touchpad",
        description: "Configure touchpad gestures and sensitivity.",
    },
    QuickAction {
        triggers: &[
            "personalization",
            "wallpaper",
            "desktop background",
            "theme",
            "dark mode",
            "light mode",
            "colors",
        ],
        name: "Personalization",
        breadcrumb: "Settings > Personalization",
        launch_command: "ms-settings:personalization",
        description: "Change wallpaper, theme, colors and lock screen.",
    },
    QuickAction {
        triggers: &["taskbar settings", "taskbar", "taskbar icons"],
        name: "Taskbar Settings",
        breadcrumb: "Settings > Personalization > Taskbar",
        launch_command: "ms-settings:taskbar",
        description: "Customize the taskbar.",
    },
    QuickAction {
        triggers: &[
            "accessibility",
            "ease of access",
            "narrator",
            "magnifier",
            "high contrast",
            "color filters",
        ],
        name: "Accessibility Settings",
        breadcrumb: "Settings > Accessibility",
        launch_command: "ms-settings:easeofaccess-display",
        description: "Accessibility features like Narrator, Magnifier.",
    },
    QuickAction {
        triggers: &["developer mode", "developer settings", "sideload"],
        name: "Developer Settings",
        breadcrumb: "Settings > System > For Developers",
        launch_command: "ms-settings:developers",
        description: "Enable developer mode and sideloading.",
    },
    QuickAction {
        triggers: &[
            "activate windows",
            "windows activation",
            "product key",
            "license",
        ],
        name: "Windows Activation",
        breadcrumb: "Settings > System > Activation",
        launch_command: "ms-settings:activation",
        description: "Check or change Windows activation.",
    },
    // ── System Control Actions ───────────────────────────────────────────────
    QuickAction {
        triggers: &[
            "restart explorer",
            "restart explorer.exe",
            "restart file explorer",
            "restart taskbar",
            "explorer restart",
        ],
        name: "Restart Explorer",
        breadcrumb: "System > Process > Restart Windows Explorer",
        launch_command: "action:restart_explorer",
        description: "Kill and restart Windows Explorer (shell).",
    },
    QuickAction {
        triggers: &[
            "volume up",
            "increase volume",
            "louder",
            "turn up volume",
            "volume increase",
        ],
        name: "Volume Up",
        breadcrumb: "System > Audio > Increase master volume",
        launch_command: "action:volume_up",
        description: "Increase master volume by 10%.",
    },
    QuickAction {
        triggers: &[
            "volume down",
            "decrease volume",
            "quieter",
            "turn down volume",
            "volume decrease",
            "lower volume",
        ],
        name: "Volume Down",
        breadcrumb: "System > Audio > Decrease master volume",
        launch_command: "action:volume_down",
        description: "Decrease master volume by 10%.",
    },
    QuickAction {
        triggers: &[
            "mute",
            "unmute",
            "toggle mute",
            "mute volume",
            "silence",
            "toggle volume",
        ],
        name: "Toggle Mute",
        breadcrumb: "System > Audio > Toggle mute/unmute",
        launch_command: "action:toggle_mute",
        description: "Toggle master audio mute on/off.",
    },
    QuickAction {
        triggers: &[
            "toggle bluetooth",
            "bluetooth on",
            "bluetooth off",
            "turn on bluetooth",
            "turn off bluetooth",
            "bt toggle",
        ],
        name: "Toggle Bluetooth",
        breadcrumb: "System > Bluetooth > Toggle on/off",
        launch_command: "action:toggle_bluetooth",
        description: "Toggle Bluetooth radio on or off.",
    },
    QuickAction {
        triggers: &[
            "toggle wifi",
            "wifi on",
            "wifi off",
            "turn on wifi",
            "turn off wifi",
            "airplane mode",
            "toggle wireless",
        ],
        name: "Toggle Wi-Fi",
        breadcrumb: "System > Network > Toggle Wi-Fi on/off",
        launch_command: "action:toggle_wifi",
        description: "Toggle Wi-Fi radio on or off.",
    },
    QuickAction {
        triggers: &[
            "ip config",
            "ip address",
            "my ip",
            "show ip",
            "ipconfig",
            "network info",
        ],
        name: "Show IP Configuration",
        breadcrumb: "Network > IP > Show IP configuration",
        launch_command: "action:ipconfig",
        description: "Show IP address and network configuration.",
    },
    QuickAction {
        triggers: &["release ip", "ip release", "dhcp release"],
        name: "Release IP Address",
        breadcrumb: "Network > IP > Release DHCP lease",
        launch_command: "action:ip_release",
        description: "Release the current DHCP IP address.",
    },
    QuickAction {
        triggers: &["renew ip", "ip renew", "dhcp renew", "renew ip address"],
        name: "Renew IP Address",
        breadcrumb: "Network > IP > Renew DHCP lease",
        launch_command: "action:ip_renew",
        description: "Renew the DHCP IP address.",
    },
    QuickAction {
        triggers: &[
            "event viewer",
            "event log",
            "events",
            "view events",
            "windows logs",
        ],
        name: "Event Viewer",
        breadcrumb: "System > Diagnostics > Event Viewer",
        launch_command: "eventvwr.msc",
        description: "Open Windows Event Viewer.",
    },
    QuickAction {
        triggers: &[
            "device manager",
            "devices",
            "hardware",
            "driver manager",
            "device settings",
        ],
        name: "Device Manager",
        breadcrumb: "System > Hardware > Device Manager",
        launch_command: "devmgmt.msc",
        description: "View and manage hardware devices and drivers.",
    },
    QuickAction {
        triggers: &[
            "services",
            "services manager",
            "windows services",
            "service manager",
            "start service",
            "stop service",
        ],
        name: "Services Manager",
        breadcrumb: "System > Services > Windows Services",
        launch_command: "services.msc",
        description: "Start, stop or configure Windows services.",
    },
    QuickAction {
        triggers: &[
            "disk cleanup",
            "clean disk",
            "free up disk",
            "disk cleanup tool",
            "cleanup",
        ],
        name: "Disk Cleanup",
        breadcrumb: "System > Storage > Disk Cleanup",
        launch_command: "cleanmgr.exe",
        description: "Free up disk space by removing temporary files.",
    },
    QuickAction {
        triggers: &[
            "group policy",
            "gpedit",
            "group policy editor",
            "local group policy",
        ],
        name: "Group Policy Editor",
        breadcrumb: "System > Advanced > Group Policy Editor",
        launch_command: "gpedit.msc",
        description: "Edit local group policy settings.",
    },
    QuickAction {
        triggers: &[
            "performance monitor",
            "perfmon",
            "performance",
            "resource monitor",
            "system performance",
        ],
        name: "Performance Monitor",
        breadcrumb: "System > Diagnostics > Performance Monitor",
        launch_command: "perfmon.msc",
        description: "Monitor system performance and resource usage.",
    },
    QuickAction {
        triggers: &[
            "system restore",
            "restore point",
            "create restore",
            "system protection",
        ],
        name: "System Restore",
        breadcrumb: "System > Recovery > System Restore",
        launch_command: "rundll32.exe shell32.dll,Control_RunDLL sysdm.cpl,,4",
        description: "Configure or start System Restore.",
    },
    QuickAction {
        triggers: &[
            "system info",
            "system information",
            "sysinfo",
            "computer info",
            "about pc",
            "pc info",
        ],
        name: "System Information",
        breadcrumb: "System > Info > System Information",
        launch_command: "msinfo32.exe",
        description: "View detailed system hardware and software info.",
    },
    QuickAction {
        triggers: &[
            "disk management",
            "partition",
            "format disk",
            "manage disks",
            "volumes",
        ],
        name: "Disk Management",
        breadcrumb: "System > Storage > Disk Management",
        launch_command: "diskmgmt.msc",
        description: "Manage disk partitions and volumes.",
    },
    QuickAction {
        triggers: &[
            "task scheduler",
            "scheduled tasks",
            "schedule task",
            "auto tasks",
        ],
        name: "Task Scheduler",
        breadcrumb: "System > Scheduled > Task Scheduler",
        launch_command: "taskschd.msc",
        description: "View and manage scheduled tasks.",
    },
    QuickAction {
        triggers: &[
            "certificate manager",
            "certificates",
            "certmgr",
            "ssl certificates",
            "manage certificates",
        ],
        name: "Certificate Manager",
        breadcrumb: "System > Security > Certificate Manager",
        launch_command: "certmgr.msc",
        description: "Manage security certificates.",
    },
    QuickAction {
        triggers: &[
            "local users",
            "user management",
            "manage users",
            "lusrmgr",
            "local users and groups",
        ],
        name: "Local Users & Groups",
        breadcrumb: "System > Security > Local Users and Groups",
        launch_command: "lusrmgr.msc",
        description: "Manage local user accounts and groups.",
    },
    QuickAction {
        triggers: &["component services", "dcom", "com+ services"],
        name: "Component Services",
        breadcrumb: "System > Advanced > Component Services",
        launch_command: "comexp.msc",
        description: "Manage COM+ applications and DCOM config.",
    },
    QuickAction {
        triggers: &[
            "shared folders",
            "file sharing",
            "shared resources",
            "network shares",
            "share folders",
        ],
        name: "Shared Folders",
        breadcrumb: "System > Network > Shared Folders",
        launch_command: "fsmgmt.msc",
        description: "View and manage shared folders and sessions.",
    },
    QuickAction {
        triggers: &[
            "wifi password",
            "show wifi password",
            "wireless password",
            "network key",
            "wifi key",
        ],
        name: "Show Wi-Fi Password",
        breadcrumb: "Network > Wi-Fi > Show saved Wi-Fi passwords",
        launch_command: "action:wifi_password",
        description: "Show saved Wi-Fi network passwords.",
    },
    QuickAction {
        triggers: &[
            "kill process",
            "end process",
            "force kill",
            "kill app",
            "terminate process",
        ],
        name: "Kill Process by Name",
        breadcrumb: "System > Process > Kill a running process",
        launch_command: "action:kill_process_prompt",
        description: "Type a process name to force-kill it.",
    },
    QuickAction {
        triggers: &[
            "eject cd",
            "eject disk",
            "open tray",
            "eject dvd",
            "open cd tray",
        ],
        name: "Eject CD/DVD Tray",
        breadcrumb: "System > Hardware > Eject optical disc tray",
        launch_command: "action:eject_cd",
        description: "Eject the CD/DVD drive tray.",
    },
    QuickAction {
        triggers: &["maximize", "maximize window", "full screen window"],
        name: "Maximize Window",
        breadcrumb: "Window Management > Maximize",
        launch_command: "action:window:maximize",
        description: "Maximize the active window.",
    },
    QuickAction {
        triggers: &["restore window", "unmaximize window"],
        name: "Restore Window",
        breadcrumb: "Window Management > Restore",
        launch_command: "action:window:restore",
        description: "Restore the active window.",
    },
    QuickAction {
        triggers: &["maximize height", "tall window", "full height"],
        name: "Maximize Height",
        breadcrumb: "Window Management > Maximize Height",
        launch_command: "action:window:maximize_height",
        description: "Resize the active window to full screen height.",
    },
    QuickAction {
        triggers: &["maximize width", "wide window", "full width"],
        name: "Maximize Width",
        breadcrumb: "Window Management > Maximize Width",
        launch_command: "action:window:maximize_width",
        description: "Resize the active window to full screen width.",
    },
    QuickAction {
        triggers: &["move left", "move window left"],
        name: "Move Left",
        breadcrumb: "Window Management > Move Left",
        launch_command: "action:window:move_left",
        description: "Move the active window to the left edge.",
    },
    QuickAction {
        triggers: &["move right", "move window right"],
        name: "Move Right",
        breadcrumb: "Window Management > Move Right",
        launch_command: "action:window:move_right",
        description: "Move the active window to the right edge.",
    },
    QuickAction {
        triggers: &["center", "center window", "align center"],
        name: "Center Window",
        breadcrumb: "Window Management > Center",
        launch_command: "action:window:center",
        description: "Center the active window on screen.",
    },
    QuickAction {
        triggers: &["almost maximize", "almost maximize window"],
        name: "Almost Maximize Window",
        breadcrumb: "Window Management > Almost Maximize",
        launch_command: "action:window:almost_maximize",
        description: "Resize the active window to 95% of the screen.",
    },
    QuickAction {
        triggers: &["reasonable size", "reasonable size window"],
        name: "Reasonable Size Window",
        breadcrumb: "Window Management > Reasonable Size",
        launch_command: "action:window:reasonable_size",
        description: "Resize the active window to 70% of the screen.",
    },
    QuickAction {
        triggers: &["left half", "snap left", "tile left"],
        name: "Tile Left Half",
        breadcrumb: "Window Management > Left Half",
        launch_command: "action:window:left_half",
        description: "Tile the active window to the left half of the screen.",
    },
    QuickAction {
        triggers: &["right half", "snap right", "tile right"],
        name: "Tile Right Half",
        breadcrumb: "Window Management > Right Half",
        launch_command: "action:window:right_half",
        description: "Tile the active window to the right half of the screen.",
    },
    QuickAction {
        triggers: &["top half", "snap top", "tile top"],
        name: "Tile Top Half",
        breadcrumb: "Window Management > Top Half",
        launch_command: "action:window:top_half",
        description: "Tile the active window to the top half of the screen.",
    },
    QuickAction {
        triggers: &["bottom half", "snap bottom", "tile bottom"],
        name: "Tile Bottom Half",
        breadcrumb: "Window Management > Bottom Half",
        launch_command: "action:window:bottom_half",
        description: "Tile the active window to the bottom half of the screen.",
    },
    QuickAction {
        triggers: &["top left quarter", "snap top left", "tile top left"],
        name: "Tile Top Left Quarter",
        breadcrumb: "Window Management > Top Left Quarter",
        launch_command: "action:window:top_left_quarter",
        description: "Tile the active window to the top left quarter.",
    },
    QuickAction {
        triggers: &["top right quarter", "snap top right", "tile top right"],
        name: "Tile Top Right Quarter",
        breadcrumb: "Window Management > Top Right Quarter",
        launch_command: "action:window:top_right_quarter",
        description: "Tile the active window to the top right quarter.",
    },
    QuickAction {
        triggers: &[
            "bottom left quarter",
            "snap bottom left",
            "tile bottom left",
        ],
        name: "Tile Bottom Left Quarter",
        breadcrumb: "Window Management > Bottom Left Quarter",
        launch_command: "action:window:bottom_left_quarter",
        description: "Tile the active window to the bottom left quarter.",
    },
    QuickAction {
        triggers: &[
            "bottom right quarter",
            "snap bottom right",
            "tile bottom right",
        ],
        name: "Tile Bottom Right Quarter",
        breadcrumb: "Window Management > Bottom Right Quarter",
        launch_command: "action:window:bottom_right_quarter",
        description: "Tile the active window to the bottom right quarter.",
    },
    QuickAction {
        triggers: &["left third", "snap left third", "tile left third"],
        name: "Tile Left Third",
        breadcrumb: "Window Management > Left Third",
        launch_command: "action:window:left_third",
        description: "Tile the active window to the left third of the screen.",
    },
    QuickAction {
        triggers: &["center third", "snap center third", "tile center third"],
        name: "Tile Center Third",
        breadcrumb: "Window Management > Center Third",
        launch_command: "action:window:center_third",
        description: "Tile the active window to the center third of the screen.",
    },
    QuickAction {
        triggers: &["right third", "snap right third", "tile right third"],
        name: "Tile Right Third",
        breadcrumb: "Window Management > Right Third",
        launch_command: "action:window:right_third",
        description: "Tile the active window to the right third of the screen.",
    },
    QuickAction {
        triggers: &["left two thirds", "snap left two thirds"],
        name: "Tile Left Two Thirds",
        breadcrumb: "Window Management > Left Two Thirds",
        launch_command: "action:window:left_two_thirds",
        description: "Tile the active window to the left two thirds.",
    },
    QuickAction {
        triggers: &["right two thirds", "snap right two thirds"],
        name: "Tile Right Two Thirds",
        breadcrumb: "Window Management > Right Two Thirds",
        launch_command: "action:window:right_two_thirds",
        description: "Tile the active window to the right two thirds.",
    },
    QuickAction {
        triggers: &["make larger", "increase window size", "enlarge window"],
        name: "Make Window Larger",
        breadcrumb: "Window Management > Enlarge",
        launch_command: "action:window:make_larger",
        description: "Increase the size of the active window.",
    },
    QuickAction {
        triggers: &["make smaller", "decrease window size", "shrink window"],
        name: "Make Window Smaller",
        breadcrumb: "Window Management > Shrink",
        launch_command: "action:window:make_smaller",
        description: "Decrease the size of the active window.",
    },
    QuickAction {
        triggers: &["move top", "move window top"],
        name: "Move Top",
        breadcrumb: "Window Management > Move Top",
        launch_command: "action:window:move_top",
        description: "Move the active window to the top edge.",
    },
    QuickAction {
        triggers: &["move bottom", "move window bottom"],
        name: "Move Bottom",
        breadcrumb: "Window Management > Move Bottom",
        launch_command: "action:window:move_bottom",
        description: "Move the active window to the bottom edge.",
    },
    QuickAction {
        triggers: &["bottom center sixth"],
        name: "Bottom Center Sixth",
        breadcrumb: "Window Management > Bottom Center Sixth",
        launch_command: "action:window:bottom_center_sixth",
        description: "Tile the active window to the bottom center sixth.",
    },
    QuickAction {
        triggers: &["top center sixth"],
        name: "Top Center Sixth",
        breadcrumb: "Window Management > Top Center Sixth",
        launch_command: "action:window:top_center_sixth",
        description: "Tile the active window to the top center sixth.",
    },
    QuickAction {
        triggers: &["bottom left sixth"],
        name: "Bottom Left Sixth",
        breadcrumb: "Window Management > Bottom Left Sixth",
        launch_command: "action:window:bottom_left_sixth",
        description: "Tile the active window to the bottom left sixth.",
    },
    QuickAction {
        triggers: &["bottom right sixth"],
        name: "Bottom Right Sixth",
        breadcrumb: "Window Management > Bottom Right Sixth",
        launch_command: "action:window:bottom_right_sixth",
        description: "Tile the active window to the bottom right sixth.",
    },
    QuickAction {
        triggers: &["top left sixth"],
        name: "Top Left Sixth",
        breadcrumb: "Window Management > Top Left Sixth",
        launch_command: "action:window:top_left_sixth",
        description: "Tile the active window to the top left sixth.",
    },
    QuickAction {
        triggers: &["top right sixth"],
        name: "Top Right Sixth",
        breadcrumb: "Window Management > Top Right Sixth",
        launch_command: "action:window:top_right_sixth",
        description: "Tile the active window to the top right sixth.",
    },
    QuickAction {
        triggers: &["bottom center two thirds"],
        name: "Bottom Center Two Thirds",
        breadcrumb: "Window Management > Bottom Center Two Thirds",
        launch_command: "action:window:bottom_center_two_thirds",
        description: "Tile the active window to the bottom center two thirds.",
    },
    QuickAction {
        triggers: &["top center two thirds"],
        name: "Top Center Two Thirds",
        breadcrumb: "Window Management > Top Center Two Thirds",
        launch_command: "action:window:top_center_two_thirds",
        description: "Tile the active window to the top center two thirds.",
    },
    QuickAction {
        triggers: &["bottom third"],
        name: "Bottom Third",
        breadcrumb: "Window Management > Bottom Third",
        launch_command: "action:window:bottom_third",
        description: "Tile the active window to the bottom third.",
    },
    QuickAction {
        triggers: &["top third"],
        name: "Top Third",
        breadcrumb: "Window Management > Top Third",
        launch_command: "action:window:top_third",
        description: "Tile the active window to the top third.",
    },
    QuickAction {
        triggers: &["bottom three fourths"],
        name: "Bottom Three Fourths",
        breadcrumb: "Window Management > Bottom Three Fourths",
        launch_command: "action:window:bottom_three_fourths",
        description: "Tile the active window to the bottom three fourths.",
    },
    QuickAction {
        triggers: &["top three fourths"],
        name: "Top Three Fourths",
        breadcrumb: "Window Management > Top Three Fourths",
        launch_command: "action:window:top_three_fourths",
        description: "Tile the active window to the top three fourths.",
    },
    QuickAction {
        triggers: &["bottom two thirds"],
        name: "Bottom Two Thirds",
        breadcrumb: "Window Management > Bottom Two Thirds",
        launch_command: "action:window:bottom_two_thirds",
        description: "Tile the active window to the bottom two thirds.",
    },
    QuickAction {
        triggers: &["top two thirds"],
        name: "Top Two Thirds",
        breadcrumb: "Window Management > Top Two Thirds",
        launch_command: "action:window:top_two_thirds",
        description: "Tile the active window to the top two thirds.",
    },
    QuickAction {
        triggers: &["first fourth"],
        name: "First Fourth",
        breadcrumb: "Window Management > First Fourth",
        launch_command: "action:window:first_fourth",
        description: "Tile the active window to the first fourth.",
    },
    QuickAction {
        triggers: &["second fourth"],
        name: "Second Fourth",
        breadcrumb: "Window Management > Second Fourth",
        launch_command: "action:window:second_fourth",
        description: "Tile the active window to the second fourth.",
    },
    QuickAction {
        triggers: &["third fourth"],
        name: "Third Fourth",
        breadcrumb: "Window Management > Third Fourth",
        launch_command: "action:window:third_fourth",
        description: "Tile the active window to the third fourth.",
    },
    QuickAction {
        triggers: &["last fourth"],
        name: "Last Fourth",
        breadcrumb: "Window Management > Last Fourth",
        launch_command: "action:window:last_fourth",
        description: "Tile the active window to the last fourth.",
    },
    QuickAction {
        triggers: &["top first fourth"],
        name: "Top First Fourth",
        breadcrumb: "Window Management > Top First Fourth",
        launch_command: "action:window:top_first_fourth",
        description: "Tile the active window to the top first fourth.",
    },
    QuickAction {
        triggers: &["top second fourth"],
        name: "Top Second Fourth",
        breadcrumb: "Window Management > Top Second Fourth",
        launch_command: "action:window:top_second_fourth",
        description: "Tile the active window to the top second fourth.",
    },
    QuickAction {
        triggers: &["top third fourth"],
        name: "Top Third Fourth",
        breadcrumb: "Window Management > Top Third Fourth",
        launch_command: "action:window:top_third_fourth",
        description: "Tile the active window to the top third fourth.",
    },
    QuickAction {
        triggers: &["top last fourth"],
        name: "Top Last Fourth",
        breadcrumb: "Window Management > Top Last Fourth",
        launch_command: "action:window:top_last_fourth",
        description: "Tile the active window to the top last fourth.",
    },
    QuickAction {
        triggers: &["bottom first fourth"],
        name: "Bottom First Fourth",
        breadcrumb: "Window Management > Bottom First Fourth",
        launch_command: "action:window:bottom_first_fourth",
        description: "Tile the active window to the bottom first fourth.",
    },
    QuickAction {
        triggers: &["bottom second fourth"],
        name: "Bottom Second Fourth",
        breadcrumb: "Window Management > Bottom Second Fourth",
        launch_command: "action:window:bottom_second_fourth",
        description: "Tile the active window to the bottom second fourth.",
    },
    QuickAction {
        triggers: &["bottom third fourth"],
        name: "Bottom Third Fourth",
        breadcrumb: "Window Management > Bottom Third Fourth",
        launch_command: "action:window:bottom_third_fourth",
        description: "Tile the active window to the bottom third fourth.",
    },
    QuickAction {
        triggers: &["bottom last fourth"],
        name: "Bottom Last Fourth",
        breadcrumb: "Window Management > Bottom Last Fourth",
        launch_command: "action:window:bottom_last_fourth",
        description: "Tile the active window to the bottom last fourth.",
    },
    QuickAction {
        triggers: &["last third"],
        name: "Last Third",
        breadcrumb: "Window Management > Last Third",
        launch_command: "action:window:last_third",
        description: "Tile the active window to the last third.",
    },
    QuickAction {
        triggers: &["last three fourths"],
        name: "Last Three Fourths",
        breadcrumb: "Window Management > Last Three Fourths",
        launch_command: "action:window:last_three_fourths",
        description: "Tile the active window to the last three fourths.",
    },
    QuickAction {
        triggers: &["last two thirds"],
        name: "Last Two Thirds",
        breadcrumb: "Window Management > Last Two Thirds",
        launch_command: "action:window:last_two_thirds",
        description: "Tile the active window to the last two thirds.",
    },
    QuickAction {
        triggers: &["first third"],
        name: "First Third",
        breadcrumb: "Window Management > First Third",
        launch_command: "action:window:first_third",
        description: "Tile the active window to the first third.",
    },
    QuickAction {
        triggers: &["first three fourths"],
        name: "First Three Fourths",
        breadcrumb: "Window Management > First Three Fourths",
        launch_command: "action:window:first_three_fourths",
        description: "Tile the active window to the first three fourths.",
    },
    QuickAction {
        triggers: &["first two thirds"],
        name: "First Two Thirds",
        breadcrumb: "Window Management > First Two Thirds",
        launch_command: "action:window:first_two_thirds",
        description: "Tile the active window to the first two thirds.",
    },
    QuickAction {
        triggers: &["center half"],
        name: "Center Half",
        breadcrumb: "Window Management > Center Half",
        launch_command: "action:window:center_half",
        description: "Tile the active window to the center half.",
    },
    QuickAction {
        triggers: &["center three fourths"],
        name: "Center Three Fourths",
        breadcrumb: "Window Management > Center Three Fourths",
        launch_command: "action:window:center_three_fourths",
        description: "Tile the active window to the center three fourths.",
    },
    QuickAction {
        triggers: &["center two thirds"],
        name: "Center Two Thirds",
        breadcrumb: "Window Management > Center Two Thirds",
        launch_command: "action:window:center_two_thirds",
        description: "Tile the active window to the center two thirds.",
    },
    QuickAction {
        triggers: &[
            "toggle always on top",
            "always on top",
            "pin window",
            "pin on top",
        ],
        name: "Toggle Always on Top",
        breadcrumb: "Window Management > Always on Top",
        launch_command: "action:window:toggle_always_on_top",
        description: "Toggle whether the active window is always on top.",
    },
    QuickAction {
        triggers: &["create quicklink", "new quicklink", "add quicklink"],
        name: "Create Quicklink",
        breadcrumb: "Quicklinks > Add custom web shortcut",
        launch_command: "action:create_quicklink",
        description: "Configure a new keyword-based search engine shortcut.",
    },
    QuickAction {
        triggers: &["create snippet", "new snippet", "add snippet"],
        name: "Create Snippet",
        breadcrumb: "Snippets > Add custom text template",
        launch_command: "action:create_snippet",
        description: "Create a new text snippet template.",
    },
    QuickAction {
        triggers: &["search quicklinks"],
        name: "Search Quicklinks",
        breadcrumb: "Quicklinks > Manage and search",
        launch_command: "ql:",
        description: "Browse and search all web search shortcuts.",
    },
    QuickAction {
        triggers: &["search snippets"],
        name: "Search Snippets",
        breadcrumb: "Snippets > Manage and search",
        launch_command: "snip:",
        description: "Browse and search all text snippets.",
    },
    QuickAction {
        triggers: &["create note", "new note", "add note"],
        name: "Create Note",
        breadcrumb: "Notes > Create Note",
        launch_command: "action:create_note",
        description: "Create a new text note and open it in Notepad.",
    },
    QuickAction {
        triggers: &["search notes", "browse notes", "open notes"],
        name: "Search Notes",
        breadcrumb: "Notes > Search Notes",
        launch_command: "notes:",
        description: "Browse and search all your saved notes.",
    },
    QuickAction {
        triggers: &[
            "create focus category",
            "new focus category",
            "add focus category",
        ],
        name: "Create Focus Category",
        breadcrumb: "Focus > Add focus category",
        launch_command: "action:create_focus_category",
        description: "Create a new focus category with blocked apps.",
    },
    QuickAction {
        triggers: &["toggle focus session", "stop focus session"],
        name: "Toggle Focus Session",
        breadcrumb: "Focus > Toggle Session",
        launch_command: "action:toggle_focus_session",
        description: "Launch Windows Focus Session to toggle state.",
    },
    QuickAction {
        triggers: &["vs code new window", "vscode new window", "code new window"],
        name: "VS Code: New Window",
        breadcrumb: "Apps > VS Code",
        launch_command: "cmd /c code -n",
        description: "Open a new VS Code window.",
    },
    QuickAction {
        triggers: &["chrome incognito", "chrome private window"],
        name: "Chrome: Incognito Window",
        breadcrumb: "Apps > Chrome",
        launch_command: "cmd /c start chrome --incognito",
        description: "Open a new Chrome incognito window.",
    },
    QuickAction {
        triggers: &["ask clipboard", "ai clipboard", "chat clipboard"],
        name: "Ask Clipboard using AI",
        breadcrumb: "AI > Ask Clipboard",
        launch_command: "action:ask_clipboard",
        description: "Start an AI chat using your current clipboard text.",
    },
    QuickAction {
        triggers: &["typing practice", "monkeytype", "typing test"],
        name: "Typing Practice",
        breadcrumb: "Fun > Typing Practice",
        launch_command: "https://monkeytype.com",
        description: "Open Monkeytype in your browser.",
    },
    QuickAction {
        triggers: &["reload script commands", "refresh script commands"],
        name: "Reload Script Commands",
        breadcrumb: "Developer > Reload Script Commands",
        launch_command: "action:reload_script_commands",
        description: "Reload all script commands from disk.",
    },
    QuickAction {
        triggers: &["export snippets"],
        name: "Export Snippets",
        breadcrumb: "Snippets > Export to Desktop",
        launch_command: "action:export_snippets",
        description: "Export all snippets to snippets_export.json on your Desktop.",
    },
    QuickAction {
        triggers: &["import snippets"],
        name: "Import Snippets",
        breadcrumb: "Snippets > Import from Desktop",
        launch_command: "action:import_snippets",
        description: "Import snippets from snippets_import.json on your Desktop.",
    },
    QuickAction {
        triggers: &["export quicklinks"],
        name: "Export Quicklinks",
        breadcrumb: "Quicklinks > Export to Desktop",
        launch_command: "action:export_quicklinks",
        description: "Export all quicklinks to quicklinks_export.json on your Desktop.",
    },
    QuickAction {
        triggers: &["import quicklinks"],
        name: "Import Quicklinks",
        breadcrumb: "Quicklinks > Import from Desktop",
        launch_command: "action:import_quicklinks",
        description: "Import quicklinks from quicklinks_import.json on your Desktop.",
    },
    QuickAction {
        triggers: &[
            "mute",
            "mute sound",
            "silence pc",
            "mute volume",
            "turn off sound",
        ],
        name: "Mute Audio",
        breadcrumb: "System > Audio > Mute sound output",
        launch_command: "action:mute",
        description: "Mute the master system audio volume.",
    },
    QuickAction {
        triggers: &["unmute", "unmute sound", "unmute volume", "turn on sound"],
        name: "Unmute Audio",
        breadcrumb: "System > Audio > Unmute sound output",
        launch_command: "action:unmute",
        description: "Unmute the master system audio volume.",
    },
    QuickAction {
        triggers: &[
            "toggle hidden files",
            "show hidden files",
            "hide hidden files",
            "hidden folders",
        ],
        name: "Toggle Hidden Files",
        breadcrumb: "System > Explorer > Show or hide hidden files",
        launch_command: "action:toggle_hidden_files",
        description: "Toggle visibility of hidden files and folders in Windows Explorer.",
    },
    QuickAction {
        triggers: &["play", "pause", "play/pause", "music pause", "resume play"],
        name: "Play/Pause Media",
        breadcrumb: "System > Media > Media playback control",
        launch_command: "action:media:play_pause",
        description: "Toggle playback state of background media players.",
    },
    QuickAction {
        triggers: &["next track", "skip track", "next song", "skip song"],
        name: "Next Track",
        breadcrumb: "System > Media > Media playback control",
        launch_command: "action:media:next",
        description: "Skip to the next media track.",
    },
    QuickAction {
        triggers: &["previous track", "prev track", "previous song", "prev song"],
        name: "Previous Track",
        breadcrumb: "System > Media > Media playback control",
        launch_command: "action:media:prev",
        description: "Return to the previous media track.",
    },
    QuickAction {
        triggers: &["stop playback", "stop music", "stop video"],
        name: "Stop Playback",
        breadcrumb: "System > Media > Media playback control",
        launch_command: "action:media:stop",
        description: "Stop media playback.",
    },
    QuickAction {
        triggers: &[
            "night light",
            "nightlight",
            "blue light settings",
            "screen warmth",
        ],
        name: "Night Light Settings",
        breadcrumb: "System > Display > Night Light settings",
        launch_command: "ms-settings:nightlight",
        description: "Open the Display settings page to toggle or configure Night Light.",
    },
    QuickAction {
        triggers: &[
            "color picker",
            "colorpicker",
            "picker",
            "hex picker",
            "eye dropper",
            "eyedropper",
            "pick color",
        ],
        name: "Color Picker",
        breadcrumb: "System > GDI > Capture screen pixel color",
        launch_command: "action:color_picker",
        description: "Launch full-screen pixel color picker to copy Hex colors to clipboard.",
    },
];

fn get_quick_actions(query: &str) -> Vec<SearchResult> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        return vec![];
    }

    let mut matches = Vec::new();
    for action in QUICK_ACTIONS {
        let mut best_score = 0.0f32;
        for &trigger in action.triggers {
            let t = trigger.to_lowercase();
            let score = if t == q {
                3.5
            } else if t.starts_with(&q) {
                3.0
            } else if q.starts_with(&t) {
                2.8
            } else if t.contains(&q) {
                2.5
            } else if q.contains(&t) {
                2.3
            } else {
                // Word-level match
                let t_words: Vec<&str> = t.split_whitespace().collect();
                let q_words: Vec<&str> = q.split_whitespace().collect();
                let matched = q_words.iter().filter(|w| t_words.contains(w)).count();
                if matched > 0 {
                    let ratio = matched as f32 / q_words.len().max(1) as f32;
                    if ratio >= 0.5 {
                        1.5 + ratio
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            };
            if score > best_score {
                best_score = score;
            }
        }
        if best_score > 0.0 {
            matches.push(SearchResult {
                entry: CatalogEntry {
                    id: format!("action.{}", action.name.to_lowercase().replace(' ', "_")),
                    control_name: action.name.to_string(),
                    breadcrumb_path: action.breadcrumb.to_string(),
                    launch_command: action.launch_command.to_string(),
                    source: "ACTION".to_string(),
                    description: action.description.to_string(),
                    synonyms: action.triggers.join("|"),
                },
                score: best_score,
            });
        }
    }
    matches.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches
}

// ── Calculator: recursive-descent expression parser ────────────────────────
// Supports: +, -, *, /, ^, %, parentheses, unary minus
// Named functions: sqrt, abs, round, floor, ceil, sin, cos, tan, log, ln
// Special form: "N% of M" → N/100 * M
pub fn try_calc(input: &str) -> Option<f64> {
    let s = input.trim();
    // Must contain at least one digit to be a math expression
    if !s.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }

    // Handle "X% of Y" shorthand
    let s = if let Some(pct_of) = try_pct_of(s) {
        return Some(pct_of);
    } else {
        s.to_string()
    };

    let tokens = tokenize(&s)?;
    // Guard the recursive-descent parser against a stack overflow on pathological input:
    // deeply nested parentheses recurse one frame per level, and search runs on a worker
    // thread. No real calculation nests anywhere near this deep.
    let mut depth = 0i32;
    let mut max_depth = 0i32;
    for t in &tokens {
        match t {
            Token::LParen => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            Token::RParen => depth -= 1,
            _ => {}
        }
    }
    if max_depth > 128 {
        return None;
    }
    let mut pos = 0usize;
    let result = parse_expr(&tokens, &mut pos)?;
    // Consume any trailing whitespace tokens
    while pos < tokens.len() {
        if tokens[pos] != Token::EOF {
            return None;
        }
        pos += 1;
    }
    if result.is_nan() || result.is_infinite() {
        return None;
    }
    Some(result)
}

fn try_pct_of(s: &str) -> Option<f64> {
    // Match "N% of M" case-insensitively.
    // SAFETY: Use ASCII-only case-insensitive search on the original bytes so the
    // returned index is always valid for slicing `s`. Lowercasing can change UTF-8
    // byte length (e.g. Kelvin sign \u{212A} lowercases to 'k'), so we must never
    // reuse an offset from a lowercased copy to slice the original string.
    let needle = b"% of ";
    let bytes = s.as_bytes();
    let idx = (0..bytes.len().saturating_sub(needle.len() - 1)).find(|&i| {
        bytes[i..]
            .get(..needle.len())
            .map_or(false, |w| w.eq_ignore_ascii_case(needle))
    })?;
    let pct_str = s[..idx].trim();
    let rest_str = s[idx + needle.len()..].trim();
    let pct: f64 = pct_str.parse().ok()?;
    let base: f64 = rest_str.parse().ok()?;
    Some(pct / 100.0 * base)
}

#[derive(Debug, PartialEq, Clone)]
enum Token {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    LParen,
    RParen,
    Ident(String),
    EOF,
}

fn tokenize(s: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = s.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' => {
                i += 1;
            }
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' | '×' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' | '÷' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                i += 1;
            } // ignore comma separators
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let n: f64 = num_str.parse().ok()?;
                tokens.push(Token::Num(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(word.to_lowercase()));
            }
            _ => return None, // unknown character → not a math expression
        }
    }
    tokens.push(Token::EOF);
    Some(tokens)
}

// expr = term (('+' | '-') term)*
fn parse_expr(tokens: &[Token], pos: &mut usize) -> Option<f64> {
    let mut left = parse_term(tokens, pos)?;
    loop {
        match tokens.get(*pos) {
            Some(Token::Plus) => {
                *pos += 1;
                left += parse_term(tokens, pos)?;
            }
            Some(Token::Minus) => {
                *pos += 1;
                left -= parse_term(tokens, pos)?;
            }
            _ => break,
        }
    }
    Some(left)
}

// term = power (('*' | '/' | '%') power)*
fn parse_term(tokens: &[Token], pos: &mut usize) -> Option<f64> {
    let mut left = parse_power(tokens, pos)?;
    loop {
        match tokens.get(*pos) {
            Some(Token::Star) => {
                *pos += 1;
                left *= parse_power(tokens, pos)?;
            }
            Some(Token::Slash) => {
                *pos += 1;
                let r = parse_power(tokens, pos)?;
                if r == 0.0 {
                    return None;
                }
                left /= r;
            }
            Some(Token::Percent) => {
                // Check if next token is 'of' (handled earlier) or treat as modulo
                *pos += 1;
                match tokens.get(*pos) {
                    Some(Token::Ident(w)) if w == "of" => {
                        *pos += 1;
                        let base = parse_power(tokens, pos)?;
                        left = left / 100.0 * base;
                    }
                    _ => {
                        // treat as percentage of the next value if present, else /100
                        left = left / 100.0;
                    }
                }
            }
            _ => break,
        }
    }
    Some(left)
}

// power = unary ('^' power)?  (right-associative)
fn parse_power(tokens: &[Token], pos: &mut usize) -> Option<f64> {
    let base = parse_unary(tokens, pos)?;
    if matches!(tokens.get(*pos), Some(Token::Caret)) {
        *pos += 1;
        let exp = parse_power(tokens, pos)?;
        Some(base.powf(exp))
    } else {
        Some(base)
    }
}

// unary = '-' unary | primary
fn parse_unary(tokens: &[Token], pos: &mut usize) -> Option<f64> {
    if matches!(tokens.get(*pos), Some(Token::Minus)) {
        *pos += 1;
        Some(-parse_unary(tokens, pos)?)
    } else {
        parse_primary(tokens, pos)
    }
}

// primary = number | ident '(' expr ')' | '(' expr ')'
fn parse_primary(tokens: &[Token], pos: &mut usize) -> Option<f64> {
    match tokens.get(*pos)?.clone() {
        Token::Num(n) => {
            *pos += 1;
            Some(n)
        }
        Token::LParen => {
            *pos += 1;
            let val = parse_expr(tokens, pos)?;
            if tokens.get(*pos) == Some(&Token::RParen) {
                *pos += 1;
            }
            Some(val)
        }
        Token::Ident(name) => {
            *pos += 1;
            // Named functions expect a parenthesised argument
            if tokens.get(*pos) == Some(&Token::LParen) {
                *pos += 1;
                let arg = parse_expr(tokens, pos)?;
                if tokens.get(*pos) == Some(&Token::RParen) {
                    *pos += 1;
                }
                match name.as_str() {
                    "sqrt" => Some(arg.sqrt()),
                    "abs" => Some(arg.abs()),
                    "round" => Some(arg.round()),
                    "floor" => Some(arg.floor()),
                    "ceil" => Some(arg.ceil()),
                    "sin" => Some(arg.to_radians().sin()),
                    "cos" => Some(arg.to_radians().cos()),
                    "tan" => Some(arg.to_radians().tan()),
                    "log" => Some(arg.log10()),
                    "ln" => Some(arg.ln()),
                    _ => None,
                }
            } else {
                // Named constants
                match name.as_str() {
                    "pi" | "π" => Some(std::f64::consts::PI),
                    "e" => Some(std::f64::consts::E),
                    _ => None,
                }
            }
        }
        _ => None,
    }
}

fn get_bmp_dimensions(path: &str) -> Option<(i32, i32)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 26];
    file.read_exact(&mut header).ok()?;
    if &header[0..2] != b"BM" {
        return None;
    }
    let width = i32::from_le_bytes(header[18..22].try_into().ok()?);
    let height = i32::from_le_bytes(header[22..26].try_into().ok()?);
    Some((width.abs(), height.abs()))
}

fn parse_time_range(query: &str) -> Option<(i64, i64, String)> {
    let q = query.to_lowercase();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let local_time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };

    let mut time_zone_info = windows::Win32::System::Time::TIME_ZONE_INFORMATION::default();
    let _ = unsafe { windows::Win32::System::Time::GetTimeZoneInformation(&mut time_zone_info) };
    let _bias_minutes = time_zone_info.Bias;

    let seconds_since_midnight = (local_time.wHour as i64 * 3600)
        + (local_time.wMinute as i64 * 60)
        + local_time.wSecond as i64;
    let today_start = now - seconds_since_midnight;
    let yesterday_start = today_start - 86400;

    let mut start_time = 0;
    let mut end_time = 0;
    let mut time_phrase = "";

    if q.contains("yesterday before lunch") {
        time_phrase = "yesterday before lunch";
        start_time = yesterday_start + 8 * 3600;
        end_time = yesterday_start + 12 * 3600;
    } else if q.contains("yesterday after lunch") {
        time_phrase = "yesterday after lunch";
        start_time = yesterday_start + 13 * 3600;
        end_time = yesterday_start + 17 * 3600;
    } else if q.contains("yesterday") {
        time_phrase = "yesterday";
        start_time = yesterday_start;
        end_time = today_start;
    } else if q.contains("before lunch") {
        time_phrase = "before lunch";
        start_time = today_start + 8 * 3600;
        end_time = today_start + 12 * 3600;
    } else if q.contains("after lunch") {
        time_phrase = "after lunch";
        start_time = today_start + 13 * 3600;
        end_time = today_start + 17 * 3600;
    } else if q.contains("before the meeting") {
        time_phrase = "before the meeting";
        start_time = today_start + 8 * 3600;
        end_time = today_start + 10 * 3600;
    } else if q.contains("this morning") {
        time_phrase = "this morning";
        start_time = today_start + 6 * 3600;
        end_time = today_start + 12 * 3600;
    } else if q.contains("this afternoon") {
        time_phrase = "this afternoon";
        start_time = today_start + 12 * 3600;
        end_time = today_start + 17 * 3600;
    } else if q.contains("earlier today") || q.contains("earlier") {
        time_phrase = if q.contains("earlier today") {
            "earlier today"
        } else {
            "earlier"
        };
        start_time = today_start;
        end_time = now;
    } else if q.contains("recently") {
        time_phrase = "recently";
        start_time = now - 6 * 3600;
        end_time = now;
    } else if q.contains("today") {
        time_phrase = "today";
        start_time = today_start;
        end_time = now;
    } else if q.contains("this week") {
        time_phrase = "this week";
        start_time = today_start - 6 * 86400;
        end_time = now;
    } else if q.contains("last week") {
        time_phrase = "last week";
        start_time = today_start - 7 * 86400;
        end_time = now;
    } else if q.contains("last month") {
        time_phrase = "last month";
        start_time = today_start - 30 * 86400;
        end_time = now;
    }

    if !time_phrase.is_empty() {
        // Strip question/filler words anywhere so natural recall ("what was I working on
        // yesterday") leaves an EMPTY keyword → all events in the window, instead of
        // using the leftover words as a title filter that matches nothing.
        const FILLER: &[&str] = &[
            "what", "whats", "what's", "was", "were", "am", "is", "i", "working", "work", "worked",
            "on", "doing", "do", "did", "show", "me", "my", "the", "a", "an", "opened", "open",
            "edited", "edit", "visited", "visit", "used", "use", "using", "before", "after",
            "during", "at", "in", "with", "that", "this", "when", "which", "files", "file", "code",
            "stuff", "things", "going", "up", "to", "of", "and",
        ];
        let clean_query = q
            .replace(time_phrase, " ")
            .split_whitespace()
            .filter(|w| {
                let bare = w.trim_matches(|c: char| !c.is_alphanumeric());
                !bare.is_empty() && !FILLER.contains(&bare)
            })
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();

        return Some((start_time, end_time, clean_query));
    }

    None
}

fn parse_sequential_query(query: &str) -> Option<(String, String, i64, i64)> {
    let q = query.to_lowercase();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let local_time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let seconds_since_midnight = (local_time.wHour as i64 * 3600)
        + (local_time.wMinute as i64 * 60)
        + local_time.wSecond as i64;
    let today_start = now - seconds_since_midnight;
    let yesterday_start = today_start - 86400;

    let (direction, anchor, time_start, time_end) = if let Some(rest) = q.strip_prefix("after ") {
        let (app, ts, te) =
            parse_sequential_anchor_and_time(rest.trim(), today_start, yesterday_start, now);
        ("after".to_string(), app, ts, te)
    } else if let Some(rest) = q.strip_prefix("before ") {
        let (app, ts, te) =
            parse_sequential_anchor_and_time(rest.trim(), today_start, yesterday_start, now);
        ("before".to_string(), app, ts, te)
    } else if let Some(rest) = q.strip_prefix("what did i use after ") {
        let (app, ts, te) =
            parse_sequential_anchor_and_time(rest.trim(), today_start, yesterday_start, now);
        ("after".to_string(), app, ts, te)
    } else if let Some(rest) = q.strip_prefix("what did i use before ") {
        let (app, ts, te) =
            parse_sequential_anchor_and_time(rest.trim(), today_start, yesterday_start, now);
        ("before".to_string(), app, ts, te)
    } else if let Some(rest) = q.strip_prefix("what was i doing after ") {
        let (app, ts, te) =
            parse_sequential_anchor_and_time(rest.trim(), today_start, yesterday_start, now);
        ("after".to_string(), app, ts, te)
    } else if let Some(rest) = q.strip_prefix("what was i doing before ") {
        let (app, ts, te) =
            parse_sequential_anchor_and_time(rest.trim(), today_start, yesterday_start, now);
        ("before".to_string(), app, ts, te)
    } else if let Some(idx) = q.find(" after ") {
        let app = q[idx + 7..].trim().to_string();
        let (clean_app, ts, te) =
            parse_sequential_anchor_and_time(&app, today_start, yesterday_start, now);
        ("after".to_string(), clean_app, ts, te)
    } else if let Some(idx) = q.find(" before ") {
        let app = q[idx + 8..].trim().to_string();
        let (clean_app, ts, te) =
            parse_sequential_anchor_and_time(&app, today_start, yesterday_start, now);
        ("before".to_string(), clean_app, ts, te)
    } else if let Some(idx) = q.find(" then ") {
        let after_app = q[..idx].trim().to_string();
        let (clean_app, ts, te) =
            parse_sequential_anchor_and_time(&after_app, today_start, yesterday_start, now);
        ("after".to_string(), clean_app, ts, te)
    } else {
        return None;
    };

    if anchor.is_empty() {
        return None;
    }
    Some((anchor, direction, time_start, time_end))
}

fn parse_sequential_anchor_and_time(
    s: &str,
    today_start: i64,
    yesterday_start: i64,
    now: i64,
) -> (String, i64, i64) {
    let s = s.trim();
    if s.contains("yesterday") {
        let app = s.replace("yesterday", "").trim().to_string();
        (app, yesterday_start, today_start)
    } else if s.contains("today") {
        let app = s.replace("today", "").trim().to_string();
        (app, today_start, now)
    } else if s.contains("this morning") {
        let app = s.replace("this morning", "").trim().to_string();
        (app, today_start + 6 * 3600, today_start + 12 * 3600)
    } else if s.contains("this afternoon") {
        let app = s.replace("this afternoon", "").trim().to_string();
        (app, today_start + 12 * 3600, today_start + 17 * 3600)
    } else if s.contains("last week") {
        let app = s.replace("last week", "").trim().to_string();
        (app, today_start - 7 * 86400, now)
    } else {
        let now_local = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        (s.to_string(), today_start - 7 * 86400, now_local)
    }
}

fn take_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn take_last_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars().rev().take(max).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

fn ellipsize_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", take_chars(s, max.saturating_sub(3)))
    }
}

fn memory_home_description(today: i64, yesterday: i64, total: i64) -> String {
    format!(
        "{} events today, {} yesterday, {} total. Stored locally on this PC.",
        today, yesterday, total
    )
}

fn workday_memory_query_days(query: &str) -> Option<i64> {
    let asks_work = query.contains("working on")
        || query.contains("worked on")
        || query.contains("what was i doing")
        || query.contains("what did i do");
    if !asks_work {
        return None;
    }
    if query.contains("yesterday") {
        Some(1)
    } else if query.contains("today") {
        Some(0)
    } else {
        None
    }
}

fn count_memory_events(conn: &Connection, start: i64, end: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM memory_events WHERE timestamp >= ? AND timestamp < ?",
        params![start, end],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn memory_source_summary(conn: &Connection, start: i64, end: i64) -> String {
    let mut stmt = match conn.prepare(
        "SELECT source, COUNT(*) FROM memory_events
         WHERE timestamp >= ? AND timestamp < ?
         GROUP BY source ORDER BY COUNT(*) DESC LIMIT 4",
    ) {
        Ok(s) => s,
        Err(_) => return "No captured events yet.".to_string(),
    };
    let parts: Vec<String> = stmt
        .query_map(params![start, end], |row| {
            Ok(format!(
                "{} {}",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?
            ))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    if parts.is_empty() {
        "No captured events yet.".to_string()
    } else {
        parts.join(", ")
    }
}

type MemoryEventRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn latest_memory_session_events(conn: &Connection) -> Vec<MemoryEventRow> {
    let mut stmt = match conn.prepare(
        "SELECT id, timestamp, source, event_type, title, coalesce(detail, ''), coalesce(app_name, ''), path, url
         FROM memory_events
         ORDER BY timestamp DESC LIMIT 200",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows: Vec<MemoryEventRow> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                normalize_event_timestamp(row.get::<_, i64>(1)?),
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    let mut session = Vec::new();
    let mut prev_ts = None;
    for row in rows {
        let ts = row.1;
        if let Some(prev) = prev_ts {
            if prev - ts > 30 * 60 {
                break;
            }
        }
        prev_ts = Some(ts);
        session.push(row);
    }
    session
}

fn session_source_summary(events: &[MemoryEventRow]) -> String {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for event in events {
        *counts.entry(event.2.as_str()).or_default() += 1;
    }
    if counts.is_empty() {
        "No captured events yet.".to_string()
    } else {
        counts
            .into_iter()
            .map(|(source, count)| format!("{} {}", source, count))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn local_day_bounds(days_ago: i64) -> (i64, i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let local_time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let seconds_since_midnight = (local_time.wHour as i64 * 3600)
        + (local_time.wMinute as i64 * 60)
        + local_time.wSecond as i64;
    let today_start = now - seconds_since_midnight;
    let start = today_start - days_ago * 86400;
    (start, start + 86400)
}

fn make_fts_prefix_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|w| {
            let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
            format!("{}*", clean)
        })
        .filter(|w| w.len() > 1)
        .collect::<Vec<String>>()
        .join(" ")
}

fn normalize_event_timestamp(timestamp: i64) -> i64 {
    if timestamp > 11_644_473_600_000_000 {
        (timestamp / 1_000_000) - 11_644_473_600
    } else if timestamp > 10_000_000_000_000 {
        timestamp / 1_000_000
    } else if timestamp > 10_000_000_000 {
        timestamp / 1_000
    } else {
        timestamp
    }
}

#[cfg(test)]
fn format_unix_date(timestamp: i64) -> String {
    let timestamp = normalize_event_timestamp(timestamp);
    let filetime_val = ((timestamp as i128) + 11_644_473_600) * 10_000_000;
    if filetime_val < 0 {
        return "1970-01-01".to_string();
    }
    let ft = windows::Win32::Foundation::FILETIME {
        dwLowDateTime: (filetime_val & 0xFFFFFFFF) as u32,
        dwHighDateTime: ((filetime_val >> 32) & 0xFFFFFFFF) as u32,
    };
    let mut st = windows::Win32::Foundation::SYSTEMTIME::default();
    unsafe {
        let _ = windows::Win32::System::Time::FileTimeToSystemTime(&ft, &mut st);
    }
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}

fn format_timestamp_local(timestamp: i64) -> String {
    let timestamp = normalize_event_timestamp(timestamp);
    // Use i128 arithmetic (same as format_unix_date) to prevent overflow on extreme
    // timestamps such as i64::MAX or hostile git commit dates near year 2^63.
    let filetime_val = ((timestamp as i128) + 11_644_473_600) * 10_000_000;
    if filetime_val < 0 {
        return "1970-01-01 12:00 AM".to_string();
    }
    let ft = windows::Win32::Foundation::FILETIME {
        dwLowDateTime: (filetime_val & 0xFFFF_FFFF) as u32,
        dwHighDateTime: ((filetime_val >> 32) & 0xFFFF_FFFF) as u32,
    };
    let mut local_ft = windows::Win32::Foundation::FILETIME::default();
    let mut st = windows::Win32::Foundation::SYSTEMTIME::default();
    unsafe {
        let _ = windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime(&ft, &mut local_ft);
        let _ = windows::Win32::System::Time::FileTimeToSystemTime(&local_ft, &mut st);
    }

    let am_pm = if st.wHour >= 12 { "PM" } else { "AM" };
    let hour = if st.wHour == 0 {
        12
    } else if st.wHour > 12 {
        st.wHour - 12
    } else {
        st.wHour
    };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} {}",
        st.wYear, st.wMonth, st.wDay, hour, st.wMinute, am_pm
    )
}

fn extract_path_or_url(text: &str) -> Option<String> {
    // First check for explicit http:// or https:// links
    if let Some(idx) = text.find("https://") {
        let part = &text[idx..];
        let end = part
            .find(|c: char| c == ' ' || c == '\t')
            .unwrap_or(part.len());
        return Some(
            part[..end]
                .trim_end_matches(|c: char| c == '.' || c == ')' || c == ']')
                .to_string(),
        );
    }
    if let Some(idx) = text.find("http://") {
        let part = &text[idx..];
        let end = part
            .find(|c: char| c == ' ' || c == '\t')
            .unwrap_or(part.len());
        return Some(
            part[..end]
                .trim_end_matches(|c: char| c == '.' || c == ')' || c == ']')
                .to_string(),
        );
    }
    // ASCII case-insensitive byte search. Returns a byte index into `haystack` that is
    // always a char boundary (the needle is ASCII, and a byte matching ASCII can only be
    // the first byte of a one-byte char). Searching a `to_lowercase()` copy instead and
    // reusing its indices panics: lowercasing can change byte lengths (e.g. Turkish 'İ').
    fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        if n.is_empty() || h.len() < n.len() {
            return None;
        }
        (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
    }

    // Detect well-known meeting/service domains without protocol prefix in window titles
    let meeting_domains = [
        "meet.google.com",
        "zoom.us",
        "teams.microsoft.com",
        "teams.live.com",
        "webex.com",
        "gotomeeting.com",
        "bluejeans.com",
        "whereby.com",
    ];
    for domain in &meeting_domains {
        if let Some(idx) = find_ascii_ci(text, domain) {
            let b = text.as_bytes();
            // Walk backwards to find 'https://' or start of token (byte compare — never panics)
            let start = if idx >= 8 && b[idx - 8..idx].eq_ignore_ascii_case(b"https://") {
                idx - 8
            } else if idx >= 7 && b[idx - 7..idx].eq_ignore_ascii_case(b"http://") {
                idx - 7
            } else {
                // Construct a proper URL
                let end_of_domain = &text[idx..];
                let end = end_of_domain
                    .find(|c: char| c == ' ' || c == '\t' || c == '|' || c == '-')
                    .unwrap_or(end_of_domain.len());
                return Some(format!("https://{}", end_of_domain[..end].trim()));
            };
            let end_text = &text[start..];
            let end = end_text
                .find(|c: char| c == ' ' || c == '\t')
                .unwrap_or(end_text.len());
            return Some(
                end_text[..end]
                    .trim_end_matches(|c: char| c == '.' || c == ')' || c == ']')
                    .to_string(),
            );
        }
    }
    // Windows absolute path (e.g. C:\Users\...). The char before ':' must be an ASCII
    // drive letter — this both validates the path and guarantees idx-1 is a char boundary.
    if let Some(idx) = text.find(":\\") {
        if idx >= 1 && text.as_bytes()[idx - 1].is_ascii_alphabetic() {
            let start = idx - 1;
            let path_part = &text[start..];
            if let Some(sep_idx) = path_part.find(" - ") {
                return Some(path_part[..sep_idx].trim().to_string());
            }
            if let Some(sep_idx) = path_part.find(" | ") {
                return Some(path_part[..sep_idx].trim().to_string());
            }
            return Some(path_part.trim().to_string());
        }
    }
    // Unix path
    if let Some(idx) = text.find(":/") {
        if idx >= 1 && text.as_bytes()[idx - 1].is_ascii_alphanumeric() {
            let start = idx - 1;
            let path_part = &text[start..];
            if let Some(sep_idx) = path_part.find(" - ") {
                return Some(path_part[..sep_idx].trim().to_string());
            }
            if let Some(sep_idx) = path_part.find(" | ") {
                return Some(path_part[..sep_idx].trim().to_string());
            }
            return Some(path_part.trim().to_string());
        }
    }
    None
}

// ── Unit Converter ─────────────────────────────────────────────────────────
// "5 km to miles", "100 lbs to kg", "32 f to c", "1 gb to mb", etc.
pub fn try_unit_convert(input: &str) -> Option<(String, String)> {
    let s = input.trim().to_lowercase();
    let sep = if s.contains(" to ") {
        " to "
    } else if s.contains(" in ") {
        " in "
    } else {
        return None;
    };
    let parts: Vec<&str> = s.splitn(2, sep).collect();
    if parts.len() != 2 {
        return None;
    }
    let left = parts[0].trim();
    let to_unit = parts[1].trim();
    let num_end = left
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(left.len());
    if num_end == 0 {
        return None;
    }
    let num: f64 = left[..num_end].parse().ok()?;
    let from_unit = left[num_end..].trim();

    // Temperature
    let tc = |u: &str| -> Option<&str> {
        match u {
            "c" | "celsius" => Some("C"),
            "f" | "fahrenheit" => Some("F"),
            "k" | "kelvin" => Some("K"),
            _ => None,
        }
    };
    if let (Some(tf), Some(tt)) = (tc(from_unit), tc(to_unit)) {
        let c = match tf {
            "C" => num,
            "F" => (num - 32.0) * 5.0 / 9.0,
            "K" => num - 273.15,
            _ => return None,
        };
        let r = match tt {
            "C" => c,
            "F" => c * 9.0 / 5.0 + 32.0,
            "K" => c + 273.15,
            _ => return None,
        };
        let d = fmt_conv(r);
        return Some((format!("{} {} = {} {}", num, tf, d, tt), d));
    }

    // Linear unit table: (aliases, base_unit, to_base_multiplier, category)
    let table: &[(&[&str], &str, f64, &str)] = &[
        (&["mm", "millimeter", "millimeters"], "m", 0.001, "len"),
        (&["cm", "centimeter", "centimeters"], "m", 0.01, "len"),
        (&["m", "meter", "meters"], "m", 1.0, "len"),
        (&["km", "kilometer", "kilometers"], "m", 1000.0, "len"),
        (&["in", "inch", "inches"], "m", 0.0254, "len"),
        (&["ft", "foot", "feet"], "m", 0.3048, "len"),
        (&["yd", "yard", "yards"], "m", 0.9144, "len"),
        (&["mi", "mile", "miles"], "m", 1609.344, "len"),
        (&["mg", "milligram", "milligrams"], "kg", 0.000001, "mass"),
        (&["g", "gram", "grams"], "kg", 0.001, "mass"),
        (&["kg", "kilogram", "kilograms"], "kg", 1.0, "mass"),
        (&["lb", "lbs", "pound", "pounds"], "kg", 0.453592, "mass"),
        (&["oz", "ounce", "ounces"], "kg", 0.0283495, "mass"),
        (&["t", "tonne", "metric ton"], "kg", 1000.0, "mass"),
        (&["b", "byte", "bytes"], "b", 1.0, "data"),
        (&["kb", "kilobyte", "kilobytes"], "b", 1024.0, "data"),
        (&["mb", "megabyte", "megabytes"], "b", 1048576.0, "data"),
        (&["gb", "gigabyte", "gigabytes"], "b", 1073741824.0, "data"),
        (
            &["tb", "terabyte", "terabytes"],
            "b",
            1099511627776.0,
            "data",
        ),
        (&["kph", "kmh", "km/h"], "ms", 0.277778, "speed"),
        (&["mph"], "ms", 0.44704, "speed"),
        (&["m/s", "mps"], "ms", 1.0, "speed"),
        (&["s", "sec", "second", "seconds"], "s", 1.0, "time"),
        (&["min", "minute", "minutes"], "s", 60.0, "time"),
        (&["h", "hr", "hour", "hours"], "s", 3600.0, "time"),
        (&["d", "day", "days"], "s", 86400.0, "time"),
        (&["week", "weeks"], "s", 604800.0, "time"),
    ];

    let lookup = |u: &str| -> Option<(f64, &str, &str)> {
        table
            .iter()
            .find(|(aliases, _, _, _)| aliases.contains(&u))
            .map(|(_, base, f, cat)| (*f, *base, *cat))
    };

    let (ff, fb, fc) = lookup(from_unit)?;
    let (tf2, tb, tc2) = lookup(to_unit)?;
    if fb != tb || fc != tc2 {
        return None;
    }
    let result = (num * ff) / tf2;
    let d = fmt_conv(result);
    Some((format!("{} {} = {} {}", num, from_unit, d, to_unit), d))
}

fn fmt_conv(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v.fract() == 0.0 && v.abs() < 1e12 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.6}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ── Process Search & Kill ──────────────────────────────────────────────────
pub fn search_processes(query: &str) -> Vec<SearchResult> {
    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;
    let q = query.trim().to_lowercase();
    if !q.is_empty() && q.len() < 2 {
        return vec![];
    }

    let output = match std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(0x08000000)
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let protected = [
        "system",
        "smss.exe",
        "csrss.exe",
        "wininit.exe",
        "services.exe",
        "lsass.exe",
        "svchost.exe",
        "dwm.exe",
        "winlogon.exe",
        "registry",
    ];
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < 2 {
            continue;
        }
        let name = fields[0].trim_matches('"');
        let pid_str = fields[1].trim_matches('"');
        let name_lower = name.to_lowercase();
        if !q.is_empty() && !name_lower.contains(&q) {
            continue;
        }
        if protected.iter().any(|p| name_lower == *p) {
            continue;
        }
        let pid: u32 = pid_str.parse().unwrap_or(0);
        let mem_kb = fields
            .get(4)
            .map(|m| m.trim_matches('"').replace(",", "").replace(" K", ""))
            .and_then(|kb| kb.parse::<u64>().ok())
            .unwrap_or(0);

        let score = if q.is_empty() {
            mem_kb as f32
        } else {
            let base = if name_lower == q {
                3.0
            } else if name_lower.starts_with(&q) {
                2.0
            } else {
                1.0
            };
            base + (mem_kb as f32 / 10_000_000.0) // Small boost for memory usage to break ties
        };

        let mem_mb = format!("{:.0} MB", mem_kb as f64 / 1024.0);
        let display = name.trim_end_matches(".exe");
        results.push(SearchResult {
            entry: CatalogEntry {
                id: format!("proc.{}.{}", name_lower.replace(' ', "_"), pid),
                control_name: format!("Kill {} (PID {})", display, pid),
                breadcrumb_path: format!("Process > {} > {}", name, mem_mb),
                launch_command: format!("kill:{}", pid),
                source: "ACTION".to_string(),
                description: format!("Force-terminate {} (PID {})", name, pid),
                synonyms: format!("kill terminate stop end process {}", name_lower),
            },
            score,
        });
    }
    results.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(8);
    results
}

struct WindowInfo {
    hwnd: windows::Win32::Foundation::HWND,
    title: String,
}

unsafe extern "system" fn enum_windows_callback(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowTextW, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, WS_CHILD,
        WS_EX_TOOLWINDOW,
    };

    if IsWindowVisible(hwnd).as_bool() {
        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title);
        if len > 0 {
            let title_str = String::from_utf16_lossy(&title[..len as usize]);
            let title_trimmed = title_str.trim_matches('\0').trim().to_string();

            if !title_trimmed.is_empty() {
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;

                let is_tool = (ex_style & WS_EX_TOOLWINDOW.0) != 0;
                let is_child = (style & WS_CHILD.0) != 0;

                // Skip common Windows/Shell system background windows
                let is_ignored = title_trimmed == "Program Manager"
                    || title_trimmed == "Settings"
                    || title_trimmed == "Start"
                    || title_trimmed == "Windows Input Experience";

                if !is_tool && !is_child && !is_ignored {
                    let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
                    list.push(WindowInfo {
                        hwnd,
                        title: title_trimmed,
                    });
                }
            }
        }
    }
    true.into()
}

impl SearchEngine {
    pub fn search_windows(&self, query: &str) -> Vec<SearchResult> {
        let mut list: Vec<WindowInfo> = Vec::new();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
                Some(enum_windows_callback),
                windows::Win32::Foundation::LPARAM(&mut list as *mut _ as isize),
            );
        }

        let q_lower = query.trim().to_lowercase();
        let mut results = Vec::new();

        for win in list {
            let win_lower = win.title.to_lowercase();
            let mut score = 0.0f32;

            if q_lower.is_empty() {
                score = 1.0;
            } else if win_lower == q_lower {
                score = 3.0;
            } else if win_lower.starts_with(&q_lower) {
                score = 2.5;
            } else if win_lower.contains(&q_lower) {
                score = 1.8;
            } else {
                let win_len = win_lower.chars().count();
                let q_len = q_lower.chars().count();
                if win_len > 0 && q_len > 0 {
                    let dist = levenshtein_distance(&q_lower, &win_lower);
                    let max_len = win_len.max(q_len);
                    let similarity = 1.0 - (dist as f32 / max_len as f32);
                    if similarity >= 0.5 {
                        score = 0.5 + 1.0 * similarity;
                    }
                }
            }

            if score > 0.0 {
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("window.{}", win.hwnd.0 as isize),
                        control_name: win.title.clone(),
                        breadcrumb_path: format!("Window Switcher > {}", win.title),
                        launch_command: format!("window:{}", win.hwnd.0 as isize),
                        source: "WINDOW".to_string(),
                        description: format!("Switch to Window: {}", win.title),
                        synonyms: win.title.to_lowercase(),
                    },
                    score,
                });
            }
        }

        results.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    pub fn check_quicklink_keyword(&self, first_word: &str) -> Option<(String, String)> {
        let conn = &self.conn;
        let mut stmt = conn
            .prepare("SELECT name, url FROM quicklinks WHERE keyword = ?1 LIMIT 1")
            .ok()?;
        let mut rows = stmt
            .query(rusqlite::params![first_word.to_lowercase()])
            .ok()?;
        if let Some(row) = rows.next().ok()? {
            let name: String = row.get(0).ok()?;
            let url: String = row.get(1).ok()?;
            Some((name, url))
        } else {
            None
        }
    }

    pub fn check_snippet_keyword(&self, first_word: &str) -> Option<(String, String, String)> {
        let keyword = first_word.trim().to_lowercase();
        if keyword.is_empty() {
            return None;
        }
        let conn = &self.conn;
        let mut stmt = conn
            .prepare(
                "SELECT name, content, COALESCE(NULLIF(keyword, ''), name) FROM snippets
                 WHERE lower(COALESCE(NULLIF(keyword, ''), name)) = ?1 LIMIT 1",
            )
            .ok()?;
        let mut rows = stmt.query(rusqlite::params![keyword]).ok()?;
        if let Some(row) = rows.next().ok()? {
            let name: String = row.get(0).ok()?;
            let content: String = row.get(1).ok()?;
            let keyword: Option<String> = row.get(2).ok()?;
            Some((name, content, keyword.unwrap_or_default()))
        } else {
            None
        }
    }

    pub fn search_quicklinks_name_matches(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;
        let q = query.trim();
        if q.is_empty() {
            return results;
        }

        let q_lower = q.to_lowercase();
        let mut stmt = match conn.prepare(
            "SELECT name, url, keyword FROM quicklinks WHERE name LIKE ?1 OR keyword = ?2 LIMIT 5",
        ) {
            Ok(s) => s,
            Err(_) => return results,
        };
        let iter = match stmt.query_map(
            rusqlite::params![format!("%{}%", q), q_lower.clone()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        ) {
            Ok(it) => it,
            Err(_) => return results,
        };
        for item in iter {
            if let Ok((name, url, keyword)) = item {
                let display_keyword = if keyword.is_empty() {
                    "".to_string()
                } else {
                    format!(" [{}]", keyword)
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("quicklink.{}", name.to_lowercase().replace(' ', "_")),
                        control_name: name.clone(),
                        breadcrumb_path: format!("Quicklink{} > {}", display_keyword, url),
                        launch_command: format!("open_quicklink:{}", url),
                        source: "QUICKLINK".to_string(),
                        description: format!("Open quicklink '{}' ({})", name, url),
                        synonyms: format!("{} {}", name.to_lowercase(), keyword),
                    },
                    score: 4.0,
                });
            }
        }
        results
    }

    pub fn search_snippets_name_matches(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;
        let q = query.trim();
        if q.is_empty() {
            return results;
        }

        let q_lower = q.to_lowercase();
        let mut stmt = match conn.prepare("SELECT name, content, keyword FROM snippets WHERE name LIKE ?1 OR keyword = ?2 COLLATE NOCASE LIMIT 5") {
            Ok(s) => s,
            Err(_) => return results,
        };
        let iter = match stmt.query_map(
            rusqlite::params![format!("%{}%", q), q_lower.clone()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        ) {
            Ok(it) => it,
            Err(_) => return results,
        };
        for item in iter {
            if let Ok((name, content, keyword)) = item {
                let kw_str = keyword.clone().unwrap_or_default();
                let display_keyword = if kw_str.is_empty() {
                    "".to_string()
                } else {
                    format!(" [{}]", kw_str)
                };
                let is_exact_keyword = !kw_str.is_empty() && kw_str.to_lowercase() == q_lower;
                let is_exact_name = name.to_lowercase() == q_lower;
                let score = if is_exact_keyword {
                    110.0
                } else if is_exact_name {
                    105.0
                } else {
                    3.9
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("snippet.{}", name.to_lowercase().replace(' ', "_")),
                        control_name: name.clone(),
                        breadcrumb_path: format!("Snippet{} > Copy to Clipboard", display_keyword),
                        launch_command: format!("copy_snippet:{}", content),
                        source: "SNIPPET".to_string(),
                        description: ellipsize_chars(&content, 63),
                        synonyms: format!("{} {}", name.to_lowercase(), kw_str),
                    },
                    score,
                });
            }
        }
        results
    }

    pub fn search_quicklinks_only(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;
        let q = query.trim();
        let q_lower = q.to_lowercase();

        let mut stmt = if q.is_empty() {
            match conn.prepare("SELECT name, url, keyword FROM quicklinks ORDER BY name ASC") {
                Ok(s) => s,
                Err(_) => return results,
            }
        } else {
            match conn.prepare("SELECT name, url, keyword FROM quicklinks WHERE name LIKE ?1 OR keyword = ?2 ORDER BY name ASC") {
                Ok(s) => s,
                Err(_) => return results,
            }
        };

        let params: Vec<String> = if q.is_empty() {
            vec![]
        } else {
            vec![format!("%{}%", q), q_lower.clone()]
        };

        let iter = match stmt.query_map(rusqlite::params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            Ok(it) => it,
            Err(_) => return results,
        };

        for item in iter {
            if let Ok((name, url, keyword)) = item {
                let display_keyword = if keyword.is_empty() {
                    "".to_string()
                } else {
                    format!(" [{}]", keyword)
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("quicklink.{}", name.to_lowercase().replace(' ', "_")),
                        control_name: name.clone(),
                        breadcrumb_path: format!("Quicklink{} > {}", display_keyword, url),
                        launch_command: format!("open_quicklink:{}", url),
                        source: "QUICKLINK".to_string(),
                        description: format!("Open quicklink '{}' ({})", name, url),
                        synonyms: format!("{} {}", name.to_lowercase(), keyword),
                    },
                    score: 8.0,
                });
            }
        }
        results
    }
    pub fn search_games(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let q = query.trim().to_lowercase();
        let paths = [
            "C:\\Program Files (x86)\\Steam\\steamapps",
            "D:\\SteamLibrary\\steamapps",
            "E:\\SteamLibrary\\steamapps",
        ];
        for p in paths.iter() {
            if let Ok(entries) = std::fs::read_dir(p) {
                for entry in entries.flatten() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    if filename.starts_with("appmanifest_") && filename.ends_with(".acf") {
                        if let Ok(content) = std::fs::read_to_string(entry.path()) {
                            let mut appid = String::new();
                            let mut name = String::new();
                            for line in content.lines() {
                                let line = line.trim();
                                if line.starts_with("\"appid\"") {
                                    appid = line
                                        .replace("\"appid\"", "")
                                        .replace("\"", "")
                                        .trim()
                                        .to_string();
                                } else if line.starts_with("\"name\"") {
                                    name = line
                                        .replace("\"name\"", "")
                                        .replace("\"", "")
                                        .trim()
                                        .to_string();
                                }
                            }
                            if !appid.is_empty() && !name.is_empty() {
                                let name_lower = name.to_lowercase();
                                let mut score = 0.0;
                                if q.is_empty() {
                                    score = 1.0;
                                } else if name_lower == q {
                                    score = 4.0;
                                } else if name_lower.starts_with(&q) {
                                    score = 3.5;
                                } else if name_lower.contains(&q) {
                                    score = 3.0;
                                }
                                if score > 0.0 {
                                    results.push(SearchResult {
                                        entry: CatalogEntry {
                                            id: format!("steam.{}", appid),
                                            control_name: format!("🎮 Steam: {}", name),
                                            breadcrumb_path: "Games > Steam".to_string(),
                                            launch_command: format!("steam://rungameid/{}", appid),
                                            source: "ACTION".to_string(),
                                            description: "Launch Steam game".to_string(),
                                            synonyms: format!("steam games play {}", name_lower),
                                        },
                                        score,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        results
    }
    pub fn search_focus_categories(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;
        let q = query.trim().to_lowercase();
        if let Ok(mut stmt) = conn.prepare("SELECT name, blocked_apps FROM focus_categories") {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    let (name, blocked) = row;
                    let name_lower = name.to_lowercase();
                    let mut score = 0.0;
                    if q.is_empty()
                        || name_lower == q
                        || q == "start focus session"
                        || q == "focus session"
                        || q == "focus"
                    {
                        score = 4.0;
                    } else if name_lower.starts_with(&q) {
                        score = 3.5;
                    } else if name_lower.contains(&q) {
                        score = 3.0;
                    }
                    if score > 0.0 {
                        results.push(SearchResult {
                            entry: CatalogEntry {
                                id: format!("focus_category.{}", name),
                                control_name: format!("Start Focus Session: {}", name),
                                breadcrumb_path: "Focus > Start Session".to_string(),
                                launch_command: format!("start_focus_session:{}", name),
                                source: "ACTION".to_string(),
                                description: format!("Blocks: {}", blocked),
                                synonyms: format!("start focus session category {}", name),
                            },
                            score,
                        });
                    }
                }
            }
        }
        results
    }

    pub fn search_snippets_only(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let conn = &self.conn;
        let q = query.trim();
        let q_lower = q.to_lowercase();

        let mut stmt = if q.is_empty() {
            match conn.prepare("SELECT name, content, keyword FROM snippets ORDER BY name ASC") {
                Ok(s) => s,
                Err(_) => return results,
            }
        } else {
            match conn.prepare("SELECT name, content, keyword FROM snippets WHERE name LIKE ?1 OR content LIKE ?2 OR keyword = ?3 COLLATE NOCASE ORDER BY name ASC") {
                Ok(s) => s,
                Err(_) => return results,
            }
        };

        let params: Vec<String> = if q.is_empty() {
            vec![]
        } else {
            vec![format!("%{}%", q), format!("%{}%", q), q_lower.clone()]
        };

        let iter = match stmt.query_map(rusqlite::params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        }) {
            Ok(it) => it,
            Err(_) => return results,
        };

        for item in iter {
            if let Ok((name, content, keyword)) = item {
                let kw_str = keyword.clone().unwrap_or_default();
                let display_keyword = if kw_str.is_empty() {
                    "".to_string()
                } else {
                    format!(" [{}]", kw_str)
                };
                results.push(SearchResult {
                    entry: CatalogEntry {
                        id: format!("snippet.{}", name.to_lowercase().replace(' ', "_")),
                        control_name: name.clone(),
                        breadcrumb_path: format!("Snippet{} > Copy to Clipboard", display_keyword),
                        launch_command: format!("copy_snippet:{}", content),
                        source: "SNIPPET".to_string(),
                        description: ellipsize_chars(&content, 63),
                        synonyms: format!("{} {}", name.to_lowercase(), kw_str),
                    },
                    score: 8.0,
                });
            }
        }
        results
    }
    pub fn search_notes(&self, query: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let q = query.trim().to_lowercase();
        if let Ok(appdata) = std::env::var("APPDATA") {
            let notes_dir = std::path::PathBuf::from(appdata)
                .join("omnisearch-lite")
                .join("notes");
            let _ = std::fs::create_dir_all(&notes_dir); // fix #1: ensure dir exists before reading
            if let Ok(entries) = std::fs::read_dir(&notes_dir) {
                for entry in entries.flatten() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    let name = filename
                        .strip_suffix(".txt")
                        .unwrap_or(&filename)
                        .to_string();
                    let name_lower = name.to_lowercase();
                    let full_path = entry.path().to_string_lossy().to_string();
                    let mut score = 0.0;
                    if q.is_empty() {
                        score = 1.0;
                    } else if name_lower == q {
                        score = 4.0;
                    } else if name_lower.starts_with(&q) {
                        score = 3.5;
                    } else if name_lower.contains(&q) {
                        score = 3.0;
                    }
                    if score > 0.0 {
                        results.push(SearchResult {
                            entry: CatalogEntry {
                                id: format!("note.{}", name),
                                control_name: format!("📝 {}", name),
                                breadcrumb_path: format!("Notes > {}", filename),
                                launch_command: format!("open_note:{}", full_path),
                                source: "ACTION".to_string(),
                                description: "Open note in app (Ctrl+S to save, Esc to close)"
                                    .to_string(),
                                synonyms: format!(
                                    "note {} {}",
                                    name_lower,
                                    filename.to_lowercase()
                                ),
                            },
                            score,
                        });
                    }
                }
            }
        }
        results
    }
}
