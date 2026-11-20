#ifndef RUNNER_ATTACHMENT_WINDOW_H_
#define RUNNER_ATTACHMENT_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/encodable_value.h>
#include <flutter/flutter_view_controller.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>
#include <string>
#include <vector>

#include "win32_window.h"

// The file drawer is a separate top-level window with its own Flutter engine,
// attached to the left edge of the main search bar (user decision 2026-11-08).
//
// Channel names must match lib/platform/attachment_window.dart and
// lib/attachment_drawer_main.dart:
//   - "lunac/attach-host" (main engine): update [paths] / remove [index] /
//     warmup / visible [bool] / shutdown
//   - "lunac/attach"      (drawer engine): ready / resize [logical w, h] /
//     remove [index]
class AttachmentWindow : public Win32Window {
 public:
  static AttachmentWindow* GetInstance();

  // Creates the window + engine. Called after the main window's reveal
  // animation (warmup) so the first file paste does not pay the engine startup
  // cost; also called on demand if an attachment arrives earlier.
  void EnsureCreated(HWND main_window);

  // Creates the drawer window + engine ahead of time if it does not exist yet
  // (called after the main window's reveal animation). It stays hidden while
  // the attachment list is empty.
  void WarmUp();

  // Stores the main window handle without creating anything.
  void AttachToMainWindow(HWND main_window);

  // Stores the attachment list and forwards it to the drawer engine.
  void UpdateFiles(const std::vector<std::string>& files);

  // Keeps the drawer glued to the left edge of the search bar, and hides it
  // when the main window is hidden.
  void SyncToMainWindow();

  // Hides the drawer without destroying it (used when the main window hides or
  // while the app is quitting).
  void HideDrawer();

  // Hides and tears the drawer window + engine down (app exit). Call only after
  // the main window is already hidden, as engine teardown blocks the message
  // loop.
  void Shutdown();

  // The main engine's host channel, used to forward "remove" back to Dart.
  void SetHostChannel(flutter::MethodChannel<flutter::EncodableValue>* channel);

 private:
  AttachmentWindow() = default;

  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window,
                         UINT const message,
                         WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

  void ApplyBounds();
  void SetDrawerVisible(bool visible);
  void SendFilesToDrawer();

  HWND main_window_ = nullptr;
  std::unique_ptr<flutter::FlutterViewController> controller_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  flutter::MethodChannel<flutter::EncodableValue>* host_channel_ = nullptr;
  std::vector<std::string> files_;
  double logical_width_ = 0.0;
  double logical_height_ = 0.0;
  bool drawer_ready_ = false;
};

#endif  // RUNNER_ATTACHMENT_WINDOW_H_
