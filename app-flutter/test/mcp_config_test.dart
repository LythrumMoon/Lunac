// 平台薄层单测：工具列表 / 写入结果的容错解析（不加载 DLL）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/mcp_config.dart';

void main() {
  test('tool file read returns raw json text and readable failures', () {
    final ok = parseToolFileContent(
      '{"ok":true,"content":"{\\"name\\":\\"a\\"}","error":""}',
    );
    expect(ok.ok, isTrue);
    expect(ok.content, contains('"name"'));

    final failed = parseToolFileContent(
      '{"ok":false,"content":"","error":"读不了 nope.json：找不到文件"}',
    );
    expect(failed.ok, isFalse);
    expect(failed.error, contains('读不了'));

    expect(parseToolFileContent('{oops').ok, isFalse);
    expect(parseToolFileContent('[]').error, '读取返回格式不对');
  });

  test('parses entries, keeps invalid rows and defaults missing fields', () {
    const raw = '''
    {
      "entries": [
        {
          "filename": "alpha.json",
          "name": "alpha",
          "description": "desc",
          "command": "echo hi",
          "kind": "shell",
          "params": ["path"],
          "valid": true
        },
        {
          "filename": "broken.json",
          "name": "",
          "description": "",
          "command": "",
          "kind": "",
          "params": [],
          "valid": false
        }
      ],
      "error": ""
    }
    ''';
    final result = parseToolConfig(raw);
    expect(result.ok, isTrue);
    expect(result.entries, hasLength(2));
    expect(result.entries.first.filename, 'alpha.json');
    expect(result.entries.first.command, 'echo hi');
    expect(result.entries.first.params, ['path']);
    expect(result.entries.first.valid, isTrue);
    // 无效条目仍列出来；name 为空时标题退回文件名（v1 `tool.name || tool.filename`）。
    expect(result.entries.last.valid, isFalse);
    expect(result.entries.last.displayName, 'broken.json');
  });

  test('bad JSON and wrong shapes collapse to an empty list with a reason', () {
    final bad = parseToolConfig('{not json');
    expect(bad.entries, isEmpty);
    expect(bad.ok, isFalse);
    expect(bad.error, isNotEmpty);

    final wrong = parseToolConfig('[]');
    expect(wrong.entries, isEmpty);
    expect(wrong.ok, isFalse);
  });

  test('save result parses ok / error and tolerates garbage', () {
    final ok = parseSaveResult('{"ok":true,"error":""}');
    expect(ok.ok, isTrue);
    expect(ok.error, '');

    final failed = parseSaveResult('{"ok":false,"error":"写不进 D:/x"}');
    expect(failed.ok, isFalse);
    expect(failed.error, contains('写不进'));

    final garbage = parseSaveResult('oops');
    expect(garbage.ok, isFalse);
    expect(garbage.error, isNotEmpty);
  });

  test('file name follows v1 regex (non [A-Za-z0-9_-] becomes dash)', () {
    expect(toolFileName('my-tool'), 'my-tool');
    expect(toolFileName('a b/c'), 'a-b-c');
    expect(toolFileName('工具'), '--');
  });
}
