#ifndef RUNNER_PLUGIN_WINDOW_H_
#define RUNNER_PLUGIN_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/encodable_value.h>
#include <flutter/flutter_view_controller.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <map>
#include <memory>
#include <string>
#include <vector>

#include "win32_window.h"

// One independent plugin window (music / proxy / convert / ocr / chat ...).
//
// User decision (2026-11-13): these plugins must live in their own top-level
// windows instead of the in-main 640px panel, matching v1's final behaviour.
// Each window owns a second Flutter engine running the "pluginWindowMain"
// entrypoint (lib/plugin_window_main.dart), so the existing panel widgets can
// be reused as-is.
//
// Relationship with v1 (app/src-tauri/src/plugin_window.rs):
//   - borderless like v1 (.decorations(false)); the panel draws its own header;
//   - dragging is started from Flutter ("drag" -> WM_NCLBUTTONDOWN/HTCAPTION) so
//     the buttons drawn in the panel header stay clickable;
//   - the frame edges are resize borders (8 physical px).
//   - listed in the taskbar (WS_EX_APPWINDOW), like v1's plugin windows.
class PluginWindow : public Win32Window {
 public:
  PluginWindow(std::string id,
               std::string title,
               int width,
               int height,
               int min_width,
               int min_height,
               HWND main_window,
               HWND anchor = nullptr);
  ~PluginWindow() override = default;

  // Creates (or re-creates) the window + its engine. The window stays hidden
  // until the first frame is rendered, then it is shown and focused.
  // Not named "CreateWindow": windows.h defines that as a macro.
  bool CreateHostWindow();

  // Attachments carried from the main window (OCR / convert need them at open
  // time). Forwarded to the window engine in the `init` message.
  void SetAttachments(std::vector<std::string> attachments) {
    attachments_ = std::move(attachments);
  }

  // Prefilled input (chat: the query the user searched for).
  void SetQuery(std::string query) { query_ = std::move(query); }

  const std::string& id() const { return id_; }
  int min_width() const { return min_width_; }
  int min_height() const { return min_height_; }

  // Notifies the window engine and destroys the HWND (the C++ object survives
  // so the same plugin can be opened again later).
  void CloseFromHost();

  // Re-glues an attached preview (the sandbox) to its anchor window's left
  // edge. Called by the anchor on move / resize / show / hide.
  void SyncToAnchor();

  // Sets the window rectangle in physical pixels (used by the "preview open"
  // layout: AI window narrows to 720×720, preview sits at its left).
  void SetBoundsPhysical(int x, int y, int width, int height);

 protected:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window,
                         UINT const message,
                         WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  void ApplyBorderlessAndPlace();
  // Attached preview: no taskbar entry, glued to the left edge of `anchor_`.
  void PlaceBesideAnchor();
  // Manual resize (WM_NCLBUTTONDOWN + WM_MOUSEMOVE): the OS sizing loop needs
  // WS_THICKFRAME, which the borderless plugin windows intentionally lack.
  //
  // [capture] = true only for the WM_NCLBUTTONDOWN path. When the drag starts
  // inside Flutter (the panel draws its own 6px edge handles → channel
  // "resizeStart"), the mouse must stay with Flutter, so no SetCapture.
  void BeginResize(int hit, bool capture = true);
  void UpdateResize();
  void EndResize();
  void SendInit();

  std::string id_;
  std::string title_;
  int width_;
  int height_;
  int min_width_;
  int min_height_;
  HWND main_window_ = nullptr;
  // Non-null for the sandbox preview window: it opens beside the window that
  // asked for it (the AI chat window), instead of centred on the main window.
  HWND anchor_ = nullptr;
  bool notified_ = false;
  // First Flutter frame already shown: from then on a sync may re-show the
  // attached preview when its anchor comes back from hidden / minimized.
  bool revealed_ = false;
  // 手动缩放状态（见 BeginResize）。
  bool resizing_ = false;
  int resize_hit_ = 0;
  POINT resize_start_cursor_ = {0, 0};
  RECT resize_start_rect_ = {0, 0, 0, 0};
  std::vector<std::string> attachments_;
  std::string query_;

  std::unique_ptr<flutter::FlutterViewController> controller_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  // "lunac/sandbox-host": lets any panel (chat window included) open the
  // sandbox preview window. Same channel name as the main engine's.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      sandbox_channel_;
  // "lunac/drop": WM_DROPFILES -> Dart (attachment chips). Same channel name /
  // method as the main window (lib/platform/drop.dart).
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> drop_channel_;
  // "lunac/clipboard": Ctrl+V with files in the clipboard
  // (lib/platform/clipboard_files.dart).
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      clipboard_channel_;
};

// Hard cap: at most two plugin windows at the same time (user decision).
//
// The *policy* (asking the user "close the oldest one or cancel?") lives in
// Dart; this class only executes open/close and reports closed windows back to
// the main engine channel "lunac/plugin-host".
class PluginWindowManager {
 public:
  static PluginWindowManager* GetInstance();

  void SetHostChannel(flutter::MethodChannel<flutter::EncodableValue>* channel);
  void SetMainWindow(HWND main_window);

  // True when the window was created (or was already open).
  bool Open(const std::string& id,
            const std::string& title,
            int width,
            int height,
            int min_width,
            int min_height,
            std::vector<std::string> attachments = std::vector<std::string>(),
            std::string query = std::string(),
            HWND anchor = nullptr);
  void Close(const std::string& id);
  bool IsOpen(const std::string& id) const;
  int OpenCount() const;
  // OpenCount() excluding the sandbox preview window (it does not consume the
  // "at most two plugin windows" quota).
  int PluginOpenCount() const;
  // Oldest open plugin id (used by the "close the oldest one" choice).
  std::string OldestOpenId() const;
  void CloseAll();

  // Called by PluginWindow::OnDestroy (user closed / host closed).
  void OnWindowDestroyed(const std::string& id);
  // Closes the sandbox preview window when the window it was attached to
  // (the AI chat window) disappears.
  void OnAnchorDestroyed(HWND anchor);
  // Keeps the attached preview glued to `anchor` (move / resize / show / hide).
  void SyncSandbox(HWND anchor);

  // 「打开预览」的双窗布局（用户 2026-11-14）：AI 窗收窄到 720×720 并保留左缘
  // 作为粘合线，预览窗 1280×720 贴在它的左侧；整块夹进工作区。
  // 只对 AI 助手窗生效（主搜索窗 / 其它插件窗保持原样）。
  void LayoutForPreview(HWND anchor);

  // 预览窗关闭后把 AI 窗宽度恢复（1280 逻辑像素），左缘不动、夹进工作区。
  void RestoreAnchorWidth(HWND anchor);

  // ── 应用预览（二档：停靠浮窗）─────────────────────────────────
  // 把目标 exe 的真实窗口挪到 `screen_rect`（屏幕物理像素），owner 设为预览窗；
  // 不改父窗口（不做 SetParent），停靠期跟随预览窗移动 / 缩放，还回时恢复原矩形。
  bool DockApp(HWND host, const std::string& target, const RECT& screen_rect,
               std::string* error);
  // [host] 为空 = 强制还回（切换 / 重开）；传宿主 HWND 时只在它自己停靠时才还回。
  void UndockApp(HWND host = nullptr);
  void SyncDock(HWND host);

 private:
  PluginWindowManager() = default;

  // 这个 HWND 是哪个插件窗（没有则空串；主搜索窗也返回空串）。
  std::string IdOfHandle(HWND handle) const;

  // 当前停靠的应用窗（同一时刻只停靠一个）。
  struct DockedApp {
    HWND hwnd = nullptr;
    HWND host = nullptr;
    RECT original_rect = {0, 0, 0, 0};
    LONG_PTR original_ex_style = 0;
    // 相对宿主客户区原点的矩形（宿主移动 / 缩放后按它重贴）。
    RECT rel_rect = {0, 0, 0, 0};
    std::string target;
  };
  DockedApp dock_;

  // 打开预览前 AI 窗的原始矩形（关闭预览时「回弹」到这里；用户 2026-11-14 报障）。
  RECT chat_restore_rect_ = {0, 0, 0, 0};
  bool chat_restore_valid_ = false;

  std::map<std::string, std::unique_ptr<PluginWindow>> windows_;
  // Insertion order of the currently open windows (oldest first).
  std::vector<std::string> open_order_;
  // Window the sandbox preview was attached to (nullptr = not open / no anchor).
  HWND sandbox_anchor_ = nullptr;
  flutter::MethodChannel<flutter::EncodableValue>* host_channel_ = nullptr;
  HWND main_window_ = nullptr;
};

#endif  // RUNNER_PLUGIN_WINDOW_H_
