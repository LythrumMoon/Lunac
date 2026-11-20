// 沙箱盘预览面板（2026-11-13，用户需求：依附 AI 助手的「小虚拟机」预览窗）。
//
// **口径（必须如实）**：这不是 OS 级隔离，也不是虚拟机。盘 = agent 的受控工作目录
// `<exe 根>/temp/transStorage`（cwd + `--add-dir` 都只有它，见 `rust/src/agent.rs`）；
// 面板只读地列目录、预览文件、显示固定额度与真实用量，并提供「打开目录 / 清空」；
// 固定额度可在用量条下方「调整额度」（256 MB / 512 MB / 1 GB / 2 GB，写
// `config/sandbox.json` 的 `quotaBytes`，Rust 侧夹取到 64 MiB–8 GiB）。
// 文案里不写「隔离环境」，写「沙箱盘 / 受控工作目录」。
//
// 数据与统计在 `rust/src/sandbox.rs`；本文件只做界面与轮询（2 秒一轮）。
import 'dart:async';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/app_preview.dart';
import '../platform/change_review.dart';
import '../platform/sandbox.dart';
import '../platform/shell_actions.dart';
import '../platform/terminal.dart';
import '../theme.dart';
import 'terminal_panel.dart';

class SandboxPanel extends StatefulWidget {
  const SandboxPanel({
    super.key,
    this.onClose,
    this.loadStatus,
    this.loadListing,
    this.loadFile,
    this.pullTabs,
    this.loadChangeDiff,
    this.windowControls,
  });

  /// 关闭窗口（窗口壳传 `close` 通道；内嵌使用时为 null）。
  final VoidCallback? onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 不再画「关闭」返回键，三个窗口键常驻右上角。
  final Widget? windowControls;

  /// 数据源（测试注入；默认走 rust 桥接）。
  final SandboxStatus? Function()? loadStatus;
  final SandboxListing? Function()? loadListing;
  final SandboxFile? Function(String path)? loadFile;

  /// 待开标签的来源（测试注入；默认走 Rust 进程内队列）。
  final List<PreviewTabRequest> Function()? pullTabs;

  /// 改动文件的 diff（测试注入；默认走 `change_review` 桥接）。
  final ChangeDiff? Function(String path)? loadChangeDiff;

  @override
  State<SandboxPanel> createState() => _SandboxPanelState();
}

class _SandboxPanelState extends State<SandboxPanel>
    with SingleTickerProviderStateMixin {
  SandboxStatus? _status;
  SandboxListing? _listing;
  String? _selected;
  SandboxFile? _file;
  Timer? _timer;
  Timer? _previewTimer;

  /// 预览标签（VS Code 风格）：`workspace` 常驻，文件 / 应用 / URL 按需加。
  final List<_PreviewTab> _tabs = [
    const _PreviewTab(id: 'workspace', title: '工作区', kind: 'workspace'),
  ];
  String _activeTabId = 'workspace';

  /// 文件标签的预览缓存（按 `file:<相对路径>` 存）。
  final Map<String, SandboxFile?> _tabFiles = {};

  /// 改动文件（`change:` 标签）的 diff 缓存。
  final Map<String, ChangeDiff?> _changeDiffs = {};

  /// 折叠的目录（资源管理器树）。
  final Set<String> _collapsedDirs = {};

  /// 终端面板（用户 2026-11-15：不做成标签，放在文件显示区**下方**）。
  ///
  /// `_terminalEverOpened` 只用于「点过才起进程」——面板一打开就 spawn 一个 shell 不合适。
  /// 收起时组件仍挂在树上（`Visibility` + `maintainState`），所以输出继续进缓冲、展开即续上。
  bool _terminalOpen = false;
  bool _terminalEverOpened = false;
  double _terminalHeight = 220;
  static const double _terminalMinHeight = 90;
  static const double _terminalMaxHeight = 560;

  /// 工作区文件列表的宽度（逻辑像素）：可左右拖，拖到最小直接收起（0），
  /// 收起后点左侧最上方的「工作区」图标恢复默认宽度（用户 2026-11-14 要求）。
  static const double _explorerDefaultWidth = 210;
  static const double _explorerMinWidth = 110;
  static const double _explorerMaxWidth = 420;
  double _explorerWidth = _explorerDefaultWidth;

  /// 标签条的横向滚动（滚轮 / 下方滚动条共用）。
  final ScrollController _tabScroll = ScrollController();

  String? _message;
  bool _error = false;
  bool _confirmClear = false;
  Timer? _confirmTimer;
  bool _busy = false;

  /// 额度调整条（v2 追加：固定额度可改，写 `config/sandbox.json`）。
  bool _editingQuota = false;

  /// 应用预览（一档缩略图 / 二档停靠）。
  AppCapture? _appShot;
  Uint8List? _appBytes;
  String? _appError;
  bool _appDocked = false;
  Timer? _appTimer;
  final GlobalKey _appBodyKey = GlobalKey();

  /// 「从粘合线向左拉开」的展开动画（只在独立窗口里播；窗口本身已经是最终尺寸，
  /// 动画只做内容裁剪，避免逐帧 SetWindowPos 重建 swapchain）。
  late final AnimationController _reveal = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 180),
    value: 1,
  );

  @override
  void initState() {
    super.initState();
    _refresh();
    // 与详细搜索同频率：2 秒一轮，窗口关闭即停。
    _timer = Timer.periodic(const Duration(seconds: 2), (_) => _refresh());
    // agent 的 `Preview` 工具事件经 Rust 进程内队列转交；1 秒取一次。
    _previewTimer = Timer.periodic(
      const Duration(seconds: 1),
      (_) => _pullPreviewTabs(),
    );
    // 应用标签的缩略图：1.5 秒抓一帧（低频，几乎不占 CPU / GPU）。
    _appTimer = Timer.periodic(
      const Duration(milliseconds: 1500),
      (_) => _refreshAppShot(),
    );
    // 独立窗口（窗口壳传了 onClose）第一次出现时播展开动画；内嵌面板不播。
    if (widget.onClose != null) {
      _reveal.value = 0;
      _reveal.forward();
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _previewTimer?.cancel();
    _confirmTimer?.cancel();
    _appTimer?.cancel();
    if (_appDocked) {
      unawaited(undockAppWindow());
    }
    // 终端是进程级单例：切标签不关（`TerminalView` 卸载后 Rust 继续缓冲输出），
    // 只有整个预览窗关掉时才收掉 shell。
    terminalStop();
    _reveal.dispose();
    _tabScroll.dispose();
    super.dispose();
  }

  void _refresh() {
    final status = _readStatus();
    final listing = _readListing();
    if (!mounted) {
      return;
    }
    final selected = _selected;
    SandboxFile? file;
    if (selected != null) {
      final stillThere = listing?.entries.any(
        (entry) => entry.path == selected && !entry.isDir,
      );
      if (stillThere == true) {
        file = _readFile(selected);
      }
    }
    setState(() {
      _status = status;
      _listing = listing;
      if (file == null) {
        _selected = null;
        _file = null;
      } else {
        _file = file;
      }
    });
  }

  SandboxStatus? _readStatus() =>
      widget.loadStatus?.call() ?? readSandboxStatus();

  SandboxListing? _readListing() =>
      widget.loadListing?.call() ?? readSandboxListing();

  SandboxFile? _readFile(String path) =>
      widget.loadFile?.call(path) ?? readSandboxFile(path);

  /// 改固定额度：写盘后立即刷新用量条。
  void _applyQuota(int bytes) {
    final result = setSandboxQuota(bytes);
    if (!mounted) {
      return;
    }
    setState(() {
      _editingQuota = false;
      _error = !result.ok;
      _message = result.ok
          ? '额度已改为 ${_formatBytes(result.quotaBytes)}'
          : '设置额度失败：${result.error}';
    });
    _refresh();
  }

  void _select(SandboxEntry entry) {
    if (entry.isDir) {
      return;
    }
    _selectPath(entry.path);
  }

  void _clear() {
    if (_busy) {
      return;
    }
    if (!_confirmClear) {
      setState(() {
        _confirmClear = true;
        _message = '再点一次「确认清空」：沙箱盘里的文件会全部删除';
        _error = false;
      });
      _confirmTimer?.cancel();
      _confirmTimer = Timer(const Duration(seconds: 4), () {
        if (!mounted) {
          return;
        }
        setState(() => _confirmClear = false);
      });
      return;
    }
    _confirmTimer?.cancel();
    setState(() {
      _busy = true;
      _confirmClear = false;
      _message = '正在清空…';
      _error = false;
    });
    final result = clearSandbox();
    if (!mounted) {
      return;
    }
    setState(() {
      _busy = false;
      _error = !result.ok;
      _message = result.ok
          ? '已清空（删除 ${result.removed} 项）'
          : '清空未完成（失败 ${result.failed} 项）：${result.error}';
      _selected = null;
      _file = null;
    });
    _refresh();
  }

  String? get _rootPath => _status?.path.isNotEmpty == true
      ? _status!.path
      : _listing?.path.isNotEmpty == true
      ? _listing!.path
      : null;

  @override
  Widget build(BuildContext context) {
    final panel = _buildPanelBody();
    if (widget.onClose == null) {
      return panel;
    }
    // 独立窗口：面板从粘合线（窗口右缘）向左「拉开」，窗口本身已在最终尺寸。
    return AnimatedBuilder(
      animation: _reveal,
      builder: (context, child) {
        final t = Curves.easeOutCubic.transform(_reveal.value);
        if (t >= 0.999) {
          return child!;
        }
        final size = MediaQuery.of(context).size;
        return Align(
          alignment: Alignment.centerRight,
          child: ClipRect(
            child: SizedBox(
              width: size.width * t,
              height: size.height,
              child: OverflowBox(
                alignment: Alignment.centerRight,
                minWidth: size.width,
                maxWidth: size.width,
                minHeight: size.height,
                maxHeight: size.height,
                child: child,
              ),
            ),
          ),
        );
      },
      child: panel,
    );
  }

  Widget _buildPanelBody() {
    return Container(
      decoration: BoxDecoration(
        color: LunacColors.surface,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(12),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          _buildHeader(),
          Divider(height: 1, color: LunacColors.border),
          Expanded(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (_explorerWidth > 1) ...[
                  SizedBox(width: _explorerWidth, child: _buildExplorer()),
                  _buildExplorerSplitter(),
                ] else
                  _buildExplorerRail(),
                Expanded(
                  child: Column(
                    children: [
                      _buildTabBar(),
                      Divider(height: 1, color: LunacColors.border),
                      Expanded(child: _buildActiveTab()),
                      if (_message != null) _buildMessage(),
                      // 终端：文件显示区下方的常驻面板（用户 2026-11-15）。
                      if (_terminalEverOpened) ...[
                        if (_terminalOpen) _buildTerminalSplitter(),
                        Visibility(
                          visible: _terminalOpen,
                          maintainState: true,
                          child: SizedBox(
                            height: _terminalHeight,
                            child: const TerminalView(),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildHeader() {
    final root = _rootPath;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 8, 12, 6),
      child: Row(
        children: [
          // 内嵌模式才画「关闭」返回键；独立窗的三个窗口键放**右上角**（用户 2026-11-20 口径）。
          if (widget.windowControls == null && widget.onClose != null)
            IconButton(
              tooltip: '关闭（Esc）',
              icon: const Icon(Icons.arrow_back, size: 18),
              color: LunacColors.textDim,
              visualDensity: VisualDensity.compact,
              onPressed: widget.onClose,
            ),
          const SizedBox(width: 2),
          Text(
            '沙箱盘',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          if (root != null)
            Expanded(
              child: Tooltip(
                message: root,
                child: Text(
                  root,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
                ),
              ),
            )
          else
            const Spacer(),
          IconButton(
            tooltip: '刷新',
            icon: const Icon(Icons.refresh, size: 17),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: _refresh,
          ),
          IconButton(
            tooltip: '打开目录',
            icon: const Icon(Icons.folder_open, size: 17),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: root == null
                ? null
                : () {
                    if (!openPath(root)) {
                      setState(() {
                        _error = true;
                        _message = '打开目录失败：$root';
                      });
                    }
                  },
          ),
          TextButton(
            onPressed: _busy ? null : _clear,
            style: TextButton.styleFrom(
              foregroundColor: _confirmClear
                  ? LunacColors.danger
                  : LunacColors.textDim,
              visualDensity: VisualDensity.compact,
            ),
            child: Text(
              _confirmClear ? '确认清空' : '清空',
              style: const TextStyle(fontSize: 12),
            ),
          ),
          // 独立窗：置顶 / 最小化 / 关闭 常驻右上角。
          if (widget.windowControls != null) ...[
            const SizedBox(width: 2),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  Widget _buildUsage() {
    final status = _status;
    final listing = _listing;
    if (status == null && listing == null) {
      return Padding(
        padding: EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        child: Text(
          '不可用（rust 桥接未加载）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    }
    final used = status?.usedBytes ?? listing?.usedBytes ?? 0;
    final quota = status?.quotaBytes ?? listing?.quotaBytes ?? 0;
    final files = status?.fileCount ?? 0;
    final dirs = status?.dirCount ?? 0;
    final over = quota > 0 && used > quota;
    final ratio = quota > 0 ? (used / quota).clamp(0.0, 1.0).toDouble() : 0.0;
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 8, 14, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text(
                '${_formatBytes(used)} / ${_formatBytes(quota)}',
                style: TextStyle(
                  color: over ? LunacColors.danger : LunacColors.textDim,
                  fontSize: 12,
                ),
              ),
              const SizedBox(width: 10),
              Text(
                '$files 个文件 · $dirs 个目录',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
              const Spacer(),
              if (over)
                Text(
                  '已超过固定额度',
                  style: TextStyle(color: LunacColors.danger, fontSize: 11),
                )
              else if (status?.truncated == true)
                Text(
                  '条目过多，用量为下限',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
                ),
            ],
          ),
          const SizedBox(height: 6),
          ClipRRect(
            borderRadius: BorderRadius.circular(2),
            child: LinearProgressIndicator(
              value: ratio,
              minHeight: 3,
              backgroundColor: LunacColors.border,
              color: over ? LunacColors.danger : LunacColors.accent,
            ),
          ),
          const SizedBox(height: 4),
          Row(
            children: [
              Expanded(
                child: Text(
                  '这是 AI 的受控工作目录（固定额度的小盘），不是 OS 级隔离环境。',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              ),
              TextButton(
                onPressed: () => setState(() => _editingQuota = !_editingQuota),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                  minimumSize: Size.zero,
                  padding: const EdgeInsets.symmetric(horizontal: 6),
                ),
                child: Text(
                  _editingQuota ? '收起' : '调整额度',
                  style: const TextStyle(fontSize: 10.5),
                ),
              ),
            ],
          ),
          if (_editingQuota) ...[
            const SizedBox(height: 2),
            Wrap(
              spacing: 6,
              runSpacing: 4,
              children: [
                _quotaChip('256 MB', 256 * 1024 * 1024),
                _quotaChip('512 MB', 512 * 1024 * 1024),
                _quotaChip('1 GB', 1024 * 1024 * 1024),
                _quotaChip('2 GB', 2 * 1024 * 1024 * 1024),
                TextButton(
                  onPressed: () => setState(() => _editingQuota = false),
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.textMuted,
                    visualDensity: VisualDensity.compact,
                    minimumSize: Size.zero,
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                  ),
                  child: const Text('取消', style: TextStyle(fontSize: 10.5)),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  /// 额度预设按钮（64 MiB–8 GiB 由 Rust 侧夹取）。
  Widget _quotaChip(String label, int bytes) {
    final current = (_status?.quotaBytes ?? _listing?.quotaBytes ?? 0) == bytes;
    return SizedBox(
      height: 24,
      child: TextButton(
        onPressed: () => _applyQuota(bytes),
        style: TextButton.styleFrom(
          foregroundColor: current ? LunacColors.text : LunacColors.textDim,
          backgroundColor: current
              ? LunacColors.surfaceHover
              : LunacColors.background,
          side: BorderSide(
            color: current ? LunacColors.divider : LunacColors.border,
          ),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
          padding: const EdgeInsets.symmetric(horizontal: 10),
          minimumSize: Size.zero,
          visualDensity: VisualDensity.compact,
        ),
        child: Text(label, style: const TextStyle(fontSize: 10.5)),
      ),
    );
  }

  /// 工作区与右侧标签区的分隔条：左右拖动改宽度，拖到最小直接收起。
  Widget _buildExplorerSplitter() {
    return MouseRegion(
      cursor: SystemMouseCursors.resizeLeftRight,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onHorizontalDragUpdate: (details) {
          setState(() {
            _explorerWidth = (_explorerWidth + details.delta.dx).clamp(
              0.0,
              _explorerMaxWidth,
            );
          });
        },
        onHorizontalDragEnd: (_) {
          if (_explorerWidth < _explorerMinWidth * 0.6) {
            setState(() => _explorerWidth = 0);
          }
        },
        child: Container(
          width: 5,
          alignment: Alignment.center,
          child: Container(width: 1, color: LunacColors.border),
        ),
      ),
    );
  }

  /// 工作区收起后的窄条：最上方就是「工作区」入口，点一下恢复默认宽度。
  Widget _buildExplorerRail() {
    return Container(
      width: 30,
      decoration: BoxDecoration(
        border: Border(right: BorderSide(color: LunacColors.border)),
      ),
      child: Column(
        children: [
          const SizedBox(height: 6),
          Tooltip(
            message: '工作区（点开文件列表）',
            child: InkWell(
              borderRadius: BorderRadius.circular(4),
              onTap: () =>
                  setState(() => _explorerWidth = _explorerDefaultWidth),
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: Icon(
                  Icons.folder_open,
                  size: 16,
                  color: LunacColors.textDim,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 左侧资源管理器（VS Code 风格）：目录可折叠、文件点开进右侧标签。
  Widget _buildExplorer() {
    final listing = _listing;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // 标题栏右键 = 在根目录新建 / 刷新 / 打开目录（空目录时没有行可右键）。
        GestureDetector(
          behavior: HitTestBehavior.opaque,
          onSecondaryTapDown: (details) =>
              _showRootMenu(details.globalPosition),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(10, 4, 4, 4),
            child: Row(
              children: [
                Icon(Icons.folder_open, size: 14, color: LunacColors.textDim),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    '工作区',
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                if (listing != null)
                  Text(
                    '${listing.entries.length}',
                    style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                  ),
                // 固定的「新建」入口（右键也能到，但不该只有右键一条路）。
                Builder(
                  builder: (buttonContext) => IconButton(
                    tooltip: '新建文件 / 文件夹',
                    icon: const Icon(Icons.add, size: 14),
                    color: LunacColors.textMuted,
                    visualDensity: VisualDensity.compact,
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints(
                      minWidth: 22,
                      minHeight: 22,
                    ),
                    onPressed: () {
                      final box =
                          buttonContext.findRenderObject() as RenderBox?;
                      final offset = box == null
                          ? Offset.zero
                          : box.localToGlobal(Offset.zero);
                      unawaited(_showRootMenu(offset));
                    },
                  ),
                ),
              ],
            ),
          ),
        ),
        Divider(height: 1, color: LunacColors.border),
        Expanded(
          child: listing == null
              ? Center(
                  child: Text(
                    '不可用（rust 桥接未加载）',
                    style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
                  ),
                )
              : _buildFileList(listing, _treeEntries),
        ),
      ],
    );
  }

  /// 资源管理器里可见的条目：折叠目录的后代全部隐藏。
  List<SandboxEntry> get _treeEntries {
    final entries = _listing?.entries ?? const <SandboxEntry>[];
    if (_collapsedDirs.isEmpty) {
      return entries;
    }
    return entries.where((entry) {
      for (final dir in _collapsedDirs) {
        if (entry.path.startsWith('$dir/')) {
          return false;
        }
      }
      return true;
    }).toList();
  }

  Widget _buildFileList(SandboxListing listing, List<SandboxEntry> entries) {
    if (entries.isEmpty) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Text(
            listing.error.isNotEmpty
                ? listing.error
                : '沙箱盘还是空的\nAI 在对话里生成的文件会出现在这里',
            textAlign: TextAlign.center,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.symmetric(vertical: 4),
      itemCount: entries.length,
      itemBuilder: (context, index) {
        final entry = entries[index];
        final selected = !entry.isDir && entry.path == _selected;
        final collapsed = _collapsedDirs.contains(entry.path);
        return Material(
          color: selected ? LunacColors.surfaceHover : Colors.transparent,
          child: InkWell(
            onTap: entry.isDir
                ? () => setState(() {
                    if (collapsed) {
                      _collapsedDirs.remove(entry.path);
                    } else {
                      _collapsedDirs.add(entry.path);
                    }
                  })
                : () => _select(entry),
            // 右键：文件操作（新建 / 重命名 / 删除 / 复制路径 / 资源管理器 / 添加到对话）。
            onSecondaryTapDown: (details) =>
                _showEntryMenu(entry, details.globalPosition),
            child: Padding(
              padding: EdgeInsets.only(
                left: 6 + entry.depth * 12.0,
                right: 8,
                top: 5,
                bottom: 5,
              ),
              child: Row(
                children: [
                  if (entry.isDir)
                    Icon(
                      collapsed ? Icons.chevron_right : Icons.expand_more,
                      size: 13,
                      color: LunacColors.textMuted,
                    )
                  else
                    const SizedBox(width: 13),
                  Icon(
                    entry.isDir
                        ? Icons.folder_outlined
                        : Icons.description_outlined,
                    size: 14,
                    color: entry.isDir
                        ? LunacColors.textDim
                        : LunacColors.textMuted,
                  ),
                  const SizedBox(width: 5),
                  Expanded(
                    child: Text(
                      entry.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: entry.isDir
                            ? LunacColors.textDim
                            : LunacColors.text,
                        fontSize: 12,
                      ),
                    ),
                  ),
                  if (!entry.isDir)
                    Text(
                      _formatBytes(entry.size),
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 10,
                      ),
                    ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  // ── 右键菜单（用户 2026-11-15：预览区做成「小 VSCode」）────────────────
  //
  // 两处入口共用同一套动作：左侧资源管理器的条目、顶部标签。区别只有作用对象
  // （条目 = 它自己；标签 = 它的 target），动作分支完全一致。

  /// 沙箱根绝对路径（复制路径 / 资源管理器 / 附件都要绝对路径）。
  String get _sandboxRoot => _listing?.path ?? _status?.path ?? '';

  String _absOf(String rel) {
    final root = _sandboxRoot;
    if (root.isEmpty || rel.isEmpty) {
      return rel;
    }
    return '$root\\${rel.replaceAll('/', '\\')}';
  }

  String _parentOf(String rel) {
    final index = rel.lastIndexOf('/');
    return index <= 0 ? '' : rel.substring(0, index);
  }

  String _joinRel(String dir, String name) {
    final clean = name.trim().replaceAll('\\', '/');
    return dir.isEmpty ? clean : '$dir/$clean';
  }

  void _setMessage(String? text, {bool error = false}) {
    setState(() {
      _message = text;
      _error = error;
    });
  }

  /// 弹出一组菜单项；返回选中的 value（点外面 / Esc ⇒ null）。
  Future<String?> _popupMenu(Offset position, List<PopupMenuEntry<String>> items) async {
    final overlay = Overlay.of(context).context.findRenderObject();
    if (overlay is! RenderBox) {
      return null;
    }
    return showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(position.dx, position.dy, 1, 1),
        Offset.zero & overlay.size,
      ),
      items: items,
    );
  }

  /// 资源管理器条目的右键菜单。
  Future<void> _showEntryMenu(SandboxEntry entry, Offset position) async {
    final action = await _popupMenu(position, [
      PopupMenuItem(value: 'open', child: Text(entry.isDir ? '展开 / 收起' : '打开')),
      const PopupMenuDivider(),
      const PopupMenuItem(value: 'new-file', child: Text('在此新建文件…')),
      const PopupMenuItem(value: 'new-dir', child: Text('在此新建文件夹…')),
      const PopupMenuItem(value: 'rename', child: Text('重命名…')),
      const PopupMenuItem(value: 'delete', child: Text('删除…')),
      const PopupMenuDivider(),
      const PopupMenuItem(value: 'copy', child: Text('复制路径')),
      const PopupMenuItem(value: 'reveal', child: Text('在资源管理器中显示')),
      if (!entry.isDir) ...[
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'attach', child: Text('添加到对话')),
      ],
    ]);
    if (!mounted || action == null) {
      return;
    }
    final dir = entry.isDir ? entry.path : _parentOf(entry.path);
    switch (action) {
      case 'open':
        if (entry.isDir) {
          setState(() {
            if (_collapsedDirs.contains(entry.path)) {
              _collapsedDirs.remove(entry.path);
            } else {
              _collapsedDirs.add(entry.path);
            }
          });
        } else {
          _select(entry);
        }
      case 'new-file':
        final name = await _promptText(
          title: '在 ${dir.isEmpty ? '工作区根目录' : dir} 新建文件',
          hint: '文件名（含扩展名）',
        );
        if (name != null && name.isNotEmpty) {
          await _createFile(dir, name);
        }
      case 'new-dir':
        final name = await _promptText(
          title: '在 ${dir.isEmpty ? '工作区根目录' : dir} 新建文件夹',
          hint: '文件夹名',
        );
        if (name != null && name.isNotEmpty) {
          await _createDir(dir, name);
        }
      case 'rename':
        final name = await _promptText(
          title: '重命名「${entry.name}」',
          initial: entry.name,
          hint: '新名字',
        );
        if (name != null && name.isNotEmpty && name != entry.name) {
          await _renameEntry(entry, name);
        }
      case 'delete':
        await _deleteEntry(entry);
      case 'copy':
        await Clipboard.setData(ClipboardData(text: _absOf(entry.path)));
        _setMessage('已复制路径：${_absOf(entry.path)}');
      case 'reveal':
        final ok = revealInExplorer(_absOf(entry.path));
        _setMessage(ok ? '已在资源管理器中定位' : '打开资源管理器失败', error: !ok);
      case 'attach':
        _attachToChat([_absOf(entry.path)]);
    }
  }

  /// 顶部标签的右键菜单（与条目菜单同一套动作，只是作用对象换成标签的 target）。
  Future<void> _showTabMenu(_PreviewTab tab, Offset position) async {
    // `change` 标签的 target 本来就是绝对路径；`file` 是相对沙箱根的路径。
    final abs = switch (tab.kind) {
      'file' => _absOf(tab.target),
      'change' => tab.target,
      _ => '',
    };
    final action = await _popupMenu(position, [
      const PopupMenuItem(value: 'close', child: Text('关闭')),
      PopupMenuItem(value: 'close-others', child: Text('关闭其他标签（${_tabs.length - 1}）')),
      if (abs.isNotEmpty) ...[
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'copy', child: Text('复制路径')),
        const PopupMenuItem(value: 'reveal', child: Text('在资源管理器中显示')),
        const PopupMenuItem(value: 'attach', child: Text('添加到对话')),
      ],
    ]);
    if (!mounted || action == null) {
      return;
    }
    switch (action) {
      case 'close':
        _closeTab(tab);
      case 'close-others':
        setState(() {
          _tabs.removeWhere((item) => item.id != tab.id);
          _activeTabId = tab.id;
        });
      case 'copy':
        await Clipboard.setData(ClipboardData(text: abs));
        _setMessage('已复制路径：$abs');
      case 'reveal':
        final ok = revealInExplorer(abs);
        _setMessage(ok ? '已在资源管理器中定位' : '打开资源管理器失败', error: !ok);
      case 'attach':
        _attachToChat([abs]);
    }
  }

  /// 工作区标题上的「新建」菜单（空目录时没有行可右键，需要一个固定入口）。
  Future<void> _showRootMenu(Offset position) async {
    final action = await _popupMenu(position, [
      const PopupMenuItem(value: 'new-file', child: Text('新建文件…')),
      const PopupMenuItem(value: 'new-dir', child: Text('新建文件夹…')),
      const PopupMenuDivider(),
      const PopupMenuItem(value: 'refresh', child: Text('刷新')),
      const PopupMenuItem(value: 'reveal', child: Text('在资源管理器中显示')),
    ]);
    if (!mounted || action == null) {
      return;
    }
    switch (action) {
      case 'new-file':
        final name = await _promptText(title: '新建文件', hint: '文件名（含扩展名）');
        if (name != null && name.isNotEmpty) {
          await _createFile('', name);
        }
      case 'new-dir':
        final name = await _promptText(title: '新建文件夹', hint: '文件夹名');
        if (name != null && name.isNotEmpty) {
          await _createDir('', name);
        }
      case 'refresh':
        _refresh();
      case 'reveal':
        final ok = revealInExplorer(_sandboxRoot);
        _setMessage(ok ? '已在资源管理器中打开沙箱盘' : '打开资源管理器失败', error: !ok);
    }
  }

  Future<String?> _promptText({
    required String title,
    String initial = '',
    String hint = '',
  }) {
    final controller = TextEditingController(text: initial);
    controller.selection = TextSelection(
      baseOffset: 0,
      extentOffset: initial.length,
    );
    return showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(title, style: TextStyle(color: LunacColors.text, fontSize: 13.5)),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: TextStyle(color: LunacColors.text, fontSize: 12.5),
          decoration: InputDecoration(hintText: hint, isDense: true),
          onSubmitted: (value) => Navigator.of(ctx).pop(value.trim()),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(controller.text.trim()),
            child: const Text('确定'),
          ),
        ],
      ),
    ).whenComplete(controller.dispose);
  }

  Future<bool> _confirm(String title, String body) async {
    final result = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(title, style: TextStyle(color: LunacColors.text, fontSize: 13.5)),
        content: Text(
          body,
          style: TextStyle(color: LunacColors.textDim, fontSize: 12, height: 1.5),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text('删除', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
    return result == true;
  }

  Future<void> _createFile(String dir, String name) async {
    final rel = _joinRel(dir, name);
    final result = sandboxWriteFile(rel, '');
    if (!result.ok) {
      _setMessage(result.error, error: true);
      return;
    }
    _refresh();
    _setMessage('已新建 $rel');
    _openFileTab(rel);
  }

  Future<void> _createDir(String dir, String name) async {
    final rel = _joinRel(dir, name);
    final result = sandboxMkdir(rel);
    if (!result.ok) {
      _setMessage(result.error, error: true);
      return;
    }
    setState(() => _collapsedDirs.remove(dir));
    _refresh();
    _setMessage('已新建文件夹 $rel');
  }

  Future<void> _renameEntry(SandboxEntry entry, String name) async {
    final target = _joinRel(_parentOf(entry.path), name);
    final result = sandboxRename(entry.path, target);
    if (!result.ok) {
      _setMessage(result.error, error: true);
      return;
    }
    // 路径变了 ⇒ 旧标签指向不存在的文件，直接关掉（比改 target 更不容易留错键）。
    final affected = _closeTabsUnder(entry.path);
    _refresh();
    _setMessage('已重命名为 $target${affected > 0 ? '（关闭了 $affected 个相关标签）' : ''}');
  }

  Future<void> _deleteEntry(SandboxEntry entry) async {
    final ok = await _confirm(
      '删除「${entry.name}」',
      entry.isDir
          ? '只能删除空文件夹；里面还有文件时会被拒绝。此操作不可撤销。'
          : '文件会直接从沙箱盘删除，此操作不可撤销。',
    );
    if (!ok) {
      return;
    }
    final result = sandboxDelete(entry.path);
    if (!result.ok) {
      _setMessage(result.error, error: true);
      return;
    }
    final affected = _closeTabsUnder(entry.path);
    if (_selected != null &&
        (_selected == entry.path || _selected!.startsWith('${entry.path}/'))) {
      _selected = null;
      _file = null;
    }
    _refresh();
    _setMessage('已删除 ${entry.path}${affected > 0 ? '（关闭了 $affected 个相关标签）' : ''}');
  }

  /// 关掉 target 落在 `rel` 之下（含自身）的文件标签，返回关掉的个数。
  int _closeTabsUnder(String rel) {
    final doomed = [
      for (final tab in _tabs)
        if (tab.kind == 'file' &&
            (tab.target == rel || tab.target.startsWith('$rel/')))
          tab,
    ];
    if (doomed.isEmpty) {
      return 0;
    }
    setState(() {
      for (final tab in doomed) {
        _tabs.remove(tab);
        _tabFiles.remove('file:${tab.target}');
      }
      if (_tabs.every((tab) => tab.id != _activeTabId)) {
        _activeTabId = _tabs.isEmpty ? 'workspace' : _tabs.first.id;
      }
      if (_tabs.isEmpty) {
        _tabs.add(
          const _PreviewTab(id: 'workspace', title: '工作区', kind: 'workspace'),
        );
      }
    });
    return doomed.length;
  }

  /// 打开一个文件标签（不切到工作区）。
  void _openFileTab(String rel) {
    _selectPath(rel);
  }

  /// 「添加到对话」：入队，聊天面板 / 主窗 1 秒内取走。
  void _attachToChat(List<String> paths) {
    final reason = pushChatAttachments(paths);
    if (reason != null) {
      _setMessage(reason, error: true);
      return;
    }
    _setMessage('已加入对话附件（${paths.length}）：${paths.first}');
  }

  /// 右侧标签条：工作区常驻，其余可关闭。
  ///
  /// 标签多起来时：**鼠标滚轮**直接横向滚动（Flutter 的横向 ListView 默认只认
  /// 横向 delta，竖直滚轮会被丢掉），底部另有一条可见的横向滚动条。
  Widget _buildTabBar() {
    return SizedBox(
      height: 40,
      child: Row(
        children: [
          Expanded(
            child: Scrollbar(
              controller: _tabScroll,
              thumbVisibility: true,
              scrollbarOrientation: ScrollbarOrientation.bottom,
              child: Listener(
                onPointerSignal: (event) {
                  if (event is! PointerScrollEvent || !_tabScroll.hasClients) {
                    return;
                  }
                  final delta = event.scrollDelta.dy != 0
                      ? event.scrollDelta.dy
                      : event.scrollDelta.dx;
                  if (delta == 0) {
                    return;
                  }
                  final position = _tabScroll.position;
                  _tabScroll.jumpTo(
                    (position.pixels + delta).clamp(0.0, position.maxScrollExtent),
                  );
                },
                child: ListView(
                  controller: _tabScroll,
                  scrollDirection: Axis.horizontal,
                  // 底部留 8px 给滚动条，避免它压住标签文字。
                  padding: const EdgeInsets.fromLTRB(4, 0, 4, 8),
                  children: [for (final tab in _tabs) _buildTabChip(tab)],
                ),
              ),
            ),
          ),
          // 终端是「文件显示区下方」的常驻面板（用户 2026-11-15），这里是展开 / 收起。
          IconButton(
            tooltip: _terminalOpen ? '收起终端' : '打开终端',
            icon: Icon(
              _terminalOpen ? Icons.terminal : Icons.terminal_outlined,
              size: 15,
            ),
            color: _terminalOpen ? LunacColors.accent : LunacColors.textMuted,
            visualDensity: VisualDensity.compact,
            onPressed: _toggleTerminal,
          ),
        ],
      ),
    );
  }

  /// 展开 / 收起终端面板；第一次展开才起 shell（复用已有进程，不重启）。
  void _toggleTerminal() {
    setState(() {
      _terminalOpen = !_terminalOpen;
      if (_terminalOpen) {
        _terminalEverOpened = true;
        _terminalHeight = _terminalHeight.clamp(
          _terminalMinHeight,
          _terminalMaxHeight,
        );
      }
    });
  }

  /// 终端面板的上边界：上下拖拽改高度（与资源管理器那条竖分隔条同口径）。
  Widget _buildTerminalSplitter() {
    return MouseRegion(
      cursor: SystemMouseCursors.resizeUpDown,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onVerticalDragUpdate: (details) {
          setState(() {
            _terminalHeight = (_terminalHeight - details.delta.dy).clamp(
              _terminalMinHeight,
              _terminalMaxHeight,
            );
          });
        },
        child: Container(
          height: 5,
          alignment: Alignment.center,
          child: Container(height: 1, color: LunacColors.border),
        ),
      ),
    );
  }

  /// 新标签加在末尾时把它滚进可见范围（标签很多时不至于「点了没反应」）。
  void _scrollTabsToEnd() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_tabScroll.hasClients) {
        return;
      }
      _tabScroll.animateTo(
        _tabScroll.position.maxScrollExtent,
        duration: const Duration(milliseconds: 160),
        curve: Curves.easeOut,
      );
    });
  }

  Widget _buildTabChip(_PreviewTab tab) {
    final active = tab.id == _activeTabId;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4, horizontal: 2),
      child: Material(
        color: active ? LunacColors.surfaceHover : Colors.transparent,
        borderRadius: BorderRadius.circular(5),
        child: InkWell(
          borderRadius: BorderRadius.circular(5),
          onTap: () => _activateTab(tab),
          // 右键：与左侧条目同一套动作（关闭 / 关闭其他 / 复制路径 / 资源管理器 / 添加到对话）。
          onSecondaryTapDown: (details) =>
              _showTabMenu(tab, details.globalPosition),
          child: Container(
            padding: const EdgeInsets.only(left: 8, right: 4),
            decoration: BoxDecoration(
              border: Border.all(
                color: active ? LunacColors.divider : LunacColors.border,
              ),
              borderRadius: BorderRadius.circular(5),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(_tabIcon(tab.kind), size: 12, color: LunacColors.textMuted),
                const SizedBox(width: 5),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 160),
                  child: Text(
                    tab.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: active ? LunacColors.text : LunacColors.textMuted,
                      fontSize: 11,
                    ),
                  ),
                ),
                // 所有标签都可关闭（含常驻的「工作区」）；关掉最后一个时面板会
                // 自动把「工作区」标签放回来（见 `_closeTab`）。
                InkWell(
                  onTap: () => _closeTab(tab),
                  child: Padding(
                    padding: const EdgeInsets.all(3),
                    child: Icon(
                      Icons.close,
                      size: 11,
                      color: LunacColors.textMuted,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  IconData _tabIcon(String kind) => switch (kind) {
    'file' => Icons.description_outlined,
    'app' => Icons.apps,
    'url' => Icons.language,
    'change' => Icons.difference_outlined,
    _ => Icons.folder_open,
  };

  Widget _buildActiveTab() {
    final tab = _tabs.firstWhere(
      (t) => t.id == _activeTabId,
      orElse: () => _tabs.first,
    );
    return switch (tab.kind) {
      'workspace' => _buildWorkspaceTab(),
      'file' => _buildPreview(),
      'app' => _buildAppTab(tab),
      'url' => _buildUrlTab(tab),
      'change' => _buildChangeTab(tab),
      _ => _buildWorkspaceTab(),
    };
  }

  /// 工作区标签：用量 / 额度。AI 改过的文件**不在这里**（用户 2026-11-14 要求），
  /// 它们只在对话面板输入框上方的「改动文件」页里。
  Widget _buildWorkspaceTab() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildUsage(),
        Divider(height: 1, color: LunacColors.border),
        Expanded(
          child: Center(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Text(
                '左侧是工作区文件。点文件看内容；AI 打开的文件 / 应用会以标签出现在上方。',
                textAlign: TextAlign.center,
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            ),
          ),
        ),
      ],
    );
  }

  /// 应用标签（用户 2026-11-14：两档）。
  ///
  /// 一档「缩略图」= 原生抓帧（只读，1.5 秒一帧）；二档「接管」= 把真实窗口
  /// 停靠到下面这块区域（owner = 预览窗，不重父化），可正常点击输入。
  /// 两种都拿不到时如实说明原因，不假装渲染成功。
  Widget _buildAppTab(_PreviewTab tab) {
    final shot = _appShot;
    final title = shot != null && shot.title.isNotEmpty ? shot.title : tab.target;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 6, 6, 4),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.text, fontSize: 12),
                ),
              ),
              if (shot != null && shot.width > 0)
                Text(
                  '${shot.width}×${shot.height}',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              TextButton(
                onPressed: () => unawaited(_refreshAppShot()),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('刷新', style: TextStyle(fontSize: 11)),
              ),
              TextButton(
                onPressed: () => unawaited(_toggleDock(tab)),
                style: TextButton.styleFrom(
                  foregroundColor: _appDocked
                      ? LunacColors.danger
                      : LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                ),
                child: Text(
                  _appDocked ? '还回' : '接管',
                  style: const TextStyle(fontSize: 11),
                ),
              ),
              TextButton(
                onPressed: () => openPath(tab.target),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('打开应用', style: TextStyle(fontSize: 11)),
              ),
            ],
          ),
        ),
        Divider(height: 1, color: LunacColors.border),
        Expanded(
          child: Container(
            key: _appBodyKey,
            color: LunacColors.background,
            child: _buildAppBody(tab),
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 4, 12, 6),
          child: Text(
            _appDocked
                ? '已接管：这是应用的**真实窗口**，停靠在这块区域；切标签 / 点「还回」恢复原位置。'
                : '缩略图是只读画面（每 1.5 秒抓一帧）；要点击输入请点「接管」。'
                      '部分 GPU 合成应用或受保护窗口会抓到黑屏，这是 Windows 的边界。',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
        ),
      ],
    );
  }

  Widget _buildAppBody(_PreviewTab tab) {
    if (_appDocked) {
      // 这块区域此刻被真实应用窗口盖住；下面只留一行说明（还回后能看见）。
      return Center(
        child: Text(
          '应用窗口已停靠在上面这块区域',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
        ),
      );
    }
    final bytes = _appBytes;
    if (bytes != null && bytes.isNotEmpty) {
      return Padding(
        padding: const EdgeInsets.all(8),
        child: Image.memory(
          bytes,
          fit: BoxFit.contain,
          gaplessPlayback: true,
          filterQuality: FilterQuality.medium,
          errorBuilder: (context, error, stack) => Center(
            child: Text(
              '画面解码失败',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
        ),
      );
    }
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Text(
          _appError ?? '还没有画面：应用没在运行，或预览桥接不可用。\n'
              '原生应用不能直接嵌进 Flutter，这里用的是抓帧 / 停靠两档。',
          textAlign: TextAlign.center,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
        ),
      ),
    );
  }

  /// 抓一帧当前应用标签的画面（非应用标签时顺带把停靠还回）。
  Future<void> _refreshAppShot() async {
    if (!mounted) {
      return;
    }
    final tab = _tabs.firstWhere(
      (t) => t.id == _activeTabId,
      orElse: () => _tabs.first,
    );
    if (tab.kind != 'app') {
      if (_appDocked) {
        setState(() => _appDocked = false);
        unawaited(undockAppWindow());
      }
      return;
    }
    final shot = await captureAppWindow(tab.target);
    if (!mounted) {
      return;
    }
    Uint8List? bytes;
    if (shot != null && shot.ok && shot.path.isNotEmpty) {
      try {
        bytes = File(shot.path).readAsBytesSync();
      } catch (_) {
        bytes = null;
      }
    }
    setState(() {
      _appShot = shot;
      _appBytes = bytes;
      if (shot == null) {
        _appError = '预览桥接不可用';
      } else if (!shot.ok) {
        _appError = shot.error.isEmpty ? '抓不到画面' : shot.error;
      } else {
        _appError = null;
      }
    });
  }

  /// 「接管 / 还回」：把真实窗口停靠到应用标签的正文区域（屏幕物理像素）。
  Future<void> _toggleDock(_PreviewTab tab) async {
    if (_appDocked) {
      final ok = await undockAppWindow();
      if (!mounted) {
        return;
      }
      setState(() {
        _appDocked = false;
        _appError = ok ? null : '还回失败（应用可能已退出）';
      });
      return;
    }
    final rect = _appBodyPhysicalRect();
    if (rect == null) {
      setState(() => _appError = '拿不到面板位置，无法接管');
      return;
    }
    final error = await dockAppWindow(tab.target, rect);
    if (!mounted) {
      return;
    }
    setState(() {
      _appDocked = error == null;
      _appError = error;
    });
  }

  /// 离开应用标签 / 关闭它时把停靠还回，避免把别人的窗口留在面板位置。
  void _undockIfNeeded() {
    if (!_appDocked) {
      return;
    }
    _appDocked = false;
    unawaited(undockAppWindow());
  }

  Rect? _appBodyPhysicalRect() {
    final box = _appBodyKey.currentContext?.findRenderObject() as RenderBox?;
    if (box == null || !box.hasSize) {
      return null;
    }
    final topLeft = box.localToGlobal(Offset.zero);
    final ratio = MediaQuery.of(context).devicePixelRatio;
    return Rect.fromLTWH(
      topLeft.dx * ratio,
      topLeft.dy * ratio,
      box.size.width * ratio,
      box.size.height * ratio,
    );
  }

  Widget _buildUrlTab(_PreviewTab tab) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.language, size: 26, color: LunacColors.textMuted),
          const SizedBox(height: 8),
          Text(tab.title, style: TextStyle(color: LunacColors.text, fontSize: 13)),
          const SizedBox(height: 4),
          Text(
            tab.target,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
          const SizedBox(height: 10),
          TextButton(
            onPressed: () => openExternalUrl(tab.target),
            child: const Text('用浏览器打开', style: TextStyle(fontSize: 11)),
          ),
        ],
      ),
    );
  }

  /// 改动文件标签：文件名下给「变更已完成 · 是否应用？」+ 全部应用 / 撤销，
  /// 正文用红底删除行、绿底新增行（用户 2026-11-14 要求）。
  Widget _buildChangeTab(_PreviewTab tab) {
    final diff = _changeDiffs[tab.id] ?? _readChangeDiff(tab.target);
    _changeDiffs[tab.id] = diff;
    if (diff == null) {
      return Center(
        child: Text(
          '读不到改动内容（预览桥接不可用）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    }
    if (!diff.ok) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Text(
            diff.error.isEmpty ? '没有改动快照' : diff.error,
            textAlign: TextAlign.center,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
          ),
        ),
      );
    }
    final name = tab.target.split(RegExp(r'[\\/]')).last;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 8, 6),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      name.isEmpty ? tab.title : name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(color: LunacColors.text, fontSize: 12.5),
                    ),
                  ),
                  Text(
                    '+${diff.added} −${diff.removed}',
                    style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
                  ),
                ],
              ),
              const SizedBox(height: 4),
              Row(
                children: [
                  Expanded(
                    child: Text(
                      '变更已完成 · 是否应用？',
                      style: TextStyle(color: LunacColors.textDim, fontSize: 11),
                    ),
                  ),
                  TextButton(
                    onPressed: _acceptAllChanges,
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.accent,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('全部应用', style: TextStyle(fontSize: 11)),
                  ),
                  TextButton(
                    onPressed: _revertAllChanges,
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.danger,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('撤销', style: TextStyle(fontSize: 11)),
                  ),
                ],
              ),
            ],
          ),
        ),
        Divider(height: 1, color: LunacColors.border),
        Expanded(child: _buildDiffBody(diff)),
      ],
    );
  }

  ChangeDiff? _readChangeDiff(String path) =>
      widget.loadChangeDiff?.call(path) ?? readChangeDiff(path);

  Widget _buildDiffBody(ChangeDiff diff) {
    if (diff.binary) {
      return Center(
        child: Text(
          '二进制文件，无法按行比较',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    }
    if (diff.lines.isEmpty) {
      return Center(
        child: Text(
          '内容没有变化',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.symmetric(vertical: 4),
      itemCount: diff.lines.length + (diff.truncated ? 1 : 0),
      itemBuilder: (context, index) {
        if (index == diff.lines.length) {
          return Padding(
            padding: const EdgeInsets.all(8),
            child: Text(
              '差异过大，已截断显示',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          );
        }
        final line = diff.lines[index];
        final isAdd = line.kind == 'add';
        final isDel = line.kind == 'del';
        return Container(
          color: isAdd
              ? const Color(0xFF22C55E).withValues(alpha: 0.14)
              : isDel
              ? const Color(0xFFEF4444).withValues(alpha: 0.14)
              : null,
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 1),
          child: Text(
            '${isAdd ? '+' : isDel ? '−' : ' '}${line.text}',
            style: TextStyle(
              color: isAdd
                  ? const Color(0xFF4ADE80)
                  : isDel
                  ? const Color(0xFFF87171)
                  : LunacColors.textDim,
              fontSize: 11.5,
              height: 1.45,
              fontFamily: 'Consolas',
            ),
          ),
        );
      },
    );
  }

  /// 全部应用：删掉所有快照，文件本身保留（用户确认改动）。
  void _acceptAllChanges() {
    final ok = acceptChanges();
    if (!mounted) {
      return;
    }
    setState(() {
      _changeDiffs.clear();
      _closeChangeTabs();
      _error = !ok;
      _message = ok ? '已全部应用（改动保留）' : '应用失败（详情见日志）';
    });
  }

  /// 撤销：把所有 AI 改动写回原文（新建的文件删除）。
  void _revertAllChanges() {
    final result = revertChanges();
    if (!mounted) {
      return;
    }
    setState(() {
      _changeDiffs.clear();
      _closeChangeTabs();
      _error = !result.ok;
      _message = result.ok
          ? '已撤销：还原 ${result.restored} 项，删除 ${result.deleted} 项'
          : '撤销失败：${result.error}';
    });
  }

  void _closeChangeTabs() {
    _tabs.removeWhere((tab) => tab.kind == 'change');
    for (final key in _changeDiffs.keys.toList()) {
      _changeDiffs.remove(key);
    }
    if (_tabs.every((tab) => tab.id != _activeTabId)) {
      _activeTabId = 'workspace';
    }
  }

  void _selectPath(String path, {String? title}) {
    final id = 'file:$path';
    final file = _readFile(path);
    final isNew = !_tabs.any((t) => t.id == id);
    setState(() {
      _tabFiles[id] = file;
      if (!_tabs.any((t) => t.id == id)) {
        _tabs.add(
          _PreviewTab(
            id: id,
            title: title ?? path.split('/').last,
            kind: 'file',
            target: path,
          ),
        );
      }
      _activeTabId = id;
      _selected = path;
      _file = file;
      _message = null;
    });
    if (isNew) {
      _scrollTabsToEnd();
    }
  }

  void _activateTab(_PreviewTab tab) {
    _undockIfNeeded();
    setState(() {
      _activeTabId = tab.id;
      if (tab.kind == 'file') {
        final cached = _tabFiles[tab.id];
        final file = cached ?? _readFile(tab.target);
        _tabFiles[tab.id] = file;
        _file = file;
        _selected = tab.target;
      }
    });
  }

  void _closeTab(_PreviewTab tab) {
    _undockIfNeeded();
    setState(() {
      _tabs.removeWhere((t) => t.id == tab.id);
      _tabFiles.remove(tab.id);
      _changeDiffs.remove(tab.id);
      if (_tabs.isEmpty) {
        // 用户 2026-11-14：标签可以关到空，但空的时候自动把「工作区」放回来。
        _tabs.add(
          const _PreviewTab(id: 'workspace', title: '工作区', kind: 'workspace'),
        );
        _activeTabId = 'workspace';
        _selected = null;
        _file = null;
        return;
      }
      if (_activeTabId == tab.id) {
        final next = _tabs.last;
        _activeTabId = next.id;
        _selected = null;
        _file = null;
      }
    });
  }

  /// 取走 agent 通过 Rust 队列转交过来的预览标签。
  void _pullPreviewTabs() {
    final requests = widget.pullTabs?.call() ?? takePreviewTabs();
    if (requests.isEmpty || !mounted) {
      return;
    }
    var added = false;
    setState(() {
      for (final request in requests) {
        if (request.kind == 'workspace') {
          _activeTabId = 'workspace';
          continue;
        }
        final id = '${request.kind}:${request.target}';
        if (!_tabs.any((t) => t.id == id)) {
          _tabs.add(
            _PreviewTab(
              id: id,
              // 用户要求的形态是「预览: <应用名>」；模型有时只给应用名，这里补齐。
              title: request.kind == 'app' && !request.title.startsWith('预览')
                  ? '预览: ${request.title}'
                  : request.title,
              kind: request.kind,
              target: request.target,
            ),
          );
          added = true;
        }
        _activeTabId = id;
        if (request.kind == 'file') {
          final file = _readFile(request.target);
          _tabFiles[id] = file;
          _file = file;
          _selected = request.target;
        }
      }
    });
    if (added) {
      _scrollTabsToEnd();
    }
  }

  Widget _buildPreview() {
    final file = _file;
    if (file == null) {
      return Center(
        child: Text(
          '选一个文件查看内容',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 8, 6),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  file.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.text, fontSize: 12),
                ),
              ),
              Text(
                _formatBytes(file.size),
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
              ),
              TextButton(
                onPressed: file.path.isEmpty ? null : () => openPath(file.path),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('打开', style: TextStyle(fontSize: 11)),
              ),
            ],
          ),
        ),
        Divider(height: 1, color: LunacColors.border),
        Expanded(child: _buildPreviewBody(file)),
      ],
    );
  }

  Widget _buildPreviewBody(SandboxFile file) {
    if (!file.ok) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Text(
            file.error.isEmpty ? '读不了这个文件' : file.error,
            textAlign: TextAlign.center,
            style: TextStyle(color: LunacColors.danger, fontSize: 12),
          ),
        ),
      );
    }
    switch (file.kind) {
      case 'image':
        return Center(
          child: Padding(
            padding: const EdgeInsets.all(10),
            child: Image.file(
              File(file.path),
              fit: BoxFit.contain,
              errorBuilder: (context, error, stack) => Text(
                '图片无法显示',
                style: TextStyle(color: LunacColors.danger, fontSize: 12),
              ),
            ),
          ),
        );
      case 'text':
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (file.truncated)
              Padding(
                padding: EdgeInsets.fromLTRB(12, 6, 12, 0),
                child: Text(
                  '文件较大，只显示前 256 KiB',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              ),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.all(12),
                child: SelectableText(
                  file.text,
                  style: TextStyle(
                    color: LunacColors.textDim,
                    fontSize: 12,
                    height: 1.45,
                    fontFamily: 'Consolas',
                  ),
                ),
              ),
            ),
          ],
        );
      default:
        return Center(
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Text(
              '二进制文件（${_formatBytes(file.size)}），不在界面里预览',
              textAlign: TextAlign.center,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
            ),
          ),
        );
    }
  }

  Widget _buildMessage() {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(14, 6, 14, 8),
      child: Text(
        _message!,
        style: TextStyle(
          color: _error ? LunacColors.danger : LunacColors.textMuted,
          fontSize: 11,
        ),
      ),
    );
  }
}

String _formatBytes(int bytes) {
  if (bytes < 1024) {
    return '$bytes B';
  }
  if (bytes < 1024 * 1024) {
    return '${(bytes / 1024).toStringAsFixed(1)} KB';
  }
  if (bytes < 1024 * 1024 * 1024) {
    return '${(bytes / 1024 / 1024).toStringAsFixed(1)} MB';
  }
  return '${(bytes / 1024 / 1024 / 1024).toStringAsFixed(2)} GB';
}

/// 预览窗里的一个标签（VS Code 风格的标签页）。
class _PreviewTab {
  const _PreviewTab({
    required this.id,
    required this.title,
    required this.kind,
    this.target = '',
  });

  final String id;
  final String title;

  /// `workspace` / `file` / `app` / `url`
  final String kind;
  final String target;
}
