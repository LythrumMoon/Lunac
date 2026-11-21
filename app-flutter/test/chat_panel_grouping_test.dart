import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/chat_model.dart';
import 'package:lunac/search/chat_panel.dart';

void noop() {}

Future<void> pumpPanel(WidgetTester tester) async {
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SizedBox(
          width: 720,
          height: 900,
          child: ChatPanel(onClose: noop, autoStart: false),
        ),
      ),
    ),
  );
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 50));
}

void main() {
  testWidgets('clicking the same task tab again collapses the drawer', (
    tester,
  ) async {
    chatSession.restore(
      id: 'test-tab-toggle',
      messages: const [ChatMessage('user', '随便问一句')],
      steps: const [
        {'kind': 'user', 'text': '随便问一句'},
      ],
    );
    // 任务条只在有内容时出现：给一条 TodoWrite，待办页才有可切换的面板。
    chatSession.apply({
      'type': 'assistant',
      'message': {
        'role': 'assistant',
        'content': [
          {
            'type': 'tool_use',
            'id': 'todo-test',
            'name': 'TodoWrite',
            'input': {
              'todos': [
                {'content': '测试待办', 'status': 'pending'},
              ],
            },
          },
        ],
      },
    });
    await pumpPanel(tester);

    // 任务抽屉标题也会用这条待办的名字，所以折叠态本来就有 1 份；
    // 展开后任务页再多一行，收起后回到 1 份。
    expect(find.text('测试待办'), findsOneWidget);
    await tester.tap(find.text('待办 0/1'));
    await tester.pump();
    expect(find.text('测试待办'), findsNWidgets(2));

    // 2026-11-21 用户口径：再点同一个页签应当收起，而不是保持展开。
    await tester.tap(find.text('待办 0/1'));
    await tester.pump();
    expect(find.text('测试待办'), findsOneWidget);
  });

  testWidgets('restored turns keep one task drawer per question', (
    tester,
  ) async {
    chatSession.restore(
      id: 'test-turn-drawers',
      messages: const [
        ChatMessage('user', '任务一'),
        ChatMessage('assistant', '中间输出'),
        ChatMessage('user', '任务二'),
        ChatMessage('assistant', '最终答案'),
      ],
      steps: const [
        {'kind': 'user', 'text': '任务一'},
        {'kind': 'assistant', 'text': '中间输出'},
        {'kind': 'user', 'text': '任务二'},
        {'kind': 'assistant', 'text': '最终答案'},
      ],
    );
    await pumpPanel(tester);

    // 四个提问 / 阶段在抽屉里各占一条：两个提问 → 两个任务抽屉。
    expect(find.byIcon(Icons.checklist_rtl), findsNWidgets(2));
    // 中间输出收进抽屉，不再漂在任务条上方；最终正文仍平铺可见。
    expect(find.text('中间输出'), findsNothing);
    expect(find.text('最终答案'), findsOneWidget);

    await tester.tap(find.byIcon(Icons.checklist_rtl).first);
    await tester.pump();
    expect(find.text('中间输出'), findsOneWidget);
  });

  testWidgets('a turn whose process was trimmed still keeps its drawer', (
    tester,
  ) async {
    // 历史体积上限只保留用户提问、丢掉早期过程 ⇒ 仍然要有任务抽屉，不能只剩 1 个。
    chatSession.restore(
      id: 'test-trimmed-turns',
      messages: const [
        ChatMessage('user', '第一个任务'),
        ChatMessage('user', '第二个任务'),
        ChatMessage('assistant', '最终答案'),
      ],
      steps: const [
        {'kind': 'user', 'text': '第一个任务'},
        {'kind': 'user', 'text': '第二个任务'},
        {'kind': 'assistant', 'text': '最终答案'},
      ],
    );
    await pumpPanel(tester);

    expect(find.byIcon(Icons.checklist_rtl), findsNWidgets(2));
    await tester.tap(find.byIcon(Icons.checklist_rtl).first);
    await tester.pump();
    expect(find.text('这一轮的过程记录已压缩，没有可展开的条目'), findsOneWidget);
  });
}
