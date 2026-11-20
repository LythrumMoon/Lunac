// AI 供应商预设 + 模型建议（v1 `app/src/plugins/builtin/settings.ts` 的同名表的 v2 副本）。
//
// 这两张表是**面板行为的单一真相源**：切供应商时「该不该改写模型名」靠它们判断。
// 口径与 v1 逐条一致（2026-10-05 的真实事故写在第 3 处）：
//   · 供应商预设 = 展示名 / 默认端点（不带 /v1）/ 默认模型；
//   · 模型建议 = 每个供应商的常用模型（只作下拉候选，用户仍可手输任意名字）；
//   · **切供应商只在「当前模型属于别的供应商」时才回落默认模型** —— 认不出来的名字
//     （用户手输的自定义名、清单里暂时漏列的名字）必须原样保留，否则「存
//     deepseek-v4-flash → 切一次供应商 → 变 deepseek-v4-pro → 保存落盘」会静默发生。
import 'package:flutter/foundation.dart' show immutable;

/// 供应商预设（顺序 = 下拉顺序；最后一个是自定义空壳）。
@immutable
class AiProviderPreset {
  const AiProviderPreset({
    required this.key,
    required this.label,
    required this.url,
    required this.model,
  });

  final String key;
  final String label;
  final String url;
  final String model;

  static const AiProviderPreset unknown = AiProviderPreset(
    key: '',
    label: '',
    url: '',
    model: '',
  );
}

/// v1 `PROVIDER_PRESETS` 的键 / 展示名 / 默认端点 / 默认模型（逐字一致）。
const List<AiProviderPreset> aiProviderPresets = [
  AiProviderPreset(
    key: 'openai',
    label: 'OpenAI',
    url: 'https://api.openai.com',
    model: 'gpt-5.5',
  ),
  AiProviderPreset(
    key: 'deepseek',
    label: 'DeepSeek',
    url: 'https://api.deepseek.com',
    model: 'deepseek-flash',
  ),
  AiProviderPreset(
    key: 'anthropic',
    label: 'Anthropic',
    url: 'https://api.anthropic.com',
    model: 'claude-sonnet-5',
  ),
  AiProviderPreset(
    key: 'google',
    label: 'Google Gemini',
    url: 'https://generativelanguage.googleapis.com/v1beta/openai',
    model: 'gemini-3.6-flash',
  ),
  AiProviderPreset(
    key: 'zhipu',
    label: 'Zhipu GLM',
    url: 'https://open.bigmodel.cn/api/paas/v4',
    model: 'glm-5',
  ),
  AiProviderPreset(
    key: 'moonshot',
    label: 'Moonshot Kimi',
    url: 'https://api.moonshot.cn',
    model: 'kimi-k3',
  ),
  AiProviderPreset(
    key: 'qwen',
    label: 'Qwen (Tongyi)',
    url: 'https://dashscope.aliyuncs.com/compatible-mode',
    model: 'qwen3.8-max',
  ),
  AiProviderPreset(
    key: 'siliconflow',
    label: 'SiliconFlow',
    url: 'https://api.siliconflow.cn',
    model: 'Qwen/Qwen3-235B-A22B',
  ),
  AiProviderPreset(key: 'custom', label: 'Custom', url: '', model: ''),
];

/// v1 `MODEL_SUGGESTIONS`（逐字一致；只作候选，不限制手输）。
///
/// DeepSeek 这一行**三个都要列**：少列一个，用户存过的那个名字就会被当成未知模型，
/// 在切供应商时被静默改写成预设默认值（真实事故：存 `deepseek-v4-flash` → 变
/// `deepseek-v4-pro`）。
const Map<String, List<String>> aiModelSuggestions = {
  'openai': ['gpt-5.5', 'gpt-5.5-pro', 'gpt-5.4', 'gpt-5.4-mini', 'gpt-5.4-nano'],
  'deepseek': ['deepseek-flash', 'deepseek-v4-flash', 'deepseek-v4-pro'],
  'anthropic': [
    'claude-sonnet-5',
    'claude-opus-5',
    'claude-fable-5',
    'claude-haiku-4-5-20251001',
  ],
  'google': [
    'gemini-3.6-flash',
    'gemini-3.5-flash',
    'gemini-3.1-pro-preview',
    'gemini-3.1-flash-lite',
  ],
  'zhipu': ['glm-5', 'glm-4.7', 'glm-4.5-air', 'glm-4.5-flash'],
  'moonshot': ['kimi-k3', 'kimi-k2.6', 'kimi-k2.7-code'],
  'qwen': ['qwen3.8-max', 'qwen3.7-max', 'qwen3.7-plus', 'qwen3.7-flash'],
  'siliconflow': [
    'Qwen/Qwen3-235B-A22B',
    'deepseek-ai/DeepSeek-V3',
    'Qwen/Qwen3-30B-A3B',
    'zai-org/GLM-5',
  ],
  'custom': [],
};

/// 按 key 找预设；未知 key 返回空壳（自定义供应商）。
AiProviderPreset providerPreset(String key) {
  for (final preset in aiProviderPresets) {
    if (preset.key == key) {
      return preset;
    }
  }
  return AiProviderPreset(key: key, label: key, url: '', model: '');
}

/// 该模型名是否属于**别的**供应商（只有这种情况才允许在切供应商时回落默认值）。
bool belongsToOtherProvider(String model, String provider) {
  if (model.isEmpty) {
    return false;
  }
  for (final entry in aiModelSuggestions.entries) {
    if (entry.key != 'custom' &&
        entry.key != provider &&
        entry.value.contains(model)) {
      return true;
    }
  }
  for (final preset in aiProviderPresets) {
    if (preset.key != 'custom' &&
        preset.key != provider &&
        preset.model == model) {
      return true;
    }
  }
  return false;
}

/// 切供应商后模型输入框该填什么（v1 `reloadModelSelector` 的选中规则）。
///
/// 保留当前模型（非空且不属于别的供应商）；否则回落该供应商的预设默认模型。
String modelAfterProviderSwitch({
  required String currentModel,
  required String provider,
}) {
  if (currentModel.isNotEmpty && !belongsToOtherProvider(currentModel, provider)) {
    return currentModel;
  }
  return providerPreset(provider).model;
}
