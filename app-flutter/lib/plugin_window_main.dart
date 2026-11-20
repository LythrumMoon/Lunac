// 插件独立窗口的 Dart 入口（runner 的第三个 Flutter engine）。
//
// 窗口由原生 `windows/runner/plugin_window.cpp` 创建（无边框、可缩放、最多两个）；
// 这里按宿主下发的插件 id 渲染对应的面板 —— 面板本身与主页面共用，不复制一份。
//
// 与 v1 的对应关系：v1 这些插件是独立 WebView 窗（`plugin-window.ts`），窗内画插件
// 自己的界面；v2 用同一套原生面板填充整个窗口，关窗 = 点面板标题栏的返回或 Esc。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'platform/appearance.dart';
import 'platform/sandbox.dart';
import 'search/chat_panel.dart';
import 'search/convert_panel.dart';
import 'search/music_panel.dart';
import 'search/ocr_panel.dart';
import 'search/pet_panel.dart';
import 'search/plugin_console_panel.dart';
import 'search/proxy_panel.dart';
import 'search/sandbox_panel.dart';
import 'theme.dart';

const MethodChannel _channel = MethodChannel('lunac/plugin-window');

/// 窗口顶部用于拖拽的高度（逻辑像素）：只有在这一条里按住并移动才会拖动窗口，
/// 面板标题栏里的按钮点一下仍然照常响应（位移不足阈值时不启动拖拽）。
const double _dragStripHeight = 44;

/// 移动超过这个距离才认作「拖窗口」（逻辑像素）。
const double _dragThreshold = 6;

/// 第三个 engine 的实际入口在 `lib/main.dart`（entrypoint 必须落在根库），
/// 那边转调这里。
void runPluginWindowApp() {
  // AI 助手窗也要能打开沙箱预览窗（它跑在本引擎里，不是主引擎）。
  installSandboxWindowBridge();
  // 每个 engine 有独立的 Dart 静态区：主窗口选的主题要在各窗口自己再套一次。
  applySavedTheme();
  // 主窗设置里换主题 / 改配色后，插件窗不用重开也能跟上（v1 是同一个 WebView）。
  startAppearanceWatch();
  runApp(const PluginWindowApp());
}

class PluginWindowApp extends StatelessWidget {
  const PluginWindowApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<int>(
      valueListenable: LunacColors.paletteRevision,
      builder: (context, revision, child) => MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: buildLunacTheme(),
        builder: (context, child) =>
            appearanceBackdrop(child ?? const SizedBox.shrink()),
        home: const PluginWindowPage(),
      ),
    );
  }
}

class PluginWindowPage extends StatefulWidget {
  const PluginWindowPage({super.key});

  @override
  State<PluginWindowPage> createState() => _PluginWindowPageState();
}

class _PluginWindowPageState extends State<PluginWindowPage> {
  String _id = '';
  String _title = '';
  List<String> _files = const [];
  String _query = '';
  bool _initialized = false;

  /// 拖拽起点（局部坐标）；null = 当前没有按在拖拽条上。
  Offset? _dragOrigin;

  /// 置顶状态（面板标题栏的 pin 按钮）。
  bool _pinned = false;

  /// 预览窗（沙箱盘）是否开着：它一关就把 AI 窗「回弹」回打开前的矩形。
  /// 原生侧销毁回调也会做一次，这里是独立兜底（0.8 秒一轮，开销可忽略）。
  bool _previewWasOpen = false;
  Timer? _previewWatch;

  @override
  void initState() {
    super.initState();
    _channel.setMethodCallHandler(_onCall);
    // 宿主先把窗口建好（第一帧前隐藏），收到 ready 才下发 init。
    unawaited(_channel.invokeMethod<void>('ready'));
    _previewWatch = Timer.periodic(
      const Duration(milliseconds: 800),
      (_) => unawaited(_watchPreviewWindow()),
    );
  }

  @override
  void dispose() {
    _previewWatch?.cancel();
    _channel.setMethodCallHandler(null);
    super.dispose();
  }

  Future<void> _watchPreviewWindow() async {
    if (_id != 'chat') {
      return;
    }
    try {
      final open = await _channel.invokeMethod<bool>('sandboxOpen') ?? false;
      if (_previewWasOpen && !open) {
        // 预览窗刚关：让原生把 AI 窗复原到打开预览前的矩形。
        await _channel.invokeMethod<void>('restoreChat');
      }
      _previewWasOpen = open;
    } on MissingPluginException {
      // 无宿主（单测）：停掉探针。
      _previewWatch?.cancel();
    }
  }

  Future<void> _onCall(MethodCall call) async {
    if (call.method != 'init') {
      return;
    }
    final args = call.arguments;
    if (args is! List || args.length < 2) {
      return;
    }
    final files = args.length > 2 && args[2] is List
        ? [for (final item in args[2] as List) if (item is String) item]
        : const <String>[];
    final query = args.length > 3 && args[3] is String ? args[3] as String : '';
    setState(() {
      _id = args[0] is String ? args[0] as String : '';
      _title = args[1] is String ? args[1] as String : '';
      _files = files;
      _query = query;
      _initialized = true;
    });
  }

  /// 正在关闭：先卸载面板树（各面板在 `dispose` 里取消定时器），再让原生销毁窗口。
  bool _closing = false;

  /// 关闭窗口。
  ///
  /// 2026-11-20（release 闪退取证）：原生侧在 `WM_DESTROY` 里直接析构这个窗口的
  /// FlutterEngine，Dart 的 `State.dispose` 不一定会被调度 —— 定时器还活着时销毁
  /// Dart VM，会在事件派发线程上读到已释放的 isolate 状态（dump 里就是
  /// `MessageHandler::HandleMessages` → `EventHandlerImplementation::SendData` →
  /// `dart::Object::HandleImpl` 的非法读）。所以先切到空树让 `dispose` 跑完，
  /// 再走原生关闭。
  Future<void> _close() async {
    if (_closing) {
      return;
    }
    if (mounted && _initialized) {
      // 先让当前按键事件派发完（在 KeyEvent 回调里卸载带焦点的面板会让
      // focus_manager 迭代中被改 —— 日志里那条 `Concurrent modification`）。
      await Future<void>.delayed(Duration.zero);
      if (!mounted) {
        return;
      }
      setState(() => _closing = true);
      await Future<void>.delayed(const Duration(milliseconds: 40));
    }
    try {
      await _channel.invokeMethod<void>('close');
    } on MissingPluginException {
      // 无宿主（单测）：窗口本来就不存在。
    }
  }

  Future<void> _minimize() async {
    try {
      await _channel.invokeMethod<void>('minimize');
    } on MissingPluginException {
      // 无宿主（单测）：忽略。
    }
  }

  Future<void> _togglePin() async {
    final next = !_pinned;
    try {
      final applied = await _channel.invokeMethod<bool>('pin', next);
      if (!mounted) {
        return;
      }
      setState(() => _pinned = applied ?? next);
    } on MissingPluginException {
      if (mounted) {
        setState(() => _pinned = next);
      }
    }
  }

  void _onPointerDown(PointerDownEvent event) {
    _dragOrigin = event.localPosition.dy <= _dragStripHeight
        ? event.localPosition
        : null;
  }

  void _onPointerMove(PointerMoveEvent event) {
    final origin = _dragOrigin;
    if (origin == null) {
      return;
    }
    if ((event.localPosition - origin).distance < _dragThreshold) {
      return;
    }
    _dragOrigin = null;
    unawaited(_channel.invokeMethod<void>('drag'));
  }

  void _onPointerEnd(PointerEvent event) {
    _dragOrigin = null;
  }

  /// 边缘缩放：无边框窗口没有系统缩放边框（Flutter 的 view 子窗又铺满客户区，
  /// 父窗收不到 WM_NCHITTEST），所以由面板自己在四周放 6px 手势条，
  /// 同时给出缩放光标；真正的改尺寸在原生侧（按全局光标位置算）。
  Future<void> _resizeStart(String hit) async {
    try {
      await _channel.invokeMethod<bool>('resizeStart', hit);
    } on MissingPluginException {
      // 无宿主（单测）：窗口本来就不存在。
    }
  }

  Future<void> _resizeUpdate() async {
    try {
      await _channel.invokeMethod<void>('resizeUpdate');
    } on MissingPluginException {
      // 同上。
    }
  }

  Future<void> _resizeEnd() async {
    try {
      await _channel.invokeMethod<void>('resizeEnd');
    } on MissingPluginException {
      // 同上。
    }
  }

  Widget _buildResizeHandles() {
    const double thickness = 6;
    Widget handle(
      String hit,
      MouseCursor cursor, {
      double? left,
      double? top,
      double? right,
      double? bottom,
      double? width,
      double? height,
    }) {
      return Positioned(
        left: left,
        top: top,
        right: right,
        bottom: bottom,
        width: width,
        height: height,
        child: MouseRegion(
          cursor: cursor,
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onPanStart: (_) => unawaited(_resizeStart(hit)),
            onPanUpdate: (_) => unawaited(_resizeUpdate()),
            onPanEnd: (_) => unawaited(_resizeEnd()),
            onPanCancel: () => unawaited(_resizeEnd()),
          ),
        ),
      );
    }

    return Stack(
      children: [
        handle(
          'left',
          SystemMouseCursors.resizeLeftRight,
          left: 0,
          top: thickness,
          bottom: thickness,
          width: thickness,
        ),
        handle(
          'right',
          SystemMouseCursors.resizeLeftRight,
          right: 0,
          top: thickness,
          bottom: thickness,
          width: thickness,
        ),
        handle(
          'top',
          SystemMouseCursors.resizeUpDown,
          left: thickness,
          right: thickness,
          top: 0,
          height: thickness,
        ),
        handle(
          'bottom',
          SystemMouseCursors.resizeUpDown,
          left: thickness,
          right: thickness,
          bottom: 0,
          height: thickness,
        ),
        handle(
          'topLeft',
          SystemMouseCursors.resizeUpLeftDownRight,
          left: 0,
          top: 0,
          width: thickness,
          height: thickness,
        ),
        handle(
          'topRight',
          SystemMouseCursors.resizeUpRightDownLeft,
          right: 0,
          top: 0,
          width: thickness,
          height: thickness,
        ),
        handle(
          'bottomLeft',
          SystemMouseCursors.resizeUpRightDownLeft,
          left: 0,
          bottom: 0,
          width: thickness,
          height: thickness,
        ),
        handle(
          'bottomRight',
          SystemMouseCursors.resizeUpLeftDownRight,
          right: 0,
          bottom: 0,
          width: thickness,
          height: thickness,
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      // 预览窗自己画面板底色（窗口背景透明），这样「拉开」动画才看得到；
      // 其它插件窗仍是整块不透明底色。
      backgroundColor: _id == 'sandbox'
          ? Colors.transparent
          : LunacColors.surface,
      body: Stack(
        fit: StackFit.expand,
        children: [
          Listener(
            onPointerDown: _onPointerDown,
            onPointerMove: _onPointerMove,
            onPointerUp: _onPointerEnd,
            onPointerCancel: _onPointerEnd,
            child: _initialized ? _buildPanel() : const SizedBox.expand(),
          ),
          // 四周 6px 缩放条（含光标提示），压在内容之上。
          _buildResizeHandles(),
        ],
      ),
    );
  }

  /// 「置顶 / 最小化 / 关闭」三连。
  ///
  /// 2026-11-20（用户口径）：**不再**在对焦 / hover 时弹出一行标题栏 —— 三个键直接
  /// 替换各面板标题栏里原来的「返回」（面板照旧自己画标题）。拖动仍从顶部 44px 条开始
  /// （见 `_onPointerDown`），点按钮不受影响。
  Widget _buildWindowControls() {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _barButton(
          tooltip: _pinned ? '取消置顶' : '置顶',
          icon: _pinned ? Icons.push_pin : Icons.push_pin_outlined,
          color: _pinned ? LunacColors.accent : LunacColors.textMuted,
          onPressed: _togglePin,
        ),
        _barButton(
          tooltip: '最小化',
          icon: Icons.horizontal_rule,
          onPressed: _minimize,
        ),
        _barButton(
          tooltip: '关闭',
          icon: Icons.close,
          onPressed: _close,
        ),
      ],
    );
  }

  Widget _barButton({
    required String tooltip,
    required IconData icon,
    required VoidCallback onPressed,
    Color? color,
  }) {
    return IconButton(
      tooltip: tooltip,
      icon: Icon(icon, size: 15),
      color: color ?? LunacColors.textMuted,
      splashRadius: 14,
      visualDensity: VisualDensity.compact,
      constraints: const BoxConstraints.tightFor(width: 28, height: 26),
      padding: EdgeInsets.zero,
      onPressed: onPressed,
    );
  }

  Widget _buildPanel() {
    if (_closing) {
      // 面板已卸载：等原生销毁窗口（见 _close 的注释）。
      return const SizedBox.shrink();
    }
    switch (_id) {
      case 'chat':
        return ChatPanel(
          onClose: _close,
          initialAttachments: _files,
          initialQuery: _query,
          // v1：聊天窗只有一个标题栏（`#detached-header`）—— 标题 + 置顶 / 最小化 / 关闭。
          title: _title.isEmpty ? 'AI 助手' : _title,
          windowControls: _buildWindowControls(),
        );
      case 'music':
        return MusicPanel(onClose: _close, windowControls: _buildWindowControls());
      case 'proxy':
        return ProxyPanel(onClose: _close, windowControls: _buildWindowControls());
      case 'convert':
        return ConvertPanel(
          onClose: _close,
          attachments: _files,
          windowControls: _buildWindowControls(),
        );
      case 'ocr':
        return OcrPanel(
          onClose: _close,
          attachments: _files,
          windowControls: _buildWindowControls(),
        );
      case 'pet':
        // 桌宠控制台（2026-11-19）：独立窗口承载，搜索主窗不再显示搜索栏。
        return PetPanel(onClose: _close, windowControls: _buildWindowControls());
      case 'sandbox':
        return SandboxPanel(
          onClose: _close,
          windowControls: _buildWindowControls(),
        );
      case 'plugin-console':
        // `_query` 由设置页「运行时」按钮带进来（插件 id）；空串时如实说明。
        return PluginConsolePanel(
          onClose: _close,
          pluginId: _query.trim(),
          windowControls: _buildWindowControls(),
        );
      default:
        // 宿主只会开已知 id（`kPluginWindowSpecs`）；走到这里说明两边版本不匹配，
        // 如实说明而不是画一块空白。
        return Center(
          child: Text(
            '未知插件窗口：${_title.isEmpty ? _id : _title}',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
          ),
        );
    }
  }
}
