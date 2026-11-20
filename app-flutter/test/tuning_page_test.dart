// 调音页的最小回归：**桥接不可用时不假装成功**（页面上必须出现原因，而不是一片空白）。
//
// 为什么不测「拖一根推子会发 IPC」：这一页的所有数据都来自宿主（链 / 曲线 / 预设），
// 测试进程里没有 `lunac_core.dll` ⇒ 那条路只能靠真机验收（见 docs/acceptance-map.md）。
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/tuning_page.dart';

void main() {
  testWidgets('调音页在没有桥接时如实报错', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: TuningPage(onClose: () {})),
      ),
    );
    await tester.pump();

    expect(find.text('调音'), findsOneWidget);
    // `readTuning()` 在测试进程里拿不到库 ⇒ 错误必须是「一句人话」，不是空页。
    // （正文与底部状态行会各出现一次，所以用 findsWidgets。）
    expect(find.text('桥接不可用'), findsWidgets);

    // 卸掉这一页（`initState` 起了 1.2s 轮询，不换成别的东西会留一个 pending timer）。
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  test('曲线窗的缩放窗口按对数轴计算并夹在两端', () {
    // 20Hz–20kHz 的对数轴（log10）。
    final lo = math.log(20) / math.ln10;
    final hi = math.log(20000) / math.ln10;
    final full = hi - lo;

    // 不缩放 = 整条轴。
    final wide = zoomWindow(zoom: 1, center: 0.5, lo: lo, hi: hi);
    expect(wide.lo, closeTo(lo, 1e-9));
    expect(wide.hi, closeTo(hi, 1e-9));

    // 4× 居中：窗口宽度是整条轴的 1/4，中点仍在正中。
    final zoomed = zoomWindow(zoom: 4, center: 0.5, lo: lo, hi: hi);
    expect(zoomed.hi - zoomed.lo, closeTo(full / 4, 1e-9));
    expect((zoomed.lo + zoomed.hi) / 2, closeTo((lo + hi) / 2, 1e-9));

    // 靠边时按半宽夹住，不会露出轴外（滚轮一直往同一个方向推就会到这两个端点）。
    final left = zoomWindow(zoom: 4, center: 0, lo: lo, hi: hi);
    expect(left.lo, closeTo(lo, 1e-9));
    final right = zoomWindow(zoom: 4, center: 1, lo: lo, hi: hi);
    expect(right.hi, closeTo(hi, 1e-9));

    // 非法缩放（0 / 负数）按 1× 处理，不产生 NaN / 反向窗口。
    final broken = zoomWindow(zoom: 0, center: 0.5, lo: lo, hi: hi);
    expect(broken.lo.isFinite && broken.hi.isFinite, isTrue);
    expect(broken.hi - broken.lo, closeTo(full, 1e-9));
  });
}
