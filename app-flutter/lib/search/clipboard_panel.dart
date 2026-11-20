// 剪贴板历史面板（v1 `clipboard-history` 插件的原生等价物）。
//
// 版式对齐 v1 `app/src/plugins/builtin/clipboard-history.ts`：
//   顶行「N 条记录」+ 右侧「全部清空」→ 单行条目（图标 / 80 字预览 / 相对时间 · N 字符 /
//   「复制」按钮）；点整行 = 复制到系统剪贴板并把该条提到最前。
// 文案全部取 v1 `i18n.ts` 的 `clipboard.*` 原文（含「已清空历史。」）。
//
// 记录在唤出窗口时由 `main.dart` 顺手写入 Rust（`ModuleData/history/clipboard-history.json`），
// 本面板只读 + 复制 + 清空。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/clipboard_history.dart';
import '../theme.dart';

/// 剪贴板历史面板：搜索「剪贴板 / clipboard / 粘贴」进入。
class ClipboardPanel extends StatefulWidget {
  const ClipboardPanel({super.key, required this.onClose});

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  @override
  State<ClipboardPanel> createState() => _ClipboardPanelState();
}

class _ClipboardPanelState extends State<ClipboardPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'clipboard-panel',
    onKeyEvent: _onKey,
  );

  List<ClipboardEntry>? _entries = readClipboardHistory();

  /// 刚复制的那一条（按钮显示 1.5s 的「✓ 已复制」）。
  String? _copied;
  Timer? _copiedTimer;

  /// 刚清空（v1 把内容换成一整行「已清空历史。」）。
  bool _cleared = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
  }

  @override
  void dispose() {
    _copiedTimer?.cancel();
    _panelFocus.dispose();
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

  /// 点整行 / 点「复制」：写系统剪贴板 + 把该条提到最前（v1 同口径）。
  Future<void> _copy(ClipboardEntry entry) async {
    try {
      await Clipboard.setData(ClipboardData(text: entry.text));
    } catch (_) {
      // 无平台通道（测试 / 剪贴板被占用）：仍然把条目提到最前，界面如实反映。
    }
    promoteClipboardEntry(entry.text);
    if (!mounted) {
      return;
    }
    setState(() {
      _entries = readClipboardHistory() ?? _entries;
      _copied = entry.text;
      _cleared = false;
    });
    _copiedTimer?.cancel();
    _copiedTimer = Timer(const Duration(milliseconds: 1500), () {
      if (mounted) {
        setState(() => _copied = null);
      }
    });
  }

  /// v1 的「全部清空」没有二次确认：点一下直接清（文案随即变成「已清空历史。」）。
  void _clearAll() {
    clearClipboardHistory();
    setState(() {
      _entries = readClipboardHistory() ?? const [];
      _copied = null;
      _cleared = true;
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
        // 上沿 1px 接缝盖在描边上：与搜索栏的分界线（与结果区同口径）。
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
      padding: const EdgeInsets.fromLTRB(6, 6, 12, 2),
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
            '剪贴板历史',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildBody() {
    final entries = _entries;
    if (entries == null) {
      return Center(
        child: Text(
          '不可用（rust 桥接未加载）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    if (_cleared || entries.isEmpty) {
      return Padding(
        padding: const EdgeInsets.fromLTRB(12, 12, 12, 12),
        child: Text(
          _cleared
              ? '已清空历史。'
              : '暂无剪贴板历史。复制任意内容后会自动记录。',
          style: TextStyle(color: LunacColors.textDim, fontSize: 13),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // v1 顶行：左「N 条记录」，右「全部清空」。
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 2, 12, 8),
          child: Row(
            children: [
              Text(
                '${entries.length} 条记录',
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 11.5,
                ),
              ),
              const Spacer(),
              TextButton(
                onPressed: _clearAll,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.danger,
                  visualDensity: VisualDensity.compact,
                  minimumSize: Size.zero,
                  padding: const EdgeInsets.symmetric(horizontal: 6),
                ),
                child: const Text('全部清空', style: TextStyle(fontSize: 10.5)),
              ),
            ],
          ),
        ),
        Expanded(
          child: ListView.builder(
            padding: const EdgeInsets.only(bottom: 8),
            itemCount: entries.length,
            itemBuilder: (context, index) => _buildRow(entries[index]),
          ),
        ),
      ],
    );
  }

  Widget _buildRow(ClipboardEntry entry) {
    final copied = _copied == entry.text;
    return InkWell(
      onTap: () => _copy(entry),
      hoverColor: LunacColors.surfaceHover,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        child: Row(
          children: [
            Icon(
              entry.isFile ? Icons.folder_outlined : Icons.article_outlined,
              size: 15,
              color: LunacColors.textMuted,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                entry.preview,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12.5,
                ),
              ),
            ),
            const SizedBox(width: 8),
            Text(
              _metaText(entry),
              style: TextStyle(
                color: LunacColors.textDim,
                fontSize: 10.5,
              ),
            ),
            const SizedBox(width: 8),
            SizedBox(
              height: 22,
              child: TextButton(
                onPressed: () => _copy(entry),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.text,
                  backgroundColor: LunacColors.surfaceHover,
                  side: BorderSide(color: LunacColors.divider),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(4),
                  ),
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  minimumSize: Size.zero,
                  visualDensity: VisualDensity.compact,
                ),
                child: Text(
                  copied ? '✓ 已复制' : '复制',
                  style: const TextStyle(fontSize: 10.5),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// 右下的「相对时间 · N 字符」（文件条目没有 N 字符）。
  static String _metaText(ClipboardEntry entry) {
    final relative = _relativeTime(entry.time);
    final size = entry.sizeHint;
    return size.isEmpty ? relative : '$relative · $size';
  }

  /// 相对时间（与 v1 `formatRelative` 同口径：刚刚 / N 分钟前 / N 小时前 / N 天前）。
  static String _relativeTime(int millis) {
    if (millis <= 0) {
      return '';
    }
    final now = DateTime.now().millisecondsSinceEpoch;
    final diff = now - millis;
    if (diff < 60 * 1000) {
      return '刚刚';
    }
    final minutes = diff ~/ (60 * 1000);
    if (minutes < 60) {
      return '$minutes 分钟前';
    }
    final hours = minutes ~/ 60;
    if (hours < 24) {
      return '$hours 小时前';
    }
    return '${hours ~/ 24} 天前';
  }
}
