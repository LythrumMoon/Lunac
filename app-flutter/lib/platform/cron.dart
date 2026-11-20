// 定时任务（宿主调度）的 Dart 桥。
//
// 数据来自 Rust 的 `cron.rs`：`ModuleData/tasks/cron.json`（定义，agent 侧写）+
// `cron-state.json`（运行态，宿主写）。这里只做「读列表 / 立即运行 / 忙标记 / 可写授权」，
// 不解析文件、不猜路径（与 v1 `cron.ts` 的分工一致）。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条定时任务（定义 + 运行态合并后的视图，字段名与 agent 的 `CronList` 对齐）。
class CronJob {
  const CronJob({
    required this.id,
    required this.prompt,
    required this.scheduleText,
    required this.enabled,
    required this.writeAccess,
    required this.autoDisabled,
    required this.lastStatus,
    required this.lastSummary,
    required this.lastError,
    required this.nextRunAtLocal,
    required this.runCount,
    required this.failureCount,
    required this.todayRuns,
    required this.maxRunsPerDay,
  });

  final String id;
  final String prompt;
  final String scheduleText;
  final bool enabled;
  final bool writeAccess;
  final bool autoDisabled;
  final String lastStatus;
  final String lastSummary;
  final String lastError;
  final String nextRunAtLocal;
  final int runCount;
  final int failureCount;
  final int todayRuns;
  final int maxRunsPerDay;

  static CronJob fromJson(Map<String, dynamic> json) {
    return CronJob(
      id: _str(json['id']),
      prompt: _str(json['prompt']),
      scheduleText: _str(json['scheduleText']),
      enabled: json['enabled'] != false,
      writeAccess: json['writeAccess'] == true,
      autoDisabled: json['autoDisabled'] == true,
      lastStatus: _str(json['lastStatus']),
      lastSummary: _str(json['lastSummary']),
      lastError: _str(json['lastError']),
      nextRunAtLocal: _str(json['nextRunAtLocal']),
      runCount: _int(json['runCount']),
      failureCount: _int(json['failureCount']),
      todayRuns: _int(json['todayRuns']),
      maxRunsPerDay: _int(json['maxRunsPerDay']),
    );
  }
}

/// 列表快照：`ok=false` 时 `reason` 给出 missing / invalid（与 agent 同词）。
class CronSnapshot {
  const CronSnapshot({
    required this.ok,
    required this.reason,
    required this.schedulerAlive,
    required this.jobs,
  });

  final bool ok;
  final String reason;
  final bool schedulerAlive;
  final List<CronJob> jobs;

  static const empty = CronSnapshot(
    ok: false,
    reason: 'unavailable',
    schedulerAlive: false,
    jobs: [],
  );
}

/// 立即运行的结果。
class CronRunResult {
  const CronRunResult({required this.ok, required this.error});

  final bool ok;
  final String error;
}

/// 启动调度线程（幂等；App 启动时调一次）。
void startCronScheduler() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<Void Function(), void Function()>('lunac_cron_start')();
  } catch (_) {
    // 旧桥没有这个符号：保持静默（列表会显示 schedulerAlive=false）。
  }
}

/// 前端忙态：流式对话开始 / 结束时调用（调度器看到忙就记 `skipped_busy`）。
void setCronBusy(bool busy) {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library
        .lookupFunction<Void Function(Int32), void Function(int)>(
          'lunac_cron_set_busy',
        )(busy ? 1 : 0);
  } catch (_) {
    // 忽略：旧桥没有该符号。
  }
}

/// 读列表（同步：只读两个小 JSON 文件）。
CronSnapshot readCronJobs() {
  final raw = _callNoArg('lunac_cron_list');
  if (raw == null) {
    return CronSnapshot.empty;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return CronSnapshot.empty;
    }
    final jobs = <CronJob>[];
    final list = value['jobs'];
    if (list is List) {
      for (final item in list) {
        if (item is Map<String, dynamic>) {
          jobs.add(CronJob.fromJson(item));
        }
      }
    }
    return CronSnapshot(
      ok: value['ok'] == true,
      reason: _str(value['reason']),
      schedulerAlive: value['schedulerAlive'] == true,
      jobs: jobs,
    );
  } catch (_) {
    return CronSnapshot.empty;
  }
}

/// 立即运行一条 job（阻塞到跑完 ⇒ 放 isolate，别卡 UI 线程）。
Future<CronRunResult> runCronJobNow(String id) {
  return Isolate.run(() {
    final raw = _callText('lunac_cron_run_now', id);
    if (raw == null) {
      return const CronRunResult(ok: false, error: '桥接不可用');
    }
    try {
      final value = jsonDecode(raw);
      if (value is! Map<String, dynamic>) {
        return const CronRunResult(ok: false, error: '返回格式异常');
      }
      return CronRunResult(
        ok: value['ok'] == true,
        error: _str(value['error']),
      );
    } catch (_) {
      return const CronRunResult(ok: false, error: '返回格式异常');
    }
  });
}

/// 逐 job 的「无人值守可写」授权（调用方负责二次确认）。
Future<CronRunResult> setCronWriteAccess(String id, bool allow) {
  return Isolate.run(() {
    final raw = _callJson('lunac_cron_set_write_access', {
      'id': id,
      'allow': allow,
    });
    if (raw == null) {
      return const CronRunResult(ok: false, error: '桥接不可用');
    }
    try {
      final value = jsonDecode(raw);
      if (value is! Map<String, dynamic>) {
        return const CronRunResult(ok: false, error: '返回格式异常');
      }
      return CronRunResult(ok: value['ok'] == true, error: _str(value['error']));
    } catch (_) {
      return const CronRunResult(ok: false, error: '返回格式异常');
    }
  });
}

/// 启用 / 停用一条 job（写 `cron.json` 的 `enabled`；重新启用会清掉自动停用）。
Future<CronRunResult> setCronEnabled(String id, bool enabled) {
  return Isolate.run(() {
    final raw = _callJson('lunac_cron_set_enabled', {
      'id': id,
      'enabled': enabled,
    });
    if (raw == null) {
      return const CronRunResult(ok: false, error: '桥接不可用');
    }
    try {
      final value = jsonDecode(raw);
      if (value is! Map<String, dynamic>) {
        return const CronRunResult(ok: false, error: '返回格式异常');
      }
      return CronRunResult(ok: value['ok'] == true, error: _str(value['error']));
    } catch (_) {
      return const CronRunResult(ok: false, error: '返回格式异常');
    }
  });
}

String _str(Object? value) => value is String ? value : '';

int _int(Object? value) => value is int ? value : 0;

typedef _TextInC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextInDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _NoArgC = Pointer<Utf8> Function();
typedef _NoArgDart = Pointer<Utf8> Function();
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

String? _callText(String symbol, String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextInC, _TextInDart>(symbol);
    input = text.toNativeUtf8();
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

String? _callJson(String symbol, Map<String, Object?> payload) {
  return _callText(symbol, jsonEncode(payload));
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
