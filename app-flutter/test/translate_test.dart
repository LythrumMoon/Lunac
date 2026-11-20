// 平台薄层单测：翻译 JSON → Dart 结构（不加载 DLL、不发网络请求）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/translate.dart';
import 'package:lunac/search/translate_panel.dart';

void main() {
  test('full result maps every v1 field', () {
    final reply = decodeTranslateReply('''
      {
        "text": "hello",
        "from": "en",
        "to": "zh-CN",
        "source": "dict",
        "cached": true,
        "translation": "你好",
        "phonetic": "英 həˈləʊ  美 həˈloʊ",
        "explains": ["int. 喂，你好"],
        "examples": [
          {"src": "Hello, world.", "dst": "你好，世界。"}
        ]
      }
    ''');
    expect(reply.isError, isFalse);
    final result = reply.result!;
    expect(result.text, 'hello');
    expect(result.from, 'en');
    expect(result.to, 'zh-CN');
    expect(result.source, 'dict');
    expect(result.cached, isTrue);
    expect(result.translation, '你好');
    expect(result.phonetic, contains('həˈləʊ'));
    expect(result.explains.single, 'int. 喂，你好');
    expect(result.examples.single.src, 'Hello, world.');
    expect(result.examples.single.dst, '你好，世界。');
  });

  test('minimal none result defaults optional fields', () {
    final reply = decodeTranslateReply('''
      {"text": "zzzz", "from": "en", "to": "zh-CN", "source": "none"}
    ''');
    final result = reply.result!;
    expect(result.translation, '');
    expect(result.phonetic, '');
    expect(result.cached, isFalse);
    expect(result.explains, isEmpty);
    expect(result.examples, isEmpty);
  });

  test('error payload and bad json become readable failures', () {
    final error = decodeTranslateReply('{"error": "还没配置 AI，请先到设置里填好"}');
    expect(error.isError, isTrue);
    expect(error.error, '还没配置 AI，请先到设置里填好');
    expect(error.result, isNull);

    expect(decodeTranslateReply('{not json').error, '翻译结果不是合法 JSON');
    expect(decodeTranslateReply('[]').error, '翻译结果格式不对');
    expect(
      decodeTranslateReply('{"from": "en"}').error,
      '翻译结果缺少必要字段',
    );
  });

  test('stripTranslateInvocation only peels the leading command word', () {
    expect(stripTranslateInvocation('翻译 hello'), 'hello');
    expect(stripTranslateInvocation('  翻译  '), '');
    expect(stripTranslateInvocation('translate good morning'), 'good morning');
    expect(stripTranslateInvocation('translator'), 'translator');
    expect(stripTranslateInvocation('查词 hello'), 'hello');
    expect(stripTranslateInvocation('查词典 你好'), '查词典 你好');
    expect(stripTranslateInvocation('hello world'), 'hello world');
  });
}
