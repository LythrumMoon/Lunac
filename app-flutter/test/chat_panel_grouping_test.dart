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

    expect(find.text('测试待办'), findsNothing);
    await tester.tap(find.text('待办 0/1'));
    await tester.pump();
    expect(find.text('测试待办'), findsOneWidget);

    // 2026-11-21 用户口径：再点同一个页签应当收起，而不是保持展开。
    await tester.tap(find.text('待办 0/1'));
    await tester.pump();
    expect(find.text('测试待办'), findsNothing);
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

    // 四个提问 / 阶段各占一条：两个提问 → 两个任务条。
    expect(find.byIcon(Icons.checklist_rtl), findsNWidgets(2));
    // 中间输出是该节的小总结，固定显示在任务条下方；最终正文仍平铺可见。
    expect(find.text('中间输出'), findsOneWidget);
    expect(find.text('最终答案'), findsOneWidget);
    final summaryY = tester.getTopLeft(find.text('中间输出')).dy;
    final finalY = tester.getTopLeft(find.text('最终答案')).dy;
    expect(summaryY, lessThan(finalY), reason: '小总结应在大总结之前');
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

  testWidgets('preamble before the first TaskPhase merges into that bar', (
    tester,
  ) async {
    chatSession.restore(
      id: 'test-task-phase-preamble',
      messages: const [
        ChatMessage('user', '做一件事'),
        ChatMessage('assistant', '先说明一下。'),
        ChatMessage('assistant', '全部完成。'),
      ],
      steps: const [
        {'kind': 'user', 'text': '做一件事'},
        {'kind': 'assistant', 'text': '先说明一下。'},
        {'kind': 'taskPhaseStart', 'text': '第一步'},
        {'kind': 'taskPhaseDone', 'text': '第一步完成'},
        {'kind': 'assistant', 'text': '全部完成。'},
      ],
    );
    await pumpPanel(tester);

    expect(find.byIcon(Icons.checklist_rtl), findsOneWidget);
    expect(find.text('第一步'), findsOneWidget);
    expect(find.text('第一步完成'), findsOneWidget);
    expect(find.text('全部完成。'), findsOneWidget);
    // 前言属于第一个任务的过程块，不单独成条（折叠时不可见）。
    expect(find.text('先说明一下。'), findsNothing);

    await tester.tap(find.byIcon(Icons.checklist_rtl));
    await tester.pump();
    expect(find.text('先说明一下。'), findsOneWidget);
  });

  testWidgets('TaskPhase renders bar -> commands -> small summary in order', (
    tester,
  ) async {
    chatSession.restore(
      id: 'test-task-phase-order',
      messages: const [
        ChatMessage('user', '做两个步骤'),
        ChatMessage('assistant', '两个步骤都完成了。'),
      ],
      steps: const [
        {'kind': 'user', 'text': '做两个步骤'},
        {'kind': 'taskPhaseStart', 'text': '第一步'},
        {'kind': 'thinking', 'text': '先看文件'},
        {'kind': 'toolUse', 'name': 'Read', 'id': 'r1', 'detail': '{}'},
        {'kind': 'taskPhaseDone', 'text': '第一步完成'},
        {'kind': 'taskPhaseStart', 'text': '第二步'},
        {'kind': 'taskPhaseDone', 'text': '第二步完成'},
        {'kind': 'assistant', 'text': '两个步骤都完成了。'},
      ],
    );
    await pumpPanel(tester);

    final first = tester.getTopLeft(find.text('第一步')).dy;
    final summary1 = tester.getTopLeft(find.text('第一步完成')).dy;
    final second = tester.getTopLeft(find.text('第二步')).dy;
    final summary2 = tester.getTopLeft(find.text('第二步完成')).dy;
    final big = tester.getTopLeft(find.text('两个步骤都完成了。')).dy;
    expect(first, lessThan(summary1));
    expect(summary1, lessThan(second));
    expect(second, lessThan(summary2));
    expect(summary2, lessThan(big));
  });
}
