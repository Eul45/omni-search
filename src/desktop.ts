import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type DesktopSettings = {
  backgroundModeEnabled: boolean;
  shortcutEnabled: boolean;
  shortcut: string;
};

export type WindowMode = "full" | "quick";

type WindowModePayload = {
  mode?: string;
};

export const DEFAULT_DESKTOP_SETTINGS: DesktopSettings = {
  backgroundModeEnabled: true,
  shortcutEnabled: true,
  shortcut: "Alt+Shift+S",
};

const WINDOW_MODE_EVENT = "omni-search://window-mode";

function normalizeDesktopSettings(
  settings?: Partial<DesktopSettings> | null,
): DesktopSettings {
  return {
    backgroundModeEnabled: Boolean(settings?.backgroundModeEnabled),
    shortcutEnabled: Boolean(settings?.shortcutEnabled),
    shortcut: settings?.shortcut?.trim() || DEFAULT_DESKTOP_SETTINGS.shortcut,
  };
}

export async function getDesktopSettings(): Promise<DesktopSettings> {
  const settings = await invoke<DesktopSettings>("get_desktop_settings");
  return normalizeDesktopSettings(settings);
}

export async function updateDesktopSettings(
  settings: DesktopSettings,
): Promise<DesktopSettings> {
  const saved = await invoke<DesktopSettings>("update_desktop_settings", {
    backgroundModeEnabled: settings.backgroundModeEnabled,
    background_mode_enabled: settings.backgroundModeEnabled,
    shortcutEnabled: settings.shortcutEnabled,
    shortcut_enabled: settings.shortcutEnabled,
    shortcut: settings.shortcut,
  });
  return normalizeDesktopSettings(saved);
}

export async function openFullWindow(): Promise<void> {
  await invoke("open_full_window_command");
}

export async function openQuickWindow(): Promise<void> {
  await invoke("open_quick_window_command");
}

export async function syncNativeWindowTheme(
  themeMode: "dark" | "light",
  backgroundColor?: string,
  titleBarColor?: string,
  titleBarTextColor?: string,
): Promise<void> {
  await invoke("sync_window_theme_command", {
    themeMode,
    theme_mode: themeMode,
    backgroundColor,
    background_color: backgroundColor,
    titleBarColor,
    title_bar_color: titleBarColor,
    titleBarTextColor,
    title_bar_text_color: titleBarTextColor,
  });
}

export async function listenForWindowMode(
  onModeChange: (mode: WindowMode) => void,
): Promise<() => void> {
  return listen<WindowModePayload>(WINDOW_MODE_EVENT, (event) => {
    onModeChange(event.payload?.mode === "quick" ? "quick" : "full");
  });
}
