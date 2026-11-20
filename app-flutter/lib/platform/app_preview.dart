// 平台薄层：预览窗里的「打开应用」（2026-11-14，用户要求两档）。
//
// 口径（如实）：
//   · 一档「缩略图」= 原生 `PrintWindow(PW_RENDERFULLCONTENT)` 抓一帧写 BMP，
//     只读、不回传任何数据给被捕获的应用；部分 GPU 合成应用 / 主动设了
//     `WDA_EXCLUDEFROMCAPTURE` 的应用会抓到黑屏。
//   · 二档「接管」= 把真实窗口挪到面板区域（owner = 预览窗，不做 SetParent），
//     可正常点击输入；还回时恢复原矩形与样式。UWP / 部分 Electron 应用可能
//     自己重新定位或拒绝，失败会如实返回错误。
//
// 通道只在插件窗引擎里存在（`windows/runner/plugin_window.cpp`）；单测 / 主窗
// 调用时一律返回 null / false。
import 'dart:async';

import 'package:flutter/services.dart';

const MethodChannel _channel = MethodChannel('lunac/plugin-window');

/// 一次抓帧的结果。
class AppCapture {
  const AppCapture({
    this.ok = false,
    this.path = '',
    this.title = '',
    this.width = 0,
    this.height = 0,
    this.error = '',
  });

  final bool ok;

  /// 落盘的 BMP 路径（每次抓帧覆盖同一个文件；Dart 侧按字节读，不用图片缓存）。
  final String path;
  final String title;
  final int width;
  final int height;
  final String error;

  static AppCapture? fromMap(Object? value) {
    if (value is! Map) {
      return null;
    }
    int number(String key) {
      final raw = value[key];
      return raw is num ? raw.toInt() : 0;
    }

    return AppCapture(
      ok: value['ok'] == true,
      path: value['path'] is String ? value['path'] as String : '',
      title: value['title'] is String ? value['title'] as String : '',
      width: number('width'),
      height: number('height'),
      error: value['error'] is String ? value['error'] as String : '',
    );
  }
}

/// 抓一帧目标应用的窗口画面；找不到窗口 / 没有宿主时返回 null。
Future<AppCapture?> captureAppWindow(String target) async {
  if (target.isEmpty) {
    return null;
  }
  try {
    final reply = await _channel.invokeMethod<Object?>('appCapture', target);
    return AppCapture.fromMap(reply);
  } on MissingPluginException {
    return null;
  } on PlatformException {
    return null;
  }
}

/// 接管应用窗口到 [physicalRect]（屏幕物理像素）。
Future<String?> dockAppWindow(String target, Rect physicalRect) async {
  if (target.isEmpty) {
    return '没有应用目标';
  }
  try {
    final reply = await _channel.invokeMethod<Object?>('appDock', <Object?>[
      target,
      physicalRect.left,
      physicalRect.top,
      physicalRect.width,
      physicalRect.height,
    ]);
    if (reply is Map && reply['ok'] == true) {
      return null;
    }
    return reply is Map && reply['error'] is String
        ? reply['error'] as String
        : '接管失败';
  } on MissingPluginException {
    return '预览窗宿主不可用';
  } on PlatformException {
    return '接管失败';
  }
}

/// 还回被接管的应用窗口（恢复原矩形与窗口样式）。
Future<bool> undockAppWindow() async {
  try {
    final ok = await _channel.invokeMethod<bool>('appUndock');
    return ok == true;
  } on MissingPluginException {
    return false;
  } on PlatformException {
    return false;
  }
}
