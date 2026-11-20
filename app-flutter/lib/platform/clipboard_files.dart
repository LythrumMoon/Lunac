// 平台薄层：剪贴板里的**文件**（资源管理器里 Ctrl+C 复制的文件 / 文件夹）。
//
// Flutter 的 `Clipboard.getData` 只读纯文本；资源管理器复制文件时剪贴板里是
// `CF_HDROP`（没有文本），所以默认粘贴什么都拿不到。这里走平台通道，原生侧读
// `CF_HDROP` 后把完整路径数组交回 Dart（实现位置：`windows/runner/flutter_window.cpp`
// 与 `plugin_window.cpp` 的 `lunac/clipboard` 通道）。
//
// 只负责「读路径」；插不插附件、插到哪里由面板决定（与 `drop.dart` 同一分工）。
import 'package:flutter/services.dart';

const MethodChannel _channel = MethodChannel('lunac/clipboard');

/// 读剪贴板里的文件路径；没有文件 / 无宿主（单测）时返回空表。
Future<List<String>> readClipboardFiles() async {
  try {
    final raw = await _channel.invokeMethod<List<Object?>>('files');
    if (raw == null) {
      return const [];
    }
    return [
      for (final item in raw)
        if (item is String && item.isNotEmpty) item,
    ];
  } on MissingPluginException {
    return const [];
  } on PlatformException {
    return const [];
  }
}
