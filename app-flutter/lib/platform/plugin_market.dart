// 平台薄层：插件市场（拉索引 / 安装 / 卸载 / 已装清单）。
//
// 边界：网络、sha256、zip 解压与路径校验全在 `rust/src/plugin_market.rs`；
// 本文件只负责数据形状与四个 C ABI。装法与 v1 完全一致：`<exe 根>/Modules/<id>/`。
//
// 线程：`fetchMarketIndexRaw` / `installMarketPluginRaw` 是**阻塞**的（走网络 + 解压），
// 调用方必须放进 `Isolate.run`；已装清单是本机扫目录（毫秒级），可以同步读。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 市场索引里的一条（只描述「去哪下」）。
class MarketIndexEntry {
  const MarketIndexEntry({
    required this.id,
    required this.name,
    required this.description,
    required this.version,
    required this.url,
    required this.sha256,
    required this.icon,
    required this.homepage,
    required this.keywords,
  });

  final String id;
  final String name;
  final String description;
  final String version;
  final String url;
  final String sha256;
  final String icon;
  final String homepage;
  final List<String> keywords;

  static MarketIndexEntry? fromJson(Map<String, dynamic> json) {
    final id = json['id'];
    final url = json['url'];
    if (id is! String || id.isEmpty || url is! String || url.isEmpty) {
      return null;
    }
    return MarketIndexEntry(
      id: id,
      name: json['name'] is String ? json['name'] as String : id,
      description:
          json['description'] is String ? json['description'] as String : '',
      version: json['version'] is String ? json['version'] as String : '',
      url: url,
      sha256: json['sha256'] is String ? json['sha256'] as String : '',
      icon: json['icon'] is String ? json['icon'] as String : '',
      homepage: json['homepage'] is String ? json['homepage'] as String : '',
      keywords: [
        for (final k in (json['keywords'] as List? ?? const []))
          if (k is String) k,
      ],
    );
  }
}

/// 已装插件（含坏包：`valid == false` 时 `error` 非空）。
class InstalledPlugin {
  const InstalledPlugin({
    required this.id,
    required this.name,
    required this.description,
    required this.version,
    required this.icon,
    required this.homepage,
    required this.dir,
    required this.entryPath,
    required this.valid,
    required this.error,
    required this.deps,
    required this.permissions,
    this.hasSidecar = false,
  });

  final String id;
  final String name;
  final String description;
  final String version;
  final String icon;
  final String homepage;
  final String dir;
  final String entryPath;
  final bool valid;
  final String error;
  final int deps;
  final List<String> permissions;

  /// 清单声明了 `sidecar`（插件自带本机进程，可用任何语言写）。
  final bool hasSidecar;

  static InstalledPlugin? fromJson(Map<String, dynamic> json) {
    final id = json['id'];
    if (id is! String || id.isEmpty) {
      return null;
    }
    return InstalledPlugin(
      id: id,
      name: json['name'] is String ? json['name'] as String : id,
      description:
          json['description'] is String ? json['description'] as String : '',
      version: json['version'] is String ? json['version'] as String : '',
      icon: json['icon'] is String ? json['icon'] as String : '',
      homepage: json['homepage'] is String ? json['homepage'] as String : '',
      dir: json['dir'] is String ? json['dir'] as String : '',
      entryPath:
          json['entryPath'] is String ? json['entryPath'] as String : '',
      valid: json['valid'] == true,
      error: json['error'] is String ? json['error'] as String : '',
      deps: json['deps'] is int ? json['deps'] as int : 0,
      permissions: [
        for (final p in (json['permissions'] as List? ?? const []))
          if (p is String) p,
      ],
      hasSidecar: json['hasSidecar'] == true,
    );
  }
}

/// 已装插件快照（`dir` = 插件根绝对路径；桥接缺失时为 null）。
class MarketInstalled {
  const MarketInstalled({
    required this.entries,
    required this.dir,
    required this.error,
  });

  final List<InstalledPlugin> entries;
  final String dir;
  final String error;
}

/// 远端索引快照。
class MarketIndex {
  const MarketIndex({required this.entries, required this.error});

  final List<MarketIndexEntry> entries;
  final String error;

  /// 解析 FFI 返回的 JSON；`raw == null` = 桥接缺失。
  factory MarketIndex.fromRaw(String? raw) {
    if (raw == null) {
      return const MarketIndex(
        entries: [],
        error: '不可用（rust 桥接未加载）',
      );
    }
    return _decode(raw, (json) {
      return MarketIndex(
        entries: [
          for (final item in (json['entries'] as List? ?? const []))
            if (item is Map<String, dynamic>) ?MarketIndexEntry.fromJson(item),
        ],
        error: json['error'] is String ? json['error'] as String : '',
      );
    }, (reason) => MarketIndex(entries: const [], error: reason));
  }
}

/// 安装 / 卸载的结果。
class MarketActionResult {
  const MarketActionResult({required this.ok, required this.id, this.error = ''});

  final bool ok;
  final String id;
  final String error;

  factory MarketActionResult.fromRaw(String? raw, String fallbackId) {
    if (raw == null) {
      return const MarketActionResult(
        ok: false,
        id: '',
        error: '不可用（rust 桥接未加载）',
      );
    }
    return _decode(
      raw,
      (json) => MarketActionResult(
        ok: json['ok'] == true,
        id: json['id'] is String ? json['id'] as String : fallbackId,
        error: json['error'] is String ? json['error'] as String : '',
      ),
      (reason) => MarketActionResult(ok: false, id: fallbackId, error: reason),
    );
  }
}

/// 当前这次安装的进度快照（`lunac_market_install_state`）。
///
/// 字段与 v1 `InstallState` 同名同义：`kind` 区分「包本体 / 补依赖」，`url` 用来把进度
/// 挂回市场里对应的那一行，`paused` / `resumable` 决定按钮写「暂停」还是「继续下载」。
class MarketInstallState {
  const MarketInstallState({
    this.busy = false,
    this.active = false,
    this.kind = '',
    this.id = '',
    this.url = '',
    this.phase = '',
    this.downloaded = 0,
    this.total = 0,
    this.paused = false,
    this.resumable = false,
    this.index = 0,
    this.count = 0,
  });

  final bool busy;
  final bool active;
  final String kind;
  final String id;
  final String url;
  final String phase;
  final int downloaded;
  final int total;
  final bool paused;
  final bool resumable;
  final int index;
  final int count;

  /// 0..1；总长未知（服务端没给 Content-Length）时返回 null。
  double? get fraction => total > 0 ? (downloaded / total).clamp(0.0, 1.0) : null;

  factory MarketInstallState.fromRaw(String? raw) {
    if (raw == null) {
      return const MarketInstallState();
    }
    return _decode(raw, (json) {
      final state = json['state'];
      final map = state is Map<String, dynamic> ? state : const <String, dynamic>{};
      int intOf(String key) => map[key] is int ? map[key] as int : 0;
      return MarketInstallState(
        busy: json['busy'] == true,
        active: map['active'] == true,
        kind: map['kind'] is String ? map['kind'] as String : '',
        id: map['id'] is String ? map['id'] as String : '',
        url: map['url'] is String ? map['url'] as String : '',
        phase: map['phase'] is String ? map['phase'] as String : '',
        downloaded: intOf('downloaded'),
        total: intOf('total'),
        paused: map['paused'] == true,
        resumable: map['resumable'] == true,
        index: intOf('index'),
        count: intOf('count'),
      );
    }, (reason) => const MarketInstallState());
  }
}

/// 磁盘上还没下完的断点（`temp/downloads/*.part`）。
class MarketPendingDownload {
  const MarketPendingDownload({
    required this.url,
    required this.id,
    required this.downloaded,
    required this.total,
  });

  final String url;
  final String id;
  final int downloaded;
  final int total;

  double? get fraction => total > 0 ? (downloaded / total).clamp(0.0, 1.0) : null;

  static MarketPendingDownload? fromJson(Map<String, dynamic> json) {
    final url = json['url'];
    if (url is! String || url.isEmpty) {
      return null;
    }
    int intOf(String key) => json[key] is int ? json[key] as int : 0;
    return MarketPendingDownload(
      url: url,
      id: json['id'] is String ? json['id'] as String : '',
      downloaded: intOf('downloaded'),
      total: intOf('total'),
    );
  }
}

T _decode<T>(String raw, T Function(Map<String, dynamic>) build, T Function(String) fail) {
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return fail('返回格式不对');
    }
    return build(value);
  } catch (e) {
    return fail('返回不是合法 JSON：$e');
  }
}

typedef _String0C = Pointer<Utf8> Function();
typedef _String0Dart = Pointer<Utf8> Function();
typedef _String2C = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _String2Dart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _TextStringC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextStringDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);
typedef _Int1C = Pointer<Utf8> Function(Int32);
typedef _Int1Dart = Pointer<Utf8> Function(int);

/// 已装插件（本机扫描；桥接缺失返回 null）。
MarketInstalled? readInstalledPlugins() {
  final raw = _callString0('lunac_market_installed');
  if (raw == null) {
    return null;
  }
  return _decode(
    raw,
    (json) => MarketInstalled(
      entries: [
        for (final item in (json['entries'] as List? ?? const []))
          if (item is Map<String, dynamic>) ?InstalledPlugin.fromJson(item),
      ],
      dir: json['dir'] is String ? json['dir'] as String : '',
      error: json['error'] is String ? json['error'] as String : '',
    ),
    (reason) => MarketInstalled(entries: const [], dir: '', error: reason),
  );
}

/// 远端索引的原始 JSON（**阻塞网络**，调用方放进后台 isolate）。
String? fetchMarketIndexRaw() => _callString0('lunac_market_index');

/// 安装的原始 JSON（**阻塞网络 + 解压**，调用方放进后台 isolate）。
String? installMarketPluginRaw(String url, String sha256) =>
    _callString2('lunac_market_install', url, sha256);

/// 卸载的原始 JSON（本机删目录，毫秒级；仍放在后台 isolate 里跑以免大目录卡界面）。
String? uninstallMarketPluginRaw(String id) =>
    _callString1('lunac_market_uninstall', id);

/// 当前安装进度（不阻塞，可高频轮询；桥接缺失 = 全部默认值）。
MarketInstallState readMarketInstallState() =>
    MarketInstallState.fromRaw(_callString0('lunac_market_install_state'));

/// 磁盘上还没下完的断点（不阻塞）。null = 桥接缺失。
List<MarketPendingDownload>? readMarketPendingDownloads() {
  final raw = _callString0('lunac_market_pending_downloads');
  if (raw == null) {
    return null;
  }
  return _decode(
    raw,
    (json) => [
      for (final item in (json['entries'] as List? ?? const []))
        if (item is Map<String, dynamic>) ?MarketPendingDownload.fromJson(item),
    ],
    (reason) => const <MarketPendingDownload>[],
  );
}

/// 暂停 / 继续当前这次下载（只置标志，不阻塞）。
bool pauseMarketInstall(bool paused) =>
    _callInt1('lunac_market_install_pause', paused ? 1 : 0) != null;

/// 取消当前这次依赖下载（删断点，下一次安装从头来）。
bool cancelMarketDependency() =>
    _callString0('lunac_market_cancel_dependency') != null;

/// 给已装插件补装清单里的依赖（**阻塞网络**，调用方放进后台 isolate）。
String? installMarketDependenciesRaw(String id) =>
    _callString1('lunac_market_install_deps', id);

String? _callString0(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_String0C, _String0Dart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

String? _callInt1(String symbol, int value) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_Int1C, _Int1Dart>(symbol)(value);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

String? _callString1(String symbol, String a) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextStringC, _TextStringDart>(symbol);
    arg = a.toNativeUtf8();
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

String? _callString2(String symbol, String a, String b) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? argA;
  Pointer<Utf8>? argB;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_String2C, _String2Dart>(symbol);
    argA = a.toNativeUtf8();
    argB = b.toNativeUtf8();
    raw = call(argA, argB);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (argA != null) {
      malloc.free(argA);
    }
    if (argB != null) {
      malloc.free(argB);
    }
    _free(library, raw);
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? pointer) {
  if (pointer == null || pointer == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}

// ── 安装进度的显示口径（纯函数，单测直接打）──────────────────────────

/// `0.3712` → `37%`；总数未知（null）时给空串 —— 不编造百分比。
String percentText(double? fraction) =>
    fraction == null ? '' : '${(fraction * 100).floor()}%';

/// 字节数的人话（1024 进制）。进度条上不需要精确到字节，最多两位小数。
String formatByteSize(int bytes) {
  if (bytes < 1024) {
    return '$bytes B';
  }
  const units = ['KB', 'MB', 'GB', 'TB'];
  var value = bytes / 1024;
  var unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return value >= 100
      ? '${value.round()} ${units[unit]}'
      : '${value.toStringAsFixed(1)} ${units[unit]}';
}

/// URL 最后一段（包本体阶段还没有插件 id，用它当人话标识）。
String urlTail(String url) {
  final cut = url.split(RegExp(r'[?#]')).first;
  final parts = cut.split('/');
  return parts.isEmpty || parts.last.isEmpty ? url : parts.last;
}
