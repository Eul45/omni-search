import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

const POLL_INTERVAL_MS = 700;
const SEARCH_DEBOUNCE_MS = 130;
const SEARCH_LIMIT = 200;

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

const DEVELOPER_NAME = "Eyuel Engida";
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

function App() {
  const [status, setStatus] = useState<IndexStatus>({
    indexing: false,
    ready: false,
    indexedCount: 0,
    lastError: null,
  });
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

  const hasFilters =
    extension.trim().length > 0 ||
    minSizeMb.trim().length > 0 ||
    maxSizeMb.trim().length > 0 ||
    createdAfter.length > 0 ||
    createdBefore.length > 0;

  const selectedDriveInfo = drives.find((drive) => drive.letter === selectedDrive);

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

    let active = true;
    const beginIndexing = async () => {
      try {
        const initial = await invoke<IndexStatus>("start_indexing", { drive: selectedDrive });
        if (active) {
          setStatus(initial);
        }
      } catch (error) {
        if (active) {
          setStatus((previous) => ({
            ...previous,
            lastError: String(error),
          }));
        }
      }
    };
    void beginIndexing();

    return () => {
      active = false;
    };
  }, [selectedDrive]);

  useEffect(() => {
    const trimmedQuery = query.trim();
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
            limit: SEARCH_LIMIT,
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
  }, [query, extension, minSizeMb, maxSizeMb, createdAfter, createdBefore, hasFilters]);

  async function reindex(): Promise<void> {
    if (!selectedDrive) {
      return;
    }
    try {
      const next = await invoke<IndexStatus>("start_indexing", { drive: selectedDrive });
      setStatus(next);
    } catch (error) {
      setStatus((previous) => ({
        ...previous,
        lastError: String(error),
      }));
    }
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

  const statusText = status.indexing
    ? `Indexing ${status.indexedCount.toLocaleString()} files...`
    : status.ready
      ? `Indexed ${status.indexedCount.toLocaleString()} files`
      : "Indexer idle";

  return (
    <div className="app-shell">
      <main className="spotlight-panel">
        <header className="panel-header">
          <h1>OmniSearch</h1>
          <div className="header-tools">
            <label className="drive-picker" htmlFor="drive-picker">
              <span>Drive</span>
              <select
                id="drive-picker"
                value={selectedDrive}
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
            <button type="button" className="ghost-button" onClick={reindex}>
              Reindex
            </button>
          </div>
        </header>

        <div className="status-row">
          <span
            className={`status-dot ${status.indexing ? "live" : status.ready ? "ready" : "idle"}`}
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
          placeholder="Type to search across indexed files..."
          autoFocus
        />

        <section className="filter-grid">
          <label>
            Extension
            <input
              type="text"
              value={extension}
              onChange={(event) => setExtension(event.currentTarget.value)}
              placeholder=".mp4"
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
          {loading ? <p className="hint">Searching...</p> : null}
          {searchError ? <p className="error-row">{searchError}</p> : null}
          {actionError ? <p className="error-row">{actionError}</p> : null}
          {!loading && !searchError && results.length === 0 && (query.trim() || hasFilters) ? (
            <p className="hint">No files match the current filters.</p>
          ) : null}
          {!loading && !searchError && results.length > 0 ? (
            <p className="hint">Click a result to show it in folder. Double-click to open it.</p>
          ) : null}

          <ul>
            {results.map((result) => (
              <li
                key={`${result.path}:${result.modifiedUnix}`}
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
                <div className="result-main">
                  <strong>{result.name}</strong>
                  <span>{result.path}</span>
                </div>
                <div className="result-meta">
                  <span>{result.extension ? `.${result.extension}` : "-"}</span>
                  <span>{formatBytes(result.size)}</span>
                  <span>{formatUnix(result.createdUnix)}</span>
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
            ))}
          </ul>
        </section>

        <section className="about-panel" aria-label="Developer info">
          <h2>Developer</h2>
          <p>{DEVELOPER_NAME}</p>
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
        </section>
      </main>
    </div>
  );
}

export default App;
