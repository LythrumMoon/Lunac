// Lunac v2 —— Windows + Linux 双端 Flutter 前端（M1：搜索主页面）。
//
// 边界（见 docs/flutter-migration.md §一 双端架构原则）：
//   · 这里只放**表现层**；agent / 代理引擎 / 文件索引等业务逻辑都在 Rust 侧，经 FFI 或
//     stream-json 通信，不在 Dart 里重写。
//   · 平台差异只允许出现在 `lib/platform/` 与 `rust/` 两个薄层；本文件不得堆
//     `Platform.isXxx` 的分支。
//   · M1 出口：无边框 / 置顶 / 680×400 窗口 + 搜索框 + 结果列表 + 键盘导航 + 动作可执行。
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'platform/agent_status.dart';
import 'platform/app_window.dart';
import 'platform/appearance.dart';
import 'platform/auto_start.dart';
import 'platform/clipboard_history.dart';
import 'platform/clipboard_payload.dart';
import 'platform/cron.dart';
import 'platform/hotkey.dart';
import 'platform/host_log.dart';
import 'platform/mcp_bridge.dart';
import 'platform/plugin_window.dart';
import 'platform/sandbox.dart';
import 'platform/settings.dart';
import 'platform/task_queue.dart';
import 'platform/tray.dart';
import 'platform/updater.dart';
import 'platform/vscode_bridge.dart';
import 'platform/wake.dart';
import 'attachment_drawer_main.dart';
import 'plugin_window_main.dart';
import 'search/search_bridge.dart';
import 'search/search_page.dart';
import 'theme.dart';

/// 第二个 Flutter engine 的入口（独立附件抽屉窗口，见 `attachment_window.cpp`）。
///
/// entrypoint 必须定义在根库（本文件），所以这里只做转发。
@pragma('vm:entry-point')
void attachmentDrawerMain() => runAttachmentDrawerApp();

/// 第三个 Flutter engine 的入口（插件独立窗口，见 `plugin_window.cpp`）。
///
/// 宿主会为每个插件窗开一个引擎（最多两个），入口都走这里。
@pragma('vm:entry-point')
void pluginWindowMain() => runPluginWindowApp();

/// 唤出窗口：先读一次剪贴板负载（v1 在 `notify_window_shown` 里做同一件事），
/// 文件 / 图片交给 `SearchPage` 加进附件栏，文本填进搜索框，再显示 / 切换窗口。
Future<void> _handleWake({required bool toggle}) async {
  final payload = readClipboardPayload();
  switch (payload.kind) {
    case ClipboardPayloadKind.files:
    case ClipboardPayloadKind.image:
      // 追加而不是覆盖：连续唤出时上一批附件可能还没被界面消费。
      if (payload.paths.isNotEmpty) {
        pendingAttachments.value = [
          ...pendingAttachments.value,
          ...payload.paths,
        ];
      }
    case ClipboardPayloadKind.text:
      // 文本口径保持不变：记剪贴板历史 + 交给搜索框。
      if (payload.text.isNotEmpty) {
        _recordClipboardHistory(payload.text);
        pendingSearchQuery.value = payload.text;
      }
    case ClipboardPayloadKind.empty:
      break;
  }
  // 与 v1 一致：每次唤出都重跑当前查询（索引可能已更新）。
  searchRefreshTick.value++;
  if (toggle) {
    await toggleMainWindow();
  } else {
    await showMainWindow();
  }
}

/// 把剪贴板内容记进历史（v1 `applyClipboardText` 的宿主侧行为）。
///
/// 按行拆开：Windows 路径与普通文本都记一条；空行跳过。
/// 长度上限（2000 UTF-16 码元）由 Rust 侧统一把关，这里不重复写判据。
void _recordClipboardHistory(String text) {
  for (final part in text.split('\n')) {
    final trimmed = part.trim();
    if (trimmed.isEmpty) {
      continue;
    }
    appendClipboardEntry(trimmed);
  }
}

/// 启动时静默自更新（v1 `update_check_silent`）：只有「自动更新开着 + 非开发构建 +
/// 真有新版且下载校验通过」才会拉起安装器；交棒成功后退出，由安装器覆盖并重新拉起。
Future<void> _autoUpdateOnStartup() async {
  final dir = dataDirectory();
  if (dir == null || dir.isEmpty) {
    return;
  }
  final handedOver = await autoUpdateOnStartup(installDir: dir);
  if (handedOver) {
    exit(0);
  }
}

Future<void> main(List<String> args) async {
  // 提权子进程（`--lunac-auto-start-task=create|delete`）：只做一次计划任务动作，
  // 在建窗 / 启引擎之前退出（v1 在 `main()` 里 std::process::exit 同一口径）。
  final elevated = parseElevatedAction(args);
  if (elevated != null) {
    exit(handleAutoStartElevated(elevated));
  }
  // 宿主 MCP 桥（agent 以 `lunac.exe --mcp-server` 拉起本进程）：只跑 Rust 侧的 stdio
  // JSON-RPC 循环，**不初始化 Flutter / 不建窗** —— stdout 必须独占给协议。
  if (args.contains('--mcp-server')) {
    exit(serveMcpBridge());
  }
  // 开机自启拉起时带 `--background`：静默进托盘（不闪窗），首次热键即用。
  final background = args.contains('--background');
  // 宿主落盘日志（v1 `temp\logs\lunac-*.log`）：release 没有控制台，启动失败 / 未捕获异常
  // 只能靠它留痕。放在所有可能失败的初始化之前，路径与轮转全在 Rust 侧。
  initHostLog();
  hostLogInfo('host start pid=$pid args=${args.join(' ')}');
  WidgetsFlutterBinding.ensureInitialized();
  // 绑定初始化会重置 `FlutterError.onError`，钩子必须在其之后装。
  installDartLogHooks();
  // 已保存的主题包：在首个 Flutter 帧之前套到运行时色板上（三处 engine 各做一次）。
  applySavedTheme();
  // 后台占用压缩：结果图标的缩略图会进 Flutter 的图片缓存，默认 100MB / 1000 张，
  // 这里压到 32MB / 200 张（32×32 图标足够覆盖多屏结果）。
  PaintingBinding.instance.imageCache
    ..maximumSize = 200
    ..maximumSizeBytes = 32 << 20;
  // 单实例：第二个进程只负责把已有实例唤出，然后退出。
  // 这里在首个 Flutter 帧之前结束，窗口还没显示，用户看不到闪一下。
  if (!acquireSingleInstance()) {
    signalExistingInstance();
    exit(0);
  }
  await initMainWindow(visible: !background);
  // 插件独立窗口的 `closed` 回推（关掉最后一个时把主窗恢复置顶）。
  installPluginWindowBridge();
  // 沙箱盘预览窗通道（AI 助手窗里也要能打开，见 plugin_window_main.dart）。
  installSandboxWindowBridge();
  // 无边框窗口没有标题栏按钮：托盘是「显示 / 退出」的固定入口（Esc 隐藏后靠它唤回）。
  await initTray();
  // 预热应用 / 文件索引：Rust 侧起后台线程，首次输入时结果更快出现。
  warmUpSearchIndex();
  // 定时任务调度线程（幂等；只读 cron.json，用户不用它就没有任何行为）。
  startCronScheduler();
  // 通用后台任务队列 worker（幂等；与定时任务共用「同时只跑一个」单飞闸）。
  startTaskQueue();
  // VSCode 扩展的本机桥（v1 `agent_server.rs`，127.0.0.1:8789）：起不来只记日志，
  // 桌面端照常可用 —— 它只是给扩展用的可选通道。
  startVscodeBridge();
  // 启动时静默自更新（v1 `update_check_silent`）：开关关着 / 开发构建 / 已是最新
  // 都什么也不做；真下载并拉起安装器时才退出交棒。失败不打扰用户。
  _autoUpdateOnStartup();
  // 全局热键 + 单实例唤出：优先事件驱动（主窗句柄交给 Rust，热键线程直接
  // PostMessage 回来），后台零轮询；旧 DLL / 通道不可用时退回轮询。
  final wakeByEvent = await startWakeEvents(
    onHotkey: () => _handleWake(toggle: true),
    onActivate: () => _handleWake(toggle: false),
  );
  startGlobalHotkey();
  if (!wakeByEvent) {
    startWakeWatcher(
      onHotkey: () => _handleWake(toggle: true),
      onActivate: () => _handleWake(toggle: false),
    );
  }
  // M0 的 Dart ↔ Rust 探针：`flutter run` 日志里能直接看到桥接状态。
  if (kDebugMode) {
    debugPrint('Lunac M0 agent status: ${readAgentStatus()}');
  }
  runApp(const LunacApp());
}

class LunacApp extends StatelessWidget {
  const LunacApp({super.key});

  @override
  Widget build(BuildContext context) {
    // 主题切换 = 色板字段变化 + 版本号自增；这里跟着重建 MaterialApp（含 ThemeData）。
    return ValueListenableBuilder<int>(
      valueListenable: LunacColors.paletteRevision,
      builder: (context, revision, child) => MaterialApp(
        title: 'Lunac',
        debugShowCheckedModeBanner: false,
        theme: buildLunacTheme(),
        // 自定义背景图（v1 `--bg-image`）：画在透明窗与玻璃面板之间。
        builder: (context, child) =>
            appearanceBackdrop(child ?? const SizedBox.shrink()),
        // 首次启动不聚焦：先只显示圆形按钮 1.5s，再拉开搜索栏；结果抽屉等聚焦后展开。
        home: const SearchPage(
          focusOnMount: false,
          revealDelay: Duration(milliseconds: 1500),
          // 桌宠右键菜单「打开控制台」：生产实例轮询 C ABI 事件队列（单测不启动常驻定时器）。
          pollPetEvents: true,
        ),
      ),
    );
  }
}
