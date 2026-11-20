#include "attachment_window.h"

#include <flutter/standard_method_codec.h>

#include <algorithm>
#include <cmath>
#include <variant>

namespace {

// Layout constants (logical pixels). Keep in sync with
// lib/search/search_page.dart: page inset 12 / 8 and search bar height 52.
constexpr double kMainLeftInsetLogical = 12.0;
constexpr double kMainTopInsetLogical = 8.0;

// Makes the window background transparent (same recipe window_manager uses for
// the main window): ACCENT_ENABLE_TRANSPARENTGRADIENT with a fully transparent
// colour.
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

double DpiScaleOf(HWND window) {
  const UINT dpi = ::GetDpiForWindow(window);
  return dpi > 0 ? static_cast<double>(dpi) / 96.0 : 1.0;
}

}  // namespace

AttachmentWindow* AttachmentWindow::GetInstance() {
  static AttachmentWindow* instance = new AttachmentWindow();
  return instance;
}

void AttachmentWindow::AttachToMainWindow(HWND main_window) {
  main_window_ = main_window;
}

void AttachmentWindow::WarmUp() {
  if (GetHandle() == nullptr && main_window_ != nullptr) {
    EnsureCreated(main_window_);
  }
}

void AttachmentWindow::EnsureCreated(HWND main_window) {
  main_window_ = main_window;
  if (GetHandle() != nullptr) {
    return;
  }

  Win32Window::Point origin(0, 0);
  Win32Window::Size size(1, 1);
  if (!Create(L"lunac-attach", origin, size)) {
    return;
  }
  SetQuitOnClose(false);

  // Helper window: never activate (the main window keeps focus / does not
  // auto-hide), stay off the taskbar, and float above normal windows.
  LONG_PTR ex_style = ::GetWindowLongPtr(GetHandle(), GWL_EXSTYLE);
  ::SetWindowLongPtr(GetHandle(), GWL_EXSTYLE,
                     ex_style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW |
                         WS_EX_TOPMOST);
  MakeWindowTransparent(GetHandle());
}

bool AttachmentWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();
  flutter::DartProject project(L"data");
  // Second entrypoint: lib/attachment_drawer_main.dart.
  project.set_dart_entrypoint("attachmentDrawerMain");

  controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project);
  if (!controller_->engine() || !controller_->view()) {
    return false;
  }
  SetChildContent(controller_->view()->GetNativeWindow());

  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      controller_->engine()->messenger(), "lunac/attach",
      &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) {
        const std::string& method = call.method_name();
        if (method == "ready") {
          drawer_ready_ = true;
          SendFilesToDrawer();
          result->Success();
          return;
        }
        if (method == "resize") {
          const auto* size = std::get_if<flutter::EncodableList>(call.arguments());
          if (size != nullptr && size->size() == 2) {
            if (const auto* width = std::get_if<double>(&(*size)[0])) {
              logical_width_ = *width;
            }
            if (const auto* height = std::get_if<double>(&(*size)[1])) {
              logical_height_ = *height;
            }
          }
          ApplyBounds();
          result->Success();
          return;
        }
        if (method == "remove") {
          const auto* index = std::get_if<int32_t>(call.arguments());
          if (index != nullptr && host_channel_ != nullptr) {
            host_channel_->InvokeMethod(
                "remove", std::make_unique<flutter::EncodableValue>(*index));
          }
          result->Success();
          return;
        }
        result->NotImplemented();
      });

  controller_->ForceRedraw();
  return true;
}

void AttachmentWindow::OnDestroy() {
  // Release the second engine as early as possible (window destroyed on exit).
  channel_.reset();
  controller_.reset();
  drawer_ready_ = false;
  Win32Window::OnDestroy();
}

LRESULT AttachmentWindow::MessageHandler(HWND window,
                                         UINT const message,
                                         WPARAM const wparam,
                                         LPARAM const lparam) noexcept {
  // Clicking the drawer must not activate it: the main window keeps the
  // foreground, so its "hide on blur" logic does not fire and the search field
  // keeps keyboard focus (the two windows behave as one surface).
  if (message == WM_MOUSEACTIVATE) {
    return MA_NOACTIVATE;
  }
  return Win32Window::MessageHandler(window, message, wparam, lparam);
}

void AttachmentWindow::UpdateFiles(const std::vector<std::string>& files) {
  // First real attachment arrives before the idle timer fired: create on
  // demand. An empty initial list must NOT create the second engine, otherwise
  // it would block the main window's reveal animation.
  if (!files.empty() && GetHandle() == nullptr && main_window_ != nullptr) {
    EnsureCreated(main_window_);
  }
  files_ = files;
  SendFilesToDrawer();
}

void AttachmentWindow::SetHostChannel(
    flutter::MethodChannel<flutter::EncodableValue>* channel) {
  host_channel_ = channel;
}

void AttachmentWindow::SendFilesToDrawer() {
  if (!drawer_ready_ || channel_ == nullptr) {
    return;
  }
  flutter::EncodableList list;
  list.reserve(files_.size());
  for (const std::string& file : files_) {
    list.emplace_back(file);
  }
  channel_->InvokeMethod("update",
                         std::make_unique<flutter::EncodableValue>(list));
}

void AttachmentWindow::SyncToMainWindow() {
  if (main_window_ == nullptr || GetHandle() == nullptr) {
    return;
  }
  ApplyBounds();
}

void AttachmentWindow::ApplyBounds() {
  if (main_window_ == nullptr || GetHandle() == nullptr) {
    return;
  }
  if (!::IsWindowVisible(main_window_)) {
    SetDrawerVisible(false);
    return;
  }

  const double scale = DpiScaleOf(main_window_);
  const int width = static_cast<int>(std::lround(logical_width_ * scale));
  const int height = static_cast<int>(std::lround(logical_height_ * scale));
  if (width <= 0 || height <= 0) {
    SetDrawerVisible(false);
    return;
  }

  RECT main_rect;
  ::GetWindowRect(main_window_, &main_rect);
  const int right = main_rect.left + static_cast<int>(
                                         std::lround(kMainLeftInsetLogical *
                                                     scale));
  const int top = main_rect.top + static_cast<int>(
                                       std::lround(kMainTopInsetLogical *
                                                   scale));
  // The drawer grows downwards but must never pass the main window's bottom
  // edge (user decision 2026-11-09); anything taller scrolls inside the panel.
  const int available = main_rect.bottom - top;
  if (available <= 0) {
    SetDrawerVisible(false);
    return;
  }
  const int clamped_height = std::min(height, available);
  ::SetWindowPos(GetHandle(), HWND_TOPMOST, right - width, top, width,
                 clamped_height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
  SetDrawerVisible(true);
}

void AttachmentWindow::HideDrawer() { SetDrawerVisible(false); }

void AttachmentWindow::Shutdown() {
  SetDrawerVisible(false);
  if (GetHandle() != nullptr) {
    // Destroys the window and (in OnDestroy) the second engine. This blocks the
    // message loop, so the main window must already be hidden by now.
    Destroy();
  }
}

void AttachmentWindow::SetDrawerVisible(bool visible) {
  if (GetHandle() == nullptr) {
    return;
  }
  if (visible) {
    if (!::IsWindowVisible(GetHandle())) {
      ::ShowWindow(GetHandle(), SW_SHOWNOACTIVATE);
    }
  } else if (::IsWindowVisible(GetHandle())) {
    ::ShowWindow(GetHandle(), SW_HIDE);
  }
}
