// OCR 平台薄层与面板的单测（不加载 DLL、不真跑 PaddleOCR）。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/ocr.dart';
import 'package:lunac/search/ocr_panel.dart';
import 'package:lunac/search/search_bridge.dart';
import 'package:lunac/search/search_page.dart';
import 'package:lunac/search/search_result.dart';
import 'package:lunac/theme.dart';

class _FakeBackend extends SearchBackend {
  _FakeBackend(this.items);

  final List<SearchResult> items;
  final List<String> actions = [];
  SearchActionOutcome outcome = SearchActionOutcome.uiHandled;

  @override
  String? get unavailableReason => null;

  @override
  List<SearchResult> search(String query) => items;

  @override
  SearchActionOutcome runAction(String id) {
    actions.add(id);
    return outcome;
  }
}

Widget _wrap(SearchPage page) =>
    MaterialApp(theme: buildLunacTheme(), home: page);

Finder _row(String text) =>
    find.descendant(of: find.byType(ListView), matching: find.text(text));

void _useTallSurface(WidgetTester tester) {
  tester.view.physicalSize = const Size(1000, 900);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

void main() {
  test('decoded reply maps text and lines', () {
    final reply = decodeOcrReply(
      '{"text":"第一行\\nsecond","lines":["第一行","second"],"error":null}',
    );
    expect(reply.isError, isFalse);
    expect(reply.result!.text, '第一行\nsecond');
    expect(reply.result!.lines, ['第一行', 'second']);
  });

  test('error payload and broken json become readable failures', () {
    final error = decodeOcrReply(
      '{"text":"","lines":[],"error":"OCR 引擎未安装：请把 PaddleOCR-json 解压到 …"}',
    );
    expect(error.isError, isTrue);
    expect(error.error, contains('引擎未安装'));
    expect(error.result, isNull);

    expect(decodeOcrReply('{not json').error, '识别结果不是合法 JSON');
    expect(decodeOcrReply('[]').error, '识别结果格式不对');
    expect(decodeOcrReply('{}').error, '识别结果缺少必要字段');
    // lines 缺省 / 非字符串都被忽略，text 缺省时由 lines 合并。
    final minimal = decodeOcrReply('{"lines":["a",2]}');
    expect(minimal.result!.text, 'a');
    expect(minimal.result!.lines, ['a']);
  });

  test('status parsing tolerates missing fields and broken json', () {
    final status = parseOcrStatus(
      '{"installed":true,"enginePath":"C:\\\\Lunac\\\\Modules\\\\ocr\\\\paddle-ocr",'
      '"searchPaths":["C:\\\\A","C:\\\\B"]}',
    );
    expect(status.installed, isTrue);
    expect(status.enginePath, contains('paddle-ocr'));
    expect(status.searchPaths, hasLength(2));
    expect(status.error, isNull);

    final broken = parseOcrStatus('{not json');
    expect(broken.installed, isFalse);
    expect(broken.error, isNotNull);

    final noFlag = parseOcrStatus('{"searchPaths":[]}');
    expect(noFlag.installed, isFalse);
    expect(noFlag.searchPaths, isEmpty);
    expect(noFlag.error, isNotNull);
  });

  test('capture reply maps path and errors', () {
    final ok = decodeOcrCapture(
      r'{"path":"D:\\temp\\clipboard-1.bmp","error":""}',
    );
    expect(ok.isError, isFalse);
    expect(ok.path, r'D:\temp\clipboard-1.bmp');

    final cancelled = decodeOcrCapture(
      '{"path":"","error":"没有截到图片（已取消或超时）"}',
    );
    expect(cancelled.isError, isTrue);
    expect(cancelled.error, contains('没有截到图片'));

    expect(decodeOcrCapture('{not json').error, '截屏结果不是合法 JSON');
    expect(decodeOcrCapture('[]').error, '截屏结果格式不对');
  });

  testWidgets('panel uses the v1 two-column layout', (tester) async {
    tester.view.physicalSize = const Size(820, 470);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: SizedBox(
            width: 820,
            height: 470,
            child: OcrPanel(onClose: () {}),
          ),
        ),
      ),
    );
    await tester.pump();

    // 左半：空态预览 + 三个动作按钮（v1 `.ocr-image-actions`）。
    expect(find.text('暂无图片'), findsOneWidget);
    expect(find.text('截屏识别'), findsOneWidget);
    expect(find.text('识别剪贴板'), findsOneWidget);
    expect(find.text('选择文件'), findsOneWidget);
    // 右半：状态行 + 右上复制按钮 + 文本区占位。
    expect(find.text('复制结果'), findsOneWidget);
    expect(find.text('识别结果显示在这里…'), findsOneWidget);
  });

  testWidgets('OCR entry opens the 640 panel and Esc returns', (tester) async {
    _useTallSurface(tester);
    final backend = _FakeBackend(const [
      SearchResult(
        id: 'ocr',
        title: 'OCR 文字识别',
        subtitle: '图片文字识别（PaddleOCR · 离线高精度）',
        kind: 'command',
      ),
    ]);
    final heights = <double>[];
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          onResize: (height) async => heights.add(height),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(_row('OCR 文字识别'));
    await tester.pumpAndSettle();

    // 面板：640 固定档；单测没有 lunac_core.dll，如实显示「不可用」。
    expect(backend.actions, contains('ocr'));
    expect(heights.last, 640);
    expect(find.text('OCR 文字识别'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    expect(find.text('不可用（rust 桥接未加载）'), findsNothing);
    expect(heights.last, 126);
  });
}
