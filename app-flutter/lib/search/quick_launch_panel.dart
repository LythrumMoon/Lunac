// 快捷启动面板（v1 `quick-launch` 插件的原生等价物）。
//
// 版式对齐 v1 `app/src/plugins/builtin/quick-launch.ts`：
//   顶栏：标题「快速启动」+ 右侧「添加启动项」（系统多选框，选中即注册）；
//   列表：每条是搜索结果那种一行（图标 / 名称 / 路径 / 徽标「已注册」或「本次会话」/ × 移除）；
//   空态：v1 `quicklaunch.empty` 原文。
//
// 注册时机与 v1 一致：搜索栏附件在**打开本面板时**才补注册（粘贴 ≠ 要进启动项），
// 未注册的附件以「本次会话」徽标列出，点开仍可直接启动。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/app_window.dart';
import '../platform/quick_launch.dart';
import '../platform/shell_actions.dart';
import '../theme.dart';
import 'attachment_chip.dart';

/// v1 `quicklaunch.empty` 原文。
const String _emptyText =
    '还没有自定义启动项：点上方“添加启动项”选择文件/文件夹；粘贴或拖入搜索栏的文件，'
    '在点开本插件时才会注册。已注册的项会固定显示在此，重启不丢失。';

/// 快捷启动面板：搜索「快捷启动 / launch」进入。
class QuickLaunchPanel extends StatefulWidget {
  const QuickLaunchPanel({
    super.key,
    required this.onClose,
    required this.attachments,
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 当前附件（拖入 / 粘贴的文件路径）；打开面板时补注册（v1 同口径）。
  final List<String> attachments;

  @override
  State<QuickLaunchPanel> createState() => _QuickLaunchPanelState();
}

/// 列表里的一行：已注册项，或本次附件里尚未注册的项。
class _QuickRow {
  const _QuickRow({
    required this.name,
    required this.path,
    required this.registered,
  });

  final String name;
  final String path;
  final bool registered;
}

class _QuickLaunchPanelState extends State<QuickLaunchPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'quick-launch-panel',
    onKeyEvent: _onKey,
  );

  List<CustomApp>? _apps = readCustomApps();

  /// 本次会话里被 × 移除的附件（v1 会顺带摘掉主窗的附件气泡；v2 只在本面板隐藏）。
  final Set<String> _hidden = <String>{};

  @override
  void initState() {
    super.initState();
    // v1 `main.ts::executePlugin`：打开本插件时把当前附件逐个补注册（已注册的会被跳过）。
    for (final path in widget.attachments) {
      addCustomApp('', path);
    }
    _apps = readCustomApps() ?? _apps;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
  }

  @override
  void dispose() {
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

  /// 已注册项在前，本次附件（未注册）在后；按 path 小写去重。
  List<_QuickRow> _rows() {
    final apps = _apps;
    if (apps == null) {
      return const [];
    }
    final rows = <_QuickRow>[];
    final seen = <String>{};
    for (final app in apps) {
      if (_hidden.contains(app.path.toLowerCase())) {
        continue;
      }
      if (seen.add(app.path.toLowerCase())) {
        rows.add(
          _QuickRow(name: app.name, path: app.path, registered: true),
        );
      }
    }
    for (final path in widget.attachments) {
      if (_hidden.contains(path.toLowerCase())) {
        continue;
      }
      if (seen.add(path.toLowerCase())) {
        rows.add(
          _QuickRow(name: fileNameOf(path), path: path, registered: false),
        );
      }
    }
    return rows;
  }

  /// 「添加启动项」：系统多选框 → 逐条注册 → 刷新列表。
  void _pickAndAdd() {
    final paths = pickLaunchFiles();
    if (paths.isEmpty) {
      return;
    }
    for (final path in paths) {
      addCustomApp('', path);
    }
    if (!mounted) {
      return;
    }
    setState(() => _apps = readCustomApps() ?? _apps);
  }

  /// v1 `×`：注销注册表项；未注册的本次附件只从本面板隐藏。
  void _remove(_QuickRow row) {
    if (row.registered) {
      removeCustomApp(row.path);
    } else {
      _hidden.add(row.path.toLowerCase());
    }
    if (!mounted) {
      return;
    }
    setState(() => _apps = readCustomApps() ?? _apps);
  }

  /// 先隐藏主窗，再用系统默认程序打开（与 v1 一致）。
  void _launch(String path) {
    unawaited(hideMainWindow());
    openPath(path);
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

  /// v1 `.ql-toolbar`：标题 + 「添加启动项」。
  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(6, 6, 12, 6),
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
            '快捷启动',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const Spacer(),
          SizedBox(
            height: 26,
            child: TextButton(
              onPressed: _pickAndAdd,
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.textDim,
                backgroundColor: LunacColors.background,
                side: BorderSide(color: LunacColors.border),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(5),
                ),
                padding: const EdgeInsets.symmetric(horizontal: 10),
                minimumSize: Size.zero,
                visualDensity: VisualDensity.compact,
              ),
              child: const Row(
                children: [
                  Icon(Icons.add, size: 12),
                  SizedBox(width: 4),
                  Text('添加启动项', style: TextStyle(fontSize: 11)),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildBody() {
    if (_apps == null) {
      return Center(
        child: Text(
          '不可用（rust 桥接未加载）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    final rows = _rows();
    if (rows.isEmpty) {
      return Padding(
        padding: EdgeInsets.fromLTRB(12, 8, 12, 12),
        child: Text(
          _emptyText,
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 12,
            height: 1.5,
          ),
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.only(top: 2, bottom: 8),
      itemCount: rows.length,
      itemBuilder: (context, index) => _buildRow(rows[index]),
    );
  }

  /// v1 用搜索结果那样的 `.result-item` 行（图标 / 名称 / 路径 / × / 徽标）。
  Widget _buildRow(_QuickRow row) {
    return InkWell(
      onTap: () => _launch(row.path),
      hoverColor: LunacColors.surfaceHover,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 6, 6, 6),
        child: Row(
          children: [
            Icon(
              Icons.push_pin_outlined,
              size: 16,
              color: LunacColors.textMuted,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    row.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.text,
                      fontSize: 12.5,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    row.path,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10.5,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            _buildBadge(
              row.registered ? '已注册' : '本次会话',
              accent: row.registered,
            ),
            const SizedBox(width: 4),
            Tooltip(
              message: '移除该启动项（同时注销）',
              child: InkWell(
                onTap: () => _remove(row),
                borderRadius: BorderRadius.circular(4),
                child: Padding(
                  padding: EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                  child: Icon(
                    Icons.close,
                    size: 12,
                    color: LunacColors.textMuted,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildBadge(String label, {required bool accent}) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: accent ? LunacColors.divider : LunacColors.border,
        ),
      ),
      child: Text(
        label,
        style: TextStyle(
          color: accent ? LunacColors.textDim : LunacColors.textMuted,
          fontSize: 10,
        ),
      ),
    );
  }
}
