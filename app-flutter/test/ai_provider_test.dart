// 供应商预设 / 模型回落规则的单测（v1 `PROVIDER_PRESETS` / `MODEL_SUGGESTIONS` 口径）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/ai_provider.dart';

void main() {
  test('presets keep the v1 order, labels, urls and default models', () {
    expect(
      aiProviderPresets.map((preset) => preset.key).toList(),
      [
        'openai',
        'deepseek',
        'anthropic',
        'google',
        'zhipu',
        'moonshot',
        'qwen',
        'siliconflow',
        'custom',
      ],
    );
    expect(providerPreset('deepseek').url, 'https://api.deepseek.com');
    expect(providerPreset('deepseek').model, 'deepseek-flash');
    expect(providerPreset('google').label, 'Google Gemini');
    expect(providerPreset('qwen').label, 'Qwen (Tongyi)');
    expect(providerPreset('custom').url, isEmpty);
    expect(providerPreset('my-host').label, 'my-host');
  });

  test('deepseek lists all three v1 model names', () {
    // 「三个都要列」：少一个就会在切供应商时被静默改写（v1 真实事故）。
    expect(
      aiModelSuggestions['deepseek'],
      containsAll(['deepseek-flash', 'deepseek-v4-flash', 'deepseek-v4-pro']),
    );
    expect(aiModelSuggestions['custom'], isEmpty);
  });

  test('belongs to another provider only for names in that provider list', () {
    expect(belongsToOtherProvider('deepseek-v4-flash', 'deepseek'), isFalse);
    expect(belongsToOtherProvider('deepseek-v4-flash', 'openai'), isTrue);
    expect(belongsToOtherProvider('claude-sonnet-5', 'anthropic'), isFalse);
    expect(belongsToOtherProvider('claude-sonnet-5', 'deepseek'), isTrue);
    // 认不出来的名字（自定义 / 清单漏列）不算任何供应商
    expect(belongsToOtherProvider('my-private-model', 'deepseek'), isFalse);
    expect(belongsToOtherProvider('', 'deepseek'), isFalse);
  });

  test('switching provider keeps unknown models and falls back on foreign ones', () {
    // 真实事故：存 deepseek-v4-flash → 切到 deepseek → 必须原样保留
    expect(
      modelAfterProviderSwitch(
        currentModel: 'deepseek-v4-flash',
        provider: 'deepseek',
      ),
      'deepseek-v4-flash',
    );
    // 自定义名字同样保留
    expect(
      modelAfterProviderSwitch(
        currentModel: 'my-private-model',
        provider: 'openai',
      ),
      'my-private-model',
    );
    // 别的供应商的模型 → 回落该供应商的默认模型
    expect(
      modelAfterProviderSwitch(
        currentModel: 'claude-sonnet-5',
        provider: 'openai',
      ),
      'gpt-5.5',
    );
    // 空模型 → 默认模型；自定义供应商没有默认模型
    expect(
      modelAfterProviderSwitch(currentModel: '', provider: 'moonshot'),
      'kimi-k3',
    );
    expect(
      modelAfterProviderSwitch(
        currentModel: 'claude-sonnet-5',
        provider: 'custom',
      ),
      isEmpty,
    );
  });
}
