use crate::search::{ensure_memory_events_schema, insert_memory_event};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::thread;

struct TempFile {
    path: PathBuf,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn start_browser_indexer(db_path: PathBuf) {
    thread::spawn(move || {
        // Initial delay to let the app start up completely lag-free
        thread::sleep(std::time::Duration::from_secs(10));
        loop {
            if let Err(e) = run_browser_indexer(&db_path) {
                eprintln!("Browser Indexer error: {:?}", e);
            }
            // Re-scan every 10 minutes
            thread::sleep(std::time::Duration::from_secs(600));
        }
    });
}

fn run_browser_indexer(db_path: &Path) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    let _ = ensure_memory_events_schema(&conn);

    conn.execute(
        "CREATE TABLE IF NOT EXISTS browser_items (
            url TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            source TEXT NOT NULL,
            visit_count INTEGER NOT NULL,
            last_visit_time INTEGER NOT NULL
        );",
        [],
    )?;

    let profiles = get_browser_profiles();
    for (browser, profile_path) in profiles {
        if browser == "firefox" {
            let places_path = profile_path.join("places.sqlite");
            if places_path.exists() {
                if let Err(e) = parse_firefox(&places_path, &conn) {
                    eprintln!(
                        "Error parsing Firefox places for {:?}: {:?}",
                        places_path, e
                    );
                }
            }
        } else {
            // Chromium (Chrome, Edge, Brave)
            // 1. Bookmarks
            let bookmarks_path = profile_path.join("Bookmarks");
            if bookmarks_path.exists() {
                let source_type = format!("{}_bookmark", browser);
                if let Err(e) = parse_bookmarks(&bookmarks_path, &source_type, &conn) {
                    eprintln!(
                        "Error parsing bookmarks for {}/{:?}: {:?}",
                        browser, bookmarks_path, e
                    );
                }
            }

            // 2. History
            let history_path = profile_path.join("History");
            if history_path.exists() {
                let source_type = format!("{}_history", browser);
                if let Err(e) = parse_history(&history_path, &source_type, &conn) {
                    eprintln!(
                        "Error parsing history for {}/{:?}: {:?}",
                        browser, history_path, e
                    );
                }
            }
        }
    }

    // Prune old history to keep database size small and search fast (keep top 5000)
    let _ = conn.execute(
        "DELETE FROM browser_items WHERE url NOT IN (
            SELECT url FROM browser_items ORDER BY last_visit_time DESC LIMIT 5000
        ) AND source LIKE '%history%'",
        [],
    );

    Ok(())
}

fn get_browser_profiles() -> Vec<(String, PathBuf)> {
    let mut profiles = Vec::new();
    let local_app_data = match std::env::var("LOCALAPPDATA") {
        Ok(d) => d,
        Err(_) => return profiles,
    };

    // Chrome
    let chrome_dir = PathBuf::from(&local_app_data)
        .join("Google")
        .join("Chrome")
        .join("User Data");
    if chrome_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&chrome_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == "Default" || name.starts_with("Profile ") {
                        profiles.push(("chrome".to_string(), path));
                    }
                }
            }
        }
    }

    // Edge
    let edge_dir = PathBuf::from(&local_app_data)
        .join("Microsoft")
        .join("Edge")
        .join("User Data");
    if edge_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&edge_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == "Default" || name.starts_with("Profile ") {
                        profiles.push(("edge".to_string(), path));
                    }
                }
            }
        }
    }

    // Brave
    let brave_dir = PathBuf::from(&local_app_data)
        .join("BraveSoftware")
        .join("Brave-Browser")
        .join("User Data");
    if brave_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&brave_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == "Default" || name.starts_with("Profile ") {
                        profiles.push(("brave".to_string(), path));
                    }
                }
            }
        }
    }

    // Firefox
    if let Ok(app_data) = std::env::var("APPDATA") {
        let firefox_dir = PathBuf::from(&app_data)
            .join("Mozilla")
            .join("Firefox")
            .join("Profiles");
        if firefox_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&firefox_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        profiles.push(("firefox".to_string(), path));
                    }
                }
            }
        }
    }

    profiles
}

fn parse_bookmarks(path: &Path, source_type: &str, conn: &Connection) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    let mut bookmarks = Vec::new();
    if let Some(roots) = json.get("roots").and_then(|v| v.as_object()) {
        for (_, root_val) in roots {
            traverse_bookmarks(root_val, &mut bookmarks, 0);
        }
    }

    let mut stmt = conn.prepare(
        "INSERT INTO browser_items (url, title, source, visit_count, last_visit_time)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(url) DO UPDATE SET
             visit_count = MAX(visit_count, excluded.visit_count),
             last_visit_time = MAX(last_visit_time, excluded.last_visit_time),
             source = CASE 
                 WHEN source LIKE '%bookmark%' OR excluded.source LIKE '%bookmark%' THEN 
                     CASE 
                         WHEN source LIKE 'chrome%' THEN 'chrome_bookmark' 
                         WHEN source LIKE 'edge%' THEN 'edge_bookmark'
                         WHEN source LIKE 'brave%' THEN 'brave_bookmark'
                         WHEN source LIKE 'firefox%' THEN 'firefox_bookmark'
                         ELSE excluded.source 
                     END
                 ELSE excluded.source
             END",
    )?;

    for (name, url) in bookmarks {
        if url.starts_with("http://") || url.starts_with("https://") {
            let _ = stmt.execute(params![url, name, source_type, 100, 0]);
        }
    }

    Ok(())
}

fn traverse_bookmarks(val: &serde_json::Value, list: &mut Vec<(String, String)>, depth: usize) {
    // Guard the recursion against a stack overflow on pathological/deeply nested bookmark
    // JSON (this runs on the browser-indexer worker thread).
    if depth > 200 {
        return;
    }
    if let Some(obj) = val.as_object() {
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if item_type == "url" {
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                list.push((name, url.to_string()));
            }
        } else if let Some(children) = obj.get("children").and_then(|v| v.as_array()) {
            for child in children {
                traverse_bookmarks(child, list, depth + 1);
            }
        }
    }
}

fn parse_history(path: &Path, source_type: &str, conn: &Connection) -> anyhow::Result<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("browser_history_temp_{}.db", timestamp));

    std::fs::copy(path, &temp_path)?;
    let _temp_file = TempFile {
        path: temp_path.clone(),
    };

    // Scope for temp connection so it is closed before we remove the file
    {
        let temp_conn = Connection::open(&temp_path)?;
        let mut stmt = temp_conn.prepare(
            "SELECT url, title, visit_count, last_visit_time 
             FROM urls 
             WHERE hidden = 0 AND title IS NOT NULL AND title != '' 
             ORDER BY last_visit_time DESC LIMIT 2000",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        let mut insert_stmt = conn.prepare(
            "INSERT INTO browser_items (url, title, source, visit_count, last_visit_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(url) DO UPDATE SET
                 visit_count = MAX(visit_count, excluded.visit_count),
                 last_visit_time = MAX(last_visit_time, excluded.last_visit_time),
                 source = CASE 
                     WHEN source LIKE '%bookmark%' OR excluded.source LIKE '%bookmark%' THEN 
                         CASE 
                             WHEN source LIKE 'chrome%' THEN 'chrome_bookmark' 
                             WHEN source LIKE 'edge%' THEN 'edge_bookmark'
                             WHEN source LIKE 'brave%' THEN 'brave_bookmark'
                             WHEN source LIKE 'firefox%' THEN 'firefox_bookmark'
                             ELSE excluded.source 
                         END
                     ELSE excluded.source
                 END",
        )?;

        for row in rows.flatten() {
            let (url, title, visit_count, last_visit_time) = row;
            if url.starts_with("http://") || url.starts_with("https://") {
                let unix_micros = chromium_time_to_unix_micros(last_visit_time);
                let _ =
                    insert_stmt.execute(params![url, title, source_type, visit_count, unix_micros]);
                if unix_micros > 0 {
                    insert_memory_event(
                        conn,
                        unix_micros_to_secs(unix_micros),
                        "Browser",
                        "Visited Page",
                        &title,
                        &url,
                        source_type,
                        None,
                        Some(&url),
                    );
                }
            }
        }
    }

    Ok(())
}

fn parse_firefox(path: &Path, conn: &Connection) -> anyhow::Result<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("firefox_places_temp_{}.db", timestamp));

    std::fs::copy(path, &temp_path)?;
    let _temp_file = TempFile {
        path: temp_path.clone(),
    };

    // Scope for connection closure
    {
        let temp_conn = Connection::open(&temp_path)?;

        // 1. Parse Bookmarks
        let mut stmt_bookmarks = temp_conn.prepare(
            "SELECT p.url, b.title, p.visit_count, p.last_visit_date 
             FROM moz_bookmarks b 
             JOIN moz_places p ON b.fk = p.id 
             WHERE b.type = 1 AND b.title IS NOT NULL AND b.title != ''",
        )?;

        let bookmark_rows = stmt_bookmarks.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        let mut insert_stmt = conn.prepare(
            "INSERT INTO browser_items (url, title, source, visit_count, last_visit_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(url) DO UPDATE SET
                 visit_count = MAX(visit_count, excluded.visit_count),
                 last_visit_time = MAX(last_visit_time, excluded.last_visit_time),
                 source = CASE 
                     WHEN source LIKE '%bookmark%' OR excluded.source LIKE '%bookmark%' THEN 
                         CASE 
                             WHEN source LIKE 'chrome%' THEN 'chrome_bookmark' 
                             WHEN source LIKE 'edge%' THEN 'edge_bookmark'
                             WHEN source LIKE 'brave%' THEN 'brave_bookmark'
                             WHEN source LIKE 'firefox%' THEN 'firefox_bookmark'
                             ELSE excluded.source 
                         END
                     ELSE excluded.source
                 END",
        )?;

        for row in bookmark_rows.flatten() {
            let (url, title, visit_count, last_visit_date) = row;
            if url.starts_with("http://") || url.starts_with("https://") {
                let _ = insert_stmt.execute(params![
                    url,
                    title,
                    "firefox_bookmark",
                    visit_count.max(100),
                    last_visit_date
                ]);
            }
        }

        // 2. Parse History
        let mut stmt_history = temp_conn.prepare(
            "SELECT url, title, visit_count, last_visit_date 
             FROM moz_places 
             WHERE visit_count > 0 AND title IS NOT NULL AND title != '' 
             ORDER BY last_visit_date DESC LIMIT 2000",
        )?;

        let history_rows = stmt_history.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        for row in history_rows.flatten() {
            let (url, title, visit_count, last_visit_date) = row;
            if url.starts_with("http://") || url.starts_with("https://") {
                let _ = insert_stmt.execute(params![
                    url,
                    title,
                    "firefox_history",
                    visit_count,
                    last_visit_date
                ]);
                if last_visit_date > 0 {
                    insert_memory_event(
                        conn,
                        unix_micros_to_secs(last_visit_date),
                        "Browser",
                        "Visited Page",
                        &title,
                        &url,
                        "firefox_history",
                        None,
                        Some(&url),
                    );
                }
            }
        }
    }

    Ok(())
}

fn chromium_time_to_unix_micros(chromium_time: i64) -> i64 {
    chromium_time.saturating_sub(11_644_473_600_000_000)
}

fn unix_micros_to_secs(unix_micros: i64) -> i64 {
    unix_micros / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_history_time_is_normalized_to_unix_micros() {
        assert_eq!(chromium_time_to_unix_micros(11_644_473_600_000_000), 0);
        assert_eq!(
            chromium_time_to_unix_micros(11_644_473_601_000_000),
            1_000_000
        );
    }

    #[test]
    fn browser_memory_events_use_unix_seconds() {
        assert_eq!(unix_micros_to_secs(1_700_000_000_123_456), 1_700_000_000);
    }
}
