use notify::Watcher;
use once_cell::sync::Lazy;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

pub static IS_INDEXING: AtomicBool = AtomicBool::new(false);
pub static INDEXING_PROGRESS: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("Idle".to_string()));
pub static LAST_INDEX_TIME: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("Never".to_string()));

const DEFAULT_CONFIGURED_IGNORED_DIRS: &[&str] = &["node_modules", ".angular"];

fn is_in_low_priority_dir(path: &Path) -> bool {
    path.components().any(|c| {
        if let Some(s) = c.as_os_str().to_str() {
            let s_lower = s.to_lowercase();
            s_lower == "target" || s_lower == "build"
        } else {
            false
        }
    })
}

fn is_indexable_content(_ext: &str) -> bool {
    false
}

/// Extract searchable text for a file (document text or image OCR), or None.
fn extract_content(_path: &Path, _ext: &str) -> Option<String> {
    None
}

fn wait_for_file_lock(path: &Path) -> bool {
    if !path.exists() || !path.is_file() {
        return false;
    }
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(3);
    while start.elapsed() < timeout {
        if std::fs::OpenOptions::new().read(true).open(path).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

fn normalize_ignored_dir_name(name: &str) -> Option<String> {
    let trimmed = name.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return None;
    }
    let trimmed = trimmed.trim_end_matches(['/', '\\']);
    let leaf = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if leaf.is_empty() {
        None
    } else {
        Some(leaf.to_ascii_lowercase())
    }
}

fn app_db_path() -> PathBuf {
    std::env::var("APPDATA")
        .map(|appdata| {
            PathBuf::from(appdata)
                .join("omnisearch-lite")
                .join("file_index.db")
        })
        .unwrap_or_else(|_| PathBuf::from("file_index.db"))
}

fn ensure_ignored_folders_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ignored_folders (
            name TEXT PRIMARY KEY COLLATE NOCASE
        );",
        [],
    )?;
    let count: i64 =
        conn.query_row("SELECT COUNT(*) FROM ignored_folders", [], |row| row.get(0))?;
    if count == 0 {
        for name in DEFAULT_CONFIGURED_IGNORED_DIRS {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO ignored_folders (name) VALUES (?1)",
                [name],
            );
        }
    }
    Ok(())
}

pub fn get_ignored_folder_names() -> Vec<String> {
    let db_path = app_db_path();
    let Some(parent) = db_path.parent() else {
        return DEFAULT_CONFIGURED_IGNORED_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect();
    };
    let _ = std::fs::create_dir_all(parent);
    let Ok(conn) = Connection::open(db_path) else {
        return DEFAULT_CONFIGURED_IGNORED_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect();
    };
    if ensure_ignored_folders_table(&conn).is_err() {
        return DEFAULT_CONFIGURED_IGNORED_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    let Ok(mut stmt) = conn.prepare("SELECT name FROM ignored_folders ORDER BY lower(name)") else {
        return Vec::new();
    };
    stmt.query_map([], |row| row.get::<_, String>(0))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

pub fn add_ignored_folder_name(name: &str) -> bool {
    let Some(name) = normalize_ignored_dir_name(name) else {
        return false;
    };
    let db_path = app_db_path();
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(conn) = Connection::open(db_path) else {
        return false;
    };
    if ensure_ignored_folders_table(&conn).is_err() {
        return false;
    }
    conn.execute(
        "INSERT OR IGNORE INTO ignored_folders (name) VALUES (?1)",
        [name],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

pub fn remove_ignored_folder_name(name: &str) -> bool {
    let Some(name) = normalize_ignored_dir_name(name) else {
        return false;
    };
    let Ok(conn) = Connection::open(app_db_path()) else {
        return false;
    };
    if ensure_ignored_folders_table(&conn).is_err() {
        return false;
    }
    conn.execute("DELETE FROM ignored_folders WHERE name = ?1", [name])
        .map(|rows| rows > 0)
        .unwrap_or(false)
}

fn configured_ignored_dirs() -> std::collections::HashSet<String> {
    get_ignored_folder_names()
        .iter()
        .filter_map(|name| normalize_ignored_dir_name(name))
        .collect()
}

fn is_ignored_dir_with_config(name: &str, configured: &std::collections::HashSet<String>) -> bool {
    is_builtin_ignored_dir(name)
        || normalize_ignored_dir_name(name)
            .map(|name| configured.contains(&name))
            .unwrap_or(false)
}

fn path_in_ignored_dir_with_config(
    path: &Path,
    configured: &std::collections::HashSet<String>,
) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|name| is_ignored_dir_with_config(name, &configured))
            .unwrap_or(false)
    })
}

/// Index a single file immediately: upsert name/meta, plus content/OCR for indexable types.
/// Used by the filesystem watcher so new files are searchable in milliseconds.
fn index_one_file(conn: &Connection, path: &Path) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if is_indexable_content(&ext) {
        if !wait_for_file_lock(path) {
            log_indexer(&format!("Watcher: File lock timeout for {:?}", path));
            return;
        }
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        _ => return,
    };
    let path_str = match path.to_str() {
        Some(s) => s.to_string(),
        None => return,
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() || is_ignored_file(&name, &ext) {
        return;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = meta.len() as i64;

    let _ = conn.execute(
        "INSERT OR REPLACE INTO files (path, name, extension, modified, size, is_dir) VALUES (?,?,?,?,?,0)",
        params![path_str, name, ext, modified, size],
    );
    if is_indexable_content(&ext) {
        let content = extract_content(path, &ext).unwrap_or_default();
        let _ = conn.execute("DELETE FROM files_fts WHERE path = ?", [&path_str]);
        let _ = conn.execute(
            "INSERT INTO files_fts (path, content) VALUES (?, ?)",
            params![path_str, content],
        );
    }
}

static WATCHER: Lazy<Mutex<Option<notify::RecommendedWatcher>>> = Lazy::new(|| Mutex::new(None));

/// Watch the user profile + fixed drives and index created/modified files instantly.
/// Falls back to the periodic crawl (safety net) for any events the OS drops.
pub fn start_watcher(db_path: PathBuf) {
    let is_initial = WATCHER.lock().unwrap().is_none();

    let mut g = WATCHER.lock().unwrap();
    // Drop the old watcher first to release all OS watches and close mpsc channels
    *g = None;

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            log_indexer(&format!("Watcher init failed: {e}"));
            return;
        }
    };

    let folders = get_scan_folders();
    for folder in &folders {
        log_indexer(&format!("Watcher watching folder: {:?}", folder));
        let _ = watcher.watch(folder, notify::RecursiveMode::Recursive);
    }

    let db_path_clone = db_path.clone();
    thread::spawn(move || {
        if is_initial {
            // Wait 15s for the initial indexer I/O burst to settle before registering
            // recursive filesystem watches.
            thread::sleep(std::time::Duration::from_secs(15));
        }

        // OCR (WinRT) needs COM initialized on this thread.
        let com_initialized = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        }
        .is_ok();

        let conn = match Connection::open(&db_path_clone) {
            Ok(c) => c,
            Err(_) => return,
        };
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
        log_indexer("Watcher started");
        let ignored_dirs = configured_ignored_dirs();

        let collect = |set: &mut std::collections::HashSet<PathBuf>,
                       res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if matches!(
                    ev.kind,
                    notify::EventKind::Create(_) | notify::EventKind::Modify(_)
                ) {
                    for p in ev.paths {
                        set.insert(p);
                    }
                }
            }
        };

        loop {
            let first = match rx.recv() {
                Ok(r) => r,
                Err(_) => break,
            };
            let mut paths = std::collections::HashSet::new();
            collect(&mut paths, first);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
            while let Ok(r) =
                rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                collect(&mut paths, r);
                if paths.len() > 2000 {
                    break;
                }
            }
            for p in paths {
                if !path_in_ignored_dir_with_config(&p, &ignored_dirs) {
                    index_one_file(&conn, &p);
                }
            }
        }
        log_indexer("Watcher thread exited");
        if com_initialized {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    });

    *g = Some(watcher);
}

fn log_indexer(msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;
    let log_dir = match std::env::var("APPDATA") {
        Ok(d) => PathBuf::from(d).join("omnisearch-lite"),
        Err(_) => PathBuf::from("."),
    };
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("indexer.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        if file
            .metadata()
            .map(|m| m.len() > 1024 * 1024)
            .unwrap_or(false)
        {
            let _ = file.set_len(0);
        }
        let _ = writeln!(file, "{}", msg);
    }
}

pub fn start_indexer(db_path: PathBuf) {
    let db_path_clone = db_path.clone();
    thread::spawn(move || {
        log_indexer("Indexer thread started");
        // Initialize COM for WinRT OCR
        let com_initialized = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        }
        .is_ok();

        // Set low priority so indexing never slows down foreground apps
        unsafe {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
            };
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }

        // ── Phase 1: Priority folders (Desktop, Downloads, Pictures, Documents) ──────
        // Delay 3s so the launcher window is fully up and the settings app can open
        // without being immediately hit by heavy first-launch DB writes.
        thread::sleep(std::time::Duration::from_secs(3));
        log_indexer("Starting Phase 1 priority scan...");
        if let Err(e) = run_indexer_folders(&db_path_clone, get_priority_folders()) {
            log_indexer(&format!("Priority indexer error: {:?}", e));
            eprintln!("Priority indexer error: {:?}", e);
        }

        // ── Phase 2: Full crawl (entire user profile + other drives) ───────
        // Runs 15s after launch to further reduce first-open contention.
        thread::sleep(std::time::Duration::from_secs(15));
        log_indexer("Starting Phase 2 full crawl...");
        if let Err(e) = run_indexer_folders(&db_path_clone, get_scan_folders()) {
            log_indexer(&format!("Indexer error: {:?}", e));
            eprintln!("Indexer error: {:?}", e);
        }
        log_indexer("Phase 2 crawl finished.");
        if com_initialized {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    });
}

/// Returns Desktop, Downloads, Pictures, Documents — fast to scan, highest value to user.
fn get_priority_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures,
            SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        };

        let get_folder = |guid| -> Option<PathBuf> {
            let result = SHGetKnownFolderPath(guid, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
            let mut len = 0;
            while *result.0.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(result.0, len));
            CoTaskMemFree(Some(result.0 as *const _));
            Some(PathBuf::from(s))
        };

        // Put Documents last so it doesn't block Desktop/Downloads/Pictures from being indexed immediately
        for guid in [
            &FOLDERID_Desktop,
            &FOLDERID_Downloads,
            &FOLDERID_Pictures,
            &FOLDERID_Documents,
        ] {
            if let Some(p) = get_folder(guid) {
                folders.push(p);
            }
        }
    }
    folders
}

fn is_builtin_ignored_dir(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    if name_lower.starts_with('$') {
        return true;
    }
    match name_lower.as_str() {
        "node_modules"
        | "dist"
        | "venv"
        | ".venv"
        | ".git"
        | "appdata"
        | "obj"
        | "bin"
        | "out"
        | ".next"
        | ".nuxt"
        | ".cache"
        | "cache"
        | ".cargo"
        | ".rustup"
        | ".npm"
        | ".m2"
        | ".nuget"
        | "vendor"
        | "cmake-build-debug"
        | "cmake-build-release"
        | ".yarn"
        | "__pycache__"
        | ".idea"
        | ".vscode"
        | ".gradle"
        | ".metadata"
        | "system volume information"
        | "temp"
        | "tmp" => true,
        _ => false,
    }
}

fn is_ignored_file(name: &str, ext: &str) -> bool {
    if name.starts_with("~$") {
        return true;
    }
    match ext {
        "tmp" | "temp" | "log" | "pdb" | "obj" | "o" | "class" | "db-wal" | "db-shm" => true,
        _ => false,
    }
}

struct PendingUpdate {
    path: String,
    name: String,
    extension: String,
    modified: i64,
    size: i64,
    is_dir: i64,
    content: Option<String>,
}

fn flush_updates(conn: &mut Connection, updates: &mut Vec<PendingUpdate>) -> anyhow::Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut insert_file_stmt = tx.prepare(
            "INSERT OR REPLACE INTO files (path, name, extension, modified, size, is_dir) VALUES (?, ?, ?, ?, ?, ?)"
        )?;
        let mut delete_fts_stmt = tx.prepare("DELETE FROM files_fts WHERE path = ?")?;
        let mut insert_fts_stmt =
            tx.prepare("INSERT INTO files_fts (path, content) VALUES (?, ?)")?;

        for update in updates.drain(..) {
            // Clone path before moving into params! so FTS statements can use it afterwards
            let path_clone = update.path.clone();
            insert_file_stmt.execute(params![
                update.path,
                update.name,
                update.extension,
                update.modified,
                update.size,
                update.is_dir
            ])?;

            if let Some(content) = update.content {
                delete_fts_stmt.execute([&path_clone])?;
                insert_fts_stmt.execute(params![path_clone, content])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

struct ExtractJob {
    path: PathBuf,
    name: String,
    ext: String,
    modified: i64,
    size: i64,
}

/// Extract document text + image OCR for the given files in parallel (the slow part of
/// indexing). Returns a receiver of completed PendingUpdates so the caller can flush them
/// to SQLite incrementally (bounded result channel keeps memory in check). Workers run at
/// below-normal priority so a fast first-pass index still yields to the foreground.
fn spawn_extractors(jobs: Vec<ExtractJob>) -> std::sync::mpsc::Receiver<PendingUpdate> {
    use std::collections::VecDeque;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    let n_workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1).min(4))
        .unwrap_or(2);

    let jobs = Arc::new(Mutex::new(VecDeque::from(jobs)));
    let (res_tx, res_rx) = mpsc::sync_channel::<PendingUpdate>(256); // bounded → backpressure

    for _ in 0..n_workers {
        let jobs = Arc::clone(&jobs);
        let res_tx = res_tx.clone();
        thread::spawn(move || {
            // OCR (WinRT) needs COM; below-normal so we don't starve the user.
            let com_initialized = unsafe {
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                )
            }
            .is_ok();
            unsafe {
                use windows::Win32::System::Threading::{
                    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
                };
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            }
            loop {
                let job = match jobs.lock().ok().and_then(|mut q| q.pop_front()) {
                    Some(j) => j,
                    None => break,
                };
                let content = extract_content(&job.path, &job.ext).unwrap_or_default();
                let _ = res_tx.send(PendingUpdate {
                    path: job.path.to_string_lossy().into_owned(),
                    name: job.name,
                    extension: job.ext,
                    modified: job.modified,
                    size: job.size,
                    is_dir: 0,
                    content: Some(content),
                });
            }
            if com_initialized {
                unsafe {
                    windows::Win32::System::Com::CoUninitialize();
                }
            }
        });
    }
    drop(res_tx); // workers hold the only senders now → res_rx ends when they finish

    res_rx
}

fn save_indexer_state_to_db(db_path: &Path, key: &str, value: &str) {
    if let Ok(conn) = Connection::open(db_path) {
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS indexer_state (
                key TEXT PRIMARY KEY,
                value TEXT
            );",
            [],
        );
        let _ = conn.execute(
            "INSERT OR REPLACE INTO indexer_state (key, value) VALUES (?, ?);",
            [key, value],
        );
    }
}

pub fn run_indexer_folders_force(db_path: &Path, folders: Vec<PathBuf>) -> anyhow::Result<()> {
    if let Ok(mut g) = INDEXING_PROGRESS.lock() {
        *g = "Forced indexing of added folder...".to_string();
        save_indexer_state_to_db(db_path, "progress", &g);
    }
    let res = run_indexer_folders_inner(db_path, folders);
    if let Ok(mut g) = INDEXING_PROGRESS.lock() {
        *g = "Idle".to_string();
        save_indexer_state_to_db(db_path, "progress", &g);
    }
    res
}

pub fn run_indexer_folders(db_path: &Path, folders: Vec<PathBuf>) -> anyhow::Result<()> {
    if IS_INDEXING.swap(true, Ordering::SeqCst) {
        log_indexer("Indexer already running, skipping overlapping run.");
        return Ok(());
    }
    save_indexer_state_to_db(db_path, "is_indexing", "1");

    if let Ok(mut g) = INDEXING_PROGRESS.lock() {
        *g = "Starting scan...".to_string();
        save_indexer_state_to_db(db_path, "progress", &g);
    }

    // RAII guard: resets IS_INDEXING to false even if run_indexer_folders_inner panics.
    // Without this, a panic would leave IS_INDEXING=true permanently until restart.
    struct IndexingGuard<'a> {
        db_path: &'a Path,
    }
    impl Drop for IndexingGuard<'_> {
        fn drop(&mut self) {
            IS_INDEXING.store(false, Ordering::SeqCst);
            save_indexer_state_to_db(self.db_path, "is_indexing", "0");
            if let Ok(mut g) = INDEXING_PROGRESS.lock() {
                *g = "Idle".to_string();
                save_indexer_state_to_db(self.db_path, "progress", &g);
            }
        }
    }
    let _guard = IndexingGuard { db_path };

    let res = run_indexer_folders_inner(db_path, folders);
    if res.is_ok() {
        if let Ok(mut g) = LAST_INDEX_TIME.lock() {
            unsafe {
                let st = windows::Win32::System::SystemInformation::GetLocalTime();
                *g = format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
                );
            }
            save_indexer_state_to_db(db_path, "last_index_time", &g);
        }
    }
    res
}

fn run_indexer_folders_inner(db_path: &Path, folders: Vec<PathBuf>) -> anyhow::Result<()> {
    log_indexer(&format!(
        "run_indexer_folders started with folders: {:?}",
        folders
    ));
    let mut conn = Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            extension TEXT NOT NULL,
            modified INTEGER NOT NULL,
            size INTEGER NOT NULL DEFAULT 0,
            is_dir INTEGER NOT NULL DEFAULT 0
        );",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_name ON files(name);",
        [],
    )?;

    // Migrate existing databases that may lack the new columns
    let _ = conn.execute(
        "ALTER TABLE files ADD COLUMN size INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE files ADD COLUMN is_dir INTEGER NOT NULL DEFAULT 0",
        [],
    );

    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
            path UNINDEXED,
            content
        );",
        [],
    )?;
    // Removed unbounded memory caching of db_files, fts_paths, seen_paths (#8)
    // They grew infinitely on large drives and caused memory leaks.
    let mut file_count = 0;
    let mut pending_updates = Vec::new();
    let mut extract_jobs: Vec<ExtractJob> = Vec::new();

    let mut low_priority_updates = Vec::new();
    let mut low_priority_extract_jobs = Vec::new();

    for folder in folders {
        log_indexer(&format!("Evaluating folder for index: {:?}", folder));
        if !folder.exists() {
            log_indexer(&format!("Folder does not exist, skipping: {:?}", folder));
            continue;
        }
        if let Ok(mut g) = INDEXING_PROGRESS.lock() {
            *g = format!("Scanning: {}", folder.to_string_lossy());
            save_indexer_state_to_db(db_path, "progress", &g);
        }
        log_indexer(&format!("Folder exists, starting WalkDir: {:?}", folder));
        let ignored_dirs = configured_ignored_dirs();
        let walker = WalkDir::new(&folder).into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !e.file_type().is_dir() || !is_ignored_dir_with_config(&name, &ignored_dirs)
        });

        let mut folder_file_count = 0;
        for entry in walker.filter_map(|e| e.ok()) {
            folder_file_count += 1;
            let path = entry.path();
            let is_file = path.is_file();
            let is_dir = path.is_dir();
            if !is_file && !is_dir {
                continue;
            }

            let path_str = match path.to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let file_size = if is_file {
                entry.metadata().map(|m| m.len()).unwrap_or(0) as i64
            } else {
                0
            };

            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let should_fts = is_indexable_content(&ext);

            // Fast path: query metadata DB to see if changed before doing FTS read
            let db_modified: Option<i64> = {
                let mut stmt = conn.prepare_cached("SELECT modified FROM files WHERE path = ?")?;
                stmt.query_row([&path_str], |r| r.get(0)).ok()
            };

            // Also check FTS table if it should have content
            let needs_fts_check = if should_fts && db_modified.is_some() {
                let mut stmt_fts =
                    conn.prepare_cached("SELECT rowid FROM files_fts WHERE path = ?")?;
                stmt_fts.query_row([&path_str], |_| Ok(())).is_err()
            } else {
                false
            };

            if db_modified.is_none() || db_modified.unwrap() != modified || needs_fts_check {
                let is_low_priority = is_in_low_priority_dir(&path);
                if is_file && should_fts {
                    // Defer the slow content/OCR extraction to the parallel pool below.
                    let job = ExtractJob {
                        path: path.to_path_buf(),
                        name,
                        ext,
                        modified,
                        size: file_size,
                    };
                    if is_low_priority {
                        low_priority_extract_jobs.push(job);
                    } else {
                        extract_jobs.push(job);
                    }
                } else {
                    // Folders / non-indexable files: just the name+meta row, no extraction.
                    let update = PendingUpdate {
                        path: path_str,
                        name,
                        extension: ext,
                        modified,
                        size: file_size,
                        is_dir: if is_dir { 1 } else { 0 },
                        content: None,
                    };
                    if is_low_priority {
                        low_priority_updates.push(update);
                    } else {
                        pending_updates.push(update);
                        if pending_updates.len() >= 1000 {
                            flush_updates(&mut conn, &mut pending_updates)?;
                        }
                    }
                }
            }

            file_count += 1;
            if file_count % 1000 == 0 {
                if let Ok(mut g) = INDEXING_PROGRESS.lock() {
                    *g = format!("Scanning: {} files processed...", file_count);
                    save_indexer_state_to_db(db_path, "progress", &g);
                }
            }
        }
        log_indexer(&format!(
            "Finished WalkDir for {:?}: scanned {} total entries",
            folder, folder_file_count
        ));
    }

    log_indexer("Flushing normal priority name/meta updates to database...");
    flush_updates(&mut conn, &mut pending_updates)?;

    // Extract document text + image OCR for changed files in parallel (the slow part),
    // flushing results as they stream back so memory stays bounded.
    if !extract_jobs.is_empty() {
        log_indexer(&format!(
            "Extracting content/OCR for {} normal priority files in parallel...",
            extract_jobs.len()
        ));
        let total_jobs = extract_jobs.len();
        let mut processed_jobs = 0;
        for update in spawn_extractors(extract_jobs) {
            processed_jobs += 1;
            if processed_jobs % 10 == 0 || processed_jobs == total_jobs {
                if let Ok(mut g) = INDEXING_PROGRESS.lock() {
                    *g = format!(
                        "Extracting: {}/{} files (OCR/Text)...",
                        processed_jobs, total_jobs
                    );
                    save_indexer_state_to_db(db_path, "progress", &g);
                }
            }
            pending_updates.push(update);
            if pending_updates.len() >= 500 {
                flush_updates(&mut conn, &mut pending_updates)?;
            }
        }
        flush_updates(&mut conn, &mut pending_updates)?;
    }

    if !low_priority_updates.is_empty() {
        log_indexer("Flushing low priority (target/build) name/meta updates to database...");
        let mut chunk = Vec::new();
        for update in low_priority_updates {
            chunk.push(update);
            if chunk.len() >= 1000 {
                flush_updates(&mut conn, &mut chunk)?;
            }
        }
        flush_updates(&mut conn, &mut chunk)?;
    }

    if !low_priority_extract_jobs.is_empty() {
        log_indexer(&format!(
            "Extracting content/OCR for {} low priority (target/build) files in parallel...",
            low_priority_extract_jobs.len()
        ));
        let total_jobs = low_priority_extract_jobs.len();
        let mut processed_jobs = 0;
        let mut pending_low = Vec::new();
        for update in spawn_extractors(low_priority_extract_jobs) {
            processed_jobs += 1;
            if processed_jobs % 10 == 0 || processed_jobs == total_jobs {
                if let Ok(mut g) = INDEXING_PROGRESS.lock() {
                    *g = format!(
                        "Extracting (low-priority): {}/{} files (OCR/Text)...",
                        processed_jobs, total_jobs
                    );
                    save_indexer_state_to_db(db_path, "progress", &g);
                }
            }
            pending_low.push(update);
            if pending_low.len() >= 500 {
                flush_updates(&mut conn, &mut pending_low)?;
            }
        }
        flush_updates(&mut conn, &mut pending_low)?;
    }

    // Deleted files cleanup skipped to prevent memory bloat of seen_paths (#8)

    log_indexer("run_indexer_folders completed successfully");
    Ok(())
}

pub fn get_scan_folders() -> Vec<PathBuf> {
    let app_settings = crate::settings::AppSettings::load();
    if !app_settings.scan_folders.is_empty() {
        return app_settings
            .scan_folders
            .iter()
            .map(PathBuf::from)
            .collect();
    }
    get_default_scan_folders()
}

pub fn get_default_scan_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();

    // Only scan the user's home profile directory by default.
    // ProgramFiles and other drive roots are excluded to prevent massive startup I/O.
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Profile, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

        let get_folder = |guid| -> Option<PathBuf> {
            let result = SHGetKnownFolderPath(guid, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
            let mut len = 0;
            while *result.0.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(result.0, len));
            CoTaskMemFree(Some(result.0 as *const _));
            Some(PathBuf::from(s))
        };

        if let Some(p) = get_folder(&FOLDERID_Profile) {
            folders.push(p);
        }
    }

    if folders.is_empty() {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            folders.push(PathBuf::from(profile));
        }
    }

    folders
}

pub fn extract_content_subprocess(path_str: &str) {
    let _ = path_str;
}

pub fn ocr_clipboard_image() -> Option<String> {
    use windows::core::Interface;
    use windows::ApplicationModel::DataTransfer::{Clipboard, StandardDataFormats};
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, SoftwareBitmap,
    };
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::IRandomAccessStream;

    let content = Clipboard::GetContent().ok()?;
    let bitmap_format = StandardDataFormats::Bitmap().ok()?;
    if !content.Contains(&bitmap_format).unwrap_or(false) {
        return None;
    }
    // RandomAccessStreamReference -> a readable stream of the clipboard bitmap.
    let stream_ref = content.GetBitmapAsync().ok()?.get().ok()?;
    let stream = stream_ref.OpenReadAsync().ok()?.get().ok()?;
    let stream: IRandomAccessStream = stream.cast().ok()?;

    let decoder = BitmapDecoder::CreateAsync(&stream).ok()?.get().ok()?;
    // Guard against absurdly large pastes (OOM protection, same spirit as the file path).
    if decoder.PixelWidth().unwrap_or(9999) > 6000 || decoder.PixelHeight().unwrap_or(9999) > 6000 {
        return None;
    }
    let raw = decoder.GetSoftwareBitmapAsync().ok()?.get().ok()?;

    // WinRT OCR requires Bgra8 + premultiplied alpha.
    let fmt_ok = raw.BitmapPixelFormat().ok() == Some(BitmapPixelFormat::Bgra8);
    let alpha_ok = raw.BitmapAlphaMode().ok() == Some(BitmapAlphaMode::Premultiplied);
    let bitmap = if fmt_ok && alpha_ok {
        raw
    } else {
        SoftwareBitmap::ConvertWithAlpha(
            &raw,
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
        )
        .ok()?
    };

    let engine = OcrEngine::TryCreateFromUserProfileLanguages().ok()?;
    let result = engine.RecognizeAsync(&bitmap).ok()?.get().ok()?;
    let text = result.Text().ok()?.to_string();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// OCR text from a saved image file on disk. Reuses the same WinRT OcrEngine pipeline
/// as the background indexer, but callable on demand (e.g., from a clipboard image result).
pub fn ocr_image_file(path: &Path) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, SoftwareBitmap,
    };
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::StorageFile;

    let path_str = path.to_str()?;
    let path_wide = HSTRING::from(path_str);

    let file = match StorageFile::GetFileFromPathAsync(&path_wide)
        .ok()
        .and_then(|async_op| async_op.get().ok())
    {
        Some(f) => f,
        None => {
            log_indexer(&format!(
                "OCR: Failed to get StorageFile for {:?}",
                path_str
            ));
            return None;
        }
    };

    let stream = match file
        .OpenAsync(windows::Storage::FileAccessMode::Read)
        .ok()
        .and_then(|async_op| async_op.get().ok())
    {
        Some(s) => s,
        None => {
            log_indexer(&format!("OCR: Failed to open stream for {:?}", path_str));
            return None;
        }
    };

    let decoder = match BitmapDecoder::CreateAsync(&stream)
        .ok()
        .and_then(|async_op| async_op.get().ok())
    {
        Some(d) => {
            // Enforce max image size for OCR to prevent OOM (#9)
            if d.PixelWidth().unwrap_or(9999) > 4000 || d.PixelHeight().unwrap_or(9999) > 4000 {
                return None;
            }
            d
        }
        None => return None,
    };

    // Get raw decoded bitmap (any pixel format)
    let raw_bitmap = match decoder
        .GetSoftwareBitmapAsync()
        .ok()
        .and_then(|async_op| async_op.get().ok())
    {
        Some(b) => b,
        None => {
            log_indexer(&format!(
                "OCR: Failed to get SoftwareBitmap for {:?}",
                path_str
            ));
            return None;
        }
    };

    // WinRT OCR requires Bgra8 + Premultiplied alpha — convert if needed
    let software_bitmap = {
        let fmt_ok = raw_bitmap.BitmapPixelFormat().ok() == Some(BitmapPixelFormat::Bgra8);
        let alpha_ok = raw_bitmap.BitmapAlphaMode().ok() == Some(BitmapAlphaMode::Premultiplied);
        if fmt_ok && alpha_ok {
            raw_bitmap
        } else {
            match SoftwareBitmap::ConvertWithAlpha(
                &raw_bitmap,
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Premultiplied,
            ) {
                Ok(converted) => converted,
                Err(e) => {
                    log_indexer(&format!(
                        "OCR: Bitmap conversion failed for {:?}: {:?}",
                        path_str, e
                    ));
                    return None;
                }
            }
        }
    };

    let ocr_engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(engine) => engine,
        Err(e) => {
            log_indexer(&format!("OCR: Failed to create OcrEngine: {:?}", e));
            return None;
        }
    };

    let ocr_result = match ocr_engine.RecognizeAsync(&software_bitmap).ok().and_then(
        |async_op: windows::Foundation::IAsyncOperation<windows::Media::Ocr::OcrResult>| {
            async_op.get().ok()
        },
    ) {
        Some(res) => res,
        None => {
            log_indexer(&format!("OCR: RecognizeAsync failed for {:?}", path_str));
            return None;
        }
    };

    let text = match ocr_result.Text() {
        Ok(t) => t.to_string(),
        Err(e) => {
            log_indexer(&format!("OCR: Failed to get text from result: {:?}", e));
            return None;
        }
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        log_indexer(&format!("OCR: No text found in {:?}", path_str));
        None
    } else {
        log_indexer(&format!(
            "OCR: Successfully extracted {} chars from {:?}",
            trimmed.len(),
            path_str
        ));
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn normalize_ignored_folder_name_uses_leaf_name() {
        assert_eq!(
            normalize_ignored_dir_name(r"C:\work\project\node_modules\"),
            Some("node_modules".to_string())
        );
        assert_eq!(
            normalize_ignored_dir_name("  .angular  "),
            Some(".angular".to_string())
        );
        assert_eq!(normalize_ignored_dir_name("  "), None);
    }

    #[test]
    fn configured_ignored_folder_matches_any_folder_name() {
        let configured = HashSet::from([".angular".to_string()]);
        assert!(is_ignored_dir_with_config(".angular", &configured));
        assert!(is_ignored_dir_with_config("node_modules", &configured));
        assert!(!is_ignored_dir_with_config("src", &configured));
    }
}
