// 平台薄层：插件的「独立窗口」（2026-11-13 用户裁决）。
//
// v1 的插件面板是**独立 WebView 窗**（`app/src-tauri/src/plugin_window.rs`）：
// 音乐 1280×720 / 代理 1280×720（min 960×600）/ 文件转换 820×470（min 780×430）/
// AI 助手 1280×720（min 720×420）。v2 把它们改成**独立 Flutter 窗口**（每个窗口
// 一个 FlutterEngine，入口 `pluginWindowMain`），主页面不再用 640 档内嵌面板承载它们。
//
// 通道分工：
//   · `lunac/plugin-host`  —— 主引擎侧：open / close / list / oldest / count / closeAll，
//     窗口被关掉时回推 `closed`（id）；
//   · `lunac/plugin-window` —— 窗口引擎侧：`ready` / `init`（回 id + 标题 + 附件 + 查询词）/
//     `drag` / `minimize` / `close`。
//
// 硬上限两个窗口（用户裁决）；「关最早还是取消」的选择在 Dart 侧问用户，
// C++ 只做执行者（见 `plugin_window.cpp`）。
import 'dart:async';

import 'package:flutter/services.dart';

/// 一个插件窗口的静态形态（尺寸沿用 v1 的缺省 / 清单值）。
class PluginWindowSpec {
  const PluginWindowSpec({
    required this.id,
    required this.title,
    required this.width,
    required this.height,
    required this.minWidth,
    required this.minHeight,
  });

  final String id;
  final String title;
  final int width;
  final int height;
  final int minWidth;
  final int minHeight;
}

/// 五个独立窗口的形态。数值来自 v1：
/// `plugin_window.rs:139-233`（缺省与各插件档）与各插件 `lunac-plugin.json` 的 `window` 段。
const Map<String, PluginWindowSpec> kPluginWindowSpecs = {
  'chat': PluginWindowSpec(
    id: 'chat',
    title: 'AI 助手',
    width: 1280,
    height: 720,
    minWidth: 720,
    minHeight: 420,
  ),
  'music': PluginWindowSpec(
    id: 'music',
    title: '音乐',
    width: 1280,
    height: 720,
    minWidth: 300,
    minHeight: 120,
  ),
  'proxy': PluginWindowSpec(
    id: 'proxy',
    title: '代理',
    width: 1280,
    height: 720,
    minWidth: 960,
    minHeight: 600,
  ),
  'convert': PluginWindowSpec(
    id: 'convert',
    title: '文件转换',
    width: 820,
    height: 470,
    minWidth: 780,
    minHeight: 430,
  ),
  // OCR 是 `layout.takeover` 插件：v1 里它接管**主窗**的 detached 视图
  // （宽 = 用户主窗宽，高 520 设计 px；见 `main.ts::applyWindowSize`）。
  // v2 按用户裁决改成独立窗，尺寸沿用那块的形状：820×520。
  'ocr': PluginWindowSpec(
    id: 'ocr',
    title: 'OCR 文字识别',
    width: 820,
    height: 520,
    minWidth: 560,
    minHeight: 360,
  ),
  // 沙箱盘预览窗（2026-11-13）：依附 AI 助手的只读预览窗，**不占**「最多两个」的额度
  // （C++ 侧按 id 豁免）。2026-11-14 起默认 1280×720，同时把 AI 窗收窄到 720×720，
  // 两块以 AI 窗左缘（粘合线）相连。
  'sandbox': PluginWindowSpec(
    id: 'sandbox',
    title: '沙箱盘',
    width: 1280,
    height: 720,
    minWidth: 560,
    minHeight: 380,
  ),
  // 插件运行时控制台（2026-11-15）：给「带 sidecar 的插件」起进程 / 信任 / 调用 / 看事件。
  // 一个窗口服务所有插件，具体操作哪个由 `query`（插件 id）带进来。
  'plugin-console': PluginWindowSpec(
    id: 'plugin-console',
    title: '插件运行时',
    width: 860,
    height: 560,
    minWidth: 560,
    minHeight: 360,
  ),
  // 桌宠控制台（2026-11-19 用户裁决）：与其余插件一致走**独立窗口**，
  // 不再挂在搜索主窗里（主窗的搜索栏不允许出现在插件界面上）。
  // 桌宠形象本身是另一扇透明窗（Rust 的 WebView2，见 `rust/src/pet.rs`）。
  'pet': PluginWindowSpec(
    id: 'pet',
    title: '桌宠',
    width: 820,
    height: 640,
    minWidth: 600,
    minHeight: 420,
  ),
};

/// 打开一个插件窗口的结果。
enum PluginOpenResult {
  /// 窗口已建好（或本来就在，已提到前台）。
  opened,

  /// 宿主说「最多两个」——由界面问用户要不要关掉最早那个。
  limitReached,

  /// 没有插件窗口宿主（单测 / Linux，没有 `lunac/plugin-host`）。
  unavailable,
}

const MethodChannel _host = MethodChannel('lunac/plugin-host');

final StreamController<String> _closed = StreamController<String>.broadcast();

bool _handlerInstalled = false;

/// 窗口被关闭（用户点关闭 / 主程序关闭）时推送插件 id。
Stream<String> get pluginWindowClosed => _closed.stream;

/// 宿主是否已就绪（`installPluginWindowBridge` 之后为真）。
///
/// 生产环境在 `main()` 里装一次；单测不装 —— 测试环境没有原生侧，平台通道
/// 不会回复（`invokeMethod` 会永远等下去），所以这里必须先判断，
/// 让调用方立即走内嵌面板的回退路径。
bool get pluginWindowHostReady => _handlerInstalled;

/// 安装 `closed` 回调（幂等）。`main()` 里调用一次即可。
void installPluginWindowBridge() {
  if (_handlerInstalled) {
    return;
  }
  _handlerInstalled = true;
  _host.setMethodCallHandler((call) async {
    if (call.method == 'closed' && call.arguments is String) {
      _closed.add(call.arguments as String);
    }
    return null;
  });
}

/// 打开一个插件窗口。
///
/// [attachments] / [query] 会随 `open` 一起传给窗口（OCR / 文件转换要附件，
/// AI 助手要预填查询词）。
Future<PluginOpenResult> openPluginWindow(
  String id, {
  List<String> attachments = const [],
  String query = '',
}) async {
  final spec = kPluginWindowSpecs[id];
  if (spec == null || !_handlerInstalled) {
    return PluginOpenResult.unavailable;
  }
  try {
    final status = await _host.invokeMethod<String>('open', {
      'id': spec.id,
      'title': spec.title,
      'width': spec.width,
      'height': spec.height,
      'minWidth': spec.minWidth,
      'minHeight': spec.minHeight,
      'attachments': attachments,
      'query': query,
    });
    return status == 'limit'
        ? PluginOpenResult.limitReached
        : PluginOpenResult.opened;
  } on MissingPluginException {
    return PluginOpenResult.unavailable;
  } on PlatformException {
    return PluginOpenResult.unavailable;
  }
}

/// 关闭指定插件窗口（不存在也算成功）。
Future<void> closePluginWindow(String id) async {
  if (!_handlerInstalled) {
    return;
  }
  try {
    await _host.invokeMethod<void>('close', id);
  } on MissingPluginException {
    // 无宿主（单测）时忽略。
  }
}

/// 当前打开的插件窗口 id 列表。
Future<List<String>> listPluginWindows() async {
  if (!_handlerInstalled) {
    return const [];
  }
  try {
    final ids = await _host.invokeMethod<List<dynamic>>('list');
    return [
      for (final id in ids ?? const [])
        if (id is String) id,
    ];
  } on MissingPluginException {
    return const [];
  }
}

/// 最早打开的插件窗口 id（空串 = 没有）。
Future<String> oldestPluginWindow() async {
  if (!_handlerInstalled) {
    return '';
  }
  try {
    return await _host.invokeMethod<String>('oldest') ?? '';
  } on MissingPluginException {
    return '';
  }
}

/// 打开的插件窗口数量。
Future<int> pluginWindowCount() async {
  if (!_handlerInstalled) {
    return 0;
  }
  try {
    return await _host.invokeMethod<int>('count') ?? 0;
  } on MissingPluginException {
    return 0;
  }
}

/// 关闭全部插件窗口（退出应用前调用）。
Future<void> closeAllPluginWindows() async {
  if (!_handlerInstalled) {
    return;
  }
  try {
    await _host.invokeMethod<void>('closeAll');
  } on MissingPluginException {
    // 忽略。
  }
}
