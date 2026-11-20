// 搜索的数据源与动作执行（M1）。
//
// 边界（docs/flutter-migration.md §一）：搜索目录、过滤与本地动作都在 Rust 侧；
// 本文件只把 `lunac_search` / `lunac_run_action` 的 C ABI 包成 Dart 接口，
// 供 UI 调用。UI 只依赖 [SearchBackend]，因此单测可以注入假数据源。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import '../platform/native_bridge.dart';
import 'search_result.dart';

/// 动作执行结果（与 `rust/src/lib.rs` 的返回码一一对应）。
enum SearchActionOutcome {
  /// 0：Rust 侧已执行（打开目录等）。
  executed,

  /// 1：未知动作 id。
  unknown,

  /// 2：由 UI 处理（AI 对话 / 设置 / 退出）。
  uiHandled,

  /// 3：执行失败。
  failed,
}

/// 详细搜索「设置 / 系统动作」分类的一条目录条目。
///
/// `id` 已经带好动作前缀（`setting:` / `action:`），直接喂给 `runAction`；
/// 提权形态另走 `admin:<动作 id>`。
class SystemCatalogEntry {
  const SystemCatalogEntry({
    required this.id,
    required this.title,
    required this.subtitle,
    required this.danger,
    required this.elevatable,
  });

  final String id;
  final String title;
  final String subtitle;

  /// 危险动作（关机 / 重启）：界面必须二次确认。
  final bool danger;

  /// 是否有「以管理员身份运行」形态。
  final bool elevatable;
}

/// 搜索数据源接口：UI 只认这个，不直接碰 FFI。
abstract class SearchBackend {
  /// 空查询 = 返回全部；实现方负责排序与截断。
  List<SearchResult> search(String query);

  /// 详细搜索：应用查询（条数更多；默认实现返回空，单测按需覆盖）。
  List<SearchResult> searchApps(String query, {int limit = 60}) => const [];

  /// 详细搜索：文件查询（可按类型过滤；`kind` 为 null = 全部类型）。
  List<SearchResult> searchFiles(
    String query, {
    String? kind,
    int limit = 60,
  }) =>
      const [];

  /// 详细搜索：系统设置页 / 系统动作目录（空查询 = 全部）。
  List<SystemCatalogEntry> systemCatalog(String query) => const [];

  /// 执行一条结果的动作。
  SearchActionOutcome runAction(String id);

  /// 数据源不可用时的人话原因；可用时为 null。
  String? get unavailableReason;

  /// 释放（后端未来若持有资源，UI 在销毁时调用）。
  void dispose() {}
}

typedef _SearchC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SearchDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SearchAppsC = Pointer<Utf8> Function(Pointer<Utf8>, Uint32);
typedef _SearchAppsDart = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _SearchFilesC = Pointer<Utf8> Function(
    Pointer<Utf8>, Pointer<Utf8>, Uint32);
typedef _SearchFilesDart = Pointer<Utf8> Function(
    Pointer<Utf8>, Pointer<Utf8>, int);
typedef _SystemCatalogC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SystemCatalogDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _RunActionC = Int32 Function(Pointer<Utf8>);
typedef _RunActionDart = int Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);
typedef _IndexInitC = Void Function();
typedef _IndexInitDart = void Function();

/// 预热应用 / 文件索引（Rust 侧起后台扫描，立即返回）。
///
/// 旧版 DLL 没有该符号时静默跳过：搜索时 Rust 侧仍会自行初始化。
void warmUpSearchIndex() {
  final opened = openLunacNative();
  final library = opened.library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<_IndexInitC, _IndexInitDart>('lunac_index_init')();
  } catch (_) {
    // 旧版库：忽略。
  }
}

/// 走 Rust 薄层的真实实现；库缺失或 ABI 不匹配时退化为「空结果 + 原因」，
/// 不抛异常把窗口打挂。
class NativeSearchBackend implements SearchBackend {
  NativeSearchBackend() {
    final opened = openLunacNative();
    final library = opened.library;
    if (library == null) {
      _unavailableReason = opened.reason;
      return;
    }
    try {
      _search = library.lookupFunction<_SearchC, _SearchDart>('lunac_search');
      _runAction =
          library.lookupFunction<_RunActionC, _RunActionDart>('lunac_run_action');
      _freeString =
          library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string');
    } catch (e) {
      _unavailableReason = 'rust bridge symbol error ($e)';
    }
    try {
      // 详细搜索用（新符号）；旧 DLL 没有时详细搜索退回紧凑搜索的结果。
      _searchApps =
          library.lookupFunction<_SearchAppsC, _SearchAppsDart>(
              'lunac_search_apps');
      _searchFiles =
          library.lookupFunction<_SearchFilesC, _SearchFilesDart>(
              'lunac_search_files');
      _systemCatalog =
          library.lookupFunction<_SystemCatalogC, _SystemCatalogDart>(
              'lunac_system_catalog');
    } catch (_) {
      _searchApps = null;
      _searchFiles = null;
      _systemCatalog = null;
    }
  }

  _SearchDart? _search;
  _SearchAppsDart? _searchApps;
  _SearchFilesDart? _searchFiles;
  _SystemCatalogDart? _systemCatalog;
  _RunActionDart? _runAction;
  _FreeDart? _freeString;
  String? _unavailableReason;

  @override
  String? get unavailableReason => _unavailableReason;

  @override
  List<SearchResult> search(String query) {
    final call = _search;
    final free = _freeString;
    if (call == null || free == null) {
      return const [];
    }
    final queryPtr = query.toNativeUtf8();
    Pointer<Utf8> raw = nullptr;
    try {
      raw = call(queryPtr);
      if (raw == nullptr) {
        return const [];
      }
      return _parseResults(raw.toDartString());
    } catch (_) {
      // 坏 JSON / 坏指针都按「这次没结果」处理，下一次查询还能继续用。
      return const [];
    } finally {
      if (raw != nullptr) {
        free(raw);
      }
      malloc.free(queryPtr);
    }
  }

  @override
  List<SearchResult> searchApps(String query, {int limit = 60}) {
    final call = _searchApps;
    final free = _freeString;
    if (call == null || free == null) {
      // 旧库：退回紧凑搜索里筛应用。
      return search(query).where((r) => r.kind == 'app').toList();
    }
    final queryPtr = query.toNativeUtf8();
    Pointer<Utf8> raw = nullptr;
    try {
      raw = call(queryPtr, limit);
      if (raw == nullptr) {
        return const [];
      }
      return _parseResults(raw.toDartString());
    } catch (_) {
      return const [];
    } finally {
      if (raw != nullptr) {
        free(raw);
      }
      malloc.free(queryPtr);
    }
  }

  @override
  List<SearchResult> searchFiles(
    String query, {
    String? kind,
    int limit = 60,
  }) {
    final call = _searchFiles;
    final free = _freeString;
    if (call == null || free == null) {
      // 旧库：退回紧凑搜索里筛文件。
      return search(query).where((r) => r.kind == 'file').toList();
    }
    final queryPtr = query.toNativeUtf8();
    final kindPtr = (kind ?? '').toNativeUtf8();
    Pointer<Utf8> raw = nullptr;
    try {
      raw = call(queryPtr, kindPtr, limit);
      if (raw == nullptr) {
        return const [];
      }
      return _parseResults(raw.toDartString());
    } catch (_) {
      return const [];
    } finally {
      if (raw != nullptr) {
        free(raw);
      }
      malloc.free(queryPtr);
      malloc.free(kindPtr);
    }
  }

  @override
  List<SystemCatalogEntry> systemCatalog(String query) {
    final call = _systemCatalog;
    final free = _freeString;
    if (call == null || free == null) {
      return const [];
    }
    final queryPtr = query.toNativeUtf8();
    Pointer<Utf8> raw = nullptr;
    try {
      raw = call(queryPtr);
      if (raw == nullptr) {
        return const [];
      }
      final decoded = jsonDecode(raw.toDartString());
      if (decoded is! List) {
        return const [];
      }
      final entries = <SystemCatalogEntry>[];
      for (final item in decoded) {
        if (item is! Map) {
          continue;
        }
        final map = Map<String, dynamic>.from(item);
        final kind = map['kind'] as String? ?? 'setting';
        final target = map['target'] as String? ?? '';
        final id = kind == 'setting' ? 'setting:$target' : 'action:$target';
        entries.add(
          SystemCatalogEntry(
            id: id,
            title: map['title'] as String? ?? '',
            subtitle: kind == 'setting' ? '系统设置' : '系统动作',
            danger: map['danger'] == true,
            elevatable: map['elevatable'] == true,
          ),
        );
      }
      return entries;
    } catch (_) {
      return const [];
    } finally {
      if (raw != nullptr) {
        free(raw);
      }
      malloc.free(queryPtr);
    }
  }

  @override
  SearchActionOutcome runAction(String id) {
    final call = _runAction;
    if (call == null) {
      return SearchActionOutcome.failed;
    }
    final idPtr = id.toNativeUtf8();
    try {
      return switch (call(idPtr)) {
        0 => SearchActionOutcome.executed,
        2 => SearchActionOutcome.uiHandled,
        1 => SearchActionOutcome.unknown,
        _ => SearchActionOutcome.failed,
      };
    } catch (_) {
      return SearchActionOutcome.failed;
    } finally {
      malloc.free(idPtr);
    }
  }

  @override
  void dispose() {
    // 库由进程生命周期持有，这里没有需要释放的资源。
  }

  static List<SearchResult> _parseResults(String text) {
    final decoded = jsonDecode(text);
    if (decoded is! List) {
      return const [];
    }
    final results = <SearchResult>[];
    for (final item in decoded) {
      if (item is! Map) {
        continue;
      }
      final parsed = SearchResult.fromJson(Map<String, dynamic>.from(item));
      if (parsed != null) {
        results.add(parsed);
      }
    }
    return results;
  }
}
