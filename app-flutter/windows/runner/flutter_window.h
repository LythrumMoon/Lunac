#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/encodable_value.h>
#include <flutter/flutter_view_controller.h>
#include <flutter/method_channel.h>

#include <memory>

#include "win32_window.h"

// A window that does nothing but host a Flutter view.
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|.
  explicit FlutterWindow(const flutter::DartProject& project);
  virtual ~FlutterWindow();

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // File-drop channel (WM_DROPFILES -> Dart "drop" with a list of paths).
  // Channel name / method name must match lib/platform/drop.dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> drop_channel_;

  // Attachment drawer bridge (main engine side). The drawer itself is a second
  // window / engine, see attachment_window.h. Channel name must match
  // lib/platform/attachment_window.dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      attach_channel_;

  // Wake bridge: the Rust hotkey / single-instance threads post WM_APP+1 / +2
  // to this window; we forward them to Dart ("wake" / "activate") so the app
  // does not have to poll a counter in the background. Channel name must match
  // lib/platform/hotkey.dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> wake_channel_;

  // Plugin windows bridge (main engine side): open / close / list / oldest.
  // The windows themselves are separate engines, see plugin_window.h. Channel
  // name must match lib/platform/plugin_window.dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      plugin_channel_;

  // Sandbox preview bridge (main engine side). The preview itself is a plugin
  // window with id "sandbox"; channel name must match lib/platform/sandbox.dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      sandbox_channel_;

  // Clipboard bridge: Explorer "copy file" puts a CF_HDROP in the clipboard
  // (no plain text), so the chat input's Ctrl+V asks the native side for the
  // path list. Channel name must match lib/platform/clipboard_files.dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      clipboard_channel_;
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
