// 通用后台任务队列的 Dart 桥（docs/todo.md §待办 3-28.16 ①）。
//
// 数据契约与 core-agent 的 `TaskEnqueue` / `TaskList` / `TaskCancel` 一致：
//   `ModuleData/tasks/queue.json` —— agent 只写入队 / 取消，宿主 worker 写运行态。
// 这里只做「起 worker / 读列表 / 取消 pending」，不解析文件、不猜路径
// （与 `cron.dart` 的分工一致）。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条后台任务（字段名与 agent 侧 `queue.json` 的条目一致）。
class TaskItem {
  const TaskItem({
    required this.id,
    required this.prompt,
    required this.status,
    required this.round,
    required this.maxRounds,
    required this.sessionId,
    required this.summary,
    required this.error,
    required this.createdAt,
    required this.updatedAt,
  });

  final String id;
  final String prompt;

  /// `pending` / `running` / `done` / `failed` / `cancelled`。
  final String status;
  final int round;
  final int maxRounds;
  final String sessionId;
  final String summary;
  final String error;
  final int createdAt;
  final int updatedAt;

  bool get isPending => status == 'pending';
  bool get isRunning => status == 'running';

  /// 界面上的中文状态（未识别时原样显示状态串，不吞信息）。
  String get statusLabel => switch (status) {
    'pending' => '排队中',
    'running' => '运行中',
    'done' => '完成',
    'failed' => '失败',
    'cancelled' => '已取消',
    _ => status,
  };

  static TaskItem fromJson(Map<String, dynamic> json) {
    return TaskItem(
      id: _str(json['id']),
      prompt: _str(json['prompt']),
      status: _str(json['status']).isEmpty ? 'pending' : _str(json['status']),
      round: _int(json['round']),
      maxRounds: _int(json['maxRounds']),
      sessionId: _str(json['sessionId']),
      summary: _str(json['summary']),
      error: _str(json['error']),
      createdAt: _int(json['createdAt']),
      updatedAt: _int(json['updatedAt']),
    );
  }
}

/// 列表快照：`ok=false` 时 `reason` 取 `missing` / `invalid`（与 Rust 侧同词）。
class TaskQueueSnapshot {
  const TaskQueueSnapshot({
    required this.ok,
    required this.reason,
    required this.items,
  });

  final bool ok;
  final String reason;
  final List<TaskItem> items;

  static const empty = TaskQueueSnapshot(
    ok: false,
    reason: 'unavailable',
    items: [],
  );
}

/// 取消 / 其它操作的结果。
class TaskActionResult {
  const TaskActionResult({required this.ok, required this.error});

  final bool ok;
  final String error;
}

/// 启动 worker（幂等；App 启动时调一次）。
void startTaskQueue() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<Void Function(), void Function()>(
      'lunac_tasks_start',
    )();
  } catch (_) {
    // 旧桥没有这个符号：保持静默（列表会显示 reason=unavailable）。
  }
}

/// 读列表（同步：只读一个小 JSON 文件）。
TaskQueueSnapshot readTaskQueue() {
  final raw = _callNoArg('lunac_tasks_list');
  if (raw == null) {
    return TaskQueueSnapshot.empty;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return TaskQueueSnapshot.empty;
    }
    final items = <TaskItem>[];
    final list = value['items'];
    if (list is List) {
      for (final item in list) {
        if (item is Map<String, dynamic>) {
          items.add(TaskItem.fromJson(item));
        }
      }
    }
    return TaskQueueSnapshot(
      ok: value['ok'] == true,
      reason: _str(value['reason']),
      items: items,
    );
  } catch (_) {
    return TaskQueueSnapshot.empty;
  }
}

/// 取消一条**排队中**的任务（running 会被 Rust 侧按拒绝处理）。
Future<TaskActionResult> cancelTask(String id) {
  return Isolate.run(() {
    final raw = _callJson('lunac_tasks_cancel', {'id': id});
    if (raw == null) {
      return const TaskActionResult(ok: false, error: '桥接不可用');
    }
    try {
      final value = jsonDecode(raw);
      if (value is! Map<String, dynamic>) {
        return const TaskActionResult(ok: false, error: '返回格式异常');
      }
      return TaskActionResult(
        ok: value['ok'] == true,
        error: _str(value['error']),
      );
    } catch (_) {
      return const TaskActionResult(ok: false, error: '返回格式异常');
    }
  });
}

String _str(Object? value) => value is String ? value : '';

int _int(Object? value) => value is int ? value : 0;

typedef _NoArgC = Pointer<Utf8> Function();
typedef _NoArgDart = Pointer<Utf8> Function();
typedef _TextInC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextInDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

String? _callNoArg(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_NoArgC, _NoArgDart>(symbol);
    raw = call();
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

String? _callJson(String symbol, Map<String, Object?> payload) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextInC, _TextInDart>(symbol);
    input = jsonEncode(payload).toNativeUtf8();
    raw = call(input);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    _free(library, raw);
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? raw) {
  if (raw == null || raw == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
  } catch (_) {
    // 符号缺失时忽略。
  }
}
