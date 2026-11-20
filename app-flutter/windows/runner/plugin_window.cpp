#include "plugin_window.h"

#include <flutter/standard_method_codec.h>
#include <shellapi.h>
#include <windowsx.h>

#include <algorithm>
#include <cmath>
#include <cwctype>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <system_error>
#include <vector>

#include "utils.h"

namespace {

// Resize grip width, in logical pixels (scaled by the window DPI).
constexpr int kResizeBorder = 8;

// Gap between the attached preview and its anchor window's left edge.
// 0 = 严密贴合（用户 2026-11-14 要求）：两块窗口看起来是一整块。
constexpr int kAnchorGap = 0;

constexpr int kMaxPluginWindows = 2;

// 预览窗与 AI 窗的默认形态（逻辑像素，用户 2026-11-14 指定）：
// preview 1280×720 贴左，AI 窗收窄到 720×720，两者之间的粘合线 = AI 窗左缘。
constexpr int kPreviewLogicalWidth = 1280;
constexpr int kChatDockedLogicalWidth = 720;
constexpr int kDockedLogicalHeight = 720;
// 预览关掉后 AI 窗回到规格宽度（`lib/platform/plugin_window.dart` 的 chat 档）。
constexpr int kChatSpecLogicalWidth = 1280;

double DpiScaleOf(HWND hwnd) {
  const UINT dpi = ::GetDpiForWindow(hwnd);
  return dpi > 0 ? static_cast<double>(dpi) / 96.0 : 1.0;
}

int LogicalToPhysical(int logical, double scale) {
  return static_cast<int>(std::lround(logical * scale));
}

RECT WorkAreaOf(HWND hwnd) {
  RECT work = {0, 0, 1280, 720};
  HMONITOR monitor = ::MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
  MONITORINFO info = {};
  info.cbSize = sizeof(MONITORINFO);
  if (::GetMonitorInfo(monitor, &info)) {
    work = info.rcWork;
  }
  return work;
}

// Makes the window background transparent (same recipe the attachment drawer
// uses): the sandbox preview needs it for the "pull out from the glue line"
// reveal — the panel itself paints the opaque surface.
void MakeWindowTransparent(HWND hwnd) {
  HMODULE user32 = ::GetModuleHandleW(L"user32.dll");
  if (user32 == nullptr) {
    return;
  }
  using SetWindowCompositionAttributeFn = BOOL(WINAPI*)(HWND, void*);
  auto set_attribute = reinterpret_cast<SetWindowCompositionAttributeFn>(
      ::GetProcAddress(user32, "SetWindowCompositionAttribute"));
  if (set_attribute == nullptr) {
    return;
  }

  struct AccentPolicy {
    int state;
    int flags;
    int color;
    int animation_id;
  };
  struct WindowCompositionAttributeData {
    int attribute;
    void* data;
    size_t size;
  };

  AccentPolicy policy = {2 /* ACCENT_ENABLE_TRANSPARENTGRADIENT */, 2, 0, 0};
  WindowCompositionAttributeData data = {19 /* WCA_ACCENT_POLICY */, &policy,
                                         sizeof(policy)};
  set_attribute(hwnd, &data);
}

// UTF-8 (from Dart) -> UTF-16 (Win32 API).
std::wstring WideFromUtf8(const std::string& text) {
  if (text.empty()) {
    return std::wstring();
  }
  const int length = ::MultiByteToWideChar(CP_UTF8, 0, text.c_str(),
                                           static_cast<int>(text.size()),
                                           nullptr, 0);
  if (length <= 0) {
    return std::wstring();
  }
  std::wstring out(static_cast<size_t>(length), L'\0');
  ::MultiByteToWideChar(CP_UTF8, 0, text.c_str(), static_cast<int>(text.size()),
                        out.data(), length);
  return out;
}

// UTF-16 (Win32 API) -> UTF-8 (回给 Dart).
std::string Utf8FromWide(const std::wstring& text) {
  if (text.empty()) {
    return std::string();
  }
  const int length = ::WideCharToMultiByte(CP_UTF8, 0, text.c_str(),
                                           static_cast<int>(text.size()),
                                           nullptr, 0, nullptr, nullptr);
  if (length <= 0) {
    return std::string();
  }
  std::string out(static_cast<size_t>(length), '\0');
  ::WideCharToMultiByte(CP_UTF8, 0, text.c_str(), static_cast<int>(text.size()),
                        out.data(), length, nullptr, nullptr);
  return out;
}

}  // namespace

// ── 应用预览：找窗口 / 抓一帧 / 停靠 ───────────────────────────────
//
// **口径（如实）**：抓帧只读、不回传任何数据给被捕获的应用；能拿到的是
// PrintWindow(PW_RENDERFULLCONTENT) 的画面，部分 GPU 合成应用会返回黑屏，
// 主动设了 SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) 的应用也抓不到。
// 停靠 = 把真实窗口挪到面板区域（不重父化），失败时如实报错。
namespace {

std::wstring ExeBaseNameLower(const std::string& target) {
  std::wstring wide = WideFromUtf8(target);
  const size_t slash = wide.find_last_of(L"\\/");
  std::wstring base = slash == std::wstring::npos ? wide : wide.substr(slash + 1);
  std::transform(base.begin(), base.end(), base.begin(),
                 [](wchar_t ch) { return static_cast<wchar_t>(::towlower(ch)); });
  if (!base.empty() && base.find(L'.') == std::wstring::npos) {
    base += L".exe";
  }
  return base;
}

struct FindWindowContext {
  std::wstring exe_lower;
  DWORD self_pid = 0;
  HWND found = nullptr;
};

BOOL CALLBACK FindWindowProc(HWND hwnd, LPARAM lparam) {
  auto* context = reinterpret_cast<FindWindowContext*>(lparam);
  if (::IsWindowVisible(hwnd) == 0) {
    return TRUE;
  }
  DWORD pid = 0;
  ::GetWindowThreadProcessId(hwnd, &pid);
  if (pid == 0 || pid == context->self_pid) {
    return TRUE;
  }
  HANDLE process = ::OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
  if (process == nullptr) {
    return TRUE;
  }
  wchar_t path[MAX_PATH * 2];
  DWORD size = static_cast<DWORD>(std::size(path));
  bool match = false;
  if (::QueryFullProcessImageNameW(process, 0, path, &size) != 0) {
    std::wstring full(path, size);
    const size_t slash = full.find_last_of(L"\\/");
    std::wstring base = slash == std::wstring::npos ? full : full.substr(slash + 1);
    std::transform(base.begin(), base.end(), base.begin(),
                   [](wchar_t ch) { return static_cast<wchar_t>(::towlower(ch)); });
    match = base == context->exe_lower;
  }
  ::CloseHandle(process);
  if (match) {
    context->found = hwnd;
    return FALSE;
  }
  return TRUE;
}

HWND FindWindowByExe(const std::wstring& exe_lower) {
  if (exe_lower.empty()) {
    return nullptr;
  }
  FindWindowContext context;
  context.exe_lower = exe_lower;
  context.self_pid = ::GetCurrentProcessId();
  ::EnumWindows(FindWindowProc, reinterpret_cast<LPARAM>(&context));
  return context.found;
}

std::wstring AppPreviewDir() {
  wchar_t exe_path[MAX_PATH * 2];
  const DWORD size = ::GetModuleFileNameW(nullptr, exe_path,
                                          static_cast<DWORD>(std::size(exe_path)));
  std::wstring root(exe_path, size);
  const size_t slash = root.find_last_of(L"\\/");
  if (slash != std::wstring::npos) {
    root = root.substr(0, slash);
  }
  return root + L"\\temp\\app-preview";
}

// PrintWindow 抓一帧 → 24 位 BMP（Flutter 能直接解码；32 位 BMP 的 alpha
// 会被当成透明，所以这里显式落 24 位）。
bool CaptureWindowToBmp(HWND target, const std::wstring& path, int* out_width,
                        int* out_height, std::string* error) {
  RECT rect;
  if (::GetWindowRect(target, &rect) == 0) {
    *error = "取窗口矩形失败";
    return false;
  }
  const int width = static_cast<int>(rect.right - rect.left);
  const int height = static_cast<int>(rect.bottom - rect.top);
  if (width <= 0 || height <= 0) {
    *error = "窗口尺寸为 0";
    return false;
  }
  // 4K 应用也只抓 1600 宽：降采样后再落 BMP，减少 IO 与解码开销。
  const int max_width = 1600;
  int out_w = width;
  int out_h = height;
  if (width > max_width) {
    out_w = max_width;
    out_h = std::max(1, static_cast<int>(static_cast<long long>(height) * max_width / width));
  }

  HDC screen = ::GetDC(nullptr);
  HDC mem = ::CreateCompatibleDC(screen);
  BITMAPINFO src_info = {};
  src_info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
  src_info.bmiHeader.biWidth = width;
  src_info.bmiHeader.biHeight = height;
  src_info.bmiHeader.biPlanes = 1;
  src_info.bmiHeader.biBitCount = 32;
  src_info.bmiHeader.biCompression = BI_RGB;
  void* src_bits = nullptr;
  HBITMAP src = ::CreateDIBSection(mem, &src_info, DIB_RGB_COLORS, &src_bits,
                                   nullptr, 0);
  if (src == nullptr || src_bits == nullptr) {
    ::DeleteDC(mem);
    ::ReleaseDC(nullptr, screen);
    *error = "创建位图失败";
    return false;
  }
  HGDIOBJ old_src = ::SelectObject(mem, src);
  // 2 = PW_RENDERFULLCONTENT：走 DWM，能抓到 GPU 渲染的内容（Win8.1+）。
  const BOOL printed = ::PrintWindow(target, mem, 2);
  (void)printed;  // 有的应用返回 0 但画面仍然画出来了，这里不据此判失败。

  const int stride = ((out_w * 24 + 31) / 32) * 4;
  BITMAPINFO dst_info = {};
  dst_info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
  dst_info.bmiHeader.biWidth = out_w;
  dst_info.bmiHeader.biHeight = out_h;
  dst_info.bmiHeader.biPlanes = 1;
  dst_info.bmiHeader.biBitCount = 24;
  dst_info.bmiHeader.biCompression = BI_RGB;
  void* dst_bits = nullptr;
  HBITMAP dst = ::CreateDIBSection(mem, &dst_info, DIB_RGB_COLORS, &dst_bits,
                                   nullptr, 0);
  bool ok = false;
  if (dst != nullptr && dst_bits != nullptr) {
    HGDIOBJ old_dst = ::SelectObject(mem, dst);
    ::SetStretchBltMode(mem, HALFTONE);
    ::StretchBlt(mem, 0, 0, out_w, out_h, mem, 0, 0, width, height, SRCCOPY);
    ::SelectObject(mem, old_dst);

    BITMAPFILEHEADER header = {};
    BITMAPINFOHEADER info = dst_info.bmiHeader;
    header.bfType = 0x4D42;  // "BM"
    header.bfOffBits = sizeof(BITMAPFILEHEADER) + sizeof(BITMAPINFOHEADER);
    header.bfSize = header.bfOffBits + static_cast<DWORD>(stride) * out_h;
    std::ofstream out(std::filesystem::path(path), std::ios::binary);
    if (out) {
      out.write(reinterpret_cast<const char*>(&header), sizeof(header));
      out.write(reinterpret_cast<const char*>(&info), sizeof(info));
      out.write(reinterpret_cast<const char*>(dst_bits),
                static_cast<std::streamsize>(stride) * out_h);
      ok = out.good();
    }
    ::DeleteObject(dst);
  }
  ::SelectObject(mem, old_src);
  ::DeleteObject(src);
  ::DeleteDC(mem);
  ::ReleaseDC(nullptr, screen);

  if (!ok) {
    *error = "写 BMP 失败";
    return false;
  }
  *out_width = out_w;
  *out_height = out_h;
  return true;
}

}  // namespace

PluginWindow::PluginWindow(std::string id,
                           std::string title,
                           int width,
                           int height,
                           int min_width,
                           int min_height,
                           HWND main_window,
                           HWND anchor)
    : id_(std::move(id)),
      title_(std::move(title)),
      width_(width),
      height_(height),
      min_width_(min_width),
      min_height_(min_height),
      main_window_(main_window),
      anchor_(anchor) {}

bool PluginWindow::CreateHostWindow() {
  if (GetHandle() != nullptr) {
    ::ShowWindow(GetHandle(), SW_SHOWNORMAL);
    // 依附窗口不抢焦点：它只是 AI 窗口旁边的一块面板。
    if (anchor_ == nullptr) {
      ::SetForegroundWindow(GetHandle());
    }
    return true;
  }
  notified_ = false;
  Win32Window::Point origin(0, 0);
  Win32Window::Size size(static_cast<unsigned int>(width_),
                         static_cast<unsigned int>(height_));
  if (!Create(WideFromUtf8(title_), origin, size)) {
    return false;
  }
  SetQuitOnClose(false);
  ApplyBorderlessAndPlace();
  if (id_ == "sandbox") {
    // 预览窗：背景透明，面板自己画底色 ⇒ 展开动画能「从粘合线拉开」。
    MakeWindowTransparent(GetHandle());
  }
  return true;
}

void PluginWindow::ApplyBorderlessAndPlace() {
  HWND hwnd = GetHandle();
  if (hwnd == nullptr) {
    return;
  }
  // Borderless like v1's plugin windows (the panel draws its own header).
  LONG_PTR style = ::GetWindowLongPtr(hwnd, GWL_STYLE);
  style &= ~(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX |
             WS_MAXIMIZEBOX);
  style |= WS_POPUP | WS_CLIPCHILDREN;
  ::SetWindowLongPtr(hwnd, GWL_STYLE, style);

  const bool attached = anchor_ != nullptr;
  LONG_PTR ex_style = ::GetWindowLongPtr(hwnd, GWL_EXSTYLE);
  ex_style &= ~WS_EX_TOPMOST;
  if (attached) {
    // 依附窗口（AI 预览）：不占任务栏、不进 Alt+Tab —— 它属于 AI 窗口的一部分。
    ex_style = (ex_style | WS_EX_TOOLWINDOW) & ~WS_EX_APPWINDOW;
  } else {
    // WS_EX_APPWINDOW: an unowned WS_POPUP window is otherwise absent from the
    // taskbar, which would make a 1280x720 plugin window hard to get back.
    ex_style = (ex_style & ~WS_EX_TOOLWINDOW) | WS_EX_APPWINDOW;
  }
  ::SetWindowLongPtr(hwnd, GWL_EXSTYLE, ex_style);

  if (attached) {
    PlaceBesideAnchor();
    return;
  }

  // Place: centred on the main window's monitor.
  RECT work = {0, 0, 1280, 720};
  if (main_window_ != nullptr) {
    HMONITOR monitor = ::MonitorFromWindow(main_window_, MONITOR_DEFAULTTONEAREST);
    MONITORINFO info = {};
    info.cbSize = sizeof(MONITORINFO);
    if (::GetMonitorInfo(monitor, &info)) {
      work = info.rcWork;
    }
  }
  const int w = width_;
  const int h = height_;
  const int work_left = static_cast<int>(work.left);
  const int work_top = static_cast<int>(work.top);
  const int work_width = static_cast<int>(work.right - work.left);
  const int work_height = static_cast<int>(work.bottom - work.top);
  int x = work_left + (work_width - w) / 2;
  int y = work_top + (work_height - h) / 2;
  x = std::max(x, work_left);
  y = std::max(y, work_top);
  if (x + w > work_left + work_width) {
    x = std::max(work_left, work_left + work_width - w);
  }
  if (y + h > work_top + work_height) {
    y = std::max(work_top, work_top + work_height - h);
  }
  ::SetWindowPos(hwnd, HWND_TOP, x, y, w, h,
                 SWP_FRAMECHANGED | SWP_NOOWNERZORDER);
}

// 依附态：贴在 anchor（AI 对话框）的**左侧**，跟着它移动 / 缩放 / 显隐。
// 左边放不下时退回右侧，保证永远和 AI 窗相邻，而不是飘到屏幕中间。
void PluginWindow::PlaceBesideAnchor() {
  HWND hwnd = GetHandle();
  if (hwnd == nullptr) {
    return;
  }
  HWND anchor = anchor_;
  if (anchor == nullptr || ::IsWindow(anchor) == 0 ||
      ::IsWindowVisible(anchor) == 0 || ::IsIconic(anchor) != 0) {
    ::ShowWindow(hwnd, SW_HIDE);
    return;
  }

  RECT self;
  ::GetWindowRect(hwnd, &self);
  int w = static_cast<int>(self.right - self.left);
  if (w <= 0) {
    w = width_;
  }

  RECT anchor_rect;
  ::GetWindowRect(anchor, &anchor_rect);
  // 严密贴合：与 AI 窗口**同高、同顶边**，像附件抽屉一样连成一整块。
  int h = static_cast<int>(anchor_rect.bottom - anchor_rect.top);
  if (h <= 0) {
    h = height_;
  }
  RECT work = {0, 0, 1280, 720};
  HMONITOR monitor = ::MonitorFromWindow(anchor, MONITOR_DEFAULTTONEAREST);
  MONITORINFO info = {};
  info.cbSize = sizeof(MONITORINFO);
  if (::GetMonitorInfo(monitor, &info)) {
    work = info.rcWork;
  }
  const int work_left = static_cast<int>(work.left);
  const int work_top = static_cast<int>(work.top);
  const int work_right = static_cast<int>(work.right);
  const int work_bottom = static_cast<int>(work.bottom);

  int x = static_cast<int>(anchor_rect.left) - kAnchorGap - w;
  if (x < work_left) {
    x = static_cast<int>(anchor_rect.right) + kAnchorGap;
  }
  int y = static_cast<int>(anchor_rect.top);
  x = std::max(x, work_left);
  y = std::max(y, work_top);
  if (x + w > work_right) {
    x = std::max(work_left, work_right - w);
  }
  if (y + h > work_bottom) {
    h = std::max(1, work_bottom - y);
  }
  // 第一帧之前不要显示（会闪白底）；之后才允许在锚点重新出现时把它带回来。
  ::SetWindowPos(hwnd, HWND_TOP, x, y, w, h,
                 SWP_NOACTIVATE | (revealed_ ? SWP_SHOWWINDOW : 0));
}

void PluginWindow::SyncToAnchor() {
  if (anchor_ != nullptr) {
    PlaceBesideAnchor();
  }
}

void PluginWindow::SetBoundsPhysical(int x, int y, int width, int height) {
  HWND hwnd = GetHandle();
  if (hwnd == nullptr) {
    return;
  }
  ::SetWindowPos(hwnd, nullptr, x, y, width, height,
                 SWP_NOZORDER | SWP_NOACTIVATE);
}

// ── 手动缩放 ─────────────────────────────────────────────────────
//
// 无边框插件窗没有 WS_THICKFRAME，系统的 sizing loop 不会启动（用户实测：
// 拖边缘没有反应、也没有光标）。而 Flutter 的 view 子窗铺满整个客户区，
// 鼠标永远落在子窗上，父窗收不到 WM_NCHITTEST —— 所以真正的入口在 Dart：
// 面板自己在四条边 / 四个角放 6px 的手势条（同时给出缩放光标），按下后
// 依次调 "resizeStart" / "resizeUpdate" / "resizeEnd"，这里只按全局光标
// 位置改窗口矩形。

void PluginWindow::BeginResize(int hit, bool capture) {
  HWND hwnd = GetHandle();
  if (hwnd == nullptr) {
    return;
  }
  resizing_ = true;
  resize_hit_ = hit;
  ::GetCursorPos(&resize_start_cursor_);
  ::GetWindowRect(hwnd, &resize_start_rect_);
  if (capture) {
    ::SetCapture(hwnd);
  }
}

void PluginWindow::UpdateResize() {
  if (!resizing_) {
    return;
  }
  HWND hwnd = GetHandle();
  if (hwnd == nullptr) {
    return;
  }
  POINT cursor;
  ::GetCursorPos(&cursor);
  const int dx = cursor.x - resize_start_cursor_.x;
  const int dy = cursor.y - resize_start_cursor_.y;
  RECT rect = resize_start_rect_;
  const UINT dpi = ::GetDpiForWindow(hwnd);
  const int min_w = ::MulDiv(min_width_, static_cast<int>(dpi), 96);
  const int min_h = ::MulDiv(min_height_, static_cast<int>(dpi), 96);

  switch (resize_hit_) {
    case HTLEFT:
      rect.left = std::min(rect.left + dx, rect.right - min_w);
      break;
    case HTRIGHT:
      rect.right = std::max(rect.right + dx, rect.left + min_w);
      break;
    case HTTOP:
      rect.top = std::min(rect.top + dy, rect.bottom - min_h);
      break;
    case HTBOTTOM:
      rect.bottom = std::max(rect.bottom + dy, rect.top + min_h);
      break;
    case HTTOPLEFT:
      rect.left = std::min(rect.left + dx, rect.right - min_w);
      rect.top = std::min(rect.top + dy, rect.bottom - min_h);
      break;
    case HTTOPRIGHT:
      rect.right = std::max(rect.right + dx, rect.left + min_w);
      rect.top = std::min(rect.top + dy, rect.bottom - min_h);
      break;
    case HTBOTTOMLEFT:
      rect.left = std::min(rect.left + dx, rect.right - min_w);
      rect.bottom = std::max(rect.bottom + dy, rect.top + min_h);
      break;
    case HTBOTTOMRIGHT:
      rect.right = std::max(rect.right + dx, rect.left + min_w);
      rect.bottom = std::max(rect.bottom + dy, rect.top + min_h);
      break;
    default:
      return;
  }
  ::SetWindowPos(hwnd, nullptr, rect.left, rect.top,
                 rect.right - rect.left, rect.bottom - rect.top,
                 SWP_NOZORDER | SWP_NOACTIVATE);
}

void PluginWindow::EndResize() {
  if (!resizing_) {
    return;
  }
  resizing_ = false;
  resize_hit_ = 0;
  if (::GetCapture() == GetHandle()) {
    ::ReleaseCapture();
  }
}

bool PluginWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();
  flutter::DartProject project(L"data");
  // Third entrypoint (main window / attachment drawer are the other two):
  // lib/plugin_window_main.dart.
  project.set_dart_entrypoint("pluginWindowMain");

  controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project);
  if (!controller_->engine() || !controller_->view()) {
    return false;
  }
  SetChildContent(controller_->view()->GetNativeWindow());

  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      controller_->engine()->messenger(), "lunac/plugin-window",
      &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) {
        const std::string& method = call.method_name();
        if (method == "ready") {
          SendInit();
          result->Success();
          return;
        }
        if (method == "close") {
          CloseFromHost();
          result->Success();
          return;
        }
        if (method == "drag") {
          // Started by the panel header. ReleaseCapture is required, otherwise
          // the mouse is still captured by the Flutter child window and the
          // modal move loop never sees WM_MOUSEMOVE.
          HWND hwnd = GetHandle();
          if (hwnd != nullptr) {
            ::ReleaseCapture();
            ::SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION, 0);
          }
          result->Success();
          return;
        }
        if (method == "resizeStart") {
          // 边缘手势条（Dart 侧）报告自己抓的是哪条边 / 哪个角。
          const auto* hit_name = std::get_if<std::string>(call.arguments());
          const std::string name = hit_name == nullptr ? std::string() : *hit_name;
          int hit = 0;
          if (name == "left") {
            hit = HTLEFT;
          } else if (name == "right") {
            hit = HTRIGHT;
          } else if (name == "top") {
            hit = HTTOP;
          } else if (name == "bottom") {
            hit = HTBOTTOM;
          } else if (name == "topLeft") {
            hit = HTTOPLEFT;
          } else if (name == "topRight") {
            hit = HTTOPRIGHT;
          } else if (name == "bottomLeft") {
            hit = HTBOTTOMLEFT;
          } else if (name == "bottomRight") {
            hit = HTBOTTOMRIGHT;
          }
          if (hit != 0) {
            ::ReleaseCapture();
            BeginResize(hit, /*capture=*/false);
          }
          result->Success(flutter::EncodableValue(hit != 0));
          return;
        }
        if (method == "resizeUpdate") {
          UpdateResize();
          result->Success();
          return;
        }
        if (method == "resizeEnd") {
          EndResize();
          result->Success();
          return;
        }
        if (method == "appCapture") {
          // 一档：抓一帧真实窗口画面（只读）。返回落盘的 BMP 路径。
          const auto* target = std::get_if<std::string>(call.arguments());
          const std::string name = target == nullptr ? std::string() : *target;
          const std::wstring exe = ExeBaseNameLower(name);
          HWND found = FindWindowByExe(exe);
          flutter::EncodableMap reply;
          if (found == nullptr) {
            reply[flutter::EncodableValue("ok")] = flutter::EncodableValue(false);
            reply[flutter::EncodableValue("error")] =
                flutter::EncodableValue("没有找到正在运行的应用窗口");
            result->Success(flutter::EncodableValue(reply));
            return;
          }
          const std::filesystem::path dir(AppPreviewDir());
          std::error_code create_error;
          std::filesystem::create_directories(dir, create_error);
          std::wstring base = exe.empty() ? std::wstring(L"app") : exe;
          const std::filesystem::path file = dir / (base + L".bmp");
          int width = 0;
          int height = 0;
          std::string error;
          const bool ok =
              CaptureWindowToBmp(found, file.wstring(), &width, &height, &error);
          wchar_t title[256] = {0};
          ::GetWindowTextW(found, title, static_cast<int>(std::size(title)));
          reply[flutter::EncodableValue("ok")] = flutter::EncodableValue(ok);
          reply[flutter::EncodableValue("path")] =
              flutter::EncodableValue(Utf8FromWide(file.wstring()));
          reply[flutter::EncodableValue("title")] =
              flutter::EncodableValue(Utf8FromWide(std::wstring(title)));
          reply[flutter::EncodableValue("width")] = flutter::EncodableValue(width);
          reply[flutter::EncodableValue("height")] =
              flutter::EncodableValue(height);
          reply[flutter::EncodableValue("error")] = flutter::EncodableValue(error);
          result->Success(flutter::EncodableValue(reply));
          return;
        }
        if (method == "appDock") {
          // 二档：把真实窗口挪到面板区域（不重父化）。参数 = [exe, x, y, w, h]。
          const auto* args = std::get_if<flutter::EncodableList>(call.arguments());
          const auto* target =
              args != nullptr && !args->empty() ? std::get_if<std::string>(&(*args)[0])
                                                : nullptr;
          flutter::EncodableMap reply;
          if (args == nullptr || args->size() < 5 || target == nullptr) {
            reply[flutter::EncodableValue("ok")] = flutter::EncodableValue(false);
            reply[flutter::EncodableValue("error")] =
                flutter::EncodableValue("参数不完整");
            result->Success(flutter::EncodableValue(reply));
            return;
          }
          auto pixel = [&args](size_t index) {
            const flutter::EncodableValue& value = (*args)[index];
            if (const auto* number = std::get_if<double>(&value)) {
              return static_cast<int>(std::lround(*number));
            }
            if (const auto* number = std::get_if<int32_t>(&value)) {
              return static_cast<int>(*number);
            }
            if (const auto* number = std::get_if<int64_t>(&value)) {
              return static_cast<int>(*number);
            }
            return 0;
          };
          RECT rect = {pixel(1), pixel(2), 0, 0};
          rect.right = rect.left + pixel(3);
          rect.bottom = rect.top + pixel(4);
          std::string error;
          const bool ok = PluginWindowManager::GetInstance()->DockApp(
              GetHandle(), *target, rect, &error);
          reply[flutter::EncodableValue("ok")] = flutter::EncodableValue(ok);
          reply[flutter::EncodableValue("error")] = flutter::EncodableValue(error);
          result->Success(flutter::EncodableValue(reply));
          return;
        }
        if (method == "appUndock") {
          PluginWindowManager::GetInstance()->UndockApp(GetHandle());
          result->Success(flutter::EncodableValue(true));
          return;
        }
        if (method == "sandboxOpen") {
          // 只在 AI 窗里用：给它一个「预览窗还在不在」的探针。
          result->Success(flutter::EncodableValue(
              PluginWindowManager::GetInstance()->IsOpen("sandbox")));
          return;
        }
        if (method == "restoreChat") {
          // 预览窗关闭后的兜底「回弹」：不依赖销毁回调是否走通。
          PluginWindowManager::GetInstance()->RestoreAnchorWidth(GetHandle());
          result->Success(flutter::EncodableValue(true));
          return;
        }
        if (method == "minimize") {
          HWND hwnd = GetHandle();
          if (hwnd != nullptr) {
            ::ShowWindow(hwnd, SW_MINIMIZE);
          }
          result->Success();
          return;
        }
        if (method == "pin") {
          // v1 plugin titlebar: the pin button toggles always-on-top.
          bool pinned = true;
          if (const bool* value = std::get_if<bool>(call.arguments())) {
            pinned = *value;
          }
          HWND hwnd = GetHandle();
          if (hwnd != nullptr) {
            ::SetWindowPos(hwnd, pinned ? HWND_TOPMOST : HWND_NOTOPMOST, 0, 0,
                           0, 0,
                           SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
          }
          result->Success(flutter::EncodableValue(pinned));
          return;
        }
        result->NotImplemented();
      });

  // "lunac/sandbox-host": any panel running in this engine (the AI chat panel)
  // can open the sandbox preview window; it opens beside this window.
  sandbox_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          controller_->engine()->messenger(), "lunac/sandbox-host",
          &flutter::StandardMethodCodec::GetInstance());
  sandbox_channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) {
        if (call.method_name() == "open") {
          PluginWindowManager* manager = PluginWindowManager::GetInstance();
          const bool ok = manager->Open(
              "sandbox", "Sandbox", kPreviewLogicalWidth, kDockedLogicalHeight,
              560, 380, std::vector<std::string>(), std::string(),
              GetHandle());
          if (ok) {
            // 双窗布局：AI 窗收窄到 720×720，预览 1280×720 贴在它左侧。
            manager->LayoutForPreview(GetHandle());
          }
          result->Success(flutter::EncodableValue(ok));
          return;
        }
        if (call.method_name() == "close") {
          PluginWindowManager::GetInstance()->Close("sandbox");
          result->Success();
          return;
        }
        result->NotImplemented();
      });

  // 文件拖放（2026-11-20 用户口径）：插件窗里的对话输入框也要能拖文件进来。
  // 主窗早就有 DragAcceptFiles + WM_DROPFILES；插件窗是独立 engine，之前没接，
  // 所以拖到 AI 对话窗上没有任何反应。这里补齐同名通道（lib/platform/drop.dart）。
  ::DragAcceptFiles(GetHandle(), TRUE);
  drop_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          controller_->engine()->messenger(), "lunac/drop",
          &flutter::StandardMethodCodec::GetInstance());

  // 剪贴板文件：资源管理器里 Ctrl+C 复制文件后 Ctrl+V 粘进对话输入框。
  clipboard_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          controller_->engine()->messenger(), "lunac/clipboard",
          &flutter::StandardMethodCodec::GetInstance());
  clipboard_channel_->SetMethodCallHandler(
      [](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
             result) {
        if (call.method_name() == "files") {
          flutter::EncodableList files;
          for (const auto& path : ClipboardFilePaths()) {
            files.emplace_back(flutter::EncodableValue(path));
          }
          result->Success(flutter::EncodableValue(files));
          return;
        }
        result->NotImplemented();
      });

  controller_->engine()->SetNextFrameCallback([this]() {
    if (GetHandle() == nullptr) {
      return;
    }
    Show();
    revealed_ = true;
    // 依附窗口不抢焦点（用户可能正在 AI 输入框里打字）。
    if (anchor_ == nullptr) {
      ::SetForegroundWindow(GetHandle());
    }
  });
  controller_->ForceRedraw();
  return true;
}

void PluginWindow::SendInit() {
  if (channel_ == nullptr) {
    return;
  }
  flutter::EncodableList args;
  args.emplace_back(flutter::EncodableValue(id_));
  args.emplace_back(flutter::EncodableValue(title_));
  flutter::EncodableList files;
  for (const auto& path : attachments_) {
    files.emplace_back(flutter::EncodableValue(path));
  }
  args.emplace_back(flutter::EncodableValue(files));
  // 4th element: the query the user searched for (chat prefill).
  args.emplace_back(flutter::EncodableValue(query_));
  channel_->InvokeMethod("init",
                         std::make_unique<flutter::EncodableValue>(args));
}

void PluginWindow::CloseFromHost() {
  HWND hwnd = GetHandle();
  if (hwnd != nullptr) {
    ::PostMessage(hwnd, WM_CLOSE, 0, 0);
  }
}

void PluginWindow::OnDestroy() {
  if (!notified_) {
    notified_ = true;
    // 停靠的应用窗：宿主没了就还回原矩形 / 原样式（否则会把别人的窗口留在面板位置）。
    PluginWindowManager::GetInstance()->UndockApp(GetHandle());
    PluginWindowManager::GetInstance()->OnWindowDestroyed(id_);
    // The sandbox preview is attached to the window it was opened from: when
    // that window goes away, the preview goes with it.
    PluginWindowManager::GetInstance()->OnAnchorDestroyed(GetHandle());
  }
  sandbox_channel_.reset();
  drop_channel_.reset();
  clipboard_channel_.reset();
  channel_.reset();
  controller_.reset();
  Win32Window::OnDestroy();
}

LRESULT PluginWindow::MessageHandler(HWND hwnd,
                                     UINT const message,
                                     WPARAM const wparam,
                                     LPARAM const lparam) noexcept {
  switch (message) {
    // 文件拖放：把真实路径数组交给本窗口的 Dart 侧（对话输入框插附件胶囊）。
    case WM_DROPFILES: {
      flutter::EncodableList files;
      for (const auto& path :
           TakeDropPaths(reinterpret_cast<HDROP>(wparam))) {
        files.emplace_back(flutter::EncodableValue(path));
      }
      if (drop_channel_) {
        drop_channel_->InvokeMethod(
            "drop", std::make_unique<flutter::EncodableValue>(files));
      }
      return 0;
    }
    // 依附预览：AI 窗口一动 / 一缩放 / 一显隐，预览始终贴在它左侧。
    case WM_MOVE:
    case WM_SIZE:
    case WM_SHOWWINDOW: {
      if (anchor_ == nullptr) {
        PluginWindowManager::GetInstance()->SyncSandbox(hwnd);
      }
      // 停靠的应用窗跟着预览窗一起移动 / 缩放。
      PluginWindowManager::GetInstance()->SyncDock(hwnd);
      break;
    }
    case WM_NCLBUTTONDOWN: {
      // 无边框窗口不会替我们启动系统 sizing loop ⇒ 自己接管（见 BeginResize）。
      const int hit = static_cast<int>(wparam);
      if (hit == HTLEFT || hit == HTRIGHT || hit == HTTOP || hit == HTBOTTOM ||
          hit == HTTOPLEFT || hit == HTTOPRIGHT || hit == HTBOTTOMLEFT ||
          hit == HTBOTTOMRIGHT) {
        BeginResize(hit);
        return 0;
      }
      break;
    }
    case WM_MOUSEMOVE: {
      if (resizing_) {
        UpdateResize();
        return 0;
      }
      break;
    }
    case WM_LBUTTONUP: {
      if (resizing_) {
        EndResize();
        return 0;
      }
      break;
    }
    case WM_CAPTURECHANGED: {
      if (resizing_) {
        resizing_ = false;
        resize_hit_ = 0;
      }
      break;
    }
    case WM_NCHITTEST: {
      POINT pt = {GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
      RECT rect;
      ::GetWindowRect(hwnd, &rect);
      const UINT dpi = ::GetDpiForWindow(hwnd);
      const int border = ::MulDiv(kResizeBorder, static_cast<int>(dpi), 96);
      const bool left = pt.x < rect.left + border;
      const bool right = pt.x >= rect.right - border;
      const bool top = pt.y < rect.top + border;
      const bool bottom = pt.y >= rect.bottom - border;
      if (top && left) return HTTOPLEFT;
      if (top && right) return HTTOPRIGHT;
      if (bottom && left) return HTBOTTOMLEFT;
      if (bottom && right) return HTBOTTOMRIGHT;
      if (left) return HTLEFT;
      if (right) return HTRIGHT;
      if (top) return HTTOP;
      if (bottom) return HTBOTTOM;
      // The rest of the window is ordinary content: dragging is started from
      // the Flutter header ("drag"), so buttons there stay clickable.
      return HTCLIENT;
    }
    case WM_GETMINMAXINFO: {
      auto* info = reinterpret_cast<MINMAXINFO*>(lparam);
      UINT dpi = ::GetDpiForWindow(hwnd);
      info->ptMinTrackSize.x =
          ::MulDiv(min_width_, static_cast<int>(dpi), 96);
      info->ptMinTrackSize.y =
          ::MulDiv(min_height_, static_cast<int>(dpi), 96);
      return 0;
    }
    case WM_CLOSE:
      Destroy();
      return 0;
    default:
      break;
  }
  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}

PluginWindowManager* PluginWindowManager::GetInstance() {
  static PluginWindowManager* instance = new PluginWindowManager();
  return instance;
}

void PluginWindowManager::SetHostChannel(
    flutter::MethodChannel<flutter::EncodableValue>* channel) {
  host_channel_ = channel;
}

void PluginWindowManager::SetMainWindow(HWND main_window) {
  main_window_ = main_window;
}

bool PluginWindowManager::Open(const std::string& id,
                               const std::string& title,
                               int width,
                               int height,
                               int min_width,
                               int min_height,
                               std::vector<std::string> attachments,
                               std::string query,
                               HWND anchor) {
  auto it = windows_.find(id);
  if (it != windows_.end() && it->second->GetHandle() != nullptr) {
    ::ShowWindow(it->second->GetHandle(), SW_SHOWNORMAL);
    // 依附的预览窗不抢焦点：用户可能正在 AI 输入框里打字。
    if (anchor == nullptr) {
      ::SetForegroundWindow(it->second->GetHandle());
    }
    return true;
  }
  // The sandbox preview does not consume the "at most two plugin windows" quota.
  const bool is_sandbox = id == "sandbox";
  if (!is_sandbox && PluginOpenCount() >= kMaxPluginWindows) {
    return false;
  }

  // Always build a fresh object: the previous one (if any) already tore its
  // engine down, and reusing it would keep stale specs.
  windows_[id] = std::make_unique<PluginWindow>(id, title, width, height,
                                                min_width, min_height,
                                                main_window_, anchor);
  PluginWindow* window = windows_[id].get();
  window->SetAttachments(std::move(attachments));
  window->SetQuery(std::move(query));
  if (!window->CreateHostWindow()) {
    windows_.erase(id);
    return false;
  }
  if (is_sandbox) {
    sandbox_anchor_ = anchor;
  }
  open_order_.push_back(id);
  return true;
}

void PluginWindowManager::Close(const std::string& id) {
  auto it = windows_.find(id);
  if (it == windows_.end()) {
    return;
  }
  it->second->CloseFromHost();
}

bool PluginWindowManager::IsOpen(const std::string& id) const {
  auto it = windows_.find(id);
  if (it == windows_.end()) {
    return false;
  }
  return it->second->GetHandle() != nullptr;
}

int PluginWindowManager::OpenCount() const {
  int count = 0;
  for (const auto& entry : windows_) {
    if (entry.second->GetHandle() != nullptr) {
      ++count;
    }
  }
  return count;
}

int PluginWindowManager::PluginOpenCount() const {
  int count = 0;
  for (const auto& entry : windows_) {
    if (entry.first == "sandbox") {
      continue;
    }
    if (entry.second->GetHandle() != nullptr) {
      ++count;
    }
  }
  return count;
}

std::string PluginWindowManager::OldestOpenId() const {
  for (const auto& id : open_order_) {
    if (id == "sandbox") {
      continue;
    }
    auto it = windows_.find(id);
    if (it != windows_.end() && it->second->GetHandle() != nullptr) {
      return id;
    }
  }
  return std::string();
}

void PluginWindowManager::CloseAll() {
  for (auto& entry : windows_) {
    entry.second->CloseFromHost();
  }
}

void PluginWindowManager::OnWindowDestroyed(const std::string& id) {
  open_order_.erase(std::remove(open_order_.begin(), open_order_.end(), id),
                    open_order_.end());
  if (id == "sandbox") {
    // 预览关掉后 AI 窗恢复规格宽度（左缘不动），否则会一直停在 720 的窄档。
    HWND anchor = sandbox_anchor_;
    sandbox_anchor_ = nullptr;
    RestoreAnchorWidth(anchor);
  }
  if (id == "chat") {
    // AI 窗自己没了：清掉「回弹」用的旧矩形，避免下次开窗弹到旧位置。
    chat_restore_valid_ = false;
  }
  if (host_channel_ != nullptr) {
    host_channel_->InvokeMethod(
        "closed", std::make_unique<flutter::EncodableValue>(id));
  }
}

void PluginWindowManager::OnAnchorDestroyed(HWND anchor) {
  if (anchor == nullptr || sandbox_anchor_ == nullptr) {
    return;
  }
  if (sandbox_anchor_ == anchor && IsOpen("sandbox")) {
    Close("sandbox");
  }
}

void PluginWindowManager::SyncSandbox(HWND anchor) {
  if (anchor == nullptr || sandbox_anchor_ == nullptr) {
    return;
  }
  if (sandbox_anchor_ != anchor) {
    return;
  }
  auto it = windows_.find("sandbox");
  if (it == windows_.end() || it->second->GetHandle() == nullptr) {
    return;
  }
  it->second->SyncToAnchor();
}

std::string PluginWindowManager::IdOfHandle(HWND handle) const {
  if (handle == nullptr) {
    return std::string();
  }
  for (const auto& entry : windows_) {
    if (entry.second->GetHandle() == handle) {
      return entry.first;
    }
  }
  return std::string();
}

// 打开预览时的双窗布局：AI 窗收窄到 720×720（左缘不动 = 粘合线），预览窗
// 1280×720 贴在左侧；整块（预览 + AI）夹进工作区，屏幕装不下时先砍预览宽度。
void PluginWindowManager::LayoutForPreview(HWND anchor) {
  if (anchor == nullptr || sandbox_anchor_ != anchor) {
    return;
  }
  // 只对 AI 助手窗做这个布局；主搜索窗 / 其它插件窗保持原样。
  if (IdOfHandle(anchor) != "chat") {
    return;
  }
  auto chat_it = windows_.find("chat");
  auto sandbox_it = windows_.find("sandbox");
  if (chat_it == windows_.end() || sandbox_it == windows_.end()) {
    return;
  }
  if (chat_it->second->GetHandle() == nullptr ||
      sandbox_it->second->GetHandle() == nullptr) {
    return;
  }

  const double scale = DpiScaleOf(anchor);
  const RECT work = WorkAreaOf(anchor);
  RECT self;
  ::GetWindowRect(anchor, &self);
  const int work_w = static_cast<int>(work.right - work.left);
  const int work_h = static_cast<int>(work.bottom - work.top);

  const int chat_w = std::min(LogicalToPhysical(kChatDockedLogicalWidth, scale),
                              work_w);
  const int chat_h = std::min(LogicalToPhysical(kDockedLogicalHeight, scale),
                              work_h);
  int preview_w = std::min(LogicalToPhysical(kPreviewLogicalWidth, scale),
                           work_w - chat_w);
  if (preview_w < 0) {
    preview_w = 0;
  }

  int top = static_cast<int>(self.top);
  top = std::max(top, static_cast<int>(work.top));
  if (top + chat_h > work.bottom) {
    top = std::max(static_cast<int>(work.top),
                   static_cast<int>(work.bottom) - chat_h);
  }

  int left = static_cast<int>(self.left) - preview_w;
  const int block = preview_w + chat_w;
  if (left < work.left) {
    left = static_cast<int>(work.left);
  }
  if (left + block > work.right) {
    left = std::max(static_cast<int>(work.left),
                    static_cast<int>(work.right) - block);
  }
  const int axis = left + preview_w;

  // 记下 AI 窗原来的矩形：预览关掉时「回弹」回这里（大小 + 位置都复原）。
  if (!chat_restore_valid_) {
    chat_restore_rect_ = self;
    chat_restore_valid_ = true;
  }

  // 先收 AI 窗（会触发 WM_SIZE → SyncSandbox 把预览贴到新位置），再钉一次。
  chat_it->second->SetBoundsPhysical(axis, top, chat_w, chat_h);
  sandbox_it->second->SetBoundsPhysical(left, top, preview_w, chat_h);
}

void PluginWindowManager::RestoreAnchorWidth(HWND anchor) {
  if (anchor == nullptr || ::IsWindow(anchor) == 0) {
    return;
  }
  if (IdOfHandle(anchor) != "chat") {
    return;
  }
  auto chat_it = windows_.find("chat");
  if (chat_it == windows_.end() || chat_it->second->GetHandle() == nullptr) {
    return;
  }

  const RECT work = WorkAreaOf(anchor);
  const int work_w = static_cast<int>(work.right - work.left);
  const int work_h = static_cast<int>(work.bottom - work.top);
  RECT self;
  ::GetWindowRect(anchor, &self);

  RECT target = self;
  if (chat_restore_valid_) {
    // 打开预览前的矩形：位置和宽度一起回去（这才是用户说的「回弹」）。
    target = chat_restore_rect_;
    chat_restore_valid_ = false;
  } else {
    const double scale = DpiScaleOf(anchor);
    target.right = target.left + LogicalToPhysical(kChatSpecLogicalWidth, scale);
  }

  int width = std::min(static_cast<int>(target.right - target.left), work_w);
  int height = std::min(static_cast<int>(target.bottom - target.top), work_h);
  if (width <= 0) {
    width = work_w;
  }
  if (height <= 0) {
    height = static_cast<int>(self.bottom - self.top);
  }
  int left = static_cast<int>(target.left);
  int top = static_cast<int>(target.top);
  left = std::max(left, static_cast<int>(work.left));
  if (left + width > work.right) {
    left = std::max(static_cast<int>(work.left),
                    static_cast<int>(work.right) - width);
  }
  top = std::max(top, static_cast<int>(work.top));
  if (top + height > work.bottom) {
    top = std::max(static_cast<int>(work.top),
                   static_cast<int>(work.bottom) - height);
  }
  chat_it->second->SetBoundsPhysical(left, top, width, height);
}

// ── 应用预览（二档）：停靠 / 还回 ─────────────────────────────────

bool PluginWindowManager::DockApp(HWND host, const std::string& target,
                                  const RECT& screen_rect, std::string* error) {
  if (host == nullptr || ::IsWindow(host) == 0) {
    *error = "预览窗不可用";
    return false;
  }
  HWND hwnd = FindWindowByExe(ExeBaseNameLower(target));
  if (hwnd == nullptr) {
    *error = "没有找到正在运行的应用窗口";
    return false;
  }
  if (dock_.hwnd == hwnd && dock_.host == host) {
    SyncDock(host);
    return true;
  }
  UndockApp();

  dock_.hwnd = hwnd;
  dock_.host = host;
  dock_.target = target;
  ::GetWindowRect(hwnd, &dock_.original_rect);
  dock_.original_ex_style = ::GetWindowLongPtr(hwnd, GWL_EXSTYLE);

  POINT origin = {0, 0};
  ::ClientToScreen(host, &origin);
  dock_.rel_rect.left = screen_rect.left - origin.x;
  dock_.rel_rect.top = screen_rect.top - origin.y;
  dock_.rel_rect.right = screen_rect.right - origin.x;
  dock_.rel_rect.bottom = screen_rect.bottom - origin.y;

  LONG_PTR style = dock_.original_ex_style;
  style = (style | WS_EX_TOOLWINDOW) & ~WS_EX_APPWINDOW;
  ::SetWindowLongPtr(hwnd, GWL_EXSTYLE, style);
  // owner = 预览窗：应用压在预览窗之上，预览窗最小化 / 关闭时一起走。
  ::SetWindowLongPtr(hwnd, GWLP_HWNDPARENT,
                     reinterpret_cast<LONG_PTR>(host));
  ::SetWindowPos(hwnd, HWND_TOP, screen_rect.left, screen_rect.top,
                 screen_rect.right - screen_rect.left,
                 screen_rect.bottom - screen_rect.top, SWP_SHOWWINDOW);
  return true;
}

void PluginWindowManager::UndockApp(HWND host) {
  if (dock_.hwnd == nullptr) {
    return;
  }
  if (host != nullptr && dock_.host != host) {
    return;
  }
  HWND hwnd = dock_.hwnd;
  if (::IsWindow(hwnd) != 0) {
    ::SetWindowLongPtr(hwnd, GWLP_HWNDPARENT, 0);
    ::SetWindowLongPtr(hwnd, GWL_EXSTYLE, dock_.original_ex_style);
    ::SetWindowPos(hwnd, nullptr, dock_.original_rect.left,
                   dock_.original_rect.top,
                   dock_.original_rect.right - dock_.original_rect.left,
                   dock_.original_rect.bottom - dock_.original_rect.top,
                   SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
  }
  dock_ = DockedApp();
}

void PluginWindowManager::SyncDock(HWND host) {
  if (dock_.hwnd == nullptr || dock_.host == nullptr || dock_.host != host) {
    return;
  }
  if (::IsWindow(dock_.hwnd) == 0 || ::IsWindow(dock_.host) == 0) {
    dock_ = DockedApp();
    return;
  }
  if (::IsWindowVisible(dock_.host) == 0) {
    ::ShowWindow(dock_.hwnd, SW_HIDE);
    return;
  }
  POINT origin = {0, 0};
  ::ClientToScreen(dock_.host, &origin);
  const int width = dock_.rel_rect.right - dock_.rel_rect.left;
  const int height = dock_.rel_rect.bottom - dock_.rel_rect.top;
  if (width <= 0 || height <= 0) {
    return;
  }
  ::SetWindowPos(dock_.hwnd, HWND_TOP, origin.x + dock_.rel_rect.left,
                 origin.y + dock_.rel_rect.top, width, height,
                 SWP_NOACTIVATE | SWP_SHOWWINDOW);
}
