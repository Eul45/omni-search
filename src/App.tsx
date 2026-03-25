import { useEffect, useMemo, useRef, useState } from "react";
import type {
  CSSProperties,
  DragEvent as ReactDragEvent,
  MouseEvent as ReactMouseEvent,
  ReactNode,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import "./App.css";
import {
  DEFAULT_DESKTOP_SETTINGS,
  getDesktopSettings,
  listenForWindowMode,
  openFullWindow,
  openQuickWindow,
  syncNativeWindowTheme,
  updateDesktopSettings,
} from "./desktop";
import type { DesktopSettings, WindowMode } from "./desktop";

const POLL_INTERVAL_MS = 700;
const SEARCH_DEBOUNCE_MS = 130;
const FILTER_SEARCH_DEBOUNCE_MS = 320;
const SEARCH_LIMIT = 200;
const SEARCH_LIMIT_MIN = SEARCH_LIMIT;
const SEARCH_LIMIT_MAX = 5000;
const PREVIEW_DATA_URL_LIMIT = 1;
const DUPLICATE_CANCEL_MESSAGE = "Duplicate scan cancelled.";
const DUPLICATE_NOTICE_TIMEOUT_MS = 2400;
const ACTION_NOTICE_TIMEOUT_MS = 1800;

type IndexStatus = {
  indexing: boolean;
  ready: boolean;
  indexedCount: number;
  lastError?: string | null;
};

type SearchResult = {
  name: string;
  path: string;
  extension: string;
  size: number;
  createdUnix: number;
  modifiedUnix: number;
  isDirectory: boolean;
};

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
};

type SearchResultDeleteCandidate = {
  rowKey: string;
  path: string;
  name: string;
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
  icon: "clipboard" | "workspace" | "capture";
  accent: "sky" | "mint" | "amber";
};

type ActiveTab = "search" | "duplicates" | "advanced" | "themes" | "about";
type ResultViewTab = "all" | "apps" | "media" | "docs" | "archives";
type ResultSortMode = "relevance" | "newest" | "largest" | "name";
type ThemeMode = "dark" | "light";
type PreviewKind = "image" | "video" | "pdf" | "none";
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
const DONATE_URL = "http://buymeacoffee.com/eyuelengida";
const THEME_STORAGE_KEY = "omnisearch_theme_mode";
const THEME_PRESET_STORAGE_KEY = "omnisearch_theme_preset";
const PREVIEW_STORAGE_KEY = "omnisearch_show_previews";
const INCLUDE_FOLDERS_STORAGE_KEY = "omnisearch_include_folders";
const INCLUDE_ALL_DRIVES_STORAGE_KEY = "omnisearch_include_all_drives";
const SEARCH_LIMIT_STORAGE_KEY = "omnisearch_search_limit";
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
    icon: "clipboard",
    accent: "sky",
  },
  {
    name: "EyuX AI - Workspace",
    url: "https://apps.microsoft.com/detail/9NX5DBW6NHW1?hl=en-us&gl=US&ocid=pdpshare",
    blurb: "An AI workspace focused on practical desktop productivity.",
    icon: "workspace",
    accent: "mint",
  },
  {
    name: "ZenCapture",
    url: "https://apps.microsoft.com/detail/9NVW8TKD5R33?hl=en-us&gl=US&ocid=pdpshare",
    blurb: "A lightweight capture tool for daily ideas with screenshots.",
    icon: "capture",
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

function DeveloperAppIcon({
  icon,
}: {
  icon: DeveloperApp["icon"];
}) {
  if (icon === "clipboard") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M9 5.5A1.5 1.5 0 0 1 10.5 4h3A1.5 1.5 0 0 1 15 5.5v1H9v-1Z" />
        <path d="M8 6.5h8A2.5 2.5 0 0 1 18.5 9v9A2.5 2.5 0 0 1 16 20.5H8A2.5 2.5 0 0 1 5.5 18V9A2.5 2.5 0 0 1 8 6.5Z" />
        <path d="M9 11.25h6M9 15.25h4.5" />
      </svg>
    );
  }

  if (icon === "workspace") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M4.5 6.5h15v11h-15z" />
        <path d="M4.5 10.5h15M10 6.5v11" />
        <path d="m16.75 3.1.62 1.68 1.68.62-1.68.62-.62 1.68-.62-1.68-1.68-.62 1.68-.62.62-1.68Z" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M8 5.5H6.5A1.5 1.5 0 0 0 5 7v1.5M16 5.5h1.5A1.5 1.5 0 0 1 19 7v1.5M8 18.5H6.5A1.5 1.5 0 0 1 5 17v-1.5M16 18.5h1.5A1.5 1.5 0 0 0 19 17v-1.5" />
      <circle cx="12" cy="12" r="3.2" />
      <path d="M16.9 8.1h.01" />
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
    <div className="number-input-shell">
      <input
        ref={inputRef}
        id={id}
        type="number"
        min={min}
        max={max}
        step={step}
        value={value}
        placeholder={placeholder}
        inputMode="numeric"
        aria-label={ariaLabel}
        onChange={(event) => onChange(event.currentTarget.value)}
      />
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

function categoryFromExtension(extension: string): ResultViewTab {
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
  return `${result.path}:${result.modifiedUnix}`;
}

function normalizedExtension(result: SearchResult): string {
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
  return "none";
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

function relevanceScore(result: SearchResult, queryValue: string): number {
  const query = queryValue.trim().toLowerCase();
  if (!query) {
    return 0;
  }

  const name = result.name.toLowerCase();
  const path = result.path.toLowerCase();

  if (name.startsWith(query)) {
    return 10_000 - name.length;
  }

  const nameIndex = name.indexOf(query);
  if (nameIndex >= 0) {
    return 7_000 - nameIndex;
  }

  const pathIndex = path.indexOf(query);
  if (pathIndex >= 0) {
    return 4_000 - pathIndex;
  }

  return 0;
}

function highlightMatch(text: string, queryValue: string): ReactNode {
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
      <mark className="match-highlight" key={`m-${key}`}>
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
  const [loading, setLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<ActiveTab>("search");
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
  const [windowMode, setWindowMode] = useState<WindowMode>("full");
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
  const [desktopSettingsError, setDesktopSettingsError] = useState<string | null>(null);
  const [desktopSettingsMessage, setDesktopSettingsMessage] = useState<string | null>(null);
  const [selectedResultKey, setSelectedResultKey] = useState<string | null>(null);
  const [previewSourceState, setPreviewSourceState] = useState<Record<string, number>>({});
  const [previewReadyState, setPreviewReadyState] = useState<Record<string, true>>({});
  const [previewDataUrls, setPreviewDataUrls] = useState<Record<string, string>>({});
  const [selectedPreviewSourceIndex, setSelectedPreviewSourceIndex] = useState<number>(0);
  const [selectedPreviewReadyState, setSelectedPreviewReadyState] = useState<Record<string, true>>({});
  const [appVersion, setAppVersion] = useState<string>("");
  const previousIndexedCountRef = useRef<number | null>(null);
  const indexSyncTimeoutRef = useRef<number | null>(null);
  const duplicateNoticeTimeoutRef = useRef<number | null>(null);
  const actionNoticeTimeoutRef = useRef<number | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const createdAfterInputRef = useRef<HTMLInputElement | null>(null);
  const createdBeforeInputRef = useRef<HTMLInputElement | null>(null);
  const searchResultContextMenuRef = useRef<HTMLDivElement | null>(null);
  const searchResultRenameInputRef = useRef<HTMLInputElement | null>(null);
  const activeThemePreset = themePresetById(themePreset);
  const isQuickMode = windowMode === "quick";
  const formattedDesktopShortcut = formatShortcutLabel(
    desktopSettings.shortcut || DEFAULT_DESKTOP_SETTINGS.shortcut,
  );
  const currentShortcutLabel = desktopSettings.shortcutEnabled
    ? `Hotkey: ${formattedDesktopShortcut}`
    : "Hotkey off";
  const quickIndexScopeLabel = includeAllDrives ? "Index: all drives" : `Index: ${selectedDrive}:`;
  const desktopSettingsDirty =
    desktopSettings.backgroundModeEnabled !== desktopSettingsDraft.backgroundModeEnabled ||
    desktopSettings.shortcutEnabled !== desktopSettingsDraft.shortcutEnabled ||
    desktopSettings.shortcut.trim() !== desktopSettingsDraft.shortcut.trim();

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
  const trimmedQuery = query.trim();
  const requestedIndexConfigKey = includeAllDrives
    ? `ALL:${includeFolders ? "1" : "0"}`
    : selectedDrive
      ? `${selectedDrive}:${includeFolders ? "1" : "0"}`
      : "";

  const resultCounts = useMemo<Record<ResultViewTab, number>>(() => {
    const counts: Record<ResultViewTab, number> = {
      all: results.length,
      apps: 0,
      media: 0,
      docs: 0,
      archives: 0,
    };

    for (const result of results) {
      const category = categoryFromExtension(result.extension);
      if (category !== "all") {
        counts[category] += 1;
      }
    }

    return counts;
  }, [results]);

  const visibleResults = useMemo(() => {
    const filtered = results.filter((result) => {
      if (resultView === "all") {
        return true;
      }
      return categoryFromExtension(result.extension) === resultView;
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

      const rankDiff = relevanceScore(right, trimmedQuery) - relevanceScore(left, trimmedQuery);
      if (rankDiff !== 0) {
        return rankDiff;
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
  }, [results, resultView, resultSort, trimmedQuery]);

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
    if (!showPreviews || !selectedResult) {
      return [];
    }
    return [selectedResult].slice(0, PREVIEW_DATA_URL_LIMIT);
  }, [selectedResult, showPreviews]);

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
    window.localStorage.setItem(INCLUDE_FOLDERS_STORAGE_KEY, includeFolders ? "1" : "0");
  }, [includeFolders]);

  useEffect(() => {
    window.localStorage.setItem(INCLUDE_ALL_DRIVES_STORAGE_KEY, includeAllDrives ? "1" : "0");
  }, [includeAllDrives]);

  useEffect(() => {
    window.localStorage.setItem(SEARCH_LIMIT_STORAGE_KEY, String(defaultSearchLimit));
  }, [defaultSearchLimit]);

  useEffect(() => {
    setSearchLimitInput(String(defaultSearchLimit));
  }, [defaultSearchLimit]);

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

  useEffect(() => {
    setPreviewSourceState({});
    setPreviewReadyState({});
    setPreviewDataUrls({});
  }, [results, showPreviews]);

  useEffect(() => {
    const selectedKey = showPreviews && selectedResult ? rowKeyForResult(selectedResult) : "";

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
  }, [selectedResult, showPreviews]);

  useEffect(() => {
    setSelectedPreviewSourceIndex(0);
    setSelectedPreviewReadyState({});
  }, [selectedResultKey, showPreviews]);

  useEffect(() => {
    if (!showPreviews || previewLoadCandidates.length === 0) {
      return;
    }

    const candidates = previewLoadCandidates.filter((result) => previewKindFromResult(result) !== "none");
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
    };
  }, []);

  useEffect(() => {
    if (!trimmedQuery && !hasFilters) {
      setResults([]);
      setSearchError(null);
      setActionError(null);
      setLoading(false);
      return;
    }

    let active = true;
    const debounceMs = hasMetadataFilters ? FILTER_SEARCH_DEBOUNCE_MS : SEARCH_DEBOUNCE_MS;
    const timer = window.setTimeout(() => {
      const runSearch = async () => {
        setLoading(true);
        const minSize = toBytesFromMb(minSizeMb);
        const maxSize = toBytesFromMb(maxSizeMb);
        const minCreatedUnix = toUnixStart(createdAfter);
        const maxCreatedUnix = toUnixEnd(createdBefore);

        try {
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
            setResults(found);
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
    };
  }, [
    trimmedQuery,
    extension,
    minSizeMb,
    maxSizeMb,
    createdAfter,
    createdBefore,
    hasFilters,
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

  async function openResultPath(path: string): Promise<void> {
    const parentPath = parentDirectoryFromPath(path);
    if (!parentPath || parentPath === path) {
      await revealResult(path);
      return;
    }
    await openResult(parentPath);
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
    if (result.isDirectory) {
      return;
    }
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
    });
  }

  function openSearchResultDelete(result: SearchResult, rowKey: string): void {
    closeSearchResultContextMenu();
    setSearchResultDeleteToRecycleBin(false);
    setSearchResultDeleteCandidate({
      rowKey,
      path: result.path,
      name: resultDisplayName(result),
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

    if (result.isDirectory) {
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

  async function openResult(path: string): Promise<void> {
    try {
      await invoke("open_file", { path });
      setActionError(null);
    } catch (error) {
      setActionError(`Failed to open file: ${String(error)}`);
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
  const selectedResultRowKey = selectedResult ? rowKeyForResult(selectedResult) : "";
  const selectedResultIsDirectory = selectedResult?.isDirectory ?? false;
  const selectedResultExtension = selectedResult ? normalizedExtension(selectedResult) : "";
  const selectedResultShortType = selectedResult
    ? selectedResultIsDirectory
      ? "DIR"
      : (selectedResultExtension || "file").slice(0, 2).toUpperCase()
    : "??";
  const selectedResultExtensionLabel = selectedResult
    ? selectedResultIsDirectory
      ? "folder"
      : selectedResultExtension
        ? `.${selectedResultExtension}`
        : "file"
    : "";
  const selectedPreviewKind = selectedResult && showPreviews ? previewKindFromResult(selectedResult) : "none";
  const selectedPreviewSources =
    selectedResult && selectedPreviewKind !== "none"
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
    !selectedPreviewFailed && selectedPreviewKind !== "none"
      ? (selectedPreviewSources[selectedPreviewIndex] ?? "")
      : "";
  const hasSelectedPreview = selectedPreviewSrc.length > 0;
  const showQuickInlineSearching = isQuickMode && loading && hasSearchRequest;
  const showQuickEmptyState = isQuickMode && visibleResults.length === 0 && !showQuickInlineSearching;
  const quickPreviewEmptyTitle = searchError
    ? "Search couldn't finish"
    : showQuickEmptyState
      ? trimmedQuery || hasFilters
        ? "No files match the current filters"
        : "Start typing to search indexed files"
      : "Pick a result to preview";
  const quickPreviewEmptyDetail = searchError
    ? "Check the message above and try again."
    : showQuickEmptyState && (trimmedQuery || hasFilters)
      ? "Try another search or adjust the filters."
      : "";
  const activeSearchResultMenu = searchResultContextMenu?.result ?? null;
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

  return (
    <div className={`app-shell ${isQuickMode ? "quick-window-mode" : ""}`}>
      <main className={`spotlight-panel ${isQuickMode ? "spotlight-panel-quick" : ""}`}>
        <header className={`panel-header ${isQuickMode ? "quick-panel-header" : ""}`}>
          <div className="panel-title-block">
            {isQuickMode ? <span className="quick-mode-badge">Quick Window</span> : null}
            <h1>OmniSearch</h1>
            {isQuickMode ? (
              <p className="panel-subtitle">
                Jump back to the full workspace when you need deeper controls.
              </p>
            ) : null}
          </div>
          <div className={`header-tools ${isQuickMode ? "quick-header-tools" : ""}`}>
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
            {isQuickMode ? (
              <>
                <span className="quick-index-indicator" title={`Current index scope: ${quickIndexScopeLabel}`}>
                  {quickIndexScopeLabel}
                </span>
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
              className={`tab ${activeTab === "about" ? "is-active" : ""}`}
              onClick={() => {
                setActiveTab("about");
              }}
            >
              About
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
                className={`status-dot ${
                  status.indexing || indexSyncing ? "live" : status.ready ? "ready" : "idle"
                }`}
              />
              <span>{statusText}</span>
            </div>

            {visibleStatusError ? <p className="error-row">{visibleStatusError}</p> : null}
            {driveError ? <p className="error-row">{driveError}</p> : null}

            <div className={`search-input-shell ${isQuickMode ? "quick-search-input-shell" : ""}`}>
              <span className="search-input-icon" aria-hidden="true">
                <SearchLensIcon />
              </span>
              <input
                ref={searchInputRef}
                className={`search-input ${isQuickMode ? "quick-search-input" : ""}`}
                type="text"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
                placeholder="Type to search across indexed items..."
                autoFocus
              />
            </div>

            <section className={`filter-grid ${isQuickMode ? "quick-filter-grid" : ""}`}>
              <label>
                Extension
                <input
                  type="text"
                  value={extension}
                  onChange={(event) => setExtension(event.currentTarget.value)}
                  placeholder=".mp4 or folder"
                />
              </label>
              <label>
                Min size (MB)
                <NumberInputField
                  min={0}
                  step={1}
                  value={minSizeMb}
                  placeholder="0"
                  ariaLabel="Minimum size in megabytes"
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

            <section className={`results-panel ${isQuickMode ? "quick-results-panel" : ""}`}>
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
                      {visibleResults.length !== results.length
                        ? ` / ${results.length.toLocaleString()}`
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
                    <span>Sort</span>
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

              {loading ? <p className="hint compact-hint">Searching...</p> : null}
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

              <div
                className={
                  isQuickMode
                    ? `results-stage quick-results-stage ${showQuickEmptyState ? "is-empty" : ""}`
                    : "results-stage"
                }
              >
                <div
                  className={`results-list-shell ${isQuickMode ? "quick-results-column" : ""} ${
                    isQuickMode && canLoadMore ? "has-overlay-load-more" : ""
                  }`}
                >
                  <ul className={`results-list ${isQuickMode ? "quick-results-list" : ""}`}>
                    {visibleResults.map((result) => {
                      const rowKey = rowKeyForResult(result);
                      const isDirectory = result.isDirectory;
                      const normalizedExt = normalizedExtension(result);
                      const shortType = isDirectory
                        ? "DIR"
                        : (normalizedExt || "file").slice(0, 2).toUpperCase();
                      const extensionLabel = isDirectory
                        ? "folder"
                        : normalizedExt
                          ? `.${normalizedExt}`
                          : "file";
                      const previewKind = showPreviews ? previewKindFromResult(result) : "none";
                      const previewSources =
                        previewKind !== "none"
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
                        !previewFailed && previewKind !== "none"
                          ? (previewSources[activePreviewIndex] ?? "")
                          : "";
                      const hasRenderablePreview = previewSrc.length > 0;
                      const canDragResultFile = !isDirectory;

                      return (
                        <li
                          key={rowKey}
                          className={`result-row clickable ${isQuickMode ? "quick-result-row" : ""} ${
                            isQuickMode && rowKey === selectedResultRowKey ? "is-selected" : ""
                          } ${canDragResultFile ? "draggable-file" : ""}`}
                          draggable={canDragResultFile}
                          onDragStart={(event) => {
                            handleSearchResultDragStart(event, result);
                          }}
                          onContextMenu={(event) => {
                            openSearchResultContextMenu(event, result, rowKey);
                          }}
                          role="button"
                          tabIndex={0}
                          title={
                            isQuickMode
                              ? "Click to select, double-click to open"
                              : "Click to reveal in folder, double-click to open"
                          }
                          onClick={() => {
                            if (hasSelectedText()) {
                              return;
                            }
                            if (isQuickMode) {
                              setSelectedResultKey(rowKey);
                              return;
                            }
                            void revealResult(result.path);
                          }}
                          onDoubleClick={() => {
                            if (hasSelectedText()) {
                              return;
                            }
                            void openResult(result.path);
                          }}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") {
                              event.preventDefault();
                              void openResult(result.path);
                            } else if (event.key === " ") {
                              event.preventDefault();
                              if (isQuickMode) {
                                setSelectedResultKey(rowKey);
                                return;
                              }
                              void revealResult(result.path);
                            }
                          }}
                        >
                          {showPreviews ? (
                            <div
                              className={`result-preview ${previewKind} ${isDirectory ? "folder" : ""}`}
                              aria-hidden="true"
                            >
                              <span className="preview-fallback">{shortType}</span>
                              {hasRenderablePreview && previewKind === "image" ? (
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
                            <div className={`result-icon ${isDirectory ? "folder" : ""}`}>{shortType}</div>
                          )}

                          <div className="result-main">
                            <strong>{highlightMatch(result.name, trimmedQuery)}</strong>
                            <span>{highlightMatch(result.path, trimmedQuery)}</span>
                          </div>
                          <div className="result-meta">
                            <span className="meta-chip">{extensionLabel}</span>
                            <span className="meta-chip">{formatBytes(result.size)}</span>
                            <span className="meta-chip">{formatUnix(result.createdUnix)}</span>
                          </div>
                          <div className="result-actions">
                            <button
                              type="button"
                              className="row-action"
                              onClick={(event) => {
                                event.stopPropagation();
                                void openResult(result.path);
                              }}
                            >
                              Open
                            </button>
                            <button
                              type="button"
                              className="row-action"
                              onClick={(event) => {
                                event.stopPropagation();
                                void revealResult(result.path);
                              }}
                            >
                              Folder
                            </button>
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

              {isQuickMode ? (
                <aside className="quick-preview-panel" aria-label="Selected file preview">
                  {selectedResult ? (
                    <div className="quick-preview-surface">
                      <div
                        className={`quick-preview-stage ${selectedPreviewKind} ${
                          selectedResultIsDirectory ? "folder" : ""
                        }`}
                      >
                        <span className="preview-fallback">{selectedResultShortType}</span>
                        {hasSelectedPreview && selectedPreviewKind === "image" ? (
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
                        {!hasSelectedPreview ? (
                          <div className="quick-preview-placeholder">
                            <strong>{showPreviews ? "Preview unavailable" : "Preview disabled"}</strong>
                            <span>
                              {showPreviews
                                ? "This file type cannot be rendered here yet, but you can still inspect the metadata and open it."
                                : "Turn previews back on from the toolbar to load images, video, and PDF previews."}
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
                                void openResult(selectedResult.path);
                              }}
                            >
                              Open file
                            </button>
                            <button
                              type="button"
                              className="row-action"
                              onClick={() => {
                                void revealResult(selectedResult.path);
                              }}
                            >
                              Reveal folder
                            </button>
                          </div>
                        </div>

                        <div className="quick-preview-meta">
                          <div className="quick-preview-meta-card">
                            <span>Type</span>
                            <strong>{selectedResultExtensionLabel}</strong>
                          </div>
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

              {!isQuickMode && canLoadMore ? (
                <div className="load-more-row">
                  <button type="button" className="load-more-button" onClick={loadMoreResults}>
                    {`Load more (+${defaultSearchLimit.toLocaleString()})`}
                  </button>
                </div>
              ) : null}
            </section>
          </section>
        ) : null}

        {activeTab === "duplicates" ? (
          <section className="tab-panel" aria-label="Find duplicate files">
            <div className="status-row">
              <span
                className={`status-dot ${
                  status.indexing || indexSyncing ? "live" : status.ready ? "ready" : "idle"
                }`}
              />
              <span>{statusText}</span>
            </div>

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
                      className={`settings-switch-card settings-switch-card-button ${
                        desktopSettingsDraft.backgroundModeEnabled ? "is-active" : ""
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
                        className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${
                          desktopSettingsDraft.backgroundModeEnabled ? "is-on" : ""
                        }`}
                        aria-hidden="true"
                      >
                        <span className="scan-switch-slider" aria-hidden="true" />
                        <span>{desktopSettingsDraft.backgroundModeEnabled ? "On" : "Off"}</span>
                      </span>
                    </button>

                    <button
                      type="button"
                      className={`settings-switch-card settings-switch-card-button ${
                        desktopSettingsDraft.shortcutEnabled ? "is-active" : ""
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
                        className={`scan-switch settings-switch-toggle settings-switch-toggle-button ${
                          desktopSettingsDraft.shortcutEnabled ? "is-on" : ""
                        }`}
                        aria-hidden="true"
                      >
                        <span className="scan-switch-slider" aria-hidden="true" />
                        <span>{desktopSettingsDraft.shortcutEnabled ? "On" : "Off"}</span>
                      </span>
                    </button>

                    <label className="desktop-shortcut-field" htmlFor="desktop-shortcut-input">
                      <span>Shortcut</span>
                      <input
                        id="desktop-shortcut-input"
                        type="text"
                        value={desktopSettingsDraft.shortcut}
                        disabled={desktopSettingsSaving || desktopSettingsLoading}
                        placeholder={DEFAULT_DESKTOP_SETTINGS.shortcut}
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
                        className="ghost-button"
                        disabled={
                          desktopSettingsSaving || desktopSettingsLoading || !desktopSettingsDirty
                        }
                        onClick={() => {
                          void saveDesktopBehaviorSettings();
                        }}
                      >
                        {desktopSettingsSaving ? "Saving..." : "Save desktop settings"}
                      </button>
                      <button
                        type="button"
                        className="ghost-button"
                        disabled={
                          desktopSettingsSaving || desktopSettingsLoading || !desktopSettingsDirty
                        }
                        onClick={() => {
                          setDesktopSettingsDraft(desktopSettings);
                          setDesktopSettingsError(null);
                          setDesktopSettingsMessage(null);
                        }}
                      >
                        Reset changes
                      </button>
                    </div>
                  </div>

                  <p className="advanced-note">
                    Tray menu includes Open Quick Window, Open Main App, Hide, and Quit. Shortcut
                    changes apply immediately after saving.
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
                    <button type="button" className="ghost-button" onClick={applySearchLimitPreference}>
                      Update
                    </button>
                    <button type="button" className="ghost-button" onClick={resetSearchLimitPreference}>
                      Reset default ({SEARCH_LIMIT})
                    </button>
                  </div>
                  <p className="advanced-note">
                    {`Current default: ${defaultSearchLimit.toLocaleString()} | Current active limit: ${searchLimit.toLocaleString()}`}
                  </p>
                  <p className="advanced-note">Load more uses this same amount each click.</p>
                  {searchLimitError ? <p className="advanced-error">{searchLimitError}</p> : null}
                  {searchLimitMessage ? <p className="advanced-success">{searchLimitMessage}</p> : null}
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
                <span className="theme-mode-status">
                  {themeMode === "dark" ? "Dark mode active" : "Light mode active"}
                </span>
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

        {activeTab === "about" ? (
          <section className="tab-panel scrollable-tab-panel" aria-label="About OmniSearch and developer">
            <div className="about-panel about-panel-flat">
              <div className="about-header">
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
                    <span className="about-support-label">Donate</span>
                    <strong>Buy me a coffee</strong>
                    <p>
                      If OmniSearch helps your workflow, you can support future updates and desktop
                      tools here.
                    </p>
                  </div>
                  <button
                    type="button"
                    className="about-support-button"
                    onClick={() => {
                      void openExternalLink(DONATE_URL);
                    }}
                  >
                    <BuyMeCoffeeIcon />
                    <span>Buy me a coffee</span>
                  </button>
                </section>

                <section className="about-apps-section" aria-label="More apps by Eyuel">
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
                            <DeveloperAppIcon icon={item.icon} />
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
                void openResult(activeSearchResultMenu.path);
              }}
            >
              Open file
            </button>
            <button
              type="button"
              className="result-context-menu-item"
              role="menuitem"
              onClick={() => {
                closeSearchResultContextMenu();
                void openResultPath(activeSearchResultMenu.path);
              }}
            >
              Open path
            </button>
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

            <button
              type="button"
              className="result-context-menu-item"
              role="menuitem"
              onClick={() => {
                closeSearchResultContextMenu();
                void handleSearchResultCopy(activeSearchResultMenu.path, "path");
              }}
            >
              Copy path
            </button>
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
              className="modal-card"
              onClick={(event) => {
                event.stopPropagation();
              }}
            >
              <h3>Rename file</h3>
              <p>{searchResultRenameDraft.currentName}</p>
              <p className="modal-path">{searchResultRenameDraft.path}</p>

              <label className="modal-input-group">
                <span>New name</span>
                <input
                  ref={searchResultRenameInputRef}
                  type="text"
                  value={searchResultRenameDraft.nextName}
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
              <h3>Delete file?</h3>
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
      </main>
    </div>
  );
}

export default App;
