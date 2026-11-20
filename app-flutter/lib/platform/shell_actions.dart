// 平台薄层：结果行的资源管理器动作（右键「打开所在位置」）。
//
// Rust 侧 `lunac_reveal_path`：Windows 用 `explorer /select,<path>`；
// 其它平台的等价实现（打开父目录）也在 Rust 里，Dart 不出现 `Platform.isXxx`。
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _RevealC = Int32 Function(Pointer<Utf8>);
typedef _RevealDart = int Function(Pointer<Utf8>);
typedef _RunC = Int32 Function(Pointer<Utf8>);
typedef _RunDart = int Function(Pointer<Utf8>);

/// 用系统默认浏览器打开一个 URL（设置面板「检查更新 → 前往下载」）。
///
/// Rust 侧只放行 http(s)；返回是否已发起。
bool openExternalUrl(String url) {
  final target = url.trim();
  if (target.isEmpty) {
    return false;
  }
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final open = library.lookupFunction<_RevealC, _RevealDart>(
      'lunac_open_url',
    );
    pointer = target.toNativeUtf8();
    return open(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 用系统默认程序打开路径（对话里的「文件呈递」卡片用）。
///
/// 复用搜索结果的动作通道（`lunac_run_action("file:<path>")`），与主页面打开文件同一路径。
bool openPath(String path) {
  if (path.isEmpty) {
    return false;
  }
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final run = library.lookupFunction<_RunC, _RunDart>('lunac_run_action');
    pointer = 'file:$path'.toNativeUtf8();
    return run(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 在文件管理器里定位路径；返回是否已发起。
bool revealInExplorer(String path) {
  if (path.isEmpty) {
    return false;
  }
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final reveal = library.lookupFunction<_RevealC, _RevealDart>('lunac_reveal_path');
    pointer = path.toNativeUtf8();
    return reveal(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}
