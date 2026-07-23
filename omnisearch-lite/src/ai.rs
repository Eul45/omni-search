//! Minimal AI client for OmniSearch Lite â€” talks to any OpenAI-compatible
//! chat-completions endpoint. Users pick a provider and paste their API key.
//! Blocking (ureq), runs on a worker thread so the UI never stalls.

use anyhow::{anyhow, Result};

// Each provider speaks the OpenAI Chat Completions protocol (Gemini through its
// OpenAI-compatibility layer), so no per-provider request code is needed.

pub struct AiProvider {
    pub id: &'static str,
    pub name: &'static str,
    pub endpoint: &'static str,
    pub default_model: &'static str,
    pub key_optional: bool,
}

pub const AI_PROVIDERS: &[AiProvider] = &[
    AiProvider {
        id: "gemini",
        name: "Google Gemini",
        endpoint: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        default_model: "gemini-2.5-flash",
        key_optional: false,
    },
    AiProvider {
        id: "openai",
        name: "OpenAI",
        endpoint: "https://api.openai.com/v1/chat/completions",
        default_model: "gpt-4o-mini",
        key_optional: false,
    },
    AiProvider {
        id: "claude",
        name: "Anthropic Claude",
        endpoint: "https://api.anthropic.com/v1/messages",
        default_model: "claude-sonnet-4-20250514",
        key_optional: false,
    },
    AiProvider {
        id: "grok",
        name: "xAI Grok",
        endpoint: "https://api.x.ai/v1/chat/completions",
        default_model: "grok-3-mini-fast",
        key_optional: false,
    },
    AiProvider {
        id: "deepseek",
        name: "DeepSeek",
        endpoint: "https://api.deepseek.com/chat/completions",
        default_model: "deepseek-chat",
        key_optional: false,
    },
    AiProvider {
        id: "ollama",
        name: "Ollama (local)",
        endpoint: "http://localhost:11434/v1/chat/completions",
        default_model: "llama3.2",
        key_optional: true,
    },
];

/// Find a provider preset by endpoint URL.
pub fn match_provider(endpoint: &str) -> Option<&'static AiProvider> {
    let e = endpoint.trim().trim_end_matches('/');
    AI_PROVIDERS.iter().find(|p| {
        let pe = p.endpoint.trim_end_matches('/');
        pe.eq_ignore_ascii_case(e)
    })
}

/// Find a provider preset by id (e.g. "gemini", "openai").
pub fn find_provider(id: &str) -> Option<&'static AiProvider> {
    AI_PROVIDERS.iter().find(|p| p.id == id)
}

// â”€â”€ Default fallbacks (DeepSeek) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
const DEFAULT_ENDPOINT: &str = "https://api.deepseek.com/chat/completions";
const DEFAULT_MODEL: &str = "deepseek-chat";

// â”€â”€ API key resolution â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// Order: SQLite settings table â†’ env vars â†’ %APPDATA%/omnisearch-lite/ai_key.txt â†’ embedded keys â†’ hardcoded
const HARDCODED_KEY: &str = "";

const EMBEDDED_KEYS_RAW: Option<&str> = option_env!("OMNISEARCH_KEYS");

fn get_embedded_keys_count() -> usize {
    if let Some(raw) = EMBEDDED_KEYS_RAW {
        if raw.trim().is_empty() {
            0
        } else {
            raw.split(',').count()
        }
    } else {
        0
    }
}

fn get_rotated_key(index: usize) -> Option<String> {
    let raw = EMBEDDED_KEYS_RAW?;
    let parts: Vec<&str> = raw.split(',').collect();
    let hex_str = parts.get(index)?;

    let mut bytes = Vec::new();
    let chars: Vec<char> = hex_str.chars().collect();
    for i in (0..chars.len()).step_by(2) {
        if i + 1 < chars.len() {
            let hex_pair: String = chars[i..=i + 1].iter().collect();
            if let Ok(b) = u8::from_str_radix(&hex_pair, 16) {
                bytes.push(b ^ 0x5F);
            }
        }
    }
    String::from_utf8(bytes).ok()
}

fn get_active_key_index(count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if let Some(conn) = get_db_conn() {
        if let Ok(val) = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'active_key_index'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            if let Ok(idx) = val.trim().parse::<usize>() {
                return idx % count;
            }
        }
    }
    0
}

fn set_active_key_index(index: usize) {
    if let Some(conn) = get_db_conn() {
        let _ = conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('active_key_index', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = ?1",
            [index.to_string()],
        );
    }
}

// â”€â”€ Config â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct AiConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
}

fn appdata_db_path() -> Option<std::path::PathBuf> {
    std::env::var("APPDATA").ok().map(|a| {
        std::path::PathBuf::from(a)
            .join("omnisearch-lite")
            .join("file_index.db")
    })
}

fn open_index_db(path: &Option<std::path::PathBuf>) -> rusqlite::Connection {
    let conn = path
        .as_ref()
        .and_then(|p| rusqlite::Connection::open(p).ok())
        .unwrap_or_else(|| rusqlite::Connection::open_in_memory().unwrap());
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
    conn
}

/// Pooled connection wrapper. Derefs to `Connection` so callers use it unchanged.
struct ConnCache {
    path: Option<std::path::PathBuf>,
    conn: rusqlite::Connection,
}
impl std::ops::Deref for ConnCache {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &rusqlite::Connection {
        &self.conn
    }
}

/// A single pooled SQLite connection (avoids opening 5-7 per AI request). The
/// connection is rebuilt only when the APPDATA-derived path changes.
fn get_db_conn() -> Option<std::sync::MutexGuard<'static, ConnCache>> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<ConnCache>> = OnceLock::new();
    let desired = appdata_db_path();
    let mutex = CACHE.get_or_init(|| {
        Mutex::new(ConnCache {
            path: desired.clone(),
            conn: open_index_db(&desired),
        })
    });
    let mut guard = mutex.lock().ok()?;
    if guard.path != desired {
        guard.conn = open_index_db(&desired);
        guard.path = desired;
    }
    Some(guard)
}

pub fn get_config() -> Result<AiConfig> {
    // Each lookup briefly locks the shared pooled connection and releases it.
    let db_get = |key: &str| -> Option<String> {
        let c = get_db_conn()?;
        let val = c
            .query_row(
                "SELECT value FROM ai_settings WHERE key = ?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .ok()?;
        let trimmed = val.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    };

    // 1. Resolve API key
    let mut api_key = None;
    let mut is_embedded = false;

    // Check SQLite settings table
    if let Some(val_trimmed) = db_get("api_key") {
        // The seeded shared key must not shadow embedded rotated keys
        if val_trimmed.starts_with("sk-HrvSzHIY") && get_embedded_keys_count() > 0 {
            let idx = get_active_key_index(get_embedded_keys_count());
            if let Some(k) = get_rotated_key(idx) {
                api_key = Some(k);
                is_embedded = true;
            } else {
                api_key = Some(val_trimmed);
            }
        } else {
            api_key = Some(val_trimmed);
        }
    }

    // Check Environment Variables
    if api_key.is_none() {
        if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
            if !k.trim().is_empty() {
                api_key = Some(k.trim().to_string());
            }
        }
    }
    if api_key.is_none() {
        if let Ok(k) = std::env::var("OPENSEARCH_AI_KEY") {
            if !k.trim().is_empty() {
                api_key = Some(k.trim().to_string());
            }
        }
    }
    if api_key.is_none() {
        if let Ok(k) = std::env::var("GEMINI_API_KEY") {
            if !k.trim().is_empty() {
                api_key = Some(k.trim().to_string());
            }
        }
    }
    if api_key.is_none() {
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            if !k.trim().is_empty() {
                api_key = Some(k.trim().to_string());
            }
        }
    }

    // Check AppData file
    if api_key.is_none() {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let p = std::path::Path::new(&appdata)
                .join("omnisearch-lite")
                .join("ai_key.txt");
            if let Ok(s) = std::fs::read_to_string(&p) {
                let k = s.trim().to_string();
                if !k.is_empty() {
                    api_key = Some(k);
                }
            }
        }
    }

    // Check Embedded Rotated Keys
    if api_key.is_none() {
        let count = get_embedded_keys_count();
        if count > 0 {
            let idx = get_active_key_index(count);
            if let Some(k) = get_rotated_key(idx) {
                api_key = Some(k);
                is_embedded = true;
            }
        }
    }

    // Check Hardcoded Key
    if api_key.is_none() && !HARDCODED_KEY.is_empty() {
        api_key = Some(HARDCODED_KEY.to_string());
    }

    // For providers that don't need a key (Ollama), allow empty key
    let endpoint_str = db_get("endpoint");
    let is_key_optional = endpoint_str
        .as_deref()
        .and_then(|ep| match_provider(ep))
        .map_or(false, |p| p.key_optional);

    let key = if is_key_optional {
        api_key.unwrap_or_default()
    } else {
        api_key.ok_or_else(|| anyhow!(
            "No AI API key configured. Go to Settings â†’ AI to pick a provider and paste your API key."
        ))?
    };

    // 2. Resolve Endpoint
    let mut endpoint = endpoint_str;

    // Check Environment Variable
    if endpoint.is_none() {
        if let Ok(ep) = std::env::var("OPENSEARCH_AI_ENDPOINT") {
            if !ep.trim().is_empty() {
                endpoint = Some(ep.trim().to_string());
            }
        }
    }

    // Check AppData file
    if endpoint.is_none() {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let p = std::path::Path::new(&appdata)
                .join("omnisearch-lite")
                .join("ai_endpoint.txt");
            if let Ok(s) = std::fs::read_to_string(&p) {
                let ep = s.trim().to_string();
                if !ep.is_empty() {
                    endpoint = Some(ep);
                }
            }
        }
    }

    // Fallback Default
    let endpoint = endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

    // 3. Resolve Model
    let mut model = db_get("model");

    // Check Environment Variable
    if model.is_none() {
        if let Ok(m) = std::env::var("OPENSEARCH_AI_MODEL") {
            if !m.trim().is_empty() {
                model = Some(m.trim().to_string());
            }
        }
    }

    // Check AppData file
    if model.is_none() {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let p = std::path::Path::new(&appdata)
                .join("omnisearch-lite")
                .join("ai_model.txt");
            if let Ok(s) = std::fs::read_to_string(&p) {
                let m = s.trim().to_string();
                if !m.is_empty() {
                    model = Some(m);
                }
            }
        }
    }

    // Fallback Default â€” use provider's default model if endpoint matches
    let model = model.unwrap_or_else(|| {
        if let Some(p) = match_provider(&endpoint) {
            p.default_model.to_string()
        } else {
            DEFAULT_MODEL.to_string()
        }
    });

    let _ = is_embedded; // used for key rotation logic

    Ok(AiConfig {
        endpoint,
        model,
        api_key: key,
    })
}

// â”€â”€ Completions â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Detect if the endpoint is Claude/Anthropic (uses a different request format).
fn is_anthropic_endpoint(endpoint: &str) -> bool {
    endpoint.contains("anthropic.com")
}

/// Build and send a chat completion request. Handles both OpenAI-compatible
/// and Anthropic (Claude) message formats.
fn send_completion(cfg: &AiConfig, messages: &[serde_json::Value]) -> Result<String> {
    let timeout_secs = 60;

    if is_anthropic_endpoint(&cfg.endpoint) {
        // Anthropic Claude uses a different format
        let system_msg = messages
            .iter()
            .find(|m| m["role"] == "system")
            .and_then(|m| m["content"].as_str())
            .unwrap_or("");
        let user_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m["role"] != "system")
            .cloned()
            .collect();

        let body = serde_json::json!({
            "model": cfg.model,
            "max_tokens": 4096,
            "system": system_msg,
            "messages": user_messages,
            "temperature": 0.3,
        });

        let resp = ureq::post(&cfg.endpoint)
            .set("x-api-key", &cfg.api_key)
            .set("anthropic-version", "2023-06-01")
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .send_json(body);

        match resp {
            Ok(r) => {
                let v: serde_json::Value =
                    r.into_json().map_err(|e| anyhow!("bad AI response: {e}"))?;
                let text = v["content"][0]["text"]
                    .as_str()
                    .ok_or_else(|| anyhow!("AI response had no content"))?;
                Ok(text.trim().to_string())
            }
            Err(ureq::Error::Status(code, r)) => {
                let msg = r.into_string().unwrap_or_default();
                Err(anyhow!(
                    "AI error {code}: {}",
                    msg.chars().take(300).collect::<String>()
                ))
            }
            Err(e) => Err(anyhow!("AI request failed: {e}")),
        }
    } else {
        // OpenAI-compatible (Gemini, OpenAI, Grok, DeepSeek, Ollama, etc.)
        let body = serde_json::json!({
            "model": cfg.model,
            "messages": messages,
            "stream": false,
            "temperature": 0.3,
        });

        let mut target_url = cfg.endpoint.trim().to_string();
        if !target_url.contains("messages")
            && !target_url.ends_with("chat/completions")
            && !target_url.ends_with("chat/completions/")
        {
            if !target_url.ends_with('/') {
                target_url.push('/');
            }
            target_url.push_str("chat/completions");
        }

        let resp = ureq::post(&target_url)
            .set("Authorization", &format!("Bearer {}", cfg.api_key))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .send_json(body);

        match resp {
            Ok(r) => {
                let v: serde_json::Value =
                    r.into_json().map_err(|e| anyhow!("bad AI response: {e}"))?;
                let text = v["choices"][0]["message"]["content"]
                    .as_str()
                    .ok_or_else(|| anyhow!("AI response had no content"))?;
                Ok(text.trim().to_string())
            }
            Err(ureq::Error::Status(code, r)) => {
                let msg = r.into_string().unwrap_or_default();
                Err(anyhow!(
                    "AI error {code}: {}",
                    msg.chars().take(300).collect::<String>()
                ))
            }
            Err(e) => Err(anyhow!("AI request failed: {e}")),
        }
    }
}

/// One-shot chat completion (non-streaming). Returns the assistant's text.
pub fn complete(system: &str, user: &str) -> Result<String> {
    let count = get_embedded_keys_count();
    let max_attempts = if count > 0 { count } else { 1 };

    for attempt in 0..max_attempts {
        let cfg = get_config()?;

        let messages = vec![
            serde_json::json!({ "role": "system", "content": system }),
            serde_json::json!({ "role": "user", "content": user }),
        ];

        match send_completion(&cfg, &messages) {
            Ok(text) => return Ok(text),
            Err(e) => {
                let err_str = e.to_string();
                let is_auth_or_quota = err_str.contains("401")
                    || err_str.contains("429")
                    || err_str.contains("402")
                    || err_str.contains("insufficient_quota")
                    || err_str.contains("quota")
                    || err_str.contains("balance");

                let is_using_embedded = count > 0 && {
                    let idx = get_active_key_index(count);
                    get_rotated_key(idx)
                        .map(|k| k == cfg.api_key)
                        .unwrap_or(false)
                };

                if is_auth_or_quota && is_using_embedded && attempt + 1 < max_attempts {
                    let next_idx = (get_active_key_index(count) + 1) % count;
                    set_active_key_index(next_idx);
                    continue;
                }

                return Err(e);
            }
        }
    }

    Err(anyhow!("All embedded API keys failed or were exhausted."))
}

/// Multi-turn chat completion. Passes conversation history to the API.
pub fn complete_chat(
    system: &str,
    prev_user: &str,
    prev_assistant: &str,
    user: &str,
) -> Result<String> {
    let count = get_embedded_keys_count();
    let max_attempts = if count > 0 { count } else { 1 };

    for attempt in 0..max_attempts {
        let cfg = get_config()?;

        let messages = vec![
            serde_json::json!({ "role": "system", "content": system }),
            serde_json::json!({ "role": "user", "content": prev_user }),
            serde_json::json!({ "role": "assistant", "content": prev_assistant }),
            serde_json::json!({ "role": "user", "content": user }),
        ];

        match send_completion(&cfg, &messages) {
            Ok(text) => return Ok(text),
            Err(e) => {
                let err_str = e.to_string();
                let is_auth_or_quota = err_str.contains("401")
                    || err_str.contains("429")
                    || err_str.contains("402")
                    || err_str.contains("insufficient_quota")
                    || err_str.contains("quota")
                    || err_str.contains("balance");

                let is_using_embedded = count > 0 && {
                    let idx = get_active_key_index(count);
                    get_rotated_key(idx)
                        .map(|k| k == cfg.api_key)
                        .unwrap_or(false)
                };

                if is_auth_or_quota && is_using_embedded && attempt + 1 < max_attempts {
                    let next_idx = (get_active_key_index(count) + 1) % count;
                    set_active_key_index(next_idx);
                    continue;
                }

                return Err(e);
            }
        }
    }

    Err(anyhow!("All embedded API keys failed or were exhausted."))
}

/// Simple one-shot agent completion (no streaming, no approval).
/// Simple wrapper around complete() for agent-style completions.
pub fn complete_agent(system: &str, user: &str) -> Result<String> {
    complete(system, user)
}

/// Multi-turn agent completion. Replaces the old complete_chat_agent.
pub fn complete_chat_agent(
    system: &str,
    prev_user: &str,
    prev_assistant: &str,
    user: &str,
) -> Result<String> {
    complete_chat(system, prev_user, prev_assistant, user)
}

// â”€â”€ URL fetching & HTMLâ†’text â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Fetch a URL and return a plain-text approximation of its readable content.
fn fetch_url_text(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .set("User-Agent", "Mozilla/5.0 (OmniSearch-Lite)")
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .map_err(|e| anyhow!("Couldn't fetch the page: {e}"))?;
    let html = resp
        .into_string()
        .map_err(|e| anyhow!("Couldn't read the page: {e}"))?;
    Ok(html_to_text(&html))
}

/// Crude HTMLâ†’text: drop script/style, strip tags, decode a few entities, collapse
/// whitespace. Good enough to summarize; not a real parser.
fn html_to_text(html: &str) -> String {
    let b = html.as_bytes();
    let n = b.len();
    let mut out = String::with_capacity(n / 2);
    let starts_ci =
        |i: usize, pat: &[u8]| i + pat.len() <= n && b[i..i + pat.len()].eq_ignore_ascii_case(pat);
    let find_ci = |from: usize, pat: &[u8]| -> Option<usize> {
        if pat.is_empty() || from >= n {
            return None;
        }
        (from..=n.saturating_sub(pat.len()))
            .find(|&j| b[j..j + pat.len()].eq_ignore_ascii_case(pat))
    };
    let mut i = 0;
    while i < n {
        if starts_ci(i, b"<script") || starts_ci(i, b"<style") {
            let close: &[u8] = if starts_ci(i, b"<script") {
                b"</script>"
            } else {
                b"</style>"
            };
            match find_ci(i, close) {
                Some(end) => {
                    i = end + close.len();
                    continue;
                }
                None => break,
            }
        }
        if b[i] == b'<' {
            match find_ci(i, b">") {
                Some(end) => {
                    i = end + 1;
                    out.push(' ');
                    continue;
                }
                None => break,
            }
        }
        let ch_len = match b[i] {
            x if x < 0x80 => 1,
            x if x < 0xE0 => 2,
            x if x < 0xF0 => 3,
            _ => 4,
        };
        let end = (i + ch_len).min(n);
        if let Ok(seg) = std::str::from_utf8(&b[i..end]) {
            out.push_str(seg);
        }
        i = end;
    }
    // Decode &amp; last, or "&amp;lt;" double-decodes into "<".
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// â”€â”€ Commands â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Commands: ask, explain, grammar, translate, summarize, bugs.
pub fn run(cmd: &str, input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(anyhow!(
            "Nothing to send â€” type text or copy something first."
        ));
    }
    // Summarize Webpage: if the input is a URL, fetch it and strip to text first.
    let owned_input: String;
    let input: &str =
        if cmd == "summarize" && (input.starts_with("http://") || input.starts_with("https://")) {
            let mut text = fetch_url_text(input)?;
            let mut end = 12000.min(text.len());
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            if text.trim().is_empty() {
                return Err(anyhow!("Couldn't extract readable text from that page."));
            }
            owned_input = text;
            &owned_input
        } else {
            input
        };
    let (system, user): (&str, String) = match cmd {
        "ask" | "chat" => (
            "You are a concise, helpful assistant. Answer directly in at most a few short paragraphs.",
            input.to_string(),
        ),
        "explain" => (
            "Explain the following clearly and simply for a general audience. Be concise.",
            input.to_string(),
        ),
        "grammar" => (
            "Fix the spelling and grammar of the text. Output ONLY the corrected text, with no preamble or quotes.",
            input.to_string(),
        ),
        "translate" => (
            "You are a translator. If the input names a target language (e.g. 'X to Spanish'), translate X into it; otherwise translate the text to English. Output ONLY the translation.",
            input.to_string(),
        ),
        "summarize" => (
            "Summarize the following text concisely as a few short bullet points.",
            input.to_string(),
        ),
        "bugs" => (
            "You are a code reviewer. List likely bugs and issues in the following code as short bullet points. Be specific.",
            input.to_string(),
        ),
        _ => (
            "You are a concise, helpful assistant.",
            input.to_string(),
        ),
    };
    complete(system, &user)
}

// â”€â”€ Tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_html_to_text() {
        let html = "<html><head><style>p{color:red}</style></head><body><p>Hello &amp; <b>world</b></p><script>alert(1)</script></body></html>";
        assert_eq!(super::html_to_text(html), "Hello & world");
    }

    #[test]
    fn test_config_resolution() {
        let _guard = TEST_LOCK.lock().unwrap();
        let old_appdata = std::env::var("APPDATA").ok();
        let temp_dir = std::env::temp_dir().join("omnisearch-lite-test-appdata");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        std::env::set_var("APPDATA", &temp_dir);

        // Clear environment variables that might interfere
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OPENSEARCH_AI_KEY");
        std::env::remove_var("OPENSEARCH_AI_ENDPOINT");
        std::env::remove_var("OPENSEARCH_AI_MODEL");
        std::env::remove_var("GEMINI_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");

        // Set DEEPSEEK_API_KEY
        std::env::set_var("DEEPSEEK_API_KEY", "sk-ds-test-key-12345");
        let cfg = get_config().unwrap();
        assert_eq!(cfg.api_key, "sk-ds-test-key-12345");
        assert_eq!(cfg.endpoint, "https://api.deepseek.com/chat/completions");
        assert_eq!(cfg.model, "deepseek-chat");

        // Cleanup
        std::env::remove_var("DEEPSEEK_API_KEY");

        // Restore APPDATA
        if let Some(val) = old_appdata {
            std::env::set_var("APPDATA", val);
        } else {
            std::env::remove_var("APPDATA");
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_provider_matching() {
        let p = match_provider("https://api.openai.com/v1/chat/completions");
        assert!(p.is_some());
        assert_eq!(p.unwrap().id, "openai");

        let p2 = match_provider(
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        );
        assert!(p2.is_some());
        assert_eq!(p2.unwrap().id, "gemini");

        let p3 = match_provider("https://custom.example.com/v1/chat");
        assert!(p3.is_none());
    }

    #[test]
    fn test_find_provider() {
        assert!(find_provider("gemini").is_some());
        assert!(find_provider("openai").is_some());
        assert!(find_provider("claude").is_some());
        assert!(find_provider("grok").is_some());
        assert!(find_provider("deepseek").is_some());
        assert!(find_provider("ollama").is_some());
        assert!(find_provider("nonexistent").is_none());
    }
}
