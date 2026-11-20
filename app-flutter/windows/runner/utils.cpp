#include "utils.h"

#include <flutter_windows.h>
#include <io.h>
#include <shellapi.h>
#include <stdio.h>
#include <windows.h>

#include <iostream>

void CreateAndAttachConsole() {
  if (::AllocConsole()) {
    FILE *unused;
    if (freopen_s(&unused, "CONOUT$", "w", stdout)) {
      _dup2(_fileno(stdout), 1);
    }
    if (freopen_s(&unused, "CONOUT$", "w", stderr)) {
      _dup2(_fileno(stdout), 2);
    }
    std::ios::sync_with_stdio();
    FlutterDesktopResyncOutputStreams();
  }
}

std::vector<std::string> GetCommandLineArguments() {
  // Convert the UTF-16 command line arguments to UTF-8 for the Engine to use.
  int argc;
  wchar_t** argv = ::CommandLineToArgvW(::GetCommandLineW(), &argc);
  if (argv == nullptr) {
    return std::vector<std::string>();
  }

  std::vector<std::string> command_line_arguments;

  // Skip the first argument as it's the binary name.
  for (int i = 1; i < argc; i++) {
    command_line_arguments.push_back(Utf8FromUtf16(argv[i]));
  }

  ::LocalFree(argv);

  return command_line_arguments;
}

std::string Utf8FromUtf16(const wchar_t* utf16_string) {
  if (utf16_string == nullptr) {
    return std::string();
  }
  // First, find the length of the string with a safe upper bound (CWE-126).
  // UNICODE_STRING_MAX_CHARS (32767) is the maximum length of a UNICODE_STRING.
  int input_length = static_cast<int>(wcsnlen(utf16_string, UNICODE_STRING_MAX_CHARS));
  // Now use that bounded length to determine the required buffer size.
  // When an explicit length is passed, WideCharToMultiByte does not include
  // the null terminator in its returned size.
  int target_length = ::WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, utf16_string,
      input_length, nullptr, 0, nullptr, nullptr);
  std::string utf8_string;
  if (target_length == 0 || static_cast<size_t>(target_length) > utf8_string.max_size()) {
    return utf8_string;
  }
  utf8_string.resize(target_length);
  int converted_length = ::WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, utf16_string,
      input_length, utf8_string.data(), target_length, nullptr, nullptr);
  if (converted_length == 0) {
    return std::string();
  }
  return utf8_string;
}

namespace {

// HDROP → UTF-8 路径（不释放句柄：剪贴板里的 CF_HDROP 归剪贴板所有）。
std::vector<std::string> QueryDropPaths(HDROP drop) {
  std::vector<std::string> paths;
  if (drop == nullptr) {
    return paths;
  }
  const UINT count = ::DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
  for (UINT index = 0; index < count; ++index) {
    const UINT length = ::DragQueryFileW(drop, index, nullptr, 0);
    if (length == 0) {
      continue;
    }
    std::wstring path(length + 1, L'\0');
    const UINT written =
        ::DragQueryFileW(drop, index, path.data(), length + 1);
    if (written == 0) {
      continue;
    }
    path.resize(written);
    paths.push_back(Utf8FromUtf16(path.c_str()));
  }
  return paths;
}

}  // namespace

std::vector<std::string> TakeDropPaths(HDROP drop) {
  std::vector<std::string> paths = QueryDropPaths(drop);
  if (drop != nullptr) {
    ::DragFinish(drop);
  }
  return paths;
}

std::vector<std::string> ClipboardFilePaths() {
  std::vector<std::string> paths;
  if (::OpenClipboard(nullptr) == 0) {
    return paths;
  }
  HANDLE handle = ::GetClipboardData(CF_HDROP);
  if (handle != nullptr) {
    // 剪贴板里的 CF_HDROP 归剪贴板所有：读完 **不要** DragFinish / GlobalFree。
    paths = QueryDropPaths(reinterpret_cast<HDROP>(handle));
  }
  ::CloseClipboard();
  return paths;
}
