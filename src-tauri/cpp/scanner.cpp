#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>
#include <winioctl.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <cwctype>
#include <limits>
#include <mutex>
#include <shared_mutex>
#include <string>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace {

struct RawUsnEntry {
  uint64_t frn;
  uint64_t parent_frn;
  std::wstring name;
  bool is_directory;
  uint32_t reason;
};

struct NodeEntry {
  uint64_t parent_frn;
  std::wstring name;
  bool is_directory;
};

struct IndexedFile {
  uint64_t frn;
  std::wstring name;
  std::wstring path;
  std::wstring extension_lower;
  bool is_directory;
};

struct ScanSnapshot {
  std::vector<IndexedFile> files;
  std::unordered_map<uint64_t, NodeEntry> nodes;
  uint64_t root_frn = 0;
  std::wstring root_path;
  uint64_t journal_id = 0;
  int64_t journal_next_usn = 0;
  bool live_updates_supported = false;
};

struct SearchRow {
  std::wstring name;
  std::wstring path;
  std::wstring extension;
  uint64_t size;
  int64_t created_unix;
  int64_t modified_unix;
  bool is_directory;
};

struct DuplicateFileRow {
  std::wstring name;
  std::wstring path;
  uint64_t size;
  int64_t created_unix;
  int64_t modified_unix;
};

struct DuplicateGroupRow {
  std::string group_id;
  uint64_t size;
  uint64_t total_bytes;
  uint32_t file_count;
  std::vector<DuplicateFileRow> files;
};

struct DriveInfo {
  std::wstring letter;
  std::wstring path;
  std::wstring filesystem;
  std::wstring drive_type;
  bool is_ntfs;
  bool can_open_volume;
};

std::shared_mutex g_index_mutex;
std::vector<IndexedFile> g_indexed_files;
std::unordered_map<uint64_t, size_t> g_file_position_by_frn;
std::unordered_map<uint64_t, NodeEntry> g_nodes;
uint64_t g_root_frn = 0;
std::wstring g_root_path;
std::atomic<bool> g_is_indexing{false};
std::atomic<bool> g_is_ready{false};
std::atomic<uint64_t> g_indexed_count{0};
std::atomic<bool> g_include_directories{false};
std::atomic<bool> g_scan_all_drives_mode{false};
std::atomic<uint64_t> g_indexing_request_token{0};
std::atomic<uint64_t> g_live_watcher_token{0};
std::atomic<bool> g_duplicate_scan_running{false};
std::atomic<bool> g_duplicate_cancel_requested{false};
std::atomic<uint64_t> g_duplicate_progress_done{0};
std::atomic<uint64_t> g_duplicate_progress_total{0};
std::atomic<uint64_t> g_duplicate_groups_found{0};
std::mutex g_error_mutex;
std::string g_last_error;

std::vector<DriveInfo> list_drives_internal();

void SetLastErrorText(const std::string& error) {
  std::lock_guard<std::mutex> lock(g_error_mutex);
  g_last_error = error;
}

std::string ReadLastErrorText() {
  std::lock_guard<std::mutex> lock(g_error_mutex);
  return g_last_error;
}

std::wstring Utf8ToWide(const std::string& value) {
  if (value.empty()) {
    return L"";
  }
  const int required = MultiByteToWideChar(
      CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0);
  if (required <= 0) {
    return L"";
  }
  std::wstring out(static_cast<size_t>(required), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()),
                      out.data(), required);
  return out;
}

std::string WideToUtf8(const std::wstring& value) {
  if (value.empty()) {
    return "";
  }
  const int required = WideCharToMultiByte(
      CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0,
      nullptr, nullptr);
  if (required <= 0) {
    return "";
  }
  std::string out(static_cast<size_t>(required), '\0');
  WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()),
                      out.data(), required, nullptr, nullptr);
  return out;
}

std::wstring ToLower(std::wstring value) {
  std::transform(value.begin(), value.end(), value.begin(), [](wchar_t ch) {
    return static_cast<wchar_t>(std::towlower(ch));
  });
  return value;
}

std::string DescribeWin32Error(const DWORD error_code) {
  LPSTR message_buffer = nullptr;
  const DWORD flags = FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM |
                      FORMAT_MESSAGE_IGNORE_INSERTS;
  const DWORD message_len =
      FormatMessageA(flags, nullptr, error_code, 0,
                     reinterpret_cast<LPSTR>(&message_buffer), 0, nullptr);
  std::string message;
  if (message_len > 0 && message_buffer != nullptr) {
    message.assign(message_buffer, message_len);
    while (!message.empty() &&
           (message.back() == '\r' || message.back() == '\n' || message.back() == ' ')) {
      message.pop_back();
    }
  }
  if (message_buffer != nullptr) {
    LocalFree(message_buffer);
  }

  char code_buffer[16];
  std::snprintf(code_buffer, sizeof(code_buffer), "0x%08lX",
                static_cast<unsigned long>(error_code));
  if (message.empty()) {
    return std::string(code_buffer);
  }
  return std::string(code_buffer) + " " + message;
}

std::string BuildWin32ErrorText(const std::string& context, const DWORD error_code) {
  std::string out = context;
  out.append(" (");
  out.append(DescribeWin32Error(error_code));
  out.append(")");
  return out;
}

bool IsUsnJournalMissingError(const DWORD error_code) {
  return error_code == ERROR_JOURNAL_NOT_ACTIVE ||
         error_code == ERROR_JOURNAL_DELETE_IN_PROGRESS ||
         error_code == ERROR_FILE_NOT_FOUND;
}

bool IsPathMissingError(const DWORD error_code) {
  return error_code == ERROR_FILE_NOT_FOUND ||
         error_code == ERROR_PATH_NOT_FOUND ||
         error_code == ERROR_INVALID_NAME ||
         error_code == ERROR_BAD_NETPATH ||
         error_code == ERROR_BAD_NET_NAME ||
         error_code == ERROR_NOT_READY;
}

bool IsLiveWatcherCancelled(const uint64_t token) {
  return g_live_watcher_token.load(std::memory_order_acquire) != token;
}

void StopLiveWatcher() {
  g_live_watcher_token.fetch_add(1, std::memory_order_acq_rel);
}

bool IsIndexingCancelled(const uint64_t request_token) {
  if (request_token == 0) {
    return false;
  }
  return g_indexing_request_token.load(std::memory_order_acquire) != request_token;
}

bool IsDuplicateScanCancelRequested() {
  return g_duplicate_cancel_requested.load(std::memory_order_acquire);
}

void ResetDuplicateProgress() {
  g_duplicate_progress_done.store(0, std::memory_order_release);
  g_duplicate_progress_total.store(0, std::memory_order_release);
  g_duplicate_groups_found.store(0, std::memory_order_release);
}

void AddDuplicateProgressTotal(const uint64_t units) {
  if (units == 0) {
    return;
  }
  g_duplicate_progress_total.fetch_add(units, std::memory_order_acq_rel);
}

void AddDuplicateProgressDone(const uint64_t units) {
  if (units == 0) {
    return;
  }
  g_duplicate_progress_done.fetch_add(units, std::memory_order_acq_rel);
}

std::wstring NormalizeDriveLetter(const char* drive_utf8) {
  std::wstring drive = Utf8ToWide(drive_utf8 == nullptr ? "C" : drive_utf8);
  if (drive.empty()) {
    return L"C";
  }
  wchar_t candidate = static_cast<wchar_t>(std::towupper(drive[0]));
  if (candidate < L'A' || candidate > L'Z') {
    candidate = L'C';
  }
  return std::wstring(1, candidate);
}

std::wstring DriveTypeToText(const UINT drive_type) {
  switch (drive_type) {
    case DRIVE_FIXED:
      return L"fixed";
    case DRIVE_REMOVABLE:
      return L"removable";
    case DRIVE_REMOTE:
      return L"network";
    case DRIVE_CDROM:
      return L"cdrom";
    case DRIVE_RAMDISK:
      return L"ramdisk";
    case DRIVE_NO_ROOT_DIR:
      return L"no-root";
    default:
      return L"unknown";
  }
}

bool CanOpenVolume(const std::wstring& drive_letter) {
  const std::wstring volume_path = L"\\\\.\\" + drive_letter + L":";
  HANDLE volume = CreateFileW(
      volume_path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL, nullptr);
  if (volume == INVALID_HANDLE_VALUE) {
    return false;
  }
  CloseHandle(volume);
  return true;
}

std::wstring ExtractExtensionLower(const std::wstring& file_name) {
  const size_t dot = file_name.find_last_of(L'.');
  if (dot == std::wstring::npos || dot == 0 || dot + 1 >= file_name.size()) {
    return L"";
  }
  return ToLower(file_name.substr(dot + 1));
}

std::wstring NormalizeExtensionFilter(const char* extension_utf8) {
  std::wstring normalized =
      ToLower(Utf8ToWide(extension_utf8 == nullptr ? "" : extension_utf8));
  while (!normalized.empty() && normalized.front() == L'.') {
    normalized.erase(normalized.begin());
  }
  return normalized;
}

wchar_t DriveBucketKeyFromPath(const std::wstring& path) {
  if (path.size() >= 2 && path[1] == L':') {
    const wchar_t drive = static_cast<wchar_t>(std::towupper(path[0]));
    if (drive >= L'A' && drive <= L'Z') {
      return drive;
    }
  }
  if (path.rfind(L"\\\\", 0) == 0) {
    return L'#';
  }
  return L'?';
}

int64_t FileTimeToUnixSeconds(const FILETIME& file_time) {
  ULARGE_INTEGER value;
  value.LowPart = file_time.dwLowDateTime;
  value.HighPart = file_time.dwHighDateTime;
  constexpr uint64_t kTicksPerSecond = 10000000ULL;
  constexpr uint64_t kUnixEpochInWindowsTicks = 11644473600ULL * kTicksPerSecond;
  if (value.QuadPart < kUnixEpochInWindowsTicks) {
    return 0;
  }
  return static_cast<int64_t>(
      (value.QuadPart - kUnixEpochInWindowsTicks) / kTicksPerSecond);
}

bool ReadFileMetadata(const std::wstring& path, uint64_t* size, int64_t* created_unix,
                      int64_t* modified_unix) {
  WIN32_FILE_ATTRIBUTE_DATA data{};
  if (!GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &data)) {
    return false;
  }
  *size = (static_cast<uint64_t>(data.nFileSizeHigh) << 32) | data.nFileSizeLow;
  *created_unix = FileTimeToUnixSeconds(data.ftCreationTime);
  *modified_unix = FileTimeToUnixSeconds(data.ftLastWriteTime);
  return true;
}

bool ContainsCaseInsensitive(const std::wstring& text, const std::wstring& needle_lower) {
  if (needle_lower.empty()) {
    return true;
  }
  if (needle_lower.size() > text.size()) {
    return false;
  }

  const size_t last_start = text.size() - needle_lower.size();
  for (size_t i = 0; i <= last_start; ++i) {
    bool matched = true;
    for (size_t j = 0; j < needle_lower.size(); ++j) {
      if (static_cast<wchar_t>(std::towlower(text[i + j])) != needle_lower[j]) {
        matched = false;
        break;
      }
    }
    if (matched) {
      return true;
    }
  }
  return false;
}

void AppendEscapedJsonString(std::string* out, const std::string& value) {
  for (const char ch : value) {
    switch (ch) {
      case '\"':
        out->append("\\\"");
        break;
      case '\\':
        out->append("\\\\");
        break;
      case '\b':
        out->append("\\b");
        break;
      case '\f':
        out->append("\\f");
        break;
      case '\n':
        out->append("\\n");
        break;
      case '\r':
        out->append("\\r");
        break;
      case '\t':
        out->append("\\t");
        break;
      default: {
        const unsigned char u = static_cast<unsigned char>(ch);
        if (u < 0x20) {
          char encoded[7];
          std::snprintf(encoded, sizeof(encoded), "\\u%04x", static_cast<unsigned int>(u));
          out->append(encoded);
        } else {
          out->push_back(ch);
        }
        break;
      }
    }
  }
}

std::string SearchRowsToJson(const std::vector<SearchRow>& rows) {
  std::string json;
  json.reserve(rows.size() * 176);
  json.push_back('[');
  for (size_t i = 0; i < rows.size(); ++i) {
    if (i > 0) {
      json.push_back(',');
    }
    json.append("{\"name\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].name));
    json.append("\",\"path\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].path));
    json.append("\",\"extension\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].extension));
    json.append("\",\"size\":");
    json.append(std::to_string(rows[i].size));
    json.append(",\"createdUnix\":");
    json.append(std::to_string(rows[i].created_unix));
    json.append(",\"modifiedUnix\":");
    json.append(std::to_string(rows[i].modified_unix));
    json.append(",\"isDirectory\":");
    json.append(rows[i].is_directory ? "true" : "false");
    json.push_back('}');
  }
  json.push_back(']');
  return json;
}

std::string DuplicateGroupsToJson(const std::vector<DuplicateGroupRow>& groups) {
  std::string json;
  json.reserve(groups.size() * 320);
  json.push_back('[');
  for (size_t i = 0; i < groups.size(); ++i) {
    if (i > 0) {
      json.push_back(',');
    }
    const DuplicateGroupRow& group = groups[i];
    json.append("{\"groupId\":\"");
    AppendEscapedJsonString(&json, group.group_id);
    json.append("\",\"size\":");
    json.append(std::to_string(group.size));
    json.append(",\"totalBytes\":");
    json.append(std::to_string(group.total_bytes));
    json.append(",\"fileCount\":");
    json.append(std::to_string(group.file_count));
    json.append(",\"files\":[");
    for (size_t file_index = 0; file_index < group.files.size(); ++file_index) {
      if (file_index > 0) {
        json.push_back(',');
      }
      const DuplicateFileRow& file = group.files[file_index];
      json.append("{\"name\":\"");
      AppendEscapedJsonString(&json, WideToUtf8(file.name));
      json.append("\",\"path\":\"");
      AppendEscapedJsonString(&json, WideToUtf8(file.path));
      json.append("\",\"size\":");
      json.append(std::to_string(file.size));
      json.append(",\"createdUnix\":");
      json.append(std::to_string(file.created_unix));
      json.append(",\"modifiedUnix\":");
      json.append(std::to_string(file.modified_unix));
      json.push_back('}');
    }
    json.append("]}");
  }
  json.push_back(']');
  return json;
}

std::string DriveRowsToJson(const std::vector<DriveInfo>& rows) {
  std::string json;
  json.reserve(rows.size() * 120);
  json.push_back('[');
  for (size_t i = 0; i < rows.size(); ++i) {
    if (i > 0) {
      json.push_back(',');
    }
    json.append("{\"letter\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].letter));
    json.append("\",\"path\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].path));
    json.append("\",\"filesystem\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].filesystem));
    json.append("\",\"driveType\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(rows[i].drive_type));
    json.append("\",\"isNtfs\":");
    json.append(rows[i].is_ntfs ? "true" : "false");
    json.append(",\"canOpenVolume\":");
    json.append(rows[i].can_open_volume ? "true" : "false");
    json.push_back('}');
  }
  json.push_back(']');
  return json;
}

std::string BasicFilesToJson(const std::vector<IndexedFile>& files) {
  std::string json;
  json.reserve(files.size() * 112);
  json.push_back('[');
  for (size_t i = 0; i < files.size(); ++i) {
    if (i > 0) {
      json.push_back(',');
    }
    json.append("{\"name\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(files[i].name));
    json.append("\",\"path\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(files[i].path));
    json.append("\",\"isDirectory\":");
    json.append(files[i].is_directory ? "true" : "false");
    json.push_back('}');
  }
  json.push_back(']');
  return json;
}

std::string DuplicateScanStatusToJson() {
  const bool running = g_duplicate_scan_running.load(std::memory_order_acquire);
  const bool cancel_requested =
      g_duplicate_cancel_requested.load(std::memory_order_acquire);
  const uint64_t scanned_files =
      g_duplicate_progress_done.load(std::memory_order_acquire);
  const uint64_t total_files =
      g_duplicate_progress_total.load(std::memory_order_acquire);
  const uint64_t groups_found =
      g_duplicate_groups_found.load(std::memory_order_acquire);
  double progress_percent = 0.0;
  if (total_files > 0) {
    progress_percent =
        (static_cast<double>(scanned_files) * 100.0) / static_cast<double>(total_files);
    if (progress_percent > 100.0) {
      progress_percent = 100.0;
    }
  }

  char percent_buffer[32];
  std::snprintf(percent_buffer, sizeof(percent_buffer), "%.2f", progress_percent);

  std::string json;
  json.reserve(196);
  json.append("{\"running\":");
  json.append(running ? "true" : "false");
  json.append(",\"cancelRequested\":");
  json.append(cancel_requested ? "true" : "false");
  json.append(",\"scannedFiles\":");
  json.append(std::to_string(scanned_files));
  json.append(",\"totalFiles\":");
  json.append(std::to_string(total_files));
  json.append(",\"groupsFound\":");
  json.append(std::to_string(groups_found));
  json.append(",\"progressPercent\":");
  json.append(percent_buffer);
  json.push_back('}');
  return json;
}

char* HeapCopyString(const std::string& value) {
  char* raw = static_cast<char*>(std::malloc(value.size() + 1));
  if (raw == nullptr) {
    return nullptr;
  }
  std::memcpy(raw, value.c_str(), value.size() + 1);
  return raw;
}

bool GetRootFrn(const std::wstring& root_path, uint64_t* out_root_frn,
                std::string* out_error) {
  HANDLE root = CreateFileW(
      root_path.c_str(), FILE_READ_ATTRIBUTES,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_FLAG_BACKUP_SEMANTICS, nullptr);
  if (root == INVALID_HANDLE_VALUE) {
    *out_error = "Failed to open drive root handle.";
    return false;
  }

  BY_HANDLE_FILE_INFORMATION info{};
  const bool ok = GetFileInformationByHandle(root, &info) != FALSE;
  CloseHandle(root);
  if (!ok) {
    *out_error = "Failed to read root file reference number.";
    return false;
  }
  *out_root_frn =
      (static_cast<uint64_t>(info.nFileIndexHigh) << 32) | info.nFileIndexLow;
  return true;
}

uint64_t FileId128ToU64(const FILE_ID_128& id128) {
  uint64_t id = 0;
  std::memcpy(&id, id128.Identifier, sizeof(id));
  return id;
}

bool ParseUsnRecord(const BYTE* record_ptr, DWORD record_length, RawUsnEntry* out) {
  if (record_length < sizeof(USN_RECORD_V2)) {
    return false;
  }

  const auto* v2 = reinterpret_cast<const USN_RECORD_V2*>(record_ptr);
  if (v2->MajorVersion == 2) {
    if (static_cast<DWORD>(v2->FileNameOffset) +
            static_cast<DWORD>(v2->FileNameLength) >
        v2->RecordLength) {
      return false;
    }
    const auto* name_ptr =
        reinterpret_cast<const wchar_t*>(record_ptr + v2->FileNameOffset);
    out->frn = v2->FileReferenceNumber;
    out->parent_frn = v2->ParentFileReferenceNumber;
    out->is_directory = (v2->FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
    out->reason = v2->Reason;
    out->name.assign(name_ptr, v2->FileNameLength / sizeof(wchar_t));
    return true;
  }

#if defined(USN_RECORD_V3)
  if (v2->MajorVersion == 3) {
    if (record_length < sizeof(USN_RECORD_V3)) {
      return false;
    }
    const auto* v3 = reinterpret_cast<const USN_RECORD_V3*>(record_ptr);
    if (static_cast<DWORD>(v3->FileNameOffset) +
            static_cast<DWORD>(v3->FileNameLength) >
        v3->RecordLength) {
      return false;
    }
    const auto* name_ptr =
        reinterpret_cast<const wchar_t*>(record_ptr + v3->FileNameOffset);
    out->frn = FileId128ToU64(v3->FileReferenceNumber);
    out->parent_frn = FileId128ToU64(v3->ParentFileReferenceNumber);
    out->is_directory = (v3->FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
    out->reason = v3->Reason;
    out->name.assign(name_ptr, v3->FileNameLength / sizeof(wchar_t));
    return true;
  }
#endif

  return false;
}

bool ResolvePathForFrn(
    uint64_t frn, uint64_t root_frn, const std::wstring& root_path,
    const std::unordered_map<uint64_t, NodeEntry>& nodes,
    std::unordered_map<uint64_t, std::wstring>* cache,
    std::unordered_set<uint64_t>* resolving, std::wstring* out_path) {
  const auto cached = cache->find(frn);
  if (cached != cache->end()) {
    *out_path = cached->second;
    return true;
  }

  if (frn == root_frn) {
    *out_path = root_path;
    return true;
  }

  const auto node_it = nodes.find(frn);
  if (node_it == nodes.end()) {
    return false;
  }

  if (!resolving->insert(frn).second) {
    return false;
  }

  std::wstring parent_path;
  const bool parent_ok = ResolvePathForFrn(node_it->second.parent_frn, root_frn, root_path,
                                           nodes, cache, resolving, &parent_path);
  resolving->erase(frn);
  if (!parent_ok) {
    return false;
  }

  std::wstring full_path = parent_path;
  if (!full_path.empty() && full_path.back() != L'\\') {
    full_path.push_back(L'\\');
  }
  full_path.append(node_it->second.name);
  (*cache)[frn] = full_path;
  *out_path = full_path;
  return true;
}

void RebuildFilePositionLookupLocked() {
  g_file_position_by_frn.clear();
  g_file_position_by_frn.reserve(g_indexed_files.size() * 2 + 1);
  for (size_t i = 0; i < g_indexed_files.size(); ++i) {
    g_file_position_by_frn[g_indexed_files[i].frn] = i;
  }
}

void RemoveIndexedFileByFrnLocked(const uint64_t frn) {
  const auto position_it = g_file_position_by_frn.find(frn);
  if (position_it == g_file_position_by_frn.end()) {
    return;
  }

  const size_t remove_index = position_it->second;
  const size_t last_index = g_indexed_files.size() - 1;
  if (remove_index != last_index) {
    g_indexed_files[remove_index] = std::move(g_indexed_files[last_index]);
    g_file_position_by_frn[g_indexed_files[remove_index].frn] = remove_index;
  }
  g_indexed_files.pop_back();
  g_file_position_by_frn.erase(position_it);
}

void UpsertIndexedFileLocked(const uint64_t frn, const std::wstring& name,
                             std::wstring full_path, const bool is_directory) {
  IndexedFile next_file{
      frn,
      name,
      std::move(full_path),
      is_directory ? L"" : ExtractExtensionLower(name),
      is_directory,
  };
  const auto position_it = g_file_position_by_frn.find(frn);
  if (position_it == g_file_position_by_frn.end()) {
    g_file_position_by_frn.emplace(frn, g_indexed_files.size());
    g_indexed_files.push_back(std::move(next_file));
    return;
  }

  g_indexed_files[position_it->second] = std::move(next_file);
}

void RebuildIndexedFilesFromNodesLocked() {
  g_indexed_files.clear();
  g_file_position_by_frn.clear();
  if (g_root_frn == 0 || g_root_path.empty() || g_nodes.empty()) {
    return;
  }

  g_indexed_files.reserve(g_nodes.size() / 2 + 1);
  g_file_position_by_frn.reserve(g_nodes.size() / 2 + 1);

  std::unordered_map<uint64_t, std::wstring> path_cache;
  path_cache.reserve(g_nodes.size() / 2 + 1);
  path_cache[g_root_frn] = g_root_path;
  std::unordered_set<uint64_t> resolving;
  const bool include_directories =
      g_include_directories.load(std::memory_order_acquire);

  for (const auto& pair : g_nodes) {
    const uint64_t frn = pair.first;
    const NodeEntry& node = pair.second;
    if (node.name.empty() || (node.is_directory && !include_directories)) {
      continue;
    }

    resolving.clear();
    std::wstring full_path;
    const bool resolved = ResolvePathForFrn(frn, g_root_frn, g_root_path, g_nodes,
                                            &path_cache, &resolving, &full_path);
    if (!resolved || full_path.empty()) {
      continue;
    }

    g_file_position_by_frn.emplace(frn, g_indexed_files.size());
    g_indexed_files.push_back(IndexedFile{
        frn,
        node.name,
        std::move(full_path),
        node.is_directory ? L"" : ExtractExtensionLower(node.name),
        node.is_directory,
    });
  }
}

void ApplyUsnBatchLocked(const std::vector<RawUsnEntry>& entries) {
  if (g_root_frn == 0 || g_root_path.empty() || entries.empty()) {
    return;
  }

  bool requires_full_rebuild = false;
  const bool include_directories =
      g_include_directories.load(std::memory_order_acquire);
  std::unordered_map<uint64_t, std::wstring> path_cache;
  path_cache.reserve(entries.size() * 2 + 8);
  path_cache[g_root_frn] = g_root_path;
  std::unordered_set<uint64_t> resolving;

  for (const RawUsnEntry& entry : entries) {
    if (entry.frn == 0 || entry.name.empty()) {
      continue;
    }

    const bool is_delete = (entry.reason & USN_REASON_FILE_DELETE) != 0;
    const bool is_old_rename_only =
        (entry.reason & USN_REASON_RENAME_OLD_NAME) != 0 &&
        (entry.reason & USN_REASON_RENAME_NEW_NAME) == 0 && !is_delete;
    if (is_old_rename_only) {
      continue;
    }

    auto old_node_it = g_nodes.find(entry.frn);
    const bool had_old_node = old_node_it != g_nodes.end();
    NodeEntry old_node{};
    if (had_old_node) {
      old_node = old_node_it->second;
    }

    if (is_delete) {
      if (had_old_node && old_node.is_directory) {
        requires_full_rebuild = true;
      }
      g_nodes.erase(entry.frn);
      RemoveIndexedFileByFrnLocked(entry.frn);
      continue;
    }

    g_nodes[entry.frn] = NodeEntry{entry.parent_frn, entry.name, entry.is_directory};

    if (entry.is_directory) {
      if (!had_old_node || !old_node.is_directory ||
          old_node.parent_frn != entry.parent_frn || old_node.name != entry.name) {
        requires_full_rebuild = true;
      }
      if (!include_directories) {
        RemoveIndexedFileByFrnLocked(entry.frn);
        continue;
      }

      resolving.clear();
      std::wstring full_path;
      const bool resolved = ResolvePathForFrn(entry.frn, g_root_frn, g_root_path, g_nodes,
                                              &path_cache, &resolving, &full_path);
      if (!resolved || full_path.empty()) {
        RemoveIndexedFileByFrnLocked(entry.frn);
        continue;
      }

      UpsertIndexedFileLocked(entry.frn, entry.name, std::move(full_path), true);
      continue;
    }

    resolving.clear();
    std::wstring full_path;
    const bool resolved = ResolvePathForFrn(entry.frn, g_root_frn, g_root_path, g_nodes,
                                            &path_cache, &resolving, &full_path);
    if (!resolved || full_path.empty()) {
      RemoveIndexedFileByFrnLocked(entry.frn);
      continue;
    }

    UpsertIndexedFileLocked(entry.frn, entry.name, std::move(full_path), false);
  }

  if (requires_full_rebuild) {
    RebuildIndexedFilesFromNodesLocked();
  }
  g_indexed_count.store(static_cast<uint64_t>(g_indexed_files.size()),
                        std::memory_order_release);
}

void ApplyScanSnapshotLocked(ScanSnapshot* snapshot) {
  g_indexed_files = std::move(snapshot->files);
  g_nodes = std::move(snapshot->nodes);
  g_root_frn = snapshot->root_frn;
  g_root_path = std::move(snapshot->root_path);
  RebuildFilePositionLookupLocked();
}

void ApplyIndexedFilesOnlyLocked(std::vector<IndexedFile> files) {
  g_indexed_files = std::move(files);
  g_nodes.clear();
  g_root_frn = 0;
  g_root_path.clear();
  RebuildFilePositionLookupLocked();
}

std::string BuildDuplicateGroupId(const uint64_t size, const uint64_t hash_value,
                                  const uint32_t serial) {
  char buffer[64];
  std::snprintf(buffer, sizeof(buffer), "%016llx-%016llx-%08lx",
                static_cast<unsigned long long>(size),
                static_cast<unsigned long long>(hash_value),
                static_cast<unsigned long>(serial));
  return std::string(buffer);
}

constexpr uint64_t kFNVOffsetBasis = 1469598103934665603ULL;
constexpr uint64_t kFNVPrime = 1099511628211ULL;

void FNV1aMixBuffer(const BYTE* bytes, const size_t len, uint64_t* hash) {
  for (size_t i = 0; i < len; ++i) {
    *hash ^= static_cast<uint64_t>(bytes[i]);
    *hash *= kFNVPrime;
  }
}

void FNV1aMixU64(const uint64_t value, uint64_t* hash) {
  const BYTE* bytes = reinterpret_cast<const BYTE*>(&value);
  FNV1aMixBuffer(bytes, sizeof(value), hash);
}

bool HashFileFNV1a64(const std::wstring& path, uint64_t* out_hash) {
  if (IsDuplicateScanCancelRequested()) {
    return false;
  }

  HANDLE file = CreateFileW(
      path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_SEQUENTIAL_SCAN, nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    return false;
  }

  constexpr DWORD kBufferSize = 1 * 1024 * 1024;
  thread_local std::vector<BYTE> buffer;
  if (buffer.size() != kBufferSize) {
    buffer.resize(kBufferSize);
  }
  uint64_t hash = kFNVOffsetBasis;

  bool ok = true;
  while (true) {
    if (IsDuplicateScanCancelRequested()) {
      ok = false;
      break;
    }

    DWORD bytes_read = 0;
    const BOOL read_ok =
        ReadFile(file, buffer.data(), kBufferSize, &bytes_read, nullptr);
    if (!read_ok) {
      ok = false;
      break;
    }
    if (bytes_read == 0) {
      break;
    }
    FNV1aMixBuffer(buffer.data(), bytes_read, &hash);
  }

  CloseHandle(file);
  if (!ok) {
    return false;
  }
  *out_hash = hash;
  return true;
}

bool HashFileQuickSignature64(const DuplicateFileRow& file, uint64_t* out_hash) {
  if (IsDuplicateScanCancelRequested()) {
    return false;
  }

  uint64_t hash = kFNVOffsetBasis;
  FNV1aMixU64(file.size, &hash);
  if (file.size == 0) {
    *out_hash = hash;
    return true;
  }

  HANDLE handle = CreateFileW(
      file.path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_RANDOM_ACCESS, nullptr);
  if (handle == INVALID_HANDLE_VALUE) {
    return false;
  }

  constexpr DWORD kChunkBytes = 64 * 1024;
  thread_local std::vector<BYTE> buffer;
  if (buffer.size() != kChunkBytes) {
    buffer.resize(kChunkBytes);
  }

  const DWORD first_bytes =
      static_cast<DWORD>(std::min<uint64_t>(file.size, kChunkBytes));
  DWORD first_read = 0;
  bool ok = ReadFile(handle, buffer.data(), first_bytes, &first_read, nullptr) != FALSE &&
            first_read == first_bytes;
  if (ok && first_read > 0) {
    FNV1aMixBuffer(buffer.data(), first_read, &hash);
  }

  if (ok && file.size > static_cast<uint64_t>(first_bytes)) {
    if (IsDuplicateScanCancelRequested()) {
      ok = false;
    }

    const DWORD tail_bytes =
        static_cast<DWORD>(std::min<uint64_t>(file.size, kChunkBytes));
    LARGE_INTEGER offset{};
    offset.QuadPart = static_cast<LONGLONG>(file.size - tail_bytes);
    ok = SetFilePointerEx(handle, offset, nullptr, FILE_BEGIN) != FALSE;
    if (ok) {
      DWORD tail_read = 0;
      ok = ReadFile(handle, buffer.data(), tail_bytes, &tail_read, nullptr) != FALSE &&
           tail_read == tail_bytes;
      if (ok && tail_read > 0) {
        FNV1aMixBuffer(buffer.data(), tail_read, &hash);
      }
    }
  }

  CloseHandle(handle);
  if (!ok) {
    return false;
  }

  *out_hash = hash;
  return true;
}

bool AreFilesByteEqual(const std::wstring& left_path, const std::wstring& right_path) {
  if (IsDuplicateScanCancelRequested()) {
    return false;
  }

  HANDLE left = CreateFileW(
      left_path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_SEQUENTIAL_SCAN, nullptr);
  if (left == INVALID_HANDLE_VALUE) {
    return false;
  }

  HANDLE right = CreateFileW(
      right_path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_SEQUENTIAL_SCAN, nullptr);
  if (right == INVALID_HANDLE_VALUE) {
    CloseHandle(left);
    return false;
  }

  constexpr DWORD kBufferSize = 1 * 1024 * 1024;
  thread_local std::vector<BYTE> left_buffer;
  thread_local std::vector<BYTE> right_buffer;
  if (left_buffer.size() != kBufferSize) {
    left_buffer.resize(kBufferSize);
  }
  if (right_buffer.size() != kBufferSize) {
    right_buffer.resize(kBufferSize);
  }
  DWORD left_read = 0;
  DWORD right_read = 0;
  bool equal = true;

  while (true) {
    if (IsDuplicateScanCancelRequested()) {
      equal = false;
      break;
    }

    const BOOL left_ok = ReadFile(left, left_buffer.data(), kBufferSize, &left_read, nullptr);
    const BOOL right_ok =
        ReadFile(right, right_buffer.data(), kBufferSize, &right_read, nullptr);
    if (!left_ok || !right_ok) {
      equal = false;
      break;
    }
    if (left_read != right_read) {
      equal = false;
      break;
    }
    if (left_read == 0) {
      break;
    }
    if (std::memcmp(left_buffer.data(), right_buffer.data(), left_read) != 0) {
      equal = false;
      break;
    }
  }

  CloseHandle(right);
  CloseHandle(left);
  return equal;
}

bool HashDuplicateFileFull(const DuplicateFileRow& file, uint64_t* out_hash) {
  return HashFileFNV1a64(file.path, out_hash);
}

bool HashDuplicateFileQuick(const DuplicateFileRow& file, uint64_t* out_hash) {
  return HashFileQuickSignature64(file, out_hash);
}

using DuplicateHashFn = bool (*)(const DuplicateFileRow&, uint64_t*);

size_t ComputeDuplicateWorkerCount(const size_t item_count) {
  if (item_count == 0) {
    return 1;
  }

  size_t worker_count = std::thread::hardware_concurrency();
  if (worker_count == 0) {
    worker_count = 4;
  }
  const size_t reserved_cores = worker_count > 4 ? 2 : 1;
  const size_t usable_workers = worker_count > reserved_cores
                                    ? worker_count - reserved_cores
                                    : static_cast<size_t>(1);
  return std::max<size_t>(1, std::min(usable_workers, item_count));
}

void HashFilesParallel(const std::vector<DuplicateFileRow>& files,
                       std::vector<uint64_t>* out_hashes,
                       std::vector<uint8_t>* out_ok_flags,
                       const DuplicateHashFn hash_fn,
                       const bool track_progress) {
  out_hashes->assign(files.size(), 0);
  out_ok_flags->assign(files.size(), 0);
  if (files.empty()) {
    return;
  }

  const size_t worker_count = ComputeDuplicateWorkerCount(files.size());

  std::atomic<size_t> next_index{0};
  std::vector<std::thread> workers;
  workers.reserve(worker_count);
  for (size_t worker = 0; worker < worker_count; ++worker) {
    workers.emplace_back([&]() {
      while (true) {
        if (IsDuplicateScanCancelRequested()) {
          return;
        }

        const size_t index = next_index.fetch_add(1, std::memory_order_relaxed);
        if (index >= files.size()) {
          return;
        }
        uint64_t hash = 0;
        if (hash_fn != nullptr && hash_fn(files[index], &hash)) {
          (*out_hashes)[index] = hash;
          (*out_ok_flags)[index] = 1;
        }
        if (track_progress) {
          AddDuplicateProgressDone(1);
        }
      }
    });
  }

  for (std::thread& worker : workers) {
    worker.join();
  }
}

std::vector<DuplicateGroupRow> find_duplicates_internal(const uint64_t min_size,
                                                        const uint32_t max_groups,
                                                        const uint32_t max_files_per_group) {
  std::vector<IndexedFile> indexed_snapshot;
  {
    std::shared_lock<std::shared_mutex> lock(g_index_mutex);
    indexed_snapshot = g_indexed_files;
  }

  std::vector<uint64_t> metadata_sizes(indexed_snapshot.size(), 0);
  std::vector<int64_t> metadata_created(indexed_snapshot.size(), 0);
  std::vector<int64_t> metadata_modified(indexed_snapshot.size(), 0);
  std::vector<uint8_t> metadata_ok(indexed_snapshot.size(), 0);
  std::unordered_map<uint64_t, std::vector<DuplicateFileRow>> size_buckets;
  size_buckets.reserve(indexed_snapshot.size() / 4 + 1);
  std::vector<DuplicateGroupRow> groups;
  groups.reserve(128);
  uint32_t group_serial = 0;

  AddDuplicateProgressTotal(static_cast<uint64_t>(indexed_snapshot.size()));
  const size_t metadata_workers = ComputeDuplicateWorkerCount(indexed_snapshot.size());
  std::atomic<size_t> metadata_index{0};
  std::vector<std::thread> metadata_threads;
  metadata_threads.reserve(metadata_workers);
  for (size_t worker = 0; worker < metadata_workers; ++worker) {
    metadata_threads.emplace_back([&]() {
      while (true) {
        if (IsDuplicateScanCancelRequested()) {
          return;
        }

        const size_t index = metadata_index.fetch_add(1, std::memory_order_relaxed);
        if (index >= indexed_snapshot.size()) {
          return;
        }
        const IndexedFile& file = indexed_snapshot[index];
        if (file.is_directory) {
          AddDuplicateProgressDone(1);
          continue;
        }
        uint64_t size = 0;
        int64_t created = 0;
        int64_t modified = 0;
        const bool metadata_loaded =
            ReadFileMetadata(file.path, &size, &created, &modified);
        if (!metadata_loaded || size < min_size) {
          AddDuplicateProgressDone(1);
          continue;
        }
        metadata_sizes[index] = size;
        metadata_created[index] = created;
        metadata_modified[index] = modified;
        metadata_ok[index] = 1;
        AddDuplicateProgressDone(1);
      }
    });
  }
  for (std::thread& worker : metadata_threads) {
    worker.join();
  }
  if (IsDuplicateScanCancelRequested()) {
    goto duplicate_finish;
  }

  for (size_t index = 0; index < indexed_snapshot.size(); ++index) {
    if (IsDuplicateScanCancelRequested()) {
      goto duplicate_finish;
    }
    if (!metadata_ok[index]) {
      continue;
    }
    const IndexedFile& file = indexed_snapshot[index];
    const uint64_t size = metadata_sizes[index];
    size_buckets[size].push_back(DuplicateFileRow{
        file.name,
        file.path,
        size,
        metadata_created[index],
        metadata_modified[index],
    });
  }

  for (auto& size_bucket : size_buckets) {
    if (IsDuplicateScanCancelRequested()) {
      goto duplicate_finish;
    }

    const uint64_t file_size = size_bucket.first;
    std::vector<DuplicateFileRow>& files = size_bucket.second;
    if (files.size() < 2) {
      continue;
    }

    // Fast path for empty files: same content by definition, no disk reads needed.
    if (file_size == 0) {
      DuplicateGroupRow row{};
      row.group_id = BuildDuplicateGroupId(0, 0, group_serial++);
      row.size = 0;
      row.file_count = static_cast<uint32_t>(files.size());
      row.total_bytes = 0;
      const size_t render_count = std::min<size_t>(files.size(), max_files_per_group);
      row.files.reserve(render_count);
      for (size_t i = 0; i < render_count; ++i) {
        row.files.push_back(files[i]);
      }
      groups.push_back(std::move(row));
      g_duplicate_groups_found.store(static_cast<uint64_t>(groups.size()),
                                     std::memory_order_release);
      if (groups.size() >= max_groups) {
        goto duplicate_finish;
      }
      continue;
    }

    // Stage 1: quick signature (size + first chunk + last chunk).
    std::vector<uint64_t> quick_signatures;
    std::vector<uint8_t> quick_ok_flags;
    AddDuplicateProgressTotal(static_cast<uint64_t>(files.size()));
    HashFilesParallel(files, &quick_signatures, &quick_ok_flags, HashDuplicateFileQuick,
                      true);
    if (IsDuplicateScanCancelRequested()) {
      goto duplicate_finish;
    }

    std::unordered_map<uint64_t, std::vector<size_t>> quick_buckets;
    quick_buckets.reserve(files.size());
    for (size_t i = 0; i < files.size(); ++i) {
      if (!quick_ok_flags[i]) {
        continue;
      }
      quick_buckets[quick_signatures[i]].push_back(i);
    }

    for (const auto& quick_bucket : quick_buckets) {
      const std::vector<size_t>& quick_indices = quick_bucket.second;
      if (quick_indices.size() < 2) {
        continue;
      }

      std::vector<DuplicateFileRow> quick_candidates;
      quick_candidates.reserve(quick_indices.size());
      for (const size_t quick_index : quick_indices) {
        quick_candidates.push_back(files[quick_index]);
      }

      // Stage 2: full-file hash only for quick-signature collisions.
      std::vector<uint64_t> full_hashes;
      std::vector<uint8_t> full_ok_flags;
      AddDuplicateProgressTotal(static_cast<uint64_t>(quick_candidates.size()));
      HashFilesParallel(quick_candidates, &full_hashes, &full_ok_flags,
                        HashDuplicateFileFull, true);
      if (IsDuplicateScanCancelRequested()) {
        goto duplicate_finish;
      }

      std::unordered_map<uint64_t, std::vector<size_t>> full_hash_buckets;
      full_hash_buckets.reserve(quick_candidates.size());
      for (size_t i = 0; i < quick_candidates.size(); ++i) {
        if (!full_ok_flags[i]) {
          continue;
        }
        full_hash_buckets[full_hashes[i]].push_back(i);
      }

      for (const auto& hash_bucket : full_hash_buckets) {
        const uint64_t hash_value = hash_bucket.first;
        const std::vector<size_t>& candidate_indices = hash_bucket.second;
        if (candidate_indices.size() < 2) {
          continue;
        }

        std::vector<std::vector<size_t>> verified_clusters;
        verified_clusters.reserve(candidate_indices.size());
        for (const size_t index : candidate_indices) {
          bool matched_cluster = false;
          for (std::vector<size_t>& cluster : verified_clusters) {
            const size_t representative = cluster.front();
            if (AreFilesByteEqual(quick_candidates[index].path,
                                  quick_candidates[representative].path)) {
              cluster.push_back(index);
              matched_cluster = true;
              break;
            }
          }
          if (!matched_cluster) {
            verified_clusters.push_back(std::vector<size_t>{index});
          }
        }

        for (const std::vector<size_t>& cluster : verified_clusters) {
          if (cluster.size() < 2) {
            continue;
          }

          DuplicateGroupRow row{};
          row.group_id = BuildDuplicateGroupId(file_size, hash_value, group_serial++);
          row.size = file_size;
          row.file_count = static_cast<uint32_t>(cluster.size());
          row.total_bytes = file_size * static_cast<uint64_t>(cluster.size());
          const size_t render_count = std::min<size_t>(cluster.size(), max_files_per_group);
          row.files.reserve(render_count);
          for (size_t i = 0; i < render_count; ++i) {
            row.files.push_back(quick_candidates[cluster[i]]);
          }
          groups.push_back(std::move(row));
          g_duplicate_groups_found.store(static_cast<uint64_t>(groups.size()),
                                         std::memory_order_release);

          if (groups.size() >= max_groups) {
            goto duplicate_finish;
          }
        }
      }
    }
  }

duplicate_finish:
  if (!IsDuplicateScanCancelRequested()) {
    const uint64_t total = g_duplicate_progress_total.load(std::memory_order_acquire);
    g_duplicate_progress_done.store(total, std::memory_order_release);
  }

  std::sort(groups.begin(), groups.end(), [](const DuplicateGroupRow& left,
                                             const DuplicateGroupRow& right) {
    const uint64_t left_reclaimable =
        left.size * static_cast<uint64_t>(left.file_count > 0 ? left.file_count - 1 : 0);
    const uint64_t right_reclaimable =
        right.size * static_cast<uint64_t>(right.file_count > 0 ? right.file_count - 1 : 0);
    if (left_reclaimable != right_reclaimable) {
      return left_reclaimable > right_reclaimable;
    }
    return left.file_count > right.file_count;
  });

  return groups;
}

bool scan_mft_internal(const std::wstring& drive_letter, ScanSnapshot* out_snapshot,
                       const bool include_directories, const uint64_t request_token,
                       bool* out_cancelled, std::string* out_error) {
  *out_cancelled = false;
  out_snapshot->files.clear();
  out_snapshot->nodes.clear();
  out_snapshot->root_frn = 0;
  out_snapshot->root_path.clear();
  out_snapshot->journal_id = 0;
  out_snapshot->journal_next_usn = 0;
  out_snapshot->live_updates_supported = false;

  const std::wstring root_path = drive_letter + L":\\";
  const std::wstring volume_path = L"\\\\.\\" + drive_letter + L":";

  HANDLE volume = CreateFileW(
      volume_path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL, nullptr);
  if (volume == INVALID_HANDLE_VALUE) {
    *out_error = BuildWin32ErrorText(
        "Unable to open volume. Run as administrator and ensure the target drive is NTFS.",
        GetLastError());
    return false;
  }

  uint64_t root_frn = 0;
  if (!GetRootFrn(root_path, &root_frn, out_error)) {
    CloseHandle(volume);
    return false;
  }

  DWORD bytes = 0;
  USN_JOURNAL_DATA_V0 journal{};
  bool has_journal = DeviceIoControl(volume, FSCTL_QUERY_USN_JOURNAL, nullptr, 0,
                                     &journal, sizeof(journal), &bytes, nullptr) !=
                     FALSE;
  if (!has_journal) {
    const DWORD query_error = GetLastError();
    if (!IsUsnJournalMissingError(query_error)) {
      CloseHandle(volume);
      *out_error = BuildWin32ErrorText("Failed to query USN journal.", query_error);
      return false;
    }

    CREATE_USN_JOURNAL_DATA create_data{};
    create_data.MaximumSize = 32ULL * 1024ULL * 1024ULL;
    create_data.AllocationDelta = 8ULL * 1024ULL * 1024ULL;
    DWORD create_bytes = 0;
    DeviceIoControl(volume, FSCTL_CREATE_USN_JOURNAL, &create_data,
                    sizeof(create_data), nullptr, 0, &create_bytes, nullptr);

    has_journal = DeviceIoControl(volume, FSCTL_QUERY_USN_JOURNAL, nullptr, 0,
                                  &journal, sizeof(journal), &bytes, nullptr) != FALSE;
  }

  MFT_ENUM_DATA_V0 enum_data{};
  enum_data.StartFileReferenceNumber = 0;
  enum_data.LowUsn = 0;
  enum_data.HighUsn =
      has_journal ? journal.NextUsn : std::numeric_limits<USN>::max();

  constexpr DWORD kBufferSize = 4 * 1024 * 1024;
  std::vector<BYTE> buffer(kBufferSize);
  std::unordered_map<uint64_t, NodeEntry> nodes;
  nodes.reserve(500000);
  uint64_t discovered_files = 0;

  while (true) {
    if (IsIndexingCancelled(request_token)) {
      CloseHandle(volume);
      *out_cancelled = true;
      return false;
    }

    DWORD returned = 0;
    const BOOL ok =
        DeviceIoControl(volume, FSCTL_ENUM_USN_DATA, &enum_data, sizeof(enum_data),
                        buffer.data(), kBufferSize, &returned, nullptr);

    if (!ok) {
      const DWORD error = GetLastError();
      if (error == ERROR_HANDLE_EOF) {
        break;
      }
      CloseHandle(volume);
      *out_error = BuildWin32ErrorText(
          "MFT enumeration failed during DeviceIoControl call.", error);
      return false;
    }

    if (returned <= sizeof(uint64_t)) {
      break;
    }

    enum_data.StartFileReferenceNumber =
        *reinterpret_cast<DWORDLONG*>(buffer.data());

    DWORD offset = sizeof(uint64_t);
    while (offset + sizeof(DWORD) <= returned) {
      const BYTE* record_ptr = buffer.data() + offset;
      const DWORD record_length = *reinterpret_cast<const DWORD*>(record_ptr);
      if (record_length == 0 || offset + record_length > returned) {
        break;
      }

      RawUsnEntry entry{};
      if (ParseUsnRecord(record_ptr, record_length, &entry) && !entry.name.empty()) {
        nodes[entry.frn] =
            NodeEntry{entry.parent_frn, std::move(entry.name), entry.is_directory};
        if (!entry.is_directory) {
          ++discovered_files;
          if ((discovered_files & 0x3FFF) == 0) {
            g_indexed_count.store(discovered_files, std::memory_order_relaxed);
          }
        }
      }

      offset += record_length;
    }
  }

  CloseHandle(volume);
  if (IsIndexingCancelled(request_token)) {
    *out_cancelled = true;
    return false;
  }
  nodes[root_frn] = NodeEntry{root_frn, L"", true};

  std::unordered_map<uint64_t, std::wstring> path_cache;
  path_cache.reserve(nodes.size() / 2 + 1);
  path_cache[root_frn] = root_path;
  std::unordered_set<uint64_t> resolving;
  std::vector<IndexedFile> files;
  files.reserve(nodes.size() / 2 + 1);

  for (const auto& pair : nodes) {
    if (IsIndexingCancelled(request_token)) {
      *out_cancelled = true;
      return false;
    }
    const uint64_t frn = pair.first;
    const NodeEntry& node = pair.second;
    if (node.name.empty() || (node.is_directory && !include_directories)) {
      continue;
    }
    resolving.clear();
    std::wstring full_path;
    const bool resolved = ResolvePathForFrn(frn, root_frn, root_path, nodes, &path_cache,
                                            &resolving, &full_path);
    if (!resolved || full_path.empty()) {
      continue;
    }
    files.push_back(IndexedFile{
        frn,
        node.name,
        std::move(full_path),
        node.is_directory ? L"" : ExtractExtensionLower(node.name),
        node.is_directory,
    });
  }

  out_snapshot->files = std::move(files);
  out_snapshot->nodes = std::move(nodes);
  out_snapshot->root_frn = root_frn;
  out_snapshot->root_path = root_path;
  out_snapshot->live_updates_supported = has_journal;
  if (has_journal) {
    out_snapshot->journal_id = journal.UsnJournalID;
    out_snapshot->journal_next_usn = journal.NextUsn;
  }
  return true;
}

void StartLiveUsnWatcher(const std::wstring& drive_letter, const uint64_t journal_id,
                         const int64_t start_usn) {
  if (journal_id == 0 || start_usn <= 0) {
    return;
  }

  const uint64_t watcher_token =
      g_live_watcher_token.fetch_add(1, std::memory_order_acq_rel) + 1;
  std::thread([drive_letter, journal_id, start_usn, watcher_token]() {
    const std::wstring volume_path = L"\\\\.\\" + drive_letter + L":";
    HANDLE volume = CreateFileW(
        volume_path.c_str(), GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL, nullptr);
    if (volume == INVALID_HANDLE_VALUE) {
      if (!IsLiveWatcherCancelled(watcher_token)) {
        SetLastErrorText(BuildWin32ErrorText(
            "Live updates could not start (unable to open volume).", GetLastError()));
      }
      return;
    }

    READ_USN_JOURNAL_DATA_V0 read_data{};
    read_data.StartUsn = static_cast<USN>(start_usn);
    read_data.ReasonMask = 0xFFFFFFFF;
    read_data.ReturnOnlyOnClose = 0;
    read_data.Timeout = 0;
    read_data.BytesToWaitFor = 0;
    read_data.UsnJournalID = journal_id;

    constexpr DWORD kWatchBufferSize = 1 * 1024 * 1024;
    std::vector<BYTE> buffer(kWatchBufferSize);

    while (!IsLiveWatcherCancelled(watcher_token)) {
      DWORD returned = 0;
      const BOOL ok = DeviceIoControl(
          volume, FSCTL_READ_USN_JOURNAL, &read_data, sizeof(read_data),
          buffer.data(), kWatchBufferSize, &returned, nullptr);
      if (!ok) {
        const DWORD error = GetLastError();
        if (IsLiveWatcherCancelled(watcher_token)) {
          break;
        }

        if (error == ERROR_HANDLE_EOF) {
          std::this_thread::sleep_for(std::chrono::milliseconds(120));
          continue;
        }

        if (error == ERROR_JOURNAL_ENTRY_DELETED ||
            error == ERROR_JOURNAL_DELETE_IN_PROGRESS ||
            error == ERROR_JOURNAL_NOT_ACTIVE || error == ERROR_INVALID_PARAMETER) {
          SetLastErrorText(
              "Live updates paused because the USN journal changed. Click Reindex.");
          break;
        }

        SetLastErrorText(BuildWin32ErrorText(
            "Live updates paused because USN monitoring failed.", error));
        break;
      }

      if (returned < sizeof(USN)) {
        std::this_thread::sleep_for(std::chrono::milliseconds(120));
        continue;
      }

      read_data.StartUsn = *reinterpret_cast<const USN*>(buffer.data());
      if (returned == sizeof(USN)) {
        std::this_thread::sleep_for(std::chrono::milliseconds(120));
        continue;
      }

      std::vector<RawUsnEntry> batch;
      batch.reserve(512);
      DWORD offset = sizeof(USN);
      while (offset + sizeof(DWORD) <= returned) {
        const BYTE* record_ptr = buffer.data() + offset;
        const DWORD record_length = *reinterpret_cast<const DWORD*>(record_ptr);
        if (record_length == 0 || offset + record_length > returned) {
          break;
        }

        RawUsnEntry entry{};
        if (ParseUsnRecord(record_ptr, record_length, &entry) && !entry.name.empty()) {
          batch.push_back(std::move(entry));
        }

        offset += record_length;
      }

      if (batch.empty() || IsLiveWatcherCancelled(watcher_token)) {
        continue;
      }

      std::unique_lock<std::shared_mutex> lock(g_index_mutex);
      ApplyUsnBatchLocked(batch);
    }

    CloseHandle(volume);
  }).detach();
}

std::vector<DriveInfo> list_drives_internal() {
  std::vector<DriveInfo> rows;
  DWORD required = GetLogicalDriveStringsW(0, nullptr);
  if (required == 0) {
    return rows;
  }

  std::vector<wchar_t> raw(static_cast<size_t>(required) + 1, L'\0');
  DWORD written = GetLogicalDriveStringsW(required, raw.data());
  if (written == 0) {
    return rows;
  }

  const wchar_t* cursor = raw.data();
  while (*cursor != L'\0') {
    std::wstring root = cursor;
    cursor += root.size() + 1;
    if (root.size() < 2) {
      continue;
    }

    const wchar_t letter = static_cast<wchar_t>(std::towupper(root[0]));
    if (letter < L'A' || letter > L'Z') {
      continue;
    }

    const std::wstring drive_letter(1, letter);
    const UINT drive_type = GetDriveTypeW(root.c_str());
    wchar_t filesystem_buffer[MAX_PATH] = L"";
    const BOOL has_fs = GetVolumeInformationW(
        root.c_str(), nullptr, 0, nullptr, nullptr, nullptr, filesystem_buffer,
        MAX_PATH);
    const std::wstring filesystem = has_fs ? filesystem_buffer : L"";
    const bool is_ntfs = ToLower(filesystem) == L"ntfs";
    const bool can_open_volume = is_ntfs ? CanOpenVolume(drive_letter) : false;

    rows.push_back(DriveInfo{drive_letter, root, filesystem,
                             DriveTypeToText(drive_type), is_ntfs,
                             can_open_volume});
  }

  return rows;
}

std::vector<std::wstring> ResolveTargetDrivesForIndexing(
    const std::wstring& preferred_drive, const bool scan_all_drives) {
  if (!scan_all_drives) {
    return {preferred_drive};
  }

  std::vector<std::wstring> drives;
  const std::vector<DriveInfo> rows = list_drives_internal();
  drives.reserve(rows.size());
  for (const DriveInfo& row : rows) {
    if (!row.is_ntfs || !row.can_open_volume) {
      continue;
    }
    drives.push_back(row.letter);
  }

  if (drives.empty()) {
    drives.push_back(preferred_drive);
  }
  return drives;
}

}  // namespace

extern "C" __declspec(dllexport) bool omni_start_indexing(
    const char* drive_utf8, const bool include_directories,
    const bool scan_all_drives) {
  const uint64_t request_token =
      g_indexing_request_token.fetch_add(1, std::memory_order_acq_rel) + 1;

  g_is_indexing.store(true, std::memory_order_release);
  g_is_ready.store(false, std::memory_order_release);
  g_indexed_count.store(0, std::memory_order_release);
  SetLastErrorText("");
  StopLiveWatcher();
  const std::wstring drive_letter = NormalizeDriveLetter(drive_utf8);
  g_include_directories.store(include_directories, std::memory_order_release);
  g_scan_all_drives_mode.store(scan_all_drives, std::memory_order_release);

  std::thread(
      [drive_letter, include_directories, scan_all_drives, request_token]() {
        if (scan_all_drives) {
          const std::vector<std::wstring> target_drives =
              ResolveTargetDrivesForIndexing(drive_letter, true);
          std::vector<IndexedFile> merged_files;
          merged_files.reserve(300000);
          std::string combined_error;
          bool has_success = false;

          for (const std::wstring& target_drive : target_drives) {
            if (IsIndexingCancelled(request_token)) {
              return;
            }

            ScanSnapshot snapshot;
            std::string error;
            bool cancelled = false;
            const bool ok = scan_mft_internal(target_drive, &snapshot, include_directories,
                                              request_token, &cancelled, &error);
            if (cancelled || IsIndexingCancelled(request_token)) {
              return;
            }

            if (!ok) {
              if (!error.empty()) {
                if (!combined_error.empty()) {
                  combined_error.append(" | ");
                }
                combined_error.append(WideToUtf8(target_drive));
                combined_error.append(": ");
                combined_error.append(error);
              }
              continue;
            }

            has_success = true;
            for (IndexedFile& file : snapshot.files) {
              merged_files.push_back(std::move(file));
            }
            g_indexed_count.store(static_cast<uint64_t>(merged_files.size()),
                                  std::memory_order_release);
          }

          if (IsIndexingCancelled(request_token)) {
            return;
          }

          if (!has_success) {
            g_is_ready.store(false, std::memory_order_release);
            g_indexed_count.store(0, std::memory_order_release);
            SetLastErrorText(combined_error.empty() ? "Unknown indexing error."
                                                    : combined_error);
          } else {
            const uint64_t indexed_count = static_cast<uint64_t>(merged_files.size());
            {
              std::unique_lock<std::shared_mutex> lock(g_index_mutex);
              ApplyIndexedFilesOnlyLocked(std::move(merged_files));
            }
            g_indexed_count.store(indexed_count, std::memory_order_release);
            g_is_ready.store(true, std::memory_order_release);
            SetLastErrorText("");
          }

          if (!IsIndexingCancelled(request_token)) {
            g_is_indexing.store(false, std::memory_order_release);
          }
          return;
        }

        ScanSnapshot snapshot;
        std::string error;
        bool cancelled = false;
        const bool ok = scan_mft_internal(drive_letter, &snapshot, include_directories,
                                          request_token, &cancelled, &error);
        if (cancelled || IsIndexingCancelled(request_token)) {
          return;
        }

        if (ok) {
          const uint64_t indexed_count = static_cast<uint64_t>(snapshot.files.size());
          {
            std::unique_lock<std::shared_mutex> lock(g_index_mutex);
            ApplyScanSnapshotLocked(&snapshot);
          }
          g_indexed_count.store(indexed_count, std::memory_order_release);
          g_is_ready.store(true, std::memory_order_release);
          SetLastErrorText("");
          if (snapshot.live_updates_supported) {
            StartLiveUsnWatcher(drive_letter, snapshot.journal_id,
                                snapshot.journal_next_usn);
          }
        } else {
          g_is_ready.store(false, std::memory_order_release);
          g_indexed_count.store(0, std::memory_order_release);
          SetLastErrorText(error.empty() ? "Unknown indexing error." : error);
        }

        if (!IsIndexingCancelled(request_token)) {
          g_is_indexing.store(false, std::memory_order_release);
        }
  }).detach();

  return true;
}

extern "C" __declspec(dllexport) bool omni_is_indexing() {
  return g_is_indexing.load(std::memory_order_acquire);
}

extern "C" __declspec(dllexport) bool omni_is_index_ready() {
  return g_is_ready.load(std::memory_order_acquire);
}

extern "C" __declspec(dllexport) uint64_t omni_indexed_file_count() {
  return g_indexed_count.load(std::memory_order_acquire);
}

extern "C" __declspec(dllexport) const char* omni_last_error() {
  thread_local std::string error_cache;
  error_cache = ReadLastErrorText();
  return error_cache.c_str();
}

extern "C" __declspec(dllexport) char* omni_list_drives_json() {
  const std::vector<DriveInfo> rows = list_drives_internal();
  const std::string json = DriveRowsToJson(rows);
  char* out = HeapCopyString(json);
  if (out == nullptr) {
    SetLastErrorText("Failed to allocate drives result buffer.");
  }
  return out;
}

extern "C" __declspec(dllexport) char* omni_search_files_json(
    const char* query_utf8, const char* extension_utf8, uint64_t min_size,
    uint64_t max_size, int64_t min_created_unix, int64_t max_created_unix,
    uint32_t requested_limit) {
  const uint32_t limit =
      (requested_limit == 0) ? 200 : std::min<uint32_t>(requested_limit, 5000);
  const std::wstring query =
      ToLower(Utf8ToWide(query_utf8 == nullptr ? "" : query_utf8));
  const std::wstring extension_filter = NormalizeExtensionFilter(extension_utf8);
  const bool has_extension_filter = !extension_filter.empty();
  const bool extension_targets_directories =
      extension_filter == L"folder" || extension_filter == L"folders" ||
      extension_filter == L"dir" || extension_filter == L"directory";
  const bool has_size_filter =
      min_size > 0 || max_size < std::numeric_limits<uint64_t>::max();
  const bool has_date_filter =
      min_created_unix > std::numeric_limits<int64_t>::min() ||
      max_created_unix < std::numeric_limits<int64_t>::max();
  const bool requires_metadata = has_size_filter || has_date_filter;
  const bool distribute_across_drives =
      g_scan_all_drives_mode.load(std::memory_order_acquire) && limit > 1 &&
      query.empty() && (has_extension_filter || has_size_filter || has_date_filter);

  std::vector<SearchRow> rows;
  rows.reserve(limit);
  std::unordered_map<wchar_t, std::vector<SearchRow>> drive_buckets;
  std::vector<wchar_t> drive_order;
  if (distribute_across_drives) {
    drive_buckets.reserve(16);
    drive_order.reserve(16);
  }

  {
    std::shared_lock<std::shared_mutex> lock(g_index_mutex);
    for (const IndexedFile& file : g_indexed_files) {
      if (!ContainsCaseInsensitive(file.path, query)) {
        continue;
      }
      if (has_extension_filter) {
        if (extension_targets_directories) {
          if (!file.is_directory) {
            continue;
          }
        } else if (file.is_directory || file.extension_lower != extension_filter) {
          continue;
        }
      }

      uint64_t size = 0;
      int64_t created = 0;
      int64_t modified = 0;
      bool metadata_loaded =
          ReadFileMetadata(file.path, &size, &created, &modified);
      if (!metadata_loaded && IsPathMissingError(GetLastError())) {
        // Skip stale entries for files that were deleted or moved.
        continue;
      }

      if (requires_metadata) {
        if (!metadata_loaded) {
          continue;
        }
        if (size < min_size || size > max_size) {
          continue;
        }
        if (created < min_created_unix || created > max_created_unix) {
          continue;
        }
      }

      if (!metadata_loaded) {
        size = 0;
        created = 0;
        modified = 0;
      }

      SearchRow row{
          file.name,
          file.path,
          file.extension_lower,
          size,
          created,
          modified,
          file.is_directory,
      };

      if (distribute_across_drives) {
        const wchar_t bucket_key = DriveBucketKeyFromPath(file.path);
        auto bucket_it = drive_buckets.find(bucket_key);
        if (bucket_it == drive_buckets.end()) {
          drive_order.push_back(bucket_key);
          bucket_it = drive_buckets.emplace(bucket_key, std::vector<SearchRow>{}).first;
          bucket_it->second.reserve(128);
        }
        bucket_it->second.push_back(std::move(row));
      } else {
        rows.push_back(std::move(row));
        if (rows.size() >= limit) {
          break;
        }
      }
    }
  }

  if (distribute_across_drives) {
    rows.clear();
    rows.reserve(limit);
    std::vector<size_t> bucket_offsets(drive_order.size(), 0);
    bool appended = true;
    while (rows.size() < limit && appended) {
      appended = false;
      for (size_t i = 0; i < drive_order.size(); ++i) {
        std::vector<SearchRow>& bucket = drive_buckets[drive_order[i]];
        size_t& offset = bucket_offsets[i];
        if (offset >= bucket.size()) {
          continue;
        }
        rows.push_back(std::move(bucket[offset]));
        ++offset;
        appended = true;
        if (rows.size() >= limit) {
          break;
        }
      }
    }
  }

  const std::string json = SearchRowsToJson(rows);
  char* out = HeapCopyString(json);
  if (out == nullptr) {
    SetLastErrorText("Failed to allocate result buffer.");
  }
  return out;
}

extern "C" __declspec(dllexport) char* omni_find_duplicates_json(
    uint64_t min_size, uint32_t requested_max_groups,
    uint32_t requested_max_files_per_group) {
  if (!g_is_ready.load(std::memory_order_acquire)) {
    SetLastErrorText("Index is not ready yet. Wait for indexing to finish.");
    return nullptr;
  }

  const bool already_running =
      g_duplicate_scan_running.exchange(true, std::memory_order_acq_rel);
  if (already_running) {
    SetLastErrorText("Duplicate scan is already running.");
    return nullptr;
  }

  g_duplicate_cancel_requested.store(false, std::memory_order_release);
  ResetDuplicateProgress();
  const uint64_t effective_min_size =
      min_size == 0 ? 1ULL * 1024ULL * 1024ULL : min_size;
  const uint32_t max_groups = std::clamp<uint32_t>(requested_max_groups, 1, 1000);
  const uint32_t max_files_per_group =
      std::clamp<uint32_t>(requested_max_files_per_group, 2, 400);

  const std::vector<DuplicateGroupRow> groups = find_duplicates_internal(
      effective_min_size, max_groups, max_files_per_group);
  const bool cancelled = IsDuplicateScanCancelRequested();
  g_duplicate_scan_running.store(false, std::memory_order_release);

  if (cancelled) {
    g_duplicate_cancel_requested.store(false, std::memory_order_release);
    SetLastErrorText("Duplicate scan cancelled.");
    return nullptr;
  }

  const std::string json = DuplicateGroupsToJson(groups);
  char* out = HeapCopyString(json);
  if (out == nullptr) {
    SetLastErrorText("Failed to allocate duplicate results buffer.");
  }
  g_duplicate_cancel_requested.store(false, std::memory_order_release);
  return out;
}

extern "C" __declspec(dllexport) bool omni_cancel_duplicate_scan() {
  if (!g_duplicate_scan_running.load(std::memory_order_acquire)) {
    return false;
  }
  g_duplicate_cancel_requested.store(true, std::memory_order_release);
  return true;
}

extern "C" __declspec(dllexport) char* omni_duplicate_scan_status_json() {
  const std::string json = DuplicateScanStatusToJson();
  char* out = HeapCopyString(json);
  if (out == nullptr) {
    SetLastErrorText("Failed to allocate duplicate status buffer.");
  }
  return out;
}

extern "C" __declspec(dllexport) char* scan_mft(const char* drive_utf8) {
  ScanSnapshot snapshot;
  std::string error;
  bool cancelled = false;
  const bool ok = scan_mft_internal(
      NormalizeDriveLetter(drive_utf8), &snapshot, false, 0, &cancelled, &error);
  if (!ok) {
    SetLastErrorText(error.empty() ? "scan_mft failed." : error);
    return nullptr;
  }

  const std::string json = BasicFilesToJson(snapshot.files);
  char* out = HeapCopyString(json);
  if (out == nullptr) {
    SetLastErrorText("Failed to allocate scan_mft result buffer.");
  }
  return out;
}

extern "C" __declspec(dllexport) void omni_free_string(char* value) {
  if (value != nullptr) {
    std::free(value);
  }
}
