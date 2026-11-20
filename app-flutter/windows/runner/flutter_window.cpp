#include "flutter_window.h"

#include <flutter/standard_method_codec.h>
#include <shellapi.h>

#include <cstdint>
#include <optional>
#include <string>
#include <vector>

#include "attachment_window.h"
#include "flutter/generated_plugin_registrant.h"
#include "plugin_window.h"
#include "utils.h"

namespace {

// Posted by the Rust hotkey / single-instance threads (see
// rust/src/hotkey.rs: WAKE_MESSAGE / ACTIVATE_MESSAGE).
constexpr UINT kLunacWakeMessage = WM_APP + 1;
constexpr UINT kLunacActivateMessage = WM_APP + 2;

}  // namespace

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  // File drop: accept WM_DROPFILES and forward paths to Dart ("lunac/drop").
  DragAcceptFiles(GetHandle(), TRUE);
  drop_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "lunac/drop",
          &flutter::StandardMethodCodec::GetInstance());

  // Attachment drawer bridge: forward the path list to the second window, and
  // receive its "remove" requests back on this engine.
  attach_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "lunac/attach-host",
          &flutter::StandardMethodCodec::GetInstance());
  AttachmentWindow::GetInstance()->SetHostChannel(attach_channel_.get());
  attach_channel_->SetMethodCallHandler(
      [](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
             result) {
        const std::string& method = call.method_name();
        if (method == "update") {
          std::vector<std::string> files;
          if (const auto* list =
                  std::get_if<flutter::EncodableList>(call.arguments())) {
            for (const auto& item : *list) {
              if (const auto* path = std::get_if<std::string>(&item)) {
                files.push_back(*path);
              }
            }
          }
          AttachmentWindow::GetInstance()->UpdateFiles(files);
          result->Success();
          return;
        }
        if (method == "warmup") {
          // Create the second engine ahead of time (window stays hidden at
          // width 0) so the first file paste does not stall on engine startup.
          AttachmentWindow::GetInstance()->WarmUp();
          result->Success();
          return;
        }
        if (method == "visible") {
          const auto* visible = std::get_if<bool>(call.arguments());
          if (visible != nullptr) {
            if (*visible) {
              AttachmentWindow::GetInstance()->SyncToMainWindow();
            } else {
              AttachmentWindow::GetInstance()->HideDrawer();
            }
          }
          result->Success();
          return;
        }
        if (method == "shutdown") {
          AttachmentWindow::GetInstance()->Shutdown();
          result->Success();
          return;
        }
        if (method == "appForeground") {
          // The main window and the drawer are two top-level windows of the
          // same process; "hide on blur" must not fire while either of them is
          // in front (they act as one surface).
          HWND foreground = ::GetForegroundWindow();
          DWORD foreground_pid = 0;
          if (foreground != nullptr) {
            ::GetWindowThreadProcessId(foreground, &foreground_pid);
          }
          result->Success(
              flutter::EncodableValue(foreground_pid == ::GetCurrentProcessId()));
          return;
        }
        result->NotImplemented();
      });

  // Wake bridge: Dart asks for the window handle once and hands it to the Rust
  // hotkey thread, which posts WM_APP messages back (event-driven wake instead
  // of polling a counter in the background).
  wake_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "lunac/wake",
          &flutter::StandardMethodCodec::GetInstance());
  wake_channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) {
        if (call.method_name() == "hwnd") {
          result->Success(flutter::EncodableValue(static_cast<int64_t>(
              reinterpret_cast<intptr_t>(GetHandle()))));
          return;
        }
        result->NotImplemented();
      });

  // Clipboard bridge: Explorer "copy file" puts a CF_HDROP in the clipboard
  // (no plain text), which Flutter's Clipboard API cannot see. The chat input
  // asks for the path list on Ctrl+V. Channel name matches
  // lib/platform/clipboard_files.dart.
  clipboard_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "lunac/clipboard",
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

  // Plugin windows bridge (2026-11-13): independent windows for music / proxy /
  // convert / ocr / chat. Policy (max two + "close the oldest?" prompt) lives in
  // Dart; C++ only executes open/close and reports "closed" back.
  PluginWindowManager::GetInstance()->SetMainWindow(GetHandle());
  plugin_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "lunac/plugin-host",
          &flutter::StandardMethodCodec::GetInstance());
  PluginWindowManager::GetInstance()->SetHostChannel(plugin_channel_.get());
  plugin_channel_->SetMethodCallHandler(
      [](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
             result) {
        auto* manager = PluginWindowManager::GetInstance();
        const std::string& method = call.method_name();
        if (method == "open") {
          const auto* args =
              std::get_if<flutter::EncodableMap>(call.arguments());
          if (args == nullptr) {
            result->Error("bad_args", "open expects a map");
            return;
          }
          auto str = [&args](const char* key) -> std::string {
            auto it = args->find(flutter::EncodableValue(key));
            if (it == args->end()) {
              return std::string();
            }
            const auto* value = std::get_if<std::string>(&it->second);
            return value != nullptr ? *value : std::string();
          };
          auto num = [&args](const char* key, int fallback) -> int {
            auto it = args->find(flutter::EncodableValue(key));
            if (it == args->end()) {
              return fallback;
            }
            if (const auto* value = std::get_if<int32_t>(&it->second)) {
              return *value;
            }
            if (const auto* value64 = std::get_if<int64_t>(&it->second)) {
              return static_cast<int>(*value64);
            }
            if (const auto* value = std::get_if<double>(&it->second)) {
              return static_cast<int>(*value);
            }
            return fallback;
          };
          // Attachments carried from the main window (OCR / convert need them).
          std::vector<std::string> attachments;
          auto files_it = args->find(flutter::EncodableValue("attachments"));
          if (files_it != args->end()) {
            if (const auto* list =
                    std::get_if<flutter::EncodableList>(&files_it->second)) {
              for (const auto& item : *list) {
                if (const auto* path = std::get_if<std::string>(&item)) {
                  attachments.push_back(*path);
                }
              }
            }
          }
          const bool ok =
              manager->Open(str("id"), str("title"), num("width", 820),
                            num("height", 560), num("minWidth", 300),
                            num("minHeight", 200), attachments, str("query"));
          // "limit" tells Dart to ask the user (close the oldest one or not);
          // the 2-window cap itself is enforced here.
          result->Success(flutter::EncodableValue(
              ok ? std::string("opened") : std::string("limit")));
          return;
        }
        if (method == "close") {
          const auto* id = std::get_if<std::string>(call.arguments());
          if (id != nullptr) {
            manager->Close(*id);
          }
          result->Success();
          return;
        }
        if (method == "list") {
          flutter::EncodableList ids;
          // Report only the windows that still exist.
          for (const auto& id : {"chat", "music", "proxy", "convert", "ocr"}) {
            if (manager->IsOpen(id)) {
              ids.emplace_back(flutter::EncodableValue(id));
            }
          }
          result->Success(flutter::EncodableValue(ids));
          return;
        }
        if (method == "oldest") {
          result->Success(
              flutter::EncodableValue(manager->OldestOpenId()));
          return;
        }
        if (method == "count") {
          result->Success(flutter::EncodableValue(manager->PluginOpenCount()));
          return;
        }
        if (method == "closeAll") {
          manager->CloseAll();
          result->Success();
          return;
        }
        result->NotImplemented();
      });

  // Sandbox preview bridge (main engine side): the preview is a plugin window
  // with id "sandbox" that opens beside the main window. It does not consume
  // the two-plugin quota (see plugin_window.cpp).
  sandbox_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "lunac/sandbox-host",
          &flutter::StandardMethodCodec::GetInstance());
  sandbox_channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) {
        if (call.method_name() == "open") {
          const bool ok = PluginWindowManager::GetInstance()->Open(
              "sandbox", "Sandbox", 860, 560, 560, 380,
              std::vector<std::string>(), std::string(), GetHandle());
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

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
    // The attachment drawer runs on a second engine; it is created lazily on
    // the first attachment so the main window's reveal animation is not
    // blocked.
    AttachmentWindow::GetInstance()->AttachToMainWindow(GetHandle());
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // File drop: send the real path list to Dart (attachment chips).
  if (message == WM_DROPFILES) {
    flutter::EncodableList files;
    // TakeDropPaths 负责 DragFinish（释放 HDROP）。
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

  // Native wake events (hotkey / second instance) forwarded to Dart. Posted by
  // the Rust side, so the app does not poll a counter in the background.
  if (message == kLunacWakeMessage || message == kLunacActivateMessage) {
    if (wake_channel_) {
      wake_channel_->InvokeMethod(
          message == kLunacWakeMessage ? "wake" : "activate",
          std::make_unique<flutter::EncodableValue>());
    }
    return 0;
  }
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  LRESULT result =
      Win32Window::MessageHandler(hwnd, message, wparam, lparam);
  // Keep the drawer glued to the search bar while the window moves / resizes
  // (its height is clamped to the main window's bottom edge), and hide it
  // together with the main window.
  if (message == WM_MOVE || message == WM_SIZE ||
      message == WM_SHOWWINDOW) {
    AttachmentWindow::GetInstance()->SyncToMainWindow();
  }
  return result;
}
