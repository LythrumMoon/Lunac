// 「命令添加」curl 解析的纯函数单测（不加载 DLL、不弹窗）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/curl_import.dart';

void main() {
  test('parses the common OpenAI-style curl into provider fields', () {
    const raw = r'''
curl https://api.openai.com/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sk-abc123" \
  -d '{"model":"gpt-4o","messages":[]}'
''';
    final result = parseCurlConfig(raw);
    expect(result.ok, isTrue);
    final parsed = result.parsed!;
    // base_url 只取 origin（v1 同口径），业务路径单独带出来提示用户。
    expect(parsed.baseUrl, 'https://api.openai.com');
    expect(parsed.path, '/v1/chat/completions');
    expect(parsed.provider, 'api.openai.com');
    expect(parsed.apiKey, 'sk-abc123');
    expect(parsed.envKey, isEmpty);
    expect(parsed.model, 'gpt-4o');
    expect(parsed.hasKey, isTrue);
  });

  test('env-var bearer is not treated as the key', () {
    const raw =
        r'''curl https://api.deepseek.com/chat/completions -H "Authorization: Bearer $DEEPSEEK_API_KEY" -d '{"model":"deepseek-chat"}' ''';
    final parsed = parseCurlConfig(raw).parsed!;
    expect(parsed.apiKey, isEmpty);
    expect(parsed.envKey, r'$DEEPSEEK_API_KEY');
    expect(parsed.hasKey, isTrue);
    expect(parsed.model, 'deepseek-chat');
  });

  test('missing url is the only hard failure', () {
    final result = parseCurlConfig('curl -H "Authorization: Bearer x"');
    expect(result.ok, isFalse);
    expect(result.error, contains('没找到 URL'));

    final empty = parseCurlConfig('');
    expect(empty.ok, isFalse);
  });

  test('quotes, www and ports are normalised', () {
    final parsed = parseCurlConfig(
      "'https://www.example.com:8443/v1/chat?x=1' -H 'Authorization: Bearer k-1'",
    ).parsed!;
    expect(parsed.baseUrl, 'https://www.example.com:8443');
    expect(parsed.provider, 'example.com');
    expect(parsed.path, '/v1/chat?x=1');
    expect(parsed.apiKey, 'k-1');
  });

  test('no auth header is reported but still parses', () {
    final parsed = parseCurlConfig(
      r'''curl https://api.test.com/v1 -d '{"model":"m1"}' ''',
    ).parsed!;
    expect(parsed.hasKey, isFalse);
    expect(parsed.model, 'm1');
    expect(parsed.baseUrl, 'https://api.test.com');
  });
}
