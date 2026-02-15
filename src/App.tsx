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
  const [query, setQuery] = useState("");
  const [extension, setExtension] = useState("");
  const [minSizeMb, setMinSizeMb] = useState("");
  const [maxSizeMb, setMaxSizeMb] = useState("");
  const [createdAfter, setCreatedAfter] = useState("");
  const [createdBefore, setCreatedBefore] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const hasFilters =
    extension.trim().length > 0 ||
    minSizeMb.trim().length > 0 ||
    maxSizeMb.trim().length > 0 ||
    createdAfter.length > 0 ||
    createdBefore.length > 0;

  useEffect(() => {
    let active = true;

    const beginIndexing = async () => {
      try {
        const initial = await invoke<IndexStatus>("start_indexing", { drive: "C" });
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
    const trimmedQuery = query.trim();
    if (!trimmedQuery && !hasFilters) {
      setResults([]);
      setSearchError(null);
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
    try {
      const next = await invoke<IndexStatus>("start_indexing", { drive: "C" });
      setStatus(next);
    } catch (error) {
      setStatus((previous) => ({
        ...previous,
        lastError: String(error),
      }));
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
          <button type="button" className="ghost-button" onClick={reindex}>
            Reindex
          </button>
        </header>

        <div className="status-row">
          <span
            className={`status-dot ${status.indexing ? "live" : status.ready ? "ready" : "idle"}`}
          />
          <span>{statusText}</span>
        </div>

        {status.lastError ? <p className="error-row">{status.lastError}</p> : null}

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
          {!loading && !searchError && results.length === 0 && (query.trim() || hasFilters) ? (
            <p className="hint">No files match the current filters.</p>
          ) : null}

          <ul>
            {results.map((result) => (
              <li key={`${result.path}:${result.modifiedUnix}`} className="result-row">
                <div className="result-main">
                  <strong>{result.name}</strong>
                  <span>{result.path}</span>
                </div>
                <div className="result-meta">
                  <span>{result.extension ? `.${result.extension}` : "-"}</span>
                  <span>{formatBytes(result.size)}</span>
                  <span>{formatUnix(result.createdUnix)}</span>
                </div>
              </li>
            ))}
          </ul>
        </section>
      </main>
    </div>
  );
}

export default App;
