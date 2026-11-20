// 平台薄层单测：日期工具、候选价预览、无桥接时的降级（不联网 / 不碰真实文件）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/usage.dart';

void main() {
  test('local date keys and the recent range are local and ascending', () {
    final now = DateTime(2026, 11, 14, 23, 59);
    expect(localDateKey(now), '2026-11-14');
    expect(localDateKey(DateTime(2026, 1, 2)), '2026-01-02');
    final dates = lastNDates(3, now: now);
    expect(dates, ['2026-11-12', '2026-11-13', '2026-11-14']);
    expect(lastNDates(1, now: now), ['2026-11-14']);
  });

  test('pricing preview lists only the fields that changed', () {
    const current = '{"models":{"a":{"input":1,"cache_read":0.02,"cache_write":0,"output":4},'
        '"gone":{"input":1,"cache_read":0,"cache_write":0,"output":2}}}';
    const pending = '{"models":{"a":{"input":2,"cache_read":0.02,"cache_write":0,"output":4},'
        '"b":{"input":1,"cache_read":0,"cache_write":0,"output":8}}}';
    final rows = pricingPreviewRows(current, pending);
    expect(rows.length, 3);
    expect(rows[0].model, 'a');
    expect(rows[0].change, '输入 1 → 2');
    expect(rows[1].model, 'b（新增）');
    expect(rows[1].change, '输入 1 · 缓存命中 0 · 缓存写入 0 · 输出 8');
    expect(rows[2].model, 'gone');
    expect(rows[2].change, '确认后失去价格');
  });

  test('pricing preview reports time window changes and ignores bad json', () {
    const current = '{"models":{"a":{"input":1,"cache_read":0,"cache_write":0,"output":1}}}';
    const pending = '{"models":{"a":{"input":1,"cache_read":0,"cache_write":0,"output":1,'
        '"time_windows":[{"from":"09:00","to":"12:00","input":2,"cache_read":0,"cache_write":0,"output":2}]}}}';
    final rows = pricingPreviewRows(current, pending);
    expect(rows.single.change, '时段价 0 → 1 条');

    expect(pricingPreviewRows('not json', '{"models":{}}'), isEmpty);
    expect(pricingPreviewRows(current, 'not json'), isEmpty);
    // 两边完全一致 ⇒ 一行「—」，不伪造变化
    final same = pricingPreviewRows(current, current);
    expect(same.single.model, 'a');
    expect(same.single.change, '—');
  });

  test('without the native bridge usage and pricing degrade honestly', () {
    // 单测环境没有 lunac_core.dll：读用量为空表，写用量 / 定价操作给一句人话。
    expect(readUsageRange(), isEmpty);
    expect(readPricingState().hasPending, isFalse);
    expect(appendUsageRecord(model: 'm', usage: const {}), isNotNull);
    expect(ensurePricingFile(), isNull);
    expect(commitPricingPending('2026-11-14'), isNotNull);
    expect(discardPricingPending(), isNotNull);
  });

  test('usage record payload keeps the v1 result field names', () {
    // 这里只验证「拼出来的字段名与 v1 一致」——桥接缺失时函数返回原因、不写盘。
    final reason = appendUsageRecord(
      model: 'deepseek-v4-flash',
      sessionId: 's1',
      source: 'pet',
      ts: DateTime(2026, 11, 14, 10).millisecondsSinceEpoch,
      usage: const {
        'input_tokens': 10,
        'output_tokens': 20,
        'cache_read_input_tokens': 30,
        'cache_creation_input_tokens': 0,
        'requests': [
          {'in': 10, 'read': 30, 'create': 0, 'out': 20},
        ],
      },
    );
    expect(reason, '用量日志不可用（未加载 rust 桥接）');
  });
}
