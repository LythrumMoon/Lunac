// 会话内「AI 改过的文件」快照 / 审阅 / 撤销（Rust 侧见 `rust/src/change_review.rs`）。
//
// 快照在 `tool_use` 事件到达时采（那时工具还没执行），所以 diff 的「旧侧」是真的原文。
// 全部操作都是本地文件读写，不联网；没有桥接（单测 / 非 Windows）时一律返回空结果。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _TextStringC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextStringDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextTextStringC = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _TextTextStringDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _TextIntStringC = Pointer<Utf8> Function(Pointer<Utf8>, Int32);
typedef _TextIntStringDart = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// diff 里的一行：`same` / `add` / `del`。
class ChangeDiffLine {
  const ChangeDiffLine({required this.kind, required this.text});

  final String kind;
  final String text;
}

/// 一个文件的净变更（快照 vs 当前磁盘）。
class ChangeDiff {
  const ChangeDiff({
    this.ok = false,
    this.existed = true,
    this.added = 0,
    this.removed = 0,
    this.truncated = false,
    this.binary = false,
    this.error = '',
    this.lines = const [],
  });

  final bool ok;

  /// 改动前文件是否存在（false = AI 新建）。
  final bool existed;
  final int added;
  final int removed;
  final bool truncated;
  final bool binary;
  final String error;
  final List<ChangeDiffLine> lines;

  static ChangeDiff? fromJson(Object? json) {
    if (json is! Map<String, dynamic>) {
      return null;
    }
    final rawLines = json['lines'];
    return ChangeDiff(
      ok: json['ok'] == true,
      existed: json['existed'] != false,
      added: json['added'] is int ? json['added'] as int : 0,
      removed: json['removed'] is int ? json['removed'] as int : 0,
      truncated: json['truncated'] == true,
      binary: json['binary'] == true,
      error: json['error'] is String ? json['error'] as String : '',
      lines: [
        if (rawLines is List)
          for (final item in rawLines)
            if (item is Map)
              ChangeDiffLine(
                kind: item['kind'] is String ? item['kind'] as String : 'same',
                text: item['text'] is String ? item['text'] as String : '',
              ),
      ],
    );
  }
}

/// 撤销结果（写回 / 删除 / 失败）。
class ChangeRevertResult {
  const ChangeRevertResult({
    this.ok = false,
    this.restored = 0,
    this.deleted = 0,
    this.failed = 0,
    this.error = '',
    this.paths = const [],
  });

  final bool ok;
  final int restored;
  final int deleted;
  final int failed;
  final String error;

  /// 这次真的动过的文件（`revertChangesSince` 用来把它们从「改动文件」列表摘掉）。
  final List<String> paths;

  static ChangeRevertResult? fromJson(Object? json) {
    if (json is! Map<String, dynamic>) {
      return null;
    }
    final rawPaths = json['paths'];
    return ChangeRevertResult(
      ok: json['ok'] == true,
      restored: json['restored'] is int ? json['restored'] as int : 0,
      deleted: json['deleted'] is int ? json['deleted'] as int : 0,
      failed: json['failed'] is int ? json['failed'] as int : 0,
      error: json['error'] is String ? json['error'] as String : '',
      paths: [
        if (rawPaths is List)
          for (final item in rawPaths)
            if (item is String && item.isNotEmpty) item,
      ],
    );
  }
}

/// 采一份「改动前」快照（同一回合、同一文件只采一次）。
///
/// [turn] = 采集时已完成的助手消息条数（v1 `sessionFileSnapshots.turn` 同口径）；
/// 回退时按它判断「哪些回合的改动要还原」。
/// [session] = 这份快照属于哪个会话（v1 `snapshots.rs` 的 per-session frames）：
/// 每个会话一个仓，**回退 A 会话不会误还原 B 会话改过的文件**。空 = 老布局（根目录）。
bool snapshotChange(String path, {int turn = 0, String session = ''}) {
  final text = _call1(
    'lunac_change_snapshot_at',
    jsonEncode({'session': session, 'path': path, 'turn': turn}),
  );
  if (text == null) {
    return false;
  }
  try {
    final value = jsonDecode(text);
    return value is Map && value['ok'] == true;
  } catch (_) {
    return false;
  }
}

/// 读一个文件的净变更；没有快照 / 无桥接时返回 null。
ChangeDiff? readChangeDiff(String path, {String session = ''}) {
  final text = _call2('lunac_change_diff', session, path);
  if (text == null) {
    return null;
  }
  try {
    return ChangeDiff.fromJson(jsonDecode(text));
  } catch (_) {
    return null;
  }
}

/// 确认保留改动（删快照，不动文件）；[path] 为空 = 全部确认。
bool acceptChanges({String path = '', String session = ''}) {
  final text = _call2('lunac_change_accept', session, path);
  if (text == null) {
    return false;
  }
  try {
    final value = jsonDecode(text);
    return value is Map && value['ok'] == true;
  } catch (_) {
    return false;
  }
}

/// 撤销改动（写回原文 / 删除 AI 新建的文件）；[path] 为空 = 全部撤销。
ChangeRevertResult revertChanges({String path = '', String session = ''}) {
  final text = _call2('lunac_change_revert', session, path);
  if (text == null) {
    return const ChangeRevertResult();
  }
  try {
    return ChangeRevertResult.fromJson(jsonDecode(text)) ??
        const ChangeRevertResult();
  } catch (_) {
    return const ChangeRevertResult();
  }
}

/// 回退到第 [kept] 回合：只还原 `turn >= kept` 的快照（v1 回退按钮的文件还原）。
///
/// 按回合倒序写回，所以同一文件被多轮改过时落在**回退点那一刻**的内容；
/// 返回的 `paths` 是这次真动过的文件。[session] 为空 = 扫全部会话的仓。
ChangeRevertResult revertChangesSince(int kept, {String session = ''}) {
  final text = _callTextInt(
    'lunac_change_revert_since',
    session,
    kept < 0 ? 0 : kept,
  );
  if (text == null) {
    return const ChangeRevertResult();
  }
  try {
    return ChangeRevertResult.fromJson(jsonDecode(text)) ??
        const ChangeRevertResult();
  } catch (_) {
    return const ChangeRevertResult();
  }
}

/// 这个文件是否还有未确认的改动快照。
bool isChangePending(String path, {String session = ''}) {
  final text = _call2('lunac_change_pending', session, path);
  if (text == null) {
    return false;
  }
  try {
    final value = jsonDecode(text);
    return value is Map && value['pending'] == true;
  } catch (_) {
    return false;
  }
}

/// 清掉已删会话的快照目录（v1 `prune_frames`）：[keep] = 仍存在的会话 id。
void pruneChangeSessions(List<String> keep) {
  _call1('lunac_change_prune', jsonEncode(keep));
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

/// 两个文本入参的 FFI（`lunac_change_*` 的 `(session, path)` 形态）。
String? _call2(String symbol, String first, String second) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? a;
  Pointer<Utf8>? b;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextTextStringC, _TextTextStringDart>(
      symbol,
    );
    a = first.toNativeUtf8();
    b = second.toNativeUtf8();
    raw = call(a, b);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (a != null) {
      malloc.free(a);
    }
    if (b != null) {
      malloc.free(b);
    }
    _free(library, raw);
  }
}

/// `(session, int)` → JSON 文本（`lunac_change_revert_since`）。
String? _callTextInt(String symbol, String text, int number) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextIntStringC, _TextIntStringDart>(
      symbol,
    );
    arg = text.toNativeUtf8();
    raw = call(arg, number);
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
