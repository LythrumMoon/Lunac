// 平台薄层：文件索引状态与重建（v1 `file_index_status` / `refresh_file_index`）。
//
// 边界：索引的扫描、落盘、上限与截断判定都在 `indexer` crate；本文件只把
// `lunac_index_status` / `lunac_index_refresh` 两个 C ABI 包成 Dart 调用。
// 桥接缺失时返回 null（界面显示「索引不可用」而不是崩掉）。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 文件索引状态（与 `indexer::file_indexer::IndexStatus` 的 JSON 字段一一对应）。
class FileIndexStatus {
  const FileIndexStatus({
    required this.count,
    required this.scanning,
    required this.savedMs,
    required this.truncated,
    required this.roots,
  });

  /// 已索引条数。
  final int count;

  /// 是否正在扫盘。
  final bool scanning;

  /// 索引落盘时间（毫秒；0 = 还没有索引文件）。
  final int savedMs;

  /// 是否因上限截断。
  final bool truncated;

  /// 实际扫描的根目录。
  final List<String> roots;

  /// 还没载入：count 为 0 且从未落盘（正在建 / 首次安装）。
  ///
  /// 与「索引确实是空的」分开：前者在界面上只给转圈（v1 同判据）。
  bool get notLoaded => count == 0 && savedMs == 0;

  static FileIndexStatus? fromJson(Map<String, dynamic> json) {
    final count = json['count'];
    final scanning = json['scanning'];
    if (count is! int || scanning is! bool) {
      return null;
    }
    return FileIndexStatus(
      count: count,
      scanning: scanning,
      savedMs: json['saved_ms'] is int ? json['saved_ms'] as int : 0,
      truncated: json['truncated'] == true,
      roots: [
        for (final root in (json['roots'] as List? ?? const []))
          if (root is String) root,
      ],
    );
  }
}

typedef _StatusC = Pointer<Utf8> Function();
typedef _StatusDart = Pointer<Utf8> Function();
typedef _RefreshC = Void Function();
typedef _RefreshDart = void Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读一次索引状态；桥接缺失 / 解析失败返回 null。
FileIndexStatus? readFileIndexStatus() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_StatusC, _StatusDart>('lunac_index_status')();
    if (raw == nullptr) {
      return null;
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic>) {
      return null;
    }
    return FileIndexStatus.fromJson(value);
  } catch (_) {
    return null;
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

/// 强制重建文件索引（Rust 侧起后台线程扫盘，立即返回）。
///
/// 返回 false = 桥接缺失 / 符号缺失（界面据此提示，而不是假装在重建）。
bool rebuildFileIndex() {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    library.lookupFunction<_RefreshC, _RefreshDart>('lunac_index_refresh')();
    return true;
  } catch (_) {
    return false;
  }
}
