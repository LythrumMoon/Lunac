// 目录选择器（2026-11-19 重做前端）。
//
// 背景：此前「添加音乐目录」用的是 shell32 的 SHBrowseForFolderW —— 那是 Win95 时代
// 的树形小窗，不能输入路径、不支持 UNC / 网络位置，观感也与 v2 面板差得远。
// 这里用 Flutter 自己画：盘符列表 → 逐级下钻 + 可直接输入路径（NAS / 网络盘）。
//
// 边界：只做「列目录 + 选一个已存在的目录」，不创建 / 不删除 / 不改名。
// 读写全部走 `dart:io`，没有跨平台宿主依赖（Linux 上盘符列表为空，可手输路径）。
import 'dart:io';

import 'package:flutter/material.dart';

import '../theme.dart';

/// 弹目录选择器；返回选中的绝对路径，取消 / 无选择时返回 null。
Future<String?> pickFolderDialog(
  BuildContext context, {
  String title = '选择文件夹',
  String initialPath = '',
}) {
  return showDialog<String>(
    context: context,
    barrierDismissible: false,
    builder: (_) => _FolderPickerDialog(title: title, initialPath: initialPath),
  );
}

class _FolderPickerDialog extends StatefulWidget {
  const _FolderPickerDialog({required this.title, required this.initialPath});

  final String title;
  final String initialPath;

  @override
  State<_FolderPickerDialog> createState() => _FolderPickerDialogState();
}

class _FolderPickerDialogState extends State<_FolderPickerDialog> {
  late final TextEditingController _pathText;
  String _path = '';
  List<String> _folders = const [];
  String _error = '';
  bool _loading = false;

  @override
  void initState() {
    super.initState();
    final initial = _normalize(widget.initialPath);
    _path = Directory(initial).existsSync() ? initial : '';
    _pathText = TextEditingController(text: _path);
    _reload();
  }

  @override
  void dispose() {
    _pathText.dispose();
    super.dispose();
  }

  /// 去掉尾部 `\` / `/`（盘符根保留 `C:\`）。
  static String _normalize(String input) {
    var text = input.trim();
    if (text.isEmpty) {
      return '';
    }
    text = text.replaceAll('/', '\\');
    while (text.length > 3 && text.endsWith('\\')) {
      text = text.substring(0, text.length - 1);
    }
    return text;
  }

  Future<void> _reload() async {
    setState(() {
      _loading = true;
      _error = '';
    });
    final folders = <String>[];
    var error = '';
    if (_path.isEmpty) {
      // 盘符：只收真实存在的（空读卡器 / 未分配盘符会跳过）。
      for (var code = 'A'.codeUnitAt(0); code <= 'Z'.codeUnitAt(0); code++) {
        final root = '${String.fromCharCode(code)}:\\';
        try {
          if (Directory(root).existsSync()) {
            folders.add(root);
          }
        } catch (_) {
          // 没权限 / 设备未就绪：当作不存在。
        }
      }
    } else {
      try {
        await for (final entry in Directory(_path).list(followLinks: false)) {
          if (entry is Directory) {
            folders.add(entry.path);
          }
        }
        folders.sort(
          (a, b) => _leaf(a).toLowerCase().compareTo(_leaf(b).toLowerCase()),
        );
      } catch (e) {
        error = '读不了这个目录：${_reason(e)}';
      }
    }
    if (!mounted) {
      return;
    }
    setState(() {
      _loading = false;
      _folders = folders;
      _error = error;
    });
  }

  static String _leaf(String path) {
    final text = path.replaceAll('/', '\\');
    final parts = text.split('\\')..removeWhere((item) => item.isEmpty);
    if (parts.isEmpty) {
      return text;
    }
    // `C:\` 这种只有盘符。
    return parts.length == 1 ? '${parts.first}\\' : parts.last;
  }

  static String _reason(Object error) {
    final text = error.toString();
    if (text.contains('Access is denied') ||
        text.contains('拒绝访问') ||
        text.contains('errno = 5')) {
      return '没有访问权限';
    }
    if (text.contains('does not exist') || text.contains('找不到')) {
      return '目录不存在';
    }
    return '路径不可用';
  }

  bool get _canChoose => _path.isNotEmpty && _error.isEmpty && !_loading;

  bool get _atDriveRoot {
    final text = _path.replaceAll('/', '\\');
    return text.length <= 3 && text.endsWith('\\');
  }

  void _go(String path) {
    final next = _normalize(path);
    _path = next;
    _pathText.text = next;
    _reload();
  }

  void _up() {
    if (_path.isEmpty) {
      return;
    }
    if (_atDriveRoot) {
      _go('');
      return;
    }
    final parent = Directory(_path).parent.path;
    _go(parent == _path ? '' : parent);
  }

  @override
  Widget build(BuildContext context) {
    return Dialog(
      backgroundColor: LunacColors.surface,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(10),
        side: BorderSide(color: LunacColors.border),
      ),
      child: SizedBox(
        width: 580,
        height: 520,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 14, 12, 8),
              child: Row(
                children: [
                  Icon(
                    Icons.folder_open_outlined,
                    size: 18,
                    color: LunacColors.accent,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      widget.title,
                      style: TextStyle(
                        color: LunacColors.text,
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 12, 8),
              child: Row(
                children: [
                  IconButton(
                    tooltip: '上一级',
                    onPressed: _path.isEmpty ? null : _up,
                    icon: const Icon(Icons.arrow_upward, size: 16),
                    color: LunacColors.textDim,
                    visualDensity: VisualDensity.compact,
                  ),
                  Expanded(
                    child: TextField(
                      controller: _pathText,
                      style: TextStyle(color: LunacColors.text, fontSize: 12),
                      decoration: InputDecoration(
                        isDense: true,
                        hintText: r'输入路径，例如 D:\Music 或 \\NAS\share',
                        hintStyle: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 11,
                        ),
                        contentPadding: const EdgeInsets.symmetric(
                          horizontal: 10,
                          vertical: 8,
                        ),
                        filled: true,
                        fillColor: LunacColors.surfaceHover,
                        border: OutlineInputBorder(
                          borderRadius: BorderRadius.circular(6),
                          borderSide: BorderSide.none,
                        ),
                      ),
                      onSubmitted: _go,
                    ),
                  ),
                  const SizedBox(width: 6),
                  TextButton(
                    onPressed: () => _go(_pathText.text),
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.accent,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('前往', style: TextStyle(fontSize: 12)),
                  ),
                ],
              ),
            ),
            if (_error.isNotEmpty)
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 6),
                child: Text(
                  _error,
                  style: TextStyle(color: LunacColors.danger, fontSize: 11),
                ),
              ),
            Expanded(
              child: Container(
                margin: const EdgeInsets.symmetric(horizontal: 16),
                decoration: BoxDecoration(
                  color: LunacColors.surfaceHover,
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(color: LunacColors.border),
                ),
                child: _loading
                    ? const Center(
                        child: SizedBox(
                          width: 18,
                          height: 18,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        ),
                      )
                    : _buildList(),
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 10, 16, 12),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      _path.isEmpty ? '选择一个盘符或输入路径' : _path,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 10.5,
                      ),
                    ),
                  ),
                  const SizedBox(width: 8),
                  TextButton(
                    onPressed: () => Navigator.of(context).pop(),
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.textDim,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('取消', style: TextStyle(fontSize: 12)),
                  ),
                  const SizedBox(width: 4),
                  TextButton(
                    onPressed: _canChoose
                        ? () => Navigator.of(context).pop(_path)
                        : null,
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.accent,
                      backgroundColor: LunacColors.surfaceHover,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text(
                      '选择此文件夹',
                      style: TextStyle(fontSize: 12),
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

  Widget _buildList() {
    final items = <Widget>[];
    if (_path.isNotEmpty) {
      items.add(
        _row(
          icon: Icons.drive_file_move_outline,
          label: '..',
          subtitle: '上一级',
          onTap: _up,
        ),
      );
    }
    for (final folder in _folders) {
      items.add(
        _row(
          icon: _path.isEmpty ? Icons.storage : Icons.folder_outlined,
          label: _path.isEmpty ? folder : _leaf(folder),
          subtitle: folder,
          onTap: () => _go(folder),
        ),
      );
    }
    if (items.isEmpty) {
      return Center(
        child: Text(
          _path.isEmpty ? '没有可用的盘符' : '这个目录里没有子文件夹',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
        ),
      );
    }
    return ListView(children: items);
  }

  Widget _row({
    required IconData icon,
    required String label,
    required String subtitle,
    required VoidCallback onTap,
  }) {
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(4),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
        child: Row(
          children: [
            Icon(icon, size: 15, color: LunacColors.textDim),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: LunacColors.text, fontSize: 12),
                  ),
                  Text(
                    subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 9.5,
                    ),
                  ),
                ],
              ),
            ),
            Icon(Icons.chevron_right, size: 14, color: LunacColors.textMuted),
          ],
        ),
      ),
    );
  }
}
