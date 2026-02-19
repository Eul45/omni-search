#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>
#include <winioctl.h>

#include <algorithm>
#include <atomic>
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
};

struct NodeEntry {
  uint64_t parent_frn;
  std::wstring name;
  bool is_directory;
};

struct IndexedFile {
  std::wstring name;
  std::wstring path;
  std::wstring extension_lower;
};

struct SearchRow {
  std::wstring name;
  std::wstring path;
  std::wstring extension;
  uint64_t size;
  int64_t created_unix;
  int64_t modified_unix;
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
std::atomic<bool> g_is_indexing{false};
std::atomic<bool> g_is_ready{false};
std::atomic<uint64_t> g_indexed_count{0};
std::mutex g_error_mutex;
std::string g_last_error;

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
  json.reserve(rows.size() * 160);
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
    json.push_back('}');
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
  json.reserve(files.size() * 96);
  json.push_back('[');
  for (size_t i = 0; i < files.size(); ++i) {
    if (i > 0) {
      json.push_back(',');
    }
    json.append("{\"name\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(files[i].name));
    json.append("\",\"path\":\"");
    AppendEscapedJsonString(&json, WideToUtf8(files[i].path));
    json.append("\"}");
  }
  json.push_back(']');
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
    out->name.assign(name_ptr, v3->FileNameLength / sizeof(wchar_t));
    return true;
  }
#endif

  return false;
}

std::wstring ResolvePathForFrn(
    uint64_t frn, uint64_t root_frn, const std::wstring& root_path,
    const std::unordered_map<uint64_t, NodeEntry>& nodes,
    std::unordered_map<uint64_t, std::wstring>* cache,
    std::unordered_set<uint64_t>* resolving) {
  const auto cached = cache->find(frn);
  if (cached != cache->end()) {
    return cached->second;
  }

  if (frn == root_frn) {
    return root_path;
  }

  const auto node_it = nodes.find(frn);
  if (node_it == nodes.end()) {
    return root_path;
  }

  if (!resolving->insert(frn).second) {
    return root_path;
  }
  const std::wstring parent_path =
      ResolvePathForFrn(node_it->second.parent_frn, root_frn, root_path, nodes, cache,
                        resolving);
  resolving->erase(frn);

  std::wstring full_path = parent_path;
  if (!full_path.empty() && full_path.back() != L'\\') {
    full_path.push_back(L'\\');
  }
  full_path.append(node_it->second.name);
  (*cache)[frn] = full_path;
  return full_path;
}

bool scan_mft_internal(const std::wstring& drive_letter, std::vector<IndexedFile>* out_files,
                       std::string* out_error) {
  out_files->clear();

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
  nodes[root_frn] = NodeEntry{root_frn, L"", true};

  std::unordered_map<uint64_t, std::wstring> path_cache;
  path_cache.reserve(nodes.size() / 2 + 1);
  path_cache[root_frn] = root_path;
  std::unordered_set<uint64_t> resolving;
  std::vector<IndexedFile> files;
  files.reserve(nodes.size() / 2 + 1);

  for (const auto& pair : nodes) {
    const uint64_t frn = pair.first;
    const NodeEntry& node = pair.second;
    if (node.is_directory || node.name.empty()) {
      continue;
    }
    resolving.clear();
    std::wstring full_path =
        ResolvePathForFrn(frn, root_frn, root_path, nodes, &path_cache, &resolving);
    if (full_path.empty()) {
      continue;
    }
    files.push_back(
        IndexedFile{node.name, std::move(full_path), ExtractExtensionLower(node.name)});
  }

  *out_files = std::move(files);
  return true;
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

}  // namespace

extern "C" __declspec(dllexport) bool omni_start_indexing(const char* drive_utf8) {
  const bool already_indexing =
      g_is_indexing.exchange(true, std::memory_order_acq_rel);
  if (already_indexing) {
    SetLastErrorText("Indexing is already running.");
    return false;
  }

  g_is_ready.store(false, std::memory_order_release);
  g_indexed_count.store(0, std::memory_order_release);
  SetLastErrorText("");
  const std::wstring drive_letter = NormalizeDriveLetter(drive_utf8);

  std::thread([drive_letter]() {
    std::vector<IndexedFile> refreshed_index;
    std::string error;
    const bool ok = scan_mft_internal(drive_letter, &refreshed_index, &error);
    if (ok) {
      const uint64_t indexed_count =
          static_cast<uint64_t>(refreshed_index.size());
      {
        std::unique_lock<std::shared_mutex> lock(g_index_mutex);
        g_indexed_files.swap(refreshed_index);
      }
      g_indexed_count.store(indexed_count, std::memory_order_release);
      g_is_ready.store(true, std::memory_order_release);
      SetLastErrorText("");
    } else {
      g_is_ready.store(false, std::memory_order_release);
      g_indexed_count.store(0, std::memory_order_release);
      SetLastErrorText(error.empty() ? "Unknown indexing error." : error);
    }
    g_is_indexing.store(false, std::memory_order_release);
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
  const bool has_size_filter =
      min_size > 0 || max_size < std::numeric_limits<uint64_t>::max();
  const bool has_date_filter =
      min_created_unix > std::numeric_limits<int64_t>::min() ||
      max_created_unix < std::numeric_limits<int64_t>::max();
  const bool requires_metadata = has_size_filter || has_date_filter;

  std::vector<SearchRow> rows;
  rows.reserve(limit);

  {
    std::shared_lock<std::shared_mutex> lock(g_index_mutex);
    for (const IndexedFile& file : g_indexed_files) {
      if (!ContainsCaseInsensitive(file.path, query)) {
        continue;
      }
      if (has_extension_filter && file.extension_lower != extension_filter) {
        continue;
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

      rows.push_back(SearchRow{file.name, file.path, file.extension_lower, size, created,
                               modified});
      if (rows.size() >= limit) {
        break;
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

extern "C" __declspec(dllexport) char* scan_mft(const char* drive_utf8) {
  std::vector<IndexedFile> files;
  std::string error;
  const bool ok = scan_mft_internal(NormalizeDriveLetter(drive_utf8), &files, &error);
  if (!ok) {
    SetLastErrorText(error.empty() ? "scan_mft failed." : error);
    return nullptr;
  }

  const std::string json = BasicFilesToJson(files);
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
