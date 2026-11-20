// 平台薄层：快捷启动（v1 `quick-launch` 插件的原生等价物）。
//
// 边界：注册表的读写 / 去重 / 落盘 / 并入应用搜索缓存都在
// `lunac_indexer::app_indexer`（数据文件 `<exe 根>/ModuleData/custom/app_registry.json`）；
// 本文件只把三个 C ABI 包成 Dart 调用。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条自定义启动项（与 Rust / v1 的 JSON 字段一一对应）。
class CustomApp {
  const CustomApp({
    required this.name,
    required this.path,
    required this.source,
  });

  /// 显示名（注册时名称为空则由 Rust 用文件名推导）。
  final String name;

  /// 文件 / 程序的绝对路径。
  final String path;

  /// `custom`（注册表里的来源，v1 同值）。
  final String source;

  static CustomApp? fromJson(Map<String, dynamic> json) {
    final name = json['name'];
    final path = json['path'];
    if (name is! String || name.isEmpty || path is! String || path.isEmpty) {
      return null;
    }
    final source = json['source'];
    return CustomApp(
      name: name,
      path: path,
      source: source is String && source.isNotEmpty ? source : 'custom',
    );
  }
}

typedef _ListC = Pointer<Utf8> Function();
typedef _ListDart = Pointer<Utf8> Function();
typedef _TextI32C = Int32 Function(Pointer<Utf8>);
typedef _TextI32Dart = int Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读全部自定义启动项；桥接缺失返回 null（面板显示「不可用」而不是假装空）。
List<CustomApp>? readCustomApps() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_ListC, _ListDart>('lunac_custom_apps')();
    if (raw == nullptr) {
      return null;
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return null;
    }
    return [
      for (final item in value)
        if (item is Map<String, dynamic>) ?CustomApp.fromJson(item),
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

/// 注册一条启动项（名称为空 = 由 Rust 用文件名推导）。返回是否注册成功。
bool addCustomApp(String name, String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final add = library.lookupFunction<_TextI32C, _TextI32Dart>(
      'lunac_add_custom_app',
    );
    pointer = jsonEncode({'name': name, 'path': path}).toNativeUtf8();
    return add(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

typedef _PickC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _PickDart = Pointer<Utf8> Function(Pointer<Utf8>);

/// 「添加启动项」：系统多选框（所有文件）。取消 / 桥接缺失返回空列表。
///
/// 复用文件转换的那条多选 ABI（`lunac_convert_pick_files`，同一个
/// GetOpenFileNameW 多选实现），`kind = "all"` 表示「所有文件」优先。
List<String> pickLaunchFiles() {
  final library = openLunacNative().library;
  if (library == null) {
    return const [];
  }
  Pointer<Utf8>? kindPtr;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_PickC, _PickDart>(
      'lunac_convert_pick_files',
    );
    kindPtr = 'all'.toNativeUtf8();
    raw = call(kindPtr);
    if (raw == nullptr) {
      return const [];
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return const [];
    }
    return [
      for (final item in value)
        if (item is String && item.isNotEmpty) item,
    ];
  } catch (_) {
    return const [];
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
    if (kindPtr != null) {
      malloc.free(kindPtr);
    }
  }
}

/// 删除一条启动项（按路径匹配，大小写不敏感）。返回是否删除成功。
bool removeCustomApp(String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final remove = library.lookupFunction<_TextI32C, _TextI32Dart>(
      'lunac_remove_custom_app',
    );
    pointer = path.toNativeUtf8();
    return remove(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}
