#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <cstdlib>

#include "flutter_window.h"
#include "utils.h"

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // 2026-11-19 关闭 Impeller（Windows 上仍是实验后端）。
  // 证据：1.0.0 的几次 APPCRASH 都落在 flutter_windows.dll（0xc0000005 / 0xc000041d，
  // offset 0x7cbe00 / 0x1e220），日志里启用的是 Impeller(OpenGLESSDF)；本程序同时有
  // 主窗 / 插件窗 / WebView2 桌宠多个窗口，Impeller 的 surface 管理在这里最容易崩。
  // 通过 embedder 的引擎开关环境变量在 FlutterEngine 创建前关掉，退回 Skia。
  // 2026-11-21 修：**开关名不能带前导 `--`**。
  // flutter_tools 给桌面端传的是 `FLUTTER_ENGINE_SWITCH_n=enable-impeller=false`
  // （见 `flutter_tools/lib/src/desktop_device.dart::addFlag`，测试里也是裸名），
  // 引擎按裸名查表；之前写成 `--enable-impeller=false` 被静默忽略 ⇒ 日志里一直
  // 打印 `Using the Impeller rendering backend`，多引擎 + 图片解码场景继续崩。
  _putenv_s("FLUTTER_ENGINE_SWITCHES", "1");
  _putenv_s("FLUTTER_ENGINE_SWITCH_1", "enable-impeller=false");

  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1280, 720);
  if (!window.Create(L"lunac", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}
