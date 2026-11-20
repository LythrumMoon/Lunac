// 平台薄层：插件存储（v1 `plugin_kv` 的 v2 等价物，docs/todo.md §M5z-补69）。
//
// 边界：路径定位 / id 与 key 白名单 / 配额 / 原子写全在 `rust/src/plugin_kv.rs`；
// 本文件只负责数据形状与五个 C ABI。存储 = `<exe 根>/ModuleData/plugins/<id>/kv.json`。
//
// 配额（与 Rust 侧同名常量一致，超了由 Rust 拒绝，本层不复制判定）：
// 单插件 256 KiB / 单值 64 KiB / 键数 256 / key ≤ 128 字符。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 读一个键的结果。
class PluginKvValue {
  const PluginKvValue({
    required this.ok,
    this.key = '',
    this.found = false,
    this.value,
    this.error = '',
  });

  final bool ok;
  final String key;

  /// 键存在与否 —— 与 [value] 是否 `null` 分开（值本身就可以是 JSON `null`）。
  final bool found;
  final dynamic value;
  final String error;

  static const unavailable = PluginKvValue(ok: false, error: '当前环境没有原生桥');
}

/// 写 / 删 / 清空的结果。
class PluginKvWrite {
  const PluginKvWrite({
    required this.ok,
    this.key = '',
    this.bytes = 0,
    this.removed = false,
    this.error = '',
  });

  final bool ok;
  final String key;

  /// 操作完成后这个插件**整份存储**的大小（画配额用量用）。
  final int bytes;

  /// `delete` 专用：这个键原本是否存在。
  final bool removed;
  final String error;

  static const unavailable = PluginKvWrite(ok: false, error: '当前环境没有原生桥');
}

/// 一个键的摘要（列表用）。
class PluginKvEntry {
  const PluginKvEntry({required this.key, required this.bytes});

  final String key;
  final int bytes;
}

/// 一个插件的存储快照。
class PluginKvSnapshot {
  const PluginKvSnapshot({
    required this.ok,
    this.entries = const [],
    this.bytes = 0,
    this.error = '',
  });

  final bool ok;
  final List<PluginKvEntry> entries;

  /// 整份 `kv.json` 的大小（含版本包装；文件不存在 = 0）。
  final int bytes;
  final String error;

  static const unavailable = PluginKvSnapshot(
    ok: false,
    error: '当前环境没有原生桥',
  );
}

/// 读一个键；桥接缺失时返回 [PluginKvValue.unavailable]。
PluginKvValue readPluginKv(String id, String key) {
  final text = _call2('lunac_plugin_kv_get', id, key);
  if (text == null) {
    return PluginKvValue.unavailable;
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map) {
      return const PluginKvValue(ok: false, error: '插件存储的返回格式不对');
    }
    return PluginKvValue(
      ok: value['ok'] == true,
      key: value['key'] is String ? value['key'] as String : key,
      found: value['found'] == true,
      value: value['value'],
      error: value['error'] is String ? value['error'] as String : '',
    );
  } catch (_) {
    return const PluginKvValue(ok: false, error: '插件存储读取失败');
  }
}

/// 写一个键（新建 / 覆盖）。`value` 可以是任意可 JSON 化的值。
PluginKvWrite writePluginKv({
  required String id,
  required String key,
  required dynamic value,
}) => _write(
  _call1(
    'lunac_plugin_kv_set',
    jsonEncode({'id': id, 'key': key, 'value': value}),
  ),
);

/// 删一个键（不存在也算成功，`removed=false`）。
PluginKvWrite deletePluginKv(String id, String key) =>
    _write(_call2('lunac_plugin_kv_delete', id, key));

/// 列出这个插件的全部键（按 key 字典序）。
PluginKvSnapshot listPluginKv(String id) {
  final text = _call1('lunac_plugin_kv_list', id);
  if (text == null) {
    return PluginKvSnapshot.unavailable;
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map) {
      return const PluginKvSnapshot(ok: false, error: '插件存储列表格式不对');
    }
    final entries = <PluginKvEntry>[];
    final raw = value['entries'];
    if (raw is List) {
      for (final item in raw) {
        if (item is Map && item['key'] is String) {
          entries.add(
            PluginKvEntry(
              key: item['key'] as String,
              bytes: item['bytes'] is num ? (item['bytes'] as num).toInt() : 0,
            ),
          );
        }
      }
    }
    return PluginKvSnapshot(
      ok: value['ok'] == true,
      entries: entries,
      bytes: value['bytes'] is num ? (value['bytes'] as num).toInt() : 0,
      error: value['error'] is String ? value['error'] as String : '',
    );
  } catch (_) {
    return const PluginKvSnapshot(ok: false, error: '插件存储列表解析失败');
  }
}

/// 清空一个插件的存储（删掉整份 `kv.json`）。
PluginKvWrite clearPluginKv(String id) =>
    _write(_call1('lunac_plugin_kv_clear', id));

PluginKvWrite _write(String? text) {
  if (text == null) {
    return PluginKvWrite.unavailable;
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map) {
      return const PluginKvWrite(ok: false, error: '插件存储的返回格式不对');
    }
    return PluginKvWrite(
      ok: value['ok'] == true,
      key: value['key'] is String ? value['key'] as String : '',
      bytes: value['bytes'] is num ? (value['bytes'] as num).toInt() : 0,
      removed: value['removed'] == true,
      error: value['error'] is String ? value['error'] as String : '',
    );
  } catch (_) {
    return const PluginKvWrite(ok: false, error: '插件存储写入失败');
  }
}

typedef _Text1C = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _Text1Dart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _Text2C = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _Text2Dart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

String? _call1(String symbol, String argument) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_Text1C, _Text1Dart>(symbol);
    arg = argument.toNativeUtf8();
    raw = call(arg);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (arg != null) {
      malloc.free(arg);
    }
    _free(library, raw);
  }
}

String? _call2(String symbol, String first, String second) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg1;
  Pointer<Utf8>? arg2;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_Text2C, _Text2Dart>(symbol);
    arg1 = first.toNativeUtf8();
    arg2 = second.toNativeUtf8();
    raw = call(arg1, arg2);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (arg1 != null) {
      malloc.free(arg1);
    }
    if (arg2 != null) {
      malloc.free(arg2);
    }
    _free(library, raw);
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? raw) {
  if (raw == null || raw == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
  } catch (_) {
    // 旧桥没有这个符号：忽略。
  }
}
