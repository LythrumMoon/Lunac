// 平台薄层：窗口形态与窗口级动作（M1）。
//
// M1 出口条件：无边框 / 常驻置顶 / 680 宽（docs/flutter-migration.md §四）。
// 高度随「搜索栏 + 结果行数」伸缩（用户 2026-11-05 裁决），由 `resizeMainWindowHeight` 施加。
// 所有 `window_manager` 调用都收口在这里，UI 代码不直接依赖该插件。
import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'agent.dart';
import 'attachment_window.dart';
import 'plugin_runtime.dart';
import 'plugin_window.dart';


/// 主窗口宽度固定 680；高度 = 搜索栏 + 结果行（由界面按内容算出来）。
const double kMainWindowWidth = 680;

/// 最小高度 = 只有搜索栏一行（结果抽屉收起时）；搜索框选中后向下展开。
const double kMainWindowMinHeight = 68;

/// 最大高度 = 详细搜索大界面的固定档（640）；紧凑搜索仍由界面自己限制在 350。
const double kMainWindowMaxHeight = 640;

/// 关闭请求只隐藏窗口（`setPreventClose` 拦截）；`quitApp()` 的 destroy 不受影响。
class _CloseToTrayListener extends WindowListener {
  @override
  void onWindowClose() {
    hideMainWindow();
  }
}

final WindowListener _closeToTrayListener = _CloseToTrayListener();

/// 失焦自动隐藏（v1 的 JS 侧 150ms 去抖 + 前台守卫冷却）。
///
/// 与 v1 的差别：v1 在 Rust 里按「前景窗口 PID 是不是本进程」判断（40ms 轮询），
/// v2 这里用 `window_manager` 的焦点事件 —— 目前只有一个窗口，两者等价；
/// 以后有插件窗/聊天窗时要改成按 PID 判断（见 docs 清单）。
class _AutoHideListener extends WindowListener {
  @override
  void onWindowBlur() => _scheduleAutoHide();

  @override
  void onWindowFocus() {
    _autoHideTimer?.cancel();
    _stopForegroundWatch();
  }
}

final WindowListener _autoHideListener = _AutoHideListener();

Timer? _autoHideTimer;

/// 主窗失焦、但前台仍是本进程（用户在点抽屉窗）时的轮询。
///
/// 两块窗是一个整体：抽屉在前台时不隐藏；一旦前台不再是本进程就立刻隐藏，
/// 避免「点过抽屉后切到别的应用，窗口反而不藏了」。
Timer? _foregroundWatch;

/// 托盘右键菜单是否打开（由 `tray.dart` 的菜单事件转发）。
///
/// 菜单是系统模态弹窗：打开期间隐藏主窗会把它一起关掉（用户看到的「菜单闪一下
/// 就消失」）。开菜单时顺便起一个 30s 兜底，防止关闭事件丢失后永远不自动隐藏。
bool _trayMenuOpen = false;
Timer? _trayMenuFailSafe;

/// 退出前要撤掉的外部资源（托盘图标由 `tray.dart` 注册，避免平台层互相依赖）。
void Function()? quitCleanup;

/// 主窗当前是否可见。
///
/// 后台占用压缩：窗口隐藏时把「热键 / 单实例轮询」降频（见 `platform/hotkey.dart`）。
/// 只反映我们自己的显示 / 隐藏动作，够用且不需要每次去问平台。
final ValueNotifier<bool> mainWindowVisible = ValueNotifier<bool>(false);

/// 托盘菜单开 / 关（`tray.dart` 调用）。
void setTrayMenuOpen(bool open) {
  _trayMenuOpen = open;
  _trayMenuFailSafe?.cancel();
  _trayMenuFailSafe = null;
  if (open) {
    _trayMenuFailSafe = Timer(const Duration(seconds: 30), () {
      _trayMenuOpen = false;
    });
  }
}

/// 宽限期：刚唤出 / 正在拖拽时，短暂失焦不应立刻把窗口藏掉。
DateTime _autoHideGraceUntil = DateTime.fromMillisecondsSinceEpoch(0);

void _extendAutoHideGrace([
  Duration duration = const Duration(milliseconds: 1500),
]) {
  final until = DateTime.now().add(duration);
  if (until.isAfter(_autoHideGraceUntil)) {
    _autoHideGraceUntil = until;
  }
}

/// 面板打开期间暂停「失焦自动隐藏」（v1 `pluginActive` 的豁免口径）。
///
/// 由搜索页面在任一大面板开合时写入：面板打开时主窗不因失焦藏起来，
/// 与 v1 `win.onFocusChanged` 里 `!pluginActive` 的判据一致。
bool _autoHideSuspended = false;

void setAutoHideSuspended(bool suspended) {
  if (_autoHideSuspended == suspended) {
    return;
  }
  _autoHideSuspended = suspended;
  if (suspended) {
    // 面板刚打开时可能已有在途的失焦计时，直接撤掉。
    _autoHideTimer?.cancel();
    _autoHideTimer = null;
    _stopForegroundWatch();
  }
}

void _scheduleAutoHide() {
  _autoHideTimer?.cancel();
  if (_autoHideSuspended) {
    return;
  }
  _autoHideTimer = Timer(const Duration(milliseconds: 150), () {
    unawaited(_checkAutoHide());
    _startForegroundWatch();
  });
}

void _startForegroundWatch() {
  _foregroundWatch?.cancel();
  _foregroundWatch = Timer.periodic(
    const Duration(milliseconds: 250),
    (_) => unawaited(_checkAutoHide()),
  );
}

void _stopForegroundWatch() {
  _foregroundWatch?.cancel();
  _foregroundWatch = null;
}

/// 主窗失焦后判断要不要隐藏（隐藏条件：既不是前台窗口、前台也不属于本进程）。
Future<void> _checkAutoHide() async {
  try {
    if (!await windowManager.isVisible()) {
      _stopForegroundWatch();
      return;
    }
    if (_autoHideSuspended) {
      _stopForegroundWatch();
      return;
    }
    if (await windowManager.isFocused()) {
      _stopForegroundWatch();
      return;
    }
    if (DateTime.now().isBefore(_autoHideGraceUntil)) {
      return;
    }
    // 托盘菜单打开期间不隐藏（否则系统菜单会被一起关掉）。
    if (_trayMenuOpen) {
      return;
    }
    // 抽屉窗是本进程的另一个窗口：点它时主窗不该藏（用户 2026-11-09 裁决）。
    if (await isAppForeground()) {
      return;
    }
    _stopForegroundWatch();
    await hideMainWindow();
  } catch (_) {
    // 无平台通道（flutter test）或窗口正在销毁：忽略。
    _stopForegroundWatch();
  }
}

/// 初始化并显示主窗口：无边框、置顶、680 宽、高度初始为最大档（随后按结果收缩）。
///
/// [visible] = false 时只建窗不显示（开机自启的 `--background`：静默进托盘，
/// 等热键 / 单实例唤出）。
Future<void> initMainWindow({bool visible = true}) async {
  await windowManager.ensureInitialized();
  const options = WindowOptions(
    // 启动时只有搜索栏一行；结果抽屉展开后再按内容长高。
    size: Size(kMainWindowWidth, kMainWindowMinHeight),
    minimumSize: Size(kMainWindowWidth, kMainWindowMinHeight),
    maximumSize: Size(kMainWindowWidth, kMainWindowMaxHeight),
    center: true,
    title: 'Lunac',
    titleBarStyle: TitleBarStyle.hidden,
    alwaysOnTop: true,
    // 与 v1 一致：窗口本身透明，只画「搜索框 + 结果区」两块面板
    // （用户 2026-11-08 裁决：去掉最底层那块面板）。
    backgroundColor: Colors.transparent,
  );
  await windowManager.waitUntilReadyToShow(options, () async {
    // 全窗口交给 Flutter 绘制：runner 侧窗口是 WS_POPUP，这里再关掉
    // window_manager 的「客户端内缩 8px」，否则面板四周会多出一圈透明边。
    await windowManager.setAsFrameless();
    // 与 v1 一致：无边框小窗不允许最大化（双击 / Win+↑ 都不该把它铺满）。
    await windowManager.setMaximizable(false);
    if (visible) {
      await windowManager.show();
      await windowManager.focus();
    }
    mainWindowVisible.value = visible;
  });
  // 与 v1 一致：Alt+F4 / 系统关闭 = 收进托盘，真退出只走托盘菜单的「退出 Lunac」。
  await windowManager.setPreventClose(true);
  windowManager.addListener(_closeToTrayListener);
  windowManager.addListener(_autoHideListener);
  _extendAutoHideGrace();
}

/// 从无边框窗口的拖拽区发起移动（搜索栏背景的拖拽层调用）。
Future<void> startWindowDrag() async {
  // 拖拽期间会短暂失焦：给足宽限，避免拖到一半窗口自己藏了。
  _extendAutoHideGrace(const Duration(seconds: 2));
  await windowManager.startDragging();
}

/// 主窗置顶开关。
///
/// 插件窗是不置顶的普通窗口（与 v1 一致），而主窗常驻置顶；只要有一个插件窗开着，
/// 主窗就必须让位，否则 680 宽的主窗会盖在插件窗上。
Future<void> setMainWindowTopmost(bool topmost) async {
  try {
    await windowManager.setAlwaysOnTop(topmost);
  } catch (_) {
    // 无平台通道（flutter test）：忽略。
  }
}

/// 把窗口高度贴合到「搜索栏 + 结果行数」；宽度保持不变。
///
/// 单测环境没有 window_manager 平台通道时静默跳过（本函数是平台边界）。
Future<void> resizeMainWindowHeight(double height) async {
  try {
    final clamped =
        height.clamp(kMainWindowMinHeight, kMainWindowMaxHeight).toDouble();
    final current = await windowManager.getSize();
    if ((current.height - clamped).abs() < 0.5) {
      return;
    }
    await windowManager.setSize(Size(current.width, clamped));
  } catch (_) {
    // 无平台通道（flutter test / 尚未初始化窗口）：忽略，不让界面崩掉。
  }
}

/// 隐藏主窗口（Esc 在空查询时调用；与 v1 一致，唤回由托盘 / 热键 / 单实例负责）。
Future<void> hideMainWindow() async {
  _autoHideTimer?.cancel();
  _stopForegroundWatch();
  mainWindowVisible.value = false;
  // 抽屉是独立窗口：显式隐藏它，保证与主窗同一帧一起消失（不依赖
  // WM_SHOWWINDOW 的时序）。
  await setAttachmentDrawerVisible(false);
  try {
    await windowManager.hide();
  } catch (_) {
    // 无平台通道（flutter test）：忽略，不让界面崩掉。
  }
}

/// 显示并聚焦主窗口（托盘菜单、热键、单实例唤出调用）。
Future<void> showMainWindow() async {
  _extendAutoHideGrace();
  await windowManager.show();
  await windowManager.focus();
  mainWindowVisible.value = true;
  // 主窗显示后同步恢复抽屉（有附件时才真的显示）。
  await setAttachmentDrawerVisible(true);
}

/// 按当前可见状态切换显示 / 隐藏（托盘左键）。
Future<void> toggleMainWindow() async {
  if (await windowManager.isVisible()) {
    await hideMainWindow();
  } else {
    await showMainWindow();
  }
}

/// 退出应用（托盘菜单「退出 Lunac」等）。
Future<void> quitApp() async {
  _autoHideTimer?.cancel();
  _stopForegroundWatch();
  _trayMenuOpen = false;
  _trayMenuFailSafe?.cancel();
  // 托盘图标等外部资源先撤掉（避免强制退出时留下「幽灵图标」）。
  quitCleanup?.call();
  // agent 是常驻子进程：先停掉，避免应用退出后留下孤儿进程（v1 用 Job Object，
  // v2 这里显式 kill；2s 兜底的 exit(0) 之前来得及）。
  stopAgent();
  // 插件 sidecar 是插件自己的进程：先收掉（正常路径），宿主崩溃时由 Job Object 兜底。
  await killAllPluginSidecars();
  // 插件窗是独立顶层窗：先关掉它们，再销毁主窗（否则主窗先退会留下几个空窗）。
  await closeAllPluginWindows();
  // 退出时的停顿来自引擎析构（会阻塞平台线程）。先把两个窗口一起藏掉，
  // 用户看不到「点了退出、窗口还停一会儿」。
  await setAttachmentDrawerVisible(false);
  try {
    await windowManager.hide();
  } catch (_) {
    // 无平台通道：忽略。
  }
  // 兜底：窗口已经消失，若 2s 内进程还没退干净就直接结束（析构阻塞时也能立刻退出）。
  Timer(const Duration(seconds: 2), () => exit(0));
  await shutdownAttachmentDrawer();
  await windowManager.destroy();
}
