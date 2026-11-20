// 平台薄层单测：剪贴板负载 JSON 的解析与容错（不加载 DLL）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/clipboard_payload.dart';

void main() {
  test('files payload keeps every non-empty path', () {
    final payload = parseClipboardPayload(
      r'{"kind":"files","paths":["D:\\Docs\\a.pdf","C:\\Pics\\b.png"]}',
    );
    expect(payload.kind, ClipboardPayloadKind.files);
    expect(payload.paths, [r'D:\Docs\a.pdf', r'C:\Pics\b.png']);
    expect(payload.text, isEmpty);
  });

  test('image payload carries the dumped bmp path', () {
    final payload = parseClipboardPayload(
      r'{"kind":"image","path":"D:\\app\\temp\\clipboard-1.bmp"}',
    );
    expect(payload.kind, ClipboardPayloadKind.image);
    expect(payload.paths, [r'D:\app\temp\clipboard-1.bmp']);
  });

  test('text payload keeps the text as-is', () {
    final payload = parseClipboardPayload(
      '{"kind":"text","text":"hello world"}',
    );
    expect(payload.kind, ClipboardPayloadKind.text);
    expect(payload.text, 'hello world');
    expect(payload.paths, isEmpty);
  });

  test('empty payload is reported as empty', () {
    final payload = parseClipboardPayload('{"kind":"empty"}');
    expect(payload.kind, ClipboardPayloadKind.empty);
    expect(payload.isEmpty, isTrue);
  });

  test('bad json / unknown kind / wrong field types degrade to empty', () {
    const inputs = [
      'not json',
      '[]',
      '{}',
      '{"kind":"nope"}',
      '{"kind":"files"}',
      '{"kind":"files","paths":"D:\\\\a.txt"}',
      '{"kind":"files","paths":[]}',
      '{"kind":"files","paths":[1,null,""]}',
      '{"kind":"image","path":""}',
      '{"kind":"image"}',
      '{"kind":"text"}',
      '{"kind":"text","text":42}',
      '{"kind":"text","text":""}',
    ];
    for (final input in inputs) {
      final payload = parseClipboardPayload(input);
      expect(
        payload.kind,
        ClipboardPayloadKind.empty,
        reason: 'should degrade to empty: $input',
      );
    }
  });

  test('files payload ignores blank entries but keeps the rest', () {
    final payload = parseClipboardPayload(
      '{"kind":"files","paths":["","D:\\\\docs\\\\a.pdf"]}',
    );
    expect(payload.kind, ClipboardPayloadKind.files);
    expect(payload.paths, [r'D:\docs\a.pdf']);
  });
}
