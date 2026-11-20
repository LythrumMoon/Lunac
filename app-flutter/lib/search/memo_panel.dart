// 备忘录面板（v1 `memo` 插件的原生等价物）。
//
// 版式对齐 v1 `app/src/plugins/builtin/memo.ts`：
//   编辑视图：可选「已保存」提示 → 文本区 → 动作行「保存为备忘录 / 草稿自动保存中 /
//            清空全部 / 历史记录」→ 保存后弹出的「设置检索标识」对话框（保存 / 跳过，
//            Enter = 保存、Esc = 跳过）；
//   历史视图：顶行「返回编辑 / 历史记录」→ 每条一张卡片（时间 + 标识、完整文本、
//            复制 / 编辑 / 删除）。
// 文案全部取 v1 `i18n.ts` 的 `memo.*` 原文。数据走 Rust（`ModuleData/memo/memo.json`）。
//
// 与 v1 的差异：图片只做**已有图片的展示 / 移除**（缩略图取 `images` 里的绝对路径）；
// 粘贴 / 拖放新增图片要先把图片写进 `ModuleData/memo/images/<id>/`，未接。
// 草稿只保留在本次运行内（v1 用 localStorage 跨重启）。
import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/memo.dart';
import '../theme.dart';

/// 本次运行内的草稿（关闭面板再打开仍在；与 v1 的 localStorage 草稿同用途）。
String _draftCache = '';

/// v1 `memo.*` 文案（zh-CN 原文）。
const String _placeholder = '输入内容，自动保存草稿…';
const String _saveLabel = '保存为备忘录';
const String _autosaveLabel = '草稿自动保存中';
const String _typingLabel = '输入中…';
const String _clearLabel = '清空全部';
const String _confirmClearLabel = '确定清空全部备忘录？';
const String _historyLabel = '历史记录';
const String _emptyLabel = '暂无备忘录';
const String _savedLabel = '已保存';
const String _clearedLabel = '已清空';
const String _backLabel = '返回编辑';
const String _tagTitle = '为此备忘录设置检索标识';
const String _tagPlaceholder = '标识（如 #项目A / travel-plan，可在搜索栏直接检索进入）';

/// 备忘录面板：搜索「备忘录 / memo / 笔记」进入，或点「编辑备忘录 · <标识>」直达某条。
class MemoPanel extends StatefulWidget {
  const MemoPanel({
    super.key,
    required this.onClose,
    this.initialMemoId,
    this.loadMemos,
    this.upsertMemo,
    this.deleteMemo,
    this.clearMemos,
    this.pasteImage,
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 从搜索结果进来的目标条目（`memo:<id>`）；为空 = 打开编辑器。
  final String? initialMemoId;

  /// 数据源（测试注入；默认走 rust 桥接）。
  final List<MemoEntry>? Function()? loadMemos;
  final bool Function(MemoEntry entry)? upsertMemo;
  final bool Function(String id)? deleteMemo;
  final bool Function()? clearMemos;

  /// 读系统剪贴板图片并落盘（测试注入；默认走 rust 桥接）。
  final MemoImagePaste? Function(String memoId)? pasteImage;

  @override
  State<MemoPanel> createState() => _MemoPanelState();
}

class _MemoPanelState extends State<MemoPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'memo-panel',
    onKeyEvent: _onKey,
  );

  late final TextEditingController _text = TextEditingController(text: _draftCache);
  final TextEditingController _tag = TextEditingController();
  final FocusNode _tagFocus = FocusNode(debugLabel: 'memo-tag');

  List<MemoEntry>? _entries;

  /// 历史子界面（v1 `.memo-root[data-view="history"]`）。
  bool _historyOpen = false;

  /// 正在编辑的既有条目（null = 新备忘）。
  String? _editingId;

  /// 保存后弹出的「设置检索标识」（v1 `#memo-tag-dialog`）。
  bool _tagDialogOpen = false;

  /// 刚保存 / 刚清空的提示（v1 `.memo-saved-msg`）。
  String? _notice;

  /// 提示是错误（红字）还是成功（主题色）。
  bool _noticeIsError = false;

  /// 动作行右侧的自动保存提示（v1 `#memo-autosave-hint`）。
  String _hint = _autosaveLabel;
  Timer? _draftTimer;

  /// 刚复制的那一条（按钮短暂变成「已复制」）。
  String? _copiedId;
  Timer? _copiedTimer;

  bool _confirmingClear = false;
  Timer? _clearTimer;

  @override
  void initState() {
    super.initState();
    _entries = _readMemos();
    final id = widget.initialMemoId;
    if (id != null && id.isNotEmpty) {
      _loadEntry(id);
    }
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
  }

  @override
  void dispose() {
    _draftTimer?.cancel();
    _clearTimer?.cancel();
    _copiedTimer?.cancel();
    _tagFocus.dispose();
    _tag.dispose();
    _text.dispose();
    _panelFocus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    // Ctrl+V：剪贴板里是图片就挂到当前备忘录；是文字则交回文本框正常粘贴。
    if (event.logicalKey == LogicalKeyboardKey.keyV &&
        HardwareKeyboard.instance.isControlPressed) {
      return _pasteImage() ? KeyEventResult.handled : KeyEventResult.ignored;
    }
    if (event.logicalKey != LogicalKeyboardKey.escape) {
      return KeyEventResult.ignored;
    }
    // Esc：标识对话框 → 跳过；清空确认 → 取消；历史 → 返回编辑；其余关面板。
    if (_tagDialogOpen) {
      _skipTag();
    } else if (_confirmingClear) {
      _cancelClear();
    } else if (_historyOpen) {
      setState(() => _historyOpen = false);
    } else {
      widget.onClose();
    }
    return KeyEventResult.handled;
  }

  void _loadEntry(String id) {
    final entry = _findEntry(id);
    if (entry == null) {
      return;
    }
    _editingId = entry.id;
    _text.text = entry.text;
    _tag.text = entry.tag;
    _historyOpen = false;
    _notice = null;
    _noticeIsError = false;
  }

  MemoEntry? _findEntry(String id) {
    final list = _entries;
    if (list == null) {
      return null;
    }
    for (final entry in list) {
      if (entry.id == id) {
        return entry;
      }
    }
    return null;
  }

  List<MemoEntry>? _readMemos() => widget.loadMemos?.call() ?? readMemos();

  bool _upsert(MemoEntry entry) =>
      widget.upsertMemo?.call(entry) ?? upsertMemo(entry);

  bool _deleteEntry(String id) =>
      widget.deleteMemo?.call(id) ?? deleteMemo(id);

  bool _clearAllEntries() => widget.clearMemos?.call() ?? clearMemos();

  /// v1 `openTagDialog`：预填条目已有的标识。
  void _openTagDialog() {
    final target = _editingId == null ? null : _findEntry(_editingId!);
    _tag.text = target?.tag ?? '';
    setState(() {
      _tagDialogOpen = true;
      _notice = null;
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _tagFocus.requestFocus();
        _tag.selection = TextSelection(
          baseOffset: 0,
          extentOffset: _tag.text.length,
        );
      }
    });
  }

  void _resetEditor() {
    _draftCache = '';
    _text.clear();
    _tag.clear();
    _editingId = null;
    _hint = _autosaveLabel;
  }

  /// v1 保存流程：先把文本存成条目 → 弹出标识对话框 → 确认后才算完整保存。
  void _save() {
    final text = _text.text.trim();
    if (text.isEmpty) {
      return;
    }
    final now = DateTime.now().millisecondsSinceEpoch;
    final existing = _editingId == null ? null : _findEntry(_editingId!);
    final entry = MemoEntry(
      id: existing?.id ??
          '$now-${Random().nextInt(1 << 20).toRadixString(36)}',
      text: text,
      // 标识等对话框确认后再落盘（v1 同口径）。
      tag: existing?.tag ?? '',
      images: existing?.images ?? const [],
      ts: now,
    );
    if (!_upsert(entry)) {
      setState(() {
        _notice = '保存失败（rust 桥接不可用）';
        _noticeIsError = true;
      });
      return;
    }
    _editingId = entry.id;
    _entries = _readMemos() ?? _entries;
    _openTagDialog();
  }

  /// 对话框「保存」：写入标识后收尾。
  void _commitTag() {
    final id = _editingId;
    if (id != null) {
      final entry = _findEntry(id);
      if (entry != null) {
        _upsert(
          MemoEntry(
            id: entry.id,
            text: entry.text,
            tag: _tag.text.trim(),
            images: entry.images,
            ts: DateTime.now().millisecondsSinceEpoch,
          ),
        );
      }
    }
    _finishSave();
  }

  /// 对话框「跳过」：标识保持原样。
  void _skipTag() {
    _finishSave();
  }

  void _finishSave() {
    setState(() {
      _tagDialogOpen = false;
      _entries = _readMemos() ?? _entries;
      _resetEditor();
      _notice = _savedLabel;
      _noticeIsError = false;
    });
  }

  void _delete(MemoEntry entry) {
    _deleteEntry(entry.id);
    setState(() {
      _entries = _readMemos() ?? _entries;
      if (_editingId == entry.id) {
        _resetEditor();
      }
    });
  }

  void _cancelClear() {
    _clearTimer?.cancel();
    _clearTimer = null;
    setState(() => _confirmingClear = false);
  }

  /// v1 清空全部：两段式确认，3 秒内不点第二次就复位。
  void _clearAll() {
    if (!_confirmingClear) {
      setState(() => _confirmingClear = true);
      _clearTimer?.cancel();
      _clearTimer = Timer(const Duration(seconds: 3), () {
        if (mounted) {
          setState(() => _confirmingClear = false);
        }
      });
      return;
    }
    _clearTimer?.cancel();
    _clearTimer = null;
    _clearAllEntries();
    setState(() {
      _confirmingClear = false;
      _entries = _readMemos() ?? const [];
      _resetEditor();
      _notice = _clearedLabel;
      _noticeIsError = false;
    });
  }

  Future<void> _copy(String id, String text) async {
    try {
      await Clipboard.setData(ClipboardData(text: text));
    } catch (_) {
      return; // 无平台通道（测试）：不假装成功。
    }
    if (!mounted) {
      return;
    }
    setState(() => _copiedId = id);
    _copiedTimer?.cancel();
    _copiedTimer = Timer(const Duration(milliseconds: 1200), () {
      if (mounted) {
        setState(() => _copiedId = null);
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
            Expanded(child: _historyOpen ? _buildHistory() : _buildEditor()),
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
            '备忘录',
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

  /// v1 `.memo-root[data-view="main"]`：padding 12、gap 8 的竖排。
  Widget _buildEditor() {
    final notice = _notice;
    final noticeColor = _noticeIsError ? LunacColors.danger : LunacColors.accent;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 2, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (notice != null) ...[
            Row(
              children: [
                Icon(
                  _noticeIsError ? Icons.error_outline : Icons.check,
                  size: 12,
                  color: noticeColor,
                ),
                const SizedBox(width: 4),
                Expanded(
                  child: Text(
                    notice,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: noticeColor, fontSize: 11.5),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
          ],
          Expanded(
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
              decoration: BoxDecoration(
                color: LunacColors.background,
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.circular(8),
              ),
              child: TextField(
                controller: _text,
                maxLines: null,
                expands: true,
                textAlignVertical: TextAlignVertical.top,
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 13,
                  height: 1.6,
                ),
                cursorColor: LunacColors.accent,
                onChanged: _onDraftChanged,
                decoration: InputDecoration(
                  isDense: true,
                  border: InputBorder.none,
                  hintText: _placeholder,
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 13,
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          if (_editorImages.isNotEmpty) ...[
            _buildImageStrip(_editorImages, removable: true),
            const SizedBox(height: 8),
          ],
          _buildActions(),
          if (_tagDialogOpen) ...[
            const SizedBox(height: 4),
            _buildTagDialog(),
          ],
        ],
      ),
    );
  }

  /// 正在编辑条目已有的图片（v1 `savedImages`；新备忘 = 空）。
  List<String> get _editorImages {
    final id = _editingId;
    if (id == null) {
      return const [];
    }
    return _findEntry(id)?.images ?? const [];
  }

  /// v1 编辑器图片条的 ×：立即从该条目摘掉这张图并写盘（时间戳不变）。
  void _removeImage(int index) {
    final id = _editingId;
    if (id == null) {
      return;
    }
    final entry = _findEntry(id);
    if (entry == null || index < 0 || index >= entry.images.length) {
      return;
    }
    final images = [...entry.images]..removeAt(index);
    _upsert(
      MemoEntry(
        id: entry.id,
        text: entry.text,
        tag: entry.tag,
        images: images,
        ts: entry.ts,
      ),
    );
    setState(() => _entries = _readMemos() ?? _entries);
  }

  /// Ctrl+V：剪贴板里是图片就落盘并挂到当前备忘录（v1：先保存成备忘录，再粘贴）。
  ///
  /// 返回 true = 已消费这次按键；false = 剪贴板里是普通文字，交回文本框。
  bool _pasteImage() {
    final id = _editingId;
    if (id == null) {
      // 草稿还没有 id：图片无处可挂。剪贴板里若是文字，下面的 return false 让它照常粘贴。
      return false;
    }
    final result = widget.pasteImage?.call(id) ?? pasteMemoImage(id);
    if (!result.ok) {
      // 没有图片 = 用户其实想粘文字：不消费，交给 TextField。
      if (result.error.contains('没有图片')) {
        return false;
      }
      setState(() {
        _notice = result.error;
        _noticeIsError = true;
      });
      return true;
    }
    final entry = _findEntry(id);
    if (entry == null) {
      return true;
    }
    _upsert(
      MemoEntry(
        id: entry.id,
        text: entry.text,
        tag: entry.tag,
        images: [...entry.images, result.path],
        ts: entry.ts,
      ),
    );
    setState(() {
      _entries = _readMemos() ?? _entries;
      _notice = null;
      _noticeIsError = false;
    });
    return true;
  }

  /// v1 `.memo-images`：88×88 缩略图，6px 间距；[removable] 时右上角带 ×。
  Widget _buildImageStrip(List<String> images, {bool removable = false}) {
    // 留出 6px，避免右上角的 × 被上一个元素压住（v1 的负偏移同理）。
    return Padding(
      padding: const EdgeInsets.only(top: 6, right: 6),
      child: Wrap(
        spacing: 6,
        runSpacing: 6,
        children: [
          for (var i = 0; i < images.length; i++)
            _buildThumb(images[i], removeIndex: removable ? i : null),
        ],
      ),
    );
  }

  Widget _buildThumb(String path, {int? removeIndex}) {
    final thumb = Container(
      width: 88,
      height: 88,
      decoration: BoxDecoration(
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      clipBehavior: Clip.antiAlias,
      child: Image.file(
        File(path),
        width: 88,
        height: 88,
        fit: BoxFit.cover,
        errorBuilder: (context, error, stack) => ColoredBox(
          color: LunacColors.background,
          child: Icon(
            Icons.broken_image_outlined,
            size: 16,
            color: LunacColors.textMuted,
          ),
        ),
      ),
    );
    if (removeIndex == null) {
      return thumb;
    }
    return Stack(
      clipBehavior: Clip.none,
      children: [
        thumb,
        Positioned(
          top: -6,
          right: -6,
          child: Tooltip(
            message: '删除',
            child: GestureDetector(
              onTap: () => _removeImage(removeIndex),
              child: Container(
                width: 18,
                height: 18,
                decoration: BoxDecoration(
                  color: LunacColors.background.withValues(alpha: 0.65),
                  shape: BoxShape.circle,
                  border: Border.all(color: LunacColors.border),
                ),
                child: Icon(
                  Icons.close,
                  size: 10,
                  color: LunacColors.danger,
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }

  /// v1 `onDraftChanged`：输入时「输入中…」，500ms 无输入后回到「草稿自动保存中」。
  void _onDraftChanged(String value) {
    _draftCache = value;
    setState(() => _hint = _typingLabel);
    _draftTimer?.cancel();
    _draftTimer = Timer(const Duration(milliseconds: 500), () {
      if (mounted) {
        setState(() => _hint = _autosaveLabel);
      }
    });
  }

  /// v1 `.memo-actions`：保存 / 提示 / 清空全部（靠右）/ 历史记录。
  Widget _buildActions() {
    return Row(
      children: [
        _smallButton(
          _saveLabel,
          onPressed: _save,
          highlight: true,
        ),
        const SizedBox(width: 8),
        Text(
          _hint,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
        ),
        const Spacer(),
        _smallButton(
          _confirmingClear ? _confirmClearLabel : _clearLabel,
          onPressed: _clearAll,
          danger: _confirmingClear,
        ),
        const SizedBox(width: 8),
        _smallButton(
          _historyLabel,
          onPressed: () {
            setState(() {
              _historyOpen = true;
              _notice = null;
              _tagDialogOpen = false;
            });
          },
        ),
      ],
    );
  }

  /// v1 `.memo-tag-dialog`：标题 + 输入框 + 右对齐「保存 / 跳过」。
  Widget _buildTagDialog() {
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.divider),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Icon(
                Icons.local_offer_outlined,
                size: 12,
                color: LunacColors.textDim,
              ),
              const SizedBox(width: 6),
              Text(
                _tagTitle,
                style: TextStyle(color: LunacColors.text, fontSize: 12),
              ),
            ],
          ),
          const SizedBox(height: 6),
          SizedBox(
            height: 30,
            child: TextField(
              controller: _tag,
              focusNode: _tagFocus,
              style: TextStyle(color: LunacColors.text, fontSize: 12),
              cursorColor: LunacColors.accent,
              onSubmitted: (_) => _commitTag(),
              decoration: InputDecoration(
                isDense: true,
                filled: true,
                fillColor: LunacColors.surface,
                contentPadding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                hintText: _tagPlaceholder,
                hintStyle: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                ),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.border),
                ),
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.border),
                ),
                focusedBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.divider),
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              _smallButton('保存', onPressed: _commitTag, highlight: true),
              const SizedBox(width: 6),
              _smallButton('跳过', onPressed: _skipTag),
            ],
          ),
        ],
      ),
    );
  }

  /// v1 `.memo-root[data-view="history"]`。
  Widget _buildHistory() {
    final entries = _entries;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 2, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              _smallButton(
                _backLabel,
                onPressed: () => setState(() => _historyOpen = false),
              ),
              const SizedBox(width: 8),
              Row(
                children: [
                  Icon(
                    Icons.format_list_bulleted,
                    size: 12,
                    color: LunacColors.text,
                  ),
                  SizedBox(width: 6),
                  Text(
                    _historyLabel,
                    style: TextStyle(
                      color: LunacColors.text,
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ),
            ],
          ),
          const SizedBox(height: 6),
          Expanded(child: _buildHistoryList(entries)),
        ],
      ),
    );
  }

  Widget _buildHistoryList(List<MemoEntry>? entries) {
    if (entries == null) {
      return Center(
        child: Text(
          '不可用（rust 桥接未加载）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    if (entries.isEmpty) {
      return Center(
        child: Text(
          _emptyLabel,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    return ListView.separated(
      padding: const EdgeInsets.only(top: 2, bottom: 4),
      itemCount: entries.length,
      separatorBuilder: (_, _) => const SizedBox(height: 6),
      itemBuilder: (context, index) => _buildHistoryItem(entries[index]),
    );
  }

  /// v1 `.memo-hist-item`：时间 + 标识 → 完整文本 → 复制 / 编辑 / 删除。
  Widget _buildHistoryItem(MemoEntry entry) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text(
                _formatTs(entry.ts),
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 10.5,
                ),
              ),
              const Spacer(),
              if (entry.tag.trim().isNotEmpty)
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
                  decoration: BoxDecoration(
                    color: LunacColors.surfaceHover,
                    border: Border.all(color: LunacColors.divider),
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Text(
                    entry.tag.trim(),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.text,
                      fontSize: 10.5,
                    ),
                  ),
                ),
            ],
          ),
          const SizedBox(height: 4),
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 120),
            child: SingleChildScrollView(
              child: Text(
                entry.text,
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12.5,
                  height: 1.45,
                ),
              ),
            ),
          ),
          if (entry.images.isNotEmpty) ...[
            const SizedBox(height: 6),
            _buildImageStrip(entry.images),
          ],
          const SizedBox(height: 6),
          Row(
            children: [
              _smallButton(
                _copiedId == entry.id ? '已复制' : '复制',
                onPressed: () => _copy(entry.id, entry.text),
              ),
              const SizedBox(width: 6),
              _smallButton(
                '编辑',
                onPressed: () => setState(() => _loadEntry(entry.id)),
              ),
              const SizedBox(width: 6),
              _smallButton('删除', onPressed: () => _delete(entry)),
            ],
          ),
        ],
      ),
    );
  }

  Widget _smallButton(
    String label, {
    required VoidCallback onPressed,
    bool highlight = false,
    bool danger = false,
  }) {
    return SizedBox(
      height: 26,
      child: TextButton(
        onPressed: onPressed,
        style: TextButton.styleFrom(
          foregroundColor: danger
              ? LunacColors.danger
              : highlight
                  ? LunacColors.text
                  : LunacColors.textDim,
          backgroundColor: highlight
              ? LunacColors.surfaceHover
              : LunacColors.background,
          side: BorderSide(
            color: highlight ? LunacColors.divider : LunacColors.border,
          ),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(5),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 10),
          minimumSize: Size.zero,
          visualDensity: VisualDensity.compact,
        ),
        child: Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(fontSize: 11),
        ),
      ),
    );
  }

  /// `yyyy-MM-dd HH:mm`（与 v1 `fmtTs` 同格式）。
  static String _formatTs(int millis) {
    if (millis <= 0) {
      return '';
    }
    final date = DateTime.fromMillisecondsSinceEpoch(millis);
    String pad(int value) => value.toString().padLeft(2, '0');
    return '${date.year}-${pad(date.month)}-${pad(date.day)} '
        '${pad(date.hour)}:${pad(date.minute)}';
  }
}
