// 对话正文轻量 markdown 的解析与渲染测试（不加载 DLL）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/chat_markdown.dart';
import 'package:lunac/theme.dart';

Widget _wrap(String text, {void Function(String url)? onOpenUrl}) =>
    MaterialApp(
      theme: buildLunacTheme(),
      home: Scaffold(
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(12),
          child: ChatMarkdownView(text: text, onOpenUrl: onOpenUrl),
        ),
      ),
    );

void main() {
  test('inline parser strips markers and finds local paths', () {
    final tokens = parseChatInline(
      r'**重点** 与 `code` 看 D:\Lunac\temp\transStorage\a.glb。',
    );
    expect(
      tokens.any((t) => t.kind == ChatInlineKind.bold && t.text == '重点'),
      isTrue,
    );
    expect(
      tokens.any((t) => t.kind == ChatInlineKind.code && t.text == 'code'),
      isTrue,
    );
    final path = tokens.where((t) => t.kind == ChatInlineKind.path).single;
    // 句末中文句号要剥掉，路径本身保持原样。
    expect(path.text, r'D:\Lunac\temp\transStorage\a.glb');

    // 反引号包住的整条路径也是路径泡泡（模型习惯这么写）。
    final quoted = parseChatInline(r'`D:\Lunac\temp\transStorage\b.glb`');
    expect(quoted.single.kind, ChatInlineKind.path);
    expect(quoted.single.text, r'D:\Lunac\temp\transStorage\b.glb');

    // http(s) 不是本机路径，而是可点击的 URL；未闭合的 ** 原样保留。
    final url = parseChatInline('https://api.example.com/v1/chat/completions')
        .single;
    expect(url.kind, ChatInlineKind.url);
    expect(url.text, 'https://api.example.com/v1/chat/completions');
    expect(
      parseChatInline('https://example.com/a')
          .every((t) => t.kind != ChatInlineKind.path),
      isTrue,
    );
    expect(parseChatInline('**没收尾').map((t) => t.text).join(), '**没收尾');
  });

  test('block parser handles headings, lists, quotes, code and tables', () {
    final blocks = parseChatBlocks(
      '# 标题\n\n'
      '第一段\n第二行\n\n'
      '- 苹果\n- 香蕉\n\n'
      '1. 第一步\n2. 第二步\n\n'
      '> 引用一句\n\n'
      '```dart\nfinal a = 1;\n```\n\n'
      '| 部件 | 距离 |\n'
      '|---|---|\n'
      '| 眉 | 0.15 |\n'
      '| 唇 | 0.16 |\n\n'
      '---\n',
    );
    expect(blocks.map((b) => b.kind), [
      ChatBlockKind.heading,
      ChatBlockKind.paragraph,
      ChatBlockKind.bullet,
      ChatBlockKind.ordered,
      ChatBlockKind.quote,
      ChatBlockKind.code,
      ChatBlockKind.table,
      ChatBlockKind.divider,
    ]);
    expect(blocks[0].level, 1);
    expect(blocks[0].text, '标题');
    expect(blocks[1].text, '第一段\n第二行');
    expect(blocks[2].items, ['苹果', '香蕉']);
    expect(blocks[3].items, ['第一步', '第二步']);
    expect(blocks[4].text, '引用一句');
    expect(blocks[5].text, 'final a = 1;');
    expect(blocks[6].table!.header, ['部件', '距离']);
    expect(blocks[6].table!.rows, [
      ['眉', '0.15'],
      ['唇', '0.16'],
    ]);
  });

  testWidgets('renders bold without asterisks, tables and path chips', (
    tester,
  ) async {
    await tester.pumpWidget(
      _wrap(
        '**没做成的部分**\n\n'
        '| 部件 | 距离 |\n'
        '|---|---|\n'
        '| 眉 | 0.15 |\n\n'
        r'文件在 D:\Lunac\temp\transStorage\npc-catalog\evelyn\a.glb 里。',
      ),
    );
    await tester.pump();

    // `**` 不再出现在界面上。
    expect(find.textContaining('**', findRichText: true), findsNothing);
    expect(find.textContaining('没做成的部分', findRichText: true), findsWidgets);
    // 表格按表格画，竖线不再是正文。
    expect(find.byType(Table), findsOneWidget);
    expect(find.textContaining('| 部件 |', findRichText: true), findsNothing);

    // 本机路径变成可点击的小泡泡（Tooltip 带完整路径）。
    expect(
      find.byTooltip(r'打开 D:\Lunac\temp\transStorage\npc-catalog\evelyn\a.glb'),
      findsOneWidget,
    );
  });

  testWidgets('clicking a URL hands it to the preview callback', (
    tester,
  ) async {
    String? opened;
    await tester.pumpWidget(
      _wrap(
        'POST https://api.example.com/v1/chat/completions 返回 401',
        onOpenUrl: (url) => opened = url,
      ),
    );
    await tester.pump();
    await tester.tap(
      find.textContaining(
        'https://api.example.com/v1/chat/completions',
        findRichText: true,
      ),
    );
    await tester.pump();
    expect(opened, 'https://api.example.com/v1/chat/completions');
  });
}
