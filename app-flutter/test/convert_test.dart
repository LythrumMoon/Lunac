// 平台薄层单测：文件转换的 JSON → Dart 结构（不加载 DLL、不启动 ffmpeg）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/convert.dart';
import 'package:lunac/search/convert_panel.dart';
import 'package:lunac/theme.dart';

void main() {
  test('plan json maps kind targets and engine state', () {
    final plan = decodeConvertPlan('''
      {
        "kind": "video",
        "targets": ["webm", "mp4", "mp3"],
        "error": "",
        "engine": "ffmpeg",
        "engineFound": true
      }
    ''');
    expect(plan, isNotNull);
    expect(plan!.kind, 'video');
    expect(plan.targets, ['webm', 'mp4', 'mp3']);
    expect(plan.error, isEmpty);
    expect(plan.engine, 'ffmpeg');
    expect(plan.engineFound, isTrue);
    expect(plan.canConvert, isTrue);
  });

  test('unsupported plan keeps the reason and disables conversion', () {
    final plan = decodeConvertPlan(
      '{"kind":"unknown","targets":[],"error":"这个后缀不支持转换",'
      '"engine":"","engineFound":false}',
    );
    expect(plan, isNotNull);
    expect(plan!.kind, 'unknown');
    expect(plan.targets, isEmpty);
    expect(plan.error, '这个后缀不支持转换');
    expect(plan.canConvert, isFalse);
    expect(decodeConvertPlan('{not json'), isNull);
    expect(decodeConvertPlan('[]'), isNull);
  });

  test('run reply maps success and engine errors', () {
    final ok = decodeConvertRunReply(
      r'''{"ok":true,"outputPath":"D:\\out\\a.webm","error":""}''',
    );
    expect(ok.isError, isFalse);
    expect(ok.outputPath, r'D:\out\a.webm');

    final failed = decodeConvertRunReply(
      '{"ok":false,"outputPath":"","error":"未找到 ffmpeg，请先安装"}',
    );
    expect(failed.isError, isTrue);
    expect(failed.error, '未找到 ffmpeg，请先安装');

    expect(decodeConvertRunReply('{not json').error, '转换结果不是合法 JSON');
    expect(decodeConvertRunReply('[]').error, '转换结果格式不对');
  });

  test('progress json maps running percent and unknown duration', () {
    final determinate = ConvertProgress.decode(
      '{"running":true,"percent":42.5,"totalMs":10000,"cancelled":false}',
    );
    expect(determinate, isNotNull);
    expect(determinate!.running, isTrue);
    expect(determinate.percent, 42.5);
    expect(determinate.determinate, isTrue);
    expect(determinate.cancelled, isFalse);

    // ffprobe 不在 ⇒ percent = -1，面板退回不确定态。
    final unknown = ConvertProgress.decode(
      '{"running":true,"percent":-1,"totalMs":0,"cancelled":false}',
    );
    expect(unknown!.determinate, isFalse);

    // 空闲 / 坏 JSON。
    expect(ConvertProgress.decode('{"running":false}')!.determinate, isFalse);
    expect(ConvertProgress.decode('{not json'), isNull);
  });

  testWidgets('panel lays out at the v1 820x470 size without overflowing', (
    tester,
  ) async {
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
            child: ConvertPanel(onClose: () {}),
          ),
        ),
      ),
    );
    await tester.pump();
    // v1 版式：左页签 + 输入/输出两列。
    expect(find.text('点击选择文件'), findsOneWidget);
    expect(find.text('转换结果会出现在这里'), findsOneWidget);
    expect(find.text('PDF 转换'), findsOneWidget);
  });
}
