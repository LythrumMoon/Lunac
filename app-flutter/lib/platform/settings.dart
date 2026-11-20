// 平台薄层：主页面设置（默认搜索引擎 + 设置面板要用的数据目录）。
//
// 持久化在 Rust（`rust/src/settings.rs` → `<exe 根>/config/search.json`），
// 本文件只提供「读当前引擎 / 切换引擎」两个调用，以及菜单用的展示名表。
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _EngineC = Pointer<Utf8> Function();
typedef _EngineDart = Pointer<Utf8> Function();
typedef _SetEngineC = Int32 Function(Pointer<Utf8>);
typedef _SetEngineDart = int Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);
typedef _DataDirC = Pointer<Utf8> Function();
typedef _DataDirDart = Pointer<Utf8> Function();

/// 支持的引擎（键与 Rust `settings::ENGINES` 一致）。
const Map<String, String> webEngines = {
  'google': 'Google',
  'bing': 'Bing',
  'baidu': '百度',
};

/// 当前默认引擎的键；桥接不可用时按 `google` 处理。
String webEngineName() {
  final library = openLunacNative().library;
  if (library == null) {
    return 'google';
  }
  Pointer<Utf8>? raw;
  try {
    final read = library.lookupFunction<_EngineC, _EngineDart>('lunac_web_engine');
    raw = read();
    if (raw == nullptr) {
      return 'google';
    }
    final name = raw.toDartString();
    return webEngines.containsKey(name) ? name : 'google';
  } catch (_) {
    return 'google';
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

/// 切换默认引擎；返回是否成功（名字非法 / 桥接缺失为 false）。
bool setWebEngine(String name) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final set = library
        .lookupFunction<_SetEngineC, _SetEngineDart>('lunac_set_web_engine');
    pointer = name.toNativeUtf8();
    return set(pointer) == 1;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 数据根目录（= 可执行文件所在目录）；桥接缺失时返回 null。
///
/// 设置面板「关于」显示它，用户排查配置 / 索引缓存位置时用。
String? dataDirectory() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_DataDirC, _DataDirDart>('lunac_data_dir')();
    if (raw == nullptr) {
      return null;
    }
    final path = raw.toDartString();
    return path.isEmpty ? null : path;
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
