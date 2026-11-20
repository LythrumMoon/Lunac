// M2 协议报文形状测试（与 v1 `app/src/main.ts` 的 startAIChat /
// respondPermission 逐字段对齐；不碰 FFI）。
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/agent.dart';
import 'package:lunac/platform/paths.dart';

void main() {
  test('terminal key data matches xterm default key bindings', () {
    expect(
      terminalKeyData(key: LogicalKeyboardKey.enter, character: '\n'),
      '\r',
    );
    expect(
      terminalKeyData(key: LogicalKeyboardKey.numpadEnter),
      '\r',
    );
    expect(terminalKeyData(key: LogicalKeyboardKey.backspace), '\b');
    expect(terminalKeyData(key: LogicalKeyboardKey.tab), '\t');
    expect(terminalKeyData(key: LogicalKeyboardKey.escape), '\u001b');
    expect(terminalKeyData(key: LogicalKeyboardKey.arrowUp), '\u001b[A');
    expect(terminalKeyData(key: LogicalKeyboardKey.arrowDown), '\u001b[B');
    expect(terminalKeyData(key: LogicalKeyboardKey.arrowRight), '\u001b[C');
    expect(terminalKeyData(key: LogicalKeyboardKey.arrowLeft), '\u001b[D');
    expect(terminalKeyData(key: LogicalKeyboardKey.home), '\u001b[H');
    expect(terminalKeyData(key: LogicalKeyboardKey.end), '\u001b[F');
    expect(terminalKeyData(key: LogicalKeyboardKey.delete), '\u001b[3~');

    // 可打印字符原样；控制字符 / 空串 / 无字符一律不发。
    expect(
      terminalKeyData(key: LogicalKeyboardKey.keyA, character: 'a'),
      'a',
    );
    expect(
      terminalKeyData(key: LogicalKeyboardKey.keyA, character: '\u0001'),
      isNull,
    );
    expect(terminalKeyData(key: LogicalKeyboardKey.keyA, character: ''), isNull);
    expect(terminalKeyData(key: LogicalKeyboardKey.keyA), isNull);
  });

  test('user message payload carries attachments inside the text block', () {
    final payload = userMessagePayload(
      '帮我看看',
      attachments: const [r'C:\docs\a.pdf', r'C:\pics\b.png'],
    );

    expect(payload['type'], 'user');
    expect(payload['session_id'], '');
    expect(payload['parent_tool_use_id'], isNull);

    final message = payload['message'] as Map;
    expect(message['role'], 'user');
    final content = (message['content'] as List).cast<Map>();
    // 第一块永远是文本（图片附件另起 image 块，见下面的专门测试）。
    final text = content.first['text'] as String;
    expect(content.first['type'], 'text');
    expect(text, contains('[Attached files]'));
    expect(text, contains(r'- C:\docs\a.pdf'));
    expect(text, contains(r'- C:\pics\b.png'));
    expect(text, contains('[User query]\n帮我看看'));
  });

  test('user message without attachments is just the text', () {
    final payload = userMessagePayload('hi');
    final content = ((payload['message'] as Map)['content'] as List).single as Map;
    expect(content['text'], 'hi');
  });

  test('steer=insert only rides on the payload when asked for it', () {
    // 默认 = 排队：**不能**带 `steer` 键，否则 agent 会在轮首插进去（语义变了）。
    expect(userMessagePayload('排队').containsKey('steer'), isFalse);
    // 立即插入：agent 侧只认 `steer == "insert"` 这一个值（见 `drain_steering`）。
    expect(userMessagePayload('插入', steerInsert: true)['steer'], 'insert');
    // 其余字段（type / message）不受影响：同一条通道，只是多一个键。
    final payload = userMessagePayload('插入', steerInsert: true);
    expect(payload['type'], 'user');
    expect(((payload['message'] as Map)['content'] as List), hasLength(1));
  });

  test('image attachments add image blocks when vision is on', () {
    final payload = userMessagePayload(
      '看图',
      attachments: const [r'C:\a\shot.PNG', r'C:\a\doc.pdf'],
      vision: true,
    );
    final content =
        ((payload['message'] as Map)['content'] as List).cast<Map>();
    // 文本块 + 仅图片的一张图块（PDF 不进 image 块）。
    expect(content.length, 2);
    expect(content.first['type'], 'text');
    expect(content[1]['type'], 'image');
    expect((content[1]['source'] as Map)['type'], 'file');
    expect((content[1]['source'] as Map)['path'], r'C:\a\shot.PNG');
  });

  test('vision off keeps images as text paths only', () {
    final payload = userMessagePayload(
      '看图',
      attachments: const [r'C:\a\shot.png'],
      vision: false,
    );
    final content =
        ((payload['message'] as Map)['content'] as List).cast<Map>();
    expect(content.length, 1);
    expect(content.first['type'], 'text');
    expect(content.first['text'], contains(r'C:\a\shot.png'));
  });

  test('isImagePath follows the clipboard extension set', () {
    expect(isImagePath(r'C:\a\b.JPEG'), isTrue);
    expect(isImagePath(r'C:\a\b.webp'), isTrue);
    expect(isImagePath(r'C:\a\b.pdf'), isFalse);
    expect(isImagePath('no-extension'), isFalse);
  });

  test('allow response matches the control_response shape', () {
    final payload = approvalResponse('req1', 'toolu_1', allow: true);
    expect(payload['type'], 'control_response');
    final response = payload['response'] as Map;
    expect(response['subtype'], 'success');
    expect(response['request_id'], 'req1');
    final inner = response['response'] as Map;
    expect(inner['behavior'], 'allow');
    expect(inner['updatedInput'], isEmpty);
    expect(inner['toolUseID'], 'toolu_1');
  });

  test('deny response carries a message and interrupt=false', () {
    final payload = approvalResponse('req2', 'toolu_2', allow: false);
    final inner =
        ((payload['response'] as Map)['response'] as Map);
    expect(inner['behavior'], 'deny');
    expect(inner['interrupt'], isFalse);
    expect(inner['toolUseID'], 'toolu_2');
    expect(inner['message'], contains('denied'));
  });

  test('deny with interrupt pauses the whole turn', () {
    final payload = approvalResponse(
      'req8',
      'toolu_8',
      allow: false,
      message: '用户拒绝并暂停了对话',
      interrupt: true,
    );
    final inner = ((payload['response'] as Map)['response'] as Map);
    expect(inner['behavior'], 'deny');
    expect(inner['interrupt'], isTrue);
    expect(inner['message'], '用户拒绝并暂停了对话');
  });

  test('updatedInput flows through for elicitation and always-trust', () {
    final trust = approvalResponse(
      'req3',
      '',
      allow: true,
      updatedInput: const {'remember': true},
    );
    expect(
      ((trust['response'] as Map)['response'] as Map)['updatedInput'],
      {'remember': true},
    );

    final elicit = approvalResponse(
      'req4',
      '',
      allow: true,
      updatedInput: const {'format': 'csv', 'rows': 10, 'zip': false},
    );
    expect(
      ((elicit['response'] as Map)['response'] as Map)['updatedInput'],
      {'format': 'csv', 'rows': 10, 'zip': false},
    );
  });

  test('set_history payload matches the v1 restore shape', () {
    final payload = setHistoryPayload(const [
      {'role': 'user', 'content': '旧问题'},
      {'role': 'assistant', 'content': '旧回答'},
    ]);
    expect(payload['type'], 'set_history');
    expect((payload['messages'] as List).length, 2);
    expect((payload['messages'] as List).first, {
      'role': 'user',
      'content': '旧问题',
    });
  });

  test('stop sends a pause line', () {
    expect(pausePayload()['type'], 'pause');
  });

  test('tool_control targets one running command by tool_use_id', () {
    // v1 `sendToolControl` 的形状：转后台 / 停止都不带 data；stdin 才带。
    expect(toolControlPayload('toolu_1', 'background'), {
      'type': 'tool_control',
      'action': 'background',
      'tool_use_id': 'toolu_1',
    });
    expect(toolControlPayload('toolu_2', 'stop'), {
      'type': 'tool_control',
      'action': 'stop',
      'tool_use_id': 'toolu_2',
    });
    final stdin = toolControlPayload('toolu_3', 'stdin', data: 'ls\r');
    expect(stdin['data'], 'ls\r');
  });

  test('compact sends a raw compact line, not a user message', () {
    expect(compactPayload(), {'type': 'compact'});
  });
}
