use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            start_indexing,
            index_status,
            search_files
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
