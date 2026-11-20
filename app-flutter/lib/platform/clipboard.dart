// 平台薄层：唤出时的剪贴板读取（v1 `notify_window_shown` 的对应实现）。
//
// 边界：读取在 Rust（`rust/src/clipboard.rs`：CF_UNICODETEXT / CF_HDROP）；本文件只返回
// 文本或路径列表。「读到之后怎么用」在 `wake.dart`（填入搜索框 / 重跑查询），由
// `main.dart` 串联；文件列表留给附件泡泡（前端改版阶段）使用。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _ClipboardTextC = Pointer<Utf8> Function();
typedef _ClipboardTextDart = Pointer<Utf8> Function();
typedef _ClipboardListC = Pointer<Utf8> Function();
typedef _ClipboardListDart = Pointer<Utf8> Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读系统剪贴板文本；不可用（无库 / 无文本 / 超长）时返回空串。
String readClipboardText() {
  final library = openLunacNative().library;
  if (library == null) {
    return '';
  }
  try {
    final read = library
        .lookupFunction<_ClipboardTextC, _ClipboardTextDart>('lunac_clipboard_text');
    final free = library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string');
    final raw = read();
    if (raw == nullptr) {
      return '';
    }
    try {
      return raw.toDartString();
    } finally {
      free(raw);
    }
  } catch (_) {
    // 旧版库没有该符号：按「剪贴板为空」处理。
    return '';
  }
}

/// 剪贴板里 CF_HDROP 的全部路径（复制文件后粘贴用；不可用时为空列表）。
List<String> readClipboardFilePaths() =>
    _readPathList('lunac_clipboard_file_paths');

/// 剪贴板里扩展名是图片的路径（唤出时判断「剪贴板有图片」，供附件泡泡 / OCR 入口）。
List<String> readClipboardImageFiles() =>
    _readPathList('lunac_clipboard_image_files');

List<String> _readPathList(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return const [];
  }
  Pointer<Utf8>? raw;
  try {
    final read = library.lookupFunction<_ClipboardListC, _ClipboardListDart>(symbol);
    raw = read();
    if (raw == nullptr) {
      return const [];
    }
    final decoded = jsonDecode(raw.toDartString());
    if (decoded is! List) {
      return const [];
    }
    return [
      for (final item in decoded)
        if (item is String) item,
    ];
  } catch (_) {
    return const [];
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      try {
        library
            .lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}
