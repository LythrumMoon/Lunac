// 平台薄层：长期记忆（全局 `/memory` + 技能级命名空间）。
//
// 实现在 Rust（`rust/src/memory.rs`）：
//   · 全局：`ModuleData/memory/MEMORY.md` + `pages/*.md`；
//   · 技能级（2026-11-19）：`ModuleData/memory/skills/<key>/MEMORY.md`，只在该技能的
//     fork 子代理任务里注入，不碰主前缀。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _ReadC = Pointer<Utf8> Function();
typedef _ReadDart = Pointer<Utf8> Function();
typedef _ReadArgC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _ReadArgDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _ClearC = Int32 Function();
typedef _ClearDart = int Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

String _readJson(Pointer<Utf8>? raw, DynamicLibrary library) {
  if (raw == null || raw == nullptr) {
    return '';
  }
  try {
    return raw.toDartString();
  } finally {
    try {
      library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
    } catch (_) {
      // 符号缺失时忽略。
    }
  }
}

/// 读长期记忆全文；没有记忆 / 桥接缺失 = 空串。
String readMemoryText() {
  final library = openLunacNative().library;
  if (library == null) {
    return '';
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_ReadC, _ReadDart>('lunac_memory_read')();
    if (raw == nullptr) {
      return '';
    }
    return raw.toDartString();
  } catch (_) {
    return '';
  } finally {
    if (raw != null && raw != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 清空长期记忆；false = 桥接缺失或写失败。
bool clearMemoryText() {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    final clear = library.lookupFunction<_ClearC, _ClearDart>(
      'lunac_memory_clear',
    );
    return clear() == 0;
  } catch (_) {
    return false;
  }
}

// ── 技能级记忆（§待办 3-28.10 ①）────────────────────────────────

class SkillMemoryEntry {
  const SkillMemoryEntry({
    required this.key,
    required this.chars,
    required this.updatedAt,
    required this.path,
  });

  final String key;
  final int chars;
  final int updatedAt;
  final String path;

  static SkillMemoryEntry? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final key = json['key'];
    if (key is! String || key.isEmpty) {
      return null;
    }
    return SkillMemoryEntry(
      key: key,
      chars: json['chars'] is num ? (json['chars'] as num).toInt() : 0,
      updatedAt: json['updatedAt'] is num
          ? (json['updatedAt'] as num).toInt()
          : 0,
      path: json['path'] is String ? json['path'] as String : '',
    );
  }
}

class SkillMemoryList {
  const SkillMemoryList({this.ok = false, this.items = const [], this.error = ''});

  final bool ok;
  final List<SkillMemoryEntry> items;
  final String error;
}

SkillMemoryList readSkillMemories() {
  final library = openLunacNative().library;
  if (library == null) {
    return const SkillMemoryList(error: 'rust 桥接不可用');
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_ReadC, _ReadDart>('lunac_skill_memory_list')();
    final text = _readJson(raw, library);
    if (text.isEmpty) {
      return const SkillMemoryList(error: '没有返回结果');
    }
    final decoded = jsonDecode(text);
    if (decoded is! Map) {
      return const SkillMemoryList(error: '返回结果不是 JSON 对象');
    }
    return SkillMemoryList(
      ok: decoded['ok'] == true,
      items: [
        for (final item in (decoded['items'] as List? ?? const []))
          ?SkillMemoryEntry.fromJson(item),
      ],
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  } catch (_) {
    return const SkillMemoryList(error: 'rust 桥接调用失败');
  }
}

class SkillMemoryText {
  const SkillMemoryText({this.ok = false, this.text = '', this.error = ''});

  final bool ok;
  final String text;
  final String error;
}

SkillMemoryText readSkillMemory(String key) {
  final library = openLunacNative().library;
  if (library == null) {
    return const SkillMemoryText(error: 'rust 桥接不可用');
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    arg = key.toNativeUtf8();
    raw = library.lookupFunction<_ReadArgC, _ReadArgDart>(
      'lunac_skill_memory_read',
    )(arg);
    final text = _readJson(raw, library);
    if (text.isEmpty) {
      return const SkillMemoryText(error: '没有返回结果');
    }
    final decoded = jsonDecode(text);
    if (decoded is! Map) {
      return const SkillMemoryText(error: '返回结果不是 JSON 对象');
    }
    return SkillMemoryText(
      ok: decoded['ok'] == true,
      text: decoded['text'] is String ? decoded['text'] as String : '',
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  } catch (_) {
    return const SkillMemoryText(error: 'rust 桥接调用失败');
  } finally {
    if (arg != null) {
      malloc.free(arg);
    }
  }
}

/// 清空某个技能的记忆；false = 桥接缺失或失败。
bool clearSkillMemory(String key) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    arg = key.toNativeUtf8();
    raw = library.lookupFunction<_ReadArgC, _ReadArgDart>(
      'lunac_skill_memory_clear',
    )(arg);
    final text = _readJson(raw, library);
    if (text.isEmpty) {
      return false;
    }
    final decoded = jsonDecode(text);
    return decoded is Map && decoded['ok'] == true;
  } catch (_) {
    return false;
  } finally {
    if (arg != null) {
      malloc.free(arg);
    }
  }
}
