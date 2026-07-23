use std::path::PathBuf;

pub const STARTUP_RUN_VALUE_NAME: &str = "omnisearch-lite";
const LEGACY_STARTUP_RUN_VALUE_NAMES: &[&str] = &["OpenSearchOS", "omnisearch", "protonsearch"];

pub fn set_run_on_startup(enable: bool) {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_str) = exe_path.to_str() {
            set_registry_startup(enable, exe_str);
        }
    }
}

pub fn sync_run_on_startup(enable: bool) {
    set_run_on_startup(enable);
    cleanup_legacy_startup_entries();
}

fn set_registry_startup(enable: bool, exe_path: &str) {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    if let Ok((key, _)) = hkcu.create_subkey(path) {
        if enable {
            let _ = key.set_value(STARTUP_RUN_VALUE_NAME, &format_startup_command(exe_path));
        } else {
            let _ = key.delete_value(STARTUP_RUN_VALUE_NAME);
        }
        for legacy_name in LEGACY_STARTUP_RUN_VALUE_NAMES {
            let _ = key.delete_value(*legacy_name);
        }
    }
}

pub fn cleanup_legacy_startup_entries() {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_SET_VALUE) {
        for name in LEGACY_STARTUP_RUN_VALUE_NAMES {
            let _ = key.delete_value(*name);
        }
    }

    for shortcut in legacy_startup_shortcut_paths() {
        let _ = std::fs::remove_file(shortcut);
    }
}

fn format_startup_command(exe_path: &str) -> String {
    if exe_path.starts_with('"') {
        exe_path.to_string()
    } else {
        format!("\"{}\"", exe_path)
    }
}

fn legacy_startup_shortcut_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
                .join("OpenSearch OS.lnk"),
        );
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_paths_with_spaces() {
        assert_eq!(
            format_startup_command(r"C:\Program Files\OmniSearchLite\OmniSearchLite.exe"),
            r#""C:\Program Files\OmniSearchLite\OmniSearchLite.exe""#
        );
        assert_eq!(
            format_startup_command(r#""C:\Program Files\OmniSearchLite\OmniSearchLite.exe""#),
            r#""C:\Program Files\OmniSearchLite\OmniSearchLite.exe""#
        );
    }
}
