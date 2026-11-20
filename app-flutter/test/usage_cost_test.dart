// 用量与成本算法层的纯 Dart 测试（v1 `usage-cost.ts` 的逐条口径）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/usage.dart';
import 'package:lunac/platform/usage_cost.dart';

UsageHourTotals hour(
  int h, {
  int input = 0,
  int output = 0,
  int read = 0,
  int create = 0,
}) => UsageHourTotals(
  hour: h,
  turns: 1,
  requests: 1,
  input: input,
  output: output,
  cacheRead: read,
  cacheCreate: create,
);

UsageModelTotals bucket(
  String model,
  String source, {
  int input = 0,
  int output = 0,
  int read = 0,
  int create = 0,
  List<UsageHourTotals> hours = const [],
}) => UsageModelTotals(
  model: model,
  source: source,
  turns: 1,
  requests: 1,
  input: input,
  output: output,
  cacheRead: read,
  cacheCreate: create,
  hours: hours,
);

UsageDay day(String date, List<UsageModelTotals> models) => UsageDay(
  date: date,
  turns: models.length,
  requests: models.length,
  input: models.fold(0, (sum, m) => sum + m.input),
  output: models.fold(0, (sum, m) => sum + m.output),
  cacheRead: models.fold(0, (sum, m) => sum + m.cacheRead),
  cacheCreate: models.fold(0, (sum, m) => sum + m.cacheCreate),
  models: models,
);

const pricingJson = '''
{
  "updated_at": "2026-10-01",
  "holidays": ["2026-10-01"],
  "models": {
    "cheap": {
      "input": 1,
      "cache_read": 0.1,
      "cache_write": 2,
      "output": 4,
      "time_windows": [
        {"from": "09:00", "to": "12:00", "input": 10, "cache_read": 1, "cache_write": 20, "output": 40},
        {"days": [6, 7], "from": "00:00", "to": "23:59", "input": 5, "cache_read": 0.5, "cache_write": 10, "output": 20}
      ]
    }
  }
}
''';

void main() {
  test('parsePricing keeps windows / holidays / updated_at', () {
    final pricing = parsePricing(pricingJson);
    expect(pricing, isNotNull);
    expect(pricing!.updatedAt, '2026-10-01');
    expect(pricing.holidays, ['2026-10-01']);
    final entry = priceOf(pricing, 'cheap');
    expect(entry, isNotNull);
    expect(entry!.price.input, 1);
    expect(entry.windows.length, 2);
    expect(entry.windows.first.from, '09:00');
    expect(entry.windows[1].days, [6, 7]);
  });

  test('parsePricing rejects empty or broken text', () {
    expect(parsePricing(''), isNull);
    expect(parsePricing('{oops'), isNull);
    expect(parsePricing('[]'), isNull);
  });

  test('priceAt applies the first matching window and weekday filter', () {
    final pricing = parsePricing(pricingJson)!;
    // 2026-11-16 是周一、2026-11-14 是周六。
    expect(priceAt(pricing, 'cheap', '2026-11-16', 10)!.input, 10);
    expect(priceAt(pricing, 'cheap', '2026-11-16', 8)!.input, 1);
    expect(priceAt(pricing, 'cheap', '2026-11-14', 8)!.input, 5);
    // 命中第一条（09:00-12:00）优先于周末档：顺序即优先级。
    expect(priceAt(pricing, 'cheap', '2026-11-14', 10)!.input, 10);
  });

  test('priceAt falls back to the base price on a holiday', () {
    final pricing = parsePricing(pricingJson)!;
    // 2026-10-01 是节假日（周四）：时段价一律不适用。
    expect(priceAt(pricing, 'cheap', '2026-10-01', 10)!.input, 1);
  });

  test('priceAt returns null for an unpriced model', () {
    expect(priceAt(parsePricing(pricingJson), 'unknown', '2026-11-16', 10), isNull);
  });

  test('dayCost prices hour buckets one by one', () {
    final pricing = parsePricing(pricingJson)!;
    final data = day('2026-11-16', [
      bucket(
        'cheap',
        '',
        input: 2_000_000,
        hours: [hour(10, input: 2_000_000)],
      ),
    ]);
    final cost = dayCost(data, pricing);
    expect(cost.exact, isTrue);
    // 200 万 token × 10 元 / 百万 = 20 元（高峰价），不是基础价 2 元。
    expect(cost.amount, closeTo(20, 1e-9));
  });

  test('dayCost marks unpriced days as inexact instead of zero', () {
    final pricing = parsePricing(pricingJson)!;
    final data = day('2026-11-16', [
      bucket('unknown', '', input: 1_000_000),
    ]);
    final cost = dayCost(data, pricing);
    expect(cost.exact, isFalse);
    expect(cost.amount, 0);
  });

  test('scopedUsageData recomputes day totals after filtering', () {
    final data = UsageCostData(
      days: [
        day('2026-11-15', [
          bucket('a', '', input: 10, output: 1),
          bucket('b', 'pet', input: 20, output: 2),
        ]),
      ],
      pricing: parsePricing(pricingJson),
    );
    final scoped = scopedUsageData(
      data,
      const UsageRange(kind: UsageRangeKind.date, date: '2026-11-15'),
      source: 'pet',
    );
    expect(scoped.days.length, 1);
    expect(scoped.days.single.models.single.model, 'b');
    expect(scoped.days.single.input, 20);
    expect(scoped.days.single.output, 2);
  });

  test('costRows uses hours for a single day and days for a range', () {
    final today = localDateKey(DateTime.now());
    final data = UsageCostData(
      days: [
        day('2026-11-14', [bucket('cheap', '', input: 1)]),
        day(today, [
          bucket('cheap', '', input: 1, hours: [hour(11, input: 1), hour(9, input: 1)]),
        ]),
      ],
    );
    final hourly = costRows(
      data,
      const UsageRange(kind: UsageRangeKind.today),
    );
    expect(hourly.map((row) => row.label), ['09:00', '11:00']);

    final daily = costRows(
      data,
      const UsageRange(kind: UsageRangeKind.days, days: 30),
    );
    expect(daily.first.label, today);
    expect(daily.last.label, '2026-11-14');
  });

  test('sumUsageCost adds unpriced models and totals', () {
    final data = UsageCostData(
      days: [
        day('2026-11-16', [
          bucket('cheap', '', input: 1_000_000, output: 1_000_000),
          bucket('unknown', '', input: 5),
        ]),
      ],
      pricing: parsePricing(pricingJson),
    );
    final totals = sumUsageCost(data);
    expect(totals.miss, 1_000_005);
    expect(totals.output, 1_000_000);
    expect(totals.exact, isFalse);
    expect(totals.unpriced, {'unknown'});
    // 基础价：输入 1 + 输出 4 = 5 元；未定价模型不编 0，只标记不精确。
    expect(totals.amount, closeTo(5, 1e-9));
  });

  test('money / token / share formatting matches v1', () {
    expect(fmtMoney(0), '0.00');
    expect(fmtMoney(0.0042), '0.0042');
    expect(fmtMoney(1.234), '1.23');
    expect(fmtTokenCount(999), '999');
    expect(fmtTokenCount(1500), '1.5k');
    expect(fmtTokenCount(2_500_000), '2.50M');
    expect(fmtDashboardTokens(1500), '1.5k');
    expect(amountText(false, 0, true), '—');
    expect(amountText(true, 1.2, true), '¥1.20');
    expect(amountText(true, 1.2, false), '≥ ¥1.20');
    expect(perRequestText(100, 0), '—');
    expect(perRequestText(1500, 3), '500');
    expect(shareText(true, 25, 100), '25.0%');
    expect(shareText(false, 25, 100), '—');
    expect(unpricedLabels({'', 'm'}), ['m', '未知模型']);
  });
}
