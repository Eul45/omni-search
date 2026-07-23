#![windows_subsystem = "windows"]

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO,
    },
};

fn show_message(text: &str, title: &str, is_question: bool) -> bool {
    let text_wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

    let flags = if is_question {
        MB_YESNO | MB_ICONQUESTION
    } else {
        MB_OK | MB_ICONINFORMATION
    };

    unsafe {
        let result = MessageBoxW(
            HWND::default(),
            windows::core::PCWSTR(text_wide.as_ptr()),
            windows::core::PCWSTR(title_wide.as_ptr()),
            flags,
        );
        result == IDYES
    }
}

fn kill_processes() {
    // Terminate ProtonSearch.exe, plus the legacy omnisearch.exe name in case an old
    for exe in ["OmniSearchLite.exe", "omnisearch-lite.exe", "ProtonSearch.exe", "protonsearch.exe", "omnisearch.exe"] {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", exe])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output();
    }

}

fn delete_registry_value(key: &str, value: &str) {
    let _ = Command::new("reg")
        .args(["delete", key, "/v", value, "/f"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
}

fn cleanup_startup_entries() {
    let run_key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    let approved_run_key =
        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
    let approved_startup_folder_key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder";

    for value in ["protonsearch", "omnisearch", "OpenSearchOS"] {
        delete_registry_value(run_key, value);
        delete_registry_value(approved_run_key, value);
    }
    delete_registry_value(approved_startup_folder_key, "OpenSearch OS.lnk");
}

fn delete_dir_with_retry(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    for _ in 0..30 {
        if fs::remove_dir_all(path).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_cleanup = args.contains(&"--run-cleanup".to_string());

    // Only prompt for confirmation on the initial run
    if !is_cleanup {
        let confirm = show_message(
            "Are you sure you want to completely uninstall OmniSearch Lite?\n\nThis will terminate all running processes and permanently delete all application files, databases, and logs.",
            "Uninstall OmniSearch Lite",
            true
        );
        if !confirm {
            return;
        }
    }

    // Terminate any active application and gateway processes first
    kill_processes();

    cleanup_startup_entries();

    // Resolve paths
    let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();

    if local_appdata.is_empty() || appdata.is_empty() || user_profile.is_empty() {
        show_message(
            "Failed to resolve standard Windows system folders. Uninstallation aborted.",
            "Uninstall Error",
            false,
        );
        return;
    }

    // installer.iss installs to {localappdata}\Programs\OmniSearchLite; older branding
    // generations used "protonsearch", "omnisearch" and "OpenSearch OS". Clean all so upgrades from any
    // prior layout uninstall fully.
    let install_dir = PathBuf::from(&local_appdata)
        .join("Programs")
        .join("OmniSearchLite");
    let legacy_install_dirs: Vec<PathBuf> = ["ProtonSearch", "protonsearch", "omnisearch", "OpenSearch OS"]
        .iter()
        .map(|name| PathBuf::from(&local_appdata).join("Programs").join(name))
        .collect();
    let data_dir = PathBuf::from(&appdata).join("omnisearch-lite");
    // If the app was never launched after updating, main.rs's startup migration never ran,
    // so the old data dir may still hold the user's actual index/settings/history.
    let legacy_data_dirs: Vec<PathBuf> = vec![
        PathBuf::from(&appdata).join("protonsearch"),
        PathBuf::from(&appdata).join("omnisearch"),
    ];

    let current_exe = std::env::current_exe().unwrap_or_default();
    let current_exe_lower = current_exe.to_string_lossy().to_lowercase();
    let install_dir_lower = install_dir.to_string_lossy().to_lowercase();

    // Self-copy/redirection trick if uninstaller is running inside the install folder
    if !is_cleanup && current_exe_lower.starts_with(&install_dir_lower) {
        let temp_dir = std::env::temp_dir();
        let temp_exe = temp_dir.join("omnisearch-lite-uninstaller.exe");

        if fs::copy(&current_exe, &temp_exe).is_ok() {
            let spawn_res = Command::new(&temp_exe).arg("--run-cleanup").spawn();
            if spawn_res.is_ok() {
                // Exit immediately so original exe is unlocked
                return;
            }
        }
    }

    // If we are in cleanup mode, wait for original process to exit and unlock file
    if is_cleanup {
        thread::sleep(Duration::from_millis(500));
    }

    // 1. Run Inno Setup silent uninstaller if present
    let inno_uninstaller = install_dir.join("unins000.exe");
    if inno_uninstaller.exists() {
        let _ = Command::new(&inno_uninstaller)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .status();
    }

    // 2. Clear application data directory (SQLite DB, logs), including the legacy-branded
    //    one in case the migration in main.rs never ran (app updated but never launched)
    let _ = delete_dir_with_retry(&data_dir);
    for dir in &legacy_data_dirs {
        let _ = delete_dir_with_retry(dir);
    }

    // 3. Clear installation folder (any leftovers), including every legacy-branded one
    let _ = delete_dir_with_retry(&install_dir);
    for dir in &legacy_install_dirs {
        let _ = delete_dir_with_retry(dir);
    }

    // 4. Manually purge any lingering shortcuts (current "protonsearch" names and
    //    every legacy-branded one)
    let start_menu_programs = PathBuf::from(&appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    for name in ["omnisearch-lite.lnk", "protonsearch.lnk", "omnisearch.lnk", "OpenSearch OS.lnk"] {
        let desktop_lnk = PathBuf::from(&user_profile).join("Desktop").join(name);
        if desktop_lnk.exists() {
            let _ = fs::remove_file(&desktop_lnk);
        }
        let startup_lnk = start_menu_programs.join("Startup").join(name);
        if startup_lnk.exists() {
            let _ = fs::remove_file(&startup_lnk);
        }
    }
    for folder in ["omnisearch-lite", "protonsearch", "omnisearch", "OpenSearch OS"] {
        let startmenu_folder = start_menu_programs.join(folder);
        if startmenu_folder.exists() {
            let _ = fs::remove_dir_all(&startmenu_folder);
        }
    }

    // Success notification
    show_message(
        "OmniSearch Lite has been successfully uninstalled from your computer.",
        "Uninstall Complete",
        false,
    );

    // Self-delete the temp uninstaller executable if running from Temp
    if is_cleanup {
        let temp_exe = std::env::temp_dir().join("omnisearch-lite-uninstaller.exe");
        let _ = Command::new("cmd")
            .args([
                "/c",
                "start",
                "/b",
                "cmd",
                "/c",
                "timeout /t 1 /nobreak && del",
                &temp_exe.to_string_lossy(),
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
    }
}
