use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use qrcode::render::svg;
use qrcode::QrCode;
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use tungstenite::accept;
use tungstenite::protocol::Message;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

const SYNC_PORT: u16 = 9876;
const MAX_INBOUND_TEXT_BYTES: usize = 512 * 1024;
const MAX_PAIRING_ATTEMPTS_PER_WINDOW: u32 = 5;
const PAIRING_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
const PAIRING_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_DEVICE_ID_CHARS: usize = 96;
const MAX_DEVICE_NAME_CHARS: usize = 120;
const MAX_TRANSFER_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TRANSFER_CHUNK_BYTES: u64 = 192 * 1024;
const MAX_UPLOAD_NAME_CHARS: usize = 180;
const MAX_TRANSFER_ID_CHARS: usize = 128;
const MAX_CONTENT_TYPE_CHARS: usize = 128;
const UPLOAD_FOLDER_OPEN_DELAY: Duration = Duration::from_millis(2500);
const UPLOAD_FOLDER_OPEN_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub name: String,
    pub path: String,
    pub extension: String,
    pub size: u64,
    pub created_unix: i64,
    pub modified_unix: i64,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub created_unix: i64,
    pub modified_unix: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub group_id: String,
    pub size: u64,
    pub total_bytes: u64,
    pub file_count: u32,
    pub files: Vec<DuplicateFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub letter: String,
    pub path: String,
    pub filesystem: String,
    pub drive_type: String,
    pub is_ntfs: bool,
    pub can_open_volume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub indexing: bool,
    pub ready: bool,
    pub indexed_count: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileTransferManifest {
    pub id: String,
    pub name: String,
    pub path: String,
    pub extension: String,
    pub size: u64,
    pub created_unix: i64,
    pub modified_unix: i64,
    pub content_type: String,
    pub total_chunks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SyncMessage {
    PairRequest {
        token: String,
        device_id: String,
        device_name: String,
    },
    SearchQuery {
        query: String,
        extension: Option<String>,
        limit: u32,
    },
    RequestDrives,
    RequestIndexStatus,
    StartIndexing {
        drive: String,
    },
    RequestDuplicates {
        min_size: Option<u64>,
    },
    RemoteAction {
        action: String,
        path: String,
        recycle_bin: Option<bool>,
    },
    RequestFileContent {
        path: String,
    },
    RequestDesktopTransfer,
    RequestDesktopTransferChunk {
        transfer_id: String,
        chunk_index: u32,
    },
    FileContentResponse {
        path: String,
        content_type: String,
        data: String,
    },
    PhoneUploadManifest {
        id: String,
        name: String,
        size: u64,
        content_type: String,
        total_chunks: u32,
    },
    PhoneUploadChunk {
        transfer_id: String,
        chunk_index: u32,
        data: String,
        done: bool,
    },

    // Server Responses:
    PairingStatus {
        status: PairingStatusKind,
        message: String,
    },
    SearchResponse {
        results: Vec<SearchResult>,
    },
    DrivesResponse {
        drives: Vec<DriveInfo>,
    },
    IndexStatusResponse {
        status: IndexStatus,
    },
    DuplicatesResponse {
        groups: Vec<DuplicateGroup>,
    },
    Confirm {
        success: bool,
        message: String,
    },
    DesktopTransferManifest {
        transfer: Option<MobileTransferManifest>,
    },
    DesktopTransferChunk {
        transfer_id: String,
        chunk_index: u32,
        total_chunks: u32,
        bytes_sent: u64,
        total_bytes: u64,
        data: String,
        done: bool,
    },
    Ping,
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatusKind {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairingApproval {
    pub device_id: String,
    pub device_name: String,
    pub peer_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncServerInfo {
    pub running: bool,
    pub address: String,
    pub port: u16,
    pub qr_svg: String,
    pub connected_clients: usize,
    pub pairing_uri: String,
    pub pending_approvals: Vec<PendingPairingApproval>,
    pub file_transfers: Vec<MobileTransferSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileTransferSnapshot {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub bytes_sent: u64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct MobileTransfer {
    id: String,
    name: String,
    path: String,
    extension: String,
    size: u64,
    created_unix: i64,
    modified_unix: i64,
    content_type: String,
    total_chunks: u32,
    bytes_sent: u64,
    assigned_device_id: Option<String>,
    received_chunks: HashSet<u32>,
    status: MobileTransferStatus,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MobileTransferStatus {
    Queued,
    Sending,
    Receiving,
    Completed,
    Failed,
}

struct PairAttemptWindow {
    attempts: u32,
    window_started: Instant,
    blocked_until: Option<Instant>,
}

pub struct SyncState {
    pub client_count: Arc<Mutex<usize>>,
    pub server_running: Arc<Mutex<bool>>,
    pub local_ip: Arc<Mutex<String>>,
    pub stop_requested: Arc<AtomicBool>,
    pub pairing_token: Arc<Mutex<String>>,
    pub pending_approvals: Arc<Mutex<HashMap<String, PendingPairingApproval>>>,
    pub approved_devices: Arc<Mutex<HashSet<String>>>,
    pub rejected_devices: Arc<Mutex<HashSet<String>>>,
    file_transfers: Arc<Mutex<Vec<MobileTransfer>>>,
    pairing_attempts: Arc<Mutex<HashMap<String, PairAttemptWindow>>>,
    last_explorer_open: Arc<Mutex<Option<Instant>>>,
    pending_explorer_open: Arc<Mutex<bool>>,
    pub tls_config: Arc<Mutex<Option<Arc<rustls::ServerConfig>>>>,
    pub cert_fingerprint: Arc<Mutex<String>>,
    pub active_port: Arc<Mutex<u16>>,
}

impl SyncState {
    pub fn new() -> Self {
        Self {
            client_count: Arc::new(Mutex::new(0)),
            server_running: Arc::new(Mutex::new(false)),
            local_ip: Arc::new(Mutex::new(String::new())),
            stop_requested: Arc::new(AtomicBool::new(false)),
            pairing_token: Arc::new(Mutex::new(String::new())),
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
            approved_devices: Arc::new(Mutex::new(HashSet::new())),
            rejected_devices: Arc::new(Mutex::new(HashSet::new())),
            file_transfers: Arc::new(Mutex::new(Vec::new())),
            pairing_attempts: Arc::new(Mutex::new(HashMap::new())),
            last_explorer_open: Arc::new(Mutex::new(None)),
            pending_explorer_open: Arc::new(Mutex::new(false)),
            tls_config: Arc::new(Mutex::new(None)),
            cert_fingerprint: Arc::new(Mutex::new(String::new())),
            active_port: Arc::new(Mutex::new(SYNC_PORT)),
        }
    }
}

pub static GLOBAL_SYNC_STATE: once_cell::sync::Lazy<Arc<SyncState>> =
    once_cell::sync::Lazy::new(|| Arc::new(SyncState::new()));

fn generate_pairing_token() -> String {
    let mut bytes = [0u8; 18];
    let mut rng = rand_core::OsRng;
    rng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn build_pairing_uri(address: &str, port: u16, token: &str, cert: &str) -> String {
    if address.trim().is_empty() || token.trim().is_empty() {
        String::new()
    } else if cert.trim().is_empty() {
        format!("omnisearch://{}:{port}?token={token}", address.trim())
    } else {
        format!(
            "omnisearch://{}:{port}?token={token}&cert={cert}",
            address.trim()
        )
    }
}

pub fn generate_qr_svg(uri: &str) -> String {
    if uri.trim().is_empty() {
        return String::new();
    }
    match QrCode::new(uri.as_bytes()) {
        Ok(qr) => qr.render::<svg::Color>().build(),
        Err(_) => String::new(),
    }
}

pub fn get_local_ip() -> String {
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

fn pairing_response(status: PairingStatusKind, message: impl Into<String>) -> SyncMessage {
    SyncMessage::PairingStatus {
        status,
        message: message.into(),
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn peer_rate_limit_key(peer_address: &str) -> String {
    peer_address
        .split(':')
        .next()
        .unwrap_or(peer_address)
        .to_string()
}

fn check_pairing_rate_limit(sync_state: &SyncState, peer_address: &str) -> Result<(), String> {
    let key = peer_rate_limit_key(peer_address);
    let now = Instant::now();
    let mut attempts = sync_state
        .pairing_attempts
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let window = attempts.entry(key).or_insert_with(|| PairAttemptWindow {
        attempts: 0,
        window_started: now,
        blocked_until: None,
    });

    if let Some(blocked_until) = window.blocked_until {
        if now < blocked_until {
            let remaining = (blocked_until - now).as_secs().max(1);
            return Err(format!(
                "Too many pairing attempts. Please wait {remaining} seconds."
            ));
        }
        window.blocked_until = None;
        window.attempts = 0;
        window.window_started = now;
    }

    if now.duration_since(window.window_started) > PAIRING_ATTEMPT_WINDOW {
        window.attempts = 0;
        window.window_started = now;
    }

    window.attempts += 1;
    if window.attempts > MAX_PAIRING_ATTEMPTS_PER_WINDOW {
        window.blocked_until = Some(now + PAIRING_COOLDOWN);
        return Err("Too many pairing attempts. Please wait 30 seconds.".to_string());
    }

    Ok(())
}

fn is_known_pairing_device(sync_state: &SyncState, device_id: &str) -> bool {
    sync_state
        .approved_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(device_id)
        || sync_state
            .rejected_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(device_id)
}

fn begin_pairing(
    sync_state: &SyncState,
    token: &str,
    device_id: &str,
    device_name: &str,
    peer_address: &str,
) -> (PairingStatusKind, String) {
    if device_id.trim().is_empty() {
        return (PairingStatusKind::Rejected, "Invalid device ID.".to_string());
    }

    {
        let approved = sync_state
            .approved_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if approved.contains(device_id) {
            return (
                PairingStatusKind::Approved,
                "Device approved for sync.".to_string(),
            );
        }
    }

    {
        let rejected = sync_state
            .rejected_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if rejected.contains(device_id) {
            return (
                PairingStatusKind::Rejected,
                "Device pairing rejected by desktop.".to_string(),
            );
        }
    }

    let active_token = sync_state
        .pairing_token
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if active_token.is_empty() || token != active_token {
        return (
            PairingStatusKind::Rejected,
            "Invalid pairing token.".to_string(),
        );
    }

    let pending = PendingPairingApproval {
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
        peer_address: peer_address.to_string(),
    };

    sync_state
        .pending_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(device_id.to_string(), pending);

    (
        PairingStatusKind::Pending,
        "Pairing request received. Please approve on desktop.".to_string(),
    )
}

pub fn approve_pairing_request(sync_state: Arc<SyncState>, device_id: &str) -> Result<(), String> {
    let mut pending = sync_state
        .pending_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if pending.remove(device_id).is_none() {
        return Err("Pairing request not found".to_string());
    }
    sync_state
        .rejected_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
    sync_state
        .approved_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(device_id.to_string());
    Ok(())
}

pub fn reject_pairing_request(sync_state: Arc<SyncState>, device_id: &str) -> Result<(), String> {
    let mut pending = sync_state
        .pending_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    pending.remove(device_id);
    sync_state
        .approved_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
    sync_state
        .rejected_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(device_id.to_string());
    Ok(())
}

fn unix_time(metadata_time: std::io::Result<std::time::SystemTime>) -> i64 {
    metadata_time
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn detect_content_type(path: &Path) -> String {
    let extension = path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    let content_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "3gp" => "video/3gpp",
        "mov" => "video/quicktime",
        "txt" | "md" | "log" | "rs" | "py" | "java" | "js" | "ts" | "kt" | "cpp" | "h"
        | "html" | "css" | "json" | "xml" | "yaml" | "yml" | "ini" | "toml" | "csv"
        | "c" | "go" | "rb" | "sh" | "bat" | "ps1" | "cfg" | "conf" => "text/plain",
        _ => "application/octet-stream",
    };

    content_type.to_string()
}

fn get_downloads_dir() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let mut p = PathBuf::from(profile);
        p.push("Downloads");
        if p.exists() {
            return p;
        }
    }
    std::env::temp_dir()
}

fn upload_folder_name(content_type: &str) -> &'static str {
    let normalized = content_type.trim().to_ascii_lowercase();
    if normalized.starts_with("image/") {
        "Images"
    } else if normalized.starts_with("video/") {
        "Videos"
    } else if normalized.starts_with("audio/") {
        "Audio"
    } else if normalized == "application/pdf"
        || normalized.starts_with("text/")
        || normalized.contains("document")
        || normalized.contains("spreadsheet")
        || normalized.contains("presentation")
    {
        "Documents"
    } else if normalized.contains("zip")
        || normalized.contains("rar")
        || normalized.contains("7z")
        || normalized.contains("tar")
        || normalized.contains("gzip")
    {
        "Archives"
    } else if normalized == "application/vnd.android.package-archive" {
        "Apps"
    } else {
        "Files"
    }
}

fn get_destination_dir(content_type: &str) -> PathBuf {
    let mut dir = get_downloads_dir();
    dir.push("OmniSearch");
    dir.push(upload_folder_name(content_type));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn sanitize_upload_filename(name: &str) -> String {
    let raw_name = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .trim();
    let mut cleaned = raw_name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .chars()
        .take(MAX_UPLOAD_NAME_CHARS)
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_string();

    if cleaned.is_empty() {
        cleaned = "uploaded_file".to_string();
    }

    let stem = Path::new(&cleaned)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(&cleaned)
        .trim_end_matches('.');
    let stem_upper = stem.to_ascii_uppercase();
    let reserved = matches!(
        stem_upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$"
    ) || (stem_upper.len() == 4
        && (stem_upper.starts_with("COM") || stem_upper.starts_with("LPT"))
        && stem_upper.as_bytes()[3].is_ascii_digit()
        && stem_upper.as_bytes()[3] != b'0');

    if reserved {
        cleaned.insert(0, '_');
    }

    cleaned
}

fn get_unique_path(dir: &Path, name: &str) -> PathBuf {
    let sanitized_name = sanitize_upload_filename(name);
    let mut path = dir.join(&sanitized_name);
    if !path.exists() {
        return path;
    }
    let stem = Path::new(&sanitized_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&sanitized_name);
    let ext = Path::new(&sanitized_name).extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut counter = 1;
    loop {
        let new_name = if ext.is_empty() {
            format!("{} ({})", stem, counter)
        } else {
            format!("{} ({}).{}", stem, counter, ext)
        };
        path = dir.join(new_name);
        if !path.exists() {
            return path;
        }
        counter += 1;
    }
}

fn expected_transfer_chunks(size: u64) -> u32 {
    ((size + TRANSFER_CHUNK_BYTES - 1) / TRANSFER_CHUNK_BYTES)
        .max(1)
        .min(u32::MAX as u64) as u32
}

fn valid_transfer_id(id: &str) -> bool {
    let trimmed = id.trim();
    !trimmed.is_empty()
        && trimmed.len() <= MAX_TRANSFER_ID_CHARS
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn transfer_id() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("lt-{}", nanos)
}

fn transfer_manifest(transfer: &MobileTransfer) -> MobileTransferManifest {
    MobileTransferManifest {
        id: transfer.id.clone(),
        name: transfer.name.clone(),
        path: transfer.path.clone(),
        extension: transfer.extension.clone(),
        size: transfer.size,
        created_unix: transfer.created_unix,
        modified_unix: transfer.modified_unix,
        content_type: transfer.content_type.clone(),
        total_chunks: transfer.total_chunks,
    }
}

fn schedule_upload_folder_open(sync_state: &SyncState, folder: PathBuf) {
    let now = Instant::now();
    if let Ok(last_open) = sync_state.last_explorer_open.lock() {
        if last_open
            .as_ref()
            .map(|last| now.duration_since(*last) < UPLOAD_FOLDER_OPEN_COOLDOWN)
            .unwrap_or(false)
        {
            return;
        }
    }

    if let Ok(mut pending) = sync_state.pending_explorer_open.lock() {
        if *pending {
            return;
        }
        *pending = true;
    } else {
        return;
    }

    let last_explorer_open = Arc::clone(&sync_state.last_explorer_open);
    let pending_explorer_open = Arc::clone(&sync_state.pending_explorer_open);
    thread::spawn(move || {
        thread::sleep(UPLOAD_FOLDER_OPEN_DELAY);
        let opened = std::process::Command::new("explorer.exe")
            .arg(folder.as_os_str())
            .spawn()
            .is_ok();

        if opened {
            if let Ok(mut last_open) = last_explorer_open.lock() {
                *last_open = Some(Instant::now());
            }
        }
        if let Ok(mut pending) = pending_explorer_open.lock() {
            *pending = false;
        }
    });
}

fn next_transfer_for_device(
    sync_state: &SyncState,
    device_id: &str,
) -> Option<MobileTransferManifest> {
    let mut transfers = sync_state
        .file_transfers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let transfer = transfers.iter_mut().find(|transfer| {
        matches!(transfer.status, MobileTransferStatus::Queued)
            && transfer
                .assigned_device_id
                .as_deref()
                .map(|assigned| assigned == device_id)
                .unwrap_or(true)
    })?;

    transfer.assigned_device_id = Some(device_id.to_string());
    transfer.status = MobileTransferStatus::Sending;
    Some(transfer_manifest(transfer))
}

fn read_transfer_chunk(
    sync_state: &SyncState,
    device_id: &str,
    transfer_id: &str,
    chunk_index: u32,
) -> SyncMessage {
    use base64::prelude::BASE64_STANDARD;
    use base64::Engine as _;

    let transfer = {
        let transfers = sync_state
            .file_transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        transfers
            .iter()
            .find(|transfer| {
                transfer.id == transfer_id
                    && transfer.assigned_device_id.as_deref() == Some(device_id)
                    && transfer.status == MobileTransferStatus::Sending
            })
            .cloned()
    };

    let Some(transfer) = transfer else {
        return SyncMessage::Confirm {
            success: false,
            message: "That mobile transfer is no longer available.".to_string(),
        };
    };

    if chunk_index >= transfer.total_chunks {
        return SyncMessage::Confirm {
            success: false,
            message: "Requested transfer chunk is out of range.".to_string(),
        };
    }

    let offset = chunk_index as u64 * TRANSFER_CHUNK_BYTES;
    let remaining = transfer.size.saturating_sub(offset);
    let bytes_to_read = remaining.min(TRANSFER_CHUNK_BYTES) as usize;
    let mut buffer = vec![0u8; bytes_to_read];

    let read_result = std::fs::File::open(&transfer.path)
        .and_then(|mut file| {
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut buffer)?;
            Ok(())
        });

    if let Err(error) = read_result {
        let mut transfers = sync_state
            .file_transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(current) = transfers.iter_mut().find(|current| current.id == transfer_id) {
            current.status = MobileTransferStatus::Failed;
            current.error = Some(format!("Read failed: {error}"));
        }
        return SyncMessage::Confirm {
            success: false,
            message: format!("Could not read transfer chunk: {error}"),
        };
    }

    let bytes_sent = (offset + bytes_to_read as u64).min(transfer.size);
    let done = chunk_index + 1 >= transfer.total_chunks;
    {
        let mut transfers = sync_state
            .file_transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(current) = transfers.iter_mut().find(|current| current.id == transfer_id) {
            current.bytes_sent = bytes_sent;
            if done {
                current.status = MobileTransferStatus::Completed;
            }
        }
    }

    SyncMessage::DesktopTransferChunk {
        transfer_id: transfer_id.to_string(),
        chunk_index,
        total_chunks: transfer.total_chunks,
        bytes_sent,
        total_bytes: transfer.size,
        data: BASE64_STANDARD.encode(buffer),
        done,
    }
}

fn process_phone_upload_chunk(
    sync_state: &SyncState,
    device_id: &str,
    transfer_id: &str,
    chunk_index: u32,
    base64_data: &str,
    done: bool,
) -> Result<(), String> {
    let result =
        process_phone_upload_chunk_impl(sync_state, device_id, transfer_id, chunk_index, base64_data, done);
    if let Err(ref err_msg) = result {
        if let Ok(mut transfers) = sync_state.file_transfers.lock() {
            if let Some(t) = transfers.iter_mut().find(|t| t.id == transfer_id) {
                t.status = MobileTransferStatus::Failed;
                t.error = Some(err_msg.clone());
            }
        }
    }
    result
}

fn process_phone_upload_chunk_impl(
    sync_state: &SyncState,
    device_id: &str,
    transfer_id: &str,
    chunk_index: u32,
    base64_data: &str,
    done: bool,
) -> Result<(), String> {
    use std::io::Write;

    let (path, size, total_chunks) = {
        let transfers = sync_state.file_transfers.lock()
            .map_err(|_| "Failed to lock transfers state".to_string())?;

        let t = transfers.iter().find(|t| t.id == transfer_id)
            .ok_or_else(|| format!("No transfer found with ID {transfer_id}"))?;

        if t.assigned_device_id.as_deref() != Some(device_id) {
            return Err("Upload belongs to a different approved device.".to_string());
        }
        if t.status != MobileTransferStatus::Receiving {
            return Err("Upload is not accepting chunks.".to_string());
        }
        if t.received_chunks.contains(&chunk_index) {
            return Err(format!("Chunk {chunk_index} was already received."));
        }
        if chunk_index != t.received_chunks.len() as u32 {
            return Err("Upload chunks must arrive in order.".to_string());
        }

        (t.path.clone(), t.size, t.total_chunks)
    };

    if chunk_index >= total_chunks {
        return Err(format!("Chunk index {chunk_index} out of bounds (total: {total_chunks})"));
    }
    let is_last_chunk = chunk_index + 1 == total_chunks;
    if done != is_last_chunk {
        return Err("Upload completion flag does not match the chunk index.".to_string());
    }

    let decoded_bytes = base64::engine::general_purpose::STANDARD.decode(base64_data)
        .map_err(|e| format!("Failed to decode base64: {e}"))?;
    let bytes_written = decoded_bytes.len() as u64;
    if bytes_written > TRANSFER_CHUNK_BYTES {
        return Err("Upload chunk is larger than the allowed chunk size.".to_string());
    }

    let offset = chunk_index as u64 * TRANSFER_CHUNK_BYTES;
    let expected_bytes = if size == 0 {
        0
    } else {
        size.saturating_sub(offset).min(TRANSFER_CHUNK_BYTES)
    };
    if bytes_written != expected_bytes {
        return Err(format!(
            "Upload chunk has wrong size. Expected {expected_bytes} bytes, got {bytes_written}."
        ));
    }
    if offset + bytes_written > size {
        return Err("Upload chunk exceeds the declared file size.".to_string());
    }

    // Open the file and write the chunk
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|e| format!("Failed to open destination file: {e}"))?;

    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("Seek failed: {e}"))?;

    file.write_all(&decoded_bytes)
        .map_err(|e| format!("Write failed: {e}"))?;

    // Update progress
    let mut completed_folder = None::<PathBuf>;
    let mut transfers = sync_state.file_transfers.lock()
        .map_err(|_| "Failed to lock transfers state".to_string())?;

    if let Some(t) = transfers.iter_mut().find(|t| t.id == transfer_id) {
        t.received_chunks.insert(chunk_index);
        t.bytes_sent = offset + bytes_written;
        if done {
            if t.bytes_sent != t.size || t.received_chunks.len() as u32 != t.total_chunks {
                return Err("Upload finished before all declared bytes were received.".to_string());
            }
            t.status = MobileTransferStatus::Completed;
            completed_folder = Path::new(&t.path).parent().map(Path::to_path_buf);
        }
    }
    drop(transfers);

    if let Some(folder) = completed_folder {
        schedule_upload_folder_open(sync_state, folder);
    }

    Ok(())
}

pub fn open_downloads_folder() {
    let dir = get_downloads_dir();
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

pub fn queue_file_transfer(
    sync_state: &SyncState,
    path: &Path,
) -> Result<MobileTransferSnapshot, String> {
    let connected_clients = *sync_state
        .client_count
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if connected_clients == 0 {
        return Err("Connect a phone before sending files.".to_string());
    }

    let canonical = path.to_path_buf();
    let metadata = std::fs::metadata(&canonical)
        .map_err(|err| format!("Could not inspect file: {err}"))?;
    if metadata.is_dir() {
        return Err("Folders cannot be sent to phone yet.".to_string());
    }
    if metadata.len() > MAX_TRANSFER_BYTES {
        return Err("Mobile transfer is limited to 2 GB per file.".to_string());
    }

    let name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let extension = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_string();
    let total_chunks = ((metadata.len() + TRANSFER_CHUNK_BYTES - 1) / TRANSFER_CHUNK_BYTES)
        .max(1)
        .min(u32::MAX as u64) as u32;

    let transfer = MobileTransfer {
        id: transfer_id(),
        name: name.clone(),
        path: canonical.to_string_lossy().into_owned(),
        extension,
        size: metadata.len(),
        created_unix: unix_time(metadata.created()),
        modified_unix: unix_time(metadata.modified()),
        content_type: detect_content_type(&canonical),
        total_chunks,
        bytes_sent: 0,
        assigned_device_id: None,
        received_chunks: HashSet::new(),
        status: MobileTransferStatus::Queued,
        error: None,
    };

    let snapshot = MobileTransferSnapshot {
        id: transfer.id.clone(),
        name: transfer.name.clone(),
        path: transfer.path.clone(),
        size: transfer.size,
        bytes_sent: transfer.bytes_sent,
        status: transfer_status_label(&transfer.status).to_string(),
        error: None,
    };

    let mut transfers = sync_state
        .file_transfers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    transfers.push(transfer);
    if transfers.len() > 20 {
        let excess = transfers.len() - 20;
        transfers.drain(0..excess);
    }

    Ok(snapshot)
}

pub fn pick_and_send_files_to_phone(sync_state: Arc<SyncState>) {
    thread::spawn(move || {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("Select files to send to Phone")
            .pick_files()
        {
            for file in files {
                let _ = queue_file_transfer(&sync_state, &file);
            }
        }
    });
}

fn log_sync(msg: &str) {
    eprintln!("[OmniSearch Sync] {}", msg);
    let log_dir = match std::env::var("APPDATA") {
        Ok(d) => std::path::PathBuf::from(d).join("omnisearch-lite"),
        Err(_) => std::path::PathBuf::from("."),
    };
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("settings_ui.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        use std::io::Write;
        let _ = writeln!(file, "[OmniSearch Sync] {}", msg);
    }
}

pub fn start_sync_server(sync_state: Arc<SyncState>) {
    log_sync("start_sync_server called from UI");
    let client_count = Arc::clone(&sync_state.client_count);
    let server_running = Arc::clone(&sync_state.server_running);
    let local_ip_state = Arc::clone(&sync_state.local_ip);
    let stop_requested = Arc::clone(&sync_state.stop_requested);

    stop_requested.store(false, Ordering::SeqCst);
    {
        let mut token = sync_state
            .pairing_token
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *token = generate_pairing_token();
    }
    sync_state
        .pending_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    sync_state
        .approved_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    sync_state
        .rejected_devices
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    sync_state
        .pairing_attempts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    sync_state
        .file_transfers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    {
        let mut count = client_count.lock().unwrap_or_else(|e| e.into_inner());
        *count = 0;
    }
    {
        let mut fp = sync_state.cert_fingerprint.lock().unwrap_or_else(|e| e.into_inner());
        fp.clear();
    }
    {
        let mut tls = sync_state.tls_config.lock().unwrap_or_else(|e| e.into_inner());
        *tls = None;
    }

    thread::spawn(move || {
        let result = std::panic::catch_unwind(move || {
            let local_ip = get_local_ip();

            {
                let mut ip = local_ip_state.lock().unwrap_or_else(|e| e.into_inner());
                *ip = local_ip.clone();
            }

            // Install ring as the default crypto provider for rustls
            let _ = rustls::crypto::ring::default_provider().install_default();

            // Generate self-signed certificate for secure WSS connection
            let subject_alt_names = vec!["127.0.0.1".to_string(), "localhost".to_string(), local_ip.clone()];
            if let Ok(cert) = rcgen::generate_simple_self_signed(subject_alt_names) {
                if let Ok(cert_der) = cert.serialize_der() {
                    let key_der = cert.serialize_private_key_der();
                    
                    let mut hasher = Sha256::new();
                    hasher.update(&cert_der);
                    let hash = hasher.finalize();
                    let fingerprint = hash.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":");
                    
                    let rustls_cert = CertificateDer::from(cert_der);
                    let rustls_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
                    if let Ok(mut config) = rustls::ServerConfig::builder()
                        .with_no_client_auth()
                        .with_single_cert(vec![rustls_cert], rustls_key) {
                            config.alpn_protocols = vec![b"http/1.1".to_vec()];
                            if let Ok(mut config_lock) = sync_state.tls_config.lock() {
                                *config_lock = Some(Arc::new(config));
                            }
                            if let Ok(mut fingerprint_lock) = sync_state.cert_fingerprint.lock() {
                                *fingerprint_lock = fingerprint;
                            }
                        }
                }
            }


            let mut bound_listener = None;
            let mut bound_port = SYNC_PORT;

            for candidate_offset in 0..20 {
                let candidate_port = SYNC_PORT + candidate_offset;
                let bind_addr = format!("0.0.0.0:{candidate_port}");
                match TcpListener::bind(&bind_addr) {
                    Ok(l) => {
                        bound_listener = Some(l);
                        bound_port = candidate_port;
                        break;
                    }
                    Err(e) => {
                        log_sync(&format!("Port {candidate_port} busy ({e}), trying next..."));
                    }
                }
            }

            let listener = match bound_listener {
                Some(l) => l,
                None => {
                    log_sync(&format!("Failed to bind any port in range {SYNC_PORT}..{}", SYNC_PORT + 20));
                    return;
                }
            };

            if let Ok(mut p) = sync_state.active_port.lock() {
                *p = bound_port;
            }

            listener
                .set_nonblocking(true)
                .expect("Cannot set non-blocking");

            {
                let mut running = server_running.lock().unwrap_or_else(|e| e.into_inner());
                *running = true;
            }

            log_sync(&format!("Server started successfully at wss://{local_ip}:{bound_port}"));

            while !stop_requested.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let sync_state = Arc::clone(&sync_state);
                        thread::spawn(move || {
                            handle_client(stream, sync_state);
                        });
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        eprintln!("[OmniSearch Sync] Accept error: {e}");
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }

            {
                let mut running = server_running.lock().unwrap_or_else(|e| e.into_inner());
                *running = false;
            }
            eprintln!("[OmniSearch Sync] Server stopped.");
        });

        if let Err(e) = result {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };
            let log_msg = format!("[OmniSearch Sync] Server thread panicked: {}", msg);
            eprintln!("{}", log_msg);
            let log_dir = match std::env::var("APPDATA") {
                Ok(d) => std::path::PathBuf::from(d).join("omnisearch-lite"),
                Err(_) => std::path::PathBuf::from("."),
            };
            let _ = std::fs::create_dir_all(&log_dir);
            let log_path = log_dir.join("settings_ui.log");
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
                use std::io::Write;
                let _ = writeln!(file, "{}", log_msg);
            }
        }
    });
}

pub fn stop_sync_server(sync_state: Arc<SyncState>) {
    sync_state.stop_requested.store(true, Ordering::SeqCst);
    if let Ok(mut fp) = sync_state.cert_fingerprint.lock() {
        fp.clear();
    }
    if let Ok(mut tls) = sync_state.tls_config.lock() {
        *tls = None;
    }
}

pub fn get_sync_server_info(sync_state: Arc<SyncState>) -> SyncServerInfo {
    let running = *sync_state
        .server_running
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut address = sync_state
        .local_ip
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if address.is_empty() {
        address = get_local_ip();
    }
    let port = *sync_state
        .active_port
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let token = sync_state
        .pairing_token
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let fingerprint = sync_state
        .cert_fingerprint
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let pairing_uri = build_pairing_uri(&address, port, &token, &fingerprint);
    let qr_svg = generate_qr_svg(&pairing_uri);
    let connected_clients = *sync_state
        .client_count
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let pending_approvals = sync_state
        .pending_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();

    let file_transfers = transfer_snapshots(sync_state.as_ref());

    SyncServerInfo {
        running,
        address,
        port,
        qr_svg,
        connected_clients,
        pairing_uri,
        pending_approvals,
        file_transfers,
    }
}

pub fn transfer_snapshots(sync_state: &SyncState) -> Vec<MobileTransferSnapshot> {
    let transfers = sync_state
        .file_transfers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    transfers
        .iter()
        .map(|t| MobileTransferSnapshot {
            id: t.id.clone(),
            name: t.name.clone(),
            path: t.path.clone(),
            size: t.size,
            bytes_sent: t.bytes_sent,
            status: transfer_status_label(&t.status).to_string(),
            error: t.error.clone(),
        })
        .collect()
}

fn transfer_status_label(status: &MobileTransferStatus) -> &'static str {
    match status {
        MobileTransferStatus::Queued => "Queued",
        MobileTransferStatus::Sending => "Sending",
        MobileTransferStatus::Receiving => "Receiving",
        MobileTransferStatus::Completed => "Completed",
        MobileTransferStatus::Failed => "Failed",
    }
}

fn handle_client(stream: TcpStream, sync_state: Arc<SyncState>) {
    let _ = stream.set_nonblocking(false);
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    log_sync(&format!("Client connected: {peer}"));

    let tls_config_opt = sync_state.tls_config.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let tls_config = match tls_config_opt {
        Some(config) => config,
        None => {
            log_sync("TLS configuration missing.");
            return;
        }
    };

    let conn = match rustls::ServerConnection::new(tls_config) {
        Ok(c) => c,
        Err(e) => {
            log_sync(&format!("Failed to create TLS connection for {peer}: {e}"));
            return;
        }
    };
    
    let tls_stream = rustls::StreamOwned::new(conn, stream);

    let mut websocket = match accept(tls_stream) {
        Ok(ws) => {
            log_sync(&format!("WebSocket handshake succeeded for {peer}"));
            ws
        }
        Err(e) => {
            log_sync(&format!("WebSocket handshake failed for {peer}: {e}"));
            return;
        }
    };

    let _ = websocket.get_mut().sock.set_read_timeout(Some(Duration::from_millis(250)));

    let mut paired_device_id = None::<String>;
    let mut approved_device_id = None::<String>;
    let mut counted_client = false;

    loop {
        if sync_state.stop_requested.load(Ordering::SeqCst) {
            let _ = websocket.close(None);
            break;
        }

        let message = match websocket.read() {
            Ok(msg) => msg,
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(error) => {
                log_sync(&format!("Client {peer} disconnected during read: {error}"));
                break;
            }
        };

        match message {
            Message::Text(text) => {
                if text.len() > MAX_INBOUND_TEXT_BYTES {
                    let response = SyncMessage::Confirm {
                        success: false,
                        message: "Message rejected because it is too large.".to_string(),
                    };
                    if let Ok(response_json) = serde_json::to_string(&response) {
                        let _ = websocket.send(Message::Text(response_json.into()));
                    }
                    continue;
                }

                let response = handle_sync_message(
                    &text,
                    &peer,
                    sync_state.as_ref(),
                    &mut paired_device_id,
                    &mut approved_device_id,
                    &mut counted_client,
                );

                if let Some(response_json) = response {
                    if websocket.send(Message::Text(response_json.into())).is_err() {
                        log_sync(&format!("Failed to send response to {peer}"));
                        break;
                    }
                }
            }
            Message::Close(_) => {
                log_sync(&format!("Client {peer} closed connection"));
                let _ = websocket.close(None);
                break;
            }
            Message::Ping(data) => {
                let _ = websocket.send(Message::Pong(data));
            }
            _ => {}
        }
    }

    if !counted_client {
        if let Some(device_id) = paired_device_id.as_deref() {
            sync_state
                .pending_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(device_id);
        }
    } else {
        let mut count = sync_state
            .client_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
    }

    if let Some(ref device_id) = approved_device_id {
        if let Ok(mut transfers) = sync_state.file_transfers.lock() {
            for t in transfers.iter_mut() {
                if t.assigned_device_id.as_deref() == Some(device_id) && t.status == MobileTransferStatus::Sending {
                    t.status = MobileTransferStatus::Queued;
                }
            }
        }
    }

    log_sync(&format!("Client disconnected: {peer}"));
}

fn handle_sync_message(
    text: &str,
    peer_address: &str,
    sync_state: &SyncState,
    paired_device_id: &mut Option<String>,
    approved_device_id: &mut Option<String>,
    counted_client: &mut bool,
) -> Option<String> {
    log_sync(&format!("handle_sync_message from {peer_address}: {text}"));
    let incoming: SyncMessage = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(error) => {
            log_sync(&format!("JSON deserialize error: {error}"));
            let error_response = SyncMessage::Confirm {
                success: false,
                message: format!("Invalid message format: {error}"),
            };
            return serde_json::to_string(&error_response).ok();
        }
    };

    let response = match incoming {
        SyncMessage::Ping => SyncMessage::Pong,
        SyncMessage::PairRequest {
            token,
            device_id,
            device_name,
        } => {
            let normalized_device_id = truncate_chars(&device_id, MAX_DEVICE_ID_CHARS);
            let device_name = truncate_chars(&device_name, MAX_DEVICE_NAME_CHARS);
            if !is_known_pairing_device(sync_state, &normalized_device_id) {
                if let Err(message) = check_pairing_rate_limit(sync_state, peer_address) {
                    return serde_json::to_string(&pairing_response(
                        PairingStatusKind::Rejected,
                        message,
                    ))
                    .ok();
                }
            }

            let (status, message) = begin_pairing(
                sync_state,
                &token,
                &normalized_device_id,
                &device_name,
                peer_address,
            );
            *paired_device_id = Some(normalized_device_id.clone());

            if status == PairingStatusKind::Approved {
                *approved_device_id = Some(normalized_device_id);
                if !*counted_client {
                    let mut count = sync_state
                        .client_count
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *count += 1;
                    *counted_client = true;
                }
            }

            pairing_response(status, message)
        }

        // === Features NOT available in OmniSearch Lite ===
        SyncMessage::SearchQuery { .. } => SyncMessage::Confirm {
            success: false,
            message: "Search is not available in OmniSearch Lite. Use the full OmniSearch app for file indexing and search.".to_string(),
        },
        SyncMessage::RequestIndexStatus => SyncMessage::IndexStatusResponse {
            status: IndexStatus {
                indexing: false,
                ready: false,
                indexed_count: 0,
                last_error: Some("Indexing is not available in OmniSearch Lite. Use this connection for file transfers only.".to_string()),
            },
        },
        SyncMessage::RequestDrives => {
            // Return empty drives - Lite doesn't support indexing
            SyncMessage::DrivesResponse { drives: vec![] }
        }
        SyncMessage::StartIndexing { .. } => SyncMessage::Confirm {
            success: false,
            message: "Indexing is not available in OmniSearch Lite. Use the full OmniSearch app for NTFS indexing.".to_string(),
        },
        SyncMessage::RequestDuplicates { .. } => SyncMessage::Confirm {
            success: false,
            message: "Duplicate scanning is not available in OmniSearch Lite.".to_string(),
        },
        SyncMessage::RemoteAction { .. } => SyncMessage::Confirm {
            success: false,
            message: "Remote actions are not available in OmniSearch Lite.".to_string(),
        },
        SyncMessage::RequestFileContent { .. } => SyncMessage::Confirm {
            success: false,
            message: "File preview is not available in OmniSearch Lite.".to_string(),
        },

        // === Desktop → Phone file transfer ===
        SyncMessage::RequestDesktopTransfer => {
            let Some(device_id) = approved_device_id.as_deref() else {
                return serde_json::to_string(&pairing_response(
                    PairingStatusKind::Pending,
                    "Approve this phone before receiving desktop transfers.",
                ))
                .ok();
            };

            SyncMessage::DesktopTransferManifest {
                transfer: next_transfer_for_device(sync_state, device_id),
            }
        }
        SyncMessage::RequestDesktopTransferChunk {
            transfer_id,
            chunk_index,
        } => {
            let Some(device_id) = approved_device_id.as_deref() else {
                return serde_json::to_string(&pairing_response(
                    PairingStatusKind::Pending,
                    "Approve this phone before receiving desktop transfers.",
                ))
                .ok();
            };

            read_transfer_chunk(sync_state, device_id, &transfer_id, chunk_index)
        }

        // === Phone → Desktop file upload ===
        SyncMessage::PhoneUploadManifest {
            id,
            name,
            size,
            content_type,
            total_chunks,
        } => {
            let Some(device_id) = approved_device_id.as_deref() else {
                return serde_json::to_string(&pairing_response(
                    PairingStatusKind::Pending,
                    "Approve this phone before uploading files.",
                ))
                .ok();
            };

            let id = id.trim().to_string();
            let content_type = truncate_chars(&content_type, MAX_CONTENT_TYPE_CHARS);
            let dest_dir = get_destination_dir(&content_type);

            if !valid_transfer_id(&id) {
                return serde_json::to_string(&SyncMessage::Confirm {
                    success: false,
                    message: "Upload transfer id is invalid.".to_string(),
                })
                .ok();
            }

            if size > MAX_TRANSFER_BYTES {
                return serde_json::to_string(&SyncMessage::Confirm {
                    success: false,
                    message: "File size exceeds the 2 GB mobile transfer limit.".to_string(),
                })
                .ok();
            }

            let expected_chunks = expected_transfer_chunks(size);
            if total_chunks == 0 || total_chunks != expected_chunks {
                return serde_json::to_string(&SyncMessage::Confirm {
                    success: false,
                    message: "Upload manifest has an invalid chunk count.".to_string(),
                })
                .ok();
            }
            if sync_state
                .file_transfers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|transfer| transfer.id == id)
            {
                return serde_json::to_string(&SyncMessage::Confirm {
                    success: false,
                    message: "Upload transfer id is already in use.".to_string(),
                })
                .ok();
            }

            let unique_path = get_unique_path(&dest_dir, &name);
            let ext = unique_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();

            // Try creating the file
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&unique_path)
            {
                Ok(_) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    let transfer = MobileTransfer {
                        id: id.clone(),
                        name: unique_path.file_name().and_then(|n| n.to_str()).unwrap_or(&name).to_string(),
                        path: unique_path.to_string_lossy().into_owned(),
                        extension: ext,
                        size,
                        created_unix: now,
                        modified_unix: now,
                        content_type,
                        total_chunks,
                        bytes_sent: 0,
                        assigned_device_id: Some(device_id.to_string()),
                        received_chunks: HashSet::new(),
                        status: MobileTransferStatus::Receiving,
                        error: None,
                    };

                    if let Ok(mut transfers) = sync_state.file_transfers.lock() {
                        transfers.push(transfer);
                    }

                    SyncMessage::Confirm {
                        success: true,
                        message: "Manifest received and file initialized".to_string(),
                    }
                }
                Err(err) => {
                    SyncMessage::Confirm {
                        success: false,
                        message: format!("Failed to create file on desktop: {err}"),
                    }
                }
            }
        }
        SyncMessage::PhoneUploadChunk {
            transfer_id,
            chunk_index,
            data,
            done,
        } => {
            let Some(device_id) = approved_device_id.as_deref() else {
                return serde_json::to_string(&pairing_response(
                    PairingStatusKind::Pending,
                    "Approve this phone before uploading files.",
                ))
                .ok();
            };

            // Process the chunk
            match process_phone_upload_chunk(sync_state, device_id, &transfer_id, chunk_index, &data, done) {
                Ok(_) => SyncMessage::Confirm {
                    success: true,
                    message: format!("Chunk {chunk_index} processed"),
                },
                Err(err) => SyncMessage::Confirm {
                    success: false,
                    message: err,
                },
            }
        }

        // Responses from server (should not arrive from client) - ignore
        _ => return None,
    };

    serde_json::to_string(&response).ok()
}
