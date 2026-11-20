// 平台薄层：用量日志 + 定价表（v1 `usage-cost.ts` / `storage.rs` 的 v2 对应物）。
//
// 用量日志：`<exe 根>\ModuleData\usage\usage-YYYY-MM-DD.jsonl`，**一行 = 一次提问**
// （含提问内所有工具往返）。由 `ChatPanel` 在 `result` 事件到达时追加 —— 与 v1
// `appendUsageLog` 同一时机，只追加不重写。
//
// 定价表：`config\pricing.json`（单位 = 元 / 百万 token）。价格不写进代码：各家单价差十倍、
// 官方还会调价，写死一个数字等于把错误金额当事实展示。候选价在
// `<沙箱盘>\lunac-pricing.pending.json`，用户在设置面板里预览并确认后才覆盖正式文件。
//
// 本文件只做「读写 + 日期工具」；金额计算在 `platform/usage_cost.dart`（价格表随时会改，
// 改完即时重算），逐日表格见对话窗底部的表盘（`search/usage_dashboard.dart`，M5z-补6）。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 汇总区间（天）：v1 `COST_RANGE_DAYS` 同值。
const int costRangeDays = 30;

/// 本地日期 `YYYY-MM-DD`（用量日志的文件名分片键）。
String localDateKey(DateTime date) {
  final month = date.month.toString().padLeft(2, '0');
  final day = date.day.toString().padLeft(2, '0');
  return '${date.year}-$month-$day';
}

/// 最近 `n` 天（含今天）的本地日期，**升序**。
List<String> lastNDates(int n, {DateTime? now}) {
  final base = now ?? DateTime.now();
  return [
    for (var i = n - 1; i >= 0; i--)
      localDateKey(base.subtract(Duration(days: i))),
  ];
}

/// 本地时区相对 UTC 的偏移（东八区 = 480），给 Rust 把 `ts` 换算成本地小时。
int localUtcOffsetMinutes() => DateTime.now().timeZoneOffset.inMinutes;

/// 某个模型在一天里某一个本地小时的合计。
class UsageHourTotals {
  const UsageHourTotals({
    required this.hour,
    required this.turns,
    required this.requests,
    required this.input,
    required this.output,
    required this.cacheRead,
    required this.cacheCreate,
  });

  final int hour;
  final int turns;
  final int requests;
  final int input;
  final int output;
  final int cacheRead;
  final int cacheCreate;

  static UsageHourTotals fromJson(Map<String, dynamic> json) => UsageHourTotals(
    hour: _int(json['hour']),
    turns: _int(json['turns']),
    requests: _int(json['requests']),
    input: _int(json['input']),
    output: _int(json['output']),
    cacheRead: _int(json['cacheRead']),
    cacheCreate: _int(json['cacheCreate']),
  );
}

/// 某个模型在一天里的合计（含来源归因：空串 = 普通聊天，`pet` = 桌宠）。
class UsageModelTotals {
  const UsageModelTotals({
    required this.model,
    required this.source,
    required this.turns,
    required this.requests,
    required this.input,
    required this.output,
    required this.cacheRead,
    required this.cacheCreate,
    required this.hours,
  });

  final String model;
  final String source;
  final int turns;
  final int requests;
  final int input;
  final int output;
  final int cacheRead;
  final int cacheCreate;
  final List<UsageHourTotals> hours;

  static UsageModelTotals fromJson(Map<String, dynamic> json) =>
      UsageModelTotals(
        model: json['model'] is String ? json['model'] as String : '',
        source: json['source'] is String ? json['source'] as String : '',
        turns: _int(json['turns']),
        requests: _int(json['requests']),
        input: _int(json['input']),
        output: _int(json['output']),
        cacheRead: _int(json['cacheRead']),
        cacheCreate: _int(json['cacheCreate']),
        hours: [
          if (json['hours'] is List)
            for (final hour in json['hours'] as List)
              if (hour is Map<String, dynamic>) UsageHourTotals.fromJson(hour),
        ],
      );
}

/// 一天的合计（成本面板「按天表格」的一行）。
class UsageDay {
  const UsageDay({
    required this.date,
    required this.turns,
    required this.requests,
    required this.input,
    required this.output,
    required this.cacheRead,
    required this.cacheCreate,
    required this.models,
  });

  final String date;
  final int turns;
  final int requests;
  final int input;
  final int output;
  final int cacheRead;
  final int cacheCreate;
  final List<UsageModelTotals> models;

  static UsageDay fromJson(Map<String, dynamic> json) => UsageDay(
    date: json['date'] is String ? json['date'] as String : '',
    turns: _int(json['turns']),
    requests: _int(json['requests']),
    input: _int(json['input']),
    output: _int(json['output']),
    cacheRead: _int(json['cacheRead']),
    cacheCreate: _int(json['cacheCreate']),
    models: [
      if (json['models'] is List)
        for (final model in json['models'] as List)
          if (model is Map<String, dynamic>) UsageModelTotals.fromJson(model),
    ],
  );
}

// ── FFI ───────────────────────────────────────────────────────────

typedef _JsonInC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _JsonInDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _NoArgC = Pointer<Utf8> Function();
typedef _NoArgDart = Pointer<Utf8> Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 调一个「入参 JSON / 出参 JSON」的 FFI；桥接缺失或抛错时返回 null。
String? _callJson(String symbol, Map<String, Object?> payload) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_JsonInC, _JsonInDart>(symbol);
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
    if (raw != null && raw != nullptr) {
      try {
        library
            .lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 调一个「入参纯字符串 / 出参 JSON」的 FFI；桥接缺失或抛错时返回 null。
String? _callText(String symbol, String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_JsonInC, _JsonInDart>(symbol);
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
    if (raw != null && raw != nullptr) {
      try {
        library
            .lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 调一个无参 JSON FFI。
String? _callNoArg(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_NoArgC, _NoArgDart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null && raw != nullptr) {
      try {
        library
            .lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 追加一条用量日志。返回 null = 成功，否则是一句原因（调用方通常忽略 —— 记账不该影响对话）。
///
/// `usage` 是 agent `result` 事件里的 `usage` 对象（字段名与 v1 原样一致）。
String? appendUsageRecord({
  required String model,
  required Map<String, dynamic> usage,
  String sessionId = '',
  String source = '',
  int? ts,
}) {
  final time = ts ?? DateTime.now().millisecondsSinceEpoch;
  final record = <String, Object?>{
    'ts': time,
    'model': model,
    if (sessionId.isNotEmpty) 'sessionId': sessionId,
    if (source.isNotEmpty) 'source': source,
    'input': _int(usage['input_tokens']),
    'output': _int(usage['output_tokens']),
    'cacheRead': _int(usage['cache_read_input_tokens']),
    'cacheCreate': _int(usage['cache_creation_input_tokens']),
    if (usage['requests'] is List) 'requests': usage['requests'],
    if (usage['subagent'] is Map) 'subagent': usage['subagent'],
    if (usage['review'] is Map) 'review': usage['review'],
    if (usage['partial'] == true) 'partial': true,
  };
  final raw = _callJson('lunac_usage_append', {
    'date': localDateKey(DateTime.fromMillisecondsSinceEpoch(time)),
    'record': record,
  });
  if (raw == null) {
    return '用量日志不可用（未加载 rust 桥接）';
  }
  final value = _decode(raw);
  if (value?['ok'] == true) {
    return null;
  }
  final reason = value?['error'];
  return reason is String && reason.isNotEmpty ? reason : '写入用量日志失败';
}

/// 读近 [days] 天用量并汇总（按天 → 按模型 → 按本地小时）。桥接缺失时返回空表。
List<UsageDay> readUsageRange({int days = costRangeDays}) {
  final raw = _callJson('lunac_usage_range', {
    'dates': lastNDates(days),
    'utcOffsetMinutes': localUtcOffsetMinutes(),
  });
  final value = raw == null ? null : _decodeList(raw);
  if (value == null) {
    return const [];
  }
  return [
    for (final day in value)
      if (day is Map<String, dynamic>) UsageDay.fromJson(day),
  ];
}

// ── 定价表 ────────────────────────────────────────────────────────

/// `get_pricing_state` 的对应物：正式表 / 候选表的路径与原文。
class PricingState {
  const PricingState({
    required this.path,
    required this.text,
    required this.pendingPath,
    required this.pendingText,
  });

  final String path;
  final String text;
  final String pendingPath;
  final String pendingText;

  bool get hasPending => pendingText.trim().isNotEmpty;

  static const PricingState empty = PricingState(
    path: '',
    text: '',
    pendingPath: '',
    pendingText: '',
  );
}

/// 读定价表状态；桥接缺失时返回 [PricingState.empty]。
PricingState readPricingState() {
  final raw = _callNoArg('lunac_pricing_state');
  final value = raw == null ? null : _decode(raw);
  if (value == null) {
    return PricingState.empty;
  }
  String field(String key) =>
      value[key] is String ? value[key] as String : '';
  return PricingState(
    path: field('path'),
    text: field('text'),
    pendingPath: field('pendingPath'),
    pendingText: field('pendingText'),
  );
}

/// 确保 `pricing.json` 存在（缺文件时落预置骨架），返回文件路径；失败返回 null。
String? ensurePricingFile() {
  final raw = _callNoArg('lunac_pricing_ensure');
  final value = raw == null ? null : _decode(raw);
  if (value == null || value['ok'] != true) {
    return null;
  }
  final path = value['path'];
  return path is String && path.isNotEmpty ? path : null;
}

/// 确认候选价格（先校验后覆盖）。返回 null = 成功，否则是一句原因。
String? commitPricingPending(String today) {
  final raw = _callText('lunac_pricing_commit', today);
  final value = raw == null ? null : _decode(raw);
  if (value == null) {
    return '定价表不可用（未加载 rust 桥接）';
  }
  if (value['ok'] == true) {
    return null;
  }
  final reason = value['error'];
  return reason is String && reason.isNotEmpty ? reason : '写入 pricing.json 失败';
}

/// 放弃候选价格。返回 null = 成功，否则是一句原因。
String? discardPricingPending() {
  final raw = _callNoArg('lunac_pricing_discard');
  final value = raw == null ? null : _decode(raw);
  if (value == null) {
    return '定价表不可用（未加载 rust 桥接）';
  }
  if (value['ok'] == true) {
    return null;
  }
  final reason = value['error'];
  return reason is String && reason.isNotEmpty ? reason : '删除候选价格失败';
}

/// 候选价格与正式表的差异预览（v1 `pricingPreviewRows` 同口径）。
///
/// 只列**变化的**：新模型整条列出四项；已有模型只列数值变化的那一项；
/// 时段价（`time_windows`）条数或内容变化单独一行；正式表里有、候选里没有的
/// 标「确认后失去价格」。语法坏 / 顶层不是对象时返回空表（面板照常显示按钮）。
List<({String model, String change})> pricingPreviewRows(
  String currentText,
  String pendingText,
) {
  final pending = _pricingModels(pendingText);
  if (pending == null) {
    return const [];
  }
  final current = _pricingModels(currentText) ?? const <String, Map<String, dynamic>>{};
  const fields = [
    ('input', '输入'),
    ('cache_read', '缓存命中'),
    ('cache_write', '缓存写入'),
    ('output', '输出'),
  ];
  final rows = <({String model, String change})>[];
  for (final entry in pending.entries) {
    final old = current[entry.key];
    final parts = <String>[];
    if (old == null) {
      for (final field in fields) {
        final value = _num(entry.value[field.$1]);
        if (value != null) {
          parts.add('${field.$2} ${_fmtPrice(value)}');
        }
      }
    } else {
      for (final field in fields) {
        final before = _num(old[field.$1]);
        final after = _num(entry.value[field.$1]);
        if (before == null || after == null || before == after) {
          continue;
        }
        parts.add('${field.$2} ${_fmtPrice(before)} → ${_fmtPrice(after)}');
      }
    }
    final oldWindows = old?['time_windows'];
    final newWindows = entry.value['time_windows'];
    final beforeCount = oldWindows is List ? oldWindows.length : 0;
    final afterCount = newWindows is List ? newWindows.length : 0;
    if (jsonEncode(oldWindows) != jsonEncode(newWindows)) {
      parts.add('时段价 $beforeCount → $afterCount 条');
    }
    final prefix = old == null ? '（新增）' : '';
    rows.add((
      model: '${entry.key.isEmpty ? '—' : entry.key}$prefix',
      change: parts.isEmpty ? '—' : parts.join(' · '),
    ));
  }
  for (final model in current.keys) {
    if (!pending.containsKey(model)) {
      rows.add((model: model.isEmpty ? '—' : model, change: '确认后失去价格'));
    }
  }
  return rows;
}

Map<String, Map<String, dynamic>>? _pricingModels(String text) {
  final trimmed = text.trim();
  if (trimmed.isEmpty) {
    return null;
  }
  try {
    final value = jsonDecode(trimmed);
    if (value is! Map<String, dynamic>) {
      return null;
    }
    final models = value['models'];
    if (models is! Map<String, dynamic>) {
      return const {};
    }
    return {
      for (final entry in models.entries)
        if (entry.value is Map<String, dynamic>) entry.key: entry.value as Map<String, dynamic>,
    };
  } catch (_) {
    return null;
  }
}

double? _num(Object? value) => value is num ? value.toDouble() : null;

/// 单价显示：整数不带小数点，其余保留原精度（价格表是「元 / 百万 token」）。
String _fmtPrice(double value) {
  if (value == value.roundToDouble()) {
    return value.toInt().toString();
  }
  return value.toString();
}

int _int(Object? value) => value is int ? value : 0;

Map<String, dynamic>? _decode(String raw) {
  try {
    final value = jsonDecode(raw);
    return value is Map<String, dynamic> ? value : null;
  } catch (_) {
    return null;
  }
}

List<dynamic>? _decodeList(String raw) {
  try {
    final value = jsonDecode(raw);
    return value is List ? value : null;
  } catch (_) {
    return null;
  }
}
