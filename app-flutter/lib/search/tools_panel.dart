// 工具编辑器面板（v1 `tool-editor` 插件的原生等价物）。
//
// 版式对齐 v1 `app/src/plugins/builtin/tool-editor.ts`：
//   列表：顶行「已加载 N 个工具」+ 右侧「+ 新建工具」；每条是「🔧 / 名称 + 有效徽标 /
//        描述」一行，右侧「编辑 / 删除」（删除为两段式确认，与 v1 同）；
//   编辑器：标题「新建工具」或「编辑: <文件名>」+ 右侧「返回」→ JSON 文本区（等宽字体）→
//         错误行 → 右下「保存 / 取消」。
// 数据走 Rust：`<exe 根>/tools/*.json`，一份文件 = 一个工具定义（v1 同形）。
import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/mcp_config.dart';
import '../theme.dart';

/// v1 `TOOL_TEMPLATE`：新建工具时预填的 JSON。
const String _toolTemplate = '''{
  "name": "my-tool",
  "description": "What this tool does",
  "inputSchema": {
    "type": "object",
    "properties": {},
    "required": []
  },
  "handler": {
    "type": "shell",
    "command": "echo hello"
  }
}''';

/// 工具编辑器面板：搜索「工具编辑器 / 工具 / mcp」进入。
class ToolsPanel extends StatefulWidget {
  const ToolsPanel({super.key, required this.onClose});

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  @override
  State<ToolsPanel> createState() => _ToolsPanelState();
}

class _ToolsPanelState extends State<ToolsPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'tools-panel',
    onKeyEvent: _onKey,
  );

  final TextEditingController _editorText = TextEditingController();

  /// null = rust 桥接不可用。
  ToolConfigResult? _config;

  /// 编辑器打开（新建或编辑）；false = 列表。
  bool _editing = false;

  /// 正在编辑的文件名；null = 新建。
  String? _editingFilename;

  String? _message;
  bool _messageError = false;

  /// 已「上膛」待二次确认删除的文件（v1 的两段式删除）。
  final Set<String> _armed = {};
  Timer? _armTimer;

  @override
  void initState() {
    super.initState();
    _reload();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
  }

  @override
  void dispose() {
    _armTimer?.cancel();
    _editorText.dispose();
    _panelFocus.dispose();
    super.dispose();
  }

  void _reload() {
    _config = readToolConfig();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent ||
        event.logicalKey != LogicalKeyboardKey.escape) {
      return KeyEventResult.ignored;
    }
    // Esc：编辑器里先退回列表（v1 的「返回」），列表里才关面板。
    if (_editing) {
      _closeEditor();
    } else {
      widget.onClose();
    }
    return KeyEventResult.handled;
  }

  void _startNew() {
    setState(() {
      _editing = true;
      _editingFilename = null;
      _message = null;
      _editorText.text = _toolTemplate;
    });
  }

  void _startEdit(ToolEntry entry) {
    final file = readToolFile(entry.filename);
    setState(() {
      _editing = true;
      _editingFilename = entry.filename;
      _message = file.ok ? null : file.error;
      _messageError = !file.ok;
      // 读不到时退回模板（v1 同口径），避免空文本区看不出该写什么。
      _editorText.text = file.ok ? file.content : _toolTemplate;
    });
  }

  void _closeEditor() {
    setState(() {
      _editing = false;
      _editingFilename = null;
      _message = null;
    });
    _panelFocus.requestFocus();
  }

  void _save() {
    final content = _editorText.text;
    Object? parsed;
    try {
      parsed = jsonDecode(content);
    } catch (e) {
      setState(() {
        _messageError = true;
        _message = '无效的 JSON: $e';
      });
      return;
    }
    var filename = _editingFilename;
    if (filename == null) {
      // v1：文件名由 `name` 推导（非 [A-Za-z0-9_-] 一律换成 -）。
      final name = parsed is Map && parsed['name'] is String
          ? (parsed['name'] as String)
          : 'tool';
      filename = '${toolFileName(name.isEmpty ? 'tool' : name)}.json';
    }
    final error = saveToolContent(filename, content);
    if (error != null) {
      setState(() {
        _messageError = true;
        _message = '保存失败: $error';
      });
      return;
    }
    _reload();
    setState(() {
      _editing = false;
      _editingFilename = null;
      _message = null;
      _messageError = false;
    });
    _panelFocus.requestFocus();
  }

  void _delete(ToolEntry entry) {
    if (!_armed.contains(entry.filename)) {
      _armed.add(entry.filename);
      setState(() {});
      _armTimer?.cancel();
      _armTimer = Timer(const Duration(milliseconds: 2500), () {
        if (mounted) {
          setState(_armed.clear);
        }
      });
      return;
    }
    final error = deleteTool(entry.filename);
    _armed.remove(entry.filename);
    _reload();
    setState(() {
      _messageError = error != null;
      _message = error == null ? null : '删除失败: $error';
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
            Expanded(child: _editing ? _buildEditor() : _buildList()),
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
            '工具编辑器',
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

  Widget _buildList() {
    final config = _config;
    if (config == null) {
      return Center(
        child: Text(
          '不可用（rust 桥接未加载）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    final message = _message;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // v1 顶行：左「已加载 N 个工具」，右「+ 新建工具」。
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 2, 12, 8),
          child: Row(
            children: [
              Text(
                '已加载 ${config.entries.length} 个工具',
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 11.5,
                ),
              ),
              const Spacer(),
              SizedBox(
                height: 26,
                child: TextButton(
                  onPressed: _startNew,
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.text,
                    backgroundColor: LunacColors.surfaceHover,
                    side: BorderSide(color: LunacColors.divider),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(5),
                    ),
                    padding: const EdgeInsets.symmetric(horizontal: 10),
                    minimumSize: Size.zero,
                    visualDensity: VisualDensity.compact,
                  ),
                  child: const Text('+ 新建工具', style: TextStyle(fontSize: 11)),
                ),
              ),
            ],
          ),
        ),
        if (message != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 6),
            child: Text(
              message,
              style: TextStyle(
                color: _messageError ? LunacColors.danger : LunacColors.textMuted,
                fontSize: 11,
              ),
            ),
          ),
        if (!config.ok)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 6),
            child: Text(
              config.error,
              style: TextStyle(color: LunacColors.danger, fontSize: 11),
            ),
          ),
        Expanded(
          child: config.entries.isEmpty
              ? Center(
                  // 与 v1 `tooleditor.no_tools`（zh-CN）逐字一致。
                  child: Text(
                    '暂无自定义工具。创建工具以扩展 AI Agent 能力。',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 13,
                    ),
                  ),
                )
              : ListView.builder(
                  padding: const EdgeInsets.only(bottom: 8),
                  itemCount: config.entries.length,
                  itemBuilder: (context, index) =>
                      _buildRow(config.entries[index]),
                ),
        ),
      ],
    );
  }

  /// v1 `.tool-item`：图标 / 名称 + 徽标 / 描述 → 编辑 / 删除。
  Widget _buildRow(ToolEntry entry) {
    final armed = _armed.contains(entry.filename);
    return InkWell(
      hoverColor: LunacColors.surfaceHover,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        child: Row(
          children: [
            const Text('🔧', style: TextStyle(fontSize: 14)),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Flexible(
                        child: Text(
                          entry.displayName,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: LunacColors.text,
                            fontSize: 12.5,
                          ),
                        ),
                      ),
                      const SizedBox(width: 6),
                      _badge(
                        entry.valid ? '有效' : '无效',
                        entry.valid ? LunacColors.accent : LunacColors.danger,
                      ),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    entry.description.isEmpty ? '（无描述）' : entry.description,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 11,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            _smallButton('编辑', onPressed: () => _startEdit(entry)),
            const SizedBox(width: 4),
            _smallButton(
              armed ? '再次点击确认删除' : '删除',
              onPressed: () => _delete(entry),
              danger: true,
              filled: armed,
            ),
          ],
        ),
      ),
    );
  }

  /// v1 `.tool-editor`：标题 + 返回 → JSON 文本区 → 消息 → 保存 / 取消。
  Widget _buildEditor() {
    final title = _editingFilename == null
        ? '新建工具'
        : '编辑: ${_editingFilename!}';
    final message = _message;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 2, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              _smallButton('返回', onPressed: _closeEditor),
            ],
          ),
          const SizedBox(height: 8),
          Expanded(
            child: Container(
              decoration: BoxDecoration(
                color: LunacColors.background,
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.circular(6),
              ),
              child: TextField(
                controller: _editorText,
                maxLines: null,
                expands: true,
                textAlignVertical: TextAlignVertical.top,
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12,
                  height: 1.5,
                  fontFamily: 'Consolas',
                ),
                cursorColor: LunacColors.accent,
                decoration: const InputDecoration(
                  isCollapsed: true,
                  border: InputBorder.none,
                  contentPadding: EdgeInsets.all(10),
                ),
              ),
            ),
          ),
          if (message != null) ...[
            const SizedBox(height: 6),
            Text(
              message,
              style: TextStyle(
                color: _messageError
                    ? LunacColors.danger
                    : LunacColors.textMuted,
                fontSize: 11,
              ),
            ),
          ],
          const SizedBox(height: 8),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              _smallButton(
                '保存',
                onPressed: _save,
                highlight: true,
              ),
              const SizedBox(width: 6),
              _smallButton('取消', onPressed: _closeEditor),
            ],
          ),
        ],
      ),
    );
  }

  Widget _badge(String text, Color color) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(4),
        border: Border.all(color: color.withAlpha(128)),
      ),
      child: Text(text, style: TextStyle(color: color, fontSize: 9.5)),
    );
  }

  Widget _smallButton(
    String label, {
    required VoidCallback onPressed,
    bool highlight = false,
    bool danger = false,
    bool filled = false,
  }) {
    return SizedBox(
      height: 26,
      child: TextButton(
        onPressed: onPressed,
        style: TextButton.styleFrom(
          foregroundColor: filled
              ? Colors.white
              : danger
                  ? LunacColors.danger
                  : highlight
                      ? LunacColors.text
                      : LunacColors.textDim,
          backgroundColor: filled
              ? LunacColors.danger
              : highlight
                  ? LunacColors.surfaceHover
                  : LunacColors.background,
          side: BorderSide(
            color: filled
                ? LunacColors.danger
                : highlight
                    ? LunacColors.divider
                    : LunacColors.border,
          ),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(4),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 9),
          minimumSize: Size.zero,
          visualDensity: VisualDensity.compact,
        ),
        child: Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(fontSize: 10.5),
        ),
      ),
    );
  }
}
