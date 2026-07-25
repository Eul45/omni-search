use base64::Engine;
use std::sync::Arc;
mod sync_server;
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

#[cfg(windows)]
use std::{
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::Path,
};
#[cfg(windows)]
use windows::{
    core::{implement, PCWSTR},
    Win32::{
        Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK},
        System::{
            Com::IDataObject,
            Ole::{IDropSource, IDropSource_Impl, DROPEFFECT, DROPEFFECT_COPY},
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
        },
        UI::Shell::{
            Common::ITEMIDLIST, ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID,
            SHCreateDataObject, SHDoDragDrop,
        },
    },
};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod apps;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;

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
    is_directory: bool,
}

const SEND_TO_PHONE_ARG: &str = "--send-to-phone";
const SEND_TO_PHONE_RESULT_EVENT: &str = "desktop-send-to-phone-result";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopSendToPhoneResult {
    queued: usize,
    failed: usize,
    messages: Vec<String>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextPreviewPayload {
    text: String,
    truncated: bool,
    matched: bool,
}

#[cfg(windows)]
struct OwnedItemIdList(*mut ITEMIDLIST);

#[cfg(windows)]
impl OwnedItemIdList {
    fn from_path(path: &Path) -> Result<Self, String> {
        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let pidl = unsafe { ILCreateFromPathW(PCWSTR(wide_path.as_ptr())) };
        if pidl.is_null() {
            Err(format!(
                "Failed to create a shell item for '{}'.",
                path.display()
            ))
        } else {
            Ok(Self(pidl))
        }
    }

    fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0 as *const ITEMIDLIST
    }

    fn as_mut_ptr(&self) -> *mut ITEMIDLIST {
        self.0
    }
}

#[cfg(windows)]
impl Drop for OwnedItemIdList {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ILFree(Some(self.0 as *const ITEMIDLIST));
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(windows)]
#[implement(IDropSource)]
struct NativeFileDropSource;

#[cfg(windows)]
#[allow(non_snake_case)]
impl IDropSource_Impl for NativeFileDropSource_Impl {
    fn QueryContinueDrag(
        &self,
        fescapepressed: windows_core::BOOL,
        grfkeystate: MODIFIERKEYS_FLAGS,
    ) -> windows_core::HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if grfkeystate & MK_LBUTTON == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> windows_core::HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

#[cfg(target_os = "windows")]
unsafe extern "C" {
    fn omni_start_indexing(
        drive_utf8: *const c_char,
        include_directories: bool,
        scan_all_drives: bool,
    ) -> bool;
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
    fn omni_cancel_search() -> bool;
    fn omni_find_duplicates_json(
        min_size: u64,
        max_groups: u32,
        max_files_per_group: u32,
    ) -> *mut c_char;
    fn omni_cancel_duplicate_scan() -> bool;
    fn omni_duplicate_scan_status_json() -> *mut c_char;
    fn omni_list_drives_json() -> *mut c_char;
    fn omni_delete_path(path_utf8: *const c_char, recycle_bin: bool) -> bool;
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
fn start_indexing(
    drive: Option<String>,
    include_folders: Option<bool>,
    #[allow(non_snake_case)] includeFolders: Option<bool>,
    include_all_drives: Option<bool>,
    #[allow(non_snake_case)] includeAllDrives: Option<bool>,
) -> Result<IndexStatus, String> {
    #[cfg(target_os = "windows")]
    {
        let drive = drive.unwrap_or_else(|| "C".to_string());
        let include_folders = include_folders.or(includeFolders).unwrap_or(false);
        let include_all_drives = include_all_drives.or(includeAllDrives).unwrap_or(false);
        let c_drive = CString::new(drive).map_err(|_| "Invalid drive parameter".to_string())?;
        // SAFETY: `c_drive` lives long enough for this synchronous call.
        let started =
            unsafe { omni_start_indexing(c_drive.as_ptr(), include_folders, include_all_drives) };
        if !started {
            return Err(read_last_error().unwrap_or_else(|| "Failed to start indexing".to_string()));
        }
        return Ok(current_status());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (
            drive,
            include_folders,
            includeFolders,
            include_all_drives,
            includeAllDrives,
        );
        Err("OmniSearch scanner is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn index_status() -> IndexStatus {
    current_status()
}

#[tauri::command]
fn current_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64"
    }
    #[cfg(target_arch = "x86")]
    {
        "i686"
    }
    #[cfg(target_arch = "arm")]
    {
        "armv7"
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "arm"
    )))]
    {
        std::env::consts::ARCH
    }
}

#[tauri::command]
async fn search_files(
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
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<SearchResult>, String> {
            let c_query = CString::new(query).map_err(|_| "Invalid query".to_string())?;
            let c_extension = CString::new(extension.unwrap_or_default())
                .map_err(|_| "Invalid extension".to_string())?;

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

            let parsed: Vec<SearchResult> = serde_json::from_str(&json)
                .map_err(|err| format!("Invalid search payload: {err}"))?;
            Ok(parsed)
        })
        .await
        .map_err(|err| format!("Search task failed: {err}"))?
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
fn cancel_search() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: FFI call only advances the search cancellation token.
        let cancelled = unsafe { omni_cancel_search() };
        Ok(cancelled)
    }

    #[cfg(not(target_os = "windows"))]
    {
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
                return Err(read_last_error()
                    .unwrap_or_else(|| "Failed to find duplicate files.".to_string()));
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
            return Err(read_last_error()
                .unwrap_or_else(|| "Failed to read duplicate scan status.".to_string()));
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
fn delete_path(
    path: String,
    recycle_bin: Option<bool>,
    #[allow(non_snake_case)] recycleBin: Option<bool>,
) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let recycle_bin = recycle_bin.or(recycleBin).unwrap_or(false);
        let c_path = CString::new(path).map_err(|_| "Invalid path parameter".to_string())?;
        let ok = unsafe { omni_delete_path(c_path.as_ptr(), recycle_bin) };
        if !ok {
            return Err(read_last_error().unwrap_or_else(|| "Delete failed".to_string()));
        }
        return Ok(true);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, recycle_bin, recycleBin);
        Err("Delete is only supported on Windows.".to_string())
    }
}

#[tauri::command]
async fn select_folder() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            unsafe {
                use windows::Win32::System::Com::{
                    CoCreateInstance, CoInitializeEx, CoUninitialize,
                    COINIT_APARTMENTTHREADED, CLSCTX_ALL,
                };
                use windows::Win32::UI::Shell::{
                    FileOpenDialog, IFileOpenDialog, FOS_PICKFOLDERS,
                    SIGDN_FILESYSPATH,
                };

                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

                let result: Result<String, String> = (|| {
                    let dialog: IFileOpenDialog = CoCreateInstance(
                        &FileOpenDialog, None, CLSCTX_ALL,
                    )
                    .map_err(|e| format!("Dialog create failed: {e}"))?;

                    let opts = dialog
                        .GetOptions()
                        .map_err(|e| format!("GetOptions: {e}"))?;
                    dialog
                        .SetOptions(opts | FOS_PICKFOLDERS)
                        .map_err(|e| format!("SetOptions: {e}"))?;

                    dialog
                        .Show(None)
                        .map_err(|_| "No folder selected".to_string())?;

                    let item = dialog
                        .GetResult()
                        .map_err(|e| format!("GetResult: {e}"))?;

                    let name = item
                        .GetDisplayName(SIGDN_FILESYSPATH)
                        .map_err(|e| format!("GetDisplayName: {e}"))?;

                    let path = name.to_string()
                        .map_err(|e| format!("to_string: {e}"))?;

                    Ok(path)
                })();

                CoUninitialize();
                let _ = tx.send(result);
            }
        });

        rx.recv()
            .map_err(|_| "Thread communication failed".to_string())?
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Folder selection is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn rename_path(path: String, new_name: String) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        use std::fs;
        use std::path::PathBuf;

        let current_path = PathBuf::from(path);
        if !current_path.exists() {
            return Err("File does not exist on disk.".to_string());
        }

        let trimmed_name = new_name.trim();
        if trimmed_name.is_empty() {
            return Err("Name cannot be empty.".to_string());
        }
        if trimmed_name.contains('\\') || trimmed_name.contains('/') {
            return Err("Name must not include path separators.".to_string());
        }

        let parent = current_path
            .parent()
            .ok_or_else(|| "Failed to resolve the parent directory.".to_string())?;
        let next_path = parent.join(trimmed_name);

        if next_path == current_path {
            return Ok(current_path.to_string_lossy().into_owned());
        }
        if next_path.exists() {
            return Err("An item with that name already exists.".to_string());
        }

        fs::rename(&current_path, &next_path)
            .map_err(|err| format!("Failed to rename item: {err}"))?;

        Ok(next_path.to_string_lossy().into_owned())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, new_name);
        Err("Rename is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn list_drives() -> Result<Vec<DriveInfo>, String> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: No parameters, returns allocated C string or null.
        let raw_json = unsafe { omni_list_drives_json() };
        if raw_json.is_null() {
            return Err(
                read_last_error().unwrap_or_else(|| "Failed to enumerate drives".to_string())
            );
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
fn open_path_in_console(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
        use windows::Win32::System::Threading::CREATE_NEW_CONSOLE;

        let requested_path = PathBuf::from(path);
        if !requested_path.exists() {
            return Err("Path does not exist on disk.".to_string());
        }

        let target_directory = if requested_path.is_dir() {
            requested_path
        } else {
            requested_path
                .parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| {
                    "Failed to resolve the parent folder for the requested path.".to_string()
                })?
        };

        if !target_directory.is_dir() {
            return Err("Resolved console target is not a directory.".to_string());
        }

        Command::new("cmd.exe")
            .creation_flags(CREATE_NEW_CONSOLE.0)
            .current_dir(&target_directory)
            .spawn()
            .map_err(|err| format!("Failed to open a console for the selected path: {err}"))?;

        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Opening a console for a path is only supported on Windows.".to_string())
    }
}

#[tauri::command]
fn start_native_file_drag(window: tauri::WebviewWindow, path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::path::PathBuf;
        use std::sync::mpsc;

        let file_path = PathBuf::from(&path);
        if !file_path.exists() {
            return Err("File does not exist on disk.".to_string());
        }
        if !file_path.is_file() {
            return Err("Only files can be dragged out of OmniSearch.".to_string());
        }

        let window_for_drag = window.clone();
        let path_for_drag = path.clone();
        let (tx, rx) = mpsc::channel();

        window
            .run_on_main_thread(move || {
                let result = start_native_file_drag_impl(&window_for_drag, &path_for_drag);
                let _ = tx.send(result);
            })
            .map_err(|err| format!("Failed to start the native drag request: {err}"))?;

        return rx
            .recv()
            .map_err(|_| "Failed to receive the native drag result.".to_string())?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, path);
        Err("Native file drag is only supported on Windows.".to_string())
    }
}

#[cfg(target_os = "windows")]
fn start_native_file_drag_impl<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    path: &str,
) -> Result<(), String> {
    let file_path = std::path::PathBuf::from(path);
    let hwnd = window
        .hwnd()
        .map_err(|err| format!("Failed to access the native window handle: {err}"))?;

    let folder_pidl = OwnedItemIdList::from_path(&file_path)?;
    let item_pidl = unsafe {
        let item_ptr = ILFindLastID(folder_pidl.as_ptr());
        if item_ptr.is_null() {
            return Err("Failed to resolve the dragged file in the Windows shell.".to_string());
        }

        let cloned_item = ILClone(item_ptr);
        if cloned_item.is_null() {
            return Err("Failed to clone the dragged file shell item.".to_string());
        }

        OwnedItemIdList(cloned_item)
    };

    if !unsafe { ILRemoveLastID(Some(folder_pidl.as_mut_ptr())) }.as_bool() {
        return Err("Failed to resolve the parent folder for drag and drop.".to_string());
    }

    let children = [item_pidl.as_ptr()];
    let data_object: IDataObject = unsafe {
        SHCreateDataObject(
            Some(folder_pidl.as_ptr()),
            Some(&children),
            None::<&IDataObject>,
        )
        .map_err(|err| format!("Failed to prepare the dragged file: {err}"))?
    };
    let drop_source: IDropSource = NativeFileDropSource.into();

    unsafe {
        SHDoDragDrop(Some(hwnd), &data_object, &drop_source, DROPEFFECT_COPY)
            .map_err(|err| format!("Failed to start the native file drag: {err}"))?;
    }

    Ok(())
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

#[cfg(target_os = "windows")]
const TEXT_PREVIEW_READ_LIMIT_BYTES: u64 = 4 * 1024 * 1024;

#[cfg(target_os = "windows")]
enum TextPreviewEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Ansi,
}

#[cfg(target_os = "windows")]
fn likely_utf16_encoding(bytes: &[u8]) -> Option<TextPreviewEncoding> {
    if bytes.len() < 4 {
        return None;
    }

    let unit_count = bytes.len() / 2;
    if unit_count < 2 {
        return None;
    }

    let even_nulls = bytes
        .iter()
        .step_by(2)
        .take(unit_count)
        .filter(|byte| **byte == 0)
        .count();
    let odd_nulls = bytes
        .iter()
        .skip(1)
        .step_by(2)
        .take(unit_count)
        .filter(|byte| **byte == 0)
        .count();

    if odd_nulls * 3 >= unit_count && even_nulls * 16 <= unit_count {
        Some(TextPreviewEncoding::Utf16Le)
    } else if even_nulls * 3 >= unit_count && odd_nulls * 16 <= unit_count {
        Some(TextPreviewEncoding::Utf16Be)
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn resolve_text_preview_encoding(
    bytes: &[u8],
    requested_mode: Option<&str>,
) -> TextPreviewEncoding {
    match requested_mode.map(|value| value.trim().to_ascii_lowercase()) {
        Some(mode) if mode == "utf8" => TextPreviewEncoding::Utf8,
        Some(mode) if mode == "utf16" => TextPreviewEncoding::Utf16Le,
        Some(mode) if mode == "utf16be" => TextPreviewEncoding::Utf16Be,
        Some(mode) if mode == "ansi" => TextPreviewEncoding::Ansi,
        _ => {
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                TextPreviewEncoding::Utf8
            } else if bytes.starts_with(&[0xFF, 0xFE]) {
                TextPreviewEncoding::Utf16Le
            } else if bytes.starts_with(&[0xFE, 0xFF]) {
                TextPreviewEncoding::Utf16Be
            } else if let Some(encoding) = likely_utf16_encoding(bytes) {
                encoding
            } else if std::str::from_utf8(bytes).is_ok() {
                TextPreviewEncoding::Utf8
            } else {
                TextPreviewEncoding::Ansi
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn decode_utf16_preview(bytes: &[u8], big_endian: bool) -> String {
    let start_offset = if !big_endian && bytes.starts_with(&[0xFF, 0xFE]) {
        2
    } else if big_endian && bytes.starts_with(&[0xFE, 0xFF]) {
        2
    } else {
        0
    };

    let units = bytes[start_offset..]
        .chunks_exact(2)
        .map(|chunk| {
            if big_endian {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_le_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();

    String::from_utf16_lossy(&units)
}

#[cfg(target_os = "windows")]
fn decode_text_preview(bytes: &[u8], requested_mode: Option<&str>) -> String {
    match resolve_text_preview_encoding(bytes, requested_mode) {
        TextPreviewEncoding::Utf16Le => decode_utf16_preview(bytes, false),
        TextPreviewEncoding::Utf16Be => decode_utf16_preview(bytes, true),
        TextPreviewEncoding::Utf8 => {
            let trimmed = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                &bytes[3..]
            } else {
                bytes
            };
            String::from_utf8_lossy(trimmed).into_owned()
        }
        TextPreviewEncoding::Ansi => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(target_os = "windows")]
fn normalize_preview_text(mut text: String) -> String {
    if text.starts_with('\u{feff}') {
        text = text.trim_start_matches('\u{feff}').to_string();
    }
    if text.contains('\0') {
        text = text.replace('\0', "");
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(target_os = "windows")]
fn char_boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect()
}

#[cfg(target_os = "windows")]
fn take_text_chars(text: &str, max_chars: usize) -> (String, bool) {
    let boundaries = char_boundaries(text);
    let total_chars = boundaries.len().saturating_sub(1);
    if total_chars <= max_chars {
        return (text.to_string(), false);
    }

    let end_byte = boundaries[max_chars];
    let mut preview = text[..end_byte].to_string();
    preview.push('…');
    (preview, true)
}

#[cfg(target_os = "windows")]
fn find_case_insensitive_match(text: &str, query: &str) -> Option<(usize, usize)> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return None;
    }

    let lower_text = text.to_lowercase();
    let lower_query = trimmed_query.to_lowercase();
    let match_index = lower_text.find(&lower_query)?;

    let start_char = lower_text[..match_index].chars().count();
    let match_char_len = lower_query.chars().count();
    let boundaries = char_boundaries(text);
    let total_chars = boundaries.len().saturating_sub(1);
    let start_byte = *boundaries.get(start_char)?;
    let end_byte = boundaries[(start_char + match_char_len).min(total_chars)];
    Some((start_byte, end_byte))
}

#[cfg(target_os = "windows")]
fn excerpt_around_match(
    text: &str,
    match_range: (usize, usize),
    max_chars: usize,
) -> (String, bool) {
    let boundaries = char_boundaries(text);
    let total_chars = boundaries.len().saturating_sub(1);
    let start_char = boundaries.partition_point(|boundary| *boundary < match_range.0);
    let end_char = boundaries.partition_point(|boundary| *boundary < match_range.1);
    let match_len_chars = end_char.saturating_sub(start_char).max(1);
    if total_chars <= max_chars {
        return (text.to_string(), false);
    }

    let focus_match_early = max_chars <= 220;
    let mut excerpt_start_char = start_char;
    let mut excerpt_end_char = (excerpt_start_char + max_chars).min(total_chars);

    if !focus_match_early {
        let available_context = max_chars.saturating_sub(match_len_chars);
        let minimum_leading_context = 24;
        let context_before =
            available_context.min((available_context / 2).max(minimum_leading_context));
        excerpt_start_char = start_char.saturating_sub(context_before);
        excerpt_end_char = (excerpt_start_char + max_chars).min(total_chars);
        let preferred_end_char =
            (end_char + available_context.saturating_sub(context_before)).min(total_chars);
        if excerpt_end_char < preferred_end_char {
            excerpt_end_char = preferred_end_char;
            excerpt_start_char = excerpt_end_char.saturating_sub(max_chars);
        }
        if excerpt_end_char.saturating_sub(excerpt_start_char) < max_chars && excerpt_start_char > 0
        {
            excerpt_start_char = excerpt_end_char.saturating_sub(max_chars);
        }
    }

    let excerpt_start_byte = boundaries[excerpt_start_char];
    let excerpt_end_byte = boundaries[excerpt_end_char];
    let mut excerpt = text[excerpt_start_byte..excerpt_end_byte].to_string();
    let mut truncated = false;

    if excerpt_start_char > 0 {
        excerpt.insert(0, '…');
        truncated = true;
    }
    if excerpt_end_char < total_chars {
        excerpt.push('…');
        truncated = true;
    }

    (excerpt, truncated)
}

#[cfg(target_os = "windows")]
fn build_text_preview_payload(
    decoded_text: String,
    max_chars: usize,
    content_query: Option<&str>,
) -> TextPreviewPayload {
    let normalized_text = normalize_preview_text(decoded_text);
    let effective_max_chars = max_chars.clamp(120, 12_000);

    if normalized_text.is_empty() {
        return TextPreviewPayload {
            text: String::new(),
            truncated: false,
            matched: false,
        };
    }

    if let Some(query) = content_query {
        if let Some(match_range) = find_case_insensitive_match(&normalized_text, query) {
            let (text, truncated) =
                excerpt_around_match(&normalized_text, match_range, effective_max_chars);
            return TextPreviewPayload {
                text,
                truncated,
                matched: true,
            };
        }
    }

    let (text, truncated) = take_text_chars(&normalized_text, effective_max_chars);
    TextPreviewPayload {
        text,
        truncated,
        matched: false,
    }
}

#[tauri::command]
fn load_text_preview(
    path: String,
    max_chars: Option<usize>,
    content_query: Option<String>,
    content_mode: Option<String>,
) -> Result<TextPreviewPayload, String> {
    #[cfg(target_os = "windows")]
    {
        use std::{fs::File, io::Read, path::PathBuf};

        let file_path = PathBuf::from(path);
        if !file_path.exists() {
            return Err("Preview target does not exist.".to_string());
        }
        if !file_path.is_file() {
            return Err("Preview target is not a file.".to_string());
        }

        let file =
            File::open(&file_path).map_err(|err| format!("Text preview open failed: {err}"))?;
        let mut reader = file.take(TEXT_PREVIEW_READ_LIMIT_BYTES + 1);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|err| format!("Text preview read failed: {err}"))?;

        let byte_truncated = bytes.len() as u64 > TEXT_PREVIEW_READ_LIMIT_BYTES;
        if byte_truncated {
            bytes.truncate(TEXT_PREVIEW_READ_LIMIT_BYTES as usize);
        }

        let decoded = decode_text_preview(&bytes, content_mode.as_deref());
        let mut payload = build_text_preview_payload(
            decoded,
            max_chars.unwrap_or(3200),
            content_query.as_deref(),
        );
        payload.truncated = payload.truncated || byte_truncated;
        Ok(payload)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, max_chars, content_query, content_mode);
        Err("Text preview loading is only supported on Windows.".to_string())
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

        let metadata = fs::metadata(&file_path)
            .map_err(|err| format!("Preview metadata read failed: {err}"))?;
        let max_preview_bytes = match mime {
            "application/pdf" => 8 * 1024 * 1024_u64,
            "video/mp4" | "video/webm" | "video/quicktime" | "video/x-m4v" | "video/x-msvideo"
            | "video/x-matroska" | "video/x-ms-wmv" => 20 * 1024 * 1024_u64,
            _ => 12 * 1024 * 1024_u64,
        };

        if metadata.len() > max_preview_bytes {
            return Err(format!(
                "Preview skipped: file too large ({} bytes).",
                metadata.len()
            ));
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

#[tauri::command]
fn start_mobile_sync_server(
    sync_state: tauri::State<'_, Arc<sync_server::SyncState>>,
) -> Result<sync_server::SyncServerInfo, String> {
    sync_server::start_sync_server(Arc::clone(&sync_state));
    get_mobile_sync_server_info(sync_state)
}

#[tauri::command]
fn stop_mobile_sync_server(
    sync_state: tauri::State<'_, Arc<sync_server::SyncState>>,
) -> Result<sync_server::SyncServerInfo, String> {
    sync_server::stop_sync_server(Arc::clone(&sync_state));
    get_mobile_sync_server_info(sync_state)
}

#[tauri::command]
fn get_mobile_sync_server_info(
    sync_state: tauri::State<'_, Arc<sync_server::SyncState>>,
) -> Result<sync_server::SyncServerInfo, String> {
    let running = *sync_state
        .server_running
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut address = sync_state
        .local_ip
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if running {
        let current_ip = sync_server::get_local_ip();
        if !current_ip.is_empty() && current_ip != "127.0.0.1" && current_ip != address {
            if let Ok(mut ip) = sync_state.local_ip.lock() {
                *ip = current_ip.clone();
            }
            address = current_ip;
        }
    }
    let port = 9876;
    let token = sync_state
        .pairing_token
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let fingerprint = sync_state
        .cert_fingerprint
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let pairing_uri = sync_server::build_pairing_uri(&address, &token, &fingerprint);
    let qr_svg = sync_server::generate_qr_svg(&pairing_uri);
    let connected_clients = *sync_state
        .client_count
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let pending_approvals = sync_state
        .pending_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let file_transfers = sync_server::transfer_snapshots(sync_state.as_ref());

    Ok(sync_server::SyncServerInfo {
        running,
        address,
        port,
        qr_svg,
        connected_clients,
        pairing_uri,
        pending_approvals,
        file_transfers,
    })
}

#[tauri::command]
fn send_file_to_mobile(
    sync_state: tauri::State<'_, Arc<sync_server::SyncState>>,
    path: String,
) -> Result<sync_server::MobileTransferSnapshot, String> {
    sync_server::queue_file_transfer(&sync_state, &path)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn shell_send_paths_from_args(args: &[String]) -> Vec<String> {
    let Some(marker_index) = args.iter().position(|arg| arg == SEND_TO_PHONE_ARG) else {
        return Vec::new();
    };

    args.iter()
        .skip(marker_index + 1)
        .filter(|arg| !arg.trim().is_empty())
        .cloned()
        .collect()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn file_name_for_notice(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn handle_shell_send_to_phone(app: &tauri::AppHandle, args: &[String]) {
    let paths = shell_send_paths_from_args(args);
    if paths.is_empty() {
        return;
    }

    desktop::focus_existing_instance(app);

    if paths.len() > 1 {
        let _ = app.emit(
            SEND_TO_PHONE_RESULT_EVENT,
            DesktopSendToPhoneResult {
                queued: 0,
                failed: paths.len(),
                messages: vec![
                    "Select one file at a time in Explorer to send to your phone.".to_string(),
                ],
            },
        );
        return;
    }

    let sync_state = app.state::<Arc<sync_server::SyncState>>();
    let mut queued = 0;
    let mut failed = 0;
    let mut messages = Vec::new();

    for path in paths {
        match sync_server::queue_file_transfer(&sync_state, &path) {
            Ok(snapshot) => {
                queued += 1;
                messages.push(format!("Queued {} for phone.", snapshot.name));
            }
            Err(err) => {
                failed += 1;
                messages.push(format!("{}: {err}", file_name_for_notice(&path)));
            }
        }
    }

    let _ = app.emit(
        SEND_TO_PHONE_RESULT_EVENT,
        DesktopSendToPhoneResult {
            queued,
            failed,
            messages,
        },
    );
}

#[cfg(windows)]
fn run_reg_add(args: &[String]) -> Result<(), String> {
    let status = std::process::Command::new("reg")
        .args(args)
        .creation_flags(0x08000000)
        .status()
        .map_err(|err| format!("Could not update Explorer context menu: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Explorer context menu registration failed with status {status}."
        ))
    }
}

#[cfg(windows)]
fn register_windows_send_to_phone_shell_verb() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("Could not locate OmniSearch executable: {err}"))?;
    let exe_path = exe.to_string_lossy().into_owned();
    let verb_key = r"HKCU\Software\Classes\*\shell\OmniSearch.SendToPhone";
    let command_key = r"HKCU\Software\Classes\*\shell\OmniSearch.SendToPhone\command";
    let command = format!("\"{exe_path}\" {SEND_TO_PHONE_ARG} \"%1\"");

    run_reg_add(&[
        "add".into(),
        verb_key.into(),
        "/ve".into(),
        "/d".into(),
        "Send to OmniSearch Phone".into(),
        "/f".into(),
    ])?;
    run_reg_add(&[
        "add".into(),
        verb_key.into(),
        "/v".into(),
        "Icon".into(),
        "/d".into(),
        exe_path,
        "/f".into(),
    ])?;
    run_reg_add(&[
        "add".into(),
        verb_key.into(),
        "/v".into(),
        "MultiSelectModel".into(),
        "/d".into(),
        "Single".into(),
        "/f".into(),
    ])?;
    run_reg_add(&[
        "add".into(),
        command_key.into(),
        "/ve".into(),
        "/d".into(),
        command,
        "/f".into(),
    ])
}

#[tauri::command]
fn approve_mobile_pairing(
    sync_state: tauri::State<'_, Arc<sync_server::SyncState>>,
    device_id: String,
) -> Result<(), String> {
    sync_server::approve_pairing_request(Arc::clone(&sync_state), &device_id)
}

#[tauri::command]
fn reject_mobile_pairing(
    sync_state: tauri::State<'_, Arc<sync_server::SyncState>>,
    device_id: String,
) -> Result<(), String> {
    sync_server::reject_pairing_request(Arc::clone(&sync_state), &device_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.manage(desktop::desktop_state_for_builder());
        builder = builder.manage(Arc::new(sync_server::SyncState::new()));
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            desktop::focus_existing_instance(app);
            handle_shell_send_to_phone(app, &argv);
        }));
        builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
        builder = builder.plugin(tauri_plugin_process::init());
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
        builder = builder.plugin(desktop::window_state_plugin());
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            start_indexing,
            index_status,
            current_arch,
            search_files,
            cancel_search,
            find_duplicate_groups,
            duplicate_scan_status,
            cancel_duplicate_scan,
            delete_path,
            select_folder,
            rename_path,
            list_drives,
            open_file,
            reveal_in_folder,
            open_path_in_console,
            start_native_file_drag,
            open_external_url,
            load_text_preview,
            load_preview_data_url,
            apps::list_installed_apps,
            apps::launch_installed_app,
            apps::reveal_installed_app,
            apps::load_installed_app_icon_data_url,
            desktop::get_desktop_settings,
            desktop::open_full_window_command,
            desktop::open_quick_window_command,
            desktop::reset_window_layout_command,
            desktop::sync_window_theme_command,
            desktop::update_desktop_settings,
            start_mobile_sync_server,
            stop_mobile_sync_server,
            get_mobile_sync_server_info,
            approve_mobile_pairing,
            reject_mobile_pairing,
            send_file_to_mobile
        ])
        .setup(|app| {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                desktop::setup(app)?;
                #[cfg(windows)]
                if let Err(err) = register_windows_send_to_phone_shell_verb() {
                    eprintln!("{err}");
                }

                let args = std::env::args().collect::<Vec<_>>();
                if !shell_send_paths_from_args(&args).is_empty() {
                    let app_handle = app.handle().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(900));
                        handle_shell_send_to_phone(&app_handle, &args);
                    });
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                desktop::handle_window_event(window, event);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
