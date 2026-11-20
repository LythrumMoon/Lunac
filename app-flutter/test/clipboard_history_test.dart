// 平台薄层单测：剪贴板记录的 JSON 映射与预览规则（不加载 DLL）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/clipboard_history.dart';

void main() {
  test('text entry keeps the full text and reports its length', () {
    final entry = ClipboardEntry.fromJson(<String, dynamic>{
      'clip_type': 'text',
      'text': 'hello world',
      'file_paths': <String>[],
      'time': 1700000000000,
    });
    expect(entry, isNotNull);
    expect(entry!.isFile, isFalse);
    expect(entry.text, 'hello world');
    expect(entry.preview, 'hello world');
    expect(entry.sizeHint, '11 字符');
    expect(entry.time, 1700000000000);
  });

  test('file entry previews the basename and has no size hint', () {
    final entry = ClipboardEntry.fromJson(<String, dynamic>{
      'clip_type': 'file',
      'text': r'D:\Docs\report.pdf',
      'file_paths': <String>[r'D:\Docs\report.pdf'],
      'time': 1,
    });
    expect(entry, isNotNull);
    expect(entry!.isFile, isTrue);
    expect(entry.preview, 'report.pdf');
    expect(entry.sizeHint, '');
    expect(entry.filePaths.single, r'D:\Docs\report.pdf');
  });

  test('preview collapses newlines and truncates long text', () {
    final entry = ClipboardEntry(
      clipType: 'text',
      text: '${'a' * 90}\nsecond line',
      filePaths: const [],
      time: 0,
    );
    final preview = entry.preview;
    expect(preview.endsWith('…'), isTrue);
    // 80 个字符 + 省略号。
    expect(preview.length, 81);
    expect(preview.contains('\n'), isFalse);

    final multiline = ClipboardEntry(
      clipType: 'text',
      text: 'line1\nline2',
      filePaths: const [],
      time: 0,
    );
    expect(multiline.preview, 'line1 line2');
  });

  test('entries without usable text are dropped', () {
    expect(ClipboardEntry.fromJson(<String, dynamic>{'time': 1}), isNull);
    expect(
      ClipboardEntry.fromJson(<String, dynamic>{'text': '', 'time': 1}),
      isNull,
    );
  });
}
