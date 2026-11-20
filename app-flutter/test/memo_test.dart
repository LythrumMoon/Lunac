// 平台薄层单测：备忘录 JSON → Dart 结构（不加载 DLL）。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/memo.dart';
import 'package:lunac/search/memo_panel.dart';
import 'package:lunac/theme.dart';

Widget _wrap(MemoPanel panel) => MaterialApp(
  theme: buildLunacTheme(),
  home: Scaffold(body: SizedBox(width: 900, height: 640, child: panel)),
);

void main() {
  test('full entry maps every field and round-trips to json', () {
    final entry = MemoEntry.fromJson(<String, dynamic>{
      'id': 'a',
      'text': '买牛奶',
      'tag': '购物',
      'images': <String>['D:/x/a.png'],
      'ts': 1700000000000,
    });
    expect(entry, isNotNull);
    expect(entry!.id, 'a');
    expect(entry.text, '买牛奶');
    expect(entry.tag, '购物');
    expect(entry.images.single, 'D:/x/a.png');
    expect(entry.ts, 1700000000000);

    final json = entry.toJson();
    expect(json['id'], 'a');
    expect(json['tag'], '购物');
    expect((json['images'] as List).single, 'D:/x/a.png');
  });

  test('minimal entry defaults tag, images and ts', () {
    final entry = MemoEntry.fromJson(<String, dynamic>{
      'id': 'b',
      'text': 'just text',
    });
    expect(entry, isNotNull);
    expect(entry!.tag, '');
    expect(entry.images, isEmpty);
    expect(entry.ts, 0);
  });

  test('entries without id or text are dropped', () {
    expect(MemoEntry.fromJson(<String, dynamic>{'text': 'no id'}), isNull);
    expect(MemoEntry.fromJson(<String, dynamic>{'id': '', 'text': 'x'}), isNull);
    expect(MemoEntry.fromJson(<String, dynamic>{'id': 'c'}), isNull);
  });

  testWidgets('editor shows saved images and removes one on tap', (tester) async {
    var entries = [
      MemoEntry(
        id: 'm1',
        text: '带图备忘',
        tag: '',
        images: ['D:/not/here/a.png'],
        ts: 1700000000000,
      ),
    ];
    await tester.pumpWidget(
      _wrap(
        MemoPanel(
          onClose: () {},
          initialMemoId: 'm1',
          loadMemos: () => entries,
          upsertMemo: (entry) {
            entries = [entry];
            return true;
          },
        ),
      ),
    );
    await tester.pump();

    expect(find.byType(Image), findsOneWidget, reason: 'v1 savedImages 图片条');
    expect(find.byTooltip('删除'), findsOneWidget);

    await tester.tap(find.byTooltip('删除'));
    await tester.pump();

    expect(entries.single.images, isEmpty, reason: '× 直接写盘摘掉这张图');
    expect(find.byType(Image), findsNothing);
    expect(entries.single.ts, 1700000000000, reason: '摘图不改时间戳');
  });

  testWidgets('ctrl+v attaches a clipboard image to the saved memo', (
    tester,
  ) async {
    var entries = [
      MemoEntry(id: 'm3', text: '粘贴图片', tag: '', images: const [], ts: 1),
    ];
    var pastedFor = '';
    await tester.pumpWidget(
      _wrap(
        MemoPanel(
          onClose: () {},
          initialMemoId: 'm3',
          loadMemos: () => entries,
          upsertMemo: (entry) {
            entries = [entry];
            return true;
          },
          pasteImage: (id) {
            pastedFor = id;
            return const MemoImagePaste(
              ok: true,
              path: r'D:\memo\images\m3\1.bmp',
            );
          },
        ),
      ),
    );
    await tester.pump();

    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();

    expect(pastedFor, 'm3', reason: '图片挂到当前正在编辑的条目上');
    expect(entries.single.images, [r'D:\memo\images\m3\1.bmp']);
    expect(entries.single.ts, 1, reason: '加图不改时间戳（与摘图同口径）');
  });

  testWidgets('history cards show image thumbnails', (tester) async {
    final entries = [
      MemoEntry(
        id: 'm2',
        text: '历史带图',
        tag: '',
        images: ['D:/not/here/b.png'],
        ts: 1700000000000,
      ),
    ];
    await tester.pumpWidget(
      _wrap(
        MemoPanel(
          onClose: () {},
          loadMemos: () => entries,
        ),
      ),
    );
    await tester.pump();

    expect(find.byType(Image), findsNothing, reason: '编辑器里没有新图');

    await tester.tap(find.text('历史记录'));
    await tester.pump();

    expect(find.byType(Image), findsOneWidget);
    expect(find.byTooltip('删除'), findsNothing, reason: '历史缩略图不带 ×');
  });
}
