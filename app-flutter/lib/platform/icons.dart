// 平台薄层：结果行的真实图标（v1 `get_app_icon` 的对应实现）。
//
// 抽取在 Rust（`indexer/src/icon.rs`：SHGetFileInfoW + GDI → 32×32 PNG → base64），
// 这里只做两件事：调 C ABI、按路径缓存（含「取不到」的负缓存）。
//
// 为什么缓存：图标抽取要过 Windows Shell + GDI，内部还是串行闸；结果列表每次击键
// 都会重建，不缓存就等于每次输入都重抽一遍同一批应用。
import 'dart:collection';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _IconC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _IconDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 路径 → data URL；`null` 表示这个路径取不到图标（负缓存）。
///
/// 用 LRU 封顶：图标是 32×32 的 base64 串（每条 1–4KB），不封顶的话长期使用
/// 会把见过的所有应用 / 文件路径都留在内存里（后台占用会一直涨）。
const int _maxCachedIcons = 512;
final LinkedHashMap<String, String?> _cache = LinkedHashMap<String, String?>();

/// 取文件 / 应用的 32×32 图标（`data:image/png;base64,...`）；取不到返回 null。
String? appIcon(String path) {
  if (_cache.containsKey(path)) {
    // 命中也要刷新 LRU 顺序（移到队尾）。
    final hit = _cache.remove(path);
    _cache[path] = hit;
    return hit;
  }
  String? result;
  final library = openLunacNative().library;
  if (library != null) {
    Pointer<Utf8>? pointer;
    try {
      final icon = library.lookupFunction<_IconC, _IconDart>('lunac_app_icon');
      final free = library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string');
      pointer = path.toNativeUtf8();
      final raw = icon(pointer);
      if (raw != nullptr) {
        try {
          final text = raw.toDartString();
          result = text.isEmpty ? null : text;
        } finally {
          free(raw);
        }
      }
    } catch (_) {
      // 旧版库没有该符号 / 抽取失败：按「没有图标」处理。
      result = null;
    } finally {
      if (pointer != null) {
        malloc.free(pointer);
      }
    }
  }
  _cache[path] = result;
  if (_cache.length > _maxCachedIcons) {
    _cache.remove(_cache.keys.first); // 淘汰最久未使用的一条
  }
  return result;
}
