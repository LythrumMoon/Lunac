// 平台薄层：剪贴板历史（v1 `load_clipboard_history` / `append_clipboard_entry`）。
//
// 边界：去重 / 置顶 / 30 条上限 / 2000 码元上限 / 落盘都在 `rust/src/clipboard_history.rs`；
// 本文件只把四个 C ABI 包成 Dart 调用。**记录由主窗口在唤出时顺手做**
// （`main.dart` 的 `_handleWake`），不依赖本面板是否打开过（与 v1 同口径）。
//
// 存储：`<exe 根>/ModuleData/history/clipboard-history.json`，字段名与 v1 完全一致
// （`clip_type` / `text` / `file_paths` / `time`），M6 数据迁移可直接拷贝。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条剪贴板记录（与 Rust / v1 的 JSON 字段一一对应）。
class ClipboardEntry {
  const ClipboardEntry({
    required this.clipType,
    required this.text,
    required this.filePaths,
    required this.time,
  });

  /// `text` | `file`（v1 同值）。
  final String clipType;

  /// 记录内容（文件条目也存完整路径）。
  final String text;

  /// 文件条目对应的路径列表（文本条目为空）。
  final List<String> filePaths;

  /// 记录时间（毫秒时间戳）。
  final int time;

  bool get isFile => clipType == 'file';

  /// 列表里显示的一行预览：文件只显示文件名，文本把换行压成空格。
  String get preview {
    final raw = isFile ? _basename(text) : text;
    final oneLine = raw.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (oneLine.length <= 80) {
      return oneLine;
    }
    return '${oneLine.substring(0, 80)}…';
  }

  /// 文本条目的「N 字符」提示；文件条目为空串（v1 `clipboard.chars` 同文案）。
  String get sizeHint => isFile ? '' : '${text.length} 字符';

  static String _basename(String path) {
    final index = path.lastIndexOf(RegExp(r'[\\/]'));
    return index >= 0 && index + 1 < path.length ? path.substring(index + 1) : path;
  }

  static ClipboardEntry? fromJson(Map<String, dynamic> json) {
    final text = json['text'];
    if (text is! String || text.isEmpty) {
      return null;
    }
    return ClipboardEntry(
      clipType: json['clip_type'] is String
          ? json['clip_type'] as String
          : 'text',
      text: text,
      filePaths: [
        for (final path in (json['file_paths'] as List? ?? const []))
          if (path is String) path,
      ],
      time: json['time'] is int ? json['time'] as int : 0,
    );
  }
}

typedef _LoadC = Pointer<Utf8> Function();
typedef _LoadDart = Pointer<Utf8> Function();
typedef _TextI32C = Int32 Function(Pointer<Utf8>);
typedef _TextI32Dart = int Function(Pointer<Utf8>);
typedef _I32C = Int32 Function();
typedef _I32Dart = int Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读全部记录；桥接缺失返回 null（面板显示「不可用」而不是假装空）。
List<ClipboardEntry>? readClipboardHistory() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_LoadC, _LoadDart>('lunac_clipboard_history')();
    if (raw == nullptr) {
      return null;
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return null;
    }
    return [
      for (final item in value)
        if (item is Map<String, dynamic>) ?ClipboardEntry.fromJson(item),
    ];
  } catch (_) {
    return null;
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 记一条（去重 → 置顶 → 最多 30 条）。返回是否调用成功（空 / 超长会被静默忽略）。
bool appendClipboardEntry(String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final append = library.lookupFunction<_TextI32C, _TextI32Dart>(
      'lunac_clipboard_history_append',
    );
    pointer = text.toNativeUtf8();
    return append(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 把一条提到最前（面板里点击复制后调用）。
bool promoteClipboardEntry(String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final promote = library.lookupFunction<_TextI32C, _TextI32Dart>(
      'lunac_clipboard_history_promote',
    );
    pointer = text.toNativeUtf8();
    return promote(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 清空历史（调用方负责二次确认）。
bool clearClipboardHistory() {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    final clear = library.lookupFunction<_I32C, _I32Dart>(
      'lunac_clipboard_history_clear',
    );
    return clear() == 0;
  } catch (_) {
    return false;
  }
}
