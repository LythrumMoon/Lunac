// M1 骨架冒烟：窗口根组件能渲染出搜索页，桥接层在没有动态库时优雅降级。
//
// 这里不断言具体结果条目（数据来自 Rust 目录），只断言「壳能起来 + 桥接不抛异常」。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:lunac/main.dart';
import 'package:lunac/platform/agent_status.dart';

void main() {
  testWidgets('shell renders the search page', (WidgetTester tester) async {
    await tester.pumpWidget(const LunacApp());
    await tester.pumpAndSettle();

    expect(find.byType(TextField), findsOneWidget);
    expect(find.text('搜索应用、文件与命令'), findsOneWidget);
  });

  testWidgets('agent status bridge degrades without a native library',
      (tester) async {
    // 直接调用薄层：无库时返回说明文本，而不是抛异常。
    expect(readAgentStatus(), startsWith('agent:'));
  });
}
