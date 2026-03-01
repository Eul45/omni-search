import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import "./App.css";

const POLL_INTERVAL_MS = 700;
const SEARCH_DEBOUNCE_MS = 130;
const SEARCH_LIMIT = 200;
const SEARCH_LIMIT_MIN = SEARCH_LIMIT;
const SEARCH_LIMIT_MAX = 5000;
const PREVIEW_PREFETCH_LIMIT = 40;

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

type ActiveTab = "search" | "duplicates" | "advanced" | "about";
type ResultViewTab = "all" | "apps" | "media" | "docs" | "archives";
type ResultSortMode = "relevance" | "newest" | "largest" | "name";
type ThemeMode = "dark" | "light";
type PreviewKind = "image" | "video" | "pdf" | "none";

const DEVELOPER_NAME = "Eyuel Engida";
const THEME_STORAGE_KEY = "omnisearch_theme_mode";
const PREVIEW_STORAGE_KEY = "omnisearch_show_previews";
const INCLUDE_FOLDERS_STORAGE_KEY = "omnisearch_include_folders";
const INCLUDE_ALL_DRIVES_STORAGE_KEY = "omnisearch_include_all_drives";
const SEARCH_LIMIT_STORAGE_KEY = "omnisearch_search_limit";
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
  const [activeTab, setActiveTab] = useState<ActiveTab>("search");
  const [duplicateMinSizeMb, setDuplicateMinSizeMb] = useState("50");
  const [duplicateGroups, setDuplicateGroups] = useState<DuplicateGroup[]>([]);
  const [duplicatesLoading, setDuplicatesLoading] = useState(false);
  const [duplicatesError, setDuplicatesError] = useState<string | null>(null);
  const [duplicateScanStatus, setDuplicateScanStatus] = useState<DuplicateScanStatus>({
    running: false,
    cancelRequested: false,
    scannedFiles: 0,
    totalFiles: 0,
    groupsFound: 0,
    progressPercent: 0,
  });
  const [resultView, setResultView] = useState<ResultViewTab>("all");
  const [resultSort, setResultSort] = useState<ResultSortMode>("relevance");
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
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [settingsMessage, setSettingsMessage] = useState<string | null>(null);
  const [previewSourceState, setPreviewSourceState] = useState<Record<string, number>>({});
  const [previewReadyState, setPreviewReadyState] = useState<Record<string, true>>({});
  const [previewDataUrls, setPreviewDataUrls] = useState<Record<string, string>>({});
  const previousIndexedCountRef = useRef<number | null>(null);
  const indexSyncTimeoutRef = useRef<number | null>(null);

  const hasFilters =
    extension.trim().length > 0 ||
    minSizeMb.trim().length > 0 ||
    maxSizeMb.trim().length > 0 ||
    createdAfter.length > 0 ||
    createdBefore.length > 0;

  const selectedDriveInfo = drives.find((drive) => drive.letter === selectedDrive);
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
    document.documentElement.setAttribute("data-theme", themeMode);
    window.localStorage.setItem(THEME_STORAGE_KEY, themeMode);
  }, [themeMode]);

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
    setSearchLimit(defaultSearchLimit);
  }, [trimmedQuery, extension, minSizeMb, maxSizeMb, createdAfter, createdBefore, hasFilters, defaultSearchLimit]);

  useEffect(() => {
    setPreviewSourceState({});
    setPreviewReadyState({});
    setPreviewDataUrls({});
  }, [results, showPreviews]);

  useEffect(() => {
    if (!showPreviews || visibleResults.length === 0) {
      return;
    }

    const candidates = visibleResults
      .slice(0, PREVIEW_PREFETCH_LIMIT)
      .filter((result) => previewKindFromResult(result) !== "none");
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
  }, [showPreviews, visibleResults, previewDataUrls]);

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
    return () => {
      if (indexSyncTimeoutRef.current !== null) {
        window.clearTimeout(indexSyncTimeoutRef.current);
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
    const timer = window.setTimeout(() => {
      const runSearch = async () => {
        setLoading(true);
        const minSize = toBytesFromMb(minSizeMb);
        const maxSize = toBytesFromMb(maxSizeMb);
        const minCreatedUnix = toUnixStart(createdAfter);
        const maxCreatedUnix = toUnixEnd(createdBefore);

        try {
          const found = await invoke<SearchResult[]>("search_files", {
            query: trimmedQuery,
            extension: extension.trim(),
            min_size: minSize,
            max_size: maxSize,
            min_created_unix: minCreatedUnix,
            max_created_unix: maxCreatedUnix,
            limit: searchLimit,
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
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [trimmedQuery, extension, minSizeMb, maxSizeMb, createdAfter, createdBefore, hasFilters, searchLimit]);

  async function reindex(): Promise<void> {
    if (!selectedDrive) {
      return;
    }
    try {
      setPendingIndexConfigKey(requestedIndexConfigKey);
      const next = await invoke<IndexStatus>("start_indexing", {
        drive: selectedDrive,
        includeFolders: includeFolders,
        include_folders: includeFolders,
        includeAllDrives: includeAllDrives,
        include_all_drives: includeAllDrives,
      });
      setStatus(next);
      setAppliedIndexConfigKey(requestedIndexConfigKey);
      setPendingIndexConfigKey("");
    } catch (error) {
      setStatus((previous) => ({
        ...previous,
        lastError: String(error),
      }));
      setPendingIndexConfigKey("");
    }
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
      setSettingsError(`Enter a valid number between ${SEARCH_LIMIT_MIN} and ${SEARCH_LIMIT_MAX}.`);
      setSettingsMessage(null);
      return;
    }

    const normalized = normalizeSearchLimit(parsed);
    setDefaultSearchLimit(normalized);
    setSearchLimit(normalized);
    setSearchLimitInput(String(normalized));
    setSettingsError(null);
    if (normalized !== parsed) {
      setSettingsMessage(
        `Saved. Adjusted to ${normalized.toLocaleString()} to keep the allowed range.`,
      );
      return;
    }
    setSettingsMessage(`Saved. New searches now start with ${normalized.toLocaleString()} results.`);
  }

  function resetSearchLimitPreference(): void {
    setDefaultSearchLimit(SEARCH_LIMIT);
    setSearchLimit(SEARCH_LIMIT);
    setSearchLimitInput(String(SEARCH_LIMIT));
    setSettingsError(null);
    setSettingsMessage(`Reset to default: ${SEARCH_LIMIT.toLocaleString()} results.`);
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
        min_size: minSize,
        max_groups: 250,
        max_files_per_group: 40,
      });
      setDuplicateGroups(groups);
      setDuplicatesError(null);
    } catch (error) {
      setDuplicateGroups([]);
      const message = String(error);
      if (message.toLowerCase().includes("cancel")) {
        setDuplicatesError("Duplicate scan cancelled.");
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
        setDuplicateScanStatus((previous) => ({
          ...previous,
          cancelRequested: true,
        }));
      }
    } catch (error) {
      setDuplicatesError(`Failed to cancel duplicate scan: ${String(error)}`);
    }
  }

  function clearDuplicateResults(): void {
    setDuplicateGroups([]);
    setDuplicatesError(null);
    setDuplicateScanStatus({
      running: false,
      cancelRequested: false,
      scannedFiles: 0,
      totalFiles: 0,
      groupsFound: 0,
      progressPercent: 0,
    });
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

  return (
    <div className="app-shell">
      <main className="spotlight-panel">
        <header className="panel-header">
          <h1>OmniSearch</h1>
          <div className="header-tools">
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
                    <option
                      key={drive.letter}
                      value={drive.letter}
                      disabled={!drive.canOpenVolume}
                    >
                      {`${drive.letter}: (${drive.filesystem || "Unknown"})${drive.canOpenVolume ? "" : " - admin required"}`}
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
                  setIncludeAllDrives(event.currentTarget.checked);
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
                  setIncludeFolders(event.currentTarget.checked);
                }}
              />
              <span>Include folders</span>
            </label>
            <button type="button" className="ghost-button" onClick={reindex}>
              Reindex
            </button>
          </div>
        </header>

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
            Advanced
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
        </nav>

        {activeTab === "search" ? (
          <section className="tab-panel" aria-label="Search files">
            <div className="status-row">
              <span
                className={`status-dot ${
                  status.indexing || indexSyncing ? "live" : status.ready ? "ready" : "idle"
                }`}
              />
              <span>{statusText}</span>
            </div>

            {status.lastError ? <p className="error-row">{status.lastError}</p> : null}
            {driveError ? <p className="error-row">{driveError}</p> : null}
            {selectedDriveInfo && !selectedDriveInfo.canOpenVolume ? (
              <p className="error-row">
                The selected drive cannot be indexed without administrator privileges.
              </p>
            ) : null}

            <input
              className="search-input"
              type="text"
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              placeholder="Type to search across indexed items..."
              autoFocus
            />

            <section className="filter-grid">
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
                <input
                  type="number"
                  min="0"
                  step="1"
                  value={minSizeMb}
                  onChange={(event) => setMinSizeMb(event.currentTarget.value)}
                  placeholder="0"
                />
              </label>
              <label>
                Max size (MB)
                <input
                  type="number"
                  min="0"
                  step="1"
                  value={maxSizeMb}
                  onChange={(event) => setMaxSizeMb(event.currentTarget.value)}
                  placeholder="2048"
                />
              </label>
              <label>
                Created after
                <input
                  type="date"
                  value={createdAfter}
                  onChange={(event) => setCreatedAfter(event.currentTarget.value)}
                />
              </label>
              <label>
                Created before
                <input
                  type="date"
                  value={createdBefore}
                  onChange={(event) => setCreatedBefore(event.currentTarget.value)}
                />
              </label>
            </section>

            <section className="results-panel">
              <div className="results-toolbar">
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
              {!loading && !searchError && visibleResults.length === 0 && (trimmedQuery || hasFilters) ? (
                <p className="hint compact-hint">No items match the current filters.</p>
              ) : null}

              <ul className="results-list">
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
                      ? [
                          ...(previewDataUrls[rowKey] ? [previewDataUrls[rowKey]] : []),
                          ...previewSourcesFromPath(result.path),
                        ].filter((source, index, all) => source.length > 0 && all.indexOf(source) === index)
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

                  return (
                    <li
                      key={rowKey}
                      className="result-row clickable"
                      role="button"
                      tabIndex={0}
                      title="Click to reveal in folder, double-click to open"
                      onClick={() => {
                        void revealResult(result.path);
                      }}
                      onDoubleClick={() => {
                        void openResult(result.path);
                      }}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void openResult(result.path);
                        } else if (event.key === " ") {
                          event.preventDefault();
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

              {canLoadMore ? (
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

            {status.lastError ? <p className="error-row">{status.lastError}</p> : null}
            {driveError ? <p className="error-row">{driveError}</p> : null}
            {selectedDriveInfo && !selectedDriveInfo.canOpenVolume ? (
              <p className="error-row">
                The selected drive cannot be indexed without administrator privileges.
              </p>
            ) : null}

            <section className="duplicate-controls">
              <label className="duplicate-size-input">
                <span>Min file size (MB)</span>
                <input
                  type="number"
                  min="0"
                  step="1"
                  value={duplicateMinSizeMb}
                  onChange={(event) => setDuplicateMinSizeMb(event.currentTarget.value)}
                  placeholder="50"
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
                              void revealResult(cleanedPath);
                            }
                          }}
                          onDoubleClick={() => {
                            if (hasPath) {
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
          </section>
        ) : null}

        {activeTab === "advanced" ? (
          <section className="tab-panel" aria-label="Advanced settings">
            <div className="about-panel advanced-panel">
              <div className="about-header">
                <div>
                  <h2>Advanced settings</h2>
                  <p className="about-tagline">
                    Control how many search results load at once.
                  </p>
                </div>
              </div>

              <div className="advanced-settings">
                <label htmlFor="search-limit-input">
                  Results per search (range {SEARCH_LIMIT_MIN} - {SEARCH_LIMIT_MAX})
                </label>
                <input
                  id="search-limit-input"
                  type="number"
                  min={SEARCH_LIMIT_MIN}
                  max={SEARCH_LIMIT_MAX}
                  step="50"
                  value={searchLimitInput}
                  onChange={(event) => {
                    setSearchLimitInput(event.currentTarget.value);
                    if (settingsError) {
                      setSettingsError(null);
                    }
                    if (settingsMessage) {
                      setSettingsMessage(null);
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
                {settingsError ? <p className="advanced-error">{settingsError}</p> : null}
                {settingsMessage ? <p className="advanced-success">{settingsMessage}</p> : null}
              </div>
            </div>
          </section>
        ) : null}

        {activeTab === "about" ? (
          <section className="tab-panel" aria-label="About OmniSearch and developer">
            <div className="about-panel">
              <div className="about-header">
                <div>
                  <h2>About OmniSearch</h2>
                  <p className="about-tagline">
                    Fast local search across your drives with rich filters.
                  </p>
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
            </div>
          </section>
        ) : null}
      </main>
    </div>
  );
}

export default App;
