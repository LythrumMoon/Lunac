// 平台薄层：系统文件拖放（v1 `onDragDropEvent` 的等价实现）。
//
// 实现位置：`windows/runner/flutter_window.cpp`（主窗）与 `plugin_window.cpp`
// （插件窗）—— 窗口 `DragAcceptFiles` + 处理 `WM_DROPFILES`，把真实路径数组经
// `lunac/drop` 通道的 `drop` 方法发给 Dart。为什么放 runner：Flutter 自身不处理
// Windows 的文件拖放；要么引第三方插件，要么在平台壳里用 Win32（本项目选后者，
// 平台差异只落在 runner / `lib/platform/`）。
//
// 只负责「把路径交出去」；加不加入附件、怎么显示由面板决定。
//
// 多个监听者：搜索页常驻、对话面板打开时叠在它上面。**只发给最后注册的那个**
// —— 顶层浮层优先，浮层销毁后自然回落到下面的监听者（不会一次拖放加进两处）。
import 'package:flutter/services.dart';

const MethodChannel _channel = MethodChannel('lunac/drop');

typedef FileDropListener = void Function(List<String> paths);

final List<FileDropListener> _listeners = <FileDropListener>[];

/// 监听文件拖放（后注册的优先）。
void addFileDropListener(FileDropListener onDrop) {
  _listeners.add(onDrop);
  _ensureHandler();
}

/// 取消监听（面板销毁时调用，避免回调打到已销毁的 State）。
void removeFileDropListener(FileDropListener onDrop) {
  _listeners.remove(onDrop);
}

void _ensureHandler() {
  _channel.setMethodCallHandler((call) async {
    if (call.method != 'drop') {
      return null;
    }
    final raw = call.arguments;
    if (raw is List) {
      final paths = <String>[
        for (final item in raw)
          if (item is String && item.isNotEmpty) item,
      ];
      if (paths.isNotEmpty && _listeners.isNotEmpty) {
        _listeners.last(paths);
      }
    }
    return null;
  });
}
