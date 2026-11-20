// 平台薄层：会话轨迹（`docs/todo.md` §待办 3-28.12，宿主侧 `rust/src/session_export.rs`）。
//
// 一份会话拍平成 **NDJSON 事件行**（`type` = session / message / step / turn / notice），
// 与「查看过程」同一份数据，但保留来源与原始字段，可直接 `jq` / `grep`。
//
// 三个不可越界的口径：
//   · **不发任何请求** —— 只读已经在内存 / 已落盘的会话，导出 ≠ 重放；
//   · **脱敏在宿主** —— 每行都过 `mask_secrets()`，这里不自己做第二套；
//   · **截断在宿主** —— 单字段 4000 字符、单次 2000 行、预览 256 KiB，界面只展示结果。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _JsonC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _JsonDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 「轨迹」预览结果：`lines` 已是脱敏后的 NDJSON 文本行。
class SessionTrace {
  const SessionTrace({
    required this.ok,
    required this.id,
    required this.lines,
    required this.count,
    required this.bytes,
    required this.truncated,
    required this.error,
  });

  final bool ok;
  final String id;
  final List<String> lines;
  final int count;
  final int bytes;

  /// 有内容被截断（单行字段 / 总行数 / 预览字节），界面要如实标。
  final bool truncated;
  final String error;

  static const unavailable = SessionTrace(
    ok: false,
    id: '',
    lines: [],
    count: 0,
    bytes: 0,
    truncated: false,
    error: '本地组件未就绪，读不了轨迹',
  );

  static SessionTrace fromJson(dynamic json) {
    if (json is! Map) {
      return unavailable;
    }
    return SessionTrace(
      ok: json['ok'] == true,
      id: json['id'] is String ? json['id'] as String : '',
      lines: [
        for (final line in (json['lines'] as List? ?? const []))
          if (line is String) line,
      ],
      count: json['count'] is num ? (json['count'] as num).toInt() : 0,
      bytes: json['bytes'] is num ? (json['bytes'] as num).toInt() : 0,
      truncated: json['truncated'] == true,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 导出结果：成功时 `path` 是落盘的 `.jsonl` 绝对路径。
class SessionExport {
  const SessionExport({
    required this.ok,
    required this.path,
    required this.count,
    required this.bytes,
    required this.truncated,
    required this.error,
  });

  final bool ok;
  final String path;
  final int count;
  final int bytes;
  final bool truncated;
  final String error;

  static const unavailable = SessionExport(
    ok: false,
    path: '',
    count: 0,
    bytes: 0,
    truncated: false,
    error: '本地组件未就绪，导不出来',
  );

  static SessionExport fromJson(dynamic json) {
    if (json is! Map) {
      return unavailable;
    }
    return SessionExport(
      ok: json['ok'] == true,
      path: json['path'] is String ? json['path'] as String : '',
      count: json['count'] is num ? (json['count'] as num).toInt() : 0,
      bytes: json['bytes'] is num ? (json['bytes'] as num).toInt() : 0,
      truncated: json['truncated'] == true,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 只读预览一条会话的轨迹（不落盘）。
SessionTrace readSessionTrace(Map<String, Object?> session) {
  final raw = _callJson('lunac_session_trace', session);
  return raw == null ? SessionTrace.unavailable : SessionTrace.fromJson(raw);
}

/// 把一条会话导出成 `.jsonl`（`<exe 根>\ModuleData\exports\session-<id>-<UTC>.jsonl`）。
SessionExport exportSessionJsonl(Map<String, Object?> session) {
  final raw = _callJson('lunac_session_export', session);
  return raw == null ? SessionExport.unavailable : SessionExport.fromJson(raw);
}

/// 调一个「入参 JSON 对象 → 回执 JSON」的宿主函数；桥接缺失 / 解析失败回 null。
dynamic _callJson(String symbol, Map<String, Object?> request) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? pointer;
  Pointer<Utf8>? out;
  try {
    final call = library.lookupFunction<_JsonC, _JsonDart>(symbol);
    pointer = jsonEncode(request).toNativeUtf8();
    out = call(pointer);
    if (out == nullptr) {
      return null;
    }
    return jsonDecode(out.toDartString());
  } catch (_) {
    return null;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
    if (out != null && out != nullptr) {
      _freeQuietly(library, out);
    }
  }
}

void _freeQuietly(DynamicLibrary library, Pointer<Utf8> pointer) {
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}
