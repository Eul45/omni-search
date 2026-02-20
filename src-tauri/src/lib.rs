use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use tauri_plugin_opener::OpenerExt;
use base64::Engine;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexStatus {
    indexing: bool,
    ready: bool,
    indexed_count: u64,
    last_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    name: String,
    path: String,
    extension: String,
    size: u64,
    created_unix: i64,
    modified_unix: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateFile {
    name: String,
    path: String,
    size: u64,
    created_unix: i64,
    modified_unix: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateGroup {
    group_id: String,
    size: u64,
    total_bytes: u64,
    file_count: u32,
    files: Vec<DuplicateFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateScanStatus {
    running: bool,
    cancel_requested: bool,
    scanned_files: u64,
    total_files: u64,
    groups_found: u64,
    progress_percent: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveInfo {
    letter: String,
    path: String,
    filesystem: String,
    drive_type: String,
    is_ntfs: bool,
    can_open_volume: bool,
}

#[cfg(target_os = "windows")]
unsafe extern "C" {
    fn omni_start_indexing(drive_utf8: *const c_char) -> bool;
    fn omni_is_indexing() -> bool;
    fn omni_is_index_ready() -> bool;
    fn omni_indexed_file_count() -> u64;
    fn omni_last_error() -> *const c_char;
    fn omni_search_files_json(
        query_utf8: *const c_char,
        extension_utf8: *const c_char,
        min_size: u64,
        max_size: u64,
        min_created_unix: i64,
        max_created_unix: i64,
        limit: u32,
    ) -> *mut c_char;
    fn omni_find_duplicates_json(
        min_size: u64,
        max_groups: u32,
        max_files_per_group: u32,
    ) -> *mut c_char;
    fn omni_cancel_duplicate_scan() -> bool;
    fn omni_duplicate_scan_status_json() -> *mut c_char;
    fn omni_list_drives_json() -> *mut c_char;
    fn omni_free_string(ptr: *mut c_char);
}

#[cfg(target_os = "windows")]
fn read_last_error() -> Option<String> {
    // SAFETY: The C++ side returns a pointer valid for this call thread.
    let ptr = unsafe { omni_last_error() };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` is expected to be a valid, null-terminated C string.
    let value = unsafe { CStr::from_ptr(ptr).to_string_lossy().to_string() };
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(target_os = "windows")]
fn current_status() -> IndexStatus {
    // SAFETY: FFI functions have no side effects beyond reading atomics.
    let indexing = unsafe { omni_is_indexing() };
    // SAFETY: FFI function reads atomic state only.
    let ready = unsafe { omni_is_index_ready() };
    // SAFETY: FFI function reads atomic state only.
    let indexed_count = unsafe { omni_indexed_file_count() };
    IndexStatus {
        indexing,
        ready,
        indexed_count,
        last_error: read_last_error(),
    }
}

#[cfg(not(target_os = "windows"))]
fn current_status() -> IndexStatus {
    IndexStatus {
        indexing: false,
        ready: false,
        indexed_count: 0,
        last_error: Some("OmniSearch scanner is only supported on Windows.".to_string()),
    }
}

#[tauri::command]
fn start_indexing(drive: Option<String>) -> Result<IndexStatus, String> {
    #[cfg(target_os = "windows")]
    {
        let drive = drive.unwrap_or_else(|| "C".to_string());
        let c_drive = CString::new(drive).map_err(|_| "Invalid drive parameter".to_string())?;
        // SAFETY: `c_drive` lives long enough for this synchronous call.
        let started = unsafe { omni_start_indexing(c_drive.as_ptr()) };
        if !started {
            return Err(read_last_error().unwrap_or_else(|| "Failed to start indexing".to_string()));
        }
        return Ok(current_status());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = drive;
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn index_status() -> IndexStatus {
    current_status()
}

#[tauri::command]
fn search_files(
    query: String,
    extension: Option<String>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    min_created_unix: Option<i64>,
    max_created_unix: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<SearchResult>, String> {
    #[cfg(target_os = "windows")]
    {
        let c_query = CString::new(query).map_err(|_| "Invalid query".to_string())?;
        let c_extension =
            CString::new(extension.unwrap_or_default()).map_err(|_| "Invalid extension".to_string())?;

        let min_size = min_size.unwrap_or(0);
        let max_size = max_size.unwrap_or(u64::MAX);
        let min_created_unix = min_created_unix.unwrap_or(i64::MIN);
        let max_created_unix = max_created_unix.unwrap_or(i64::MAX);
        let limit = limit.unwrap_or(200).clamp(1, 5_000);

        // SAFETY: Inputs are valid null-terminated strings and primitive values.
        let raw_json = unsafe {
            omni_search_files_json(
                c_query.as_ptr(),
                c_extension.as_ptr(),
                min_size,
                max_size,
                min_created_unix,
                max_created_unix,
                limit,
            )
        };
        if raw_json.is_null() {
            return Err(read_last_error().unwrap_or_else(|| "Search failed".to_string()));
        }

        // SAFETY: `raw_json` points to a C string allocated by C++.
        let json = unsafe { CStr::from_ptr(raw_json).to_string_lossy().to_string() };
        // SAFETY: `raw_json` was allocated by C++ and must be released by C++.
        unsafe { omni_free_string(raw_json) };

        let parsed: Vec<SearchResult> =
            serde_json::from_str(&json).map_err(|err| format!("Invalid search payload: {err}"))?;
        Ok(parsed)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (
            query,
            extension,
            min_size,
            max_size,
            min_created_unix,
            max_created_unix,
            limit,
        );
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
async fn find_duplicate_groups(
    min_size: Option<u64>,
    max_groups: Option<u32>,
    max_files_per_group: Option<u32>,
) -> Result<Vec<DuplicateGroup>, String> {
    #[cfg(target_os = "windows")]
    {
        let min_size = min_size.unwrap_or(50 * 1024 * 1024);
        let max_groups = max_groups.unwrap_or(200).clamp(1, 1_000);
        let max_files_per_group = max_files_per_group.unwrap_or(80).clamp(2, 400);
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<DuplicateGroup>, String> {
            // SAFETY: Inputs are plain integers and function returns an allocated C string or null.
            let raw_json =
                unsafe { omni_find_duplicates_json(min_size, max_groups, max_files_per_group) };
            if raw_json.is_null() {
                return Err(
                    read_last_error().unwrap_or_else(|| "Failed to find duplicate files.".to_string())
                );
            }

            // SAFETY: `raw_json` points to a C string allocated by C++.
            let json = unsafe { CStr::from_ptr(raw_json).to_string_lossy().to_string() };
            // SAFETY: `raw_json` was allocated by C++ and must be released by C++.
            unsafe { omni_free_string(raw_json) };

            let parsed: Vec<DuplicateGroup> = serde_json::from_str(&json)
                .map_err(|err| format!("Invalid duplicate payload: {err}"))?;
            Ok(parsed)
        })
        .await
        .map_err(|err| format!("Duplicate scan task failed: {err}"))?
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (min_size, max_groups, max_files_per_group);
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn duplicate_scan_status() -> Result<DuplicateScanStatus, String> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: No inputs, returns an allocated C string or null.
        let raw_json = unsafe { omni_duplicate_scan_status_json() };
        if raw_json.is_null() {
            return Err(
                read_last_error().unwrap_or_else(|| "Failed to read duplicate scan status.".to_string())
            );
        }

        // SAFETY: `raw_json` points to a C string allocated by C++.
        let json = unsafe { CStr::from_ptr(raw_json).to_string_lossy().to_string() };
        // SAFETY: `raw_json` was allocated by C++ and must be released by C++.
        unsafe { omni_free_string(raw_json) };

        let parsed: DuplicateScanStatus = serde_json::from_str(&json)
            .map_err(|err| format!("Invalid duplicate status payload: {err}"))?;
        Ok(parsed)
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn cancel_duplicate_scan() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: FFI call only flips an atomic flag.
        let requested = unsafe { omni_cancel_duplicate_scan() };
        Ok(requested)
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn list_drives() -> Result<Vec<DriveInfo>, String> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: No parameters, returns allocated C string or null.
        let raw_json = unsafe { omni_list_drives_json() };
        if raw_json.is_null() {
            return Err(read_last_error().unwrap_or_else(|| "Failed to enumerate drives".to_string()));
        }

        // SAFETY: `raw_json` points to a C string allocated by C++.
        let json = unsafe { CStr::from_ptr(raw_json).to_string_lossy().to_string() };
        // SAFETY: `raw_json` was allocated by C++ and must be released by C++.
        unsafe { omni_free_string(raw_json) };

        let parsed: Vec<DriveInfo> =
            serde_json::from_str(&json).map_err(|err| format!("Invalid drives payload: {err}"))?;
        Ok(parsed)
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn open_file(app: tauri::AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::path::PathBuf;

        let target = PathBuf::from(path);
        if !target.exists() {
            return Err("File does not exist on disk.".to_string());
        }

        let target_path = target.to_string_lossy().into_owned();
        app.opener()
            .open_path(target_path, None::<&str>)
            .map_err(|err| format!("Failed to open file: {err}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, path);
        Err("File open is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn reveal_in_folder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::path::PathBuf;

        let target = PathBuf::from(path);
        if !target.exists() {
            return Err("File does not exist on disk.".to_string());
        }

        app.opener()
            .reveal_item_in_dir(&target)
            .map_err(|err| format!("Failed to reveal file in folder: {err}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, path);
        Err("Folder reveal is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|err| format!("Failed to open link: {err}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, url);
        Err("Opening external links is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn load_preview_data_url(path: String) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        use std::fs;
        use std::path::PathBuf;

        let file_path = PathBuf::from(path);
        if !file_path.exists() {
            return Err("Preview target does not exist.".to_string());
        }
        if !file_path.is_file() {
            return Err("Preview target is not a file.".to_string());
        }

        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "ico" => "image/x-icon",
            "pdf" => "application/pdf",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "m4v" => "video/x-m4v",
            "avi" => "video/x-msvideo",
            "mkv" => "video/x-matroska",
            "wmv" => "video/x-ms-wmv",
            _ => return Err("Preview not supported for this file type.".to_string()),
        };

        let metadata = fs::metadata(&file_path).map_err(|err| format!("Preview metadata read failed: {err}"))?;
        let max_preview_bytes = match mime {
            "application/pdf" => 8 * 1024 * 1024_u64,
            "video/mp4" | "video/webm" | "video/quicktime" | "video/x-m4v" | "video/x-msvideo"
            | "video/x-matroska" | "video/x-ms-wmv" => 20 * 1024 * 1024_u64,
            _ => 12 * 1024 * 1024_u64,
        };

        if metadata.len() > max_preview_bytes {
            return Err(format!("Preview skipped: file too large ({} bytes).", metadata.len()));
        }

        let bytes = fs::read(&file_path).map_err(|err| format!("Preview read failed: {err}"))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(format!("data:{mime};base64,{encoded}"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Preview loading is only supported on Windows.".to_string())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            start_indexing,
            index_status,
            search_files,
            find_duplicate_groups,
            duplicate_scan_status,
            cancel_duplicate_scan,
            list_drives,
            open_file,
            reveal_in_folder,
            open_external_url,
            load_preview_data_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
