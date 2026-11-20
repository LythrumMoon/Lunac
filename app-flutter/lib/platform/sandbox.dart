// 平台薄层：AI 沙箱盘（预览窗的数据层 + 窗口通道）。
//
// **口径**：这里的「沙箱」= agent 的受控工作目录 `<exe 根>/temp/transStorage`，
// 边界来自 agent 启动参数（cwd + `--add-dir`），不是 OS 级隔离
// （见 `docs/sandbox-research.md`）。所以界面上写「沙箱盘 / 受控工作目录」，
// 并如实显示固定额度与真实用量 —— 不承诺「虚拟机」。
//
// 数据在 `rust/src/sandbox.rs`（只读统计 / 清单 / 预览 / 清空）；
// 窗口是复用插件窗口基础设施的一个独立顶层窗（id = `sandbox`，不占「最多两个」的额度）。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';

import 'native_bridge.dart';

/// 沙箱盘状态。
class SandboxStatus {
  const SandboxStatus({
    this.path = '',
    this.exists = false,
    this.usedBytes = 0,
    this.quotaBytes = 0,
    this.fileCount = 0,
    this.dirCount = 0,
    this.truncated = false,
    this.recent = const [],
    this.error = '',
  });

  final String path;
  final bool exists;
  final int usedBytes;
  final int quotaBytes;
  final int fileCount;
  final int dirCount;

  /// 统计超过预算（用量是「至少这么多」）。
  final bool truncated;

  /// 最近改动的文件（按 mtime 倒序，最多 8 条；预览窗顶部那块列表）。
  final List<SandboxRecentItem> recent;
  final String error;

  /// 用量占比（0–1；额度为 0 时返回 0）。
  double get usageRatio {
    if (quotaBytes <= 0) {
      return 0;
    }
    return (usedBytes / quotaBytes).clamp(0.0, 1.0).toDouble();
  }

  bool get overQuota => quotaBytes > 0 && usedBytes > quotaBytes;

  static SandboxStatus fromJson(Map<String, dynamic> json) {
    int number(String key) {
      final value = json[key];
      return value is num ? value.toInt() : 0;
    }

    return SandboxStatus(
      path: json['path'] is String ? json['path'] as String : '',
      exists: json['exists'] == true,
      usedBytes: number('usedBytes'),
      quotaBytes: number('quotaBytes'),
      fileCount: number('fileCount'),
      dirCount: number('dirCount'),
      truncated: json['truncated'] == true,
      recent: [
        for (final item in (json['recent'] as List? ?? const []))
          if (item is Map<String, dynamic>) ?SandboxRecentItem.fromJson(item),
      ],
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 沙箱盘「最近改动」的一条（路径相对沙箱根，用 `/` 分隔）。
class SandboxRecentItem {
  const SandboxRecentItem({
    required this.path,
    required this.name,
    required this.size,
    required this.modifiedMs,
  });

  final String path;
  final String name;
  final int size;
  final int modifiedMs;

  static SandboxRecentItem? fromJson(Map<String, dynamic> json) {
    final path = json['path'];
    if (path is! String || path.isEmpty) {
      return null;
    }
    final size = json['size'];
    final modified = json['modifiedMs'];
    return SandboxRecentItem(
      path: path,
      name: json['name'] is String ? json['name'] as String : path,
      size: size is num ? size.toInt() : 0,
      modifiedMs: modified is num ? modified.toInt() : 0,
    );
  }
}

/// 沙箱盘里的一个条目（路径相对沙箱根，用 `/` 分隔）。
class SandboxEntry {
  const SandboxEntry({
    required this.path,
    required this.name,
    required this.isDir,
    required this.size,
    required this.modifiedMs,
  });

  final String path;
  final String name;
  final bool isDir;
  final int size;
  final int modifiedMs;

  /// 缩进层数（`a/b/c` → 2）。
  int get depth => path.split('/').length - 1;

  static SandboxEntry? fromJson(Map<String, dynamic> json) {
    final path = json['path'];
    if (path is! String || path.isEmpty) {
      return null;
    }
    final value = json['modifiedMs'];
    final size = json['size'];
    return SandboxEntry(
      path: path,
      name: json['name'] is String ? json['name'] as String : path,
      isDir: json['isDir'] == true,
      size: size is num ? size.toInt() : 0,
      modifiedMs: value is num ? value.toInt() : 0,
    );
  }
}

/// 沙箱盘清单。
class SandboxListing {
  const SandboxListing({
    this.path = '',
    this.entries = const [],
    this.usedBytes = 0,
    this.quotaBytes = 0,
    this.truncated = false,
    this.error = '',
  });

  final String path;
  final List<SandboxEntry> entries;
  final int usedBytes;
  final int quotaBytes;
  final bool truncated;
  final String error;

  static SandboxListing fromJson(Map<String, dynamic> json) {
    int number(String key) {
      final value = json[key];
      return value is num ? value.toInt() : 0;
    }

    return SandboxListing(
      path: json['path'] is String ? json['path'] as String : '',
      entries: [
        for (final item in (json['entries'] as List? ?? const []))
          if (item is Map<String, dynamic>) ?SandboxEntry.fromJson(item),
      ],
      usedBytes: number('usedBytes'),
      quotaBytes: number('quotaBytes'),
      truncated: json['truncated'] == true,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 单个文件的只读预览。
class SandboxFile {
  const SandboxFile({
    this.ok = false,
    this.kind = 'binary',
    this.path = '',
    this.name = '',
    this.size = 0,
    this.truncated = false,
    this.text = '',
    this.error = '',
  });

  final bool ok;

  /// `text` / `image` / `binary`
  final String kind;
  final String path;
  final String name;
  final int size;
  final bool truncated;
  final String text;
  final String error;

  static SandboxFile fromJson(Map<String, dynamic> json) {
    final size = json['size'];
    return SandboxFile(
      ok: json['ok'] == true,
      kind: json['kind'] is String ? json['kind'] as String : 'binary',
      path: json['path'] is String ? json['path'] as String : '',
      name: json['name'] is String ? json['name'] as String : '',
      size: size is num ? size.toInt() : 0,
      truncated: json['truncated'] == true,
      text: json['text'] is String ? json['text'] as String : '',
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 清空结果。
class SandboxClearResult {
  const SandboxClearResult({
    this.ok = false,
    this.removed = 0,
    this.failed = 0,
    this.error = '',
  });

  final bool ok;
  final int removed;
  final int failed;
  final String error;
}

// ── C ABI ────────────────────────────────────────────────────────

typedef _StringC = Pointer<Utf8> Function();
typedef _StringDart = Pointer<Utf8> Function();
typedef _TextStringC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextStringDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _QuotaC = Pointer<Utf8> Function(Uint64);
typedef _QuotaDart = Pointer<Utf8> Function(int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

Map<String, dynamic>? _decode(String? text) {
  if (text == null || text.isEmpty) {
    return null;
  }
  try {
    final value = jsonDecode(text);
    return value is Map<String, dynamic> ? value : null;
  } catch (_) {
    return null;
  }
}

/// 读沙箱盘状态；桥接缺失返回 null。
SandboxStatus? readSandboxStatus() {
  final json = _decode(_call0('lunac_sandbox_status'));
  return json == null ? null : SandboxStatus.fromJson(json);
}

/// 读沙箱盘清单；桥接缺失返回 null。
SandboxListing? readSandboxListing() {
  final json = _decode(_call0('lunac_sandbox_list'));
  return json == null ? null : SandboxListing.fromJson(json);
}

/// 只读预览一个文件（相对路径）；桥接缺失返回 null。
SandboxFile? readSandboxFile(String relativePath) {
  final json = _decode(_call1('lunac_sandbox_read', relativePath));
  return json == null ? null : SandboxFile.fromJson(json);
}

/// 清空沙箱盘（调用方负责二次确认）。
SandboxClearResult clearSandbox() {
  final json = _decode(_call0('lunac_sandbox_clear'));
  if (json == null) {
    return const SandboxClearResult(error: '不可用（rust 桥接未加载）');
  }
  int number(String key) {
    final value = json[key];
    return value is num ? value.toInt() : 0;
  }

  return SandboxClearResult(
    ok: json['ok'] == true,
    removed: number('removed'),
    failed: number('failed'),
    error: json['error'] is String ? json['error'] as String : '',
  );
}

/// 设置固定额度的结果。
class SandboxQuotaResult {
  const SandboxQuotaResult({
    this.ok = false,
    this.quotaBytes = 0,
    this.error = '',
  });

  final bool ok;

  /// 夹取后真正落盘的额度（失败时是当前生效值）。
  final int quotaBytes;
  final String error;
}

/// 设置沙箱盘的固定额度（Rust 侧夹取到 64 MiB–8 GiB）。
SandboxQuotaResult setSandboxQuota(int bytes) {
  final library = openLunacNative().library;
  if (library == null) {
    return const SandboxQuotaResult(error: '不可用（rust 桥接未加载）');
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_QuotaC, _QuotaDart>(
      'lunac_sandbox_set_quota',
    )(bytes);
    if (raw == nullptr) {
      return const SandboxQuotaResult(error: '设置额度没有返回结果');
    }
    final json = _decode(raw.toDartString());
    if (json == null) {
      return const SandboxQuotaResult(error: '设置额度的返回不是合法 JSON');
    }
    final quota = json['quotaBytes'];
    return SandboxQuotaResult(
      ok: json['ok'] == true,
      quotaBytes: quota is num ? quota.toInt() : 0,
      error: json['error'] is String ? json['error'] as String : '',
    );
  } catch (_) {
    return const SandboxQuotaResult(error: '设置额度失败');
  } finally {
    _free(library, raw);
  }
}

// ── 预览窗标签队列（AI 工具 → 聊天面板 → 预览窗）─────────────────

/// 一条待开的预览标签（`Preview` 工具事件）。
class PreviewTabRequest {
  const PreviewTabRequest({
    required this.title,
    required this.kind,
    required this.target,
  });

  /// 标签文本（`预览: 记事本` 这种由调用方拼好）。
  final String title;

  /// `app` / `file` / `url` / `workspace`
  final String kind;

  /// 应用可执行文件 / 文件相对路径 / URL；`workspace` 时为空。
  final String target;

  static PreviewTabRequest? fromJson(Object? json) {
    if (json is! Map<String, dynamic>) {
      return null;
    }
    final title = json['title'];
    if (title is! String || title.trim().isEmpty) {
      return null;
    }
    return PreviewTabRequest(
      title: title.trim(),
      kind: json['kind'] is String ? json['kind'] as String : 'app',
      target: json['target'] is String ? json['target'] as String : '',
    );
  }
}

/// 入队一条预览标签（聊天面板收到 agent 的 `Preview` 事件后调用）。
///
/// 与预览窗跨 FlutterEngine 转交，队列在 `lunac_core.dll` 的进程内静态区。
/// 返回 null = 成功，否则是一句原因。
String? pushPreviewTab(Map<String, Object?> tab) {
  final library = openLunacNative().library;
  if (library == null) {
    return '当前环境不支持预览窗';
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextStringC, _TextStringDart>(
      'lunac_preview_push',
    );
    input = jsonEncode(tab).toNativeUtf8();
    raw = call(input);
    if (raw == nullptr) {
      return '预览请求没有返回结果';
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic>) {
      return '预览请求的返回不是合法 JSON';
    }
    if (value['ok'] == true) {
      return null;
    }
    final reason = value['error'];
    return reason is String && reason.isNotEmpty ? reason : '入队预览标签失败';
  } catch (_) {
    return '入队预览标签失败';
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    _free(library, raw);
  }
}

/// 取走全部待开的预览标签（预览窗每秒调用一次）。
List<PreviewTabRequest> takePreviewTabs() {
  final text = _call0('lunac_preview_take');
  if (text == null || text.isEmpty) {
    return const [];
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map<String, dynamic>) {
      return const [];
    }
    return [
      for (final item in (value['tabs'] as List? ?? const []))
        ?PreviewTabRequest.fromJson(item),
    ];
  } catch (_) {
    return const [];
  }
}

// ── 文件操作（预览区＝「小 VSCode」）────────────────────────────────
//
// 全部走 Rust 的双重越界闸（`safe_join` + `canonicalize`）；同名不覆盖、
// 目录只能删空的 —— 界面上的二次确认只是礼貌，真正的约束在 Rust 侧。

/// 文件操作回执。
class SandboxOpResult {
  const SandboxOpResult({required this.ok, this.path = '', this.error = ''});

  final bool ok;

  /// 成功时 = 落点的绝对路径（删除时回显请求里的相对路径）。
  final String path;

  /// 失败原因（一句人话，可直接显示）。
  final String error;

  static SandboxOpResult fromRaw(String? text) {
    if (text == null || text.isEmpty) {
      return const SandboxOpResult(ok: false, error: '当前环境没有原生桥，文件操作不可用');
    }
    try {
      final value = jsonDecode(text);
      if (value is! Map<String, dynamic>) {
        return const SandboxOpResult(ok: false, error: '文件操作的返回不是合法 JSON');
      }
      return SandboxOpResult(
        ok: value['ok'] == true,
        path: value['path'] is String ? value['path'] as String : '',
        error: value['error'] is String ? value['error'] as String : '',
      );
    } catch (_) {
      return const SandboxOpResult(ok: false, error: '文件操作失败');
    }
  }
}

/// 新建文本文件（相对沙箱根；同名已存在会被拒绝，不覆盖）。
SandboxOpResult sandboxWriteFile(String path, String text) =>
    SandboxOpResult.fromRaw(
      _call1('lunac_sandbox_write', jsonEncode({'path': path, 'text': text})),
    );

/// 新建目录（上级目录必须已存在）。
SandboxOpResult sandboxMkdir(String path) => SandboxOpResult.fromRaw(
  _call1('lunac_sandbox_mkdir', jsonEncode({'path': path})),
);

/// 重命名 / 移动（同一沙箱内；目标不存在）。
SandboxOpResult sandboxRename(String from, String to) =>
    SandboxOpResult.fromRaw(
      _call1('lunac_sandbox_rename', jsonEncode({'from': from, 'to': to})),
    );

/// 删除文件 / 空目录（沙箱根不可删）。
SandboxOpResult sandboxDelete(String path) => SandboxOpResult.fromRaw(
  _call1('lunac_sandbox_delete', jsonEncode({'path': path})),
);

// ── 「添加到对话」队列（预览窗 → 聊天面板 / 主窗）─────────────────────

/// 把绝对路径放进跨窗口队列；返回 null = 成功，否则一句原因。
///
/// 消费方：聊天面板（开着就加到附件栏）或主窗（没开对话就开一个）。
String? pushChatAttachments(List<String> paths) {
  if (paths.isEmpty) {
    return null;
  }
  final text = _call1('lunac_chat_attach_push', jsonEncode({'paths': paths}));
  if (text == null) {
    return '当前环境不支持「添加到对话」';
  }
  try {
    final value = jsonDecode(text);
    if (value is Map && value['ok'] == true) {
      return null;
    }
    final error = value is Map ? value['error'] : null;
    return error is String && error.isNotEmpty ? error : '入队附件失败';
  } catch (_) {
    return '入队附件失败';
  }
}

/// 取走全部待加入对话的文件（聊天面板 / 主窗每秒调用一次；取走即排空）。
List<String> takeChatAttachments() {
  final text = _call0('lunac_chat_attach_take');
  if (text == null || text.isEmpty) {
    return const [];
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map<String, dynamic>) {
      return const [];
    }
    return [
      for (final item in (value['paths'] as List? ?? const []))
        if (item is String && item.isNotEmpty) item,
    ];
  } catch (_) {
    return const [];
  }
}

// ── 工作区指纹（命令类产出识别，§待办 3-28.13 ②）────────────────────
//
// 只抓元数据（大小 + 改动时间），前后各一次、比出新增 / 更新 —— 命令里的写目标
// （`> out.txt`、`Out-File`、`python gen.py`）无法静态判定，只能靠目录差异。
// **不产生可还原快照**：改动已经发生，没有前像；界面据此标「命令产出」。

/// 一次抓帧：`{相对路径: "大小:改动ms"}` + 是否被预算截断。
class WorkspaceFingerprint {
  const WorkspaceFingerprint({
    required this.ok,
    required this.truncated,
    required this.entries,
  });

  final bool ok;
  final bool truncated;
  final Map<String, String> entries;

  static const empty = WorkspaceFingerprint(
    ok: false,
    truncated: false,
    entries: {},
  );
}

WorkspaceFingerprint readWorkspaceFingerprint() {
  final raw = _call0('lunac_workspace_fingerprint');
  if (raw == null) {
    return WorkspaceFingerprint.empty;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return WorkspaceFingerprint.empty;
    }
    final entries = <String, String>{};
    final map = value['entries'];
    if (map is Map) {
      map.forEach((key, val) {
        if (key is String && val is String) {
          entries[key] = val;
        }
      });
    }
    return WorkspaceFingerprint(
      ok: value['ok'] == true,
      truncated: value['truncated'] == true,
      entries: entries,
    );
  } catch (_) {
    return WorkspaceFingerprint.empty;
  }
}

/// 两次指纹的差：新增 + 尺寸 / 改动时间变化的相对路径（删除不算「产出」）。
List<String> diffWorkspaceFingerprints(
  Map<String, String> before,
  Map<String, String> after,
) {
  final out = <String>[];
  for (final entry in after.entries) {
    if (before[entry.key] != entry.value) {
      out.add(entry.key);
    }
  }
  out.sort();
  return out;
}

String? _call0(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_StringC, _StringDart>(symbol)();
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

String? _call1(String symbol, String argument) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextStringC, _TextStringDart>(symbol);
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

// ── 窗口通道 ─────────────────────────────────────────────────────

const MethodChannel _host = MethodChannel('lunac/sandbox-host');

bool _hostInstalled = false;

/// 宿主是否已就绪（`installSandboxWindowBridge()` 之后为真）。
///
/// 单测 / 无原生侧时不装，调用方据此立即回退（否则 `invokeMethod` 会一直等）。
bool get sandboxWindowHostReady => _hostInstalled;

/// 安装沙箱窗口通道（幂等）。主引擎与插件窗口引擎各调一次。
void installSandboxWindowBridge() {
  _hostInstalled = true;
}

/// 预览窗是否开着（AI 标题栏按钮用它做「再点一次关闭」）。
Future<bool> isSandboxWindowOpen() async {
  if (!_hostInstalled) {
    return false;
  }
  try {
    final open = await _host.invokeMethod<bool>('isOpen');
    return open == true;
  } on MissingPluginException {
    return false;
  } on PlatformException {
    return false;
  }
}

/// 打开沙箱预览窗（已在则提到前台）。返回是否已打开。
Future<bool> openSandboxWindow() async {
  if (!_hostInstalled) {
    return false;
  }
  try {
    final opened = await _host.invokeMethod<bool>('open');
    return opened == true;
  } on MissingPluginException {
    return false;
  } on PlatformException {
    return false;
  }
}

/// 关闭沙箱预览窗（不存在也算成功）。
Future<void> closeSandboxWindow() async {
  if (!_hostInstalled) {
    return;
  }
  try {
    await _host.invokeMethod<void>('close');
  } on MissingPluginException {
    // 无宿主（单测）时忽略。
  }
}
