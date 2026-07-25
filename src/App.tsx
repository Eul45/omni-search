import { useEffect, useMemo, useRef, useState } from "react";
import type {
  CSSProperties,
  DragEvent as ReactDragEvent,
  MouseEvent as ReactMouseEvent,
  ReactNode,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { BundleType, getBundleType, getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import { flushSync } from "react-dom";
import "./App.css";
import {
  DEFAULT_DESKTOP_SETTINGS,
  getDesktopSettings,
  listenForWindowMode,
  openFullWindow,
  openQuickWindow,
  resetWindowLayout,
  syncNativeWindowTheme,
  updateDesktopSettings,
} from "./desktop";
import type { DesktopSettings, WindowMode } from "./desktop";

const POLL_INTERVAL_MS = 700;
const SEARCH_DEBOUNCE_MS = 130;
const FILTER_SEARCH_DEBOUNCE_MS = 320;
const CONTENT_SEARCH_DEBOUNCE_MS = 560;
const SEARCH_LIMIT = 200;
const SEARCH_LIMIT_MIN = SEARCH_LIMIT;
const SEARCH_LIMIT_MAX = 5000;
const PREVIEW_DATA_URL_LIMIT = 1;
const DUPLICATE_CANCEL_MESSAGE = "Duplicate scan cancelled.";
const DUPLICATE_NOTICE_TIMEOUT_MS = 2400;
const ACTION_NOTICE_TIMEOUT_MS = 1800;
const FULL_SEARCH_COMPACT_WIDTH = 760;
const FULL_SEARCH_COMPACT_HEIGHT = 560;
const QUICK_SEARCH_COMPACT_WIDTH = 1060;
const QUICK_SEARCH_COMPACT_HEIGHT = 660;
const QUICK_RESULTS_STACK_BREAKPOINT = 980;
const QUICK_RESULTS_SPLITTER_WIDTH = 16;
const QUICK_RESULTS_PANE_MIN_WIDTH = 320;
const QUICK_PREVIEW_PANE_MIN_WIDTH = 360;
const QUICK_RESULTS_PANE_DEFAULT_RATIO = 0.42;
const QUICK_LOOK_COPY_FEEDBACK_MS = 1400;
const TEXT_PREVIEW_MAX_CHARS = 3200;
const CONTENT_SNIPPET_MAX_CHARS = 220;
const CONTENT_SNIPPET_RESULT_LIMIT = 24;
const TEXT_PREVIEW_COMMAND_TIMEOUT_MS = 2800;
const UPDATE_SKIP_STORAGE_KEY = "omnisearch_skipped_update_version";
const UPDATE_CHECK_DELAY_MS = 1600;

type TauriUpdate = NonNullable<Awaited<ReturnType<typeof check>>>;

type AppUpdateInfo = {
  update: TauriUpdate;
  version: string;
  currentVersion: string;
  date?: string;
  body?: string;
};

type UpdateProgressState = {
  stage: "idle" | "downloading" | "installing" | "relaunching" | "error";
  downloaded: number;
  total: number | null;
  percent: number;
  message: string;
  error?: string;
};

type TauriDownloadEvent =
  | { event: "Started"; data?: { contentLength?: number } }
  | { event: "Progress"; data?: { chunkLength?: number } }
  | { event: "Finished"; data?: unknown };

type IndexStatus = {
  indexing: boolean;
  ready: boolean;
  indexedCount: number;
  lastError?: string | null;
};

type ContentSearchMode = "auto" | "ansi" | "utf8" | "utf16" | "utf16be";

type SearchResult = {
  resultKind: "file" | "app";
  name: string;
  path: string;
  extension: string;
  size: number;
  createdUnix: number;
  modifiedUnix: number;
  isDirectory: boolean;
  appId?: string;
  launchTarget?: string;
  revealPath?: string | null;
  isFileSystemApp?: boolean;
  iconDataUrl?: string | null;
  locationText?: string;
  locationLabel?: string;
};

type TextPreviewPayload = {
  text: string;
  truncated: boolean;
  matched: boolean;
};

type InstalledApp = {
  id: string;
  name: string;
  launchTarget: string;
  isFileSystem: boolean;
  revealPath?: string | null;
  displayPath: string;
  size: number;
  createdUnix: number;
  modifiedUnix: number;
};

type ResultIconKind = "folder" | "app" | "image" | "video" | "pdf" | "archive" | "doc" | "file";

type DuplicateFile = {
  name: string;
  path: string;
  size: number;
  createdUnix: number;
  modifiedUnix: number;
};

type DuplicateGroup = {
  groupId: string;
  size: number;
  totalBytes: number;
  fileCount: number;
  files: DuplicateFile[];
};

type DuplicateScanStatus = {
  running: boolean;
  cancelRequested: boolean;
  scannedFiles: number;
  totalFiles: number;
  groupsFound: number;
  progressPercent: number;
};

type DuplicateDeleteCandidate = {
  groupId: string;
  path: string;
  name: string;
  size: number;
};

type SearchResultContextMenuState = {
  x: number;
  y: number;
  rowKey: string;
  result: SearchResult;
};

type SearchResultRenameDraft = {
  rowKey: string;
  path: string;
  currentName: string;
  nextName: string;
  isDirectory: boolean;
};

type SearchResultDeleteCandidate = {
  rowKey: string;
  path: string;
  name: string;
  isDirectory: boolean;
};

type DriveInfo = {
  letter: string;
  path: string;
  filesystem: string;
  driveType: string;
  isNtfs: boolean;
  canOpenVolume: boolean;
};

type SocialIconName = "github" | "linkedin" | "telegram";

type SocialLink = {
  label: string;
  url: string;
  icon: SocialIconName;
};

type DeveloperApp = {
  name: string;
  url: string;
  blurb: string;
  iconSrc: string;
  accent: "sky" | "mint" | "amber";
};
type SearchSyntaxHelpSection = {
  id: string;
  label: string;
  title: string;
  summary: string;
  syntax?: string;
  details: string[];
  examples: string[];
  notes?: string[];
};
type RecentActivityQueryEntry = {
  kind: "query";
  query: string;
  usedAt: number;
  pinned: boolean;
};
type RecentActivityItemEntry = SearchResult & {
  kind: "item";
  usedAt: number;
  pinned: boolean;
};
type RecentActivityEntry = RecentActivityQueryEntry | RecentActivityItemEntry;

type PendingPairingApproval = {
  deviceId: string;
  deviceName: string;
  peerAddress: string;
};

type MobileTransferSnapshot = {
  id: string;
  name: string;
  path: string;
  size: number;
  bytesSent: number;
  status: "queued" | "sending" | "completed" | "failed";
  error?: string | null;
};

type DesktopSendToPhoneResult = {
  queued: number;
  failed: number;
  messages: string[];
};

type SyncServerInfo = {
  running: boolean;
  address: string;
  port: number;
  qrSvg: string;
  connectedClients: number;
  pairingUri: string;
  pendingApprovals: PendingPairingApproval[];
  fileTransfers: MobileTransferSnapshot[];
};

type ActiveTab = "search" | "duplicates" | "advanced" | "themes" | "syntax" | "about" | "sync";
type ResultViewTab = "all" | "apps" | "media" | "docs" | "archives";
type ResultSortMode = "relevance" | "newest" | "largest" | "name";
type ThemeMode = "dark" | "light";
type PreviewKind = "image" | "video" | "pdf" | "text" | "none";
const THEME_PRESET_IDS = [
  "slate-glass",
  "slate",
  "modern",
  "metro",
  "nordic",
  "aurora",
  "ember",
  "cedar",
  "solar",
] as const;
type ThemePresetId = (typeof THEME_PRESET_IDS)[number];
const DEFAULT_THEME_PRESET: ThemePresetId = "slate-glass";
type ThemeVariableSet = Record<string, string>;
type ThemePreviewSwatch = {
  bg: string;
  panel: string;
  panelAlt: string;
  accent: string;
  text: string;
  muted: string;
  glow: string;
};
type ThemePreset = {
  id: ThemePresetId;
  label: string;
  description: string;
  dark: ThemeVariableSet;
  light: ThemeVariableSet;
  preview: {
    dark: ThemePreviewSwatch;
    light: ThemePreviewSwatch;
  };
};

const DEVELOPER_NAME = "Eyuel Engida";
const GITHUB_PROFILE_URL = "https://github.com/Eul45";
const GITHUB_SPONSOR_URL = "https://github.com/sponsors/Eul45";
const DONATE_URL = "http://buymeacoffee.com/eyuelengida";
const THEME_STORAGE_KEY = "omnisearch_theme_mode";
const THEME_PRESET_STORAGE_KEY = "omnisearch_theme_preset";
const PREVIEW_STORAGE_KEY = "omnisearch_show_previews";
const SEARCH_METRICS_HIDDEN_STORAGE_KEY = "omnisearch_search_metrics_hidden";
const QUICK_RESULTS_PANE_RATIO_STORAGE_KEY = "omnisearch_quick_results_pane_ratio";
const INCLUDE_FOLDERS_STORAGE_KEY = "omnisearch_include_folders";
const INCLUDE_ALL_DRIVES_STORAGE_KEY = "omnisearch_include_all_drives";
const INCLUDE_INSTALLED_APPS_STORAGE_KEY = "omnisearch_include_installed_apps";
const SEARCH_LIMIT_STORAGE_KEY = "omnisearch_search_limit";
const RECENT_ACTIVITY_STORAGE_KEY = "omnisearch_recent_activity";
const RECENT_ACTIVITY_ENABLED_STORAGE_KEY = "omnisearch_recent_activity_enabled";
const RECENT_ACTIVITY_LIMIT = 5;
const THEME_PRESETS: ThemePreset[] = [
  {
    id: "aurora",
    label: "Aurora",
    description: "The blue-green OmniSearch look.",
    dark: {
      "--bg-deep": "#07090f",
      "--bg-mid": "#0d1322",
      "--panel": "rgba(13, 18, 33, 0.82)",
      "--panel-border": "rgba(124, 141, 182, 0.28)",
      "--text-main": "#edf2ff",
      "--text-muted": "#9ea8c4",
      "--accent": "#59d8a1",
      "--danger": "#ff7575",
      "--body-glow-a": "rgba(41, 116, 255, 0.22)",
      "--body-glow-b": "rgba(86, 212, 153, 0.15)",
      "--panel-shadow": "0 28px 76px rgba(0, 0, 0, 0.5)",
      "--surface-elevated": "rgba(9, 13, 26, 0.78)",
      "--surface-strong": "rgba(12, 17, 30, 0.9)",
      "--surface-input": "rgba(8, 11, 21, 0.92)",
      "--surface-muted": "rgba(8, 10, 18, 0.78)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(11, 15, 27, 0.9), rgba(9, 13, 22, 0.78))",
      "--border-soft": "rgba(136, 151, 183, 0.32)",
      "--border-strong": "rgba(124, 141, 182, 0.38)",
      "--text-soft-contrast": "#b7c8ea",
      "--highlight-bg": "rgba(89, 216, 161, 0.22)",
      "--highlight-text": "#d8fff1",
      "--result-hover": "rgba(77, 143, 224, 0.1)",
    },
    light: {
      "--bg-deep": "#eef4ff",
      "--bg-mid": "#d8e6fb",
      "--panel": "rgba(248, 252, 255, 0.93)",
      "--panel-border": "rgba(124, 141, 182, 0.44)",
      "--text-main": "#1b2c47",
      "--text-muted": "#587096",
      "--accent": "#2f9d73",
      "--danger": "#b53d3d",
      "--body-glow-a": "rgba(58, 124, 227, 0.24)",
      "--body-glow-b": "rgba(71, 190, 150, 0.2)",
      "--panel-shadow": "0 24px 58px rgba(78, 108, 156, 0.24)",
      "--surface-elevated": "rgba(228, 236, 252, 0.84)",
      "--surface-strong": "rgba(241, 246, 255, 0.95)",
      "--surface-input": "rgba(246, 250, 255, 0.97)",
      "--surface-muted": "rgba(236, 243, 255, 0.9)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(228, 236, 252, 0.95), rgba(236, 243, 255, 0.88))",
      "--border-soft": "rgba(114, 139, 185, 0.34)",
      "--border-strong": "rgba(104, 129, 177, 0.42)",
      "--text-soft-contrast": "#38547c",
      "--highlight-bg": "rgba(57, 151, 115, 0.2)",
      "--highlight-text": "#123a2e",
      "--result-hover": "rgba(91, 135, 214, 0.12)",
    },
    preview: {
      dark: {
        bg: "#0b1120",
        panel: "#172137",
        panelAlt: "#11192c",
        accent: "#59d8a1",
        text: "#edf2ff",
        muted: "#90a5c8",
        glow: "rgba(89, 216, 161, 0.24)",
      },
      light: {
        bg: "#eaf3ff",
        panel: "#f8fbff",
        panelAlt: "#dbe8f8",
        accent: "#2f9d73",
        text: "#1b2c47",
        muted: "#5f789b",
        glow: "rgba(47, 157, 115, 0.18)",
      },
    },
  },
  {
    id: "nordic",
    label: "Nordic Ink",
    description: "Deep navy with cool aqua edges.",
    dark: {
      "--bg-deep": "#0b1620",
      "--bg-mid": "#162635",
      "--panel": "rgba(17, 31, 45, 0.84)",
      "--panel-border": "rgba(121, 160, 177, 0.28)",
      "--text-main": "#ebf7ff",
      "--text-muted": "#97b4c4",
      "--accent": "#56c7ce",
      "--danger": "#ff8a88",
      "--body-glow-a": "rgba(59, 132, 180, 0.2)",
      "--body-glow-b": "rgba(86, 199, 187, 0.18)",
      "--panel-shadow": "0 28px 76px rgba(0, 0, 0, 0.46)",
      "--surface-elevated": "rgba(12, 24, 36, 0.78)",
      "--surface-strong": "rgba(16, 30, 43, 0.91)",
      "--surface-input": "rgba(10, 21, 32, 0.94)",
      "--surface-muted": "rgba(9, 20, 30, 0.82)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(15, 30, 43, 0.92), rgba(10, 20, 30, 0.8))",
      "--border-soft": "rgba(120, 154, 169, 0.3)",
      "--border-strong": "rgba(112, 160, 176, 0.38)",
      "--text-soft-contrast": "#c1e7ef",
      "--highlight-bg": "rgba(86, 199, 206, 0.22)",
      "--highlight-text": "#d8ffff",
      "--result-hover": "rgba(79, 172, 188, 0.11)",
    },
    light: {
      "--bg-deep": "#eef7fb",
      "--bg-mid": "#d9ebf3",
      "--panel": "rgba(248, 252, 255, 0.94)",
      "--panel-border": "rgba(111, 145, 162, 0.36)",
      "--text-main": "#173141",
      "--text-muted": "#567387",
      "--accent": "#2c99a6",
      "--danger": "#b84a4a",
      "--body-glow-a": "rgba(86, 160, 194, 0.22)",
      "--body-glow-b": "rgba(93, 197, 184, 0.18)",
      "--panel-shadow": "0 24px 58px rgba(73, 106, 124, 0.2)",
      "--surface-elevated": "rgba(229, 241, 247, 0.88)",
      "--surface-strong": "rgba(243, 249, 252, 0.96)",
      "--surface-input": "rgba(248, 252, 254, 0.97)",
      "--surface-muted": "rgba(235, 245, 249, 0.92)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(230, 240, 246, 0.96), rgba(238, 247, 251, 0.9))",
      "--border-soft": "rgba(108, 142, 156, 0.28)",
      "--border-strong": "rgba(100, 145, 162, 0.36)",
      "--text-soft-contrast": "#355f76",
      "--highlight-bg": "rgba(44, 153, 166, 0.18)",
      "--highlight-text": "#103640",
      "--result-hover": "rgba(74, 158, 177, 0.12)",
    },
    preview: {
      dark: {
        bg: "#11202d",
        panel: "#1a3043",
        panelAlt: "#152537",
        accent: "#56c7ce",
        text: "#ecf8ff",
        muted: "#8faab8",
        glow: "rgba(86, 199, 206, 0.24)",
      },
      light: {
        bg: "#eff8fb",
        panel: "#f9fdff",
        panelAlt: "#d8eaf1",
        accent: "#2c99a6",
        text: "#173141",
        muted: "#567387",
        glow: "rgba(44, 153, 166, 0.18)",
      },
    },
  },
  {
    id: "slate",
    label: "Win Slate",
    description: "A calm Windows 11-style steel palette.",
    dark: {
      "--bg-deep": "#171b22",
      "--bg-mid": "#262b32",
      "--panel": "rgba(31, 36, 44, 0.86)",
      "--panel-border": "rgba(136, 149, 171, 0.26)",
      "--text-main": "#f1f5fb",
      "--text-muted": "#aab4c6",
      "--accent": "#7ab3ff",
      "--danger": "#ff7c86",
      "--body-glow-a": "rgba(86, 123, 184, 0.18)",
      "--body-glow-b": "rgba(122, 179, 255, 0.16)",
      "--panel-shadow": "0 28px 76px rgba(0, 0, 0, 0.44)",
      "--surface-elevated": "rgba(28, 34, 42, 0.8)",
      "--surface-strong": "rgba(34, 40, 49, 0.92)",
      "--surface-input": "rgba(23, 28, 36, 0.95)",
      "--surface-muted": "rgba(24, 29, 37, 0.84)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(34, 40, 49, 0.92), rgba(25, 30, 38, 0.8))",
      "--border-soft": "rgba(137, 150, 170, 0.28)",
      "--border-strong": "rgba(131, 148, 173, 0.36)",
      "--text-soft-contrast": "#cdd9ee",
      "--highlight-bg": "rgba(122, 179, 255, 0.2)",
      "--highlight-text": "#eef6ff",
      "--result-hover": "rgba(122, 179, 255, 0.1)",
    },
    light: {
      "--bg-deep": "#f0f3f7",
      "--bg-mid": "#dfe5ed",
      "--panel": "rgba(250, 252, 255, 0.95)",
      "--panel-border": "rgba(126, 138, 157, 0.34)",
      "--text-main": "#1f2835",
      "--text-muted": "#657385",
      "--accent": "#3a82e6",
      "--danger": "#b44752",
      "--body-glow-a": "rgba(93, 127, 189, 0.18)",
      "--body-glow-b": "rgba(118, 171, 245, 0.16)",
      "--panel-shadow": "0 24px 58px rgba(72, 85, 107, 0.18)",
      "--surface-elevated": "rgba(232, 238, 245, 0.9)",
      "--surface-strong": "rgba(244, 247, 251, 0.96)",
      "--surface-input": "rgba(248, 250, 253, 0.98)",
      "--surface-muted": "rgba(237, 242, 247, 0.92)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(232, 237, 244, 0.96), rgba(241, 245, 250, 0.9))",
      "--border-soft": "rgba(120, 132, 150, 0.28)",
      "--border-strong": "rgba(115, 128, 147, 0.34)",
      "--text-soft-contrast": "#495c73",
      "--highlight-bg": "rgba(58, 130, 230, 0.18)",
      "--highlight-text": "#0f2448",
      "--result-hover": "rgba(87, 146, 230, 0.11)",
    },
    preview: {
      dark: {
        bg: "#1d222a",
        panel: "#2a313b",
        panelAlt: "#212831",
        accent: "#7ab3ff",
        text: "#f1f5fb",
        muted: "#aab4c6",
        glow: "rgba(122, 179, 255, 0.22)",
      },
      light: {
        bg: "#eff3f7",
        panel: "#fbfcfe",
        panelAlt: "#dde4ec",
        accent: "#3a82e6",
        text: "#1f2835",
        muted: "#657385",
        glow: "rgba(58, 130, 230, 0.16)",
      },
    },
  },
  {
    id: "slate-glass",
    label: "Slate Glass",
    description: "Slate glass surfaces with a crisp Windows-inspired companion light mode.",
    dark: {
      "--bg-deep": "#0b1117",
      "--bg-mid": "#161b22",
      "--panel": "rgba(22, 27, 34, 0.8)",
      "--panel-border": "rgba(255, 255, 255, 0.08)",
      "--text-main": "#ffffff",
      "--text-muted": "rgba(255, 255, 255, 0.6)",
      "--accent": "#58a6ff",
      "--danger": "#ff7b72",
      "--body-glow-a": "rgba(88, 166, 255, 0.1)",
      "--body-glow-b": "rgba(0, 0, 0, 0)",
      "--panel-shadow": "0 8px 32px rgba(0, 0, 0, 0.45)",
      "--surface-elevated": "rgba(48, 54, 61, 0.95)",
      "--surface-strong": "rgba(33, 38, 45, 0.98)",
      "--surface-input": "rgba(255, 255, 255, 0.05)",
      "--surface-muted": "rgba(255, 255, 255, 0.03)",
      "--surface-toolbar": "rgba(22, 27, 34, 0.6)",
      "--border-soft": "rgba(255, 255, 255, 0.06)",
      "--border-strong": "rgba(255, 255, 255, 0.12)",
      "--text-soft-contrast": "rgba(255, 255, 255, 0.85)",
      "--highlight-bg": "rgba(88, 166, 255, 0.15)",
      "--highlight-text": "#ffffff",
      "--result-hover": "rgba(255, 255, 255, 0.04)",
    },
    light: {
      "--bg-deep": "#f3f3f3",
      "--bg-mid": "#eeeeee",
      "--panel": "rgba(255, 255, 255, 0.7)",
      "--panel-border": "rgba(0, 0, 0, 0.06)",
      "--text-main": "#1a1a1a",
      "--text-muted": "rgba(0, 0, 0, 0.6)",
      "--accent": "#005fb8",
      "--danger": "#c42b1c",
      "--body-glow-a": "rgba(0, 95, 184, 0.05)",
      "--body-glow-b": "rgba(255, 255, 255, 0)",
      "--panel-shadow": "0 8px 32px rgba(0, 0, 0, 0.1)",
      "--surface-elevated": "rgba(255, 255, 255, 0.85)",
      "--surface-strong": "#ffffff",
      "--surface-input": "rgba(255, 255, 255, 0.6)",
      "--surface-muted": "rgba(0, 0, 0, 0.02)",
      "--surface-toolbar": "rgba(243, 243, 243, 0.8)",
      "--border-soft": "rgba(0, 0, 0, 0.05)",
      "--border-strong": "rgba(0, 0, 0, 0.1)",
      "--text-soft-contrast": "rgba(0, 0, 0, 0.8)",
      "--highlight-bg": "rgba(0, 95, 184, 0.1)",
      "--highlight-text": "#005fb8",
      "--result-hover": "rgba(0, 0, 0, 0.03)",
    },
    preview: {
      dark: {
        bg: "#0b1117",
        panel: "#161b22",
        panelAlt: "#21262d",
        accent: "#58a6ff",
        text: "#ffffff",
        muted: "rgba(255, 255, 255, 0.6)",
        glow: "rgba(88, 166, 255, 0.18)",
      },
      light: {
        bg: "#f3f3f3",
        panel: "#ffffff",
        panelAlt: "#eeeeee",
        accent: "#005fb8",
        text: "#1a1a1a",
        muted: "rgba(0, 0, 0, 0.6)",
        glow: "rgba(0, 95, 184, 0.16)",
      },
    },
  },
  {
    id: "modern",
    label: "Modern Sleek",
    description: "Windows-style productivity neutrals with a clean blue accent.",
    dark: {
      "--bg-deep": "#1c1c1c",
      "--bg-mid": "#262626",
      "--panel": "rgba(32, 32, 32, 0.75)",
      "--panel-border": "rgba(255, 255, 255, 0.08)",
      "--text-main": "#ffffff",
      "--text-muted": "rgba(255, 255, 255, 0.6)",
      "--accent": "#60cdff",
      "--danger": "#ff99a4",
      "--body-glow-a": "rgba(96, 205, 255, 0.08)",
      "--body-glow-b": "rgba(0, 0, 0, 0)",
      "--panel-shadow": "0 8px 32px rgba(0, 0, 0, 0.4)",
      "--surface-elevated": "rgba(45, 45, 45, 0.95)",
      "--surface-strong": "rgba(50, 50, 50, 0.98)",
      "--surface-input": "rgba(255, 255, 255, 0.06)",
      "--surface-muted": "rgba(255, 255, 255, 0.04)",
      "--surface-toolbar": "rgba(32, 32, 32, 0.6)",
      "--border-soft": "rgba(255, 255, 255, 0.05)",
      "--border-strong": "rgba(255, 255, 255, 0.12)",
      "--text-soft-contrast": "rgba(255, 255, 255, 0.9)",
      "--highlight-bg": "rgba(96, 205, 255, 0.15)",
      "--highlight-text": "#ffffff",
      "--result-hover": "rgba(255, 255, 255, 0.06)",
    },

    light: {
      "--bg-deep": "#f3f3f3",
      "--bg-mid": "#fbfbfb",
      "--panel": "rgba(255, 255, 255, 0.97)",
      "--panel-border": "rgba(31, 31, 31, 0.09)",
      "--text-main": "#1f1f1f",
      "--text-muted": "#666666",
      "--accent": "#0078d4",
      "--danger": "#c04451",
      "--body-glow-a": "rgba(0, 120, 212, 0.1)",
      "--body-glow-b": "rgba(31, 31, 31, 0.03)",
      "--panel-shadow": "0 20px 44px rgba(31, 31, 31, 0.1)",
      "--surface-elevated": "rgba(245, 245, 245, 0.96)",
      "--surface-strong": "rgba(255, 255, 255, 0.99)",
      "--surface-input": "rgba(255, 255, 255, 1)",
      "--surface-muted": "rgba(248, 248, 248, 0.98)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(247, 247, 247, 0.98), rgba(255, 255, 255, 0.96))",
      "--border-soft": "rgba(31, 31, 31, 0.08)",
      "--border-strong": "rgba(31, 31, 31, 0.14)",
      "--text-soft-contrast": "#505050",
      "--highlight-bg": "rgba(0, 120, 212, 0.16)",
      "--highlight-text": "#0f3358",
      "--result-hover": "rgba(31, 31, 31, 0.04)",
    },
    preview: {
      dark: {
        bg: "#202020",
        panel: "#2c2c2c",
        panelAlt: "#343434",
        accent: "#0078d4",
        text: "#f0f0f0",
        muted: "#bcbcbc",
        glow: "rgba(0, 120, 212, 0.22)",
      },
      light: {
        bg: "#f3f3f3",
        panel: "#ffffff",
        panelAlt: "#ececec",
        accent: "#0078d4",
        text: "#1f1f1f",
        muted: "#666666",
        glow: "rgba(0, 120, 212, 0.14)",
      },
    },
  },
  {
    id: "metro",
    label: "Metro",
    description: "Sharper blue-gray surfaces with a polished Windows utility feel.",
    dark: {
      "--bg-deep": "#1c222b",
      "--bg-mid": "#283341",
      "--panel": "rgba(42, 52, 66, 0.92)",
      "--panel-border": "rgba(145, 170, 214, 0.18)",
      "--text-main": "#eef4fc",
      "--text-muted": "#aebed6",
      "--accent": "#2f89ff",
      "--danger": "#ff7b84",
      "--body-glow-a": "rgba(47, 137, 255, 0.16)",
      "--body-glow-b": "rgba(168, 199, 255, 0.08)",
      "--panel-shadow": "0 26px 72px rgba(0, 0, 0, 0.38)",
      "--surface-elevated": "rgba(35, 44, 56, 0.9)",
      "--surface-strong": "rgba(42, 52, 66, 0.97)",
      "--surface-input": "rgba(28, 36, 46, 0.98)",
      "--surface-muted": "rgba(37, 46, 58, 0.92)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(44, 54, 68, 0.96), rgba(32, 40, 51, 0.9))",
      "--border-soft": "rgba(143, 170, 211, 0.18)",
      "--border-strong": "rgba(145, 170, 214, 0.28)",
      "--text-soft-contrast": "#d8e5f8",
      "--highlight-bg": "rgba(47, 137, 255, 0.24)",
      "--highlight-text": "#f4f9ff",
      "--result-hover": "rgba(47, 137, 255, 0.08)",
    },
    light: {
      "--bg-deep": "#f3f7fb",
      "--bg-mid": "#e7eef7",
      "--panel": "rgba(255, 255, 255, 0.97)",
      "--panel-border": "rgba(103, 131, 176, 0.18)",
      "--text-main": "#1e2a3b",
      "--text-muted": "#61748c",
      "--accent": "#2f79e9",
      "--danger": "#b94752",
      "--body-glow-a": "rgba(47, 121, 233, 0.12)",
      "--body-glow-b": "rgba(95, 125, 170, 0.08)",
      "--panel-shadow": "0 22px 46px rgba(60, 83, 121, 0.12)",
      "--surface-elevated": "rgba(239, 244, 250, 0.96)",
      "--surface-strong": "rgba(255, 255, 255, 0.99)",
      "--surface-input": "rgba(255, 255, 255, 1)",
      "--surface-muted": "rgba(244, 247, 251, 0.98)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(239, 244, 250, 0.98), rgba(249, 251, 254, 0.96))",
      "--border-soft": "rgba(102, 130, 173, 0.16)",
      "--border-strong": "rgba(102, 130, 173, 0.24)",
      "--text-soft-contrast": "#4c617e",
      "--highlight-bg": "rgba(47, 121, 233, 0.16)",
      "--highlight-text": "#113766",
      "--result-hover": "rgba(47, 121, 233, 0.06)",
    },
    preview: {
      dark: {
        bg: "#1c222b",
        panel: "#2a3442",
        panelAlt: "#22303d",
        accent: "#2f89ff",
        text: "#eef4fc",
        muted: "#aebed6",
        glow: "rgba(47, 137, 255, 0.22)",
      },
      light: {
        bg: "#f3f7fb",
        panel: "#ffffff",
        panelAlt: "#e6eef7",
        accent: "#2f79e9",
        text: "#1e2a3b",
        muted: "#61748c",
        glow: "rgba(47, 121, 233, 0.16)",
      },
    },
  },
  {
    id: "ember",
    label: "Ember",
    description: "Warm copper highlights over dark charcoal.",
    dark: {
      "--bg-deep": "#160f10",
      "--bg-mid": "#2a1d21",
      "--panel": "rgba(33, 23, 27, 0.86)",
      "--panel-border": "rgba(190, 127, 94, 0.24)",
      "--text-main": "#fff1ea",
      "--text-muted": "#d2a894",
      "--accent": "#ff9b62",
      "--danger": "#ff6f78",
      "--body-glow-a": "rgba(242, 131, 82, 0.2)",
      "--body-glow-b": "rgba(255, 176, 85, 0.14)",
      "--panel-shadow": "0 28px 76px rgba(0, 0, 0, 0.46)",
      "--surface-elevated": "rgba(28, 19, 23, 0.8)",
      "--surface-strong": "rgba(36, 24, 28, 0.92)",
      "--surface-input": "rgba(23, 16, 18, 0.95)",
      "--surface-muted": "rgba(27, 17, 20, 0.84)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(35, 24, 28, 0.92), rgba(23, 16, 19, 0.8))",
      "--border-soft": "rgba(176, 122, 98, 0.28)",
      "--border-strong": "rgba(195, 130, 96, 0.36)",
      "--text-soft-contrast": "#ffd0bc",
      "--highlight-bg": "rgba(255, 155, 98, 0.22)",
      "--highlight-text": "#fff5ef",
      "--result-hover": "rgba(255, 155, 98, 0.1)",
    },
    light: {
      "--bg-deep": "#fff3eb",
      "--bg-mid": "#f7dfd2",
      "--panel": "rgba(255, 250, 247, 0.95)",
      "--panel-border": "rgba(195, 137, 105, 0.3)",
      "--text-main": "#45231d",
      "--text-muted": "#936556",
      "--accent": "#d66c37",
      "--danger": "#b33d46",
      "--body-glow-a": "rgba(229, 132, 83, 0.2)",
      "--body-glow-b": "rgba(255, 186, 91, 0.16)",
      "--panel-shadow": "0 24px 58px rgba(133, 89, 67, 0.18)",
      "--surface-elevated": "rgba(249, 233, 224, 0.9)",
      "--surface-strong": "rgba(255, 247, 242, 0.97)",
      "--surface-input": "rgba(255, 251, 248, 0.98)",
      "--surface-muted": "rgba(251, 240, 233, 0.92)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(249, 233, 224, 0.96), rgba(253, 244, 238, 0.9))",
      "--border-soft": "rgba(188, 132, 101, 0.28)",
      "--border-strong": "rgba(178, 126, 98, 0.34)",
      "--text-soft-contrast": "#8a4f3d",
      "--highlight-bg": "rgba(214, 108, 55, 0.18)",
      "--highlight-text": "#4b2418",
      "--result-hover": "rgba(214, 108, 55, 0.1)",
    },
    preview: {
      dark: {
        bg: "#1b1416",
        panel: "#332227",
        panelAlt: "#281b1f",
        accent: "#ff9b62",
        text: "#fff1ea",
        muted: "#d1a693",
        glow: "rgba(255, 155, 98, 0.24)",
      },
      light: {
        bg: "#fff4ec",
        panel: "#fffaf6",
        panelAlt: "#f4dfd2",
        accent: "#d66c37",
        text: "#45231d",
        muted: "#936556",
        glow: "rgba(214, 108, 55, 0.18)",
      },
    },
  },
  {
    id: "cedar",
    label: "Cedar",
    description: "Calm workstation green with softer contrast.",
    dark: {
      "--bg-deep": "#09110f",
      "--bg-mid": "#14241d",
      "--panel": "rgba(15, 25, 22, 0.84)",
      "--panel-border": "rgba(112, 162, 129, 0.24)",
      "--text-main": "#eefcf5",
      "--text-muted": "#96b7a6",
      "--accent": "#67d69a",
      "--danger": "#ff7d8e",
      "--body-glow-a": "rgba(70, 158, 122, 0.18)",
      "--body-glow-b": "rgba(120, 220, 171, 0.16)",
      "--panel-shadow": "0 28px 76px rgba(0, 0, 0, 0.46)",
      "--surface-elevated": "rgba(11, 20, 17, 0.8)",
      "--surface-strong": "rgba(15, 27, 22, 0.92)",
      "--surface-input": "rgba(8, 18, 14, 0.95)",
      "--surface-muted": "rgba(10, 18, 15, 0.84)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(15, 27, 22, 0.92), rgba(9, 17, 14, 0.8))",
      "--border-soft": "rgba(117, 161, 134, 0.28)",
      "--border-strong": "rgba(114, 170, 135, 0.36)",
      "--text-soft-contrast": "#c4efd9",
      "--highlight-bg": "rgba(103, 214, 154, 0.2)",
      "--highlight-text": "#effff5",
      "--result-hover": "rgba(103, 214, 154, 0.1)",
    },
    light: {
      "--bg-deep": "#eff7f1",
      "--bg-mid": "#dcebdd",
      "--panel": "rgba(249, 253, 249, 0.95)",
      "--panel-border": "rgba(108, 148, 122, 0.32)",
      "--text-main": "#1d3428",
      "--text-muted": "#5d7e6e",
      "--accent": "#31965f",
      "--danger": "#b24454",
      "--body-glow-a": "rgba(66, 155, 117, 0.18)",
      "--body-glow-b": "rgba(109, 209, 161, 0.16)",
      "--panel-shadow": "0 24px 58px rgba(80, 111, 93, 0.18)",
      "--surface-elevated": "rgba(232, 241, 232, 0.9)",
      "--surface-strong": "rgba(246, 250, 246, 0.97)",
      "--surface-input": "rgba(250, 252, 250, 0.98)",
      "--surface-muted": "rgba(239, 245, 239, 0.92)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(232, 241, 232, 0.96), rgba(241, 247, 241, 0.9))",
      "--border-soft": "rgba(111, 148, 123, 0.28)",
      "--border-strong": "rgba(104, 141, 118, 0.34)",
      "--text-soft-contrast": "#426856",
      "--highlight-bg": "rgba(49, 150, 95, 0.18)",
      "--highlight-text": "#123322",
      "--result-hover": "rgba(73, 164, 111, 0.1)",
    },
    preview: {
      dark: {
        bg: "#0f1b17",
        panel: "#193027",
        panelAlt: "#12231c",
        accent: "#67d69a",
        text: "#eefcf5",
        muted: "#96b7a6",
        glow: "rgba(103, 214, 154, 0.22)",
      },
      light: {
        bg: "#eff7f1",
        panel: "#fbfefb",
        panelAlt: "#dcebdd",
        accent: "#31965f",
        text: "#1d3428",
        muted: "#5d7e6e",
        glow: "rgba(49, 150, 95, 0.16)",
      },
    },
  },
  {
    id: "solar",
    label: "Solar Sand",
    description: "Warm sandstone tones with gold accents.",
    dark: {
      "--bg-deep": "#15120b",
      "--bg-mid": "#2a2417",
      "--panel": "rgba(32, 27, 18, 0.85)",
      "--panel-border": "rgba(180, 150, 94, 0.24)",
      "--text-main": "#fff7e6",
      "--text-muted": "#d3bf95",
      "--accent": "#e8c15a",
      "--danger": "#ff7f78",
      "--body-glow-a": "rgba(212, 164, 70, 0.18)",
      "--body-glow-b": "rgba(244, 214, 115, 0.14)",
      "--panel-shadow": "0 28px 76px rgba(0, 0, 0, 0.46)",
      "--surface-elevated": "rgba(23, 19, 12, 0.8)",
      "--surface-strong": "rgba(31, 26, 17, 0.92)",
      "--surface-input": "rgba(19, 16, 10, 0.95)",
      "--surface-muted": "rgba(22, 18, 11, 0.84)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(31, 26, 17, 0.92), rgba(20, 17, 10, 0.8))",
      "--border-soft": "rgba(173, 148, 96, 0.28)",
      "--border-strong": "rgba(187, 157, 92, 0.36)",
      "--text-soft-contrast": "#f5ddb0",
      "--highlight-bg": "rgba(232, 193, 90, 0.2)",
      "--highlight-text": "#fff8ea",
      "--result-hover": "rgba(232, 193, 90, 0.1)",
    },
    light: {
      "--bg-deep": "#fbf5e7",
      "--bg-mid": "#efe2be",
      "--panel": "rgba(255, 253, 248, 0.95)",
      "--panel-border": "rgba(172, 147, 95, 0.3)",
      "--text-main": "#44351a",
      "--text-muted": "#8c7751",
      "--accent": "#b88f1f",
      "--danger": "#b3453f",
      "--body-glow-a": "rgba(201, 160, 67, 0.18)",
      "--body-glow-b": "rgba(236, 204, 111, 0.14)",
      "--panel-shadow": "0 24px 58px rgba(121, 102, 61, 0.18)",
      "--surface-elevated": "rgba(245, 236, 213, 0.9)",
      "--surface-strong": "rgba(255, 251, 243, 0.97)",
      "--surface-input": "rgba(255, 253, 248, 0.98)",
      "--surface-muted": "rgba(248, 241, 223, 0.92)",
      "--surface-toolbar":
        "linear-gradient(180deg, rgba(245, 236, 213, 0.96), rgba(251, 246, 231, 0.9))",
      "--border-soft": "rgba(171, 145, 90, 0.28)",
      "--border-strong": "rgba(165, 140, 87, 0.34)",
      "--text-soft-contrast": "#7a6537",
      "--highlight-bg": "rgba(184, 143, 31, 0.18)",
      "--highlight-text": "#46340a",
      "--result-hover": "rgba(184, 143, 31, 0.1)",
    },
    preview: {
      dark: {
        bg: "#1c170e",
        panel: "#322919",
        panelAlt: "#251f13",
        accent: "#e8c15a",
        text: "#fff7e6",
        muted: "#d3bf95",
        glow: "rgba(232, 193, 90, 0.22)",
      },
      light: {
        bg: "#fbf4e5",
        panel: "#fffdf8",
        panelAlt: "#ede1bf",
        accent: "#b88f1f",
        text: "#44351a",
        muted: "#8c7751",
        glow: "rgba(184, 143, 31, 0.18)",
      },
    },
  },
];
const RESULT_VIEW_TABS: Array<{ id: ResultViewTab; label: string }> = [
  { id: "all", label: "All" },
  { id: "apps", label: "Apps" },
  { id: "media", label: "Media" },
  { id: "docs", label: "Docs" },
  { id: "archives", label: "Archives" },
];

type SearchSyntaxPreview = {
  pathQuery: string;
  hasContentSearch: boolean;
  contentQuery: string;
  contentMode: ContentSearchMode | null;
};

const SEARCH_SYNTAX_PATTERN =
  /(?:^|\s)(content|ansicontent|utf8content|utf16content|utf16becontent|ext):(?:"([^"]*)"|(\S+))/gi;

const SEARCH_SYNTAX_HELP_SECTIONS: SearchSyntaxHelpSection[] = [
  {
    id: "basics",
    label: "Basics",
    title: "Search Basics",
    summary: "Normal OmniSearch queries stay indexed and fast.",
    syntax: "plain words",
    details: [
      "Plain words match against the indexed file name and full path.",
      "Inline operators can be mixed into the same query.",
      "Content scanning only starts when a content operator is present.",
    ],
    examples: ["invoice", "src", "src ext:ts;tsx"],
    notes: ["Only the operators shown in this help are currently supported in OmniSearch."],
  },
  {
    id: "ext",
    label: "ext:",
    title: "Inline Extension Filter",
    syntax: "ext:txt;md",
    summary: "Filter file types directly inside the main search box.",
    details: [
      "Use a semicolon-separated list like ext:ts;tsx or ext:json;toml.",
      "This works together with the normal Extension field and metadata filters.",
      "Folder aliases like folder, folders, dir, and directory are also supported.",
    ],
    examples: ["ext:txt", "ext:ts;tsx", 'src ext:json;toml content:"localhost"'],
  },
  {
    id: "content",
    label: "content:",
    title: "Auto Text Content Search",
    syntax: 'content:"text"',
    summary: "Read matching files from disk and look for text inside them.",
    details: [
      "Use quotes for phrases with spaces.",
      "This auto-detects common text encodings and skips folders.",
      "Pair it with ext: or path words for much better speed.",
    ],
    examples: [
      "content:hello",
      'content:"quick start"',
      'ext:txt content:"invoice number"',
      'src ext:tsx content:"before"',
    ],
    notes: [
      "Broad content: searches can take longer because files must be opened and read from disk.",
    ],
  },
  {
    id: "ansi",
    label: "ANSI",
    title: "ANSI Content Search",
    syntax: 'ansicontent:"text"',
    summary: "Force ANSI decoding for older text files and exports.",
    details: [
      "Useful for older .ini, .bat, .cmd, .log, or legacy exported text files.",
      "Use this when the default content: search misses text that looks readable in older editors.",
    ],
    examples: [
      'ansicontent:"[Settings]"',
      'ext:ini ansicontent:"InstallPath"',
      'ext:log;txt ansicontent:"Access denied"',
    ],
  },
  {
    id: "utf8",
    label: "UTF-8",
    title: "UTF-8 Content Search",
    syntax: 'utf8content:"text"',
    summary: "Force UTF-8 decoding for modern code and text files.",
    details: [
      "Best for source code, JSON, Markdown, HTML, CSS, and many plain-text project files.",
      "This is useful when you want predictable UTF-8 matching during testing.",
    ],
    examples: [
      'utf8content:"useEffect"',
      'ext:json utf8content:"apiKey"',
      'src ext:ts;tsx utf8content:"startTransition"',
    ],
  },
  {
    id: "utf16",
    label: "UTF-16",
    title: "UTF-16 Content Search",
    syntax: 'utf16content:"text"',
    summary: "Force UTF-16 little-endian decoding for Windows-style text files.",
    details: [
      "Useful for some Windows-generated logs and exported text files.",
      "This targets UTF-16 little-endian content specifically.",
    ],
    examples: ['utf16content:"Event ID"', 'ext:txt utf16content:"Windows Error Reporting"'],
  },
  {
    id: "utf16be",
    label: "UTF-16 BE",
    title: "UTF-16 Big Endian Content Search",
    syntax: 'utf16becontent:"text"',
    summary: "Force UTF-16 big-endian decoding when you need exact compatibility testing.",
    details: [
      "This is less common, but it is supported for testing and special files.",
      "Use it only when auto mode or utf16content: does not match the file correctly.",
    ],
    examples: ['utf16becontent:"Title"', 'ext:txt utf16becontent:"Chapter 1"'],
  },
  {
    id: "combine",
    label: "Combine",
    title: "Combining Search Tools",
    syntax: 'path words + ext:<list> + content:"text"',
    summary: "Combine the search box syntax with the normal UI filters for tighter results.",
    details: [
      "Path words, ext:, and content operators all work together in the main query.",
      "The Extension field, size controls, and created date filters still apply too.",
      "The best pattern is usually folder/path words plus ext: plus a quoted content phrase.",
    ],
    examples: [
      'src ext:tsx content:"before"',
      'docs ext:md content:"quick start"',
      'config ext:json;toml content:"localhost"',
      'ext:log content:"connection refused"',
    ],
  },
  {
    id: "speed",
    label: "Speed",
    title: "Speed Tips",
    summary: "Content search is much faster when you narrow candidates first.",
    details: [
      "content:hello is slow because it may need to open many files from disk.",
      "Add path words, ext:, size filters, or date filters to reduce the amount of work.",
      "Changing or clearing the search box automatically cancels the previous content search.",
    ],
    examples: ['content:"license"', 'ext:txt content:"license"', 'src ext:ts;tsx content:"useEffect"'],
    notes: ["For the best speed, prefer targeted queries over broad content-only searches."],
  },
];

const APP_EXTENSIONS = new Set([
  "exe",
  "msi",
  "bat",
  "cmd",
  "com",
  "ps1",
  "lnk",
  "appx",
]);

const MEDIA_EXTENSIONS = new Set([
  "mp3",
  "wav",
  "flac",
  "aac",
  "ogg",
  "m4a",
  "mp4",
  "mkv",
  "avi",
  "mov",
  "wmv",
  "webm",
  "jpg",
  "jpeg",
  "png",
  "gif",
  "bmp",
  "webp",
]);

const DOC_EXTENSIONS = new Set([
  "pdf",
  "doc",
  "docx",
  "ppt",
  "pptx",
  "xls",
  "xlsx",
  "txt",
  "md",
  "rtf",
  "csv",
  "json",
  "xml",
  "html",
  "css",
  "js",
  "ts",
  "tsx",
  "rs",
  "cpp",
  "h",
  "py",
  "java",
]);

const ARCHIVE_EXTENSIONS = new Set(["zip", "rar", "7z", "tar", "gz", "iso"]);
const IMAGE_PREVIEW_EXTENSIONS = new Set([
  "jpg",
  "jpeg",
  "png",
  "gif",
  "bmp",
  "webp",
  "ico",
]);
const VIDEO_PREVIEW_EXTENSIONS = new Set(["mp4", "mkv", "avi", "mov", "wmv", "webm", "m4v"]);
const TEXT_PREVIEW_EXTENSIONS = new Set([
  "txt",
  "md",
  "log",
  "ini",
  "cfg",
  "conf",
  "csv",
  "json",
  "xml",
  "yaml",
  "yml",
  "toml",
  "html",
  "htm",
  "css",
  "js",
  "jsx",
  "ts",
  "tsx",
  "rs",
  "py",
  "java",
  "c",
  "cpp",
  "h",
  "hpp",
  "cs",
  "sql",
  "bat",
  "cmd",
  "ps1",
]);

const SOCIAL_LINKS: SocialLink[] = [
  { label: "GitHub", url: "https://github.com/Eul45", icon: "github" },
  {
    label: "LinkedIn",
    url: "https://www.linkedin.com/in/eyuel-engida-77155a317",
    icon: "linkedin",
  },
  { label: "Telegram", url: "https://t.me/Eul_zzz", icon: "telegram" },
];

const MORE_APPS: DeveloperApp[] = [
  {
    name: "OmniClip",
    url: "https://apps.microsoft.com/detail/9N53Z3QVL322?hl=en-us&gl=US&ocid=pdpshare",
    blurb:
      "A lightweight, searchable clipboard manager with persistent SQLite storage and global shortcuts.",
    iconSrc: "/omniclip_icon.png",
    accent: "sky",
  },
  {
    name: "EyuX AI - Workspace",
    url: "https://apps.microsoft.com/detail/9NX5DBW6NHW1?hl=en-us&gl=US&ocid=pdpshare",
    blurb: "An AI workspace focused on practical desktop productivity.",
    iconSrc: "/Eyux_icon.png",
    accent: "mint",
  },
  {
    name: "ZenCapture",
    url: "https://apps.microsoft.com/detail/9NVW8TKD5R33?hl=en-us&gl=US&ocid=pdpshare",
    blurb: "A lightweight capture tool for daily ideas with screenshots.",
    iconSrc: "/zencapture_icon.png",
    accent: "amber",
  },
];

function SocialIcon({ icon }: { icon: SocialIconName }) {
  if (icon === "github") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 .297a12 12 0 0 0-3.79 23.4c.6.113.82-.258.82-.577 0-.285-.01-1.04-.016-2.04-3.338.724-4.043-1.61-4.043-1.61-.545-1.385-1.332-1.754-1.332-1.754-1.09-.744.084-.729.084-.729 1.205.084 1.84 1.237 1.84 1.237 1.07 1.834 2.807 1.304 3.492.997.108-.775.418-1.305.762-1.605-2.665-.305-5.467-1.335-5.467-5.93 0-1.31.467-2.38 1.236-3.22-.124-.303-.536-1.523.117-3.176 0 0 1.008-.322 3.301 1.23A11.52 11.52 0 0 1 12 6.844c1.02.005 2.046.138 3.003.404 2.291-1.552 3.297-1.23 3.297-1.23.654 1.653.243 2.873.119 3.176.77.84 1.235 1.91 1.235 3.22 0 4.607-2.807 5.624-5.48 5.921.43.37.823 1.102.823 2.222 0 1.606-.014 2.898-.014 3.293 0 .322.216.694.825.576A12 12 0 0 0 12 .297Z" />
      </svg>
    );
  }

  if (icon === "linkedin") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M20.447 20.452H16.89V14.87c0-1.33-.027-3.04-1.852-3.04-1.853 0-2.136 1.445-2.136 2.94v5.682H9.34V9h3.414v1.561h.049c.476-.9 1.637-1.85 3.37-1.85 3.601 0 4.268 2.37 4.268 5.455v6.286zM5.337 7.433a2.063 2.063 0 1 1 .002-4.126 2.063 2.063 0 0 1-.002 4.126zM7.119 20.452H3.555V9h3.564v11.452zM22.225 0H1.771A1.75 1.75 0 0 0 0 1.729v20.542C0 23.227.792 24 1.771 24h20.451C23.2 24 24 23.227 24 22.271V1.729C24 .774 23.2 0 22.222 0z" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 11.944 0zm5.255 8.599c-.162 1.703-.866 5.834-1.224 7.741-.151.807-.449 1.078-.737 1.104-.625.058-1.1-.413-1.706-.81-.949-.624-1.485-1.012-2.405-1.621-1.063-.699-.374-1.083.232-1.715.159-.166 2.91-2.666 2.963-2.895.006-.028.013-.133-.05-.189s-.156-.037-.223-.022c-.095.021-1.597 1.014-4.507 2.979-.427.294-.814.437-1.161.429-.382-.008-1.117-.216-1.664-.394-.67-.218-1.203-.334-1.157-.705.024-.193.291-.391.8-.593 3.132-1.364 5.221-2.264 6.268-2.699 2.986-1.242 3.607-1.458 4.011-1.465.088-.002.285.02.413.124.108.087.138.205.152.288.014.083.031.272.017.42z" />
    </svg>
  );
}

function CalendarIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3.5" y="5.5" width="17" height="15" rx="2.5" />
      <path d="M7 3.75v3.5M17 3.75v3.5M3.5 9.5h17M8 13h3M13 13h3M8 17h3" />
    </svg>
  );
}

function GitHubSponsorIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z" />
    </svg>
  );
}

function BuyMeCoffeeIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 8.25h10.75a1.75 1.75 0 0 1 1.75 1.75v1.25a4 4 0 0 1-4 4H9a4 4 0 0 1-4-4v-3z" />
      <path d="M15.75 9h1.5a2.75 2.75 0 1 1 0 5.5H16.5" />
      <path d="M7 5.25c.5-.75 1.1-1.5 2-2m3 .5c.55-.65 1.05-1.2 1.75-1.75M7.5 19h9" />
    </svg>
  );
}

function MicrosoftStoreIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 4.3 10.5 3v8H3V4.3Zm10.5-1.55L21 1.5V11h-7.5V2.75ZM3 13h7.5v8L3 19.7V13Zm10.5 0H21v9.5L13.5 21v-8Z" />
    </svg>
  );
}

function SearchLensIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <defs>
        <linearGradient id="search-lens-stroke" x1="5" y1="4.5" x2="18.5" y2="19" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#86e8ff" />
          <stop offset="0.52" stopColor="#2b95ff" />
          <stop offset="1" stopColor="#2e63ff" />
        </linearGradient>
        <radialGradient
          id="search-lens-fill"
          cx="0"
          cy="0"
          r="1"
          gradientUnits="userSpaceOnUse"
          gradientTransform="translate(9.3 8.3) rotate(45) scale(7.8)"
        >
          <stop offset="0" stopColor="#b8ebff" stopOpacity="0.4" />
          <stop offset="0.46" stopColor="#67bfff" stopOpacity="0.14" />
          <stop offset="1" stopColor="#2e63ff" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle cx="10" cy="10" r="5.55" fill="url(#search-lens-fill)" />
      <circle
        cx="10"
        cy="10"
        r="5.55"
        fill="none"
        stroke="url(#search-lens-stroke)"
        strokeWidth="2.35"
      />
      <path
        d="M14.45 14.45L19.15 19.15"
        fill="none"
        stroke="url(#search-lens-stroke)"
        strokeLinecap="round"
        strokeWidth="2.55"
      />
    </svg>
  );
}

function PinIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="6.5" r="2.5" />
      <path d="M10.35 8.75 8.15 13h7.7l-2.2-4.25" />
      <path d="M12 13v7.25" />
    </svg>
  );
}

function SearchLayoutToggleIcon({ hidden }: { hidden: boolean }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="4.25" y="5" width="15.5" height="14" rx="3" />
      <path d="M7.5 8.75h7" />
      {hidden ? (
        <>
          <path d="M7.5 12.25h9" />
          <path d="M7.5 15.5h9" />
          <path d="m15.75 7.2 2.25 2.2 2.25-2.2" />
        </>
      ) : (
        <>
          <path d="M7.5 12.25h3.25" />
          <path d="M13.1 12.25h3.4" />
          <path d="M7.5 15.5h9" />
          <path d="m15.75 9.4 2.25-2.2 2.25 2.2" />
        </>
      )}
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="9" y="8" width="10" height="11" rx="2.25" />
      <path d="M7 15H6a2 2 0 0 1-2-2V6.75A2.75 2.75 0 0 1 6.75 4h7.5A2.75 2.75 0 0 1 17 6.75V7.5" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5.5 12.5 10 17l8.5-9" />
    </svg>
  );
}

function ResultTypeIcon({ kind, className }: { kind: ResultIconKind; className?: string }) {
  if (kind === "folder") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <path d="M3.75 8.25a2.25 2.25 0 0 1 2.25-2.25h4.1l1.65 1.85H18a2.25 2.25 0 0 1 2.25 2.25v5.9A2.25 2.25 0 0 1 18 18.25H6A2.25 2.25 0 0 1 3.75 16z" />
        <path d="M3.75 9.5h16.5" />
      </svg>
    );
  }

  if (kind === "app") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <rect x="4.25" y="4.25" width="6.25" height="6.25" rx="1.2" />
        <rect x="13.5" y="4.25" width="6.25" height="6.25" rx="1.2" />
        <rect x="4.25" y="13.5" width="6.25" height="6.25" rx="1.2" />
        <rect x="13.5" y="13.5" width="6.25" height="6.25" rx="1.2" />
      </svg>
    );
  }

  if (kind === "image") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <rect x="4" y="4.5" width="16" height="15" rx="2.2" />
        <circle cx="9" cy="9.25" r="1.5" />
        <path d="m6.5 17 3.7-3.7a1.3 1.3 0 0 1 1.84 0L14 15.25l1.3-1.3a1.3 1.3 0 0 1 1.84 0L19 15.8" />
      </svg>
    );
  }

  if (kind === "video") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <rect x="4" y="5" width="12.75" height="14" rx="2.1" />
        <path d="m19.5 8.25-2.75 2v3.5l2.75 2z" />
        <path d="m9.25 9.25 4.25 2.75-4.25 2.75z" />
      </svg>
    );
  }

  if (kind === "pdf") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <path d="M7 3.75h6.95L18.5 8.3V18A2.25 2.25 0 0 1 16.25 20.25h-9A2.25 2.25 0 0 1 5 18V6A2.25 2.25 0 0 1 7.25 3.75z" />
        <path d="M13.75 3.95v4.1h4.1" />
        <path d="M8 15.75h2.1a1.3 1.3 0 0 0 0-2.6H8zm0 0v2.1M12.1 17.85v-4.7h1.35a1.95 1.95 0 1 1 0 3.9H12.1m5.25.8h-2.1v-4.7h2" />
      </svg>
    );
  }

  if (kind === "archive") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <rect x="5" y="4.75" width="14" height="14.5" rx="2" />
        <path d="M9.5 4.75v14.5M11.3 7h1.4M11.3 10h1.4M11.3 13h1.4M10.9 16h2.2" />
      </svg>
    );
  }

  if (kind === "doc") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
        <path d="M7 3.75h6.95L18.5 8.3V18A2.25 2.25 0 0 1 16.25 20.25h-9A2.25 2.25 0 0 1 5 18V6A2.25 2.25 0 0 1 7.25 3.75z" />
        <path d="M13.75 3.95v4.1h4.1" />
        <path d="M8.25 11.25h7.5M8.25 14.5h7.5M8.25 17.75h5.25" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" className={className}>
      <path d="M7 3.75h6.95L18.5 8.3V18A2.25 2.25 0 0 1 16.25 20.25h-9A2.25 2.25 0 0 1 5 18V6A2.25 2.25 0 0 1 7.25 3.75z" />
      <path d="M13.75 3.95v4.1h4.1" />
    </svg>
  );
}

function ConsoleIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3.75" y="5" width="16.5" height="14" rx="2.4" />
      <path d="m7.5 10 2.5 2-2.5 2M11.75 16h4.75" />
    </svg>
  );
}

function PhoneIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="7.25" y="3" width="9.5" height="18" rx="2.4" />
      <path d="M10.5 17.75h3" />
    </svg>
  );
}

function FolderOpenIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M4.5 19.25h15a1.75 1.75 0 0 0 1.69-1.3l1.56-6a1.25 1.25 0 0 0-1.21-1.57H5.24" />
      <path d="M2.75 7.5V17a1.5 1.5 0 0 0 .1.5L4.9 11a1.75 1.75 0 0 1 1.69-1.3H19V8.25A1.75 1.75 0 0 0 17.25 6.5h-5.5L9.5 4.75H4.5A1.75 1.75 0 0 0 2.75 6.5z" />
    </svg>
  );
}

function StepChevronIcon({ direction }: { direction: "up" | "down" }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d={direction === "up" ? "M3.5 10 8 5.5 12.5 10" : "M3.5 6 8 10.5 12.5 6"} />
    </svg>
  );
}

function openDateInputPicker(input: HTMLInputElement | null): void {
  if (!input) {
    return;
  }

  input.focus();
  try {
    const pickerInput = input as HTMLInputElement & { showPicker?: () => void };
    if (typeof pickerInput.showPicker === "function") {
      pickerInput.showPicker();
      return;
    }
  } catch {
    // Ignore and fall back to a normal click below.
  }

  input.click();
}

type NumberInputFieldProps = {
  id?: string;
  value: string;
  min?: number;
  max?: number;
  step?: number | string;
  placeholder?: string;
  ariaLabel?: string;
  clearable?: boolean;
  onChange: (value: string) => void;
};

function NumberInputField({
  id,
  value,
  min,
  max,
  step = 1,
  placeholder,
  ariaLabel,
  clearable = false,
  onChange,
}: NumberInputFieldProps) {
  const inputRef = useRef<HTMLInputElement | null>(null);

  const nudgeValue = (direction: "up" | "down") => {
    const input = inputRef.current;
    if (!input) {
      return;
    }

    input.focus();
    if (direction === "up") {
      input.stepUp();
    } else {
      input.stepDown();
    }
    onChange(input.value);
  };

  return (
    <div className={`number-input-shell ${clearable ? "has-clear" : ""}`}>
      <input
        ref={inputRef}
        id={id}
        type="number"
        autoComplete="off"
        min={min}
        max={max}
        step={step}
        value={value}
        placeholder={placeholder}
        inputMode="numeric"
        aria-label={ariaLabel}
        onChange={(event) => onChange(event.currentTarget.value)}
      />
      {clearable && value.trim().length > 0 ? (
        <button
          type="button"
          className="filter-input-clear"
          aria-label={`Clear ${ariaLabel ?? "number input"}`}
          onMouseDown={(event) => {
            event.preventDefault();
          }}
          onClick={() => {
            onChange("");
            inputRef.current?.focus();
          }}
        >
          <span aria-hidden="true">x</span>
        </button>
      ) : null}
      <div className="number-input-steppers" aria-hidden="true">
        <button
          type="button"
          className="number-input-stepper"
          tabIndex={-1}
          onMouseDown={(event) => {
            event.preventDefault();
          }}
          onClick={() => {
            nudgeValue("up");
          }}
        >
          <StepChevronIcon direction="up" />
        </button>
        <button
          type="button"
          className="number-input-stepper"
          tabIndex={-1}
          onMouseDown={(event) => {
            event.preventDefault();
          }}
          onClick={() => {
            nudgeValue("down");
          }}
        >
          <StepChevronIcon direction="down" />
        </button>
      </div>
    </div>
  );
}

function toBytesFromMb(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed) || parsed < 0) {
    return undefined;
  }
  return Math.floor(parsed * 1024 * 1024);
}

function normalizeSearchLimit(value: number): number {
  if (!Number.isFinite(value)) {
    return SEARCH_LIMIT;
  }
  return Math.min(SEARCH_LIMIT_MAX, Math.max(SEARCH_LIMIT_MIN, Math.floor(value)));
}

function toUnixStart(dateValue: string): number | undefined {
  if (!dateValue) {
    return undefined;
  }
  const unix = Date.parse(`${dateValue}T00:00:00`);
  if (Number.isNaN(unix)) {
    return undefined;
  }
  return Math.floor(unix / 1000);
}

function toUnixEnd(dateValue: string): number | undefined {
  if (!dateValue) {
    return undefined;
  }
  const unix = Date.parse(`${dateValue}T23:59:59`);
  if (Number.isNaN(unix)) {
    return undefined;
  }
  return Math.floor(unix / 1000);
}

function formatBytes(size: number): string {
  if (size <= 0) {
    return "-";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = size;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(unitIndex > 1 ? 1 : 0)} ${units[unitIndex]}`;
}

function formatUnix(unixSeconds: number): string {
  if (unixSeconds <= 0) {
    return "-";
  }
  const date = new Date(unixSeconds * 1000);
  if (Number.isNaN(date.getTime())) {
    return "-";
  }
  return date.toLocaleDateString();
}

function stripInvisibleText(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f-\u009f]/g, "");
}

function basenameFromPath(path: string): string {
  const normalized = path.replace(/\//g, "\\");
  const lastSlash = normalized.lastIndexOf("\\");
  if (lastSlash < 0 || lastSlash + 1 >= normalized.length) {
    return normalized.trim();
  }
  return normalized.slice(lastSlash + 1).trim();
}

function parentDirectoryFromPath(path: string): string {
  const normalized = path.replace(/\//g, "\\").trim();
  const lastSlash = normalized.lastIndexOf("\\");
  if (lastSlash <= 0) {
    return normalized;
  }
  return normalized.slice(0, lastSlash);
}

function extensionFromName(name: string): string {
  const trimmed = name.trim();
  const dotIndex = trimmed.lastIndexOf(".");
  if (dotIndex <= 0 || dotIndex === trimmed.length - 1) {
    return "";
  }
  return trimmed.slice(dotIndex + 1).toLowerCase();
}

function normalizeFileSearchResult(result: Omit<SearchResult, "resultKind">): SearchResult {
  return {
    ...result,
    resultKind: "file",
  };
}

function isAppSearchResult(result: SearchResult): boolean {
  return result.resultKind === "app";
}

function extensionFromPathLike(pathValue: string): string {
  const filename = basenameFromPath(pathValue.trim());
  const dotIndex = filename.lastIndexOf(".");
  if (dotIndex <= 0 || dotIndex === filename.length - 1) {
    return "";
  }
  return filename.slice(dotIndex + 1).trim().toLowerCase();
}

function appResultFromInstalledApp(app: InstalledApp, iconDataUrl?: string | null): SearchResult {
  const extension =
    extensionFromPathLike(app.revealPath ?? "") ||
    extensionFromPathLike(app.launchTarget) ||
    "app";

  return {
    resultKind: "app",
    name: app.name,
    path: app.displayPath,
    extension,
    size: app.size,
    createdUnix: app.createdUnix,
    modifiedUnix: app.modifiedUnix,
    isDirectory: false,
    appId: app.id,
    launchTarget: app.launchTarget,
    revealPath: app.revealPath ?? null,
    isFileSystemApp: app.isFileSystem,
    iconDataUrl: iconDataUrl ?? null,
    locationText: app.revealPath || app.launchTarget,
    locationLabel: app.revealPath ? "Path" : "App ID",
  };
}

function resultDisplayName(result: SearchResult): string {
  return result.name.trim() || basenameFromPath(result.path) || "(unnamed file)";
}

function resultFilenameWithoutExtension(result: SearchResult): string {
  const displayName = resultDisplayName(result);
  if (result.isDirectory) {
    return displayName;
  }
  const dotIndex = displayName.lastIndexOf(".");
  if (dotIndex <= 0) {
    return displayName;
  }
  return displayName.slice(0, dotIndex);
}

async function copyTextToClipboard(text: string): Promise<void> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  if (typeof document === "undefined") {
    throw new Error("Clipboard access is not available.");
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  textarea.style.pointerEvents = "none";
  document.body.appendChild(textarea);
  textarea.select();

  const copied = document.execCommand("copy");
  document.body.removeChild(textarea);
  if (!copied) {
    throw new Error("Clipboard copy failed.");
  }
}

function categoryFromResult(result: SearchResult): ResultViewTab {
  if (isAppSearchResult(result)) {
    return "apps";
  }
  const extension = result.extension;
  const ext = extension.trim().toLowerCase();
  if (!ext) {
    return "all";
  }
  if (APP_EXTENSIONS.has(ext)) {
    return "apps";
  }
  if (MEDIA_EXTENSIONS.has(ext)) {
    return "media";
  }
  if (DOC_EXTENSIONS.has(ext)) {
    return "docs";
  }
  if (ARCHIVE_EXTENSIONS.has(ext)) {
    return "archives";
  }
  return "all";
}

function rowKeyForResult(result: SearchResult): string {
  if (isAppSearchResult(result)) {
    return `app:${result.appId ?? result.launchTarget ?? result.name.toLowerCase()}`;
  }
  return `${result.path}:${result.modifiedUnix}`;
}

function normalizedExtension(result: SearchResult): string {
  if (isAppSearchResult(result)) {
    return result.extension.trim().replace(/^\./, "").toLowerCase();
  }
  if (result.isDirectory) {
    return "";
  }
  const ext = result.extension.trim().replace(/^\./, "").toLowerCase();
  if (ext) {
    return ext;
  }
  const dotIndex = result.name.lastIndexOf(".");
  if (dotIndex < 0 || dotIndex === result.name.length - 1) {
    return "";
  }
  return result.name.slice(dotIndex + 1).trim().toLowerCase();
}

function previewKindFromResult(result: SearchResult): PreviewKind {
  if (isAppSearchResult(result)) {
    return "none";
  }
  if (result.isDirectory) {
    return "none";
  }
  const ext = normalizedExtension(result);
  if (!ext) {
    return "none";
  }
  if (IMAGE_PREVIEW_EXTENSIONS.has(ext)) {
    return "image";
  }
  if (VIDEO_PREVIEW_EXTENSIONS.has(ext)) {
    return "video";
  }
  if (ext === "pdf") {
    return "pdf";
  }
  if (TEXT_PREVIEW_EXTENSIONS.has(ext)) {
    return "text";
  }
  return "none";
}

function isAssetPreviewKind(kind: PreviewKind): kind is "image" | "video" | "pdf" {
  return kind === "image" || kind === "video" || kind === "pdf";
}

function textPreviewCacheKey(
  result: SearchResult,
  contentQuery: string,
  contentMode: ContentSearchMode | null,
  maxChars: number,
): string {
  return [
    rowKeyForResult(result),
    maxChars,
    (contentMode ?? "auto").toLowerCase(),
    contentQuery.trim().toLowerCase(),
  ].join(":");
}

function promiseWithTimeout<T>(promise: Promise<T>, timeoutMs: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      reject(new Error(`${label} timed out.`));
    }, timeoutMs);

    promise
      .then((value) => {
        window.clearTimeout(timeout);
        resolve(value);
      })
      .catch((error) => {
        window.clearTimeout(timeout);
        reject(error);
      });
  });
}

function likelyUtf16Encoding(bytes: Uint8Array): "utf-16le" | "utf-16be" | null {
  if (bytes.length < 4) {
    return null;
  }

  const unitCount = Math.floor(bytes.length / 2);
  if (unitCount < 2) {
    return null;
  }

  let evenNulls = 0;
  let oddNulls = 0;
  for (let index = 0; index < unitCount * 2; index += 2) {
    if (bytes[index] === 0) {
      evenNulls += 1;
    }
    if (bytes[index + 1] === 0) {
      oddNulls += 1;
    }
  }

  if (oddNulls * 3 >= unitCount && evenNulls * 16 <= unitCount) {
    return "utf-16le";
  }
  if (evenNulls * 3 >= unitCount && oddNulls * 16 <= unitCount) {
    return "utf-16be";
  }
  return null;
}

function decodeTextPreviewBytes(bytes: Uint8Array, contentMode: ContentSearchMode | null): string {
  const mode = contentMode?.toLowerCase() ?? "auto";

  const decodeWith = (encoding: string, sourceBytes: Uint8Array) => {
    try {
      return new TextDecoder(encoding, { fatal: false }).decode(sourceBytes);
    } catch {
      return new TextDecoder("utf-8", { fatal: false }).decode(sourceBytes);
    }
  };

  if (mode === "utf8") {
    const trimmed = bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf ? bytes.slice(3) : bytes;
    return decodeWith("utf-8", trimmed);
  }
  if (mode === "utf16") {
    const trimmed = bytes[0] === 0xff && bytes[1] === 0xfe ? bytes.slice(2) : bytes;
    return decodeWith("utf-16le", trimmed);
  }
  if (mode === "utf16be") {
    const trimmed = bytes[0] === 0xfe && bytes[1] === 0xff ? bytes.slice(2) : bytes;
    return decodeWith("utf-16be", trimmed);
  }

  if (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    return decodeWith("utf-8", bytes.slice(3));
  }
  if (bytes[0] === 0xff && bytes[1] === 0xfe) {
    return decodeWith("utf-16le", bytes.slice(2));
  }
  if (bytes[0] === 0xfe && bytes[1] === 0xff) {
    return decodeWith("utf-16be", bytes.slice(2));
  }

  const detectedUtf16 = likelyUtf16Encoding(bytes);
  if (detectedUtf16) {
    return decodeWith(detectedUtf16, bytes);
  }

  return decodeWith("utf-8", bytes);
}

function normalizePreviewText(text: string): string {
  return text.replace(/^\uFEFF/, "").replace(/\0/g, "").replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}

function takeTextPreviewSnippet(
  text: string,
  maxChars: number,
  contentQuery: string,
): TextPreviewPayload {
  const normalizedText = normalizePreviewText(text);
  const effectiveMaxChars = Math.min(Math.max(maxChars, 120), 12_000);
  if (!normalizedText) {
    return {
      text: "",
      truncated: false,
      matched: false,
    };
  }

  const trimmedQuery = contentQuery.trim();
  if (trimmedQuery) {
    const lowerText = normalizedText.toLowerCase();
    const lowerQuery = trimmedQuery.toLowerCase();
    const matchIndex = lowerText.indexOf(lowerQuery);

    if (matchIndex >= 0) {
      const focusMatchEarly = effectiveMaxChars <= CONTENT_SNIPPET_MAX_CHARS;
      let start = matchIndex;
      let end = Math.min(normalizedText.length, start + effectiveMaxChars);

      if (!focusMatchEarly) {
        const availableContext = Math.max(0, effectiveMaxChars - trimmedQuery.length);
        const minimumLeadingContext = 24;
        const contextBefore = Math.min(
          availableContext,
          Math.max(minimumLeadingContext, Math.floor(availableContext * 0.5)),
        );
        start = Math.max(0, matchIndex - contextBefore);
        end = Math.min(normalizedText.length, start + effectiveMaxChars);
        const preferredEnd = Math.min(
          normalizedText.length,
          matchIndex + trimmedQuery.length + (availableContext - contextBefore),
        );
        if (end < preferredEnd) {
          end = preferredEnd;
          start = Math.max(0, end - effectiveMaxChars);
        }
      }
      const excerpt = `${start > 0 ? "…" : ""}${normalizedText.slice(start, end)}${end < normalizedText.length ? "…" : ""}`;
      return {
        text: excerpt,
        truncated: start > 0 || end < normalizedText.length,
        matched: true,
      };
    }
  }

  if (normalizedText.length <= effectiveMaxChars) {
    return {
      text: normalizedText,
      truncated: false,
      matched: false,
    };
  }

  return {
    text: `${normalizedText.slice(0, effectiveMaxChars)}…`,
    truncated: true,
    matched: false,
  };
}

function loadTextPreviewViaTauri(
  path: string,
  maxChars: number,
  contentQuery: string | null,
  contentMode: ContentSearchMode | null,
): Promise<TextPreviewPayload> {
  return promiseWithTimeout(
    invoke<TextPreviewPayload>("load_text_preview", {
      path,
      maxChars,
      max_chars: maxChars,
      contentQuery,
      content_query: contentQuery,
      contentMode: contentMode,
      content_mode: contentMode,
    }),
    TEXT_PREVIEW_COMMAND_TIMEOUT_MS,
    "Text preview",
  );
}

async function loadTextPreviewFallback(
  path: string,
  maxChars: number,
  contentQuery: string,
  contentMode: ContentSearchMode | null,
): Promise<TextPreviewPayload> {
  const candidates = [previewSrcFromPath(path), fileUrlFromPath(path)].filter((value) => value.length > 0);
  if (candidates.length === 0) {
    throw new Error("No URL available for text preview fallback.");
  }

  let lastError: unknown = null;
  for (const source of candidates) {
    const controller = new AbortController();
    const abortTimeout = window.setTimeout(() => {
      controller.abort();
    }, 2500);

    try {
      const response = await fetch(source, {
        cache: "no-store",
        signal: controller.signal,
      });
      if (!response.ok) {
        throw new Error(`Text preview fallback failed with status ${response.status}.`);
      }

      const bytes = new Uint8Array(await response.arrayBuffer());
      const decoded = decodeTextPreviewBytes(bytes, contentMode);
      return takeTextPreviewSnippet(decoded, maxChars, contentQuery);
    } catch (error) {
      lastError = error;
    } finally {
      window.clearTimeout(abortTimeout);
    }
  }

  throw lastError instanceof Error ? lastError : new Error("Text preview fallback failed.");
}

function iconKindFromResult(result: SearchResult): ResultIconKind {
  if (isAppSearchResult(result)) {
    return "app";
  }
  if (result.isDirectory) {
    return "folder";
  }

  const ext = normalizedExtension(result);
  if (!ext) {
    return "file";
  }

  if (APP_EXTENSIONS.has(ext)) {
    return "app";
  }
  if (IMAGE_PREVIEW_EXTENSIONS.has(ext)) {
    return "image";
  }
  if (VIDEO_PREVIEW_EXTENSIONS.has(ext)) {
    return "video";
  }
  if (ext === "pdf") {
    return "pdf";
  }
  if (ARCHIVE_EXTENSIONS.has(ext)) {
    return "archive";
  }
  if (DOC_EXTENSIONS.has(ext)) {
    return "doc";
  }
  return "file";
}

function resultLocationText(result: SearchResult): string {
  return result.locationText?.trim() || result.path;
}

function resultLocationLabel(result: SearchResult): string {
  return result.locationLabel?.trim() || "Path";
}

function canRevealResultLocation(result: SearchResult): boolean {
  if (isAppSearchResult(result)) {
    return Boolean(result.revealPath);
  }
  return true;
}

function ResultFallbackIcon({
  result,
  className,
}: {
  result: SearchResult;
  className?: string;
}) {
  const kind = iconKindFromResult(result);
  return (
    <span className={`result-fallback-icon-shell kind-${kind}${className ? ` ${className}` : ""}`} aria-hidden="true">
      <ResultTypeIcon kind={kind} className="result-fallback-icon" />
    </span>
  );
}

function previewSrcFromPath(path: string): string {
  try {
    return convertFileSrc(path, "asset");
  } catch {
    return "";
  }
}

function fileUrlFromPath(path: string): string {
  try {
    const normalizedPath = path.replace(/\\/g, "/");
    if (/^[A-Za-z]:\//.test(normalizedPath)) {
      const drive = normalizedPath.slice(0, 2);
      const rest = normalizedPath
        .slice(3)
        .split("/")
        .map((part) => encodeURIComponent(part))
        .join("/");
      return `file:///${drive}/${rest}`;
    }
    if (normalizedPath.startsWith("//")) {
      const uncPath = normalizedPath
        .split("/")
        .filter((part) => part.length > 0)
        .map((part) => encodeURIComponent(part))
        .join("/");
      return `file:///${uncPath}`;
    }
    return "";
  } catch {
    return "";
  }
}

function previewSourcesFromPath(path: string): string[] {
  const candidates = [previewSrcFromPath(path), fileUrlFromPath(path)].filter(
    (value) => value.length > 0,
  );
  return [...new Set(candidates)];
}

function recentActivityEntryKey(entry: RecentActivityEntry): string {
  return entry.kind === "query"
    ? `query:${entry.query.trim().toLowerCase()}`
    : isAppSearchResult(entry)
      ? `item:app:${(entry.appId ?? entry.launchTarget ?? entry.name).trim().toLowerCase()}`
      : `item:${stripInvisibleText(entry.path).trim().toLowerCase()}`;
}

function isPinnedRecentActivityEntry(entry: RecentActivityEntry): boolean {
  return entry.pinned;
}

function sortRecentActivityEntries(entries: RecentActivityEntry[]): RecentActivityEntry[] {
  return [...entries].sort((left, right) => {
    const pinnedDelta = Number(isPinnedRecentActivityEntry(right)) - Number(isPinnedRecentActivityEntry(left));
    if (pinnedDelta !== 0) {
      return pinnedDelta;
    }
    return right.usedAt - left.usedAt;
  });
}

function normalizeRecentActivityEntries(value: unknown): RecentActivityEntry[] {
  if (!Array.isArray(value)) {
    return [];
  }

  const normalized: RecentActivityEntry[] = [];

  for (const candidate of value) {
    if (!candidate || typeof candidate !== "object") {
      continue;
    }

    const entry = candidate as Record<string, unknown>;
    const rawUsedAt = Number(entry.usedAt);
    const usedAt = Number.isFinite(rawUsedAt) && rawUsedAt > 0 ? rawUsedAt : Date.now();

    if (entry.kind === "query") {
      const query = typeof entry.query === "string" ? stripInvisibleText(entry.query).trim() : "";
      if (query.length === 0) {
        continue;
      }
      normalized.push({
        kind: "query",
        query,
        usedAt,
        pinned: Boolean(entry.pinned),
      });
      continue;
    }

    if (entry.kind !== "item") {
      continue;
    }

    const name = typeof entry.name === "string" ? stripInvisibleText(entry.name).trim() : "";
    const path = typeof entry.path === "string" ? stripInvisibleText(entry.path).trim() : "";
    if (name.length === 0 || path.length === 0) {
      continue;
    }

    normalized.push({
      kind: "item",
      resultKind: entry.resultKind === "app" ? "app" : "file",
      name,
      path,
      extension: typeof entry.extension === "string" ? entry.extension : "",
      size: Number.isFinite(Number(entry.size)) ? Number(entry.size) : 0,
      createdUnix: Number.isFinite(Number(entry.createdUnix)) ? Number(entry.createdUnix) : 0,
      modifiedUnix: Number.isFinite(Number(entry.modifiedUnix)) ? Number(entry.modifiedUnix) : 0,
      isDirectory: Boolean(entry.isDirectory),
      appId: typeof entry.appId === "string" ? entry.appId : undefined,
      launchTarget: typeof entry.launchTarget === "string" ? entry.launchTarget : undefined,
      revealPath: typeof entry.revealPath === "string" ? entry.revealPath : undefined,
      isFileSystemApp: Boolean(entry.isFileSystemApp),
      iconDataUrl: typeof entry.iconDataUrl === "string" ? entry.iconDataUrl : undefined,
      locationText: typeof entry.locationText === "string" ? entry.locationText : undefined,
      locationLabel: typeof entry.locationLabel === "string" ? entry.locationLabel : undefined,
      usedAt,
      pinned: Boolean(entry.pinned),
    });
  }

  const deduped = new Map<string, RecentActivityEntry>();
  for (const entry of sortRecentActivityEntries(normalized)) {
    const key = recentActivityEntryKey(entry);
    if (!deduped.has(key)) {
      deduped.set(key, entry);
    }
  }

  return sortRecentActivityEntries(Array.from(deduped.values())).slice(0, RECENT_ACTIVITY_LIMIT);
}

function relevanceScore(result: SearchResult, queryValue: string): number {
  const query = queryValue.trim().toLowerCase();
  if (!query) {
    return 0;
  }

  const name = result.name.toLowerCase();
  const path = result.path.toLowerCase();
  const location = result.locationText?.toLowerCase() ?? "";
  const appBoost = isAppSearchResult(result) ? 500 : 0;

  if (name.startsWith(query)) {
    return 10_000 - name.length + appBoost;
  }

  const nameIndex = name.indexOf(query);
  if (nameIndex >= 0) {
    return 7_000 - nameIndex + appBoost;
  }

  const pathIndex = path.indexOf(query);
  if (pathIndex >= 0) {
    return 4_000 - pathIndex + appBoost;
  }

  const locationIndex = location.indexOf(query);
  if (locationIndex >= 0) {
    return 3_000 - locationIndex + appBoost;
  }

  return 0;
}

function highlightMatch(
  text: string,
  queryValue: string,
  variant: "default" | "content" = "default",
): ReactNode {
  const query = queryValue.trim();
  if (!query) {
    return text;
  }

  const lowerText = text.toLowerCase();
  const lowerQuery = query.toLowerCase();
  if (!lowerText.includes(lowerQuery)) {
    return text;
  }

  const nodes: ReactNode[] = [];
  let cursor = 0;
  let key = 0;

  while (cursor < text.length) {
    const nextIndex = lowerText.indexOf(lowerQuery, cursor);
    if (nextIndex === -1) {
      nodes.push(<span key={`t-${key}`}>{text.slice(cursor)}</span>);
      break;
    }

    if (nextIndex > cursor) {
      nodes.push(<span key={`t-${key}`}>{text.slice(cursor, nextIndex)}</span>);
      key += 1;
    }

    nodes.push(
      <mark className={`match-highlight ${variant === "content" ? "content-match-highlight" : ""}`} key={`m-${key}`}>
        {text.slice(nextIndex, nextIndex + query.length)}
      </mark>,
    );
    key += 1;
    cursor = nextIndex + query.length;
  }

  return <>{nodes}</>;
}

function hasSelectedText(): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  const selection = window.getSelection();
  return Boolean(selection && selection.toString().trim().length > 0);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return Boolean(
    target.closest(
      'input, textarea, select, [contenteditable=""], [contenteditable="true"], [role="textbox"]',
    ),
  );
}

function isQuickLookShortcutTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return Boolean(
    target.closest(
      'input, textarea, select, button, a, iframe, video, audio, summary, [contenteditable=""], [contenteditable="true"], [role="button"], [role="link"], [role="textbox"]',
    ),
  );
}

function isThemePresetId(value: string | null): value is ThemePresetId {
  return value !== null && THEME_PRESET_IDS.includes(value as ThemePresetId);
}

function normalizeThemePresetId(value: string | null): ThemePresetId | null {
  if (value === "graphite") {
    return "slate-glass";
  }

  return isThemePresetId(value) ? value : null;
}

function themePresetById(id: ThemePresetId): ThemePreset {
  return (
    THEME_PRESETS.find((preset) => preset.id === id) ??
    THEME_PRESETS.find((preset) => preset.id === DEFAULT_THEME_PRESET) ??
    THEME_PRESETS[0]
  );
}

function themePreviewStyle(preview: ThemePreviewSwatch): CSSProperties {
  return {
    "--theme-preview-bg": preview.bg,
    "--theme-preview-panel": preview.panel,
    "--theme-preview-panel-alt": preview.panelAlt,
    "--theme-preview-accent": preview.accent,
    "--theme-preview-text": preview.text,
    "--theme-preview-muted": preview.muted,
    "--theme-preview-glow": preview.glow,
  } as CSSProperties;
}

function formatShortcutLabel(value: string): string {
  return value
    .split("+")
    .map((segment) => {
      const part = segment.trim();
      if (!part) {
        return "";
      }
      if (/^key[a-z]$/i.test(part)) {
        return part.slice(3).toUpperCase();
      }
      if (/^digit\d$/i.test(part)) {
        return part.slice(5);
      }
      if (/^meta$/i.test(part)) {
        return "Win";
      }
      if (/^control$/i.test(part) || /^ctrl$/i.test(part)) {
        return "Ctrl";
      }
      if (/^alt$/i.test(part)) {
        return "Alt";
      }
      if (/^shift$/i.test(part)) {
        return "Shift";
      }
      return part.charAt(0).toUpperCase() + part.slice(1);
    })
    .filter((part) => part.length > 0)
    .join("+");
}

function parseSearchSyntaxPreview(value: string): SearchSyntaxPreview {
  let hasContentSearch = false;
  let contentQuery = "";
  let contentMode: ContentSearchMode | null = null;

  const pathQuery = value
    .replace(SEARCH_SYNTAX_PATTERN, (match, keyword: string, quotedValue?: string, bareValue?: string) => {
      if (keyword.toLowerCase().includes("content")) {
        hasContentSearch = true;
        contentQuery = (quotedValue ?? bareValue ?? "").trim();
        contentMode =
          keyword.toLowerCase() === "content"
            ? "auto"
            : (keyword.toLowerCase().replace("content", "") as ContentSearchMode);
      }
      return match.startsWith(" ") ? " " : "";
    })
    .replace(/\s+/g, " ")
    .trim();

  return {
    pathQuery,
    hasContentSearch,
    contentQuery,
    contentMode,
  };
}

function cancelSearchRequest(): void {
  void invoke("cancel_search").catch(() => {
    // Ignore cancellation failures outside the desktop shell.
  });
}

function readViewportSize(): { width: number; height: number } {
  if (typeof window === "undefined") {
    return {
      width: QUICK_SEARCH_COMPACT_WIDTH,
      height: QUICK_SEARCH_COMPACT_HEIGHT,
    };
  }

  return {
    width: window.innerWidth,
    height: window.innerHeight,
  };
}

function normalizeQuickResultsPaneRatio(value: unknown): number {
  if (value === null || value === undefined || value === "") {
    return QUICK_RESULTS_PANE_DEFAULT_RATIO;
  }

  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    return QUICK_RESULTS_PANE_DEFAULT_RATIO;
  }

  return Math.min(Math.max(parsed, 0.2), 0.8);
}

function clampQuickResultsPaneRatio(value: number, stageWidth: number): number {
  const normalized = normalizeQuickResultsPaneRatio(value);
  if (!Number.isFinite(stageWidth) || stageWidth <= 0) {
    return normalized;
  }

  const minRatio = QUICK_RESULTS_PANE_MIN_WIDTH / stageWidth;
  const maxRatio = (stageWidth - QUICK_PREVIEW_PANE_MIN_WIDTH - QUICK_RESULTS_SPLITTER_WIDTH) / stageWidth;
  if (!Number.isFinite(minRatio) || !Number.isFinite(maxRatio) || maxRatio <= minRatio) {
    return 0.5;
  }

  return Math.min(Math.max(normalized, minRatio), maxRatio);
}

function normalizeReleaseNotesMarkdown(notes: string): string {
  return notes
    .replace(/\r\n/g, "\n")
    .replace(/\\([#&*_[\]()`>+\-.!])/g, "$1")
    .trim();
}

function renderInlineMarkdown(text: string): ReactNode[] {
  const segments: ReactNode[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*|\*[^*]+\*)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      segments.push(text.slice(lastIndex, match.index));
    }

    const token = match[0];
    const key = `${match.index}-${token}`;
    if (token.startsWith("`")) {
      segments.push(<code key={key}>{token.slice(1, -1)}</code>);
    } else if (token.startsWith("**")) {
      segments.push(<strong key={key}>{token.slice(2, -2)}</strong>);
    } else {
      segments.push(<em key={key}>{token.slice(1, -1)}</em>);
    }
    lastIndex = match.index + token.length;
  }

  if (lastIndex < text.length) {
    segments.push(text.slice(lastIndex));
  }

  return segments;
}

function renderUpdateReleaseNotes(notes: string): ReactNode[] {
  const normalizedNotes = normalizeReleaseNotesMarkdown(notes);
  if (!normalizedNotes) {
    return [];
  }

  const nodes: ReactNode[] = [];
  let listItems: string[] = [];
  const flushList = () => {
    if (listItems.length === 0) {
      return;
    }
    const items = listItems;
    listItems = [];
    nodes.push(
      <ul key={`list-${nodes.length}`} className="update-notes-list">
        {items.map((item, index) => (
          <li key={`${index}-${item}`}>{renderInlineMarkdown(item)}</li>
        ))}
      </ul>,
    );
  };

  normalizedNotes.split("\n").forEach((rawLine) => {
    const line = rawLine.trim();
    if (!line) {
      flushList();
      return;
    }

    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading) {
      flushList();
      nodes.push(
        <p
          key={`heading-${nodes.length}`}
          className={heading[1].length === 1 ? "update-notes-heading-main" : "update-notes-heading"}
        >
          {renderInlineMarkdown(heading[2])}
        </p>,
      );
      return;
    }

    const bullet = /^[-*]\s+(.+)$/.exec(line) ?? /^\d+[.)]\s+(.+)$/.exec(line);
    if (bullet) {
      listItems.push(bullet[1]);
      return;
    }

    flushList();
    nodes.push(
      <p key={`paragraph-${nodes.length}`} className="update-notes-paragraph">
        {renderInlineMarkdown(line)}
      </p>,
    );
  });

  flushList();
  return nodes;
}

async function resolveUpdaterTarget(): Promise<string | undefined> {
  try {
    const bundleType = await getBundleType();
    if (bundleType !== BundleType.Nsis && bundleType !== BundleType.Msi) {
      return undefined;
    }
    const arch = await invoke<string>("current_arch");
    return `windows-${arch}-${bundleType}`;
  } catch {
    return undefined;
  }
}

function App() {
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => {
    if (typeof window === "undefined") {
      return "dark";
    }
    const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (saved === "light" || saved === "dark") {
      return saved;
    }
    return "dark";
  });
  const [themePreset, setThemePreset] = useState<ThemePresetId>(() => {
    if (typeof window === "undefined") {
      return DEFAULT_THEME_PRESET;
    }
    const saved = window.localStorage.getItem(THEME_PRESET_STORAGE_KEY);
    const normalizedPreset = normalizeThemePresetId(saved);
    if (normalizedPreset) {
      return normalizedPreset;
    }
    return DEFAULT_THEME_PRESET;
  });
  const [status, setStatus] = useState<IndexStatus>({
    indexing: false,
    ready: false,
    indexedCount: 0,
    lastError: null,
  });
  const [indexSyncing, setIndexSyncing] = useState(false);
  const [appliedIndexConfigKey, setAppliedIndexConfigKey] = useState("");
  const [pendingIndexConfigKey, setPendingIndexConfigKey] = useState("");
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [selectedDrive, setSelectedDrive] = useState("");
  const [driveError, setDriveError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [extension, setExtension] = useState("");
  const [minSizeMb, setMinSizeMb] = useState("");
  const [maxSizeMb, setMaxSizeMb] = useState("");
  const [createdAfter, setCreatedAfter] = useState("");
  const [createdBefore, setCreatedBefore] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [installedApps, setInstalledApps] = useState<InstalledApp[]>([]);
  const [installedAppsLoading, setInstalledAppsLoading] = useState(false);
  const [installedAppsError, setInstalledAppsError] = useState<string | null>(null);
  const [installedAppsRefreshKey, setInstalledAppsRefreshKey] = useState(0);
  const [includeInstalledApps, setIncludeInstalledApps] = useState<boolean>(() => {
    if (typeof window === "undefined") {
      return false;
    }
    return window.localStorage.getItem(INCLUDE_INSTALLED_APPS_STORAGE_KEY) === "1";
  });
  const [appIconDataUrls, setAppIconDataUrls] = useState<Record<string, string>>({});
  const [appIconFailures, setAppIconFailures] = useState<Record<string, true>>({});
  const [loading, setLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [searchSyntaxHelpSectionId, setSearchSyntaxHelpSectionId] = useState<string>("basics");
  const [activeTab, setActiveTab] = useState<ActiveTab>("search");
  const [syncServerInfo, setSyncServerInfo] = useState<SyncServerInfo | null>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [pairingUriCopied, setPairingUriCopied] = useState(false);
  const [duplicateMinSizeMb, setDuplicateMinSizeMb] = useState("50");
  const [duplicateGroups, setDuplicateGroups] = useState<DuplicateGroup[]>([]);
  const [duplicatesLoading, setDuplicatesLoading] = useState(false);
  const [duplicatesError, setDuplicatesError] = useState<string | null>(null);
  const [duplicateNotice, setDuplicateNotice] = useState<string | null>(null);
  const [duplicateScanStatus, setDuplicateScanStatus] = useState<DuplicateScanStatus>({
    running: false,
    cancelRequested: false,
    scannedFiles: 0,
    totalFiles: 0,
    groupsFound: 0,
    progressPercent: 0,
  });
  const [duplicateDeleteCandidate, setDuplicateDeleteCandidate] =
    useState<DuplicateDeleteCandidate | null>(null);
  const [duplicateDeleteBusy, setDuplicateDeleteBusy] = useState(false);
  const [duplicateDeleteToRecycleBin, setDuplicateDeleteToRecycleBin] = useState(false);
  const [searchResultContextMenu, setSearchResultContextMenu] =
    useState<SearchResultContextMenuState | null>(null);
  const [searchResultRenameDraft, setSearchResultRenameDraft] =
    useState<SearchResultRenameDraft | null>(null);
  const [searchResultRenameBusy, setSearchResultRenameBusy] = useState(false);
  const [searchResultDeleteCandidate, setSearchResultDeleteCandidate] =
    useState<SearchResultDeleteCandidate | null>(null);
  const [searchResultDeleteBusy, setSearchResultDeleteBusy] = useState(false);
  const [searchResultDeleteToRecycleBin, setSearchResultDeleteToRecycleBin] = useState(false);
  const [resultView, setResultView] = useState<ResultViewTab>("all");
  const [resultSort, setResultSort] = useState<ResultSortMode>("relevance");
  const [selectedPathsForBulk, setSelectedPathsForBulk] = useState<Set<string>>(() => new Set());
  const [isSelectionModeActive, setIsSelectionModeActive] = useState(false);

  // Bulk rename states
  const [bulkRenameOpen, setBulkRenameOpen] = useState(false);
  const [bulkRenameMode, setBulkRenameMode] = useState<"replace" | "prefix_suffix" | "enumerate">("replace");
  const [bulkRenameReplaceSearch, setBulkRenameReplaceSearch] = useState("");
  const [bulkRenameReplaceWith, setBulkRenameReplaceWith] = useState("");
  const [bulkRenamePrefix, setBulkRenamePrefix] = useState("");
  const [bulkRenameSuffix, setBulkRenameSuffix] = useState("");
  const [bulkRenameEnumerateBase, setBulkRenameEnumerateBase] = useState("");
  const [bulkRenameEnumerateDigits, setBulkRenameEnumerateDigits] = useState(3);
  const [bulkRenameEnumerateStart, setBulkRenameEnumerateStart] = useState(1);
  const [bulkRenameBusy, setBulkRenameBusy] = useState(false);

  // Bulk delete states
  const [bulkDeleteOpen, setBulkDeleteOpen] = useState(false);
  const [bulkDeleteRecycleBin, setBulkDeleteRecycleBin] = useState(true);
  const [bulkDeleteBusy, setBulkDeleteBusy] = useState(false);

  // Bulk action progress state
  const [bulkActionProgress, setBulkActionProgress] = useState<{
    current: number;
    total: number;
    label: string;
  } | null>(null);

  const [windowMode, setWindowMode] = useState<WindowMode>("full");
  const [viewportSize, setViewportSize] = useState(() => readViewportSize());
  const [quickResultsPaneRatio, setQuickResultsPaneRatio] = useState<number>(() => {
    if (typeof window === "undefined") {
      return QUICK_RESULTS_PANE_DEFAULT_RATIO;
    }
    return normalizeQuickResultsPaneRatio(
      window.localStorage.getItem(QUICK_RESULTS_PANE_RATIO_STORAGE_KEY),
    );
  });
  const [quickResultsStageWidth, setQuickResultsStageWidth] = useState(0);
  const [quickSplitDragging, setQuickSplitDragging] = useState(false);
  const [showPreviews, setShowPreviews] = useState<boolean>(() => {
    if (typeof window === "undefined") {
      return true;
    }
    const saved = window.localStorage.getItem(PREVIEW_STORAGE_KEY);
    if (saved === "0") {
      return false;
    }
    if (saved === "1") {
      return true;
    }
    return true;
  });
  const [searchMetricsHidden, setSearchMetricsHidden] = useState<boolean>(() => {
    if (typeof window === "undefined") {
      return false;
    }
    return window.localStorage.getItem(SEARCH_METRICS_HIDDEN_STORAGE_KEY) === "1";
  });
  const [includeFolders, setIncludeFolders] = useState<boolean>(() => {
    if (typeof window === "undefined") {
      return false;
    }
    return window.localStorage.getItem(INCLUDE_FOLDERS_STORAGE_KEY) === "1";
  });
  const [includeAllDrives, setIncludeAllDrives] = useState<boolean>(() => {
    if (typeof window === "undefined") {
      return false;
    }
    return window.localStorage.getItem(INCLUDE_ALL_DRIVES_STORAGE_KEY) === "1";
  });
  const [defaultSearchLimit, setDefaultSearchLimit] = useState<number>(() => {
    if (typeof window === "undefined") {
      return SEARCH_LIMIT;
    }
    const savedRaw = window.localStorage.getItem(SEARCH_LIMIT_STORAGE_KEY);
    if (!savedRaw) {
      return SEARCH_LIMIT;
    }
    const saved = Number(savedRaw);
    return normalizeSearchLimit(saved);
  });
  const [recentActivityEnabled, setRecentActivityEnabled] = useState<boolean>(() => {
    if (typeof window === "undefined") {
      return true;
    }
    return window.localStorage.getItem(RECENT_ACTIVITY_ENABLED_STORAGE_KEY) !== "0";
  });
  const [recentActivity, setRecentActivity] = useState<RecentActivityEntry[]>(() => {
    if (typeof window === "undefined") {
      return [];
    }
    try {
      const saved = window.localStorage.getItem(RECENT_ACTIVITY_STORAGE_KEY);
      if (!saved) {
        return [];
      }
      return normalizeRecentActivityEntries(JSON.parse(saved));
    } catch {
      return [];
    }
  });
  const [searchLimit, setSearchLimit] = useState<number>(defaultSearchLimit);
  const [searchLimitInput, setSearchLimitInput] = useState<string>(() =>
    String(defaultSearchLimit),
  );
  const [searchLimitError, setSearchLimitError] = useState<string | null>(null);
  const [searchLimitMessage, setSearchLimitMessage] = useState<string | null>(null);
  const [desktopSettings, setDesktopSettings] =
    useState<DesktopSettings>(DEFAULT_DESKTOP_SETTINGS);
  const [desktopSettingsDraft, setDesktopSettingsDraft] =
    useState<DesktopSettings>(DEFAULT_DESKTOP_SETTINGS);
  const [desktopSettingsLoading, setDesktopSettingsLoading] = useState(true);
  const [desktopSettingsSaving, setDesktopSettingsSaving] = useState(false);
  const [desktopLayoutResetting, setDesktopLayoutResetting] = useState(false);
  const [desktopSettingsError, setDesktopSettingsError] = useState<string | null>(null);
  const [desktopSettingsMessage, setDesktopSettingsMessage] = useState<string | null>(null);
  const [selectedResultKey, setSelectedResultKey] = useState<string | null>(null);
  const [previewSourceState, setPreviewSourceState] = useState<Record<string, number>>({});
  const [previewReadyState, setPreviewReadyState] = useState<Record<string, true>>({});
  const [previewDataUrls, setPreviewDataUrls] = useState<Record<string, string>>({});
  const [textPreviewState, setTextPreviewState] = useState<Record<string, TextPreviewPayload>>({});
  const [textPreviewLoadingState, setTextPreviewLoadingState] = useState<Record<string, true>>({});
  const [selectedPreviewSourceIndex, setSelectedPreviewSourceIndex] = useState<number>(0);
  const [selectedPreviewReadyState, setSelectedPreviewReadyState] = useState<Record<string, true>>({});
  const [quickLookOpen, setQuickLookOpen] = useState(false);
  const [quickLookCopyState, setQuickLookCopyState] = useState<"idle" | "copied" | "error">("idle");
  const [appVersion, setAppVersion] = useState<string>("");
  const [availableUpdate, setAvailableUpdate] = useState<AppUpdateInfo | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgressState>({
    stage: "idle",
    downloaded: 0,
    total: null,
    percent: 0,
    message: "",
  });
  const previousIndexedCountRef = useRef<number | null>(null);
  const indexSyncTimeoutRef = useRef<number | null>(null);
  const duplicateNoticeTimeoutRef = useRef<number | null>(null);
  const actionNoticeTimeoutRef = useRef<number | null>(null);
  const quickLookCopyTimeoutRef = useRef<number | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const extensionInputRef = useRef<HTMLInputElement | null>(null);
  const createdAfterInputRef = useRef<HTMLInputElement | null>(null);
  const createdBeforeInputRef = useRef<HTMLInputElement | null>(null);
  const quickResultsStageRef = useRef<HTMLDivElement | null>(null);
  const quickLookDialogRef = useRef<HTMLDivElement | null>(null);
  const quickLookReturnFocusRef = useRef<HTMLElement | null>(null);
  const searchResultContextMenuRef = useRef<HTMLDivElement | null>(null);
  const searchResultRenameInputRef = useRef<HTMLInputElement | null>(null);
  const previousSearchQueryRef = useRef("");
  const longPressTimeoutRef = useRef<number | null>(null);
  const longPressTriggeredRef = useRef(false);
  const activeThemePreset = themePresetById(themePreset);
  const isQuickMode = windowMode === "quick";
  const isCompactSearchViewport = isQuickMode
    ? viewportSize.width <= QUICK_SEARCH_COMPACT_WIDTH || viewportSize.height <= QUICK_SEARCH_COMPACT_HEIGHT
    : viewportSize.width <= FULL_SEARCH_COMPACT_WIDTH || viewportSize.height <= FULL_SEARCH_COMPACT_HEIGHT;
  const formattedDesktopShortcut = formatShortcutLabel(
    desktopSettings.shortcut || DEFAULT_DESKTOP_SETTINGS.shortcut,
  );
  const currentShortcutLabel = desktopSettings.shortcutEnabled
    ? `Hotkey: ${formattedDesktopShortcut}`
    : "Hotkey off";
  const searchLimitInputTrimmed = searchLimitInput.trim();
  const savedSearchLimitInput = String(defaultSearchLimit);
  const parsedSearchLimitInput = Number(searchLimitInputTrimmed);
  const pendingSearchLimit =
    searchLimitInputTrimmed.length > 0 &&
      Number.isFinite(parsedSearchLimitInput) &&
      parsedSearchLimitInput > 0
      ? normalizeSearchLimit(parsedSearchLimitInput)
      : null;
  const searchLimitValueNeedsNormalization =
    pendingSearchLimit !== null && searchLimitInputTrimmed !== String(pendingSearchLimit);
  const searchLimitHasPendingChanges = searchLimitInputTrimmed !== savedSearchLimitInput;
  const searchLimitCanResetToDefault =
    defaultSearchLimit !== SEARCH_LIMIT || searchLimitInputTrimmed !== String(SEARCH_LIMIT);
  const quickIndexScopeLabel = includeAllDrives ? "Index: all drives" : `Index: ${selectedDrive}:`;
  const desktopSettingsDirty =
    desktopSettings.backgroundModeEnabled !== desktopSettingsDraft.backgroundModeEnabled ||
    desktopSettings.shortcutEnabled !== desktopSettingsDraft.shortcutEnabled ||
    desktopSettings.shortcut.trim() !== desktopSettingsDraft.shortcut.trim() ||
    desktopSettings.rememberWindowBounds !== desktopSettingsDraft.rememberWindowBounds;

  const hasFilters =
    extension.trim().length > 0 ||
    minSizeMb.trim().length > 0 ||
    maxSizeMb.trim().length > 0 ||
    createdAfter.length > 0 ||
    createdBefore.length > 0;
  const hasMetadataFilters =
    minSizeMb.trim().length > 0 ||
    maxSizeMb.trim().length > 0 ||
    createdAfter.length > 0 ||
    createdBefore.length > 0;
  const visibleStatusError =
    status.lastError && status.lastError.toLowerCase().includes(DUPLICATE_CANCEL_MESSAGE.toLowerCase())
      ? null
      : status.lastError;
  const searchSyntaxPreview = useMemo(() => parseSearchSyntaxPreview(query), [query]);
  const trimmedQuery = query.trim();
  const displayQuery = searchSyntaxPreview.pathQuery.trim();
  const hasContentSearchSyntax = searchSyntaxPreview.hasContentSearch;
  const contentSearchQuery = searchSyntaxPreview.contentQuery.trim();
  const contentSearchMode = searchSyntaxPreview.contentMode;
  const requestedIndexConfigKey = includeAllDrives
    ? `ALL:${includeFolders ? "1" : "0"}`
    : selectedDrive
      ? `${selectedDrive}:${includeFolders ? "1" : "0"}`
      : "";

  const canIncludeInstalledAppsInResults =
    includeInstalledApps &&
    trimmedQuery.length > 0 &&
    !hasContentSearchSyntax;

  const appResults = useMemo<SearchResult[]>(() => {
    if (!canIncludeInstalledAppsInResults) {
      return [];
    }

    const normalizedQuery = trimmedQuery.trim().toLowerCase();
    if (!normalizedQuery) {
      return [];
    }

    const matches = installedApps.filter((app) => {
      const name = app.name.toLowerCase();
      const displayPath = app.displayPath.toLowerCase();
      const launchTarget = app.launchTarget.toLowerCase();
      return (
        name.includes(normalizedQuery) ||
        displayPath.includes(normalizedQuery) ||
        launchTarget.includes(normalizedQuery)
      );
    });

    matches.sort((left, right) => {
      const leftScore = relevanceScore(appResultFromInstalledApp(left), trimmedQuery);
      const rightScore = relevanceScore(appResultFromInstalledApp(right), trimmedQuery);
      if (rightScore !== leftScore) {
        return rightScore - leftScore;
      }
      return left.name.localeCompare(right.name, undefined, { sensitivity: "base" });
    });

    return matches
      .slice(0, Math.min(searchLimit, 40))
      .map((app) => appResultFromInstalledApp(app, appIconDataUrls[app.id]));
  }, [
    appIconDataUrls,
    canIncludeInstalledAppsInResults,
    installedApps,
    searchLimit,
    trimmedQuery,
  ]);

  const combinedResults = useMemo<SearchResult[]>(
    () => [...appResults, ...results],
    [appResults, results],
  );

  const filteredCombinedResults = useMemo(() => {
    const normalizedExtensionFilter = extension.trim().replace(/^\./, "").toLowerCase();
    const minSize = toBytesFromMb(minSizeMb);
    const maxSize = toBytesFromMb(maxSizeMb);
    const minCreatedUnix = toUnixStart(createdAfter);
    const maxCreatedUnix = toUnixEnd(createdBefore);

    return combinedResults.filter((result) => {
      if (normalizedExtensionFilter) {
        if (normalizedExtensionFilter === "folder") {
          if (!result.isDirectory) {
            return false;
          }
        } else if (normalizedExtension(result) !== normalizedExtensionFilter) {
          return false;
        }
      }

      if (minSize !== undefined || maxSize !== undefined) {
        if (isAppSearchResult(result) && result.size <= 0) {
          return false;
        }
        if (minSize !== undefined && result.size < minSize) {
          return false;
        }
        if (maxSize !== undefined && result.size > maxSize) {
          return false;
        }
      }

      if (minCreatedUnix !== undefined || maxCreatedUnix !== undefined) {
        if (result.createdUnix <= 0) {
          return false;
        }
        if (minCreatedUnix !== undefined && result.createdUnix < minCreatedUnix) {
          return false;
        }
        if (maxCreatedUnix !== undefined && result.createdUnix > maxCreatedUnix) {
          return false;
        }
      }

      return true;
    });
  }, [combinedResults, createdAfter, createdBefore, extension, maxSizeMb, minSizeMb]);

  const resultCounts = useMemo<Record<ResultViewTab, number>>(() => {
    const counts: Record<ResultViewTab, number> = {
      all: filteredCombinedResults.length,
      apps: 0,
      media: 0,
      docs: 0,
      archives: 0,
    };

    for (const result of filteredCombinedResults) {
      const category = categoryFromResult(result);
      if (category !== "all") {
        counts[category] += 1;
      }
    }

    return counts;
  }, [filteredCombinedResults]);

  const visibleResults = useMemo(() => {
    const filtered = filteredCombinedResults.filter((result) => {
      if (resultView === "all") {
        return true;
      }
      return categoryFromResult(result) === resultView;
    });

    const sorted = [...filtered];
    sorted.sort((left, right) => {
      if (resultSort === "newest") {
        return right.modifiedUnix - left.modifiedUnix;
      }
      if (resultSort === "largest") {
        return right.size - left.size;
      }
      if (resultSort === "name") {
        return left.name.localeCompare(right.name, undefined, { sensitivity: "base" });
      }

      const rankDiff = relevanceScore(right, displayQuery) - relevanceScore(left, displayQuery);
      if (rankDiff !== 0) {
        return rankDiff;
      }
      if (left.resultKind !== right.resultKind) {
        return left.resultKind === "app" ? -1 : 1;
      }
      if (left.isDirectory !== right.isDirectory) {
        return left.isDirectory ? 1 : -1;
      }
      if (right.modifiedUnix !== left.modifiedUnix) {
        return right.modifiedUnix - left.modifiedUnix;
      }
      return left.name.localeCompare(right.name, undefined, { sensitivity: "base" });
    });

    return sorted;
  }, [displayQuery, filteredCombinedResults, resultView, resultSort]);

  const visibleTotalBytes = useMemo(
    () => visibleResults.reduce((sum, result) => sum + result.size, 0),
    [visibleResults],
  );
  const selectedResult = useMemo(() => {
    if (visibleResults.length === 0) {
      return null;
    }
    return visibleResults.find((result) => rowKeyForResult(result) === selectedResultKey) ?? visibleResults[0];
  }, [selectedResultKey, visibleResults]);
  const previewLoadCandidates = useMemo(() => {
    if (!selectedResult || (!showPreviews && !quickLookOpen)) {
      return [];
    }
    return [selectedResult].slice(0, PREVIEW_DATA_URL_LIMIT);
  }, [quickLookOpen, selectedResult, showPreviews]);
  const contentSnippetCandidates = useMemo(() => {
    if (!hasContentSearchSyntax || !contentSearchQuery) {
      return [];
    }

    return visibleResults
      .filter(
        (result) =>
          !isAppSearchResult(result) &&
          !result.isDirectory &&
          previewKindFromResult(result) === "text",
      )
      .slice(0, CONTENT_SNIPPET_RESULT_LIMIT);
  }, [contentSearchQuery, hasContentSearchSyntax, visibleResults]);

  const duplicateStats = useMemo(() => {
    let totalFiles = 0;
    let reclaimableBytes = 0;
    let listedFiles = 0;
    for (const group of duplicateGroups) {
      totalFiles += group.fileCount;
      listedFiles += group.files.length;
      reclaimableBytes += Math.max(0, group.fileCount - 1) * group.size;
    }
    return {
      groupCount: duplicateGroups.length,
      totalFiles,
      listedFiles,
      reclaimableBytes,
    };
  }, [duplicateGroups]);

  useEffect(() => {
    const root = document.documentElement;
    root.setAttribute("data-theme", themeMode);
    root.setAttribute("data-theme-preset", themePreset);
    const palette = activeThemePreset[themeMode];
    for (const [token, value] of Object.entries(palette)) {
      root.style.setProperty(token, value);
    }
    window.localStorage.setItem(THEME_STORAGE_KEY, themeMode);
    window.localStorage.setItem(THEME_PRESET_STORAGE_KEY, themePreset);
    const nativeBackground = palette["--bg-deep"] ?? palette["--surface-strong"];
    const titleBarColor = palette["--bg-deep"] ?? nativeBackground;
    const titleBarTextColor = palette["--text-main"];
    void syncNativeWindowTheme(themeMode, nativeBackground, titleBarColor, titleBarTextColor).catch(() => {
      // Ignore native window sync failures outside the desktop shell.
    });
  }, [activeThemePreset, themeMode, themePreset]);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      document.body.classList.add("app-boot-ready");
    });

    return () => {
      window.cancelAnimationFrame(frame);
      document.body.classList.remove("app-boot-ready");
    };
  }, []);

  useEffect(() => {
    let active = true;
    let timer: any;

    const pollSyncServer = () => {
      invoke<SyncServerInfo>("get_mobile_sync_server_info")
        .then((info) => {
          if (active) {
            setSyncServerInfo(info);
          }
        })
        .catch((error) => {
          console.error("Failed to poll mobile sync server info:", error);
        })
        .finally(() => {
          if (active) {
            timer = window.setTimeout(pollSyncServer, 1000);
          }
        });
    };

    pollSyncServer();

    return () => {
      active = false;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen<DesktopSendToPhoneResult>("desktop-send-to-phone-result", (event) => {
      const { queued, failed, messages } = event.payload;
      setActiveTab("sync");
      void invoke<SyncServerInfo>("get_mobile_sync_server_info")
        .then(setSyncServerInfo)
        .catch((error) => {
          console.error("Failed to refresh mobile sync server info:", error);
        });

      if (queued > 0) {
        setActionError(null);
        if (failed > 0) {
          showActionNotice(`Queued ${queued} file${queued === 1 ? "" : "s"}; ${failed} skipped.`);
        } else if (queued === 1 && messages[0]) {
          showActionNotice(messages[0]);
        } else {
          showActionNotice(`Queued ${queued} files for phone.`);
        }
        return;
      }

      setActionError(messages[0] ?? "Connect a phone before sending files.");
    }).then((dispose) => {
      if (disposed) {
        dispose();
      } else {
        unlisten = dispose;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const startSyncServer = async () => {
    setSyncBusy(true);
    setSyncError(null);
    try {
      const info = await invoke<SyncServerInfo>("start_mobile_sync_server");
      setSyncServerInfo(info);
    } catch (err) {
      setSyncError(String(err));
    } finally {
      setSyncBusy(false);
    }
  };

  const stopSyncServer = async () => {
    setSyncBusy(true);
    setSyncError(null);
    try {
      const info = await invoke<SyncServerInfo>("stop_mobile_sync_server");
      setSyncServerInfo(info);
    } catch (err) {
      setSyncError(String(err));
    } finally {
      setSyncBusy(false);
    }
  };

  const approvePairing = async (deviceId: string) => {
    setSyncError(null);
    try {
      await invoke("approve_mobile_pairing", { deviceId });
      const info = await invoke<SyncServerInfo>("get_mobile_sync_server_info");
      setSyncServerInfo(info);
    } catch (err) {
      setSyncError(String(err));
    }
  };

  const rejectPairing = async (deviceId: string) => {
    setSyncError(null);
    try {
      await invoke("reject_mobile_pairing", { deviceId });
      const info = await invoke<SyncServerInfo>("get_mobile_sync_server_info");
      setSyncServerInfo(info);
    } catch (err) {
      setSyncError(String(err));
    }
  };

  useEffect(() => {
    let active = true;
    getVersion()
      .then((version) => {
        if (active) {
          setAppVersion(version);
        }
      })
      .catch(() => {
        if (active) {
          setAppVersion("unknown");
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    const timer = window.setTimeout(() => {
      void resolveUpdaterTarget()
        .then((target) => check(target ? { target } : undefined))
        .then((update) => {
          if (!active || !update?.available) {
            return;
          }
          const skippedVersion = window.localStorage.getItem(UPDATE_SKIP_STORAGE_KEY);
          if (skippedVersion === update.version) {
            return;
          }
          const updateMeta = update as TauriUpdate & {
            currentVersion?: string;
            date?: string;
            body?: string;
          };
          setAvailableUpdate({
            update,
            version: update.version,
            currentVersion: updateMeta.currentVersion ?? appVersion,
            date: updateMeta.date,
            body: updateMeta.body,
          });
          setUpdateProgress({
            stage: "idle",
            downloaded: 0,
            total: null,
            percent: 0,
            message: "",
          });
        })
        .catch((error) => {
          console.error("Update check failed:", error);
        });
    }, UPDATE_CHECK_DELAY_MS);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [appVersion]);

  function dismissUpdateForNow(): void {
    if (updateProgress.stage !== "idle" && updateProgress.stage !== "error") {
      return;
    }
    setAvailableUpdate(null);
    setUpdateProgress({
      stage: "idle",
      downloaded: 0,
      total: null,
      percent: 0,
      message: "",
    });
  }

  function skipAvailableUpdate(): void {
    if (!availableUpdate || (updateProgress.stage !== "idle" && updateProgress.stage !== "error")) {
      return;
    }
    window.localStorage.setItem(UPDATE_SKIP_STORAGE_KEY, availableUpdate.version);
    setAvailableUpdate(null);
  }

  async function installAvailableUpdate(): Promise<void> {
    if (!availableUpdate || (updateProgress.stage !== "idle" && updateProgress.stage !== "error")) {
      return;
    }

    let downloaded = 0;
    let total: number | null = null;
    setUpdateProgress({
      stage: "downloading",
      downloaded: 0,
          total: null,
          percent: 0,
          message: "Preparing update...",
    });

    try {
      await availableUpdate.update.downloadAndInstall((event: TauriDownloadEvent) => {
        if (event.event === "Started") {
          total = typeof event.data?.contentLength === "number" ? event.data.contentLength : null;
          downloaded = 0;
          setUpdateProgress({
            stage: "downloading",
            downloaded,
            total,
            percent: 0,
            message: total ? "Downloading update..." : "Downloading update...",
          });
          return;
        }

        if (event.event === "Progress") {
          downloaded += event.data?.chunkLength ?? 0;
          const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : 0;
          setUpdateProgress({
            stage: "downloading",
            downloaded,
            total,
            percent,
            message: "Downloading update...",
          });
          return;
        }

        if (event.event === "Finished") {
          setUpdateProgress({
            stage: "installing",
            downloaded,
            total,
            percent: 100,
            message: "Installing update...",
          });
        }
      });

      setUpdateProgress({
        stage: "relaunching",
        downloaded,
        total,
        percent: 100,
        message: "Update installed. Relaunching OmniSearch...",
      });
      await relaunch();
    } catch (error) {
      setUpdateProgress({
        stage: "error",
        downloaded,
        total,
        percent: total ? Math.min(100, Math.round((downloaded / total) * 100)) : 0,
        message: "Update failed.",
        error: String(error),
      });
    }
  }

  useEffect(() => {
    let active = true;

    const loadDesktopBehavior = async () => {
      try {
        const settings = await getDesktopSettings();
        if (!active) {
          return;
        }
        setDesktopSettings(settings);
        setDesktopSettingsDraft(settings);
        setDesktopSettingsError(null);
      } catch (error) {
        if (!active) {
          return;
        }
        setDesktopSettings(DEFAULT_DESKTOP_SETTINGS);
        setDesktopSettingsDraft(DEFAULT_DESKTOP_SETTINGS);
        setDesktopSettingsError(`Failed to load desktop settings: ${String(error)}`);
      } finally {
        if (active) {
          setDesktopSettingsLoading(false);
        }
      }
    };

    void loadDesktopBehavior();

    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void listenForWindowMode((mode) => {
      setWindowMode(mode);
      setActiveTab("search");
    }).then((dispose) => {
      if (cancelled) {
        dispose();
        return;
      }
      unlisten = dispose;
    });

    return () => {
      cancelled = true;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  useEffect(() => {
    const scrollContainers = Array.from(
      document.querySelectorAll<HTMLElement>(".scrollable-tab-panel"),
    );
    if (scrollContainers.length === 0) {
      return;
    }

    const timeouts = new Map<HTMLElement, number>();
    const listeners = scrollContainers.map((container) => {
      container.dataset.scrollState = "idle";
      const handleScroll = () => {
        container.dataset.scrollState = "active";
        const existingTimeout = timeouts.get(container);
        if (existingTimeout !== undefined) {
          window.clearTimeout(existingTimeout);
        }
        const timeout = window.setTimeout(() => {
          container.dataset.scrollState = "idle";
          timeouts.delete(container);
        }, 720);
        timeouts.set(container, timeout);
      };

      container.addEventListener("scroll", handleScroll, { passive: true });
      return { container, handleScroll };
    });

    return () => {
      for (const { container, handleScroll } of listeners) {
        container.removeEventListener("scroll", handleScroll);
        container.dataset.scrollState = "idle";
      }
      for (const timeout of timeouts.values()) {
        window.clearTimeout(timeout);
      }
    };
  }, [activeTab, duplicateGroups.length, results.length]);

  useEffect(() => {
    window.localStorage.setItem(PREVIEW_STORAGE_KEY, showPreviews ? "1" : "0");
  }, [showPreviews]);

  useEffect(() => {
    window.localStorage.setItem(SEARCH_METRICS_HIDDEN_STORAGE_KEY, searchMetricsHidden ? "1" : "0");
  }, [searchMetricsHidden]);

  useEffect(() => {
    window.localStorage.setItem(QUICK_RESULTS_PANE_RATIO_STORAGE_KEY, String(quickResultsPaneRatio));
  }, [quickResultsPaneRatio]);

  useEffect(() => {
    window.localStorage.setItem(INCLUDE_FOLDERS_STORAGE_KEY, includeFolders ? "1" : "0");
  }, [includeFolders]);

  useEffect(() => {
    window.localStorage.setItem(INCLUDE_ALL_DRIVES_STORAGE_KEY, includeAllDrives ? "1" : "0");
  }, [includeAllDrives]);

  useEffect(() => {
    window.localStorage.setItem(INCLUDE_INSTALLED_APPS_STORAGE_KEY, includeInstalledApps ? "1" : "0");
  }, [includeInstalledApps]);

  useEffect(() => {
    window.localStorage.setItem(SEARCH_LIMIT_STORAGE_KEY, String(defaultSearchLimit));
  }, [defaultSearchLimit]);

  useEffect(() => {
    window.localStorage.setItem(
      RECENT_ACTIVITY_ENABLED_STORAGE_KEY,
      recentActivityEnabled ? "1" : "0",
    );
  }, [recentActivityEnabled]);

  useEffect(() => {
    window.localStorage.setItem(RECENT_ACTIVITY_STORAGE_KEY, JSON.stringify(recentActivity));
  }, [recentActivity]);

  useEffect(() => {
    if (!includeInstalledApps) {
      setInstalledAppsError(null);
      return;
    }

    let active = true;
    setInstalledAppsLoading(true);

    void invoke<InstalledApp[]>("list_installed_apps")
      .then((found) => {
        if (!active) {
          return;
        }
        setInstalledApps(found);
        setInstalledAppsError(null);
      })
      .catch((error) => {
        if (!active) {
          return;
        }
        setInstalledApps([]);
        setInstalledAppsError(`Failed to load installed apps: ${String(error)}`);
      })
      .finally(() => {
        if (active) {
          setInstalledAppsLoading(false);
        }
      });

    return () => {
      active = false;
    };
  }, [includeInstalledApps, installedAppsRefreshKey]);

  useEffect(() => {
    const syncViewportSize = () => {
      setViewportSize((current) => {
        const next = readViewportSize();
        if (current.width === next.width && current.height === next.height) {
          return current;
        }
        return next;
      });
    };

    syncViewportSize();
    window.addEventListener("resize", syncViewportSize);
    return () => {
      window.removeEventListener("resize", syncViewportSize);
    };
  }, []);

  useEffect(() => {
    setSearchLimitInput(String(defaultSearchLimit));
  }, [defaultSearchLimit]);

  useEffect(() => {
    const previousQuery = previousSearchQueryRef.current;
    if (
      activeTab === "search" &&
      recentActivityEnabled &&
      previousQuery.length > 0 &&
      trimmedQuery.length === 0
    ) {
      setRecentActivity((previous) => {
        const existingEntry = previous.find(
          (entry) => recentActivityEntryKey(entry) === `query:${previousQuery.trim().toLowerCase()}`,
        );
        const nextEntry: RecentActivityQueryEntry = {
          kind: "query",
          query: previousQuery,
          usedAt: Date.now(),
          pinned: existingEntry?.kind === "query" ? existingEntry.pinned : false,
        };
        const next = [
          nextEntry,
          ...previous.filter(
            (entry) => recentActivityEntryKey(entry) !== recentActivityEntryKey(nextEntry),
          ),
        ];
        return sortRecentActivityEntries(next).slice(0, RECENT_ACTIVITY_LIMIT);
      });
    }
    previousSearchQueryRef.current = trimmedQuery;
  }, [activeTab, isQuickMode, recentActivityEnabled, trimmedQuery]);

  useEffect(() => {
    if (visibleResults.length === 0) {
      setSelectedResultKey(null);
      return;
    }

    if (!selectedResultKey) {
      setSelectedResultKey(rowKeyForResult(visibleResults[0]));
      return;
    }

    const hasSelection = visibleResults.some((result) => rowKeyForResult(result) === selectedResultKey);
    if (!hasSelection) {
      setSelectedResultKey(rowKeyForResult(visibleResults[0]));
    }
  }, [selectedResultKey, visibleResults]);

  useEffect(() => {
    setSearchResultContextMenu(null);
    if (activeTab !== "search") {
      setSearchResultRenameDraft(null);
      setSearchResultDeleteCandidate(null);
    }
  }, [activeTab, results]);

  useEffect(() => {
    if (!searchResultContextMenu) {
      return;
    }

    const dismissContextMenu = () => {
      setSearchResultContextMenu(null);
    };

    const handlePointerDown = (event: MouseEvent) => {
      const menu = searchResultContextMenuRef.current;
      if (menu && menu.contains(event.target as Node)) {
        return;
      }
      dismissContextMenu();
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        dismissContextMenu();
      }
    };

    window.addEventListener("mousedown", handlePointerDown, true);
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("resize", dismissContextMenu);
    window.addEventListener("scroll", dismissContextMenu, true);

    return () => {
      window.removeEventListener("mousedown", handlePointerDown, true);
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("resize", dismissContextMenu);
      window.removeEventListener("scroll", dismissContextMenu, true);
    };
  }, [searchResultContextMenu]);

  useEffect(() => {
    if (!searchResultRenameDraft) {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      const input = searchResultRenameInputRef.current;
      if (!input) {
        return;
      }
      input.focus();
      input.select();
    });

    return () => {
      window.cancelAnimationFrame(frame);
    };
  }, [searchResultRenameDraft]);

  useEffect(() => {
    if (!duplicateDeleteCandidate) {
      setDuplicateDeleteToRecycleBin(false);
    }
  }, [duplicateDeleteCandidate]);

  useEffect(() => {
    if (!searchResultDeleteCandidate) {
      setSearchResultDeleteToRecycleBin(false);
    }
  }, [searchResultDeleteCandidate]);

  useEffect(() => {
    setSearchLimit(defaultSearchLimit);
  }, [trimmedQuery, extension, minSizeMb, maxSizeMb, createdAfter, createdBefore, hasFilters, defaultSearchLimit]);

  // Multi-selection helper functions
  function toggleBulkSelection(result: SearchResult): void {
    if (result.resultKind === "app" || isAppSearchResult(result)) {
      return; // Installed apps cannot be bulk selected
    }
    setSelectedPathsForBulk((prev) => {
      const next = new Set(prev);
      if (next.has(result.path)) {
        next.delete(result.path);
      } else {
        next.add(result.path);
      }
      return next;
    });
  }

  function handleRangeSelection(clickedResult: SearchResult): void {
    if (clickedResult.resultKind === "app" || isAppSearchResult(clickedResult)) {
      return; // Installed apps cannot be bulk selected
    }
    if (visibleResults.length === 0) return;

    const activePath = selectedResult?.path;
    if (!activePath) {
      toggleBulkSelection(clickedResult);
      return;
    }

    const clickedIndex = visibleResults.findIndex((r) => r.path === clickedResult.path);
    const activeIndex = visibleResults.findIndex((r) => r.path === activePath);

    if (clickedIndex === -1 || activeIndex === -1) {
      toggleBulkSelection(clickedResult);
      return;
    }

    const start = Math.min(activeIndex, clickedIndex);
    const end = Math.max(activeIndex, clickedIndex);

    // Filter out folders and apps if we shouldn't bulk select them (folders are fine, apps are not)
    const pathsToSelect = visibleResults
      .slice(start, end + 1)
      .filter((r) => r.resultKind !== "app" && !isAppSearchResult(r))
      .map((r) => r.path);

    setSelectedPathsForBulk((prev) => {
      const next = new Set(prev);
      const isChecking = !prev.has(clickedResult.path);
      for (const p of pathsToSelect) {
        if (isChecking) {
          next.add(p);
        } else {
          next.delete(p);
        }
      }
      return next;
    });
  }

  function startLongPress(result: SearchResult): void {
    if (result.resultKind === "app" || isAppSearchResult(result)) {
      return;
    }
    longPressTriggeredRef.current = false;
    if (longPressTimeoutRef.current !== null) {
      window.clearTimeout(longPressTimeoutRef.current);
    }
    longPressTimeoutRef.current = window.setTimeout(() => {
      setIsSelectionModeActive(true);
      setSelectedPathsForBulk((prev) => {
        const next = new Set(prev);
        next.add(result.path);
        return next;
      });
      longPressTriggeredRef.current = true;
      longPressTimeoutRef.current = null;
    }, 500);
  }

  function cancelLongPress(): void {
    if (longPressTimeoutRef.current !== null) {
      window.clearTimeout(longPressTimeoutRef.current);
      longPressTimeoutRef.current = null;
    }
  }

  useEffect(() => {
    if (isSelectionModeActive && selectedPathsForBulk.size === 0) {
      setIsSelectionModeActive(false);
    }
  }, [selectedPathsForBulk, isSelectionModeActive]);

  useEffect(() => {
    if (!isSelectionModeActive) {
      return;
    }

    const handleGlobalKeyDown = (event: KeyboardEvent) => {
      if (isEditableTarget(event.target)) {
        return;
      }
      if (event.key === "Escape") {
        setIsSelectionModeActive(false);
        setSelectedPathsForBulk(new Set<string>());
      }
    };

    window.addEventListener("keydown", handleGlobalKeyDown);
    return () => {
      window.removeEventListener("keydown", handleGlobalKeyDown);
    };
  }, [isSelectionModeActive]);

  // Bulk operations helper functions
  async function handleBulkCopyPaths(): Promise<void> {
    if (selectedPathsForBulk.size === 0) return;
    const pathsArray = Array.from(selectedPathsForBulk);
    const joinedPaths = pathsArray.join("\n");
    try {
      await copyTextToClipboard(joinedPaths);
      showActionNotice(`Copied ${pathsArray.length} paths to clipboard.`);
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to copy paths: ${String(error)}`);
    }
  }

  async function handleBulkSendToPhone(): Promise<void> {
    if (selectedPathsForBulk.size === 0) return;

    const validResults = visibleResults.filter(
      (r) => selectedPathsForBulk.has(r.path) && !r.isDirectory && r.resultKind !== "app" && !isAppSearchResult(r)
    );

    if (validResults.length === 0) {
      setActionError("No valid files selected to send to phone.");
      return;
    }

    setIsSelectionModeActive(false);
    setSelectedPathsForBulk(new Set<string>());
    setActiveTab("sync");
    setActionError(null);

    setBulkActionProgress({
      current: 0,
      total: validResults.length,
      label: "Sending files to phone..."
    });

    let successCount = 0;
    let failCount = 0;
    const errorMessages: string[] = [];

    for (let i = 0; i < validResults.length; i++) {
      const result = validResults[i];
      setBulkActionProgress({
        current: i,
        total: validResults.length,
        label: `Sending "${result.name}" to phone...`
      });

      try {
        await invoke("send_file_to_mobile", { path: result.path });
        successCount++;
      } catch (error) {
        failCount++;
        errorMessages.push(`Failed to send "${result.name}": ${String(error)}`);
      }
    }

    setBulkActionProgress(null);

    try {
      const info = await invoke<SyncServerInfo>("get_mobile_sync_server_info");
      setSyncServerInfo(info);
    } catch (error) {
      console.error("Failed to update sync server info:", error);
    }

    if (successCount > 0) {
      showActionNotice(`Queued ${successCount} files for phone.`);
    }
    if (failCount > 0) {
      setActionError(errorMessages[0] || "Failed to send some files to phone.");
    }
  }

  async function confirmBulkDelete(): Promise<void> {
    if (selectedPathsForBulk.size === 0 || bulkDeleteBusy) return;

    setBulkDeleteBusy(true);
    const pathsArray = Array.from(selectedPathsForBulk);

    setBulkActionProgress({
      current: 0,
      total: pathsArray.length,
      label: "Deleting files..."
    });

    let deletedCount = 0;
    let failCount = 0;
    const errorMessages: string[] = [];

    for (let i = 0; i < pathsArray.length; i++) {
      const path = pathsArray[i];
      setBulkActionProgress({
        current: i,
        total: pathsArray.length,
        label: `Deleting "${path.split(/[\/\\]/).pop()}"...`
      });

      try {
        const deleted = await invoke<boolean>("delete_path", {
          path,
          recycleBin: bulkDeleteRecycleBin,
          recycle_bin: bulkDeleteRecycleBin
        });
        if (deleted) {
          removeSearchResultFromState(path);
          deletedCount++;
        } else {
          failCount++;
          errorMessages.push(`Could not delete "${path}"`);
        }
      } catch (error) {
        failCount++;
        errorMessages.push(`Failed to delete "${path}": ${String(error)}`);
      }
    }

    setBulkActionProgress(null);
    setBulkDeleteOpen(false);
    setBulkDeleteBusy(false);

    setIsSelectionModeActive(false);
    setSelectedPathsForBulk(new Set<string>());

    if (deletedCount > 0) {
      showActionNotice(`Deleted ${deletedCount} items.`);
    }
    if (failCount > 0) {
      setActionError(errorMessages[0] || "Failed to delete some items.");
    }
  }

  async function confirmBulkRename(): Promise<void> {
    if (selectedPathsForBulk.size === 0 || bulkRenameBusy) return;

    const selectedResults = visibleResults.filter((r) => selectedPathsForBulk.has(r.path));
    if (selectedResults.length === 0) return;

    if (bulkRenameMode === "replace" && !bulkRenameReplaceSearch) {
      setActionError("Rename failed: search text cannot be empty.");
      return;
    }
    if (bulkRenameMode === "enumerate" && !bulkRenameEnumerateBase.trim()) {
      setActionError("Rename failed: base name cannot be empty.");
      return;
    }

    setBulkRenameBusy(true);
    setActionError(null);

    setBulkActionProgress({
      current: 0,
      total: selectedResults.length,
      label: "Renaming files..."
    });

    let renamedCount = 0;
    let failCount = 0;
    const errorMessages: string[] = [];

    const formatNumber = (num: number, digits: number): string => {
      let s = String(num);
      while (s.length < digits) {
        s = "0" + s;
      }
      return s;
    };

    for (let i = 0; i < selectedResults.length; i++) {
      const result = selectedResults[i];
      let newName = "";

      const ext = result.extension ? `.${result.extension}` : "";
      const baseName = result.extension && result.name.endsWith(ext)
        ? result.name.slice(0, -ext.length)
        : result.name;

      if (bulkRenameMode === "replace") {
        const searchEscaped = bulkRenameReplaceSearch.replace(/[-\/\\^$*+?.()|[\]{}]/g, "\\$&");
        const regex = new RegExp(searchEscaped, "gi");
        newName = result.name.replace(regex, bulkRenameReplaceWith);
      } else if (bulkRenameMode === "prefix_suffix") {
        newName = `${bulkRenamePrefix}${baseName}${bulkRenameSuffix}${ext}`;
      } else if (bulkRenameMode === "enumerate") {
        const numStr = formatNumber(bulkRenameEnumerateStart + i, bulkRenameEnumerateDigits);
        newName = `${bulkRenameEnumerateBase.trim()}${numStr}${ext}`;
      }

      newName = newName.trim();
      if (!newName || newName === result.name) {
        renamedCount++;
        continue;
      }

      setBulkActionProgress({
        current: i,
        total: selectedResults.length,
        label: `Renaming "${result.name}" to "${newName}"...`
      });

      try {
        const nextPath = await invoke<string>("rename_path", {
          path: result.path,
          newName,
          new_name: newName
        });
        renameSearchResultInState(result.path, nextPath, newName);
        renamedCount++;
      } catch (error) {
        failCount++;
        errorMessages.push(`Failed to rename "${result.name}": ${String(error)}`);
      }
    }

    setBulkActionProgress(null);
    setBulkRenameOpen(false);
    setBulkRenameBusy(false);

    setIsSelectionModeActive(false);
    setSelectedPathsForBulk(new Set<string>());

    if (renamedCount > 0) {
      showActionNotice(`Renamed items.`);
    }
    if (failCount > 0) {
      setActionError(errorMessages[0] || "Failed to rename some items.");
    }
  }

  useEffect(() => {
    setPreviewSourceState({});
    setPreviewReadyState({});
    setPreviewDataUrls({});
    setTextPreviewState({});
    setTextPreviewLoadingState({});
  }, [results, showPreviews]);

  useEffect(() => {
    const selectedKey =
      (showPreviews || quickLookOpen) && selectedResult ? rowKeyForResult(selectedResult) : "";

    setPreviewDataUrls((previous) => {
      if (!selectedKey) {
        return Object.keys(previous).length === 0 ? previous : {};
      }

      const current = previous[selectedKey];
      if (!current) {
        return Object.keys(previous).length === 0 ? previous : {};
      }

      return Object.keys(previous).length === 1 && previous[selectedKey]
        ? previous
        : { [selectedKey]: current };
    });
  }, [quickLookOpen, selectedResult, showPreviews]);

  useEffect(() => {
    setSelectedPreviewSourceIndex(0);
    setSelectedPreviewReadyState({});
  }, [quickLookOpen, selectedResultKey, showPreviews]);

  useEffect(() => {
    if (!showPreviews || previewLoadCandidates.length === 0) {
      return;
    }

    const candidates = previewLoadCandidates.filter((result) =>
      isAssetPreviewKind(previewKindFromResult(result)),
    );
    if (candidates.length === 0) {
      return;
    }

    const missing = candidates.filter((result) => !previewDataUrls[rowKeyForResult(result)]);
    if (missing.length === 0) {
      return;
    }

    let cancelled = false;
    const load = async () => {
      for (const result of missing) {
        if (cancelled) {
          return;
        }
        try {
          const dataUrl = await invoke<string>("load_preview_data_url", { path: result.path });
          if (cancelled || !dataUrl || !dataUrl.startsWith("data:")) {
            continue;
          }
          const rowKey = rowKeyForResult(result);
          setPreviewDataUrls((previous) => {
            if (previous[rowKey]) {
              return previous;
            }
            return {
              ...previous,
              [rowKey]: dataUrl,
            };
          });
        } catch {
          // Ignore per-file preview generation failures; UI will fallback gracefully.
        }
      }
    };

    void load();

    return () => {
      cancelled = true;
    };
  }, [showPreviews, previewDataUrls, previewLoadCandidates]);

  useEffect(() => {
    if ((!showPreviews && !quickLookOpen) || !selectedResult) {
      return;
    }
    if (previewKindFromResult(selectedResult) !== "text") {
      return;
    }

    const cacheKey = textPreviewCacheKey(
      selectedResult,
      contentSearchQuery,
      contentSearchMode,
      TEXT_PREVIEW_MAX_CHARS,
    );

    if (textPreviewState[cacheKey] || textPreviewLoadingState[cacheKey]) {
      return;
    }

    let cancelled = false;
    setTextPreviewLoadingState((previous) => ({ ...previous, [cacheKey]: true }));

    void loadTextPreviewViaTauri(
      selectedResult.path,
      TEXT_PREVIEW_MAX_CHARS,
      contentSearchQuery || null,
      contentSearchMode,
    )
      .then((payload) => {
        if (cancelled) {
          return;
        }
        setTextPreviewState((previous) => ({ ...previous, [cacheKey]: payload }));
      })
      .catch(async () => {
        try {
          const payload = await loadTextPreviewFallback(
            selectedResult.path,
            TEXT_PREVIEW_MAX_CHARS,
            contentSearchQuery,
            contentSearchMode,
          );
          if (!cancelled) {
            setTextPreviewState((previous) => ({ ...previous, [cacheKey]: payload }));
          }
        } catch {
          if (!cancelled) {
            setTextPreviewState((previous) => ({
              ...previous,
              [cacheKey]: { text: "", truncated: false, matched: false },
            }));
          }
        }
      })
      .finally(() => {
        setTextPreviewLoadingState((previous) => {
          if (!previous[cacheKey]) {
            return previous;
          }
          const next = { ...previous };
          delete next[cacheKey];
          return next;
        });
      });

    return () => {
      cancelled = true;
    };
  }, [
    contentSearchMode,
    contentSearchQuery,
    quickLookOpen,
    selectedResult,
    showPreviews,
  ]);

  useEffect(() => {
    if (!hasContentSearchSyntax || !contentSearchQuery || contentSnippetCandidates.length === 0) {
      return;
    }

    const pendingCandidates = contentSnippetCandidates.filter((result) => {
      const cacheKey = textPreviewCacheKey(
        result,
        contentSearchQuery,
        contentSearchMode,
        CONTENT_SNIPPET_MAX_CHARS,
      );
      return !textPreviewState[cacheKey] && !textPreviewLoadingState[cacheKey];
    });

    if (pendingCandidates.length === 0) {
      return;
    }

    let cancelled = false;

    const load = async () => {
      for (const result of pendingCandidates) {
        const cacheKey = textPreviewCacheKey(
          result,
          contentSearchQuery,
          contentSearchMode,
          CONTENT_SNIPPET_MAX_CHARS,
        );

        setTextPreviewLoadingState((previous) => ({ ...previous, [cacheKey]: true }));

        try {
          let payload: TextPreviewPayload;
          try {
            payload = await loadTextPreviewViaTauri(
              result.path,
              CONTENT_SNIPPET_MAX_CHARS,
              contentSearchQuery,
              contentSearchMode,
            );
          } catch {
            payload = await loadTextPreviewFallback(
              result.path,
              CONTENT_SNIPPET_MAX_CHARS,
              contentSearchQuery,
              contentSearchMode,
            );
          }

          if (!cancelled && payload.matched) {
            setTextPreviewState((previous) => ({ ...previous, [cacheKey]: payload }));
          }
        } catch {
          // Ignore per-result text snippet failures.
        } finally {
          setTextPreviewLoadingState((previous) => {
            if (!previous[cacheKey]) {
              return previous;
            }
            const next = { ...previous };
            delete next[cacheKey];
            return next;
          });
        }
      }
    };

    void load();

    return () => {
      cancelled = true;
    };
  }, [
    contentSearchMode,
    contentSearchQuery,
    contentSnippetCandidates,
    hasContentSearchSyntax,
  ]);

  useEffect(() => {
    if (activeTab !== "search") {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      if (searchInputRef.current) {
        searchInputRef.current.focus();
      }
    });

    return () => {
      window.cancelAnimationFrame(frame);
    };
  }, [activeTab, windowMode]);

  useEffect(() => {
    let active = true;

    const loadDrives = async () => {
      try {
        const available = await invoke<DriveInfo[]>("list_drives");
        if (!active) {
          return;
        }
        setDrives(available);
        const preferred =
          available.find((drive) => drive.letter === "C" && drive.isNtfs && drive.canOpenVolume) ??
          available.find((drive) => drive.isNtfs && drive.canOpenVolume) ??
          available.find((drive) => drive.isNtfs) ??
          available[0];
        if (preferred) {
          setSelectedDrive(preferred.letter);
          setDriveError(null);
        } else {
          setDriveError("No available drives were found.");
        }
      } catch (error) {
        if (active) {
          setDriveError(`Failed to load drives: ${String(error)}`);
        }
      }
    };

    void loadDrives();

    const poll = window.setInterval(() => {
      void invoke<IndexStatus>("index_status")
        .then((next) => {
          if (active) {
            setStatus(next);
          }
        })
        .catch((error) => {
          if (active) {
            setStatus((previous) => ({
              ...previous,
              lastError: String(error),
            }));
          }
        });
    }, POLL_INTERVAL_MS);

    return () => {
      active = false;
      window.clearInterval(poll);
    };
  }, []);

  useEffect(() => {
    if (!selectedDrive) {
      return;
    }
    if (
      requestedIndexConfigKey === appliedIndexConfigKey ||
      requestedIndexConfigKey === pendingIndexConfigKey
    ) {
      return;
    }

    let active = true;
    setPendingIndexConfigKey(requestedIndexConfigKey);
    const beginIndexing = async () => {
      try {
        const initial = await invoke<IndexStatus>("start_indexing", {
          drive: selectedDrive,
          includeFolders: includeFolders,
          include_folders: includeFolders,
          includeAllDrives: includeAllDrives,
          include_all_drives: includeAllDrives,
        });
        if (active) {
          setStatus(initial);
          setAppliedIndexConfigKey(requestedIndexConfigKey);
          setPendingIndexConfigKey("");
        }
      } catch (error) {
        if (active) {
          setStatus((previous) => ({
            ...previous,
            lastError: String(error),
          }));
          setPendingIndexConfigKey("");
        }
      }
    };
    void beginIndexing();

    return () => {
      active = false;
    };
  }, [
    selectedDrive,
    includeFolders,
    includeAllDrives,
    requestedIndexConfigKey,
    appliedIndexConfigKey,
    pendingIndexConfigKey,
  ]);

  useEffect(() => {
    setDuplicateGroups([]);
    setDuplicatesError(null);
    setDuplicatesLoading(false);
    setDuplicateScanStatus({
      running: false,
      cancelRequested: false,
      scannedFiles: 0,
      totalFiles: 0,
      groupsFound: 0,
      progressPercent: 0,
    });
  }, [selectedDrive, includeFolders, includeAllDrives]);

  useEffect(() => {
    if (!duplicatesLoading) {
      return;
    }

    let active = true;
    const pollStatus = async () => {
      try {
        const next = await invoke<DuplicateScanStatus>("duplicate_scan_status");
        if (active) {
          setDuplicateScanStatus(next);
        }
      } catch {
        // Best effort polling only; ignore intermittent status read errors.
      }
    };

    void pollStatus();
    const poll = window.setInterval(() => {
      void pollStatus();
    }, 220);

    return () => {
      active = false;
      window.clearInterval(poll);
    };
  }, [duplicatesLoading]);

  useEffect(() => {
    if (!status.ready) {
      previousIndexedCountRef.current = status.indexedCount;
      setIndexSyncing(false);
      if (indexSyncTimeoutRef.current !== null) {
        window.clearTimeout(indexSyncTimeoutRef.current);
        indexSyncTimeoutRef.current = null;
      }
      return;
    }

    const previousCount = previousIndexedCountRef.current;
    if (
      previousCount !== null &&
      status.indexedCount !== previousCount &&
      !status.indexing
    ) {
      setIndexSyncing(true);
      if (indexSyncTimeoutRef.current !== null) {
        window.clearTimeout(indexSyncTimeoutRef.current);
      }
      indexSyncTimeoutRef.current = window.setTimeout(() => {
        setIndexSyncing(false);
        indexSyncTimeoutRef.current = null;
      }, 1600);
    }
    previousIndexedCountRef.current = status.indexedCount;
  }, [status.ready, status.indexing, status.indexedCount]);

  useEffect(() => {
    if (actionError) {
      setActionNotice(null);
    }
  }, [actionError]);

  useEffect(() => {
    return () => {
      if (indexSyncTimeoutRef.current !== null) {
        window.clearTimeout(indexSyncTimeoutRef.current);
      }
      if (duplicateNoticeTimeoutRef.current !== null) {
        window.clearTimeout(duplicateNoticeTimeoutRef.current);
      }
      if (actionNoticeTimeoutRef.current !== null) {
        window.clearTimeout(actionNoticeTimeoutRef.current);
      }
      if (quickLookCopyTimeoutRef.current !== null) {
        window.clearTimeout(quickLookCopyTimeoutRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!trimmedQuery && !hasFilters) {
      cancelSearchRequest();
      setResults([]);
      setSearchError(null);
      setActionError(null);
      setLoading(false);
      return;
    }

    let active = true;
    const debounceMs = hasContentSearchSyntax
      ? CONTENT_SEARCH_DEBOUNCE_MS
      : hasMetadataFilters
        ? FILTER_SEARCH_DEBOUNCE_MS
        : SEARCH_DEBOUNCE_MS;
    const timer = window.setTimeout(() => {
      const runSearch = async () => {
        setLoading(true);
        const minSize = toBytesFromMb(minSizeMb);
        const maxSize = toBytesFromMb(maxSizeMb);
        const minCreatedUnix = toUnixStart(createdAfter);
        const maxCreatedUnix = toUnixEnd(createdBefore);

        try {
          cancelSearchRequest();
          const searchArgs = {
            query: trimmedQuery,
            extension: extension.trim(),
            minSize,
            min_size: minSize,
            maxSize,
            max_size: maxSize,
            minCreatedUnix,
            min_created_unix: minCreatedUnix,
            maxCreatedUnix,
            max_created_unix: maxCreatedUnix,
            limit: searchLimit,
          };
          const found = await invoke<SearchResult[]>("search_files", {
            ...searchArgs,
          });
          if (active) {
            setResults(found.map(normalizeFileSearchResult));
            setSearchError(null);
          }
        } catch (error) {
          if (active) {
            setResults([]);
            setSearchError(String(error));
          }
        } finally {
          if (active) {
            setLoading(false);
          }
        }
      };

      void runSearch();
    }, debounceMs);

    return () => {
      active = false;
      window.clearTimeout(timer);
      cancelSearchRequest();
    };
  }, [
    trimmedQuery,
    extension,
    minSizeMb,
    maxSizeMb,
    createdAfter,
    createdBefore,
    hasFilters,
    hasContentSearchSyntax,
    hasMetadataFilters,
    searchLimit,
  ]);

  async function reindexWithConfig(
    nextIncludeFolders: boolean,
    nextIncludeAllDrives: boolean,
  ): Promise<void> {
    if (!selectedDrive) {
      return;
    }
    const nextConfigKey = nextIncludeAllDrives
      ? `ALL:${nextIncludeFolders ? "1" : "0"}`
      : `${selectedDrive}:${nextIncludeFolders ? "1" : "0"}`;

    try {
      setPendingIndexConfigKey(nextConfigKey);
      const next = await invoke<IndexStatus>("start_indexing", {
        drive: selectedDrive,
        includeFolders: nextIncludeFolders,
        include_folders: nextIncludeFolders,
        includeAllDrives: nextIncludeAllDrives,
        include_all_drives: nextIncludeAllDrives,
      });
      setStatus(next);
      setAppliedIndexConfigKey(nextConfigKey);
      setPendingIndexConfigKey("");
    } catch (error) {
      setStatus((previous) => ({
        ...previous,
        lastError: String(error),
      }));
      setPendingIndexConfigKey("");
    }
  }

  async function reindex(): Promise<void> {
    await reindexWithConfig(includeFolders, includeAllDrives);
  }

  function clearSearchFilters(): void {
    setExtension("");
    setMinSizeMb("");
    setMaxSizeMb("");
    setCreatedAfter("");
    setCreatedBefore("");
  }

  function loadMoreResults(): void {
    setSearchLimit((previous) =>
      Math.min(SEARCH_LIMIT_MAX, previous + Math.max(defaultSearchLimit, SEARCH_LIMIT_MIN)),
    );
  }

  function applySearchLimitPreference(): void {
    const parsed = Number(searchLimitInput.trim());
    if (!Number.isFinite(parsed) || parsed <= 0) {
      setSearchLimitError(`Enter a valid number between ${SEARCH_LIMIT_MIN} and ${SEARCH_LIMIT_MAX}.`);
      setSearchLimitMessage(null);
      return;
    }

    const normalized = normalizeSearchLimit(parsed);
    setDefaultSearchLimit(normalized);
    setSearchLimit(normalized);
    setSearchLimitInput(String(normalized));
    setSearchLimitError(null);
    if (normalized !== parsed) {
      setSearchLimitMessage(
        `Saved. Adjusted to ${normalized.toLocaleString()} to keep the allowed range.`,
      );
      return;
    }
    setSearchLimitMessage(`Saved. New searches now start with ${normalized.toLocaleString()} results.`);
  }

  function resetSearchLimitPreference(): void {
    setDefaultSearchLimit(SEARCH_LIMIT);
    setSearchLimit(SEARCH_LIMIT);
    setSearchLimitInput(String(SEARCH_LIMIT));
    setSearchLimitError(null);
    setSearchLimitMessage(`Reset to default: ${SEARCH_LIMIT.toLocaleString()} results.`);
  }

  async function saveDesktopBehaviorSettings(): Promise<void> {
    const normalizedShortcut =
      desktopSettingsDraft.shortcut.trim() || DEFAULT_DESKTOP_SETTINGS.shortcut;
    const nextSettings: DesktopSettings = {
      backgroundModeEnabled: desktopSettingsDraft.backgroundModeEnabled,
      shortcutEnabled: desktopSettingsDraft.shortcutEnabled,
      rememberWindowBounds: desktopSettingsDraft.rememberWindowBounds,
      shortcut: normalizedShortcut,
    };

    setDesktopSettingsSaving(true);
    setDesktopSettingsError(null);
    setDesktopSettingsMessage(null);

    try {
      const savedSettings = await updateDesktopSettings(nextSettings);
      setDesktopSettings(savedSettings);
      setDesktopSettingsDraft(savedSettings);
      setDesktopSettingsMessage(
        savedSettings.shortcutEnabled
          ? `Saved. ${savedSettings.shortcut} is active immediately.`
          : "Saved. The global shortcut is now disabled.",
      );
    } catch (error) {
      setDesktopSettingsError(`Failed to save desktop settings: ${String(error)}`);
    } finally {
      setDesktopSettingsSaving(false);
    }
  }

  async function resetDesktopWindowLayout(): Promise<void> {
    setDesktopLayoutResetting(true);
    setDesktopSettingsError(null);
    setDesktopSettingsMessage(null);

    try {
      await resetWindowLayout();
      setDesktopSettingsMessage(
        desktopSettingsDraft.rememberWindowBounds
          ? "Reset. Full workspace layout returned to its default size and position."
          : "Reset. The saved full workspace layout was cleared.",
      );
    } catch (error) {
      setDesktopSettingsError(`Failed to reset the window layout: ${String(error)}`);
    } finally {
      setDesktopLayoutResetting(false);
    }
  }

  async function findDuplicates(): Promise<void> {
    if (duplicatesLoading || duplicateScanStatus.running) {
      return;
    }

    if (!status.ready) {
      setDuplicatesError("Index is not ready yet. Wait for indexing to finish.");
      return;
    }

    setDuplicateGroups([]);
    setDuplicatesError(null);
    setDuplicateNotice(null);
    setDuplicatesLoading(true);
    setDuplicateScanStatus({
      running: true,
      cancelRequested: false,
      scannedFiles: 0,
      totalFiles: 0,
      groupsFound: 0,
      progressPercent: 0,
    });
    const minSize = toBytesFromMb(duplicateMinSizeMb) ?? 50 * 1024 * 1024;

    try {
      const groups = await invoke<DuplicateGroup[]>("find_duplicate_groups", {
        minSize,
        min_size: minSize,
        maxGroups: 250,
        max_groups: 250,
        maxFilesPerGroup: 40,
        max_files_per_group: 40,
      });
      setDuplicateGroups(groups);
      setDuplicatesError(null);
    } catch (error) {
      setDuplicateGroups([]);
      const message = String(error);
      if (message.toLowerCase().includes("cancel")) {
        showDuplicateNotice(DUPLICATE_CANCEL_MESSAGE);
        setDuplicatesError(null);
      } else {
        setDuplicatesError(message);
      }
    } finally {
      try {
        const finalStatus = await invoke<DuplicateScanStatus>("duplicate_scan_status");
        setDuplicateScanStatus(finalStatus);
      } catch {
        setDuplicateScanStatus((previous) => ({
          ...previous,
          running: false,
          cancelRequested: false,
        }));
      }
      setDuplicatesLoading(false);
    }
  }

  async function cancelDuplicateScan(): Promise<void> {
    try {
      const requested = await invoke<boolean>("cancel_duplicate_scan");
      if (requested) {
        setDuplicatesError(null);
        setDuplicateNotice(null);
        setDuplicateScanStatus((previous) => ({
          ...previous,
          cancelRequested: true,
        }));
      }
    } catch (error) {
      setDuplicatesError(`Failed to cancel duplicate scan: ${String(error)}`);
    }
  }

  function removeDuplicateFromState(groupId: string, path: string): void {
    setDuplicateGroups((previous) =>
      previous
        .map((group) => {
          if (group.groupId !== groupId) {
            return group;
          }
          const nextFiles = group.files.filter(
            (file) => stripInvisibleText(file.path).trim() !== path,
          );
          if (nextFiles.length === group.files.length) {
            return group;
          }
          const nextFileCount = Math.max(0, group.fileCount - 1);
          const nextTotalBytes =
            group.totalBytes >= group.size ? group.totalBytes - group.size : group.totalBytes;
          return {
            ...group,
            fileCount: nextFileCount,
            totalBytes: nextTotalBytes,
            files: nextFiles,
          };
        })
        .filter((group) => group.fileCount >= 2),
    );
  }

  async function confirmDuplicateDelete(): Promise<void> {
    if (!duplicateDeleteCandidate || duplicateDeleteBusy) {
      return;
    }
    setDuplicateDeleteBusy(true);
    try {
      const deleted = await invoke<boolean>("delete_path", {
        path: duplicateDeleteCandidate.path,
        recycleBin: duplicateDeleteToRecycleBin,
        recycle_bin: duplicateDeleteToRecycleBin,
      });
      if (deleted) {
        removeDuplicateFromState(
          duplicateDeleteCandidate.groupId,
          duplicateDeleteCandidate.path,
        );
        setDuplicateDeleteCandidate(null);
        setDuplicatesError(null);
      }
    } catch (error) {
      setDuplicatesError(`Failed to delete item: ${String(error)}`);
    } finally {
      setDuplicateDeleteBusy(false);
    }
  }

  function clearDuplicateResults(): void {
    setDuplicateGroups([]);
    setDuplicatesError(null);
    setDuplicateNotice(null);
    setDuplicateScanStatus({
      running: false,
      cancelRequested: false,
      scannedFiles: 0,
      totalFiles: 0,
      groupsFound: 0,
      progressPercent: 0,
    });
  }

  function showDuplicateNotice(message: string): void {
    setDuplicateNotice(message);
    if (duplicateNoticeTimeoutRef.current !== null) {
      window.clearTimeout(duplicateNoticeTimeoutRef.current);
    }
    duplicateNoticeTimeoutRef.current = window.setTimeout(() => {
      setDuplicateNotice(null);
      duplicateNoticeTimeoutRef.current = null;
    }, DUPLICATE_NOTICE_TIMEOUT_MS);
  }

  function closeSearchResultContextMenu(): void {
    setSearchResultContextMenu(null);
  }

  function removeSearchResultFromState(path: string): void {
    setResults((previous) => previous.filter((result) => result.path !== path));
    setPreviewSourceState({});
    setPreviewReadyState({});
    setPreviewDataUrls({});
  }

  function renameSearchResultInState(oldPath: string, nextPath: string, nextName: string): void {
    let nextSelectedKey: string | null = null;

    setResults((previous) =>
      previous.map((result) => {
        if (result.path !== oldPath) {
          return result;
        }
        const updatedResult: SearchResult = {
          ...result,
          name: nextName,
          path: nextPath,
          extension: extensionFromName(nextName),
        };
        nextSelectedKey = rowKeyForResult(updatedResult);
        return updatedResult;
      }),
    );

    if (nextSelectedKey) {
      setSelectedResultKey(nextSelectedKey);
    }
    setPreviewSourceState({});
    setPreviewReadyState({});
    setPreviewDataUrls({});
  }

  function showActionNotice(message: string): void {
    setActionNotice(message);
    if (actionNoticeTimeoutRef.current !== null) {
      window.clearTimeout(actionNoticeTimeoutRef.current);
    }
    actionNoticeTimeoutRef.current = window.setTimeout(() => {
      setActionNotice(null);
      actionNoticeTimeoutRef.current = null;
    }, ACTION_NOTICE_TIMEOUT_MS);
  }

  async function handleSearchResultCopy(text: string, label: string): Promise<void> {
    try {
      await copyTextToClipboard(text);
      setActionError(null);
      showActionNotice(`${label} copied.`);
    } catch (error) {
      setActionError(`Failed to copy ${label}: ${String(error)}`);
    }
  }

  async function openResultPath(result: SearchResult): Promise<void> {
    if (isAppSearchResult(result)) {
      if (!result.revealPath) {
        showActionNotice("This app does not expose a file location.");
        return;
      }
      await revealResult(result.revealPath);
      return;
    }

    const targetPath = result.isDirectory ? result.path : parentDirectoryFromPath(result.path);
    if (!targetPath) {
      await revealResult(result.path);
      return;
    }
    await openResult(targetPath);
  }

  async function openResultInConsole(result: SearchResult): Promise<void> {
    if (isAppSearchResult(result)) {
      if (!result.revealPath) {
        showActionNotice("This app does not expose a file location.");
        return;
      }
    }
    try {
      await invoke("open_path_in_console", { path: result.revealPath ?? result.path });
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to open path in console: ${String(error)}`);
    }
  }

  async function sendSearchResultToPhone(result: SearchResult): Promise<void> {
    closeSearchResultContextMenu();
    if (result.isDirectory || isAppSearchResult(result)) {
      return;
    }

    try {
      await invoke<MobileTransferSnapshot>("send_file_to_mobile", { path: result.path });
      const info = await invoke<SyncServerInfo>("get_mobile_sync_server_info");
      setSyncServerInfo(info);
      setActiveTab("sync");
      setActionError(null);
      showActionNotice(`Sending ${resultDisplayName(result)} to phone.`);
    } catch (error) {
      setActionError(`Failed to send to phone: ${String(error)}`);
    }
  }

  function openSearchResultContextMenu(
    event: ReactMouseEvent<HTMLLIElement>,
    result: SearchResult,
    rowKey: string,
  ): void {
    if (hasSelectedText()) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();
    if (isQuickMode) {
      setSelectedResultKey(rowKey);
    }
    setSearchResultContextMenu({
      x: event.clientX,
      y: event.clientY,
      rowKey,
      result,
    });
  }

  function openSearchResultRename(result: SearchResult, rowKey: string): void {
    closeSearchResultContextMenu();
    const currentName = resultDisplayName(result);
    setSearchResultRenameDraft({
      rowKey,
      path: result.path,
      currentName,
      nextName: currentName,
      isDirectory: result.isDirectory,
    });
  }

  function openSearchResultDelete(result: SearchResult, rowKey: string): void {
    closeSearchResultContextMenu();
    setSearchResultDeleteToRecycleBin(false);
    setSearchResultDeleteCandidate({
      rowKey,
      path: result.path,
      name: resultDisplayName(result),
      isDirectory: result.isDirectory,
    });
  }

  async function startNativeSearchResultDrag(path: string): Promise<void> {
    try {
      await invoke("start_native_file_drag", { path });
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to drag file: ${String(error)}`);
    }
  }

  async function confirmSearchResultRename(): Promise<void> {
    if (!searchResultRenameDraft || searchResultRenameBusy) {
      return;
    }

    const nextName = searchResultRenameDraft.nextName.trim();
    if (!nextName) {
      setActionError("Rename failed: name cannot be empty.");
      return;
    }

    setSearchResultRenameBusy(true);
    try {
      const nextPath = await invoke<string>("rename_path", {
        path: searchResultRenameDraft.path,
        newName: nextName,
        new_name: nextName,
      });
      renameSearchResultInState(searchResultRenameDraft.path, nextPath, nextName);
      setSearchResultRenameDraft(null);
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to rename item: ${String(error)}`);
    } finally {
      setSearchResultRenameBusy(false);
    }
  }

  async function confirmSearchResultDelete(): Promise<void> {
    if (!searchResultDeleteCandidate || searchResultDeleteBusy) {
      return;
    }

    setSearchResultDeleteBusy(true);
    try {
      const deleted = await invoke<boolean>("delete_path", {
        path: searchResultDeleteCandidate.path,
        recycleBin: searchResultDeleteToRecycleBin,
        recycle_bin: searchResultDeleteToRecycleBin,
      });
      if (deleted) {
        removeSearchResultFromState(searchResultDeleteCandidate.path);
        setSearchResultDeleteCandidate(null);
        setActionError(null);
      }
    } catch (error) {
      setActionError(`Failed to delete item: ${String(error)}`);
    } finally {
      setSearchResultDeleteBusy(false);
    }
  }

  function handleSearchResultDragStart(
    event: ReactDragEvent<HTMLLIElement>,
    result: SearchResult,
  ): void {
    event.preventDefault();
    event.stopPropagation();

    if (result.isDirectory || isAppSearchResult(result)) {
      return;
    }

    closeSearchResultContextMenu();
    void startNativeSearchResultDrag(result.path);
  }

  async function revealResult(path: string): Promise<void> {
    try {
      await invoke("reveal_in_folder", { path });
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to reveal item in folder: ${String(error)}`);
    }
  }

  async function revealSearchResult(result: SearchResult): Promise<void> {
    if (isAppSearchResult(result)) {
      try {
        await invoke("reveal_installed_app", {
          revealPath: result.revealPath,
          reveal_path: result.revealPath,
        });
        setActionError(null);
      } catch (error) {
        setActionError(`Failed to reveal app location: ${String(error)}`);
      }
      return;
    }

    await revealResult(result.path);
  }

  function removeRecentActivityEntry(entryKey: string): void {
    setRecentActivity((previous) =>
      previous.filter((entry) => recentActivityEntryKey(entry) !== entryKey),
    );
  }

  function toggleRecentActivityPin(entryKey: string): void {
    setRecentActivity((previous) => {
      const next = previous.map((entry) => {
        if (recentActivityEntryKey(entry) !== entryKey) {
          return entry;
        }
        const nextPinned = !entry.pinned;
        return {
          ...entry,
          pinned: nextPinned,
          usedAt: nextPinned ? Date.now() : entry.usedAt,
        };
      });
      return sortRecentActivityEntries(next).slice(0, RECENT_ACTIVITY_LIMIT);
    });
  }

  function recordRecentResult(result: SearchResult): void {
    if (!recentActivityEnabled) {
      return;
    }

    setRecentActivity((previous) => {
      const nextEntryKey = recentActivityEntryKey({
        kind: "item",
        ...result,
        usedAt: 0,
        pinned: false,
      });
      const existingEntry = previous.find(
        (entry) => recentActivityEntryKey(entry) === nextEntryKey,
      );
      const nextEntry: RecentActivityItemEntry = {
        kind: "item",
        ...result,
        usedAt: Date.now(),
        pinned: existingEntry?.kind === "item" ? existingEntry.pinned : false,
      };
      const next = [
        nextEntry,
        ...previous.filter(
          (entry) => recentActivityEntryKey(entry) !== recentActivityEntryKey(nextEntry),
        ),
      ];
      return sortRecentActivityEntries(next).slice(0, RECENT_ACTIVITY_LIMIT);
    });
  }

  function reopenRecentQuery(queryValue: string): void {
    flushSync(() => {
      setQuery(queryValue);
    });
    window.requestAnimationFrame(() => {
      const input = searchInputRef.current;
      if (!input) {
        return;
      }
      input.focus();
      const caretPosition = input.value.length;
      try {
        input.setSelectionRange(caretPosition, caretPosition);
      } catch {
        // Ignore selection failures on unsupported input states.
      }
    });
  }

  async function launchSearchResult(result: SearchResult): Promise<void> {
    if (isAppSearchResult(result)) {
      try {
        await invoke("launch_installed_app", {
          launchTarget: result.launchTarget,
          launch_target: result.launchTarget,
          revealPath: result.revealPath,
          reveal_path: result.revealPath,
          isFileSystem: result.isFileSystemApp,
          is_file_system: result.isFileSystemApp,
        });
        if (recentActivityEnabled) {
          recordRecentResult(result);
        }
        setActionError(null);
      } catch (error) {
        setActionError(`Failed to launch app: ${String(error)}`);
      }
      return;
    }

    await openResult(result.path, result);
  }

  async function openResult(path: string, sourceResult?: SearchResult): Promise<void> {
    try {
      await invoke("open_file", { path });
      if (recentActivityEnabled) {
        const matchedResult =
          sourceResult ??
          results.find(
            (result) =>
              stripInvisibleText(result.path).trim() === stripInvisibleText(path).trim(),
          );
        if (matchedResult) {
          recordRecentResult(matchedResult);
        }
      }
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to open item: ${String(error)}`);
    }
  }

  async function openExternalLink(url: string): Promise<void> {
    try {
      await invoke("open_external_url", { url });
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to open link: ${String(error)}`);
    }
  }

  function toggleThemeMode(): void {
    setThemeMode((previous) => (previous === "dark" ? "light" : "dark"));
  }

  function resetQuickLookCopyFeedback(): void {
    if (quickLookCopyTimeoutRef.current !== null) {
      window.clearTimeout(quickLookCopyTimeoutRef.current);
      quickLookCopyTimeoutRef.current = null;
    }
    setQuickLookCopyState("idle");
  }

  function showQuickLookCopyFeedback(nextState: "copied" | "error"): void {
    if (quickLookCopyTimeoutRef.current !== null) {
      window.clearTimeout(quickLookCopyTimeoutRef.current);
    }
    setQuickLookCopyState(nextState);
    quickLookCopyTimeoutRef.current = window.setTimeout(() => {
      setQuickLookCopyState("idle");
      quickLookCopyTimeoutRef.current = null;
    }, QUICK_LOOK_COPY_FEEDBACK_MS);
  }

  async function handleQuickLookPathCopy(path: string): Promise<void> {
    try {
      await copyTextToClipboard(path);
      showQuickLookCopyFeedback("copied");
    } catch {
      showQuickLookCopyFeedback("error");
    }
  }

  function rememberQuickLookFocus(target?: HTMLElement | null): void {
    if (target) {
      quickLookReturnFocusRef.current = target;
      return;
    }
    if (typeof document === "undefined") {
      quickLookReturnFocusRef.current = null;
      return;
    }
    quickLookReturnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }

  function restoreQuickLookFocus(): void {
    const target = quickLookReturnFocusRef.current;
    quickLookReturnFocusRef.current = null;
    window.requestAnimationFrame(() => {
      if (target) {
        target.focus();
        return;
      }
      searchInputRef.current?.focus();
    });
  }

  function openQuickLook(rowKey?: string, focusTarget?: HTMLElement | null): void {
    if (activeTab !== "search" || visibleResults.length === 0) {
      return;
    }
    rememberQuickLookFocus(focusTarget);
    closeSearchResultContextMenu();
    if (rowKey) {
      flushSync(() => {
        setSelectedResultKey(rowKey);
      });
    }
    setQuickLookOpen(true);
  }

  function closeQuickLook(options: { restoreFocus?: boolean } = {}): void {
    const { restoreFocus = true } = options;
    setQuickLookOpen(false);
    if (restoreFocus) {
      restoreQuickLookFocus();
    } else {
      quickLookReturnFocusRef.current = null;
    }
  }

  function findResultRowElement(rowKey: string): HTMLElement | null {
    if (typeof document === "undefined") {
      return null;
    }
    const rows = document.querySelectorAll<HTMLElement>("[data-result-key]");
    for (const row of rows) {
      if (row.dataset.resultKey === rowKey) {
        return row;
      }
    }
    return null;
  }

  function scrollResultRowIntoView(rowKey: string): void {
    window.requestAnimationFrame(() => {
      const row = findResultRowElement(rowKey);
      row?.scrollIntoView({ block: "nearest" });
    });
  }

  function selectVisibleResultAtIndex(nextIndex: number): void {
    if (visibleResults.length === 0) {
      return;
    }
    const clampedIndex = Math.max(0, Math.min(visibleResults.length - 1, nextIndex));
    const nextRowKey = rowKeyForResult(visibleResults[clampedIndex]);
    setSelectedResultKey((current) => (current === nextRowKey ? current : nextRowKey));
    scrollResultRowIntoView(nextRowKey);
  }

  function moveSelectedResult(delta: number): void {
    if (visibleResults.length === 0) {
      return;
    }
    const currentIndex = selectedResultKey
      ? visibleResults.findIndex((result) => rowKeyForResult(result) === selectedResultKey)
      : -1;
    const baseIndex = currentIndex >= 0 ? currentIndex : 0;
    selectVisibleResultAtIndex(baseIndex + delta);
  }

  function jumpToVisibleResult(edge: "start" | "end"): void {
    if (visibleResults.length === 0) {
      return;
    }
    selectVisibleResultAtIndex(edge === "start" ? 0 : visibleResults.length - 1);
  }

  function handlePreviewError(rowKey: string, sourceCount: number): void {
    setPreviewSourceState((previous) => {
      const currentIndex = previous[rowKey] ?? 0;
      if (currentIndex < 0) {
        return previous;
      }
      const nextIndex = currentIndex + 1;
      if (nextIndex < sourceCount) {
        return {
          ...previous,
          [rowKey]: nextIndex,
        };
      }
      return {
        ...previous,
        [rowKey]: -1,
      };
    });
  }

  function handlePreviewReady(previewKey: string): void {
    setPreviewReadyState((previous) => {
      if (previous[previewKey]) {
        return previous;
      }
      return {
        ...previous,
        [previewKey]: true,
      };
    });
  }

  function handleSelectedPreviewError(sourceCount: number): void {
    setSelectedPreviewSourceIndex((currentIndex) => {
      if (currentIndex < 0) {
        return currentIndex;
      }
      const nextIndex = currentIndex + 1;
      return nextIndex < sourceCount ? nextIndex : -1;
    });
  }

  function handleSelectedPreviewReady(previewKey: string): void {
    setSelectedPreviewReadyState((previous) => {
      if (previous[previewKey]) {
        return previous;
      }
      return {
        ...previous,
        [previewKey]: true,
      };
    });
  }

  function openSearchSyntaxHelp(sectionId?: string): void {
    if (sectionId) {
      setSearchSyntaxHelpSectionId(sectionId);
    }
    setActiveTab("syntax");
  }

  function applySearchSyntaxExample(example: string): void {
    flushSync(() => {
      setActiveTab("search");
      setQuery(example);
    });
    window.requestAnimationFrame(() => {
      const input = searchInputRef.current;
      if (!input) {
        return;
      }
      input.focus();
      const caretPosition = input.value.length;
      try {
        input.setSelectionRange(caretPosition, caretPosition);
      } catch {
        // Ignore selection failures on unsupported input states.
      }
    });
  }

  const statusText = status.indexing
    ? `Indexing ${status.indexedCount.toLocaleString()} items...`
    : indexSyncing
      ? `Syncing updates... Indexed ${status.indexedCount.toLocaleString()} items`
      : status.ready
        ? `Indexed ${status.indexedCount.toLocaleString()} items`
        : "Indexer idle";
  const duplicateProgressPercent = Math.max(
    0,
    Math.min(100, Number.isFinite(duplicateScanStatus.progressPercent) ? duplicateScanStatus.progressPercent : 0),
  );
  const duplicateProgressLabel =
    duplicateScanStatus.totalFiles > 0
      ? `${duplicateScanStatus.scannedFiles.toLocaleString()} / ${duplicateScanStatus.totalFiles.toLocaleString()} files scanned`
      : `${duplicateScanStatus.scannedFiles.toLocaleString()} files scanned`;
  const showDuplicateProgress =
    duplicatesLoading ||
    duplicateScanStatus.running ||
    duplicateScanStatus.cancelRequested ||
    duplicateScanStatus.totalFiles > 0;
  const hasSearchRequest = Boolean(trimmedQuery || hasFilters);
  const canLoadMore =
    hasSearchRequest &&
    !loading &&
    !searchError &&
    results.length > 0 &&
    results.length >= searchLimit &&
    searchLimit < SEARCH_LIMIT_MAX;
  const visibleRecentActivity = recentActivity.slice(0, RECENT_ACTIVITY_LIMIT);
  const showRecentActivity =
    !isQuickMode &&
    activeTab === "search" &&
    recentActivityEnabled &&
    trimmedQuery.length === 0 &&
    !hasFilters;
  const appIconLoadCandidates = useMemo(() => {
    const seen = new Set<string>();
    const candidates: SearchResult[] = [];
    const pushCandidate = (result: SearchResult | null | undefined) => {
      if (!result || !isAppSearchResult(result)) {
        return;
      }
      const key = result.appId ?? result.launchTarget ?? result.name;
      if (!key || seen.has(key) || result.iconDataUrl) {
        return;
      }
      seen.add(key);
      candidates.push(result);
    };

    visibleResults.slice(0, 16).forEach(pushCandidate);
    pushCandidate(selectedResult);
    visibleRecentActivity.forEach((entry) => {
      if (entry.kind === "item") {
        pushCandidate(entry);
      }
    });

    return candidates;
  }, [selectedResult, visibleRecentActivity, visibleResults]);
  const searchMetricsAutoHidden = activeTab === "search" && isCompactSearchViewport;
  const effectiveSearchMetricsHidden = searchMetricsHidden || searchMetricsAutoHidden;
  const searchMetricsToggleLabel = searchMetricsAutoHidden
    ? "Compact search layout is on automatically while the window is small"
    : searchMetricsHidden
      ? "Show full search layout"
      : "Switch to compact search layout";
  const selectedResultRowKey = selectedResult ? rowKeyForResult(selectedResult) : "";
  const selectedResultIsDirectory = selectedResult?.isDirectory ?? false;
  const selectedResultExtension = selectedResult ? normalizedExtension(selectedResult) : "";
  const selectedResultIconKind = selectedResult ? iconKindFromResult(selectedResult) : "file";
  const selectedResultAppIconSrc =
    selectedResult && isAppSearchResult(selectedResult)
      ? selectedResult.iconDataUrl ??
      (selectedResult.appId ? appIconDataUrls[selectedResult.appId] ?? "" : "")
      : "";
  const selectedResultLocationText = selectedResult ? resultLocationText(selectedResult) : "";
  const selectedResultLocationLabel = selectedResult ? resultLocationLabel(selectedResult) : "Path";
  const selectedResultExtensionLabel = selectedResult
    ? selectedResultIsDirectory
      ? "folder"
      : selectedResultExtension
        ? `.${selectedResultExtension}`
        : "file"
    : "";
  const selectedPreviewKind = selectedResult && showPreviews ? previewKindFromResult(selectedResult) : "none";
  const selectedPreviewSources =
    selectedResult && isAssetPreviewKind(selectedPreviewKind)
      ? [
        ...(previewDataUrls[selectedResultRowKey] ? [previewDataUrls[selectedResultRowKey]] : []),
        ...previewSourcesFromPath(selectedResult.path),
      ].filter((source, index, all) => source.length > 0 && all.indexOf(source) === index)
      : [];
  const selectedPreviewIndex = selectedResult ? selectedPreviewSourceIndex : 0;
  const selectedPreviewFailed = selectedPreviewIndex < 0;
  const selectedPreviewRenderKey = selectedResult
    ? `${selectedResultRowKey}:${selectedPreviewIndex}:${selectedPreviewKind}:selected`
    : "";
  const selectedPreviewReady = Boolean(
    selectedPreviewRenderKey && selectedPreviewReadyState[selectedPreviewRenderKey],
  );
  const selectedPreviewSrc =
    !selectedPreviewFailed && isAssetPreviewKind(selectedPreviewKind)
      ? (selectedPreviewSources[selectedPreviewIndex] ?? "")
      : "";
  const hasSelectedPreview = selectedPreviewSrc.length > 0;
  const selectedTextPreviewKey =
    selectedResult && selectedPreviewKind === "text"
      ? textPreviewCacheKey(selectedResult, contentSearchQuery, contentSearchMode, TEXT_PREVIEW_MAX_CHARS)
      : "";
  const selectedTextPreview = selectedTextPreviewKey ? textPreviewState[selectedTextPreviewKey] : undefined;
  const selectedTextPreviewLoading = Boolean(
    selectedTextPreviewKey && textPreviewLoadingState[selectedTextPreviewKey],
  );
  const hasSelectedTextPreview = Boolean(selectedTextPreview?.text);
  const quickLookPreviewKind =
    quickLookOpen && selectedResult ? previewKindFromResult(selectedResult) : "none";
  const quickLookPreviewSources =
    selectedResult && isAssetPreviewKind(quickLookPreviewKind)
      ? [
        ...(previewDataUrls[selectedResultRowKey] ? [previewDataUrls[selectedResultRowKey]] : []),
        ...previewSourcesFromPath(selectedResult.path),
      ].filter((source, index, all) => source.length > 0 && all.indexOf(source) === index)
      : [];
  const quickLookPreviewIndex = selectedResult ? selectedPreviewSourceIndex : 0;
  const quickLookPreviewFailed = quickLookPreviewIndex < 0;
  const quickLookPreviewRenderKey = selectedResult
    ? `${selectedResultRowKey}:${quickLookPreviewIndex}:${quickLookPreviewKind}:quick-look`
    : "";
  const quickLookPreviewReady = Boolean(
    quickLookPreviewRenderKey && selectedPreviewReadyState[quickLookPreviewRenderKey],
  );
  const quickLookPreviewSrc =
    !quickLookPreviewFailed && isAssetPreviewKind(quickLookPreviewKind)
      ? (quickLookPreviewSources[quickLookPreviewIndex] ?? "")
      : "";
  const hasQuickLookPreview = quickLookPreviewSrc.length > 0;
  const quickLookTextPreviewKey =
    selectedResult && quickLookPreviewKind === "text"
      ? textPreviewCacheKey(selectedResult, contentSearchQuery, contentSearchMode, TEXT_PREVIEW_MAX_CHARS)
      : "";
  const quickLookTextPreview = quickLookTextPreviewKey ? textPreviewState[quickLookTextPreviewKey] : undefined;
  const quickLookTextPreviewLoading = Boolean(
    quickLookTextPreviewKey && textPreviewLoadingState[quickLookTextPreviewKey],
  );
  const hasQuickLookTextPreview = Boolean(quickLookTextPreview?.text);
  const selectedResultPositionIndex = selectedResult
    ? visibleResults.findIndex((result) => rowKeyForResult(result) === selectedResultRowKey)
    : -1;
  const selectedResultPosition =
    selectedResultPositionIndex >= 0 ? selectedResultPositionIndex + 1 : visibleResults.length > 0 ? 1 : 0;
  const activeSearchSyntaxHelpSection =
    SEARCH_SYNTAX_HELP_SECTIONS.find((section) => section.id === searchSyntaxHelpSectionId) ??
    SEARCH_SYNTAX_HELP_SECTIONS[0];
  const contentSearchStatusMessage = hasContentSearchSyntax
    ? "Searching file contents... hold on tight, this can take a few seconds."
    : "Searching...";
  const contentSearchWarningMessage =
    "Content search reads matching files from disk. Pair it with ext:txt or other filters for the best speed.";
  const showQuickInlineSearching = isQuickMode && loading && hasSearchRequest;
  const showQuickEmptyState = isQuickMode && visibleResults.length === 0 && !showQuickInlineSearching;
  const quickSplitResizeEnabled =
    isQuickMode && !showQuickEmptyState && viewportSize.width > QUICK_RESULTS_STACK_BREAKPOINT;
  const effectiveQuickResultsPaneRatio = quickSplitResizeEnabled
    ? clampQuickResultsPaneRatio(quickResultsPaneRatio, quickResultsStageWidth)
    : QUICK_RESULTS_PANE_DEFAULT_RATIO;
  const quickResultsStageStyle = quickSplitResizeEnabled
    ? ({
      "--quick-results-pane-width": `${(effectiveQuickResultsPaneRatio * 100).toFixed(2)}%`,
    } as CSSProperties)
    : undefined;
  const quickPreviewEmptyTitle = searchError
    ? "Search couldn't finish"
    : showQuickEmptyState
      ? trimmedQuery || hasFilters
        ? "No results match the current filters"
        : includeInstalledApps
          ? "Start typing to search indexed files and apps"
          : "Start typing to search indexed files"
      : "Pick a result to preview";
  const quickPreviewEmptyDetail = searchError
    ? "Check the message above and try again."
    : showQuickEmptyState && (trimmedQuery || hasFilters)
      ? "Try another search or adjust the filters."
      : "";
  const activeSearchResultMenu = searchResultContextMenu?.result ?? null;
  const activeSearchResultIsApp = activeSearchResultMenu ? isAppSearchResult(activeSearchResultMenu) : false;
  const activeSearchResultCanReveal = activeSearchResultMenu
    ? canRevealResultLocation(activeSearchResultMenu)
    : false;
  const activeSearchResultCanSendToPhone = Boolean(
    activeSearchResultMenu &&
    !activeSearchResultMenu.isDirectory &&
    !activeSearchResultIsApp,
  );
  const activeSearchResultLocationText = activeSearchResultMenu
    ? resultLocationText(activeSearchResultMenu)
    : "";
  const activeSearchResultLocationLabel = activeSearchResultMenu
    ? resultLocationLabel(activeSearchResultMenu)
    : "Path";
  const searchResultContextMenuStyle = searchResultContextMenu
    ? ({
      left: `${Math.max(
        12,
        Math.min(
          searchResultContextMenu.x,
          (typeof window !== "undefined" ? window.innerWidth : searchResultContextMenu.x + 260) -
          272,
        ),
      )}px`,
      top: `${Math.max(
        12,
        Math.min(
          searchResultContextMenu.y,
          (typeof window !== "undefined" ? window.innerHeight : searchResultContextMenu.y + 320) -
          332,
        ),
      )}px`,
    } as CSSProperties)
    : undefined;

  useEffect(() => {
    const node = quickResultsStageRef.current;
    if (!node || !isQuickMode) {
      setQuickResultsStageWidth(0);
      return;
    }

    const updateStageWidth = () => {
      setQuickResultsStageWidth(node.getBoundingClientRect().width);
    };

    updateStageWidth();

    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", updateStageWidth);
      return () => {
        window.removeEventListener("resize", updateStageWidth);
      };
    }

    const observer = new ResizeObserver(() => {
      updateStageWidth();
    });
    observer.observe(node);

    return () => {
      observer.disconnect();
    };
  }, [isQuickMode, showQuickEmptyState]);

  useEffect(() => {
    if (!quickSplitResizeEnabled) {
      setQuickSplitDragging(false);
    }
  }, [quickSplitResizeEnabled]);

  useEffect(() => {
    if (!quickSplitDragging) {
      return;
    }

    const handleMouseMove = (event: MouseEvent) => {
      const node = quickResultsStageRef.current;
      if (!node) {
        return;
      }

      const rect = node.getBoundingClientRect();
      if (rect.width <= 0) {
        return;
      }

      const nextRatio = (event.clientX - rect.left - QUICK_RESULTS_SPLITTER_WIDTH / 2) / rect.width;
      setQuickResultsPaneRatio(clampQuickResultsPaneRatio(nextRatio, rect.width));
    };

    const stopDragging = () => {
      setQuickSplitDragging(false);
    };

    const previousBodyCursor = document.body.style.cursor;
    const previousBodyUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", stopDragging);
    window.addEventListener("blur", stopDragging);

    return () => {
      document.body.style.cursor = previousBodyCursor;
      document.body.style.userSelect = previousBodyUserSelect;
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", stopDragging);
      window.removeEventListener("blur", stopDragging);
    };
  }, [quickSplitDragging]);

  useEffect(() => {
    if (!quickLookOpen) {
      return;
    }
    if (activeTab !== "search" || !selectedResult) {
      setQuickLookOpen(false);
      quickLookReturnFocusRef.current = null;
    }
  }, [activeTab, quickLookOpen, selectedResult]);

  useEffect(() => {
    resetQuickLookCopyFeedback();
  }, [quickLookOpen, selectedResultRowKey]);

  useEffect(() => {
    if (!quickLookOpen) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      quickLookDialogRef.current?.focus();
    });
    return () => {
      window.cancelAnimationFrame(frame);
    };
  }, [quickLookOpen, selectedResultRowKey]);

  useEffect(() => {
    if (!quickLookOpen || activeTab !== "search") {
      return;
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (isQuickLookShortcutTarget(event.target)) {
        if (event.key === "Escape") {
          event.preventDefault();
          closeQuickLook();
        }
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        closeQuickLook();
      } else if (event.key === "ArrowDown" || event.key === "ArrowRight") {
        event.preventDefault();
        moveSelectedResult(1);
      } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
        event.preventDefault();
        moveSelectedResult(-1);
      } else if (event.key === "Home") {
        event.preventDefault();
        jumpToVisibleResult("start");
      } else if (event.key === "End") {
        event.preventDefault();
        jumpToVisibleResult("end");
      } else if (event.key === "Enter" && selectedResult) {
        event.preventDefault();
        closeQuickLook({ restoreFocus: false });
        void launchSearchResult(selectedResult);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [activeTab, quickLookOpen, selectedResult, selectedResultKey, selectedResultRowKey, visibleResults]);

  useEffect(() => {
    const missing = appIconLoadCandidates.filter((result) => {
      const appId = result.appId;
      if (!appId) {
        return false;
      }
      return !appIconDataUrls[appId] && !appIconFailures[appId];
    });
    if (missing.length === 0) {
      return;
    }

    let cancelled = false;
    const load = async () => {
      for (const result of missing) {
        const appId = result.appId;
        if (cancelled || !appId) {
          return;
        }
        try {
          const dataUrl = await invoke<string>("load_installed_app_icon_data_url", {
            launchTarget: result.launchTarget,
            launch_target: result.launchTarget,
            revealPath: result.revealPath,
            reveal_path: result.revealPath,
            isFileSystem: result.isFileSystemApp,
            is_file_system: result.isFileSystemApp,
          });
          if (cancelled || !dataUrl.startsWith("data:")) {
            continue;
          }
          setAppIconDataUrls((previous) => {
            if (previous[appId]) {
              return previous;
            }
            return {
              ...previous,
              [appId]: dataUrl,
            };
          });
          setRecentActivity((previous) =>
            previous.map((entry) =>
              entry.kind === "item" && entry.appId === appId && !entry.iconDataUrl
                ? {
                  ...entry,
                  iconDataUrl: dataUrl,
                }
                : entry,
            ),
          );
        } catch {
          if (cancelled) {
            return;
          }
          setAppIconFailures((previous) => ({
            ...previous,
            [appId]: true,
          }));
        }
      }
    };

    void load();

    return () => {
      cancelled = true;
    };
  }, [appIconDataUrls, appIconFailures, appIconLoadCandidates]);

  function adjustQuickResultsPaneRatio(delta: number): void {
    setQuickResultsPaneRatio((current) =>
      clampQuickResultsPaneRatio(current + delta, quickResultsStageWidth),
    );
  }

  function renderRecentActivityCard(entry: RecentActivityEntry): ReactNode {
    const entryKey = recentActivityEntryKey(entry);
    const isQueryEntry = entry.kind === "query";
    const isPinnedEntry = entry.pinned;
    const previewKind = !isQueryEntry && showPreviews ? previewKindFromResult(entry) : "none";
    const previewSources =
      !isQueryEntry && (previewKind === "image" || previewKind === "video")
        ? previewSourcesFromPath(entry.path).filter(
          (source, index, all) => source.length > 0 && all.indexOf(source) === index,
        )
        : [];
    const previewSrc = previewSources[0] ?? "";
    const iconKind = !isQueryEntry ? iconKindFromResult(entry) : null;
    const appIconSrc =
      !isQueryEntry && isAppSearchResult(entry)
        ? entry.iconDataUrl ?? (entry.appId ? appIconDataUrls[entry.appId] ?? "" : "")
        : "";

    const activateCard = () => {
      if (isQueryEntry) {
        reopenRecentQuery(entry.query);
        return;
      }
      void launchSearchResult(entry);
    };

    return (
      <article
        key={entryKey}
        className={`recent-activity-card ${isQueryEntry ? "is-query" : "is-item"} ${isPinnedEntry ? "is-pinned" : ""
          }`}
        role="button"
        tabIndex={0}
        onClick={activateCard}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            activateCard();
          }
        }}
      >
        <div
          className={`result-preview recent-activity-preview ${isQueryEntry ? "query" : previewKind} ${!isQueryEntry && entry.isDirectory ? "folder" : ""
            } ${iconKind ? `kind-${iconKind}` : ""}`}
          aria-hidden="true"
        >
          {!isQueryEntry ? <ResultFallbackIcon result={entry} className="recent-activity-fallback-icon-shell" /> : null}
          {isQueryEntry ? (
            <span className="recent-activity-query-icon">
              <SearchLensIcon />
            </span>
          ) : appIconSrc.length > 0 ? (
            <img className="preview-media ready app-result-icon-media" src={appIconSrc} alt="" loading="lazy" />
          ) : previewSrc.length > 0 && previewKind === "image" ? (
            <img className="preview-media ready" src={previewSrc} alt="" loading="lazy" />
          ) : previewSrc.length > 0 && previewKind === "video" ? (
            <video
              className="preview-media ready"
              src={previewSrc}
              muted
              playsInline
              preload="metadata"
            />
          ) : null}
        </div>

        <div className="recent-activity-copy">
          <strong>{isQueryEntry ? entry.query : entry.name}</strong>
          {!isQueryEntry ? <span>{entry.path}</span> : null}
        </div>

        <div className="recent-activity-actions">
          <button
            type="button"
            className={`recent-activity-pin ${isPinnedEntry ? "is-pinned" : ""}`}
            aria-label={
              isPinnedEntry
                ? isQueryEntry
                  ? `Unpin recent search ${entry.query}`
                  : `Unpin recent item ${entry.name}`
                : isQueryEntry
                  ? `Pin recent search ${entry.query}`
                  : `Pin recent item ${entry.name}`
            }
            title={
              isPinnedEntry
                ? isQueryEntry
                  ? "Unpin search"
                  : "Unpin item"
                : isQueryEntry
                  ? "Pin search"
                  : "Pin item"
            }
            onClick={(event) => {
              event.stopPropagation();
              toggleRecentActivityPin(entryKey);
            }}
          >
            <PinIcon />
          </button>
          <button
            type="button"
            className="recent-activity-remove"
            aria-label={
              isQueryEntry
                ? `Remove recent search ${entry.query}`
                : `Remove recent item ${entry.name}`
            }
            onClick={(event) => {
              event.stopPropagation();
              removeRecentActivityEntry(entryKey);
            }}
          >
            <span aria-hidden="true">x</span>
          </button>
        </div>
      </article>
    );
  }

  return (
    <div
      className={`app-shell ${isQuickMode ? "quick-window-mode" : ""}`}
      onContextMenu={(event) => {
        event.preventDefault();
      }}
    >
      <div className="app-background" aria-hidden="true">
        <div className="app-background-base" />
        <div className="app-background-overlay" />
      </div>
      <main className={`spotlight-panel ${isQuickMode ? "spotlight-panel-quick" : ""}`}>
        <header className={`panel-header ${isQuickMode ? "quick-panel-header" : ""}`}>
          <div className="panel-title-block">
            {isQuickMode ? <span className="quick-mode-badge">Quick Window</span> : null}
            {!isQuickMode ? <h1>OmniSearch</h1> : null}
          </div>
          <div className={`header-tools ${isQuickMode ? "quick-header-tools" : ""}`}>
            {isQuickMode ? (
              <>
                <button
                  type="button"
                  className="theme-toggle"
                  onClick={toggleThemeMode}
                  aria-label={themeMode === "dark" ? "Switch to light mode" : "Switch to dark mode"}
                  title={themeMode === "dark" ? "Switch to light mode" : "Switch to dark mode"}
                >
                  <span className="theme-toggle-dot" aria-hidden="true" />
                  <span>{themeMode === "dark" ? "Light mode" : "Dark mode"}</span>
                </button>
                <span className="quick-index-indicator" title={`Current index scope: ${quickIndexScopeLabel}`}>
                  {quickIndexScopeLabel}
                </span>
                <button
                  type="button"
                  className={`ghost-button ${activeTab === "sync" ? "is-active-tab-sync" : ""}`}
                  style={{ display: "inline-flex", alignItems: "center", gap: "6px" }}
                  onClick={() => {
                    setActiveTab(activeTab === "sync" ? "search" : "sync");
                  }}
                >
                  <svg
                    viewBox="0 0 24 24"
                    width="14"
                    height="14"
                    stroke="currentColor"
                    strokeWidth="2"
                    fill="none"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  >
                    <rect x="2" y="3" width="13" height="9" rx="1.5" />
                    <path d="M16 8h2a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2v-2" />
                    <path d="M12 18h.01" />
                  </svg>
                  <span>Sync</span>
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => {
                    setWindowMode("full");
                    setActiveTab("search");
                    void openFullWindow().catch((error) => {
                      setWindowMode("quick");
                      setDesktopSettingsError(`Failed to open the full workspace: ${String(error)}`);
                    });
                  }}
                >
                  Full workspace
                </button>
              </>
            ) : (
              <>
                <label className="drive-picker" htmlFor="drive-picker">
                  <span>Drive</span>
                  <select
                    id="drive-picker"
                    value={selectedDrive}
                    disabled={includeAllDrives}
                    onChange={(event) => {
                      setSelectedDrive(event.currentTarget.value);
                    }}
                  >
                    {drives
                      .filter((drive) => drive.isNtfs)
                      .map((drive) => (
                        <option key={drive.letter} value={drive.letter}>
                          {`${drive.letter}: (${drive.filesystem || "Unknown"})`}
                        </option>
                      ))}
                  </select>
                </label>
                <label
                  className="scan-switch"
                  htmlFor="all-drives-toggle"
                  title="Scan all NTFS drives before search. Uses more time and resources."
                >
                  <input
                    id="all-drives-toggle"
                    type="checkbox"
                    checked={includeAllDrives}
                    onChange={(event) => {
                      const nextIncludeAllDrives = event.currentTarget.checked;
                      setIncludeAllDrives(nextIncludeAllDrives);
                      void reindexWithConfig(includeFolders, nextIncludeAllDrives);
                    }}
                  />
                  <span className="scan-switch-slider" aria-hidden="true" />
                  <span>Scan all drives</span>
                </label>
                <label
                  className="scan-option"
                  htmlFor="include-folders-toggle"
                  title="Include folders in index."
                >
                  <input
                    id="include-folders-toggle"
                    type="checkbox"
                    checked={includeFolders}
                    onChange={(event) => {
                      const nextIncludeFolders = event.currentTarget.checked;
                      setIncludeFolders(nextIncludeFolders);
                      void reindexWithConfig(nextIncludeFolders, includeAllDrives);
                    }}
                  />
                  <span>Include folders</span>
                </label>
                <button type="button" className="ghost-button" onClick={reindex}>
                  Reindex
                </button>
              </>
            )}
          </div>
        </header>

        {!isQuickMode ? (
          <nav className="tab-row" aria-label="Main sections">
            <button
              type="button"
              className={`tab ${activeTab === "search" ? "is-active" : ""}`}
              onClick={() => {
                setActiveTab("search");
              }}
            >
              Search
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "duplicates" ? "is-active" : ""}`}
              onClick={() => {
                setActiveTab("duplicates");
              }}
            >
              Duplicates
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "advanced" ? "is-active" : ""}`}
              onClick={() => {
                setActiveTab("advanced");
              }}
            >
              Settings
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "themes" ? "is-active" : ""}`}
              onClick={() => {
                setActiveTab("themes");
              }}
            >
              Themes
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "syntax" ? "is-active" : ""}`}
              onClick={() => {
                openSearchSyntaxHelp();
              }}
            >
              Syntax
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "about" ? "is-active" : ""}`}
              onClick={() => {
                setActiveTab("about");
              }}
            >
              About
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "sync" ? "is-active" : ""}`}
              style={{ display: "inline-flex", alignItems: "center", gap: "6px" }}
              onClick={() => {
                setActiveTab("sync");
              }}
            >
              <svg
                viewBox="0 0 24 24"
                width="16"
                height="16"
                stroke="currentColor"
                strokeWidth="2"
                fill="none"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <rect x="2" y="3" width="13" height="9" rx="1.5" />
                <path d="M5 12v3M11 12v3M3 15h11" />
                <rect x="14" y="6" width="8" height="15" rx="1.5" fill="var(--surface-strong)" />
                <circle cx="18" cy="18" r="0.5" fill="currentColor" />
                <path d="M17 8h2" />
              </svg>
              <span>Sync</span>
            </button>
            <button
              type="button"
              className={`tab-icon-button ${effectiveSearchMetricsHidden ? "is-active" : ""}`}
              aria-pressed={effectiveSearchMetricsHidden}
              aria-label={searchMetricsToggleLabel}
              title={searchMetricsToggleLabel}
              disabled={searchMetricsAutoHidden}
              onClick={() => {
                setSearchMetricsHidden((current) => !current);
              }}
            >
              <SearchLayoutToggleIcon hidden={effectiveSearchMetricsHidden} />
            </button>
            <span className="tab-row-spacer" aria-hidden="true" />
            <button
              type="button"
              className="ghost-button tab-row-action"
              title="Open quick window"
              onClick={() => {
                void openQuickWindow().catch((error) => {
                  setDesktopSettingsError(`Failed to open the quick window: ${String(error)}`);
                });
              }}
            >
              {currentShortcutLabel}
            </button>
          </nav>
        ) : null}

        {activeTab === "search" ? (
          <section className={`tab-panel ${isQuickMode ? "quick-tab-panel" : ""}`} aria-label="Search files">
            <div className={`status-row ${isQuickMode ? "quick-status-row" : ""}`}>
              <span
                className={`status-dot ${status.indexing || indexSyncing ? "live" : status.ready ? "ready" : "idle"
                  }`}
              />
              <span>{statusText}</span>
            </div>

            {visibleStatusError ? <p className="error-row">{visibleStatusError}</p> : null}
            {driveError ? <p className="error-row">{driveError}</p> : null}
            {hasContentSearchSyntax ? (
              <p className="warning-row">{contentSearchWarningMessage}</p>
            ) : null}

            <div className={`search-input-shell ${isQuickMode ? "quick-search-input-shell" : ""}`}>
              <span className="search-input-icon" aria-hidden="true">
                <SearchLensIcon />
              </span>
              <input
                ref={searchInputRef}
                className={`search-input ${isQuickMode ? "quick-search-input" : ""}`}
                type="text"
                value={query}
                autoComplete="off"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                onChange={(event) => setQuery(event.currentTarget.value)}
                placeholder="Type to search across indexed items..."
                autoFocus
              />
              {query.length > 0 ? (
                <button
                  type="button"
                  className="search-input-clear"
                  aria-label="Clear search"
                  onMouseDown={(event) => {
                    event.preventDefault();
                  }}
                  onClick={() => {
                    cancelSearchRequest();
                    setQuery("");
                    searchInputRef.current?.focus();
                  }}
                >
                  <span aria-hidden="true">x</span>
                </button>
              ) : null}
              <button
                type="button"
                className="search-input-folder-btn"
                aria-label="Open folder"
                title="Open folder"
                style={{ right: query.length > 0 ? "40px" : "12px" }}
                onMouseDown={(event) => {
                  event.preventDefault();
                }}
                onClick={async () => {
                  try {
                    const folderPath: string = await invoke("select_folder");
                    if (folderPath) {
                      setQuery(folderPath);
                      searchInputRef.current?.focus();
                    }
                  } catch (_err) {
                    // user cancelled or error – ignore
                  }
                }}
              >
                <FolderOpenIcon />
              </button>
            </div>

            {!effectiveSearchMetricsHidden ? (
              <section className={`filter-grid ${isQuickMode ? "quick-filter-grid" : ""}`}>
                <label>
                  Extension
                  <div className="filter-input-shell">
                    <input
                      ref={extensionInputRef}
                      type="text"
                      value={extension}
                      autoComplete="off"
                      autoCapitalize="none"
                      autoCorrect="off"
                      spellCheck={false}
                      onChange={(event) => setExtension(event.currentTarget.value)}
                      placeholder=".mp4 or folder"
                    />
                    {extension.trim().length > 0 ? (
                      <button
                        type="button"
                        className="filter-input-clear"
                        aria-label="Clear extension filter"
                        onMouseDown={(event) => {
                          event.preventDefault();
                        }}
                        onClick={() => {
                          setExtension("");
                          extensionInputRef.current?.focus();
                        }}
                      >
                        <span aria-hidden="true">x</span>
                      </button>
                    ) : null}
                  </div>
                </label>
                <label>
                  Min size (MB)
                  <NumberInputField
                    min={0}
                    step={1}
                    value={minSizeMb}
                    placeholder="0"
                    ariaLabel="Minimum size in megabytes"
                    clearable
                    onChange={setMinSizeMb}
                  />
                </label>
                <label>
                  Max size (MB)
                  <NumberInputField
                    min={0}
                    step={1}
                    value={maxSizeMb}
                    placeholder="2048"
                    ariaLabel="Maximum size in megabytes"
                    clearable
                    onChange={setMaxSizeMb}
                  />
                </label>
                <label>
                  Created after
                  <div className="date-input-shell">
                    <input
                      ref={createdAfterInputRef}
                      type="date"
                      value={createdAfter}
                      autoComplete="off"
                      onChange={(event) => setCreatedAfter(event.currentTarget.value)}
                    />
                    <button
                      type="button"
                      className="date-input-trigger"
                      aria-label="Open created after date picker"
                      onClick={() => {
                        openDateInputPicker(createdAfterInputRef.current);
                      }}
                    >
                      <CalendarIcon />
                    </button>
                  </div>
                </label>
                <label>
                  Created before
                  <div className="date-input-shell">
                    <input
                      ref={createdBeforeInputRef}
                      type="date"
                      value={createdBefore}
                      autoComplete="off"
                      onChange={(event) => setCreatedBefore(event.currentTarget.value)}
                    />
                    <button
                      type="button"
                      className="date-input-trigger"
                      aria-label="Open created before date picker"
                      onClick={() => {
                        openDateInputPicker(createdBeforeInputRef.current);
                      }}
                    >
                      <CalendarIcon />
                    </button>
                  </div>
                </label>
              </section>
            ) : null}

            <section className={`results-panel ${isQuickMode ? "quick-results-panel" : ""}`}>
              {!effectiveSearchMetricsHidden ? (
                <div className={`results-toolbar ${isQuickMode ? "quick-results-toolbar" : ""}`}>
                  <div className="results-scope-tabs" aria-label="Result categories">
                    {RESULT_VIEW_TABS.map((item) => (
                      <button
                        key={item.id}
                        type="button"
                        className={`scope-tab ${resultView === item.id ? "is-active" : ""}`}
                        onClick={() => {
                          setResultView(item.id);
                        }}
                      >
                        <span>{item.label}</span>
                        <small>{resultCounts[item.id].toLocaleString()}</small>
                      </button>
                    ))}
                  </div>

                  <div className="results-toolbar-actions">
                    <div className="results-inline-stats" aria-live="polite">
                      <span>
                        {`${visibleResults.length.toLocaleString()} shown`}
                        {visibleResults.length !== filteredCombinedResults.length
                          ? ` / ${filteredCombinedResults.length.toLocaleString()}`
                          : ""}
                        {` (limit ${searchLimit.toLocaleString()})`}
                      </span>
                      <span>{formatBytes(visibleTotalBytes)}</span>
                    </div>

                    <label className="preview-toggle" htmlFor="preview-toggle">
                      <input
                        id="preview-toggle"
                        type="checkbox"
                        checked={showPreviews}
                        onChange={(event) => {
                          setShowPreviews(event.currentTarget.checked);
                        }}
                      />
                      <span>Show previews</span>
                    </label>
                    <label className="sort-picker" htmlFor="result-sort">
                      <span className="sort-picker-label">Sort</span>
                      <select
                        id="result-sort"
                        value={resultSort}
                        onChange={(event) => {
                          setResultSort(event.currentTarget.value as ResultSortMode);
                        }}
                      >
                        <option value="relevance">Best match</option>
                        <option value="newest">Newest</option>
                        <option value="largest">Largest</option>
                        <option value="name">Name A-Z</option>
                      </select>
                    </label>
                    {hasFilters ? (
                      <button type="button" className="clear-filters" onClick={clearSearchFilters}>
                        Clear filters
                      </button>
                    ) : null}
                  </div>
                </div>
              ) : null}

              {loading ? <p className="hint compact-hint">{contentSearchStatusMessage}</p> : null}
              {searchError ? <p className="error-row">{searchError}</p> : null}
              {actionError ? <p className="error-row">{actionError}</p> : null}
              {actionNotice ? <p className="info-row">{actionNotice}</p> : null}
              {!isQuickMode &&
                !loading &&
                !searchError &&
                visibleResults.length === 0 &&
                (trimmedQuery || hasFilters) ? (
                <p className="hint compact-hint">No items match the current filters.</p>
              ) : null}

              {showRecentActivity ? (
                <section className="recent-activity-panel" aria-label="Recent searches and files">
                  <div className="recent-activity-header">
                    <strong>Recent</strong>
                  </div>

                  {visibleRecentActivity.length > 0 ? (
                    <div className="recent-activity-grid">
                      {visibleRecentActivity.map(renderRecentActivityCard)}
                    </div>
                  ) : (
                    <div className="recent-activity-empty">
                      <strong>No recent activity yet</strong>
                      <span>
                        Search something or open a result from the list, then it will show up
                        here.
                      </span>
                    </div>
                  )}
                </section>
              ) : (
                <div
                  ref={quickResultsStageRef}
                  className={
                    isQuickMode
                      ? `results-stage quick-results-stage ${showQuickEmptyState ? "is-empty" : ""}`
                      : "results-stage"
                  }
                  style={quickResultsStageStyle}
                >
                  <div
                    className={`results-list-shell ${isQuickMode ? "quick-results-column" : ""} ${isQuickMode && canLoadMore ? "has-overlay-load-more" : ""
                      }`}
                  >
                    <ul className={`results-list ${isQuickMode ? "quick-results-list" : ""}`}>
                      {visibleResults.map((result) => {
                        const rowKey = rowKeyForResult(result);
                        const isDirectory = result.isDirectory;
                        const isAppResult = isAppSearchResult(result);
                        const normalizedExt = normalizedExtension(result);
                        const iconKind = iconKindFromResult(result);
                        const extensionLabel = isAppResult
                          ? "app"
                          : isDirectory
                            ? "folder"
                            : normalizedExt
                              ? `.${normalizedExt}`
                              : "file";
                        const previewKind = showPreviews ? previewKindFromResult(result) : "none";
                        const previewSources =
                          isAssetPreviewKind(previewKind)
                            ? previewSourcesFromPath(result.path).filter(
                              (source, index, all) =>
                                source.length > 0 && all.indexOf(source) === index,
                            )
                            : [];
                        const activePreviewIndex = previewSourceState[rowKey] ?? 0;
                        const previewFailed = activePreviewIndex < 0;
                        const previewRenderKey = `${rowKey}:${activePreviewIndex}:${previewKind}`;
                        const previewReady = Boolean(previewReadyState[previewRenderKey]);
                        const previewSrc =
                          !previewFailed && isAssetPreviewKind(previewKind)
                            ? (previewSources[activePreviewIndex] ?? "")
                            : "";
                        const hasRenderablePreview = previewSrc.length > 0;
                        const appIconSrc = isAppResult
                          ? result.iconDataUrl ?? (result.appId ? appIconDataUrls[result.appId] ?? "" : "")
                          : "";
                        const canDragResultFile = !isDirectory && !isAppResult;
                        const canRevealLocation = canRevealResultLocation(result);
                        const contentSnippetKey =
                          hasContentSearchSyntax && contentSearchQuery && previewKind === "text"
                            ? textPreviewCacheKey(
                              result,
                              contentSearchQuery,
                              contentSearchMode,
                              CONTENT_SNIPPET_MAX_CHARS,
                            )
                            : "";
                        const contentSnippet = contentSnippetKey
                          ? textPreviewState[contentSnippetKey]?.matched
                            ? textPreviewState[contentSnippetKey]?.text ?? ""
                            : ""
                          : "";
                        const isBulkSelected = selectedPathsForBulk.has(result.path);

                        return (
                          <li
                            key={rowKey}
                            className={`result-row clickable ${isQuickMode ? "quick-result-row" : ""} ${(isQuickMode || quickLookOpen) && rowKey === selectedResultRowKey
                              ? "is-selected"
                              : ""
                              } ${canDragResultFile ? "draggable-file" : ""} ${isBulkSelected ? "is-bulk-selected" : ""}`}
                            data-result-key={rowKey}
                            draggable={canDragResultFile}
                            onDragStart={(event) => {
                              handleSearchResultDragStart(event, result);
                            }}
                            onContextMenu={(event) => {
                              openSearchResultContextMenu(event, result, rowKey);
                            }}
                            onMouseDown={(event) => {
                              if (event.button === 0) {
                                startLongPress(result);
                              }
                            }}
                            onMouseUp={cancelLongPress}
                            onMouseLeave={cancelLongPress}
                            role="button"
                            tabIndex={0}
                            title={
                              isQuickMode
                                ? "Click to select, press Space for Quick Look, double-click to open"
                                : "Click to select, press Space for Quick Look, double-click to open"
                            }
                            onFocus={() => {
                              setSelectedResultKey(rowKey);
                            }}
                            onClick={(event) => {
                              if (hasSelectedText()) {
                                return;
                              }
                              event.currentTarget.focus();
                              cancelLongPress();

                              if (longPressTriggeredRef.current) {
                                longPressTriggeredRef.current = false;
                                return;
                              }

                              const isApp = result.resultKind === "app" || isAppSearchResult(result);
                              if (!isApp && (isSelectionModeActive || event.ctrlKey || event.metaKey || event.shiftKey)) {
                                if (!isSelectionModeActive) {
                                  setIsSelectionModeActive(true);
                                }
                                if (event.shiftKey) {
                                  handleRangeSelection(result);
                                } else {
                                  toggleBulkSelection(result);
                                }
                                setSelectedResultKey(rowKey);
                              } else {
                                setSelectedResultKey(rowKey);
                              }
                            }}
                            onDoubleClick={() => {
                              if (hasSelectedText()) {
                                return;
                              }
                              void launchSearchResult(result);
                            }}
                            onKeyDown={(event) => {
                              if (event.key === "Enter") {
                                event.preventDefault();
                                void launchSearchResult(result);
                              } else if (event.key === " ") {
                                event.preventDefault();
                                if (isEditableTarget(event.target)) {
                                  return;
                                }
                                openQuickLook(rowKey, event.currentTarget);
                              }
                            }}
                          >
                            {showPreviews ? (
                              <div
                                className={`result-preview ${previewKind} ${isDirectory ? "folder" : ""} kind-${iconKind}`}
                                aria-hidden="true"
                              >
                                {isSelectionModeActive && !isAppResult ? (
                                  <div className={`bulk-checkbox-overlay ${isBulkSelected ? "is-selected" : ""}`}>
                                    {isBulkSelected && <span className="bulk-checkmark-icon">✓</span>}
                                  </div>
                                ) : null}
                                <ResultFallbackIcon result={result} className="preview-fallback-icon-shell" />
                                {appIconSrc.length > 0 ? (
                                  <img
                                    className="preview-media ready app-result-icon-media"
                                    src={appIconSrc}
                                    alt=""
                                    loading="lazy"
                                  />
                                ) : hasRenderablePreview && previewKind === "image" ? (
                                  <img
                                    key={`${rowKey}:${activePreviewIndex}:image`}
                                    className={`preview-media ${previewReady ? "ready" : ""}`}
                                    src={previewSrc}
                                    alt=""
                                    loading="lazy"
                                    onLoad={() => {
                                      handlePreviewReady(previewRenderKey);
                                    }}
                                    onError={() => {
                                      handlePreviewError(rowKey, previewSources.length);
                                    }}
                                  />
                                ) : null}
                                {hasRenderablePreview && previewKind === "video" ? (
                                  <video
                                    key={`${rowKey}:${activePreviewIndex}:video`}
                                    className={`preview-media ${previewReady ? "ready" : ""}`}
                                    src={previewSrc}
                                    muted
                                    playsInline
                                    preload="metadata"
                                    onLoadedData={() => {
                                      handlePreviewReady(previewRenderKey);
                                    }}
                                    onError={() => {
                                      handlePreviewError(rowKey, previewSources.length);
                                    }}
                                  />
                                ) : null}
                                {hasRenderablePreview && previewKind === "pdf" ? (
                                  <iframe
                                    key={`${rowKey}:${activePreviewIndex}:pdf`}
                                    className={`preview-media ${previewReady ? "ready" : ""}`}
                                    src={`${previewSrc}#toolbar=0&navpanes=0&scrollbar=0&page=1&view=FitH`}
                                    title=""
                                    loading="lazy"
                                    onLoad={() => {
                                      handlePreviewReady(previewRenderKey);
                                    }}
                                    onError={() => {
                                      handlePreviewError(rowKey, previewSources.length);
                                    }}
                                  />
                                ) : null}
                              </div>
                            ) : (
                              <div className={`result-icon ${isDirectory ? "folder" : ""} kind-${iconKind}`} aria-hidden="true">
                                {isSelectionModeActive && !isAppResult ? (
                                  <div className={`bulk-checkbox-overlay ${isBulkSelected ? "is-selected" : ""}`}>
                                    {isBulkSelected && <span className="bulk-checkmark-icon">✓</span>}
                                  </div>
                                ) : null}
                                {appIconSrc.length > 0 ? (
                                  <img className="result-icon-image" src={appIconSrc} alt="" loading="lazy" />
                                ) : (
                                  <ResultTypeIcon kind={iconKind} className="result-icon-glyph" />
                                )}
                              </div>
                            )}

                            <div className="result-main">
                              <strong>{highlightMatch(result.name, displayQuery)}</strong>
                              {contentSnippet ? (
                                <span className="result-content-snippet">
                                  {highlightMatch(contentSnippet, contentSearchQuery, "content")}
                                </span>
                              ) : null}
                              <span className="result-location">{highlightMatch(result.path, displayQuery)}</span>
                            </div>
                            <div className="result-meta">
                              <span className="meta-chip">{isAppResult ? "app" : extensionLabel}</span>
                              {isAppResult ? (
                                <span className="meta-chip">
                                  {result.revealPath ? "Desktop app" : "Installed app"}
                                </span>
                              ) : (
                                <>
                                  <span className="meta-chip">{formatBytes(result.size)}</span>
                                  <span className="meta-chip">{formatUnix(result.createdUnix)}</span>
                                </>
                              )}
                            </div>
                            <div className="result-actions">
                              <button
                                type="button"
                                className="row-action"
                                onClick={(event) => {
                                  event.stopPropagation();
                                  void launchSearchResult(result);
                                }}
                              >
                                Open
                              </button>
                              {canRevealLocation ? (
                                <button
                                  type="button"
                                  className="row-action"
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    void revealSearchResult(result);
                                  }}
                                >
                                  Folder
                                </button>
                              ) : null}
                            </div>
                          </li>
                        );
                      })}
                    </ul>

                    {isQuickMode && canLoadMore ? (
                      <div className="load-more-row quick-load-more-row">
                        <button type="button" className="load-more-button" onClick={loadMoreResults}>
                          {`Load more (+${defaultSearchLimit.toLocaleString()})`}
                        </button>
                      </div>
                    ) : null}
                  </div>

                  {quickSplitResizeEnabled ? (
                    <div
                      className={`quick-preview-splitter ${quickSplitDragging ? "is-dragging" : ""}`}
                      role="separator"
                      aria-orientation="vertical"
                      aria-label="Resize quick results and preview panels"
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={Math.round(effectiveQuickResultsPaneRatio * 100)}
                      tabIndex={0}
                      title="Drag to resize results and preview. Double-click to reset."
                      onMouseDown={(event) => {
                        event.preventDefault();
                        setQuickSplitDragging(true);
                      }}
                      onDoubleClick={() => {
                        setQuickResultsPaneRatio(QUICK_RESULTS_PANE_DEFAULT_RATIO);
                      }}
                      onKeyDown={(event) => {
                        if (event.key === "ArrowLeft") {
                          event.preventDefault();
                          adjustQuickResultsPaneRatio(-0.02);
                        } else if (event.key === "ArrowRight") {
                          event.preventDefault();
                          adjustQuickResultsPaneRatio(0.02);
                        } else if (event.key === "Home") {
                          event.preventDefault();
                          setQuickResultsPaneRatio(0.3);
                        } else if (event.key === "End") {
                          event.preventDefault();
                          setQuickResultsPaneRatio(0.7);
                        }
                      }}
                    >
                      <span className="quick-preview-splitter-grip" aria-hidden="true" />
                    </div>
                  ) : null}

                  {isQuickMode ? (
                    <aside className="quick-preview-panel" aria-label="Selected file preview">
                      {selectedResult ? (
                        <div className="quick-preview-surface">
                          <div
                            className={`quick-preview-stage ${selectedPreviewKind} ${selectedResultIsDirectory ? "folder" : ""
                              } kind-${selectedResultIconKind}`}
                          >
                            {!hasSelectedPreview && !hasSelectedTextPreview && selectedResultAppIconSrc.length === 0 ? (
                              <span className={`result-fallback-icon-shell kind-${selectedResultIconKind} quick-preview-fallback-icon-shell`} aria-hidden="true">
                                <ResultTypeIcon kind={selectedResultIconKind} className="result-fallback-icon" />
                              </span>
                            ) : null}
                            {selectedResultAppIconSrc.length > 0 ? (
                              <img
                                className="preview-media ready app-result-icon-media"
                                src={selectedResultAppIconSrc}
                                alt=""
                                loading="lazy"
                              />
                            ) : hasSelectedPreview && selectedPreviewKind === "image" ? (
                              <img
                                key={`${selectedResultRowKey}:${selectedPreviewIndex}:image:quick`}
                                className={`preview-media ${selectedPreviewReady ? "ready" : ""}`}
                                src={selectedPreviewSrc}
                                alt=""
                                loading="lazy"
                                onLoad={() => {
                                  handleSelectedPreviewReady(selectedPreviewRenderKey);
                                }}
                                onError={() => {
                                  handleSelectedPreviewError(selectedPreviewSources.length);
                                }}
                              />
                            ) : null}
                            {hasSelectedPreview && selectedPreviewKind === "video" ? (
                              <video
                                key={`${selectedResultRowKey}:${selectedPreviewIndex}:video:quick`}
                                className={`preview-media ${selectedPreviewReady ? "ready" : ""}`}
                                src={selectedPreviewSrc}
                                controls
                                muted
                                playsInline
                                preload="metadata"
                                onLoadedData={() => {
                                  handleSelectedPreviewReady(selectedPreviewRenderKey);
                                }}
                                onError={() => {
                                  handleSelectedPreviewError(selectedPreviewSources.length);
                                }}
                              />
                            ) : null}
                            {hasSelectedPreview && selectedPreviewKind === "pdf" ? (
                              <iframe
                                key={`${selectedResultRowKey}:${selectedPreviewIndex}:pdf:quick`}
                                className={`preview-media ${selectedPreviewReady ? "ready" : ""}`}
                                src={`${selectedPreviewSrc}#toolbar=0&navpanes=0&scrollbar=0&page=1&view=FitH`}
                                title={selectedResult.name}
                                loading="lazy"
                                onLoad={() => {
                                  handleSelectedPreviewReady(selectedPreviewRenderKey);
                                }}
                                onError={() => {
                                  handleSelectedPreviewError(selectedPreviewSources.length);
                                }}
                              />
                            ) : null}
                            {hasSelectedTextPreview && selectedPreviewKind === "text" ? (
                              <pre className="text-preview-content">
                                {highlightMatch(selectedTextPreview?.text ?? "", contentSearchQuery, "content")}
                              </pre>
                            ) : null}
                            {!hasSelectedPreview && !hasSelectedTextPreview && selectedResultAppIconSrc.length === 0 ? (
                              <div className="quick-preview-placeholder">
                                <strong>
                                  {selectedPreviewKind === "text" && selectedTextPreviewLoading
                                    ? "Loading text preview"
                                    : showPreviews
                                      ? "Preview unavailable"
                                      : "Preview disabled"}
                                </strong>
                                <span>
                                  {selectedPreviewKind === "text" && selectedTextPreviewLoading
                                    ? "Reading the file and preparing a text preview."
                                    : showPreviews
                                      ? "This file type cannot be rendered here yet, but you can still inspect the metadata and open it."
                                      : "Turn previews back on from the toolbar to load images, video, PDF, and text previews."}
                                </span>
                              </div>
                            ) : null}
                          </div>

                          <div className="quick-preview-body">
                            <div className="quick-preview-header">
                              <div className="quick-preview-copy">
                                <strong>{selectedResult.name}</strong>
                                <span>{selectedResult.path}</span>
                              </div>
                              <div className="quick-preview-actions">
                                <button
                                  type="button"
                                  className="row-action"
                                  onClick={() => {
                                    void launchSearchResult(selectedResult);
                                  }}
                                >
                                  {isAppSearchResult(selectedResult) ? "Open app" : "Open file"}
                                </button>
                                {canRevealResultLocation(selectedResult) ? (
                                  <button
                                    type="button"
                                    className="row-action"
                                    onClick={() => {
                                      void revealSearchResult(selectedResult);
                                    }}
                                  >
                                    Reveal folder
                                  </button>
                                ) : null}
                              </div>
                            </div>

                            <div className="quick-preview-meta">
                              <div className="quick-preview-meta-card">
                                <span>Type</span>
                                <strong>{isAppSearchResult(selectedResult) ? "app" : selectedResultExtensionLabel}</strong>
                              </div>
                              {isAppSearchResult(selectedResult) ? (
                                <>
                                  <div className="quick-preview-meta-card">
                                    <span>Kind</span>
                                    <strong>{selectedResult.revealPath ? "Desktop app" : "Installed app"}</strong>
                                  </div>
                                  <div className="quick-preview-meta-card quick-preview-meta-card-wide">
                                    <span>{selectedResultLocationLabel}</span>
                                    <strong>{selectedResultLocationText}</strong>
                                  </div>
                                </>
                              ) : (
                                <>
                                  <div className="quick-preview-meta-card">
                                    <span>Size</span>
                                    <strong>{formatBytes(selectedResult.size)}</strong>
                                  </div>
                                  <div className="quick-preview-meta-card">
                                    <span>Created</span>
                                    <strong>{formatUnix(selectedResult.createdUnix)}</strong>
                                  </div>
                                  <div className="quick-preview-meta-card">
                                    <span>Modified</span>
                                    <strong>{formatUnix(selectedResult.modifiedUnix)}</strong>
                                  </div>
                                </>
                              )}
                            </div>
                          </div>
                        </div>
                      ) : (
                        <div className="quick-preview-empty">
                          <strong>{quickPreviewEmptyTitle}</strong>
                          {quickPreviewEmptyDetail ? <span>{quickPreviewEmptyDetail}</span> : null}
                        </div>
                      )}
                    </aside>
                  ) : null}
                </div>
              )}

              {!isQuickMode && canLoadMore ? (
                <div className="load-more-row">
                  <button type="button" className="load-more-button" onClick={loadMoreResults}>
                    {`Load more (+${defaultSearchLimit.toLocaleString()})`}
                  </button>
                </div>
              ) : null}

              {isSelectionModeActive ? (
                <div className="bulk-action-bar" role="toolbar" aria-label="Bulk actions">
                  <div className="bulk-action-summary">
                    <strong>{selectedPathsForBulk.size}</strong>
                    <span>{selectedPathsForBulk.size === 1 ? "item selected" : "items selected"}</span>
                  </div>
                  <div className="bulk-action-buttons">
                    <button
                      type="button"
                      className="bulk-action-btn primary"
                      disabled={selectedPathsForBulk.size === 0}
                      onClick={() => setBulkRenameOpen(true)}
                    >
                      Rename
                    </button>
                    <button
                      type="button"
                      className="bulk-action-btn primary"
                      disabled={selectedPathsForBulk.size === 0 || !(syncServerInfo?.running && syncServerInfo.connectedClients > 0)}
                      onClick={handleBulkSendToPhone}
                    >
                      Send to phone
                    </button>
                    <button
                      type="button"
                      className="bulk-action-btn danger"
                      disabled={selectedPathsForBulk.size === 0}
                      onClick={() => setBulkDeleteOpen(true)}
                    >
                      Delete
                    </button>
                    <button
                      type="button"
                      className="bulk-action-btn secondary"
                      disabled={selectedPathsForBulk.size === 0}
                      onClick={handleBulkCopyPaths}
                    >
                      Copy paths
                    </button>
                    <button
                      type="button"
                      className="bulk-action-btn ghost"
                      onClick={() => {
                        setIsSelectionModeActive(false);
                        setSelectedPathsForBulk(new Set<string>());
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              ) : null}
            </section>
          </section>
        ) : null}

        {activeTab === "duplicates" ? (
          <section className="tab-panel" aria-label="Find duplicate files">
            {visibleStatusError ? <p className="error-row">{visibleStatusError}</p> : null}
            {driveError ? <p className="error-row">{driveError}</p> : null}

            <section className="duplicate-controls">
              <label className="duplicate-size-input">
                <span>Min file size (MB)</span>
                <NumberInputField
                  min={0}
                  step={1}
                  value={duplicateMinSizeMb}
                  placeholder="50"
                  ariaLabel="Minimum duplicate file size in megabytes"
                  onChange={setDuplicateMinSizeMb}
                />
              </label>
              <button
                type="button"
                className="ghost-button"
                disabled={!status.ready || duplicatesLoading || duplicateScanStatus.running}
                onClick={() => {
                  void findDuplicates();
                }}
              >
                {duplicatesLoading || duplicateScanStatus.running ? "Scanning..." : "Find duplicates"}
              </button>
              {duplicatesLoading || duplicateScanStatus.running ? (
                <button
                  type="button"
                  className="ghost-button danger-ghost-button"
                  disabled={duplicateScanStatus.cancelRequested}
                  onClick={() => {
                    void cancelDuplicateScan();
                  }}
                >
                  {duplicateScanStatus.cancelRequested ? "Cancelling..." : "Cancel scan"}
                </button>
              ) : null}
              {duplicateGroups.length > 0 && !duplicatesLoading && !duplicateScanStatus.running ? (
                <button type="button" className="ghost-button" onClick={clearDuplicateResults}>
                  Clear results
                </button>
              ) : null}
            </section>

            <section className="results-panel">
              <div className="results-toolbar">
                <div className="results-inline-stats" aria-live="polite">
                  <span>{`${duplicateStats.groupCount.toLocaleString()} groups`}</span>
                  <span>{`${duplicateStats.totalFiles.toLocaleString()} files`}</span>
                  <span>{`Reclaimable ${formatBytes(duplicateStats.reclaimableBytes)}`}</span>
                </div>
              </div>

              {showDuplicateProgress ? (
                <div
                  className={`duplicate-progress ${duplicateScanStatus.cancelRequested ? "is-cancelling" : ""}`}
                  aria-live="polite"
                >
                  <div className="duplicate-progress-top">
                    <span>
                      {duplicateScanStatus.cancelRequested
                        ? "Cancelling duplicate scan..."
                        : duplicatesLoading || duplicateScanStatus.running
                          ? "Scanning duplicate files..."
                          : "Last duplicate scan"}
                    </span>
                    <strong>{`${duplicateProgressPercent.toFixed(1)}%`}</strong>
                  </div>
                  <div
                    className="duplicate-progress-track"
                    role="progressbar"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(duplicateProgressPercent)}
                  >
                    <span
                      className="duplicate-progress-fill"
                      style={{ width: `${duplicateProgressPercent}%` }}
                    />
                  </div>
                  <div className="duplicate-progress-meta">
                    <span>{duplicateProgressLabel}</span>
                    <span>{`${duplicateScanStatus.groupsFound.toLocaleString()} groups found`}</span>
                  </div>
                </div>
              ) : null}

              {duplicatesLoading ? <p className="hint compact-hint">Scanning for duplicates...</p> : null}
              {duplicateNotice ? <p className="info-row">{duplicateNotice}</p> : null}
              {duplicatesError ? <p className="error-row">{duplicatesError}</p> : null}
              {!duplicatesLoading && !duplicatesError && duplicateGroups.length === 0 ? (
                <p className="hint compact-hint">
                  Run a duplicate scan to group files with identical content.
                </p>
              ) : null}

              <ul className="results-list">
                {duplicateGroups.flatMap((group, groupIndex) => {
                  const hiddenCount = Math.max(0, group.fileCount - group.files.length);
                  const renderedRows: ReactNode[] = [];
                  renderedRows.push(
                    <li key={`${group.groupId}:summary`} className="result-row duplicate-summary-row">
                      <div className="result-icon duplicate-group-icon">DP</div>
                      <div className="result-main duplicate-group-main">
                        <span className="duplicate-group-label">Group</span>
                        <strong>{`${group.fileCount.toLocaleString()} matching files`}</strong>
                        <span>
                          {hiddenCount > 0
                            ? `${hiddenCount.toLocaleString()} files hidden for performance`
                            : "All files in this group are shown"}
                        </span>
                      </div>
                      <div className="result-meta">
                        <span className="meta-chip">{`${formatBytes(group.size)} each`}</span>
                        <span className="meta-chip">{`${formatBytes(group.totalBytes)} total`}</span>
                        <span className="meta-chip">
                          {`Reclaimable ${formatBytes(Math.max(0, group.fileCount - 1) * group.size)}`}
                        </span>
                      </div>
                      <div className="result-actions" />
                    </li>,
                  );

                  if (group.files.length === 0) {
                    renderedRows.push(
                      <li key={`${group.groupId}:empty`} className="result-row duplicate-empty-row">
                        <div className="result-icon">--</div>
                        <div className="result-main">
                          <strong>No files available in this group</strong>
                          <span>Try running scan again.</span>
                        </div>
                        <div className="result-meta" />
                        <div className="result-actions" />
                      </li>,
                    );
                  } else {
                    for (let fileIndex = 0; fileIndex < group.files.length; fileIndex += 1) {
                      const file = group.files[fileIndex];
                      const cleanedPath = stripInvisibleText(file.path);
                      const cleanedName = stripInvisibleText(file.name);
                      const hasPath = cleanedPath.trim().length > 0;
                      const filePath = hasPath ? cleanedPath.trim() : "(path unavailable)";
                      const fileNameFromPath = hasPath ? basenameFromPath(cleanedPath) : "";
                      const fileName =
                        cleanedName.trim() || fileNameFromPath || "(unknown file name)";
                      const rowKey = `${group.groupId}:${cleanedPath || cleanedName || fileIndex}`;

                      renderedRows.push(
                        <li
                          key={rowKey}
                          className="result-row clickable duplicate-file-row-flat"
                          role="button"
                          tabIndex={0}
                          title="Click to reveal in folder, double-click to open"
                          onClick={() => {
                            if (hasPath) {
                              if (hasSelectedText()) {
                                return;
                              }
                              void revealResult(cleanedPath);
                            }
                          }}
                          onDoubleClick={() => {
                            if (hasPath) {
                              if (hasSelectedText()) {
                                return;
                              }
                              void openResult(cleanedPath);
                            }
                          }}
                          onKeyDown={(event) => {
                            if (!hasPath) {
                              return;
                            }
                            if (event.key === "Enter") {
                              event.preventDefault();
                              void openResult(cleanedPath);
                            } else if (event.key === " ") {
                              event.preventDefault();
                              void revealResult(cleanedPath);
                            }
                          }}
                        >
                          <div className="result-icon">DU</div>
                          <div className="result-main">
                            <strong title={fileName}>{fileName}</strong>
                            <span title={filePath}>{filePath}</span>
                          </div>
                          <div className="result-meta">
                            <span className="meta-chip">{formatBytes(file.size)}</span>
                            <span className="meta-chip">{formatUnix(file.modifiedUnix)}</span>
                          </div>
                          <div className="result-actions">
                            <button
                              type="button"
                              className="row-action"
                              disabled={!hasPath}
                              onClick={(event) => {
                                event.stopPropagation();
                                if (hasPath) {
                                  void openResult(cleanedPath);
                                }
                              }}
                            >
                              Open
                            </button>
                            <button
                              type="button"
                              className="row-action"
                              disabled={!hasPath}
                              onClick={(event) => {
                                event.stopPropagation();
                                if (hasPath) {
                                  void revealResult(cleanedPath);
                                }
                              }}
                            >
                              Folder
                            </button>
                            <button
                              type="button"
                              className="row-action danger-row-action"
                              disabled={!hasPath}
                              title="Delete"
                              aria-label="Delete duplicate"
                              onClick={(event) => {
                                event.stopPropagation();
                                if (!hasPath) {
                                  return;
                                }
                                setDuplicateDeleteToRecycleBin(false);
                                setDuplicateDeleteCandidate({
                                  groupId: group.groupId,
                                  path: cleanedPath.trim(),
                                  name: fileName,
                                  size: file.size,
                                });
                              }}
                            >
                              <svg viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M9 3h6l1 2h4v2H4V5h4l1-2zm1 7h2v8h-2v-8zm4 0h2v8h-2v-8zM7 10h2v8H7v-8z" />
                              </svg>
                            </button>
                          </div>
                        </li>,
                      );
                    }
                  }

                  if (groupIndex < duplicateGroups.length - 1) {
                    renderedRows.push(
                      <li key={`${group.groupId}:divider`} className="duplicate-group-divider" />,
                    );
                  }

                  return renderedRows;
                })}
              </ul>
            </section>

            {duplicateDeleteCandidate ? (
              <div
                className="modal-overlay"
                role="dialog"
                aria-modal="true"
                onClick={() => {
                  if (!duplicateDeleteBusy) {
                    setDuplicateDeleteCandidate(null);
                  }
                }}
              >
                <div
                  className="modal-card"
                  onClick={(event) => {
                    event.stopPropagation();
                  }}
                >
                  <h3>Delete duplicate?</h3>
                  <p>
                    {duplicateDeleteCandidate.name || "Selected file"}
                  </p>
                  <p className="modal-path">{duplicateDeleteCandidate.path}</p>
                  <p className="modal-meta">
                    {formatBytes(duplicateDeleteCandidate.size)}
                  </p>
                  <label className="modal-checkbox-option">
                    <input
                      type="checkbox"
                      checked={duplicateDeleteToRecycleBin}
                      disabled={duplicateDeleteBusy}
                      onChange={(event) => {
                        setDuplicateDeleteToRecycleBin(event.currentTarget.checked);
                      }}
                    />
                    <span>Move to Recycle Bin</span>
                  </label>
                  <div className="modal-actions">
                    <button
                      type="button"
                      className="ghost-button"
                      disabled={duplicateDeleteBusy}
                      onClick={() => {
                        setDuplicateDeleteCandidate(null);
                      }}
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      className="row-action danger-row-action"
                      disabled={duplicateDeleteBusy}
                      onClick={() => {
                        void confirmDuplicateDelete();
                      }}
                    >
                      {duplicateDeleteBusy
                        ? duplicateDeleteToRecycleBin
                          ? "Moving..."
                          : "Deleting..."
                        : duplicateDeleteToRecycleBin
                          ? "Recycle"
                          : "Delete"}
                    </button>
                  </div>
                </div>
              </div>
            ) : null}
          </section>
        ) : null}

        {activeTab === "advanced" ? (
          <section className="tab-panel scrollable-tab-panel" aria-label="App settings">
            <div className="about-panel advanced-panel">
              <div className="about-header">
                <div>
                  <h2>Settings</h2>
                  <p className="about-tagline">
                    Manage desktop behavior and default result count for OmniSearch.
                  </p>
                </div>
              </div>

              <div className="advanced-settings">
                <div className="advanced-settings-section">
                  <div className="advanced-section-header">
                    <div>
                      <h3>Desktop behavior</h3>
                      <p className="advanced-note">
                        Control the tray behavior and the global shortcut that opens the quick
                        window as a normal desktop window.
                      </p>
                    </div>
                    <span className="theme-mode-status">
                      {desktopSettings.shortcutEnabled
                        ? `Shortcut: ${formattedDesktopShortcut}`
                        : "Shortcut disabled"}
                    </span>
                  </div>

                  <div className="desktop-settings-grid">
                    <button
                      type="button"
                      className={`settings-switch-card settings-switch-card-button ${desktopSettingsDraft.backgroundModeEnabled ? "is-active" : ""
                        }`}
                      role="switch"
                      aria-checked={desktopSettingsDraft.backgroundModeEnabled}
                      disabled={desktopSettingsSaving || desktopSettingsLoading}
                      onClick={() => {
                        setDesktopSettingsDraft((previous) => ({
                          ...previous,
                          backgroundModeEnabled: !previous.backgroundModeEnabled,
                        }));
                        setDesktopSettingsError(null);
                        setDesktopSettingsMessage(null);
                      }}
                    >
                      <div className="settings-switch-copy">
                        <strong>Keep app running in the background</strong>
                        <span>
                          When enabled, clicking the window X button hides OmniSearch to the tray
                          instead of quitting.
                        </span>
                      </div>
                      <span
                        className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${desktopSettingsDraft.backgroundModeEnabled ? "is-on" : ""
                          }`}
                        aria-hidden="true"
                      >
                        <span className="scan-switch-slider" aria-hidden="true" />
                        <span>{desktopSettingsDraft.backgroundModeEnabled ? "On" : "Off"}</span>
                      </span>
                    </button>

                    <button
                      type="button"
                      className={`settings-switch-card settings-switch-card-button ${desktopSettingsDraft.shortcutEnabled ? "is-active" : ""
                        }`}
                      role="switch"
                      aria-checked={desktopSettingsDraft.shortcutEnabled}
                      disabled={desktopSettingsSaving || desktopSettingsLoading}
                      onClick={() => {
                        setDesktopSettingsDraft((previous) => ({
                          ...previous,
                          shortcutEnabled: !previous.shortcutEnabled,
                        }));
                        setDesktopSettingsError(null);
                        setDesktopSettingsMessage(null);
                      }}
                    >
                      <div className="settings-switch-copy">
                        <strong>Enable global shortcut</strong>
                        <span>
                          Register a system-wide hotkey that opens the quick search window without
                          using overlay or always-on-top behavior.
                        </span>
                      </div>
                      <span
                        className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${desktopSettingsDraft.shortcutEnabled ? "is-on" : ""
                          }`}
                        aria-hidden="true"
                      >
                        <span className="scan-switch-slider" aria-hidden="true" />
                        <span>{desktopSettingsDraft.shortcutEnabled ? "On" : "Off"}</span>
                      </span>
                    </button>

                    <button
                      type="button"
                      className={`settings-switch-card settings-switch-card-button ${desktopSettingsDraft.rememberWindowBounds ? "is-active" : ""
                        }`}
                      role="switch"
                      aria-checked={desktopSettingsDraft.rememberWindowBounds}
                      disabled={
                        desktopSettingsSaving || desktopSettingsLoading || desktopLayoutResetting
                      }
                      onClick={() => {
                        setDesktopSettingsDraft((previous) => ({
                          ...previous,
                          rememberWindowBounds: !previous.rememberWindowBounds,
                        }));
                        setDesktopSettingsError(null);
                        setDesktopSettingsMessage(null);
                      }}
                    >
                      <div className="settings-switch-copy">
                        <strong>Remember full window size and position</strong>
                        <span>
                          Reopen the full workspace where you last left it. Quick Window keeps its
                          fixed launcher-style layout.
                        </span>
                      </div>
                      <span
                        className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${desktopSettingsDraft.rememberWindowBounds ? "is-on" : ""
                          }`}
                        aria-hidden="true"
                      >
                        <span className="scan-switch-slider" aria-hidden="true" />
                        <span>{desktopSettingsDraft.rememberWindowBounds ? "On" : "Off"}</span>
                      </span>
                    </button>

                    <label className="desktop-shortcut-field" htmlFor="desktop-shortcut-input">
                      <span>Shortcut</span>
                      <input
                        id="desktop-shortcut-input"
                        type="text"
                        value={desktopSettingsDraft.shortcut}
                        disabled={
                          desktopSettingsSaving || desktopSettingsLoading || desktopLayoutResetting
                        }
                        placeholder={DEFAULT_DESKTOP_SETTINGS.shortcut}
                        autoComplete="off"
                        autoCapitalize="none"
                        autoCorrect="off"
                        spellCheck={false}
                        onChange={(event) => {
                          const { value } = event.currentTarget;
                          setDesktopSettingsDraft((previous) => ({
                            ...previous,
                            shortcut: value,
                          }));
                          setDesktopSettingsError(null);
                          setDesktopSettingsMessage(null);
                        }}
                      />
                      <small className="desktop-shortcut-hint">
                        Type a shortcut like <code>Alt+Shift+S</code> or <code>Ctrl+Alt+S</code>.
                        The saved shortcut updates immediately after you save.
                      </small>
                    </label>

                    <div className="advanced-settings-actions">
                      <button
                        type="button"
                        className={`ghost-button ${desktopSettingsDirty ? "is-pending" : ""}`}
                        disabled={
                          desktopSettingsSaving ||
                          desktopSettingsLoading ||
                          desktopLayoutResetting ||
                          !desktopSettingsDirty
                        }
                        onClick={() => {
                          void saveDesktopBehaviorSettings();
                        }}
                      >
                        {desktopSettingsSaving ? "Saving..." : "Save desktop settings"}
                      </button>
                      <button
                        type="button"
                        className={`ghost-button ${desktopSettingsDirty ? "is-pending" : ""}`}
                        disabled={
                          desktopSettingsSaving ||
                          desktopSettingsLoading ||
                          desktopLayoutResetting ||
                          !desktopSettingsDirty
                        }
                        onClick={() => {
                          setDesktopSettingsDraft(desktopSettings);
                          setDesktopSettingsError(null);
                          setDesktopSettingsMessage(null);
                        }}
                      >
                        Reset changes
                      </button>
                      <button
                        type="button"
                        className="ghost-button"
                        disabled={
                          desktopSettingsSaving || desktopSettingsLoading || desktopLayoutResetting
                        }
                        onClick={() => {
                          void resetDesktopWindowLayout();
                        }}
                      >
                        {desktopLayoutResetting ? "Resetting layout..." : "Reset window layout"}
                      </button>
                    </div>
                    {desktopSettingsDirty ? (
                      <p className="advanced-pending">
                        Pending desktop setting changes. Click Save desktop settings to apply them.
                      </p>
                    ) : null}
                  </div>

                  <p className="advanced-note">
                    Tray menu includes Open Quick Window, Open Main App, Hide, and Quit. Shortcut
                    changes apply immediately after saving. Quick Window always keeps its fixed
                    launcher layout.
                  </p>
                  {desktopSettingsLoading ? (
                    <p className="advanced-note">Loading desktop settings...</p>
                  ) : null}
                  {desktopSettingsError ? (
                    <p className="advanced-error">{desktopSettingsError}</p>
                  ) : null}
                  {desktopSettingsMessage ? (
                    <p className="advanced-success">{desktopSettingsMessage}</p>
                  ) : null}
                </div>

                <div className="advanced-settings-section">
                  <label htmlFor="search-limit-input">
                    Results per search (range {SEARCH_LIMIT_MIN} - {SEARCH_LIMIT_MAX})
                  </label>
                  <NumberInputField
                    id="search-limit-input"
                    min={SEARCH_LIMIT_MIN}
                    max={SEARCH_LIMIT_MAX}
                    step={50}
                    value={searchLimitInput}
                    ariaLabel="Results per search"
                    onChange={(value) => {
                      setSearchLimitInput(value);
                      if (searchLimitError) {
                        setSearchLimitError(null);
                      }
                      if (searchLimitMessage) {
                        setSearchLimitMessage(null);
                      }
                    }}
                  />
                  <div className="advanced-settings-actions">
                    <button
                      type="button"
                      className={`ghost-button ${searchLimitHasPendingChanges ? "is-pending" : ""}`}
                      disabled={!searchLimitHasPendingChanges}
                      onClick={applySearchLimitPreference}
                    >
                      {searchLimitHasPendingChanges ? "Apply update" : "Updated"}
                    </button>
                    <button
                      type="button"
                      className={`ghost-button ${searchLimitCanResetToDefault ? "is-pending" : ""}`}
                      disabled={!searchLimitCanResetToDefault}
                      onClick={resetSearchLimitPreference}
                    >
                      Reset default ({SEARCH_LIMIT})
                    </button>
                  </div>
                  {searchLimitHasPendingChanges ? (
                    <p className="advanced-pending">
                      {pendingSearchLimit === null
                        ? "Pending update. Enter a valid number, then click Apply update."
                        : searchLimitValueNeedsNormalization
                          ? `Pending update. Click Apply update to normalize this value to ${pendingSearchLimit.toLocaleString()}.`
                          : `Pending update. Click Apply update to use ${pendingSearchLimit.toLocaleString()} results by default.`}
                    </p>
                  ) : null}
                  <p className="advanced-note">
                    {`Current default: ${defaultSearchLimit.toLocaleString()} | Current active limit: ${searchLimit.toLocaleString()}`}
                  </p>
                  <p className="advanced-note">Load more uses this same amount each click.</p>
                  {searchLimitError ? <p className="advanced-error">{searchLimitError}</p> : null}
                  {searchLimitMessage ? <p className="advanced-success">{searchLimitMessage}</p> : null}
                </div>

                <div className="advanced-settings-section">
                  <div className="advanced-section-header">
                    <div>
                      <h3>Installed app search</h3>
                      <p className="advanced-note">
                        Search and launch installed apps alongside files in both the full workspace
                        and Quick Window.
                      </p>
                    </div>
                    <span className="theme-mode-status">
                      {includeInstalledApps
                        ? installedAppsLoading
                          ? "Loading apps..."
                          : `${installedApps.length.toLocaleString()} apps ready`
                        : "App search off"}
                    </span>
                  </div>

                  <button
                    type="button"
                    className={`settings-switch-card settings-switch-card-button ${includeInstalledApps ? "is-active" : ""
                      }`}
                    role="switch"
                    aria-checked={includeInstalledApps}
                    onClick={() => {
                      setIncludeInstalledApps((previous) => !previous);
                    }}
                  >
                    <div className="settings-switch-copy">
                      <strong>Include installed apps in search results</strong>
                      <span>
                        Shows launchable apps with their real icons.
                      </span>
                    </div>
                    <span
                      className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${includeInstalledApps ? "is-on" : ""
                        }`}
                      aria-hidden="true"
                    >
                      <span className="scan-switch-slider" aria-hidden="true" />
                      <span>{includeInstalledApps ? "On" : "Off"}</span>
                    </span>
                  </button>

                  <div className="advanced-settings-actions">
                    <button
                      type="button"
                      className="ghost-button"
                      disabled={!includeInstalledApps || installedAppsLoading}
                      onClick={() => {
                        setAppIconFailures({});
                        setInstalledAppsRefreshKey((current) => current + 1);
                      }}
                    >
                      {installedAppsLoading ? "Refreshing apps..." : "Refresh installed apps"}
                    </button>
                  </div>
                  <p className="advanced-note">
                    Apps do not use the file index. OmniSearch reads the Windows app catalog and
                    merges app matches into the same Search view.
                  </p>
                  {installedAppsError ? <p className="advanced-error">{installedAppsError}</p> : null}
                </div>

                <div className="advanced-settings-section">
                  <div className="advanced-section-header">
                    <div>
                      <h3>Search history</h3>
                      <p className="advanced-note">
                        Show recent searches and opened results in the full workspace when the
                        search box is empty.
                      </p>
                    </div>
                    <span className="theme-mode-status">
                      {recentActivityEnabled ? "History on" : "History off"}
                    </span>
                  </div>

                  <button
                    type="button"
                    className={`settings-switch-card settings-switch-card-button ${recentActivityEnabled ? "is-active" : ""
                      }`}
                    role="switch"
                    aria-checked={recentActivityEnabled}
                    onClick={() => {
                      setRecentActivityEnabled((previous) => !previous);
                    }}
                  >
                    <div className="settings-switch-copy">
                      <strong>Show recent searches in the empty search area</strong>
                      <span>
                        Keeps up to {RECENT_ACTIVITY_LIMIT} recent searches and opened files or
                        apps ready to reopen from the Search tab.
                      </span>
                    </div>
                    <span
                      className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${recentActivityEnabled ? "is-on" : ""
                        }`}
                      aria-hidden="true"
                    >
                      <span className="scan-switch-slider" aria-hidden="true" />
                      <span>{recentActivityEnabled ? "On" : "Off"}</span>
                    </span>
                  </button>
                  <p className="advanced-note">
                    Turn this off to hide the recent panel and stop adding new history entries.
                  </p>
                </div>
              </div>
            </div>
          </section>
        ) : null}

        {activeTab === "themes" ? (
          <section className="tab-panel scrollable-tab-panel" aria-label="Theme gallery">
            <div className="about-panel advanced-panel">
              <div className="about-header">
                <div>
                  <h2>Themes</h2>
                  <p className="about-tagline">
                    Pick a complete app style for OmniSearch. Every preset adapts to both dark and
                    light mode.
                  </p>
                </div>
                <div className="theme-header-tools">
                  <span className="theme-mode-status">
                    {themeMode === "dark" ? "Dark mode active" : "Light mode active"}
                  </span>
                  <button
                    type="button"
                    className="theme-toggle compact-theme-toggle"
                    onClick={toggleThemeMode}
                    aria-label={themeMode === "dark" ? "Switch to light mode" : "Switch to dark mode"}
                    title={themeMode === "dark" ? "Switch to light mode" : "Switch to dark mode"}
                  >
                    <span className="theme-toggle-dot" aria-hidden="true" />
                    <span>{themeMode === "dark" ? "Light mode" : "Dark mode"}</span>
                  </button>
                </div>
              </div>

              <div className="advanced-settings">
                <div className="advanced-settings-section">
                  <div className="theme-grid" aria-label="Theme presets">
                    {THEME_PRESET_IDS.map((presetId) => {
                      const preset = themePresetById(presetId);
                      const isActive = preset.id === themePreset;

                      return (
                        <button
                          key={preset.id}
                          type="button"
                          className={`theme-card ${isActive ? "is-active" : ""}`}
                          aria-pressed={isActive}
                          onClick={() => {
                            setThemePreset(preset.id);
                          }}
                        >
                          <div className="theme-card-preview" aria-hidden="true">
                            <div className="theme-mini" style={themePreviewStyle(preset.preview.dark)}>
                              <div className="theme-mini-header">
                                <span className="theme-mini-label">Dark</span>
                                <span className="theme-mini-dot" />
                              </div>
                              <div className="theme-mini-search-bar" />
                              <div className="theme-mini-tabs">
                                <span className="theme-mini-pill theme-mini-pill-accent" />
                                <span className="theme-mini-pill" />
                              </div>
                              <div className="theme-mini-list">
                                <span className="theme-mini-line" />
                                <span className="theme-mini-line theme-mini-line-short" />
                              </div>
                            </div>

                            <div className="theme-mini" style={themePreviewStyle(preset.preview.light)}>
                              <div className="theme-mini-header">
                                <span className="theme-mini-label">Light</span>
                                <span className="theme-mini-dot" />
                              </div>
                              <div className="theme-mini-search-bar" />
                              <div className="theme-mini-tabs">
                                <span className="theme-mini-pill theme-mini-pill-accent" />
                                <span className="theme-mini-pill" />
                              </div>
                              <div className="theme-mini-list">
                                <span className="theme-mini-line" />
                                <span className="theme-mini-line theme-mini-line-short" />
                              </div>
                            </div>
                          </div>

                          <div className="theme-card-body">
                            <div className="theme-card-copy">
                              <strong>{preset.label}</strong>
                              <span>{preset.description}</span>
                            </div>
                            <span className="theme-card-tag">
                              {isActive ? "Selected" : "Apply"}
                            </span>
                          </div>
                        </button>
                      );
                    })}
                  </div>

                  <p className="advanced-note">
                    {`Current preset: ${activeThemePreset.label}. Use the header toggle anytime to switch between its dark and light versions.`}
                  </p>
                </div>

              </div>
            </div>
          </section>
        ) : null}

        {activeTab === "syntax" ? (
          <section className="tab-panel scrollable-tab-panel" aria-label="OmniSearch query syntax help">
            <div className="about-panel advanced-panel syntax-help-page">
              <div className="about-header">
                <div>
                  <h2>Search Syntax</h2>
                  <p className="about-tagline">
                    Only operators currently supported in OmniSearch are shown here.
                  </p>
                </div>
                <span className="theme-mode-status">Examples jump back to Search</span>
              </div>

              <div className="syntax-help-layout syntax-help-layout-panel">
                <aside className="syntax-help-sidebar" aria-label="Syntax topics">
                  {SEARCH_SYNTAX_HELP_SECTIONS.map((section) => (
                    <button
                      key={section.id}
                      type="button"
                      className={`syntax-help-topic ${activeSearchSyntaxHelpSection.id === section.id ? "is-active" : ""
                        }`}
                      onClick={() => {
                        setSearchSyntaxHelpSectionId(section.id);
                      }}
                    >
                      <span className="syntax-help-topic-label">{section.label}</span>
                      <small>{section.title}</small>
                    </button>
                  ))}
                </aside>

                <section className="syntax-help-body">
                  <div className="syntax-help-body-header">
                    <div>
                      <strong>{activeSearchSyntaxHelpSection.title}</strong>
                      <p>{activeSearchSyntaxHelpSection.summary}</p>
                    </div>
                    {activeSearchSyntaxHelpSection.syntax ? (
                      <code className="syntax-help-code">{activeSearchSyntaxHelpSection.syntax}</code>
                    ) : null}
                  </div>

                  <div className="syntax-help-section">
                    <span className="syntax-help-section-label">How it works</span>
                    <ul className="syntax-help-list">
                      {activeSearchSyntaxHelpSection.details.map((detail) => (
                        <li key={detail}>{detail}</li>
                      ))}
                    </ul>
                  </div>

                  <div className="syntax-help-section">
                    <span className="syntax-help-section-label">Examples</span>
                    <div className="syntax-help-example-grid">
                      {activeSearchSyntaxHelpSection.examples.map((example) => (
                        <button
                          key={example}
                          type="button"
                          className="syntax-help-example"
                          onClick={() => {
                            applySearchSyntaxExample(example);
                          }}
                          title="Use this example in the search box"
                        >
                          <code>{example}</code>
                          <span>Use example</span>
                        </button>
                      ))}
                    </div>
                  </div>

                  {activeSearchSyntaxHelpSection.notes?.length ? (
                    <div className="syntax-help-section">
                      <span className="syntax-help-section-label">Notes</span>
                      <ul className="syntax-help-list is-notes">
                        {activeSearchSyntaxHelpSection.notes.map((note) => (
                          <li key={note}>{note}</li>
                        ))}
                      </ul>
                    </div>
                  ) : null}
                </section>
              </div>
            </div>
          </section>
        ) : null}

        {activeTab === "about" ? (
          <section className="tab-panel scrollable-tab-panel" aria-label="About OmniSearch and developer">
            <div className="about-panel about-panel-flat sync-panel">
              <div className="sync-title-row">
                <div>
                  <h2>About OmniSearch</h2>
                  <p className="about-tagline">
                    Fast local search across your drives with rich filters.
                  </p>
                  <div className="about-version">
                    <span className="about-version-label">Version</span>
                    <span className="about-version-value">
                      {appVersion ? `v${appVersion}` : "Loading..."}
                    </span>
                  </div>
                </div>
                <div className="about-developer">
                  <span className="about-label">Built by</span>
                  <span className="about-name">{DEVELOPER_NAME}</span>
                </div>
              </div>
              <div className="social-links">
                {SOCIAL_LINKS.map((item) => (
                  <button
                    key={item.url}
                    type="button"
                    className="social-link"
                    onClick={() => {
                      void openExternalLink(item.url);
                    }}
                  >
                    <SocialIcon icon={item.icon} />
                    <span>{item.label}</span>
                  </button>
                ))}
              </div>

              <div className="about-sections">
                <section className="about-support-card" aria-label="Support the developer">
                  <div className="about-support-copy">
                    <span className="about-support-label">Support</span>
                    <strong>Sponsor OmniSearch</strong>
                    <p>
                      If OmniSearch helps your workflow, you can support future updates and desktop
                      tools here.
                    </p>
                  </div>
                  <div className="about-support-actions">
                    <button
                      type="button"
                      className="about-support-button is-sponsor"
                      onClick={() => {
                        void openExternalLink(GITHUB_SPONSOR_URL);
                      }}
                    >
                      <GitHubSponsorIcon />
                      <span>Sponsor on GitHub</span>
                    </button>
                    <button
                      type="button"
                      className="about-support-button is-bmc"
                      onClick={() => {
                        void openExternalLink(DONATE_URL);
                      }}
                    >
                      <BuyMeCoffeeIcon />
                      <span>Buy me a coffee</span>
                    </button>
                  </div>
                </section>

                <section
                  className="about-apps-section about-apps-section-flat"
                  aria-label="More apps by Eyuel"
                >
                  <div className="about-section-heading">
                    <h3>More Apps by Eyuel Engida</h3>
                    <p>Other desktop apps.</p>
                  </div>
                  <div className="developer-app-grid">
                    {MORE_APPS.map((item) => (
                      <button
                        key={item.url}
                        type="button"
                        className={`developer-app-card is-${item.accent}`}
                        onClick={() => {
                          void openExternalLink(item.url);
                        }}
                      >
                        <div className="developer-app-card-top">
                          <span className={`developer-app-icon-shell is-${item.accent}`} aria-hidden="true">
                            <img
                              className="developer-app-icon-image"
                              src={item.iconSrc}
                              alt=""
                              draggable={false}
                            />
                          </span>
                          <div className="developer-app-copy">
                            <strong>{item.name}</strong>
                            <span>{item.blurb}</span>
                          </div>
                        </div>
                        <div className="developer-app-card-footer">
                          <span className="developer-app-link">
                            <MicrosoftStoreIcon />
                            <span>Store</span>
                          </span>
                        </div>
                      </button>
                    ))}
                  </div>
                </section>
              </div>
            </div>
          </section>
        ) : null}

        {activeTab === "sync" ? (
          <section className="tab-panel scrollable-tab-panel" aria-label="Mobile Sync Settings">
            <div className="about-panel about-panel-flat sync-panel">
              <div className="about-header">
                <div>
                  <h2>Mobile App Sync</h2>
                  <p className="about-tagline">
                    Connect the native OmniSearch Android companion app to search, view duplicates, and trigger actions remotely.
                  </p>
                  <div className="sync-status-row">
                    <span className={`sync-status-pill ${syncServerInfo?.running ? "live" : "idle"}`}>
                      <span className="status-dot" aria-hidden="true" />
                      {syncServerInfo?.running ? "Server running" : "Server stopped"}
                    </span>
                    {syncServerInfo?.running ? (
                      <>
                        <span className={`sync-status-pill ${syncServerInfo.connectedClients > 0 ? "live" : "idle"}`}>
                          <span className="status-dot" aria-hidden="true" />
                          {syncServerInfo.connectedClients > 0
                            ? `${syncServerInfo.connectedClients} phone${syncServerInfo.connectedClients === 1 ? "" : "s"} connected`
                            : "Waiting for phone"}
                        </span>
                        <button
                          type="button"
                          className="sync-icon-button"
                          title="Refresh server status and IP address"
                          disabled={syncBusy}
                          onClick={async () => {
                            setSyncBusy(true);
                            try {
                              const info = await invoke<SyncServerInfo>("get_mobile_sync_server_info");
                              setSyncServerInfo(info);
                            } catch (err) {
                              setSyncError(String(err));
                            } finally {
                              setSyncBusy(false);
                            }
                          }}
                        >
                          <svg
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            style={{ animation: syncBusy ? "spin 1s linear infinite" : "none" }}
                          >
                            <path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67" />
                          </svg>
                        </button>
                      </>
                    ) : null}
                  </div>
                </div>
              </div>
              {syncError && (
                <div className="error-row" style={{ margin: "18px 0 0", padding: "12px", borderRadius: "8px", backgroundColor: "rgba(255, 117, 117, 0.12)", border: "1px solid rgba(255, 117, 117, 0.35)", color: "var(--danger)" }}>
                  {syncError}
                </div>
              )}

              {!syncServerInfo || !syncServerInfo.running ? (
                <section className="sync-empty-state" aria-label="Start Sync Server">
                  <div className="sync-detail-group">
                    <span className="sync-label">Setup</span>
                    <p>
                      Start the local sync server, then scan the QR code from the Android companion app while both devices are on the same Wi-Fi network.
                    </p>
                  </div>
                  <div className="sync-actions-row">
                    <button
                      type="button"
                      className="sync-action-button primary"
                      disabled={syncBusy}
                      onClick={startSyncServer}
                    >
                      {syncBusy ? "Starting..." : "Start Sync Server"}
                    </button>
                  </div>
                </section>
              ) : (
                <div className="about-sections" style={{ marginTop: 0, width: "100%" }}>
                  <div style={{ display: "flex", justifyContent: "center", width: "100%", marginTop: "8px" }}>
                    <section
                      className="sync-card"
                      aria-label="Scan QR Code"
                      style={{
                        display: "flex",
                        flexDirection: "column",
                        alignItems: "center",
                        textAlign: "center",
                        width: "min(340px, 100%)",
                        padding: "20px 20px 16px",
                        gap: "0"
                      }}
                    >
                      {syncServerInfo.connectedClients > 0 ? (
                        <div style={{ display: "flex", flexDirection: "column", alignItems: "center", padding: "16px 0 8px", gap: "10px" }}>
                          <div style={{
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "center",
                            width: "48px",
                            height: "48px",
                            borderRadius: "50%",
                            background: "rgba(34, 197, 94, 0.12)",
                            color: "#22c55e",
                            border: "1.5px solid rgba(34, 197, 94, 0.3)"
                          }}>
                            <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                              <polyline points="20 6 9 17 4 12" />
                            </svg>
                          </div>
                          <strong style={{ color: "#22c55e", fontSize: "1rem", fontWeight: 600 }}>Connected Successfully</strong>
                          <p style={{ fontSize: "12px", color: "var(--text-muted)", maxWidth: "240px", margin: 0, lineHeight: "1.45" }}>
                            Phone is connected and syncing.
                          </p>
                        </div>
                      ) : (
                        <>
                          <span
                            className="about-support-label"
                            style={{ letterSpacing: "1.5px", fontSize: "10px", fontWeight: "bold", color: "var(--accent)", textTransform: "uppercase", marginBottom: "10px" }}
                          >
                            SCAN QR TO PAIR
                          </span>
                          {syncServerInfo.qrSvg ? (
                            <>
                              <style>{`
                                .qr-code-svg-container svg {
                                  width: 100% !important;
                                  height: 100% !important;
                                  display: block !important;
                                }
                              `}</style>
                              <div
                                className="qr-code-svg-container"
                                dangerouslySetInnerHTML={{ __html: syncServerInfo.qrSvg }}
                              />
                            </>
                          ) : (
                            <div style={{ width: "130px", height: "130px", background: "var(--surface-muted)", borderRadius: "10px", display: "flex", alignItems: "center", justifyContent: "center" }}>
                              <span style={{ color: "var(--text-muted)", fontSize: "11px" }}>Generating QR...</span>
                            </div>
                          )}
                          <p style={{ fontSize: "11px", color: "var(--text-muted)", margin: "10px 0 14px", maxWidth: "220px", lineHeight: "1.45" }}>
                            Open the scanner on the Android settings tab to pair automatically.
                          </p>
                          <div style={{ width: "100%", textAlign: "left" }}>
                            <span style={{ display: "block", fontSize: "9px", fontWeight: "bold", color: "var(--text-muted)", letterSpacing: "0.8px", marginBottom: "5px", textTransform: "uppercase" }}>
                              Pairing Address URI
                            </span>
                            <div style={{ display: "flex", gap: "6px", width: "100%" }}>
                              <input
                                type="text"
                                readOnly
                                value={syncServerInfo.pairingUri}
                                style={{ flex: 1, padding: "6px 10px", background: "var(--surface-input)", border: "1px solid var(--border-soft)", borderRadius: "6px", color: "var(--text-main)", fontSize: "11px", fontFamily: "monospace", textOverflow: "ellipsis", minWidth: 0 }}
                              />
                              <button
                                type="button"
                                className={`ghost-button ${pairingUriCopied ? "success" : ""}`}
                                style={{
                                  padding: "6px 12px",
                                  borderColor: pairingUriCopied ? "#22c55e" : undefined,
                                  color: pairingUriCopied ? "#22c55e" : undefined,
                                  background: pairingUriCopied ? "rgba(34, 197, 94, 0.08)" : undefined,
                                  fontSize: "11px",
                                  fontWeight: 500,
                                  cursor: "pointer",
                                  transition: "all 0.2s ease",
                                  flexShrink: 0
                                }}
                                onClick={() => {
                                  void navigator.clipboard.writeText(syncServerInfo.pairingUri);
                                  setPairingUriCopied(true);
                                  setTimeout(() => setPairingUriCopied(false), 2000);
                                }}
                              >
                                {pairingUriCopied ? "✓ Copied" : "Copy"}
                              </button>
                            </div>
                          </div>
                        </>
                      )}

                      <button
                        type="button"
                        className="about-support-button"
                        style={{
                          padding: "8px 16px",
                          marginTop: "14px",
                          width: "fit-content",
                          minWidth: "148px",
                          alignSelf: "center",
                          display: "inline-flex",
                          alignItems: "center",
                          justifyContent: "center",
                          textAlign: "center",
                          background: "rgba(255, 117, 117, 0.1)",
                          color: "var(--danger)",
                          border: "1px solid rgba(255, 117, 117, 0.2)",
                          borderRadius: "7px",
                          cursor: "pointer",
                          fontWeight: "bold",
                          fontSize: "12px",
                          transition: "all 0.2s ease"
                        }}
                        disabled={syncBusy}
                        onClick={stopSyncServer}
                        onMouseEnter={(e) => {
                          e.currentTarget.style.background = "rgba(255, 117, 117, 0.18)";
                          e.currentTarget.style.borderColor = "rgba(255, 117, 117, 0.35)";
                        }}
                        onMouseLeave={(e) => {
                          e.currentTarget.style.background = "rgba(255, 117, 117, 0.1)";
                          e.currentTarget.style.borderColor = "rgba(255, 117, 117, 0.2)";
                        }}
                      >
                        {syncBusy ? "Stopping..." : "Stop Sync Server"}
                      </button>
                    </section>
                  </div>

                  {syncServerInfo?.fileTransfers?.length ? (
                    <section className="sync-transfer-panel" aria-label="Phone transfer progress">
                      <div className="sync-transfer-header">
                        <span className="sync-label">Phone Transfers</span>
                        <span>{syncServerInfo.fileTransfers.length} recent</span>
                      </div>
                      <div className="sync-transfer-list">
                        {syncServerInfo.fileTransfers.map((transfer) => {
                          const percent = transfer.size > 0
                            ? Math.min(100, Math.round((transfer.bytesSent / transfer.size) * 100))
                            : transfer.status === "completed" ? 100 : 0;
                          return (
                            <div className="sync-transfer-row" key={transfer.id}>
                              <div className="sync-transfer-copy">
                                <div style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                                  <strong>{transfer.name}</strong>
                                  {transfer.status === "completed" && transfer.path && (
                                    <button
                                      className="sync-transfer-folder-btn"
                                      title="Open file location"
                                      onClick={() => revealResult(transfer.path)}
                                      style={{
                                        background: "none",
                                        border: "none",
                                        cursor: "pointer",
                                        padding: "2px",
                                        display: "inline-flex",
                                        alignItems: "center",
                                        opacity: 0.7,
                                        transition: "opacity 0.15s",
                                      }}
                                      onMouseEnter={(e) => (e.currentTarget.style.opacity = "1")}
                                      onMouseLeave={(e) => (e.currentTarget.style.opacity = "0.7")}
                                    >
                                      <svg width="16" height="16" viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg">
                                        <path d="M2 5.5C2 4.11929 3.11929 3 4.5 3H7.58579C8.11622 3 8.62493 3.21071 9 3.58579L10.4142 5H15.5C16.8807 5 18 6.11929 18 7.5V14.5C18 15.8807 16.8807 17 15.5 17H4.5C3.11929 17 2 15.8807 2 14.5V5.5Z" fill="var(--accent)" opacity="0.8" />
                                      </svg>
                                    </button>
                                  )}
                                </div>
                                <span>
                                  {transfer.status === "failed"
                                    ? transfer.error || "Transfer failed"
                                    : `${formatBytes(transfer.bytesSent)} / ${formatBytes(transfer.size)} - ${transfer.status}`}
                                </span>
                              </div>
                              <div className="sync-transfer-meter" aria-label={`${transfer.name} ${percent}%`}>
                                <span style={{ width: `${percent}%` }} />
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    </section>
                  ) : null}


                </div>
              )}
            </div>
          </section>
        ) : null}

        {activeTab === "search" && searchResultContextMenu && activeSearchResultMenu ? (
          <div
            ref={searchResultContextMenuRef}
            className="result-context-menu"
            role="menu"
            style={searchResultContextMenuStyle}
            onClick={(event) => {
              event.stopPropagation();
            }}
          >
            <button
              type="button"
              className="result-context-menu-item"
              role="menuitem"
              onClick={() => {
                closeSearchResultContextMenu();
                void launchSearchResult(activeSearchResultMenu);
              }}
            >
              {activeSearchResultMenu.isDirectory
                ? "Open folder"
                : activeSearchResultIsApp
                  ? "Open app"
                  : "Open file"}
            </button>
            {!activeSearchResultMenu.isDirectory && activeSearchResultCanReveal ? (
              <button
                type="button"
                className="result-context-menu-item"
                role="menuitem"
                onClick={() => {
                  closeSearchResultContextMenu();
                  void openResultPath(activeSearchResultMenu);
                }}
              >
                {activeSearchResultIsApp ? "Open app location" : "Open containing folder"}
              </button>
            ) : null}
            {!activeSearchResultIsApp ? (
              <>
                <button
                  type="button"
                  className="result-context-menu-item"
                  role="menuitem"
                  onClick={() => {
                    closeSearchResultContextMenu();
                    setIsSelectionModeActive(true);
                    setSelectedPathsForBulk((prev) => {
                      const next = new Set(prev);
                      next.add(activeSearchResultMenu.path);
                      return next;
                    });
                  }}
                >
                  <span className="result-context-menu-item-label">Select file</span>
                </button>
                <button
                  type="button"
                  className="result-context-menu-item"
                  role="menuitem"
                  onClick={() => {
                    closeSearchResultContextMenu();
                    void openResultInConsole(activeSearchResultMenu);
                  }}
                >
                  <span className="result-context-menu-item-icon" aria-hidden="true">
                    <ConsoleIcon />
                  </span>
                  <span className="result-context-menu-item-label">Open path in console</span>
                </button>
                {activeSearchResultCanSendToPhone ? (
                  <button
                    type="button"
                    className="result-context-menu-item"
                    role="menuitem"
                    disabled={!(syncServerInfo?.running && syncServerInfo.connectedClients > 0)}
                    onClick={() => {
                      void sendSearchResultToPhone(activeSearchResultMenu);
                    }}
                  >
                    <span className="result-context-menu-item-icon" aria-hidden="true">
                      <PhoneIcon />
                    </span>
                    <span className="result-context-menu-item-label">Send to phone</span>
                  </button>
                ) : null}
                <button
                  type="button"
                  className="result-context-menu-item"
                  role="menuitem"
                  onClick={() => {
                    openSearchResultRename(activeSearchResultMenu, searchResultContextMenu.rowKey);
                  }}
                >
                  Rename
                </button>

                <div className="result-context-menu-divider" />
              </>
            ) : null}

            <button
              type="button"
              className="result-context-menu-item"
              role="menuitem"
              onClick={() => {
                closeSearchResultContextMenu();
                void handleSearchResultCopy(
                  activeSearchResultLocationText,
                  activeSearchResultLocationLabel.toLowerCase(),
                );
              }}
            >
              {`Copy ${activeSearchResultLocationLabel.toLowerCase()}`}
            </button>
            {activeSearchResultMenu.isDirectory ? (
              <button
                type="button"
                className="result-context-menu-item"
                role="menuitem"
                onClick={() => {
                  closeSearchResultContextMenu();
                  void handleSearchResultCopy(resultDisplayName(activeSearchResultMenu), "folder name");
                }}
              >
                Copy folder name
              </button>
            ) : !activeSearchResultIsApp ? (
              <>
                <button
                  type="button"
                  className="result-context-menu-item"
                  role="menuitem"
                  onClick={() => {
                    closeSearchResultContextMenu();
                    void handleSearchResultCopy(
                      resultFilenameWithoutExtension(activeSearchResultMenu),
                      "filename",
                    );
                  }}
                >
                  Copy filename
                </button>
                <button
                  type="button"
                  className="result-context-menu-item"
                  role="menuitem"
                  onClick={() => {
                    closeSearchResultContextMenu();
                    void handleSearchResultCopy(resultDisplayName(activeSearchResultMenu), "full filename");
                  }}
                >
                  Copy filename + extension
                </button>
              </>
            ) : null}

            {!activeSearchResultIsApp ? (
              <>
                <div className="result-context-menu-divider" />

                <button
                  type="button"
                  className="result-context-menu-item danger"
                  role="menuitem"
                  onClick={() => {
                    openSearchResultDelete(activeSearchResultMenu, searchResultContextMenu.rowKey);
                  }}
                >
                  Delete
                </button>
              </>
            ) : null}
          </div>
        ) : null}

        {quickLookOpen && selectedResult ? (
          <div
            className="quick-look-overlay"
            role="dialog"
            aria-modal="true"
            aria-label={`Quick Look: ${selectedResult.name}`}
            onClick={(event) => {
              if (event.target === event.currentTarget) {
                closeQuickLook();
              }
            }}
          >
            <div
              ref={quickLookDialogRef}
              className="quick-look-dialog"
              tabIndex={-1}
              onClick={(event) => {
                event.stopPropagation();
              }}
            >
              <div className="quick-look-header">
                <div className="quick-look-copy">
                  <div className="quick-look-title-row">
                    <strong>{selectedResult.name}</strong>
                    <span className="quick-look-position-pill">
                      {`${selectedResultPosition} of ${visibleResults.length.toLocaleString()}`}
                    </span>
                  </div>
                  <p className="quick-look-hint">Esc to close, Arrow keys to switch results, Enter to open.</p>
                </div>
                <div className="quick-look-actions">
                  <button
                    type="button"
                    className="row-action"
                    onClick={() => {
                      closeQuickLook({ restoreFocus: false });
                      void launchSearchResult(selectedResult);
                    }}
                  >
                    {selectedResultIsDirectory
                      ? "Open folder"
                      : isAppSearchResult(selectedResult)
                        ? "Open app"
                        : "Open file"}
                  </button>
                  {canRevealResultLocation(selectedResult) ? (
                    <button
                      type="button"
                      className="row-action"
                      onClick={() => {
                        closeQuickLook({ restoreFocus: false });
                        void revealSearchResult(selectedResult);
                      }}
                    >
                      Reveal folder
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="ghost-button quick-look-close-button"
                    onClick={() => {
                      closeQuickLook();
                    }}
                  >
                    Close
                  </button>
                </div>
              </div>

              <div className="quick-look-layout">
                <div
                  className={`quick-look-stage ${selectedResultIsDirectory ? "folder" : ""} kind-${selectedResultIconKind}`}
                >
                  {!hasQuickLookPreview && !hasQuickLookTextPreview && selectedResultAppIconSrc.length === 0 ? (
                    <span
                      className={`result-fallback-icon-shell kind-${selectedResultIconKind} quick-look-fallback-icon-shell`}
                      aria-hidden="true"
                    >
                      <ResultTypeIcon kind={selectedResultIconKind} className="result-fallback-icon" />
                    </span>
                  ) : null}
                  {selectedResultAppIconSrc.length > 0 ? (
                    <img
                      className="preview-media ready app-result-icon-media"
                      src={selectedResultAppIconSrc}
                      alt=""
                      loading="lazy"
                    />
                  ) : hasQuickLookPreview && quickLookPreviewKind === "image" ? (
                    <img
                      key={`${selectedResultRowKey}:${quickLookPreviewIndex}:image:quick-look`}
                      className={`preview-media ${quickLookPreviewReady ? "ready" : ""}`}
                      src={quickLookPreviewSrc}
                      alt=""
                      loading="lazy"
                      onLoad={() => {
                        handleSelectedPreviewReady(quickLookPreviewRenderKey);
                      }}
                      onError={() => {
                        handleSelectedPreviewError(quickLookPreviewSources.length);
                      }}
                    />
                  ) : null}
                  {hasQuickLookPreview && quickLookPreviewKind === "video" ? (
                    <video
                      key={`${selectedResultRowKey}:${quickLookPreviewIndex}:video:quick-look`}
                      className={`preview-media ${quickLookPreviewReady ? "ready" : ""}`}
                      src={quickLookPreviewSrc}
                      controls
                      muted
                      playsInline
                      preload="metadata"
                      onLoadedData={() => {
                        handleSelectedPreviewReady(quickLookPreviewRenderKey);
                      }}
                      onError={() => {
                        handleSelectedPreviewError(quickLookPreviewSources.length);
                      }}
                    />
                  ) : null}
                  {hasQuickLookPreview && quickLookPreviewKind === "pdf" ? (
                    <iframe
                      key={`${selectedResultRowKey}:${quickLookPreviewIndex}:pdf:quick-look`}
                      className={`preview-media ${quickLookPreviewReady ? "ready" : ""}`}
                      src={`${quickLookPreviewSrc}#toolbar=0&navpanes=0&scrollbar=0&page=1&view=FitH`}
                      title={selectedResult.name}
                      loading="lazy"
                      onLoad={() => {
                        handleSelectedPreviewReady(quickLookPreviewRenderKey);
                      }}
                      onError={() => {
                        handleSelectedPreviewError(quickLookPreviewSources.length);
                      }}
                    />
                  ) : null}
                  {hasQuickLookTextPreview && quickLookPreviewKind === "text" ? (
                    <pre className="text-preview-content">
                      {highlightMatch(quickLookTextPreview?.text ?? "", contentSearchQuery, "content")}
                    </pre>
                  ) : null}
                  {!hasQuickLookPreview && !hasQuickLookTextPreview && selectedResultAppIconSrc.length === 0 ? (
                    <div className="quick-look-placeholder">
                      <strong>
                        {quickLookPreviewKind === "text" && quickLookTextPreviewLoading
                          ? "Loading text preview"
                          : quickLookPreviewKind === "none"
                            ? "Large preview isn't available for this result yet"
                            : "Preview couldn't be loaded"}
                      </strong>
                      <span>
                        {quickLookPreviewKind === "text" && quickLookTextPreviewLoading
                          ? "Reading the file and preparing a larger text preview."
                          : quickLookPreviewKind === "none"
                            ? "Quick Look still gives you a bigger read on the file details so you can decide whether to open it."
                            : "Try opening the file directly, or move through the results with the Arrow keys to inspect another match."}
                      </span>
                    </div>
                  ) : null}
                </div>

                <aside className="quick-look-sidebar">
                  <div className="quick-look-meta-grid">
                    <div className="quick-look-meta-card">
                      <span>Type</span>
                      <strong>{isAppSearchResult(selectedResult) ? "app" : selectedResultExtensionLabel}</strong>
                    </div>
                    {isAppSearchResult(selectedResult) ? (
                      <div className="quick-look-meta-card">
                        <span>Kind</span>
                        <strong>{selectedResult.revealPath ? "Desktop app" : "Installed app"}</strong>
                      </div>
                    ) : (
                      <>
                        <div className="quick-look-meta-card">
                          <span>Size</span>
                          <strong>{formatBytes(selectedResult.size)}</strong>
                        </div>
                        <div className="quick-look-meta-card">
                          <span>Created</span>
                          <strong>{formatUnix(selectedResult.createdUnix)}</strong>
                        </div>
                        <div className="quick-look-meta-card">
                          <span>Modified</span>
                          <strong>{formatUnix(selectedResult.modifiedUnix)}</strong>
                        </div>
                      </>
                    )}
                    <div className="quick-look-meta-card quick-look-meta-card-wide">
                      <div className="quick-look-meta-card-header">
                        <span>{selectedResultLocationLabel}</span>
                        <button
                          type="button"
                          className={`quick-look-copy-button ${quickLookCopyState === "copied"
                            ? "is-copied"
                            : quickLookCopyState === "error"
                              ? "is-error"
                              : ""
                            }`}
                          aria-label={
                            quickLookCopyState === "copied"
                              ? "Path copied"
                              : quickLookCopyState === "error"
                                ? "Copy failed"
                                : "Copy path"
                          }
                          title={
                            quickLookCopyState === "copied"
                              ? "Path copied"
                              : quickLookCopyState === "error"
                                ? "Copy failed"
                                : "Copy path"
                          }
                          onClick={() => {
                            void handleQuickLookPathCopy(selectedResultLocationText);
                          }}
                        >
                          {quickLookCopyState === "copied" ? <CheckIcon /> : <CopyIcon />}
                        </button>
                      </div>
                      <strong>{selectedResultLocationText}</strong>
                    </div>
                  </div>
                </aside>
              </div>
            </div>
          </div>
        ) : null}

        {availableUpdate ? (
          <div className="modal-overlay update-modal-overlay" role="dialog" aria-modal="true">
            <div className="modal-card update-modal-card">
              <div className="update-modal-header">
                <div className="update-modal-title">
                  <h3>Update Available</h3>
                </div>
                <button
                  type="button"
                  className="update-github-link"
                  aria-label="Open OmniSearch GitHub"
                  title="Open GitHub"
                  onClick={() => {
                    void openExternalLink(GITHUB_PROFILE_URL);
                  }}
                >
                  <SocialIcon icon="github" />
                </button>
              </div>

              <div className="update-version-row">
                <span className="update-current-version">
                  v{(availableUpdate.currentVersion || appVersion || "current").replace(/^v/i, "")}
                </span>
                <strong aria-hidden="true">-&gt;</strong>
                <span className="update-next-version">v{availableUpdate.version.replace(/^v/i, "")}</span>
              </div>

              {availableUpdate.body ? (
                <div className="update-notes">
                  <span>Release Notes</span>
                  <div className="update-notes-content">
                    {renderUpdateReleaseNotes(availableUpdate.body)}
                  </div>
                </div>
              ) : null}

              {updateProgress.stage !== "idle" || updateProgress.error ? (
                <div className={`update-progress ${updateProgress.stage === "error" ? "is-error" : ""}`}>
                  <div className="update-progress-top">
                    <span>{updateProgress.message || "Working on update..."}</span>
                    {updateProgress.stage === "downloading" && updateProgress.total ? (
                      <strong>{updateProgress.percent}%</strong>
                    ) : null}
                  </div>
                  <div className="update-progress-track" aria-hidden="true">
                    <div
                      className="update-progress-fill"
                      style={{
                        width: `${updateProgress.percent}%`,
                      }}
                    />
                  </div>
                  {updateProgress.error ? <p>{updateProgress.error}</p> : null}
                </div>
              ) : null}

              <div className="update-modal-actions">
                <button
                  type="button"
                  className="update-skip-button"
                  disabled={updateProgress.stage !== "idle" && updateProgress.stage !== "error"}
                  onClick={skipAvailableUpdate}
                >
                  Skip this version
                </button>
                <div className="update-modal-action-group">
                  <button
                    type="button"
                    className="ghost-button"
                    disabled={updateProgress.stage !== "idle" && updateProgress.stage !== "error"}
                    onClick={dismissUpdateForNow}
                  >
                    Later
                  </button>
                  <button
                    type="button"
                    className="ghost-button update-now-button"
                    disabled={updateProgress.stage !== "idle" && updateProgress.stage !== "error"}
                    onClick={() => {
                      void installAvailableUpdate();
                    }}
                  >
                    {updateProgress.stage === "error" ? "Try Again" : "Update Now"}
                  </button>
                </div>
              </div>
            </div>
          </div>
        ) : null}

        {searchResultRenameDraft ? (
          <div
            className="modal-overlay"
            role="dialog"
            aria-modal="true"
            onClick={() => {
              if (!searchResultRenameBusy) {
                setSearchResultRenameDraft(null);
              }
            }}
          >
            <div
              className="modal-card rename-modal-card"
              onClick={(event) => {
                event.stopPropagation();
              }}
            >
              <h3>{searchResultRenameDraft.isDirectory ? "Rename folder" : "Rename file"}</h3>
              <p>{searchResultRenameDraft.currentName}</p>
              <p className="modal-path">{searchResultRenameDraft.path}</p>

              <label className="modal-input-group">
                <span>New name</span>
                <input
                  ref={searchResultRenameInputRef}
                  type="text"
                  value={searchResultRenameDraft.nextName}
                  autoComplete="off"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  disabled={searchResultRenameBusy}
                  onChange={(event) => {
                    const { value } = event.currentTarget;
                    setSearchResultRenameDraft((previous) =>
                      previous
                        ? {
                          ...previous,
                          nextName: value,
                        }
                        : previous,
                    );
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void confirmSearchResultRename();
                    } else if (event.key === "Escape" && !searchResultRenameBusy) {
                      event.preventDefault();
                      setSearchResultRenameDraft(null);
                    }
                  }}
                />
              </label>

              <div className="modal-actions">
                <button
                  type="button"
                  className="ghost-button"
                  disabled={searchResultRenameBusy}
                  onClick={() => {
                    setSearchResultRenameDraft(null);
                  }}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  disabled={searchResultRenameBusy}
                  onClick={() => {
                    void confirmSearchResultRename();
                  }}
                >
                  {searchResultRenameBusy ? "Renaming..." : "Rename"}
                </button>
              </div>
            </div>
          </div>
        ) : null}

        {searchResultDeleteCandidate ? (
          <div
            className="modal-overlay"
            role="dialog"
            aria-modal="true"
            onClick={() => {
              if (!searchResultDeleteBusy) {
                setSearchResultDeleteCandidate(null);
              }
            }}
          >
            <div
              className="modal-card"
              onClick={(event) => {
                event.stopPropagation();
              }}
            >
              <h3>{searchResultDeleteCandidate.isDirectory ? "Delete folder?" : "Delete file?"}</h3>
              <p>{searchResultDeleteCandidate.name}</p>
              <p className="modal-path">{searchResultDeleteCandidate.path}</p>
              <label className="modal-checkbox-option">
                <input
                  type="checkbox"
                  checked={searchResultDeleteToRecycleBin}
                  disabled={searchResultDeleteBusy}
                  onChange={(event) => {
                    setSearchResultDeleteToRecycleBin(event.currentTarget.checked);
                  }}
                />
                <span>Move to Recycle Bin</span>
              </label>
              <div className="modal-actions">
                <button
                  type="button"
                  className="ghost-button"
                  disabled={searchResultDeleteBusy}
                  onClick={() => {
                    setSearchResultDeleteCandidate(null);
                  }}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="row-action danger-row-action"
                  disabled={searchResultDeleteBusy}
                  onClick={() => {
                    void confirmSearchResultDelete();
                  }}
                >
                  {searchResultDeleteBusy
                    ? searchResultDeleteToRecycleBin
                      ? "Moving..."
                      : "Deleting..."
                    : searchResultDeleteToRecycleBin
                      ? "Recycle"
                      : "Delete"}
                </button>
              </div>
            </div>
          </div>
        ) : null}

        {bulkDeleteOpen ? (
          <div
            className="modal-overlay"
            role="dialog"
            aria-modal="true"
            onClick={() => {
              if (!bulkDeleteBusy) {
                setBulkDeleteOpen(false);
              }
            }}
          >
            <div
              className="modal-card"
              onClick={(event) => {
                event.stopPropagation();
              }}
            >
              <h3>Delete selected items?</h3>
              <p style={{ margin: "8px 0 16px 0", fontSize: "0.9rem", color: "var(--text-muted)" }}>
                {selectedPathsForBulk.size} {selectedPathsForBulk.size === 1 ? "item" : "items"} will be deleted.
              </p>
              <label className="modal-checkbox-option">
                <input
                  type="checkbox"
                  checked={bulkDeleteRecycleBin}
                  disabled={bulkDeleteBusy}
                  onChange={(event) => {
                    setBulkDeleteRecycleBin(event.currentTarget.checked);
                  }}
                />
                <span>Move to Recycle Bin</span>
              </label>
              <div className="modal-actions">
                <button
                  type="button"
                  className="ghost-button"
                  disabled={bulkDeleteBusy}
                  onClick={() => {
                    setBulkDeleteOpen(false);
                  }}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="row-action danger-row-action"
                  disabled={bulkDeleteBusy}
                  onClick={() => {
                    void confirmBulkDelete();
                  }}
                >
                  {bulkDeleteBusy
                    ? bulkDeleteRecycleBin
                      ? "Moving..."
                      : "Deleting..."
                    : bulkDeleteRecycleBin
                      ? "Recycle"
                      : "Delete"}
                </button>
              </div>
            </div>
          </div>
        ) : null}

        {bulkRenameOpen ? (
          <div
            className="modal-overlay"
            role="dialog"
            aria-modal="true"
            onClick={() => {
              if (!bulkRenameBusy) {
                setBulkRenameOpen(false);
              }
            }}
          >
            <div
              className="modal-card rename-modal-card"
              onClick={(event) => {
                event.stopPropagation();
              }}
              style={{ width: "min(500px, 95vw)" }}
            >
              <h3>Bulk Rename</h3>
              <p style={{ fontSize: "0.85rem", color: "var(--text-muted)", marginBottom: "16px" }}>
                Renaming {selectedPathsForBulk.size} {selectedPathsForBulk.size === 1 ? "item" : "items"}.
              </p>

              <div className="bulk-rename-tabs" style={{ display: "flex", gap: "8px", marginBottom: "16px" }}>
                <button
                  type="button"
                  className={`tab-button ${bulkRenameMode === "replace" ? "active" : ""}`}
                  style={{
                    flex: 1,
                    padding: "8px 12px",
                    borderRadius: "6px",
                    border: "1px solid var(--border-soft)",
                    background: bulkRenameMode === "replace" ? "var(--accent)" : "none",
                    color: bulkRenameMode === "replace" ? "var(--text-main)" : "var(--text-muted)",
                    cursor: "pointer"
                  }}
                  onClick={() => setBulkRenameMode("replace")}
                >
                  Find & Replace
                </button>
                <button
                  type="button"
                  className={`tab-button ${bulkRenameMode === "prefix_suffix" ? "active" : ""}`}
                  style={{
                    flex: 1,
                    padding: "8px 12px",
                    borderRadius: "6px",
                    border: "1px solid var(--border-soft)",
                    background: bulkRenameMode === "prefix_suffix" ? "var(--accent)" : "none",
                    color: bulkRenameMode === "prefix_suffix" ? "var(--text-main)" : "var(--text-muted)",
                    cursor: "pointer"
                  }}
                  onClick={() => setBulkRenameMode("prefix_suffix")}
                >
                  Add Prefix/Suffix
                </button>
                <button
                  type="button"
                  className={`tab-button ${bulkRenameMode === "enumerate" ? "active" : ""}`}
                  style={{
                    flex: 1,
                    padding: "8px 12px",
                    borderRadius: "6px",
                    border: "1px solid var(--border-soft)",
                    background: bulkRenameMode === "enumerate" ? "var(--accent)" : "none",
                    color: bulkRenameMode === "enumerate" ? "var(--text-main)" : "var(--text-muted)",
                    cursor: "pointer"
                  }}
                  onClick={() => setBulkRenameMode("enumerate")}
                >
                  Enumerate
                </button>
              </div>

              {bulkRenameMode === "replace" && (
                <div style={{ display: "flex", flexDirection: "column", gap: "12px", marginBottom: "20px" }}>
                  <label className="modal-input-group">
                    <span>Find text</span>
                    <input
                      type="text"
                      value={bulkRenameReplaceSearch}
                      disabled={bulkRenameBusy}
                      onChange={(e) => setBulkRenameReplaceSearch(e.target.value)}
                      placeholder="e.g. old-name"
                    />
                  </label>
                  <label className="modal-input-group">
                    <span>Replace with</span>
                    <input
                      type="text"
                      value={bulkRenameReplaceWith}
                      disabled={bulkRenameBusy}
                      onChange={(e) => setBulkRenameReplaceWith(e.target.value)}
                      placeholder="e.g. new-name"
                    />
                  </label>
                </div>
              )}

              {bulkRenameMode === "prefix_suffix" && (
                <div style={{ display: "flex", flexDirection: "column", gap: "12px", marginBottom: "20px" }}>
                  <label className="modal-input-group">
                    <span>Prefix</span>
                    <input
                      type="text"
                      value={bulkRenamePrefix}
                      disabled={bulkRenameBusy}
                      onChange={(e) => setBulkRenamePrefix(e.target.value)}
                      placeholder="Prefix to prepend"
                    />
                  </label>
                  <label className="modal-input-group">
                    <span>Suffix</span>
                    <input
                      type="text"
                      value={bulkRenameSuffix}
                      disabled={bulkRenameBusy}
                      onChange={(e) => setBulkRenameSuffix(e.target.value)}
                      placeholder="Suffix to append"
                    />
                  </label>
                </div>
              )}

              {bulkRenameMode === "enumerate" && (
                <div style={{ display: "flex", flexDirection: "column", gap: "12px", marginBottom: "20px" }}>
                  <label className="modal-input-group">
                    <span>Base Name</span>
                    <input
                      type="text"
                      value={bulkRenameEnumerateBase}
                      disabled={bulkRenameBusy}
                      onChange={(e) => setBulkRenameEnumerateBase(e.target.value)}
                      placeholder="Base filename"
                    />
                  </label>
                  <div style={{ display: "flex", gap: "12px" }}>
                    <label className="modal-input-group" style={{ flex: 1 }}>
                      <span>Start Number</span>
                      <input
                        type="number"
                        min="0"
                        value={bulkRenameEnumerateStart}
                        disabled={bulkRenameBusy}
                        onChange={(e) => setBulkRenameEnumerateStart(parseInt(e.target.value) || 1)}
                      />
                    </label>
                    <label className="modal-input-group" style={{ flex: 1 }}>
                      <span>Digits</span>
                      <input
                        type="number"
                        min="1"
                        max="10"
                        value={bulkRenameEnumerateDigits}
                        disabled={bulkRenameBusy}
                        onChange={(e) => setBulkRenameEnumerateDigits(parseInt(e.target.value) || 3)}
                      />
                    </label>
                  </div>
                </div>
              )}

              <div className="modal-actions">
                <button
                  type="button"
                  className="ghost-button"
                  disabled={bulkRenameBusy}
                  onClick={() => {
                    setBulkRenameOpen(false);
                  }}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  disabled={bulkRenameBusy}
                  onClick={() => {
                    void confirmBulkRename();
                  }}
                >
                  {bulkRenameBusy ? "Renaming..." : "Rename"}
                </button>
              </div>
            </div>
          </div>
        ) : null}

        {bulkActionProgress ? (
          <div
            className="modal-overlay"
            role="dialog"
            aria-modal="true"
            style={{ zIndex: 10000 }}
          >
            <div className="modal-card" style={{ textAlign: "center", padding: "30px 40px" }}>
              <h3>{bulkActionProgress.label}</h3>
              <div style={{ margin: "20px 0", background: "var(--border-soft)", borderRadius: "10px", height: "8px", overflow: "hidden" }}>
                <div
                  style={{
                    background: "var(--accent)",
                    height: "100%",
                    width: `${(bulkActionProgress.current / bulkActionProgress.total) * 100}%`,
                    transition: "width 0.2s ease"
                  }}
                />
              </div>
              <p style={{ color: "var(--text-muted)", fontSize: "0.9rem" }}>
                {bulkActionProgress.current} / {bulkActionProgress.total} completed
              </p>
            </div>
          </div>
        ) : null}

        {syncServerInfo?.running && syncServerInfo.pendingApprovals.length > 0 ? (
          <div
            className="modal-overlay"
            role="dialog"
            aria-modal="true"
            style={{
              backdropFilter: "blur(12px)",
              WebkitBackdropFilter: "blur(12px)",
              background: "rgba(5, 7, 15, 0.75)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center"
            }}
          >
            <div
              className="modal-card"
              style={{
                width: "min(440px, 90vw)",
                border: "1px solid var(--border-soft)",
                background: "color-mix(in srgb, var(--surface-strong) 98%, #000)",
                borderRadius: "14px",
                padding: "20px 24px",
                boxShadow: "0 20px 50px rgba(0,0,0,0.6)"
              }}
            >
              <h3 style={{ margin: "0 0 4px 0", fontSize: "1.1rem", fontWeight: 700, color: "var(--text-main)" }}>
                Phone Pairing Request
              </h3>
              <p style={{ margin: "0 0 16px 0", fontSize: "0.84rem", color: "var(--text-muted)" }}>
                A mobile companion device is attempting to connect to your PC.
              </p>

              <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
                {syncServerInfo.pendingApprovals.map((req) => (
                  <div
                    key={req.deviceId}
                    style={{
                      background: "rgba(255, 255, 255, 0.03)",
                      border: "1px solid var(--border-soft)",
                      borderRadius: "10px",
                      padding: "14px",
                      display: "flex",
                      flexDirection: "column",
                      gap: "12px"
                    }}
                  >
                    <div style={{ display: "flex", flexDirection: "column", gap: "4px" }}>
                      <strong style={{ color: "var(--text-main)", fontSize: "0.95rem" }}>
                        {req.deviceName}
                      </strong>
                      <span style={{ fontSize: "0.78rem", color: "var(--text-muted)" }}>
                        IP Address: {req.peerAddress}
                      </span>
                      <span style={{ fontSize: "0.74rem", color: "var(--text-muted)", fontFamily: "monospace" }}>
                        ID: {req.deviceId}
                      </span>
                    </div>

                    <div style={{ display: "flex", gap: "10px", marginTop: "4px" }}>
                      <button
                        type="button"
                        className="row-action"
                        style={{
                          flex: 1,
                          background: "var(--accent)",
                          color: "var(--bg-deep)",
                          border: "none",
                          borderRadius: "8px",
                          fontWeight: "bold",
                          padding: "10px",
                          cursor: "pointer",
                          fontSize: "0.84rem"
                        }}
                        onClick={() => approvePairing(req.deviceId)}
                      >
                        Approve Connection
                      </button>
                      <button
                        type="button"
                        className="row-action danger-row-action"
                        style={{
                          flex: 0.4,
                          borderRadius: "8px",
                          padding: "10px",
                          cursor: "pointer",
                          fontSize: "0.84rem"
                        }}
                        onClick={() => rejectPairing(req.deviceId)}
                      >
                        Reject
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        ) : null}
      </main>
    </div>
  );
}

export default App;
