// 平台薄层：独立附件抽屉窗口（runner 的第二个 Flutter engine）。
//
// 链路：主窗口 Dart --(lunac/attach-host: update)--> 原生
//       --> 抽屉窗口 Dart(lunac/attach: update) --(resize)--> 原生吸附窗口
//       <--(lunac/attach: remove)--- 抽屉窗口 Dart
import 'package:flutter/services.dart';

const MethodChannel _channel = MethodChannel('lunac/attach-host');

/// 把附件列表同步给独立抽屉窗口（空列表 = 抽屉收起）。
Future<void> syncAttachmentWindow(List<String> paths) async {
  try {
    await _channel.invokeMethod<void>('update', List<String>.of(paths));
  } catch (_) {
    // 无平台通道（单测 / 非 Windows）：忽略。
  }
}

/// 预热抽屉窗口的第二个 engine（启动动画结束后调用）。
///
/// 抽屉是按需创建的；第一次粘贴文件时才建引擎会把主窗口卡住几百毫秒。
/// 提前创建、宽度 0 时窗口保持隐藏，粘贴就只剩下窗口吸附的开销。
Future<void> warmUpAttachmentDrawer() async {
  try {
    await _channel.invokeMethod<void>('warmup');
  } catch (_) {
    // 无平台通道：忽略。
  }
}

/// 立即隐藏 / 恢复抽屉窗口（与主窗口同帧显隐）。
Future<void> setAttachmentDrawerVisible(bool visible) async {
  try {
    await _channel.invokeMethod<void>('visible', visible);
  } catch (_) {
    // 无平台通道：忽略。
  }
}

/// 退出时销毁抽屉窗口与第二个 engine；必须在主窗口已经隐藏之后调用。
Future<void> shutdownAttachmentDrawer() async {
  try {
    await _channel.invokeMethod<void>('shutdown');
  } catch (_) {
    // 无平台通道：忽略。
  }
}

/// 前台窗口是否属于本进程（主窗或抽屉窗）。
///
/// 主窗的「失焦自动隐藏」用它把两块窗当成一个整体：点抽屉时主窗不该藏。
/// 通道不可用时返回 false（保持原来的行为）。
Future<bool> isAppForeground() async {
  try {
    return await _channel.invokeMethod<bool>('appForeground') ?? false;
  } catch (_) {
    return false;
  }
}

/// 抽屉窗口里点 × 时，原生把索引转回来；顶部「一键删除」浮层回 `clear`。
void startAttachmentDrawerBridge(
  void Function(int index) onRemove, {
  void Function()? onClear,
}) {
  _channel.setMethodCallHandler((call) async {
    if (call.method == 'remove' && call.arguments is int) {
      onRemove(call.arguments as int);
    } else if (call.method == 'clear') {
      onClear?.call();
    }
  });
}

void stopAttachmentDrawerBridge() {
  _channel.setMethodCallHandler(null);
}
