// 平台薄层：唤出时的剪贴板负载（v1 `notify_window_shown` 的判定口径）。
//
// 判定在 Rust（`rust/src/clipboard_payload.rs`）：文件（CF_HDROP）优先，其次
// 图片（CF_DIB / CF_DIBV5 → `<exe 根>\temp\clipboard-<时间戳>.bmp`），最后是文本。
// 本文件只负责 FFI 调用与 JSON 解析 —— 「读到之后怎么用」在 `wake.dart`
// （附件通知器 / 搜索框通知器），由 `main.dart` 串联。
//
// 解析一律容错：坏 JSON、缺字段、旧版库没有该符号都按 empty 处理，绝不抛异常。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 剪贴板负载类型（与 Rust 侧 `kind` 一一对应）。
enum ClipboardPayloadKind { files, image, text, empty }

/// 一次唤出读到的剪贴板负载。
class ClipboardPayload {
  const ClipboardPayload._(this.kind, this.paths, this.text);

  /// 空负载（不可用 / 剪贴板为空 / JSON 非法）。
  const ClipboardPayload.empty() : this._(ClipboardPayloadKind.empty, const [], '');

  final ClipboardPayloadKind kind;

  /// 文件路径（[ClipboardPayloadKind.files]）或图片落盘路径
  /// （[ClipboardPayloadKind.image]，只有一项）。
  final List<String> paths;

  /// 文本（[ClipboardPayloadKind.text]）。
  final String text;

  bool get isEmpty => kind == ClipboardPayloadKind.empty;
}

typedef _PayloadC = Pointer<Utf8> Function();
typedef _PayloadDart = Pointer<Utf8> Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读一次剪贴板负载；库不可用 / 旧版符号缺失时返回 empty。
ClipboardPayload readClipboardPayload() {
  final library = openLunacNative().library;
  if (library == null) {
    return const ClipboardPayload.empty();
  }
  Pointer<Utf8>? raw;
  try {
    final read =
        library.lookupFunction<_PayloadC, _PayloadDart>('lunac_clipboard_payload');
    raw = read();
    if (raw == nullptr) {
      return const ClipboardPayload.empty();
    }
    return parseClipboardPayload(raw.toDartString());
  } catch (_) {
    // 旧版库没有该符号：按「剪贴板为空」处理。
    return const ClipboardPayload.empty();
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

/// 解析 Rust 返回的负载 JSON（纯函数，单测覆盖）。
///
/// 契约：`{"kind":"files","paths":[...]}` / `{"kind":"image","path":"..."}` /
/// `{"kind":"text","text":"..."}` / `{"kind":"empty"}`。
ClipboardPayload parseClipboardPayload(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const ClipboardPayload.empty();
  }
  if (decoded is! Map) {
    return const ClipboardPayload.empty();
  }

  final kind = decoded['kind'];
  if (kind == 'files') {
    final rawPaths = decoded['paths'];
    if (rawPaths is! List) {
      return const ClipboardPayload.empty();
    }
    final paths = <String>[
      for (final item in rawPaths)
        if (item is String && item.isNotEmpty) item,
    ];
    if (paths.isEmpty) {
      return const ClipboardPayload.empty();
    }
    return ClipboardPayload._(
      ClipboardPayloadKind.files,
      List.unmodifiable(paths),
      '',
    );
  }
  if (kind == 'image') {
    final path = decoded['path'];
    if (path is! String || path.isEmpty) {
      return const ClipboardPayload.empty();
    }
    return ClipboardPayload._(
      ClipboardPayloadKind.image,
      List.unmodifiable([path]),
      '',
    );
  }
  if (kind == 'text') {
    final text = decoded['text'];
    if (text is! String || text.isEmpty) {
      return const ClipboardPayload.empty();
    }
    return ClipboardPayload._(ClipboardPayloadKind.text, const [], text);
  }
  return const ClipboardPayload.empty();
}
