import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/usage_dashboard.dart';

void main() {
  testWidgets('usage panel opens as an independent overlay', (tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          body: Center(
            child: UsageDashboard(
              session: UsageSessionStats(hit: 80, miss: 20, output: 40),
            ),
          ),
        ),
      ),
    );

    expect(find.text('用量与成本'), findsOneWidget);

    await tester.tap(find.text('用量与成本'));
    await tester.pumpAndSettle();

    // 面板内容出现在浮层里，且对话区不会被撑高（浮层挂在 Overlay 上）。
    expect(find.text('价格表'), findsOneWidget);
    expect(find.byType(OverlayPortal), findsWidgets);
  });
}
