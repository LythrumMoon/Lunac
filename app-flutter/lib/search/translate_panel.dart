// 翻译面板（v1 `translate` 插件的原生等价物）。
//
// 语义与 v1 `app/src/plugins/builtin/translate.ts` 对齐：
//   · 首屏不自动查询：待译内容由搜索栏预填（`翻译 hello` → `hello`，见
//     `stripTranslateInvocation`，对应 v1 `translate.ts:89-93` 的 `stripInvocation`）；
//   · 源语言下拉含「自动」，目标语言必须明确；**语言选择跨面板实例保留**
//     （v1 `lastFrom` / `lastTo`，`translate.ts:48-50`）；
//   · 点「翻译」或按 Enter 查词典底座，Shift+Enter 换行（`translate.ts:160-167`）；
//   · 结果区：译文 / 音标 / 释义 / 例句 / 来源小字 /「用 AI 重译」；
//     两层都没结果时才出现「用 AI 翻译」（`translate.ts:183-208`）。
//
// 与 v1 的差异：联网与缓存都在 Rust（`lunac_translate_lookup`），本文件只做展示；
// 翻译期间 UI 不冻结（平台层把 FFI 放进独立 isolate）。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/translate.dart';
import '../theme.dart';

/// 源语言下拉：第一项是「自动」。
const List<String> _fromLangs = ['auto', 'zh-CN', 'en', 'ja', 'ko'];

/// 目标语言下拉：没有「自动」—— 目标语言必须明确。
const List<String> _toLangs = ['zh-CN', 'en', 'ja', 'ko'];

/// 语言码 → 面板文案。
const Map<String, String> _langLabels = {
  'auto': '自动',
  'zh-CN': '中文',
  'en': '英语',
  'ja': '日语',
  'ko': '韩语',
};

/// 上次选的语言：**按面板实例重建不清空**（v1 同口径）。
String _lastFrom = 'auto';
String _lastTo = 'zh-CN';

/// 剥掉调用词（`翻译 hello` / `translate hello` → `hello`）。
///
/// 只在开头匹配、后面必须跟空白或到头 —— 否则 `translator` 会被切成 `or`。
String stripTranslateInvocation(String input) {
  final s = input.trim();
  final match = RegExp(
    r'^(翻译|translate|词典|字典|查词|dict)(\s+|$)',
    caseSensitive: false,
  ).firstMatch(s);
  return match == null ? s : s.substring(match.end).trim();
}

/// 翻译面板：搜索「翻译 / translate / 词典」进入。
class TranslatePanel extends StatefulWidget {
  const TranslatePanel({
    super.key,
    required this.onClose,
    this.initialQuery = '',
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 搜索栏里的原始查询词（会剥掉「翻译」等调用词后预填）。
  final String initialQuery;

  @override
  State<TranslatePanel> createState() => _TranslatePanelState();
}

class _TranslatePanelState extends State<TranslatePanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'translate-panel',
    onKeyEvent: _onKey,
  );

  late final TextEditingController _text = TextEditingController(
    text: stripTranslateInvocation(widget.initialQuery),
  );

  /// Enter 翻译、Shift+Enter 换行（v1 `translate.ts:160-167`）。
  late final FocusNode _inputFocus = FocusNode(
    debugLabel: 'translate-input',
    onKeyEvent: _onInputKey,
  );

  late String _from = _fromLangs.contains(_lastFrom) ? _lastFrom : 'auto';
  late String _to = _toLangs.contains(_lastTo) ? _lastTo : 'zh-CN';

  bool _busy = false;
  TranslateResult? _result;
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _inputFocus.requestFocus();
      }
    });
  }

  @override
  void dispose() {
    _panelFocus.dispose();
    _inputFocus.dispose();
    _text.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  KeyEventResult _onInputKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.enter &&
        !HardwareKeyboard.instance.isShiftPressed) {
      _run(useAi: false);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  /// 词典底座（`useAi=false`）或模型补漏（`useAi=true`）。
  Future<void> _run({required bool useAi}) async {
    final text = _text.text.trim();
    if (text.isEmpty || _busy) {
      return;
    }
    _lastFrom = _from;
    _lastTo = _to;
    setState(() {
      _busy = true;
      _error = null;
      _result = null; // 立刻进「翻译中」，别让按钮在慢请求里看着没反应
    });
    final reply = useAi
        ? await translateWithAi(text, from: _from, to: _to)
        : await lookupTranslation(text, from: _from, to: _to);
    if (!mounted) {
      return;
    }
    setState(() {
      _busy = false;
      if (reply.isError) {
        _error = reply.error;
        _result = null;
      } else {
        _error = null;
        _result = reply.result;
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      focusNode: _panelFocus,
      autofocus: true,
      onKeyEvent: _onKey,
      child: Container(
        decoration: BoxDecoration(
          color: LunacColors.surface,
          border: Border.all(color: LunacColors.border),
          borderRadius: const BorderRadius.vertical(
            bottom: Radius.circular(12),
          ),
        ),
        foregroundDecoration: BoxDecoration(
          border: Border(top: BorderSide(color: LunacColors.divider)),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            _buildHeader(),
            Expanded(child: _buildBody()),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 8, 12, 6),
      child: Row(
        children: [
          IconButton(
            tooltip: '返回（Esc）',
            icon: const Icon(Icons.arrow_back, size: 18),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: widget.onClose,
          ),
          const SizedBox(width: 2),
          Text(
            '翻译',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          Text(
            '词典底座 · 免 key',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          const Spacer(),
          Text(
            '缓存：ModuleData/translate/cache.db',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
          ),
        ],
      ),
    );
  }

  Widget _buildBody() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 2, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 82,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
              decoration: BoxDecoration(
                color: LunacColors.background,
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.circular(8),
              ),
              child: TextField(
                controller: _text,
                focusNode: _inputFocus,
                maxLines: null,
                expands: true,
                textAlignVertical: TextAlignVertical.top,
                style: TextStyle(color: LunacColors.text, fontSize: 13),
                cursorColor: LunacColors.accent,
                decoration: InputDecoration(
                  isDense: true,
                  border: InputBorder.none,
                  hintText: '输入要翻译的文字或单词',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 13,
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              _buildLangDropdown(
                value: _from,
                options: _fromLangs,
                onChanged: (value) => setState(() => _from = value),
              ),
              Padding(
                padding: EdgeInsets.symmetric(horizontal: 6),
                child: Text(
                  '→',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
                ),
              ),
              _buildLangDropdown(
                value: _to,
                options: _toLangs,
                onChanged: (value) => setState(() => _to = value),
              ),
              const Spacer(),
              SizedBox(
                height: 30,
                child: TextButton(
                  onPressed: _busy ? null : () => _run(useAi: false),
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.accent,
                    backgroundColor: LunacColors.surfaceHover,
                    visualDensity: VisualDensity.compact,
                    padding: const EdgeInsets.symmetric(horizontal: 14),
                  ),
                  child: const Text('翻译', style: TextStyle(fontSize: 12)),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Expanded(child: _buildResult()),
        ],
      ),
    );
  }

  Widget _buildLangDropdown({
    required String value,
    required List<String> options,
    required ValueChanged<String> onChanged,
  }) {
    return Container(
      height: 30,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(6),
      ),
      child: DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: value,
          isDense: true,
          dropdownColor: LunacColors.surfaceHover,
          iconSize: 16,
          iconEnabledColor: LunacColors.textMuted,
          style: TextStyle(color: LunacColors.textDim, fontSize: 12),
          items: [
            for (final code in options)
              DropdownMenuItem<String>(
                value: code,
                child: Text(_langLabels[code] ?? code),
              ),
          ],
          onChanged: (value) {
            if (value != null) {
              onChanged(value);
            }
          },
        ),
      ),
    );
  }

  Widget _buildResult() {
    if (_busy) {
      return Center(
        child: Text(
          '翻译中…',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    final error = _error;
    if (error != null) {
      return Align(
        alignment: Alignment.topLeft,
        child: Text(
          error,
          style: TextStyle(color: LunacColors.danger, fontSize: 13),
        ),
      );
    }
    final result = _result;
    if (result == null) {
      // v1 的首屏结果区是空的（不预置提示语）。
      return const SizedBox.shrink();
    }
    if (result.source == 'none') {
      return Align(
        alignment: Alignment.topLeft,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              '词典里没有这个词条。',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
            ),
            const SizedBox(height: 6),
            _buildAiButton('用 AI 翻译'),
          ],
        ),
      );
    }

    final parts = <Widget>[];
    if (result.translation.isNotEmpty) {
      parts.add(
        Text(
          result.translation,
          style: TextStyle(
            color: LunacColors.text,
            fontSize: 16,
            height: 1.35,
          ),
        ),
      );
    }
    if (result.phonetic.isNotEmpty) {
      parts.add(
        Padding(
          padding: const EdgeInsets.only(top: 4),
          child: Text(
            result.phonetic,
            style: TextStyle(color: LunacColors.textDim, fontSize: 12),
          ),
        ),
      );
    }
    if (result.explains.isNotEmpty) {
      parts.add(
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (final line in result.explains)
                Padding(
                  padding: const EdgeInsets.only(bottom: 2),
                  child: Text(
                    line,
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 12,
                      height: 1.4,
                    ),
                  ),
                ),
            ],
          ),
        ),
      );
    }
    if (result.examples.isNotEmpty) {
      parts.add(
        Padding(
          padding: const EdgeInsets.only(top: 8),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (final example in result.examples)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        example.src,
                        style: TextStyle(
                          color: LunacColors.textDim,
                          fontSize: 12,
                          height: 1.35,
                        ),
                      ),
                      Text(
                        example.dst,
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 12,
                          height: 1.35,
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      );
    }

    // 状态小字：来源 + 是否来自缓存（v1 不弹提示，就地一行）。
    final meta = result.cached
        ? '来自本地缓存'
        : result.source == 'ai'
            ? '来自 AI'
            : '来自词典';
    parts.add(
      Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Text(
          meta,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
        ),
      ),
    );
    // 词典结果旁给「用 AI 重新翻译」：整句质量不如模型（v1 放在小字下方）。
    parts.add(
      Align(
        alignment: Alignment.centerLeft,
        child: _buildAiButton('用 AI 重新翻译'),
      ),
    );
    return SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: parts,
      ),
    );
  }

  Widget _buildAiButton(String label) {
    return TextButton(
      onPressed: _busy ? null : () => _run(useAi: true),
      style: TextButton.styleFrom(
        foregroundColor: LunacColors.accent,
        visualDensity: VisualDensity.compact,
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        minimumSize: const Size(0, 26),
      ),
      child: Text(label, style: const TextStyle(fontSize: 12)),
    );
  }
}
