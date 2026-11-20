// 计划留档（`docs/todo.md` §待办 3-28.3）：批准前改过的计划与原文一起落盘。
//
// 落盘位置与内容由 Rust 侧负责（`<exe 根>/ModuleData/plans/plan-*.json`，
// 见 `rust/src/plan_archive.rs`）；本文件只做一次 FFI 调用。
// **失败不打断批准**：留档是事后取证，写不进去只记一条宿主日志，用户照样能批计划。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'host_log.dart';
import 'native_bridge.dart';

/// 留档成功返回文件路径；失败 / 桥接缺失返回 null（只记日志）。
String? archivePlan({
  required String original,
  required String edited,
  String note = '',
}) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? rawOriginal;
  Pointer<Utf8>? rawEdited;
  Pointer<Utf8>? rawNote;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_ArchiveC, _ArchiveDart>(
      'lunac_plan_archive',
    );
    rawOriginal = original.toNativeUtf8();
    rawEdited = edited.toNativeUtf8();
    rawNote = note.toNativeUtf8();
    raw = call(rawOriginal, rawEdited, rawNote);
    if (raw == nullptr) {
      return null;
    }
    final decoded = jsonDecode(raw.toDartString());
    if (decoded is! Map) {
      return null;
    }
    if (decoded['ok'] == true) {
      final path = decoded['path'];
      return path is String ? path : '';
    }
    hostLogWarn('计划留档失败：${decoded['error'] ?? '未知原因'}');
    return null;
  } catch (error) {
    hostLogWarn('计划留档调用失败：$error');
    return null;
  } finally {
    for (final pointer in [rawOriginal, rawEdited, rawNote]) {
      if (pointer != null) {
        malloc.free(pointer);
      }
    }
    if (raw != null && raw != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

// ── FFI ───────────────────────────────────────────────────────────

typedef _ArchiveC = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _ArchiveDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);
