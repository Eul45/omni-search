use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Cursor,
    process::Command,
    sync::{Mutex, OnceLock},
};

#[cfg(windows)]
use image::{DynamicImage, ImageFormat, RgbaImage};
#[cfg(windows)]
use std::{mem::size_of, os::windows::process::CommandExt, path::PathBuf};
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use tauri_plugin_opener::OpenerExt;
#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{SIZE, RPC_E_CHANGED_MODE},
        Graphics::Gdi::{
            BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS,
            DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP,
        },
        System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::Shell::{
            IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF,
            SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
        },
    },
};
#[cfg(windows)]
use windows_core::{Error as WindowsError, Interface};

static APP_ICON_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn app_icon_cache() -> &'static Mutex<HashMap<String, String>> {
    APP_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub launch_target: String,
    pub is_file_system: bool,
    pub reveal_path: Option<String>,
    pub display_path: String,
    pub size: u64,
    pub created_unix: u64,
    pub modified_unix: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInstalledApp {
    name: String,
    path: String,
    is_file_system: bool,
}

#[cfg(windows)]
struct ComInitGuard {
    should_uninitialize: bool,
}

#[cfg(windows)]
impl Drop for ComInitGuard {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

#[cfg(windows)]
fn ensure_com_initialized() -> Result<ComInitGuard, String> {
    unsafe {
        let result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if result.is_ok() {
            Ok(ComInitGuard {
                should_uninitialize: true,
            })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(ComInitGuard {
                should_uninitialize: false,
            })
        } else {
            Err(format!(
                "Failed to initialize COM for app shell access: {}",
                WindowsError::from(result)
            ))
        }
    }
}

#[cfg(windows)]
fn run_powershell_script(script: &str) -> Result<String, String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|err| format!("Failed to run PowerShell for installed apps: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "PowerShell app discovery failed.".to_string()
        } else {
            format!("PowerShell app discovery failed: {stderr}")
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(windows)]
fn allowed_app_path(path: &str) -> bool {
    let extension = PathBuf::from(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "exe" | "lnk" | "appref-ms" | "bat" | "cmd" | "com" | "msc"
    )
}

#[cfg(windows)]
fn system_time_to_unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(windows)]
fn discover_installed_apps() -> Result<Vec<InstalledApp>, String> {
    let script = r#"
$shell = New-Object -ComObject Shell.Application
$folder = $shell.NameSpace('shell:AppsFolder')
if ($null -eq $folder) {
  throw 'Failed to open shell:AppsFolder.'
}
$allowedExtensions = @('.exe', '.lnk', '.appref-ms', '.bat', '.cmd', '.com', '.msc')
$items = foreach ($item in $folder.Items()) {
  $name = "$($item.Name)".Trim()
  $path = "$($item.Path)".Trim()
  if ([string]::IsNullOrWhiteSpace($name) -or [string]::IsNullOrWhiteSpace($path)) {
    continue
  }

  $isFileSystem = $false
  try { $isFileSystem = [bool]$item.IsFileSystem } catch {}

  if ($isFileSystem) {
    $extension = [System.IO.Path]::GetExtension($path)
    if ([string]::IsNullOrWhiteSpace($extension) -or -not ($allowedExtensions -contains $extension.ToLowerInvariant())) {
      continue
    }
  }

  [PSCustomObject]@{
    name = $name
    path = $path
    isFileSystem = $isFileSystem
  }
}

@($items | Sort-Object Name, Path -Unique) | ConvertTo-Json -Compress -Depth 3
"#;

    let json = run_powershell_script(script)?;
    if json.is_empty() {
        return Ok(Vec::new());
    }

    let raw: Vec<RawInstalledApp> = serde_json::from_str(&json)
        .map_err(|err| format!("Invalid installed app payload: {err}"))?;

    let mut seen = HashSet::new();
    let mut apps = Vec::with_capacity(raw.len());

    for entry in raw {
        let name = entry.name.trim().to_string();
        let launch_target = entry.path.trim().to_string();
        if name.is_empty() || launch_target.is_empty() {
            continue;
        }

        let reveal_path = if entry.is_file_system && allowed_app_path(&launch_target) {
            let path = PathBuf::from(&launch_target);
            if path.exists() {
                Some(path.to_string_lossy().into_owned())
            } else {
                None
            }
        } else {
            None
        };

        let dedupe_key = launch_target.to_ascii_lowercase();
        if !seen.insert(dedupe_key) {
            continue;
        }

        let display_path = reveal_path
            .clone()
            .unwrap_or_else(|| "Installed app".to_string());

        let (size, created_unix, modified_unix) = reveal_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| {
                (
                    metadata.len(),
                    metadata.created().map(system_time_to_unix).unwrap_or(0),
                    metadata.modified().map(system_time_to_unix).unwrap_or(0),
                )
            })
            .unwrap_or((0, 0, 0));

        apps.push(InstalledApp {
            id: launch_target.clone(),
            name,
            launch_target,
            is_file_system: entry.is_file_system,
            reveal_path,
            display_path,
            size,
            created_unix,
            modified_unix,
        });
    }

    apps.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.launch_target.cmp(&right.launch_target))
    });

    Ok(apps)
}

#[cfg(windows)]
fn to_wide_null_terminated(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
fn resolve_shell_parsing_name(launch_target: &str, reveal_path: Option<&str>, is_file_system: bool) -> String {
    if is_file_system {
        reveal_path.unwrap_or(launch_target).to_string()
    } else {
        format!(r"shell:AppsFolder\{launch_target}")
    }
}

#[cfg(windows)]
fn hbitmap_to_png_data_url(bitmap_handle: HBITMAP) -> Result<String, String> {
    let mut bitmap = BITMAP::default();
    let object_read = unsafe {
        GetObjectW(
            bitmap_handle.into(),
            size_of::<BITMAP>() as i32,
            Some((&mut bitmap as *mut BITMAP).cast()),
        )
    };
    if object_read == 0 {
        unsafe {
            let _ = DeleteObject(bitmap_handle.into());
        }
        return Err("Failed to inspect the installed app icon bitmap.".to_string());
    }

    let width = bitmap.bmWidth;
    let height = bitmap.bmHeight.abs();
    if width <= 0 || height <= 0 {
        unsafe {
            let _ = DeleteObject(bitmap_handle.into());
        }
        return Err("Installed app icon returned invalid dimensions.".to_string());
    }

    let mut bitmap_info = BITMAPINFO::default();
    bitmap_info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };

    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    let device_context = unsafe { CreateCompatibleDC(None) };
    if device_context.is_invalid() {
        unsafe {
            let _ = DeleteObject(bitmap_handle.into());
        }
        return Err("Failed to create an icon bitmap context.".to_string());
    }

    let scanlines = unsafe {
        GetDIBits(
            device_context,
            bitmap_handle,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        )
    };

    unsafe {
        let _ = DeleteDC(device_context);
        let _ = DeleteObject(bitmap_handle.into());
    }

    if scanlines == 0 {
        return Err("Failed to read installed app icon pixels.".to_string());
    }

    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let rgba = RgbaImage::from_raw(width as u32, height as u32, pixels)
        .ok_or_else(|| "Installed app icon pixel buffer was invalid.".to_string())?;

    let mut png_bytes = Vec::new();
    DynamicImage::ImageRgba8(rgba)
        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
        .map_err(|err| format!("Failed to encode installed app icon: {err}"))?;

    let encoded = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    Ok(format!("data:image/png;base64,{encoded}"))
}

#[cfg(windows)]
fn load_shell_icon_data_url(
    launch_target: &str,
    reveal_path: Option<&str>,
    is_file_system: bool,
) -> Result<String, String> {
    let parsing_name = resolve_shell_parsing_name(launch_target, reveal_path, is_file_system);
    let cache_key = parsing_name.to_ascii_lowercase();

    if let Ok(cache) = app_icon_cache().lock() {
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.clone());
        }
    }

    let _com_guard = ensure_com_initialized()?;
    let wide_name = to_wide_null_terminated(&parsing_name);
    let shell_item: IShellItem = unsafe {
        SHCreateItemFromParsingName(PCWSTR(wide_name.as_ptr()), None)
            .map_err(|err| format!("Failed to access the installed app shell item: {err}"))?
    };
    let image_factory: IShellItemImageFactory = shell_item
        .cast()
        .map_err(|err| format!("Failed to read the installed app icon provider: {err}"))?;

    let bitmap = unsafe {
        image_factory
            .GetImage(
                SIZE { cx: 64, cy: 64 },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK | SIIGBF(0),
            )
            .map_err(|err| format!("Failed to render the installed app icon: {err}"))?
    };
    let data_url = hbitmap_to_png_data_url(bitmap)?;

    if let Ok(mut cache) = app_icon_cache().lock() {
        cache.insert(cache_key, data_url.clone());
    }

    Ok(data_url)
}

#[tauri::command]
pub async fn list_installed_apps() -> Result<Vec<InstalledApp>, String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(discover_installed_apps)
            .await
            .map_err(|err| format!("Installed app discovery failed: {err}"))?
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Installed app search is only supported on Windows.".to_string())
    }
}

#[tauri::command]
pub fn launch_installed_app(
    app: tauri::AppHandle,
    launch_target: String,
    reveal_path: Option<String>,
    is_file_system: bool,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        if is_file_system {
            let target = reveal_path.unwrap_or(launch_target);
            let target_path = PathBuf::from(&target);
            if !target_path.exists() {
                return Err("Installed app path no longer exists on disk.".to_string());
            }
            app.opener()
                .open_path(target, None::<&str>)
                .map_err(|err| format!("Failed to launch installed app: {err}"))?;
            return Ok(());
        }

        Command::new("explorer.exe")
            .arg(format!(r"shell:AppsFolder\{launch_target}"))
            .spawn()
            .map_err(|err| format!("Failed to launch installed app: {err}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, launch_target, reveal_path, is_file_system);
        Err("Installed app launch is only supported on Windows.".to_string())
    }
}

#[tauri::command]
pub fn reveal_installed_app(
    app: tauri::AppHandle,
    reveal_path: Option<String>,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let target = reveal_path
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "This installed app does not expose a file location.".to_string())?;
        let target_path = PathBuf::from(&target);
        if !target_path.exists() {
            return Err("Installed app location no longer exists on disk.".to_string());
        }
        app.opener()
            .reveal_item_in_dir(&target_path)
            .map_err(|err| format!("Failed to reveal the installed app location: {err}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, reveal_path);
        Err("Installed app location reveal is only supported on Windows.".to_string())
    }
}

#[tauri::command]
pub fn load_installed_app_icon_data_url(
    launch_target: String,
    reveal_path: Option<String>,
    is_file_system: bool,
) -> Result<String, String> {
    #[cfg(windows)]
    {
        load_shell_icon_data_url(&launch_target, reveal_path.as_deref(), is_file_system)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (launch_target, reveal_path, is_file_system);
        Err("Installed app icons are only supported on Windows.".to_string())
    }
}
