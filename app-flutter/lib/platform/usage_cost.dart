// 用量与成本的**算法层**（v1 `app/src/usage-cost.ts` 的 Dart 对应物）。
//
// 为什么单独一层：设置面板与主界面表盘都要算金额，算法只允许有一份（v1 同纪律）。
// 本文件纯 Dart、不碰 FFI 与 Widget —— 定价表的时段价 / 法定节假日 / 逐小时计价
// 都能直接用单测钉住。
//
// 三条 v1 纪律原样保留：
//  ① 价格不写进代码 —— 只读 `config\pricing.json`；
//  ② 「更新价格」交给 agent 写候选文件，面板只做确认；
//  ③ 金额在前端算（价格表随时会改，改完即时重算）。
import 'dart:convert';

import 'usage.dart';

// ── 定价表 ────────────────────────────────────────────────────────

/// 四类单价（元 / 百万 token）。
class Price {
  const Price({
    this.input = 0,
    this.cacheRead = 0,
    this.cacheWrite = 0,
    this.output = 0,
  });

  final double input;
  final double cacheRead;
  final double cacheWrite;
  final double output;
}

/// 一条「时段价」：本地时间 `[from, to)` 内用这一档价。`days` 省略 = 每天，
/// 给了就是 ISO 周几（1=周一 … 7=周日）。**列表里第一条命中的生效**（顺序即优先级）。
class PricingWindow {
  const PricingWindow({
    this.days,
    this.from = '',
    this.to = '',
    this.price = const Price(),
  });

  final List<int>? days;
  final String from;
  final String to;
  final Price price;
}

/// 一个模型的定价：基础四类价 + 可选时段价 + 该模型的法定节假日。
class PricingEntry {
  const PricingEntry({
    this.price = const Price(),
    this.windows = const [],
    this.holidays,
  });

  final Price price;
  final List<PricingWindow> windows;

  /// 该模型的法定节假日（`YYYY-MM-DD`）：命中 ⇒ 当天全天按基础价，时段价不适用。
  final List<String>? holidays;
}

/// `pricing.json` 的解析结果。
class PricingFile {
  const PricingFile({
    this.updatedAt = '',
    this.holidays = const [],
    this.models = const {},
  });

  final String updatedAt;

  /// 顶层法定节假日（模型级 `holidays` 优先）。
  final List<String> holidays;

  final Map<String, PricingEntry> models;

  bool get hasModels => models.isNotEmpty;
}

double _num(Object? value) => value is num ? value.toDouble() : 0;

List<int>? _days(Object? value) {
  if (value is! List) {
    return null;
  }
  final days = [
    for (final day in value)
      if (day is num) day.toInt(),
  ];
  return days.isEmpty ? null : days;
}

List<String> _strings(Object? value) {
  if (value is! List) {
    return const [];
  }
  return [
    for (final item in value)
      if (item is String) item,
  ];
}

Price _priceOfJson(Map<String, dynamic> json) => Price(
  input: _num(json['input']),
  cacheRead: _num(json['cache_read']),
  cacheWrite: _num(json['cache_write']),
  output: _num(json['output']),
);

/// 解析 `pricing.json` 原文。空 / 语法坏 / 顶层不是对象 ⇒ null（面板按「未定价」处理，
/// 不去猜）。
PricingFile? parsePricing(String text) {
  final trimmed = text.trim();
  if (trimmed.isEmpty) {
    return null;
  }
  Object? decoded;
  try {
    decoded = jsonDecode(trimmed);
  } catch (_) {
    return null;
  }
  if (decoded is! Map) {
    return null;
  }
  final root = Map<String, dynamic>.from(decoded);
  final models = <String, PricingEntry>{};
  final rawModels = root['models'];
  if (rawModels is Map) {
    for (final entry in rawModels.entries) {
      final value = entry.value;
      if (value is! Map) {
        continue;
      }
      final json = Map<String, dynamic>.from(value);
      final windows = <PricingWindow>[];
      final rawWindows = json['time_windows'];
      if (rawWindows is List) {
        for (final window in rawWindows) {
          if (window is! Map) {
            continue;
          }
          final w = Map<String, dynamic>.from(window);
          windows.add(
            PricingWindow(
              days: _days(w['days']),
              from: w['from'] is String ? w['from'] as String : '',
              to: w['to'] is String ? w['to'] as String : '',
              price: _priceOfJson(w),
            ),
          );
        }
      }
      models['${entry.key}'] = PricingEntry(
        price: _priceOfJson(json),
        windows: windows,
        holidays: json.containsKey('holidays') ? _strings(json['holidays']) : null,
      );
    }
  }
  return PricingFile(
    updatedAt: root['updated_at'] is String ? root['updated_at'] as String : '',
    holidays: _strings(root['holidays']),
    models: models,
  );
}

/// 取某模型的定价；没有就是「未定价」（**不许当 0**）。
PricingEntry? priceOf(PricingFile? pricing, String model) {
  final entry = pricing?.models[model];
  return entry;
}

/// `YYYY-MM-DD` 的 ISO 周几（1=周一 … 7=周日）。解析不出来返回 0（不命中任何带 `days`
/// 的时段 ⇒ 退回基础价）。
int weekdayOf(String date) {
  final match = RegExp(r'^(\d{4})-(\d{2})-(\d{2})$').firstMatch(date);
  if (match == null) {
    return 0;
  }
  final year = int.tryParse(match.group(1)!);
  final month = int.tryParse(match.group(2)!);
  final day = int.tryParse(match.group(3)!);
  if (year == null || month == null || day == null) {
    return 0;
  }
  final local = DateTime(year, month, day);
  return local.weekday;
}

/// `HH:MM` → 「零点起的分钟数」；写法不合法返回 null（**只认两位小时**，与 v1 同口径：
/// 宽松解析会把用户的意思读成另一个时刻）。
int? hhmmToMinutes(String? text) {
  if (text == null) {
    return null;
  }
  final match = RegExp(r'^(\d{2}):(\d{2})$').firstMatch(text);
  if (match == null) {
    return null;
  }
  final hour = int.tryParse(match.group(1)!);
  final minute = int.tryParse(match.group(2)!);
  if (hour == null || minute == null || hour > 23 || minute > 59) {
    return null;
  }
  return hour * 60 + minute;
}

/// 这一天是不是价格表认定的法定节假日（模型级优先，回落顶层）。
bool isHoliday(PricingFile? pricing, String model, String date) {
  final list = priceOf(pricing, model)?.holidays ?? pricing?.holidays ?? const [];
  return list.contains(date);
}

/// 该模型在「本地日期 + 本地小时」这一格实际生效的价：基础价是缺省，
/// `time_windows` 里**第一条命中**的条目覆盖它；法定节假日全天不适用时段窗。
/// 未定价返回 null（调用方据此标「未定价」）。
Price? priceAt(PricingFile? pricing, String model, String date, int hour) {
  final base = priceOf(pricing, model);
  if (base == null) {
    return null;
  }
  if (base.windows.isEmpty || isHoliday(pricing, model, date)) {
    return base.price;
  }
  final weekday = weekdayOf(date);
  final minutes = hour * 60;
  for (final window in base.windows) {
    final from = hhmmToMinutes(window.from);
    final to = hhmmToMinutes(window.to);
    if (from == null || to == null || from >= to) {
      continue;
    }
    if (minutes < from || minutes >= to) {
      continue;
    }
    final days = window.days;
    if (days != null && !days.contains(weekday)) {
      continue;
    }
    return window.price;
  }
  return base.price;
}

/// 一天的金额。**必须逐模型算**（一天里换过模型时单价差十倍）。
/// `exact=false` 表示这天有模型没价格，金额只是已知部分。
///
/// 有 `hours` 桶就逐桶计价（分时价要求「这一批 token 发生在几点」）；没有才退回总量法。
({double amount, bool exact}) dayCost(UsageDay day, PricingFile? pricing) {
  var amount = 0.0;
  var exact = true;
  for (final model in day.models) {
    if (priceOf(pricing, model.model) == null) {
      exact = false;
      continue;
    }
    if (model.hours.isNotEmpty) {
      for (final hour in model.hours) {
        final price = priceAt(pricing, model.model, day.date, hour.hour);
        if (price == null) {
          exact = false;
          continue;
        }
        amount += (hour.input * price.input +
                hour.cacheRead * price.cacheRead +
                hour.cacheCreate * price.cacheWrite +
                hour.output * price.output) /
            1e6;
      }
      continue;
    }
    final price = priceOf(pricing, model.model)!.price;
    amount += (model.input * price.input +
            model.cacheRead * price.cacheRead +
            model.cacheCreate * price.cacheWrite +
            model.output * price.output) /
        1e6;
  }
  return (amount: amount, exact: exact);
}

// ── 显示格式 ──────────────────────────────────────────────────────

/// 金额显示：单价是「元 / 百万 token」，单次提问常常只有几厘 —— 太小就多给两位小数。
String fmtMoney(double value) {
  if (!(value > 0)) {
    return '0.00';
  }
  return value < 0.01 ? value.toStringAsFixed(4) : value.toStringAsFixed(2);
}

/// token 数显示（表格里用 k / M 缩写）。
String fmtTokenCount(int n) {
  if (n >= 1000000) {
    return '${(n / 1000000).toStringAsFixed(2)}M';
  }
  if (n >= 1000) {
    return '${(n / 1000).toStringAsFixed(1)}k';
  }
  return '$n';
}

/// 主界面表盘的紧凑显示（1.2k / 987）。
String fmtDashboardTokens(int n) {
  if (n >= 1000000) {
    return '${(n / 1000000).toStringAsFixed(1)}M';
  }
  if (n >= 1000) {
    return '${(n / 1000).toStringAsFixed(1)}k';
  }
  return '$n';
}

/// 模型名显示：早期日志没有模型名 ⇒ `—`（空单元格看起来像渲染坏了）。
String modelLabel(String model) => model.isEmpty ? '—' : model;

// ── 范围 / 模型 / 来源筛选 ────────────────────────────────────────

enum UsageRangeKind { today, days, date }

/// 面板的时间范围：**单日 ⇒ 表格按小时；区间 ⇒ 按天**。
class UsageRange {
  const UsageRange({required this.kind, this.days = 0, this.date = ''});

  const UsageRange.today() : kind = UsageRangeKind.today, days = 0, date = '';

  final UsageRangeKind kind;
  final int days;
  final String date;

  /// 下拉框 value ↔ 范围（一进一出成对写在一处，别两头各拼一套）。
  String get value => switch (kind) {
    UsageRangeKind.today => 'today',
    UsageRangeKind.days => 'days:$days',
    UsageRangeKind.date => 'date:$date',
  };

  static UsageRange parse(String value) {
    if (value == 'today') {
      return const UsageRange.today();
    }
    final days = RegExp(r'^days:(\d+)$').firstMatch(value);
    if (days != null) {
      return UsageRange(
        kind: UsageRangeKind.days,
        days: int.parse(days.group(1)!),
      );
    }
    final date = RegExp(r'^date:(\d{4}-\d{2}-\d{2})$').firstMatch(value);
    if (date != null) {
      return UsageRange(kind: UsageRangeKind.date, date: date.group(1)!);
    }
    return const UsageRange.today();
  }
}

const UsageRange defaultUsageRange = UsageRange.today();

/// 记录里的来源 → 面板分区键：`pet` / `cron` 各单列一档，其余（含空串）算普通聊天。
String usageSourcePartition(String source) {
  if (source == 'pet') {
    return 'pet';
  }
  if (source == 'cron') {
    return 'cron';
  }
  return 'chat';
}

/// 面板要用的那份数据（价格表状态 + 近 N 天用量）。
class UsageCostData {
  const UsageCostData({
    this.days = const [],
    this.pricing,
    this.pending,
    this.state,
  });

  final List<UsageDay> days;
  final PricingFile? pricing;
  final PricingFile? pending;
  final PricingState? state;

  bool get hasPrices => pricing?.hasModels ?? false;
}

/// 取面板数据：任何一步失败都退回「没有它」，绝不让金额面板整个渲染不出来。
UsageCostData loadUsageCost() {
  final state = readPricingState();
  final days = readUsageRange();
  return UsageCostData(
    days: days,
    pricing: parsePricing(state.text),
    pending: parsePricing(state.pendingText),
    state: state,
  );
}

/// 这批数据里出现过的模型名（去重、升序）。**取自未裁剪的那份**，否则
/// 「选了某一天 → 别的模型从选项里消失 → 切不回去」。
List<String> usageModels(UsageCostData data) {
  final set = <String>{};
  for (final day in data.days) {
    for (final model in day.models) {
      set.add(model.model);
    }
  }
  final models = set.toList()..sort();
  return models;
}

UsageDay? _keepModels(UsageDay day, String? model, String? source) {
  final models = [
    for (final bucket in day.models)
      if ((model == null || bucket.model == model) &&
          (source == null || usageSourcePartition(bucket.source) == source))
        bucket,
  ];
  if (models.isEmpty) {
    return null;
  }
  var turns = 0;
  var requests = 0;
  var input = 0;
  var output = 0;
  var cacheRead = 0;
  var cacheCreate = 0;
  for (final bucket in models) {
    turns += bucket.turns;
    requests += bucket.requests;
    input += bucket.input;
    output += bucket.output;
    cacheRead += bucket.cacheRead;
    cacheCreate += bucket.cacheCreate;
  }
  return UsageDay(
    date: day.date,
    turns: turns,
    requests: requests,
    input: input,
    output: output,
    cacheRead: cacheRead,
    cacheCreate: cacheCreate,
    models: models,
  );
}

/// 把读回来的那份裁到所选范围 + 所选模型 + 所选来源 —— 面板的头（未定价）与表格
/// 必须看同一段，否则会出现「表格里没有的量却把模型列进未定价」。
UsageCostData scopedUsageData(
  UsageCostData data,
  UsageRange range, {
  String? model,
  String? source,
}) {
  final Set<String> keep;
  switch (range.kind) {
    case UsageRangeKind.today:
      keep = {localDateKey(DateTime.now())};
    case UsageRangeKind.date:
      keep = {range.date};
    case UsageRangeKind.days:
      keep = lastNDates(range.days).toSet();
  }
  final days = <UsageDay>[];
  for (final day in data.days) {
    if (!keep.contains(day.date)) {
      continue;
    }
    if (model == null && source == null) {
      days.add(day);
      continue;
    }
    final kept = _keepModels(day, model, source);
    if (kept != null) {
      days.add(kept);
    }
  }
  return UsageCostData(
    days: days,
    pricing: data.pricing,
    pending: data.pending,
    state: data.state,
  );
}

/// 表格一行的形状（按天或按小时，形状一样）。
class CostRow {
  const CostRow({
    required this.label,
    this.requests = 0,
    this.hit = 0,
    this.miss = 0,
    this.write = 0,
    this.output = 0,
    this.amount = 0,
    this.exact = true,
  });

  final String label;
  final int requests;

  /// 输入（命中缓存）。
  final int hit;

  /// 输入（未命中缓存）。
  final int miss;

  /// 缓存写入。
  final int write;

  final int output;
  final double amount;
  final bool exact;
}

/// 区间合计（逐日累加 + 逐模型查价）。
class UsageTotals {
  const UsageTotals({
    this.requests = 0,
    this.hit = 0,
    this.miss = 0,
    this.write = 0,
    this.output = 0,
    this.amount = 0,
    this.exact = true,
    this.unpriced = const {},
  });

  final int requests;
  final int hit;
  final int miss;
  final int write;
  final int output;
  final double amount;
  final bool exact;
  final Set<String> unpriced;
}

UsageTotals sumUsageCost(UsageCostData data) {
  var requests = 0;
  var hit = 0;
  var miss = 0;
  var write = 0;
  var output = 0;
  var amount = 0.0;
  var exact = true;
  final unpriced = <String>{};
  for (final day in data.days) {
    requests += day.requests;
    hit += day.cacheRead;
    miss += day.input;
    write += day.cacheCreate;
    output += day.output;
    final cost = dayCost(day, data.pricing);
    amount += cost.amount;
    if (!cost.exact) {
      exact = false;
    }
    for (final model in day.models) {
      if (priceOf(data.pricing, model.model) == null) {
        unpriced.add(model.model);
      }
    }
  }
  return UsageTotals(
    requests: requests,
    hit: hit,
    miss: miss,
    write: write,
    output: output,
    amount: amount,
    exact: exact,
    unpriced: unpriced,
  );
}

/// 按天：一行一天。**新的在上**（看用量几乎总是先看最近几天）。
List<CostRow> dayRows(UsageCostData data) {
  final rows = <CostRow>[];
  for (final day in data.days) {
    final cost = dayCost(day, data.pricing);
    rows.add(
      CostRow(
        label: day.date,
        requests: day.requests,
        hit: day.cacheRead,
        miss: day.input,
        write: day.cacheCreate,
        output: day.output,
        amount: cost.amount,
        exact: cost.exact,
      ),
    );
  }
  return rows.reversed.toList();
}

/// 按小时：一行一个小时（只列有量的那几个小时），一天之内按时间正序。
///
/// **金额必须逐模型逐桶算** —— 同一小时里不同模型单价不同，把一天的金额按比例摊到
/// 各小时上是错的。所以这里是「模型 × 小时」的汇总，与 [dayCost] 同源（都过 [priceAt]）。
List<CostRow> hourRows(UsageDay day, PricingFile? pricing) {
  final byHour = <int, CostRow>{};
  for (final model in day.models) {
    for (final hour in model.hours) {
      final old = byHour[hour.hour];
      final price = priceAt(pricing, model.model, day.date, hour.hour);
      final amount = price == null
          ? 0.0
          : (hour.input * price.input +
                  hour.cacheRead * price.cacheRead +
                  hour.cacheCreate * price.cacheWrite +
                  hour.output * price.output) /
              1e6;
      byHour[hour.hour] = CostRow(
        label: '${hour.hour.toString().padLeft(2, '0')}:00',
        requests: (old?.requests ?? 0) + hour.requests,
        hit: (old?.hit ?? 0) + hour.cacheRead,
        miss: (old?.miss ?? 0) + hour.input,
        write: (old?.write ?? 0) + hour.cacheCreate,
        output: (old?.output ?? 0) + hour.output,
        amount: (old?.amount ?? 0) + amount,
        exact: (old?.exact ?? true) && price != null,
      );
    }
  }
  final hours = byHour.keys.toList()..sort();
  return [for (final hour in hours) byHour[hour]!];
}

/// 表格行：单日按小时、区间按天。
List<CostRow> costRows(UsageCostData data, UsageRange range) {
  if (range.kind == UsageRangeKind.days) {
    return dayRows(data);
  }
  final date = range.kind == UsageRangeKind.today
      ? localDateKey(DateTime.now())
      : range.date;
  for (final day in data.days) {
    if (day.date == date) {
      return hourRows(day, data.pricing);
    }
  }
  return const [];
}

/// 金额单元格：没定价为 `—`；不精确加 `≥ ` 前缀（如实标出「只是已知部分」）。
String amountText(bool hasPrices, double amount, bool exact) {
  if (!hasPrices) {
    return '—';
  }
  return '${exact ? '' : '≥ '}¥${fmtMoney(amount)}';
}

/// 派生列：每请求的未命中 / 输出（请求数为 0 时给 `—`，不是 0）。
String perRequestText(int total, int requests) {
  if (requests <= 0) {
    return '—';
  }
  return fmtTokenCount((total / requests).round());
}

/// 派生列：这一行金额占本次统计总额的比例。没定价 / 总额为 0 时 `—`（不编 0%）。
String shareText(bool hasPrices, double amount, double totalAmount) {
  if (!hasPrices || !(totalAmount > 0)) {
    return '—';
  }
  return '${((amount / totalAmount) * 100).toStringAsFixed(1)}%';
}

/// 未定价模型的名字（空模型名单独给一个可读标签，**不能**渲染成 `—`：
/// 那一格会变成「未定价：—」，读起来正好相反）。
List<String> unpricedLabels(Set<String> unpriced) {
  final labels = [
    for (final model in unpriced) model.isEmpty ? '未知模型' : model,
  ]..sort();
  return labels;
}
