use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vkey: u32,
}

impl HotkeyConfig {
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut vkey = 0;

        let parts: Vec<String> = s.split('+').map(|p| p.trim().to_lowercase()).collect();
        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            match part.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "win" | "meta" => win = true,
                key if is_last => vkey = key_to_vkey(key)?,
                _ => {}
            }
        }
        (vkey != 0).then_some(Self {
            ctrl,
            alt,
            shift,
            win,
            vkey,
        })
    }

    pub fn modifiers(&self) -> HOT_KEY_MODIFIERS {
        let mut modifiers = HOT_KEY_MODIFIERS(0);
        if self.alt {
            modifiers |= MOD_ALT;
        }
        if self.ctrl {
            modifiers |= MOD_CONTROL;
        }
        if self.shift {
            modifiers |= MOD_SHIFT;
        }
        if self.win {
            modifiers |= MOD_WIN;
        }
        modifiers | MOD_NOREPEAT
    }

    fn has_modifier(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
}

pub fn validate_hotkey(config_str: &str, current_config: &str) -> Result<(), &'static str> {
    validate_hotkey_unique(config_str, current_config, None)
}

pub fn validate_hotkey_unique(
    config_str: &str,
    current_config: &str,
    other_config: Option<&str>,
) -> Result<(), &'static str> {
    let Some(cfg) = HotkeyConfig::parse(config_str) else {
        return Err("Press a supported key with Ctrl, Alt, Shift, or Win.");
    };
    if !cfg.has_modifier() {
        return Err("Use Ctrl, Alt, Shift, or Win with the key.");
    }
    if other_config.is_some_and(|other| same_hotkey(config_str, other)) {
        return Err("This hotkey is already assigned to another launcher action.");
    }
    if same_hotkey(config_str, current_config) || hotkey_available(config_str) {
        Ok(())
    } else {
        Err("That hotkey is already used by another app.")
    }
}

pub fn register_hotkey(hwnd: HWND, id: i32, config_str: &str) -> bool {
    unsafe {
        let _ = UnregisterHotKey(hwnd, id);
        let Some(cfg) = HotkeyConfig::parse(config_str) else {
            return false;
        };
        RegisterHotKey(hwnd, id, cfg.modifiers(), cfg.vkey).is_ok()
    }
}

pub fn hotkey_available(config_str: &str) -> bool {
    unsafe {
        let Some(cfg) = HotkeyConfig::parse(config_str) else {
            return false;
        };
        if RegisterHotKey(HWND(std::ptr::null_mut()), 9999, cfg.modifiers(), cfg.vkey).is_err() {
            return false;
        }
        let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), 9999);
        true
    }
}

/// The real Win32 modifiers. `Tab` is offered in the dropdowns as a normal key, not a modifier.
pub const MODIFIER_NAMES: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

/// Every non-modifier key offered in the dropdowns. Must stay in sync with `key_to_vkey`.
pub fn dropdown_keys() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for c in b'A'..=b'Z' {
        v.push((c as char).to_string());
    }
    for c in b'0'..=b'9' {
        v.push((c as char).to_string());
    }
    for n in 1..=12 {
        v.push(format!("F{n}"));
    }
    for k in [
        "Space",
        "Enter",
        "Esc",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Up",
        "Down",
        "Left",
        "Right",
    ] {
        v.push(k.to_string());
    }
    v
}

/// Models for the four dropdowns: (slot1, slot2, slot3-and-4).
/// Slot1 = modifiers + Tab. Slot2 = those plus every key. Slots 3/4 = keys only, leading "—" (none).
pub fn slot_models() -> (Vec<String>, Vec<String>, Vec<String>) {
    let slot1: Vec<String> = MODIFIER_NAMES
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once("Tab".to_string()))
        .collect();
    let keys = dropdown_keys();
    let mut slot2 = slot1.clone();
    slot2.extend(keys.clone());
    let mut slot34 = vec!["—".to_string()];
    slot34.extend(keys);
    (slot1, slot2, slot34)
}

/// Assemble + count only (no availability probe) — deterministic, unit-tested.
fn assemble_combo(slots: &[&str]) -> Result<String, String> {
    let mut mods: Vec<&str> = Vec::new();
    let mut keys: Vec<&str> = Vec::new();
    for &s in slots {
        let s = s.trim();
        if s.is_empty() || s == "—" {
            continue;
        }
        if MODIFIER_NAMES.contains(&s) {
            if !mods.contains(&s) {
                mods.push(s);
            }
        } else {
            keys.push(s);
        }
    }
    if mods.is_empty() {
        return Err("Pick at least one of Ctrl, Alt, Shift or Win.".to_string());
    }
    if keys.len() != 1 {
        return Err("Pick exactly one key (e.g. K, Space or Tab).".to_string());
    }
    // Canonical order: Ctrl, Alt, Shift, Win, then the key.
    let mut parts: Vec<&str> = MODIFIER_NAMES
        .iter()
        .copied()
        .filter(|m| mods.contains(m))
        .collect();
    parts.push(keys[0]);
    Ok(parts.join("+"))
}

/// Assemble a hotkey from the 4 dropdown values (empty/"—" = unused) and validate it is
/// registrable and free. `current_config` (the saved hotkey) is always treated as available.
pub fn assemble_hotkey(
    slots: &[&str],
    current_config: &str,
    other_config: Option<&str>,
) -> Result<String, String> {
    let combo = assemble_combo(slots)?;
    validate_hotkey_unique(&combo, current_config, other_config)
        .map(|_| combo.clone())
        .map_err(|e| e.to_string())
}

/// Split a saved hotkey string back into the four dropdown slot values (padded with "—").
pub fn hotkey_to_slots(s: &str) -> [String; 4] {
    let parts: Vec<String> = s
        .split('+')
        .map(|p| canon_part(p.trim()))
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return [
            "Ctrl".to_string(),
            "Space".to_string(),
            "—".to_string(),
            "—".to_string(),
        ];
    }
    let mut slots = [
        "—".to_string(),
        "—".to_string(),
        "—".to_string(),
        "—".to_string(),
    ];
    for (i, p) in parts.into_iter().take(4).enumerate() {
        slots[i] = p;
    }
    slots
}

/// Normalize a hotkey part to exactly match a dropdown model entry (e.g. "ctrl"→"Ctrl", "k"→"K").
fn canon_part(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    let low = p.to_lowercase();
    match low.as_str() {
        "ctrl" | "control" => "Ctrl".to_string(),
        "alt" => "Alt".to_string(),
        "shift" => "Shift".to_string(),
        "win" | "meta" => "Win".to_string(),
        "tab" => "Tab".to_string(),
        "space" => "Space".to_string(),
        "enter" | "return" => "Enter".to_string(),
        "esc" | "escape" => "Esc".to_string(),
        _ if p.len() == 1 => p.to_ascii_uppercase(),
        _ if low.starts_with('f')
            && low[1..]
                .parse::<u32>()
                .map(|n| (1..=24).contains(&n))
                .unwrap_or(false) =>
        {
            format!("F{}", &low[1..])
        }
        _ => p.to_string(),
    }
}

pub fn same_hotkey(left: &str, right: &str) -> bool {
    HotkeyConfig::parse(left) == HotkeyConfig::parse(right)
}

fn key_to_vkey(key: &str) -> Option<u32> {
    Some(match key {
        "space" => VK_SPACE.0 as u32,
        "enter" | "return" => VK_RETURN.0 as u32,
        "esc" | "escape" => VK_ESCAPE.0 as u32,
        "tab" => VK_TAB.0 as u32,
        "up" => VK_UP.0 as u32,
        "down" => VK_DOWN.0 as u32,
        "left" => VK_LEFT.0 as u32,
        "right" => VK_RIGHT.0 as u32,
        "backspace" => VK_BACK.0 as u32,
        "delete" => VK_DELETE.0 as u32,
        "insert" => VK_INSERT.0 as u32,
        "home" => VK_HOME.0 as u32,
        "end" => VK_END.0 as u32,
        "pageup" => VK_PRIOR.0 as u32,
        "pagedown" => VK_NEXT.0 as u32,
        key if key.starts_with('f') => {
            let n = key[1..].parse::<u32>().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            VK_F1.0 as u32 + n - 1
        }
        key if key.len() == 1 => {
            let c = key.chars().next()?;
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase() as u32
            } else if c.is_ascii_digit() {
                c as u32
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_launcher_hotkeys() {
        let alt_space = HotkeyConfig::parse("Alt+Space").unwrap();
        assert!(alt_space.alt);
        assert!(!alt_space.ctrl);
        assert_eq!(alt_space.vkey, VK_SPACE.0 as u32);

        let ctrl_shift_f12 = HotkeyConfig::parse("Ctrl + Shift + F12").unwrap();
        assert!(ctrl_shift_f12.ctrl);
        assert!(ctrl_shift_f12.shift);
        assert_eq!(ctrl_shift_f12.vkey, VK_F12.0 as u32);
    }

    #[test]
    fn builds_register_hotkey_modifiers() {
        let cfg = HotkeyConfig::parse("Ctrl+Alt+K").unwrap();
        let modifiers = cfg.modifiers();
        assert_ne!(modifiers & MOD_CONTROL, HOT_KEY_MODIFIERS(0));
        assert_ne!(modifiers & MOD_ALT, HOT_KEY_MODIFIERS(0));
        assert_ne!(modifiers & MOD_NOREPEAT, HOT_KEY_MODIFIERS(0));
        assert_eq!(cfg.vkey, b'K' as u32);
    }

    #[test]
    fn assembles_combo_from_dropdown_slots() {
        // modifier + key
        assert_eq!(
            assemble_combo(&["Alt", "Space", "—", "—"]).unwrap(),
            "Alt+Space"
        );
        // modifiers reorder into canonical Ctrl+Alt+Shift+Win order regardless of slot order
        assert_eq!(
            assemble_combo(&["Shift", "Ctrl", "K", "—"]).unwrap(),
            "Ctrl+Shift+K"
        );
        // Tab counts as the key, not a modifier
        assert_eq!(
            assemble_combo(&["Ctrl", "Tab", "—", "—"]).unwrap(),
            "Ctrl+Tab"
        );
        // duplicate modifier is collapsed
        assert_eq!(
            assemble_combo(&["Ctrl", "Ctrl", "K", "—"]).unwrap(),
            "Ctrl+K"
        );
        // no modifier (Tab alone is not one)
        assert!(assemble_combo(&["Tab", "Space", "—", "—"]).is_err());
        // two normal keys
        assert!(assemble_combo(&["Ctrl", "K", "M", "—"]).is_err());
        // no key at all
        assert!(assemble_combo(&["Ctrl", "Shift", "—", "—"]).is_err());
    }

    #[test]
    fn round_trips_saved_hotkey_into_slots() {
        assert_eq!(
            hotkey_to_slots("Ctrl+Shift+K"),
            ["Ctrl", "Shift", "K", "—"].map(String::from)
        );
        assert_eq!(
            hotkey_to_slots("Alt+Space"),
            ["Alt", "Space", "—", "—"].map(String::from)
        );
    }

    #[test]
    fn every_dropdown_key_parses() {
        // a key offered in a dropdown but not understood by key_to_vkey would be silently unselectable
        for k in dropdown_keys() {
            assert!(
                assemble_combo(&["Ctrl", &k, "—", "—"]).is_ok(),
                "dropdown key {k} does not parse"
            );
        }
    }

    #[test]
    fn validates_launcher_hotkeys_without_broken_states() {
        assert!(validate_hotkey("Alt+Space", "Alt+Space").is_ok());
        assert!(validate_hotkey("K", "Alt+Space").is_err());
        assert!(validate_hotkey("Nope", "Alt+Space").is_err());
        assert!(
            validate_hotkey_unique("Ctrl+Shift+Space", "Alt+Space", Some("Ctrl+Shift+Space"))
                .is_err()
        );
    }
}
