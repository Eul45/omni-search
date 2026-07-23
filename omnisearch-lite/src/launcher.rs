use std::sync::atomic::{AtomicBool, Ordering};

static FOCUS_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;
use windows::{
    core::{w, PCWSTR},
    Win32::{Foundation::HWND, UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL},
};

pub fn launch(cmd: &str) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return;
    }

    // Map breadcrumbs and non-executable control panel applets to valid commands
    let cmd = match cmd {
        "Windows Defender Firewall > Customize settings > Private network settings"
        | "Windows Defender Firewall > Customize settings > Public network settings" => {
            "control.exe /name Microsoft.WindowsFirewall"
        }
        "System > Set priority notifications > Calls and reminders > Show incoming calls"
        | "System > Set priority notifications > Calls and reminders > Show reminders" => {
            "ms-settings:notifications"
        }
        "inetcpl.cpl" => "control.exe inetcpl.cpl",
        _ => cmd,
    };

    // ── VS Code direct line number opening ──────────────────────────────
    if let Some(rest) = cmd.strip_prefix("vscode:") {
        if let Some(last_colon) = rest.rfind(':') {
            let file_path = &rest[..last_colon];
            let line_number = &rest[last_colon + 1..];
            let _ = Command::new("code")
                .arg("-g")
                .arg(format!("{file_path}:{line_number}"))
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();
        }
        return;
    }

    // ── Window switcher: Focus window by HWND ──────────────────────────────
    if let Some(hwnd_str) = cmd.strip_prefix("window:") {
        if let Ok(hwnd_val) = hwnd_str.trim().parse::<isize>() {
            let target_hwnd = windows::Win32::Foundation::HWND(hwnd_val as *mut std::ffi::c_void);
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
                use windows::Win32::UI::WindowsAndMessaging::{
                    IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
                };
                if IsIconic(target_hwnd).as_bool() {
                    let _ = ShowWindow(target_hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(target_hwnd);
                let _ = SetActiveWindow(target_hwnd);
            }
        }
        return;
    }

    // ── Kill process by PID ────────────────────────────────────────────────
    if let Some(pid) = cmd.strip_prefix("kill:") {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", pid.trim()])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
        return;
    }

    // ── Action commands ────────────────────────────────────────────────────
    if let Some(action) = cmd.strip_prefix("action:") {
        if let Some(window_action) = action.strip_prefix("window:") {
            handle_window_action(window_action);
        } else {
            handle_action(action);
        }
        return;
    }

    let cmd_lower = cmd.to_lowercase();

    // ── ChatGPT: open URL (fills box via ?q=) then auto-submit with Enter ─
    if cmd.starts_with("https://chatgpt.com/?q=") {
        let cmd_wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            ShellExecuteW(
                HWND::default(),
                w!("open"),
                PCWSTR(cmd_wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
        // Spawn a thread: poll for the ChatGPT browser window to appear,
        // then focus it and send Enter to submit the filled prompt.
        std::thread::spawn(|| {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "Add-Type -AssemblyName Microsoft.VisualBasic; \
                     Add-Type -AssemblyName System.Windows.Forms; \
                     for ($i = 0; $i -lt 30; $i++) { \
                         $proc = Get-Process | Where-Object { $_.MainWindowTitle -match 'ChatGPT|OpenAI' -and $_.ProcessName -notmatch 'notepad|code' } | Select-Object -First 1; \
                         if ($proc) { \
                             for ($j = 0; $j -lt 4; $j++) { \
                                 $activated = $false; \
                                 try { \
                                     [Microsoft.VisualBasic.Interaction]::AppActivate($proc.Id); \
                                     $activated = $true; \
                                 } catch { \
                                     try { \
                                         [Microsoft.VisualBasic.Interaction]::AppActivate($proc.MainWindowTitle); \
                                         $activated = $true; \
                                     } catch {} \
                                 } \
                                 if ($activated) { \
                                     Start-Sleep -Milliseconds 200; \
                                     [System.Windows.Forms.SendKeys]::SendWait('{ENTER}'); \
                                 } \
                                 Start-Sleep -Milliseconds 1500; \
                             } \
                             break; \
                         } \
                         Start-Sleep -Milliseconds 500; \
                     }",
                ])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();
        });
        return;
    }

    if cmd.starts_with("http://")
        || cmd.starts_with("https://")
        || cmd_lower.ends_with(".lnk")
        || std::path::Path::new(cmd).exists()
    {
        let cmd_wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let res = ShellExecuteW(
                HWND::default(),
                w!("open"),
                PCWSTR(cmd_wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            let hinstance = res.0 as isize;
            if hinstance <= 32 {
                let res_openas = ShellExecuteW(
                    HWND::default(),
                    w!("openas"),
                    PCWSTR(cmd_wide.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    SW_SHOWNORMAL,
                );
                if res_openas.0 as isize <= 32 {
                    let _ = Command::new("notepad.exe").arg(cmd).spawn();
                }
            }
        }
        return;
    }

    let _ = if cmd.starts_with("ms-settings:") || cmd.starts_with("shell:") {
        Command::new("explorer.exe").arg(cmd).spawn()
    } else if let Some(rest) = cmd.strip_prefix("control.exe") {
        let mut c = Command::new("control.exe");
        for arg in rest.split_whitespace() {
            c.arg(arg);
        }
        c.spawn()
    } else if cmd.ends_with(".msc") {
        Command::new("mmc.exe").arg(cmd).spawn()
    } else {
        let mut parts = cmd.split_whitespace();
        if let Some(exe) = parts.next() {
            let mut c = Command::new(exe);
            for arg in parts {
                c.arg(arg);
            }
            match c.spawn() {
                Ok(child) => Ok(child),
                Err(_) => {
                    let cmd_wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
                    unsafe {
                        let _ = ShellExecuteW(
                            HWND::default(),
                            w!("open"),
                            PCWSTR(cmd_wide.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        );
                    }
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "fallback to ShellExecuteW",
                    ))
                }
            }
        } else {
            return;
        }
    };
}

fn handle_action(action: &str) {
    if action.starts_with("volume:") {
        if let Some(num_str) = action.strip_prefix("volume:") {
            if let Ok(pct) = num_str.parse::<u32>() {
                let percent = pct as f32 / 100.0;
                let _ = set_master_volume(percent);
            }
        }
        return;
    }
    match action {
        "toggle_theme" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "$p = 'HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize'; $v = (Get-ItemProperty -Path $p).AppsUseLightTheme; $n = if ($v -eq 1) { 0 } else { 1 }; Set-ItemProperty -Path $p -Name AppsUseLightTheme -Value $n; Set-ItemProperty -Path $p -Name SystemUsesLightTheme -Value $n",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "quit_all_apps" => unsafe {
            use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
            };
            struct Data {
                my_pid: u32,
            }
            unsafe extern "system" fn enum_func(hwnd: HWND, lparam: LPARAM) -> BOOL {
                let data = &*(lparam.0 as *const Data);
                if IsWindowVisible(hwnd).as_bool() {
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if pid != data.my_pid {
                        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                BOOL(1)
            }
            let data = Data {
                my_pid: std::process::id(),
            };
            let _ = EnumWindows(Some(enum_func), LPARAM(&data as *const _ as isize));
        },
        "quit_other_apps" => unsafe {
            use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
                PostMessageW, WM_CLOSE,
            };
            struct Data {
                fg: HWND,
                my_pid: u32,
            }
            unsafe extern "system" fn enum_func(hwnd: HWND, lparam: LPARAM) -> BOOL {
                let data = &*(lparam.0 as *const Data);
                if hwnd != data.fg && IsWindowVisible(hwnd).as_bool() {
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if pid != data.my_pid {
                        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                BOOL(1)
            }
            let fg = GetForegroundWindow();
            let data = Data {
                fg,
                my_pid: std::process::id(),
            };
            let _ = EnumWindows(Some(enum_func), LPARAM(&data as *const _ as isize));
        },
        "hide_other_apps" => unsafe {
            use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
                ShowWindow, SW_MINIMIZE,
            };
            struct Data {
                fg: HWND,
                my_pid: u32,
            }
            unsafe extern "system" fn enum_func(hwnd: HWND, lparam: LPARAM) -> BOOL {
                let data = &*(lparam.0 as *const Data);
                if hwnd != data.fg && IsWindowVisible(hwnd).as_bool() {
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if pid != data.my_pid {
                        let _ = ShowWindow(hwnd, SW_MINIMIZE);
                    }
                }
                BOOL(1)
            }
            let fg = GetForegroundWindow();
            let data = Data {
                fg,
                my_pid: std::process::id(),
            };
            let _ = EnumWindows(Some(enum_func), LPARAM(&data as *const _ as isize));
        },
        "toggle_hdr" => unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_LMENU, VK_LWIN,
            };
            const VK_B: u8 = 0x42;
            keybd_event(VK_LWIN.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(VK_LMENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(VK_B, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(VK_B, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_LMENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_LWIN.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        },
        "toggle_focus_session" => {
            FOCUS_ACTIVE.store(false, Ordering::Relaxed);
            let _ = Command::new("cmd")
                .args(["/C", "start", "ms-settings:focus"])
                .spawn();
        }
        cmd_str if cmd_str.starts_with("start_focus_session:") => {
            FOCUS_ACTIVE.store(true, Ordering::Relaxed);
            let cat = cmd_str
                .strip_prefix("start_focus_session:")
                .unwrap()
                .to_string();
            std::thread::spawn(move || {
                let _ = Command::new("cmd")
                    .args(["/C", "start", "ms-settings:focus"])
                    .spawn();
                if let Ok(appdata) = std::env::var("APPDATA") {
                    let db_path = std::path::PathBuf::from(appdata)
                        .join("omnisearch-lite")
                        .join("file_index.db");
                    if let Ok(conn) = rusqlite::Connection::open(db_path) {
                        if let Ok(blocked) = conn.query_row(
                            "SELECT blocked_apps FROM focus_categories WHERE name = ?",
                            [cat],
                            |row| row.get::<_, String>(0),
                        ) {
                            let items: Vec<String> = blocked
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            while FOCUS_ACTIVE.load(Ordering::Relaxed) {
                                for item in &items {
                                    if item.to_lowercase().ends_with(".exe") {
                                        let _ = Command::new("taskkill")
                                            .args(["/F", "/IM", item])
                                            .creation_flags(0x08000000)
                                            .output();
                                    } else {
                                        // It's a website or window title (e.g. "YouTube" or "youtube.com")
                                        let filter = format!("WINDOWTITLE eq *{}*", item);
                                        let _ = Command::new("taskkill")
                                            .args(["/F", "/FI", &filter, "/IM", "*"])
                                            .creation_flags(0x08000000)
                                            .output();
                                    }
                                }
                                std::thread::sleep(std::time::Duration::from_secs(3));
                            }
                        }
                    }
                }
            });
        }
        "open_settings" => {
            if let Ok(exe) = std::env::current_exe() {
                let _ = Command::new(exe).arg("--settings").spawn();
            }
        }
        "reveal_logs" => {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let _ = Command::new("explorer.exe")
                    .arg(std::path::PathBuf::from(appdata).join("omnisearch-lite"))
                    .spawn();
            }
        }
        "copy_version" => {
            let version = env!("CARGO_PKG_VERSION");
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    &format!("Set-Clipboard -Value '{}'", version),
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "copy_logs" => {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let log_path = std::path::PathBuf::from(appdata)
                    .join("omnisearch-lite")
                    .join("omnisearch-lite.log");
                // Escape single quotes for the PS literal ('' inside '...') so an
                // apostrophe in the user profile path can't break the command.
                let escaped = log_path.display().to_string().replace('\'', "''");
                let _ = Command::new("powershell")
                    .args([
                        "-WindowStyle",
                        "Hidden",
                        "-Command",
                        &format!("Get-Content '{}' -Raw | Set-Clipboard", escaped),
                    ])
                    .creation_flags(0x08000000)
                    .spawn();
            }
        }
        "quick_look" => unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_SPACE,
            };
            keybd_event(VK_SPACE.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(VK_SPACE.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        },
        "lock" => unsafe {
            let _ = windows::Win32::System::Shutdown::LockWorkStation();
        },
        "shutdown" => {
            let _ = Command::new("shutdown").args(["/s", "/t", "0"]).spawn();
        }
        "restart" => {
            let _ = Command::new("shutdown").args(["/r", "/t", "0"]).spawn();
        }
        "clipboard:paste_sequentially" => {
            if let Ok(appdata) = std::env::var("APPDATA") {
                // clipboard_history lives in file_index.db (the app's main DB) — this
                // previously opened a non-existent "index.db" and silently did nothing.
                let db_path = std::path::PathBuf::from(appdata)
                    .join("omnisearch-lite")
                    .join("file_index.db");
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    if let Ok(mut stmt) = conn.prepare("SELECT content FROM clipboard_history WHERE is_image = 0 ORDER BY timestamp DESC LIMIT 3") {
                        let items: Vec<String> = stmt.query_map([], |row| row.get(0)).map(|m| m.filter_map(|r| r.ok()).collect()).unwrap_or_default();
                        std::thread::spawn(move || {
                            for item in items.into_iter().rev() {
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                let _clip_guard = crate::clipboard_lock().lock().unwrap();
                                use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, SetClipboardData, CloseClipboard};
                                use windows::Win32::Foundation::{HANDLE, HWND};
                                use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
                                unsafe {
                                    let wide: Vec<u16> = item.encode_utf16().chain(std::iter::once(0)).collect();
                                    let size = wide.len() * 2;
                                    if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, size) {
                                        let ptr = GlobalLock(hmem) as *mut u16;
                                        if !ptr.is_null() {
                                            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                                            let _ = GlobalUnlock(hmem);
                                            if OpenClipboard(HWND::default()).is_ok() {
                                                let _ = EmptyClipboard();
                                                let _ = SetClipboardData(13, HANDLE(hmem.0 as *mut _)); // CF_UNICODETEXT
                                                let _ = CloseClipboard();
                                            }
                                        }
                                    }
                                }
                                // Send Ctrl+V, then Enter
                                let _ = std::process::Command::new("powershell")
                                    .args([
                                        "-WindowStyle", "Hidden",
                                        "-Command",
                                        "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('^v'); Start-Sleep -Milliseconds 100; [System.Windows.Forms.SendKeys]::SendWait('{ENTER}');"
                                    ])
                                    .creation_flags(0x08000000)
                                    .spawn();
                                std::thread::sleep(std::time::Duration::from_millis(500));
                            }
                        });
                    }
                }
            }
        }
        "sleep" => unsafe {
            let _ = windows::Win32::System::Power::SetSuspendState(false, false, false);
        },
        "hibernate" => unsafe {
            let _ = windows::Win32::System::Power::SetSuspendState(true, false, false);
        },
        "logout" => {
            let _ = Command::new("shutdown").arg("/l").spawn();
        }
        "sleep_displays" => {
            sleep_displays();
        }
        "show_screensaver" => {
            show_screensaver();
        }
        "show_desktop" => {
            send_win_d();
        }
        "open_run" => {
            let _ = Command::new("explorer.exe")
                .arg("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}")
                .spawn();
        }
        "open_recycle_bin" => {
            let _ = Command::new("explorer.exe")
                .arg("shell:RecycleBinFolder")
                .spawn();
        }
        "recycle" => unsafe {
            use windows::Win32::UI::Shell::{
                SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
            };
            let _ = SHEmptyRecycleBinW(
                HWND::default(),
                PCWSTR::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI,
            );
        },
        "flushdns" => {
            let _ = Command::new("cmd")
                .args(["/c", "ipconfig /flushdns"])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();
        }
        "envvars" => {
            let _ = Command::new("rundll32.exe")
                .args(["sysdm.cpl,EditEnvironmentVariables"])
                .spawn();
        }
        "clearclip" => unsafe {
            use windows::Win32::System::DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard,
            };
            if OpenClipboard(HWND::default()).is_ok() {
                let _ = EmptyClipboard();
                let _ = CloseClipboard();
            }
        },
        "hosts" => {
            let hosts = r"C:\Windows\System32\drivers\etc\hosts";
            let _ = Command::new("notepad.exe").arg(hosts).spawn();
        }
        "restart_explorer" => {
            let _ = Command::new("cmd")
                .args(["/c", "taskkill /F /IM explorer.exe & timeout /t 2 /nobreak >nul & start explorer.exe"])
                .creation_flags(0x08000000)
                .spawn();
        }
        "volume_up" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "$wshShell = New-Object -ComObject WScript.Shell; 1..5 | ForEach-Object { $wshShell.SendKeys([char]175) }",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "volume_down" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "$wshShell = New-Object -ComObject WScript.Shell; 1..5 | ForEach-Object { $wshShell.SendKeys([char]174) }",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "toggle_mute" => {
            if let Ok(m) = get_mute() {
                let _ = set_mute(!m);
            }
        }
        "mute" => {
            let _ = set_mute(true);
        }
        "unmute" => {
            let _ = set_mute(false);
        }
        "toggle_hidden_files" => {
            let _ = toggle_hidden_files();
        }
        "media:play_pause" => {
            send_media_key(windows::Win32::UI::Input::KeyboardAndMouse::VK_MEDIA_PLAY_PAUSE);
        }
        "media:next" => {
            send_media_key(windows::Win32::UI::Input::KeyboardAndMouse::VK_MEDIA_NEXT_TRACK);
        }
        "media:prev" => {
            send_media_key(windows::Win32::UI::Input::KeyboardAndMouse::VK_MEDIA_PREV_TRACK);
        }
        "media:stop" => {
            send_media_key(windows::Win32::UI::Input::KeyboardAndMouse::VK_MEDIA_STOP);
        }
        "toggle_bluetooth" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "$ErrorActionPreference='Stop'; try { $s = Get-Service bthserv; if ($s.Status -eq 'Running') { Stop-Service -Name 'bthserv' -Force } else { Start-Service -Name 'bthserv' } } catch { Start-Process 'ms-settings:bluetooth' }",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "toggle_wifi" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "$adapter = Get-NetAdapter -Name 'Wi-Fi' -ErrorAction SilentlyContinue; if ($adapter) { try { if ($adapter.Status -eq 'Up') { Disable-NetAdapter -Name 'Wi-Fi' -Confirm:$false -ErrorAction Stop } else { Enable-NetAdapter -Name 'Wi-Fi' -Confirm:$false -ErrorAction Stop } } catch { Start-Process 'ms-settings:network-wifi' } } else { Start-Process 'ms-settings:network-wifi' }",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "ipconfig" => {
            let _ = Command::new("cmd")
                .args(["/c", "start cmd /k ipconfig /all"])
                .creation_flags(0x08000000)
                .spawn();
        }
        "ip_release" => {
            let _ = Command::new("cmd")
                .args(["/c", "ipconfig /release"])
                .creation_flags(0x08000000)
                .spawn();
        }
        "ip_renew" => {
            let _ = Command::new("cmd")
                .args(["/c", "ipconfig /renew"])
                .creation_flags(0x08000000)
                .spawn();
        }
        "wifi_password" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "Start-Process cmd -ArgumentList '/k', 'netsh', 'wlan', 'show', 'profiles' -Wait; $profiles = (netsh wlan show profiles) -join \"`n\"; Start-Process cmd -ArgumentList '/k', 'echo', 'Run:', 'netsh wlan show profile name=\"PROFILE\" key=clear'",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        "kill_process_prompt" => {
            let _ = Command::new("cmd")
                .args(["/c", "start cmd /k \"echo Kill a process by name && set /p pname=Process name: && taskkill /F /IM %pname%\""])
                .creation_flags(0x08000000)
                .spawn();
        }
        "eject_cd" => {
            let _ = Command::new("powershell")
                .args([
                    "-WindowStyle", "Hidden",
                    "-Command",
                    "$wmp = New-Object -ComObject WMPlayer.OCX; $wmp.cdromCollection.Item(0).Eject()",
                ])
                .creation_flags(0x08000000)
                .spawn();
        }
        folder if folder.starts_with("folder:") => {
            let which = &folder[7..];
            let path = match which {
                "downloads" => {
                    get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Downloads)
                }
                "desktop" => get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Desktop),
                "documents" => {
                    get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Documents)
                }
                "pictures" => get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Pictures),
                "music" => get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Music),
                "videos" => get_known_folder_path(&windows::Win32::UI::Shell::FOLDERID_Videos),
                "temp" => std::env::var("TEMP").ok(),
                _ => None,
            };
            if let Some(p) = path {
                let _ = Command::new("explorer.exe").arg(p).spawn();
            }
        }
        _ => {}
    }
}

pub fn get_known_folder_path(folder_id: &windows::core::GUID) -> Option<String> {
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
        let result = SHGetKnownFolderPath(folder_id, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
        let mut len = 0;
        while *result.0.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(result.0, len));
        windows::Win32::System::Com::CoTaskMemFree(Some(result.0 as *const _));
        Some(s)
    }
}

fn get_target_window() -> Option<HWND> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowTextW,
        IsWindowVisible, GW_HWNDNEXT,
    };

    unsafe {
        let class: Vec<u16> = "omnisearch-lite\0".encode_utf16().collect();
        let launcher_hwnd = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).unwrap_or_default();
        let fg = GetForegroundWindow();

        if fg != launcher_hwnd && !fg.0.is_null() {
            Some(fg)
        } else if !launcher_hwnd.0.is_null() {
            let mut curr = GetWindow(launcher_hwnd, GW_HWNDNEXT);
            while let Ok(c) = curr {
                if c.0.is_null() {
                    break;
                }
                if IsWindowVisible(c).as_bool() {
                    let mut class_buf = [0u16; 256];
                    let class_len = GetClassNameW(c, &mut class_buf) as usize;
                    let class_name = String::from_utf16_lossy(&class_buf[..class_len]);
                    if class_name != "Shell_TrayWnd"
                        && class_name != "Progman"
                        && class_name != "omnisearch-lite"
                    {
                        let mut title_buf = [0u16; 256];
                        let title_len = GetWindowTextW(c, &mut title_buf) as usize;
                        if title_len > 0 {
                            return Some(c);
                        }
                    }
                }
                curr = GetWindow(c, GW_HWNDNEXT);
            }
            None
        } else {
            None
        }
    }
}

fn handle_window_action(action: &str) {
    let target_hwnd = match get_target_window() {
        Some(h) => h,
        None => return,
    };
    if let Some(num_str) = action.strip_prefix("open_desktop_") {
        if let Ok(idx) = num_str.parse::<u32>() {
            if let Ok(desktops) = winvd::get_desktops() {
                if let Some(d) = desktops.get(idx.saturating_sub(1) as usize) {
                    let _ = winvd::switch_desktop(d.clone());
                }
            }
        }
        return;
    }

    if action == "close_desktop" || action == "close_desktop_active" {
        if let Ok(d) = winvd::get_current_desktop() {
            if let Ok(desktops) = winvd::get_desktops() {
                if let Some(fallback) = desktops.first() {
                    let _ = winvd::remove_desktop(d.clone(), fallback.clone());
                }
            }
        }
        return;
    }

    if action == "rename_desktop" {
        std::thread::spawn(|| {
            let output = std::process::Command::new("powershell")
                .args(["-Command", "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.Interaction]::InputBox('Enter new desktop name:', 'Rename Desktop')"])
                .creation_flags(0x08000000)
                .output();
            if let Ok(out) = output {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    if let Ok(d) = winvd::get_current_desktop() {
                        let _ = d.set_name(&name);
                    }
                }
            }
        });
        return;
    }

    if let Some(num_str) = action.strip_prefix("move_desktop_") {
        if let Ok(idx) = num_str.parse::<u32>() {
            if let Ok(desktops) = winvd::get_desktops() {
                if let Some(d) = desktops.get(idx.saturating_sub(1) as usize) {
                    let _ = winvd::move_window_to_desktop(d.clone(), &target_hwnd);
                }
            }
        }
        return;
    }

    if action == "move_next_desktop" || action == "move_previous_desktop" {
        if let Ok(desktops) = winvd::get_desktops() {
            if let Ok(current) = winvd::get_desktop_by_window(target_hwnd.clone()) {
                if let Ok(idx) = current.get_index() {
                    let new_idx = if action == "move_next_desktop" {
                        idx as usize + 1
                    } else {
                        (idx as usize).saturating_sub(1)
                    };
                    if let Some(d) = desktops.get(new_idx) {
                        let _ = winvd::move_window_to_desktop(d.clone(), &target_hwnd);
                    }
                }
            }
        }
        return;
    }

    if action == "move_next_display" || action == "move_previous_display" {
        unsafe {
            use windows::Win32::Foundation::{BOOL, LPARAM};
            use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

            struct MonitorData {
                monitors: Vec<HMONITOR>,
            }

            unsafe extern "system" fn monitor_enum(
                hmonitor: HMONITOR,
                _hdc: HDC,
                _rect: *mut RECT,
                lparam: LPARAM,
            ) -> BOOL {
                let data = &mut *(lparam.0 as *mut MonitorData);
                data.monitors.push(hmonitor);
                BOOL(1)
            }

            let mut data = MonitorData {
                monitors: Vec::new(),
            };
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum),
                LPARAM(&mut data as *mut _ as isize),
            );

            if data.monitors.len() > 1 {
                use windows::Win32::Graphics::Gdi::{
                    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
                };
                let current_monitor = MonitorFromWindow(target_hwnd, MONITOR_DEFAULTTONEAREST);
                let current_idx = data
                    .monitors
                    .iter()
                    .position(|&m| m.0 == current_monitor.0)
                    .unwrap_or(0);

                let new_idx = if action == "move_next_display" {
                    (current_idx + 1) % data.monitors.len()
                } else {
                    (current_idx + data.monitors.len() - 1) % data.monitors.len()
                };

                let new_monitor = data.monitors[new_idx];

                let mut current_info = MONITORINFO::default();
                current_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                let _ = GetMonitorInfoW(current_monitor, &mut current_info);

                let mut new_info = MONITORINFO::default();
                new_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                let _ = GetMonitorInfoW(new_monitor, &mut new_info);

                use windows::Win32::UI::WindowsAndMessaging::{
                    GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
                };
                let mut r = windows::Win32::Foundation::RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    let w = r.right - r.left;
                    let h = r.bottom - r.top;

                    let cw = current_info.rcWork.right - current_info.rcWork.left;
                    let ch = current_info.rcWork.bottom - current_info.rcWork.top;
                    let nw = new_info.rcWork.right - new_info.rcWork.left;
                    let nh = new_info.rcWork.bottom - new_info.rcWork.top;

                    let rx = (r.left - current_info.rcWork.left) as f32 / cw as f32;
                    let ry = (r.top - current_info.rcWork.top) as f32 / ch as f32;

                    let new_x = new_info.rcWork.left + (rx * nw as f32) as i32;
                    let new_y = new_info.rcWork.top + (ry * nh as f32) as i32;

                    let _ = SetWindowPos(
                        target_hwnd,
                        HWND::default(),
                        new_x,
                        new_y,
                        w,
                        h,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
        }
        return;
    }

    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GetWindowRect, IsZoomed, SetWindowPos, ShowWindow, GWL_EXSTYLE,
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SW_MAXIMIZE, SW_RESTORE, WS_EX_TOPMOST,
    };

    unsafe {
        if action == "restore" {
            let _ = ShowWindow(target_hwnd, SW_RESTORE);
            return;
        }

        // Restore if maximized, unless we are maximizing or toggling topmost
        if action != "maximize" && action != "toggle_always_on_top" {
            if IsZoomed(target_hwnd).as_bool() {
                let _ = ShowWindow(target_hwnd, SW_RESTORE);
            }
        }

        if action == "maximize" {
            let _ = ShowWindow(target_hwnd, SW_MAXIMIZE);
            return;
        }

        if action == "toggle_always_on_top" {
            let ex_style = GetWindowLongPtrW(target_hwnd, GWL_EXSTYLE) as u32;
            let is_topmost = (ex_style & WS_EX_TOPMOST.0) != 0;
            let z_order = if is_topmost {
                HWND_NOTOPMOST
            } else {
                HWND_TOPMOST
            };
            let _ = SetWindowPos(
                target_hwnd,
                z_order,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            return;
        }

        // Get monitor info
        let monitor = MonitorFromWindow(target_hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return;
        }
        let mut info = MONITORINFO::default();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }

        let work = info.rcWork;
        let sw = work.right - work.left;
        let sh = work.bottom - work.top;

        let (x, y, w, h);

        match action {
            "center" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    let cw = r.right - r.left;
                    let ch = r.bottom - r.top;
                    w = cw;
                    h = ch;
                    x = work.left + (sw - w) / 2;
                    y = work.top + (sh - h) / 2;
                } else {
                    return;
                }
            }
            "almost_maximize" => {
                w = sw * 95 / 100;
                h = sh * 95 / 100;
                x = work.left + (sw - w) / 2;
                y = work.top + (sh - h) / 2;
            }
            "reasonable_size" => {
                w = sw * 70 / 100;
                h = sh * 70 / 100;
                x = work.left + (sw - w) / 2;
                y = work.top + (sh - h) / 2;
            }
            "maximize_height" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    w = r.right - r.left;
                    h = sh;
                    x = r.left;
                    y = work.top;
                } else {
                    return;
                }
            }
            "maximize_width" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    w = sw;
                    h = r.bottom - r.top;
                    x = work.left;
                    y = r.top;
                } else {
                    return;
                }
            }
            "move_left" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    w = r.right - r.left;
                    h = r.bottom - r.top;
                    x = work.left;
                    y = r.top;
                } else {
                    return;
                }
            }
            "move_right" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    w = r.right - r.left;
                    h = r.bottom - r.top;
                    x = work.right - w;
                    y = r.top;
                } else {
                    return;
                }
            }
            "left_half" => {
                w = sw / 2;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "right_half" => {
                w = sw / 2;
                h = sh;
                x = work.left + sw / 2;
                y = work.top;
            }
            "top_half" => {
                w = sw;
                h = sh / 2;
                x = work.left;
                y = work.top;
            }
            "bottom_half" => {
                w = sw;
                h = sh / 2;
                x = work.left;
                y = work.top + sh / 2;
            }
            "top_left_quarter" => {
                w = sw / 2;
                h = sh / 2;
                x = work.left;
                y = work.top;
            }
            "top_right_quarter" => {
                w = sw / 2;
                h = sh / 2;
                x = work.left + sw / 2;
                y = work.top;
            }
            "bottom_left_quarter" => {
                w = sw / 2;
                h = sh / 2;
                x = work.left;
                y = work.top + sh / 2;
            }
            "bottom_right_quarter" => {
                w = sw / 2;
                h = sh / 2;
                x = work.left + sw / 2;
                y = work.top + sh / 2;
            }
            "left_third" => {
                w = sw / 3;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "center_third" => {
                w = sw / 3;
                h = sh;
                x = work.left + sw / 3;
                y = work.top;
            }
            "right_third" => {
                w = sw / 3;
                h = sh;
                x = work.left + 2 * sw / 3;
                y = work.top;
            }
            "left_two_thirds" => {
                w = 2 * sw / 3;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "right_two_thirds" => {
                w = 2 * sw / 3;
                h = sh;
                x = work.left + sw / 3;
                y = work.top;
            }
            "make_larger" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    let cw = r.right - r.left;
                    let ch = r.bottom - r.top;
                    let cx = r.left + cw / 2;
                    let cy = r.top + ch / 2;
                    w = (cw as f32 * 1.1) as i32;
                    h = (ch as f32 * 1.1) as i32;
                    x = cx - w / 2;
                    y = cy - h / 2;
                } else {
                    return;
                }
            }
            "make_smaller" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    let cw = r.right - r.left;
                    let ch = r.bottom - r.top;
                    let cx = r.left + cw / 2;
                    let cy = r.top + ch / 2;
                    w = (cw as f32 * 0.9) as i32;
                    h = (ch as f32 * 0.9) as i32;
                    x = cx - w / 2;
                    y = cy - h / 2;
                } else {
                    return;
                }
            }
            "move_top" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    w = r.right - r.left;
                    h = r.bottom - r.top;
                    x = r.left;
                    y = work.top;
                } else {
                    return;
                }
            }
            "move_bottom" => {
                let mut r = RECT::default();
                if GetWindowRect(target_hwnd, &mut r).is_ok() {
                    w = r.right - r.left;
                    h = r.bottom - r.top;
                    x = r.left;
                    y = work.bottom - h;
                } else {
                    return;
                }
            }
            "bottom_center_sixth" => {
                w = sw / 3;
                h = sh / 2;
                x = work.left + sw / 3;
                y = work.top + sh / 2;
            }
            "top_center_sixth" => {
                w = sw / 3;
                h = sh / 2;
                x = work.left + sw / 3;
                y = work.top;
            }
            "bottom_left_sixth" => {
                w = sw / 3;
                h = sh / 2;
                x = work.left;
                y = work.top + sh / 2;
            }
            "bottom_right_sixth" => {
                w = sw / 3;
                h = sh / 2;
                x = work.left + 2 * sw / 3;
                y = work.top + sh / 2;
            }
            "top_left_sixth" => {
                w = sw / 3;
                h = sh / 2;
                x = work.left;
                y = work.top;
            }
            "top_right_sixth" => {
                w = sw / 3;
                h = sh / 2;
                x = work.left + 2 * sw / 3;
                y = work.top;
            }
            "bottom_center_two_thirds" => {
                w = 2 * sw / 3;
                h = sh / 2;
                x = work.left + sw / 6;
                y = work.top + sh / 2;
            }
            "top_center_two_thirds" => {
                w = 2 * sw / 3;
                h = sh / 2;
                x = work.left + sw / 6;
                y = work.top;
            }
            "bottom_third" => {
                w = sw;
                h = sh / 3;
                x = work.left;
                y = work.top + 2 * sh / 3;
            }
            "top_third" => {
                w = sw;
                h = sh / 3;
                x = work.left;
                y = work.top;
            }
            "bottom_three_fourths" => {
                w = sw;
                h = 3 * sh / 4;
                x = work.left;
                y = work.top + sh / 4;
            }
            "top_three_fourths" => {
                w = sw;
                h = 3 * sh / 4;
                x = work.left;
                y = work.top;
            }
            "bottom_two_thirds" => {
                w = sw;
                h = 2 * sh / 3;
                x = work.left;
                y = work.top + sh / 3;
            }
            "top_two_thirds" => {
                w = sw;
                h = 2 * sh / 3;
                x = work.left;
                y = work.top;
            }
            "first_fourth" => {
                w = sw / 4;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "second_fourth" => {
                w = sw / 4;
                h = sh;
                x = work.left + sw / 4;
                y = work.top;
            }
            "third_fourth" => {
                w = sw / 4;
                h = sh;
                x = work.left + sw / 2;
                y = work.top;
            }
            "last_fourth" => {
                w = sw / 4;
                h = sh;
                x = work.left + 3 * sw / 4;
                y = work.top;
            }
            "top_first_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left;
                y = work.top;
            }
            "top_second_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left + sw / 4;
                y = work.top;
            }
            "top_third_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left + sw / 2;
                y = work.top;
            }
            "top_last_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left + 3 * sw / 4;
                y = work.top;
            }
            "bottom_first_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left;
                y = work.top + sh / 2;
            }
            "bottom_second_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left + sw / 4;
                y = work.top + sh / 2;
            }
            "bottom_third_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left + sw / 2;
                y = work.top + sh / 2;
            }
            "bottom_last_fourth" => {
                w = sw / 4;
                h = sh / 2;
                x = work.left + 3 * sw / 4;
                y = work.top + sh / 2;
            }
            "last_third" => {
                w = sw / 3;
                h = sh;
                x = work.left + 2 * sw / 3;
                y = work.top;
            }
            "last_three_fourths" => {
                w = 3 * sw / 4;
                h = sh;
                x = work.left + sw / 4;
                y = work.top;
            }
            "last_two_thirds" => {
                w = 2 * sw / 3;
                h = sh;
                x = work.left + sw / 3;
                y = work.top;
            }
            "first_third" => {
                w = sw / 3;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "first_three_fourths" => {
                w = 3 * sw / 4;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "first_two_thirds" => {
                w = 2 * sw / 3;
                h = sh;
                x = work.left;
                y = work.top;
            }
            "center_half" => {
                w = sw / 2;
                h = sh;
                x = work.left + sw / 4;
                y = work.top;
            }
            "center_three_fourths" => {
                w = 3 * sw / 4;
                h = sh;
                x = work.left + sw / 8;
                y = work.top;
            }
            "center_two_thirds" => {
                w = 2 * sw / 3;
                h = sh;
                x = work.left + sw / 6;
                y = work.top;
            }
            _ => return,
        }

        let _ = SetWindowPos(
            target_hwnd,
            windows::Win32::Foundation::HWND::default(),
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn set_master_volume(percent: f32) -> Result<(), windows::core::Error> {
    unsafe {
        use windows::Win32::Media::Audio::Endpoints::*;
        use windows::Win32::Media::Audio::*;
        use windows::Win32::System::Com::*;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;

        volume.SetMasterVolumeLevelScalar(percent, std::ptr::null())?;
        Ok(())
    }
}

pub fn set_mute(muted: bool) -> Result<(), windows::core::Error> {
    unsafe {
        use windows::Win32::Media::Audio::Endpoints::*;
        use windows::Win32::Media::Audio::*;
        use windows::Win32::System::Com::*;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;

        volume.SetMute(muted, std::ptr::null())?;
        Ok(())
    }
}

pub fn get_mute() -> Result<bool, windows::core::Error> {
    unsafe {
        use windows::Win32::Media::Audio::Endpoints::*;
        use windows::Win32::Media::Audio::*;
        use windows::Win32::System::Com::*;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;

        let val = volume.GetMute()?;
        Ok(val.as_bool())
    }
}

pub fn toggle_hidden_files() -> Result<bool, windows::core::Error> {
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::*;
        use windows::Win32::UI::Shell::SHChangeNotify;
        use windows::Win32::UI::Shell::SHCNE_ASSOCCHANGED;
        use windows::Win32::UI::Shell::SHCNF_IDLIST;

        let subkey = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced\0"
            .encode_utf16()
            .collect::<Vec<u16>>();
        let value_name = "Hidden\0".encode_utf16().collect::<Vec<u16>>();

        let mut hkey = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ | KEY_WRITE,
            &mut hkey,
        );

        if status.is_err() {
            return Err(status.into());
        }

        let mut value_type = REG_VALUE_TYPE::default();
        let mut data = 0u32;
        let mut data_size = std::mem::size_of::<u32>() as u32;

        let status = RegQueryValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut data_size),
        );

        let new_val = if status.is_ok() && data == 1 {
            2u32 // change to hide
        } else {
            1u32 // change to show
        };

        let data_slice = std::slice::from_raw_parts(&new_val as *const u32 as *const u8, 4);
        let status = RegSetValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            0,
            REG_DWORD,
            Some(data_slice),
        );

        let _ = RegCloseKey(hkey);

        if status.is_ok() {
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
            Ok(new_val == 1)
        } else {
            Err(status.into())
        }
    }
}

fn sleep_displays() {
    unsafe {
        use windows::Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            UI::WindowsAndMessaging::{SendNotifyMessageW, WM_SYSCOMMAND},
        };

        const SC_MONITORPOWER: usize = 0xF170;
        // Broadcast without waiting: SendMessageW to HWND_BROADCAST blocks the UI
        // thread until every top-level window responds - one hung app froze us.
        let _ = SendNotifyMessageW(
            HWND(-1isize as *mut std::ffi::c_void),
            WM_SYSCOMMAND,
            WPARAM(SC_MONITORPOWER),
            LPARAM(2),
        );
    }
}

fn show_screensaver() {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let screensaver = std::path::PathBuf::from(system_root)
        .join("System32")
        .join("scrnsave.scr");
    if screensaver.exists() {
        let _ = Command::new(screensaver).arg("/s").spawn();
    } else {
        let _ = Command::new("explorer.exe")
            .arg("ms-settings:lockscreen")
            .spawn();
    }
}

fn send_win_d() {
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;

        let win = VK_LWIN;
        let d = VIRTUAL_KEY(0x44);
        let inputs = [
            key_input(win, KEYBD_EVENT_FLAGS(0)),
            key_input(d, KEYBD_EVENT_FLAGS(0)),
            key_input(d, KEYEVENTF_KEYUP),
            key_input(win, KEYEVENTF_KEYUP),
        ];

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn key_input(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub fn send_media_key(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) {
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;

        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_EXTENDEDKEY,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}
