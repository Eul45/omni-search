pub fn log(msg: &str) {
    // Basic diagnostic logging to app_log.txt beside the exe; falls back to
    // %APPDATA%\omnisearch-lite when the exe dir isn't writable (Program Files),
    // so diagnostics aren't silently lost on installed copies.
    let exe_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("app_log.txt")));
    let appdata_path = std::env::var("APPDATA").ok().map(|a| {
        std::path::PathBuf::from(a)
            .join("omnisearch-lite")
            .join("app_log.txt")
    });

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    for path in [exe_path, appdata_path].into_iter().flatten() {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            // Truncate in-place if file exceeds 1MB (atomic: no TOCTOU race).
            if file
                .metadata()
                .map(|m| m.len() > 1024 * 1024)
                .unwrap_or(false)
            {
                let _ = file.set_len(0);
            }
            if writeln!(file, "[{}] {}", now_ms, msg).is_ok() {
                return;
            }
        }
        // Open/write failed (e.g. read-only dir) — try the fallback location.
    }
}
