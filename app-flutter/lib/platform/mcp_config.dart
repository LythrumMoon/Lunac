// 平台薄层：工具编辑器（v1 `tool-editor` 插件的原生等价物）。
//
// 边界：定位 / 解析 / 校验 / 原子写 / 删除都在 `rust/src/mcp_config.rs`；本文件只负责
// 数据形状、容错解析与两个 C ABI（`lunac_mcp_config` / `lunac_mcp_config_save`）。
//
// 存储口径与 v1 一致：`<exe 根>/tools/*.json`，一份文件 = 一个工具定义
// （`name` / `description` / `inputSchema` / `handler`）。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条工具定义（`lunac_mcp_config` 的 `entries[]`）。
class ToolEntry {
  const ToolEntry({
    required this.filename,
    required this.name,
    required this.description,
    required this.command,
    required this.kind,
    required this.params,
    required this.valid,
  });

  /// 来源文件（`tools/` 下的文件名）。
  final String filename;
  final String name;
  final String description;

  /// handler 的一行摘要（shell 的 command / http 的 `METHOD url` / builtin 的 name）。
  final String command;

  /// `shell` / `http` / `builtin`；坏文件为空串。
  final String kind;

  /// `inputSchema.properties` 的键（参数摘要）。
  final List<String> params;

  /// 结构与 v1 `ToolDef` 一致（缺 name / handler 等 = false，仍会列出来）。
  final bool valid;

  /// 面板标题：name 为空时退回文件名（v1 `tool.name || tool.filename`）。
  String get displayName => name.isNotEmpty ? name : filename;

  static ToolEntry? fromJson(Map<String, dynamic> json) {
    final filename = json['filename'];
    if (filename is! String || filename.isEmpty) {
      return null;
    }
    return ToolEntry(
      filename: filename,
      name: json['name'] is String ? json['name'] as String : '',
      description: json['description'] is String
          ? json['description'] as String
          : '',
      command: json['command'] is String ? json['command'] as String : '',
      kind: json['kind'] is String ? json['kind'] as String : '',
      params: [
        for (final param in (json['params'] as List? ?? const []))
          if (param is String && param.isNotEmpty) param,
      ],
      valid: json['valid'] == true,
    );
  }
}

/// `lunac_mcp_config` 的解析结果。
class ToolConfigResult {
  const ToolConfigResult({required this.entries, required this.error});

  final List<ToolEntry> entries;

  /// 目录读不了 / 解析失败时的一句人话（正常为空串）。
  final String error;

  bool get ok => error.isEmpty;
}

/// 解析 Rust 返回的工具列表（纯函数，单测不加载 DLL）。
///
/// 坏 JSON / 形状不对都收敛成「空列表 + 一句原因」，绝不抛异常。
ToolConfigResult parseToolConfig(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const ToolConfigResult(entries: [], error: '工具列表不是合法 JSON');
  }
  if (decoded is! Map) {
    return const ToolConfigResult(entries: [], error: '工具列表格式不对');
  }
  final entries = <ToolEntry>[];
  for (final item in (decoded['entries'] as List? ?? const [])) {
    if (item is Map<String, dynamic>) {
      final entry = ToolEntry.fromJson(item);
      if (entry != null) {
        entries.add(entry);
      }
    }
  }
  return ToolConfigResult(
    entries: entries,
    error: decoded['error'] is String ? decoded['error'] as String : '',
  );
}

/// 写入结果（`lunac_mcp_config_save` 的 JSON）。
class ToolSaveResult {
  const ToolSaveResult({required this.ok, required this.error});

  final bool ok;
  final String error;
}

/// 解析写入结果（纯函数）。
ToolSaveResult parseSaveResult(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const ToolSaveResult(ok: false, error: '写入返回的不是合法 JSON');
  }
  if (decoded is! Map) {
    return const ToolSaveResult(ok: false, error: '写入返回格式不对');
  }
  return ToolSaveResult(
    ok: decoded['ok'] == true,
    error: decoded['error'] is String ? decoded['error'] as String : '',
  );
}

/// 一份工具的原始 JSON 文本（`lunac_tool_read` 的 JSON）。
class ToolFileContent {
  const ToolFileContent({
    required this.ok,
    required this.content,
    required this.error,
  });

  final bool ok;
  final String content;
  final String error;
}

/// 解析读取结果（纯函数）。
ToolFileContent parseToolFileContent(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const ToolFileContent(ok: false, content: '', error: '读取返回的不是合法 JSON');
  }
  if (decoded is! Map) {
    return const ToolFileContent(ok: false, content: '', error: '读取返回格式不对');
  }
  final ok = decoded['ok'] == true;
  final content = decoded['content'];
  final error = decoded['error'];
  if (!ok) {
    return ToolFileContent(
      ok: false,
      content: '',
      error: error is String && error.trim().isNotEmpty
          ? error.trim()
          : '读不到这份工具定义',
    );
  }
  return ToolFileContent(
    ok: true,
    content: content is String ? content : '',
    error: '',
  );
}

typedef _TextC = Pointer<Utf8> Function();
typedef _TextDart = Pointer<Utf8> Function();
typedef _TextWithArgC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextWithArgDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读全部工具定义；桥接缺失返回 null（面板显示「不可用」）。
ToolConfigResult? readToolConfig() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_TextC, _TextDart>('lunac_mcp_config')();
    if (raw == nullptr) {
      return null;
    }
    return parseToolConfig(raw.toDartString());
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

/// 发送一条写入请求（`save` / `delete`）。
ToolSaveResult saveToolRequest(Map<String, dynamic> request) {
  final library = openLunacNative().library;
  if (library == null) {
    return const ToolSaveResult(ok: false, error: 'rust 桥接不可用');
  }
  Pointer<Utf8>? pointer;
  Pointer<Utf8>? raw;
  try {
    final save = library.lookupFunction<_TextWithArgC, _TextWithArgDart>(
      'lunac_mcp_config_save',
    );
    pointer = jsonEncode(request).toNativeUtf8();
    raw = save(pointer);
    if (raw == nullptr) {
      return const ToolSaveResult(ok: false, error: '写入没有返回结果');
    }
    return parseSaveResult(raw.toDartString());
  } catch (_) {
    return const ToolSaveResult(ok: false, error: 'rust 桥接调用失败');
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
    _free(library, raw);
  }
}

/// 读取一份工具的原始 JSON 文本（v1 `read_tool_file`）。
///
/// 桥接缺失 / 读不到时 `ok=false` 并带一句人话。
ToolFileContent readToolFile(String filename) {
  final library = openLunacNative().library;
  if (library == null) {
    return const ToolFileContent(
      ok: false,
      content: '',
      error: 'rust 桥接不可用',
    );
  }
  Pointer<Utf8>? pointer;
  Pointer<Utf8>? raw;
  try {
    final read = library.lookupFunction<_TextWithArgC, _TextWithArgDart>(
      'lunac_tool_read',
    );
    pointer = filename.toNativeUtf8();
    raw = read(pointer);
    if (raw == nullptr) {
      return const ToolFileContent(ok: false, content: '', error: '读取没有返回结果');
    }
    return parseToolFileContent(raw.toDartString());
  } catch (_) {
    return const ToolFileContent(ok: false, content: '', error: 'rust 桥接调用失败');
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
    _free(library, raw);
  }
}

/// 直接保存一份工具的原始 JSON 文本（v1 `save_tool_file`）。
///
/// 返回 null = 成功；否则是一句可显示的原因（Rust 已做 JSON / 结构校验）。
String? saveToolContent(String filename, String content) {
  final result = saveToolRequest({
    'action': 'save',
    'filename': filename,
    'content': content,
  });
  return result.ok ? null : result.error;
}

/// 删除一条；不存在也算成功。
String? deleteTool(String filename) {
  final result = saveToolRequest({'action': 'delete', 'filename': filename});
  return result.ok ? null : result.error;
}

/// 文件名口径与 v1 `tool-editor.ts:148` 一致：非 `[A-Za-z0-9_-]` 一律换成 `-`。
String toolFileName(String name) =>
    name.replaceAll(RegExp(r'[^A-Za-z0-9_-]'), '-');

/// 远端 MCP 配置（`<exe 根>\config\mcp.json`）的绝对路径；文件不存在时宿主先落空模板。
///
/// 这是 v1 设置「工具 (MCP)」组唯一的动作（打开配置文件）—— 图形编辑器我们**刻意不做**：
/// 写错一个字段就让整份配置失效（agent 侧 `parse_config` 整份忽略），不如让用户直接改 JSON。
///
/// 返回 `(path, error)`；`path` 为空 = 没准备好（桥接缺失 / 目录只读），`error` 是一句人话。
({String path, String error}) ensureMcpConfigFile() {
  final library = openLunacNative().library;
  if (library == null) {
    return (path: '', error: 'rust 桥接不可用');
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_TextC, _TextDart>('lunac_mcp_config_file')();
    if (raw == nullptr) {
      return (path: '', error: '没有返回配置路径');
    }
    final decoded = jsonDecode(raw.toDartString());
    if (decoded is! Map) {
      return (path: '', error: '配置路径返回格式不对');
    }
    final ok = decoded['ok'] == true;
    final path = decoded['path'] is String ? decoded['path'] as String : '';
    final error = decoded['error'] is String ? decoded['error'] as String : '';
    if (!ok || path.isEmpty) {
      return (path: '', error: error.isEmpty ? '准备不了 mcp.json' : error);
    }
    return (path: path, error: '');
  } catch (_) {
    return (path: '', error: 'rust 桥接调用失败');
  } finally {
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
