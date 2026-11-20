// 文件转换面板（v1 `convert` 插件的原生等价物）。
//
// 版式对齐 v1 `app/src/plugins/builtin/convert.ts` 的 `shellHtml()`（2026-10-24 重构版）：
//   顶部：引擎状态 + 保存目录 + 「选择目录」按钮；
//   主区：左侧四个竖排页签（PDF 转换 / 视频格式转换 / 视频音频提取 / 音频格式转换）
//         → 输入列（目标格式胶囊 + 输入框 + 文件泡泡）
//         → 中间（换向图标 + 进度条）
//         → 输出列（占位目标行 + 输出框 + 结果泡泡）；
//   底部：一行消息（v1 `convert-msg`）。
//
// 与 v1 的差异（如实登记）：
//   · v1 的拖放进窗口还没接（插件窗不处理 WM_DROPFILES）—— 输入框**点击**走系统多选框，
//     与 v1 点击输入框的行为一致；搜索栏附件仍会一开始就带进来；
//   · 「确定态进度 + 取消」已接（2026-11-15）：Rust 解析 ffmpeg 的 `out_time_us` 并经
//     `lunac_convert_progress` 供 Dart 300ms 轮询；ffprobe 不在时退回不确定态。
//     PDF（LibreOffice）是一次性进程，不能中途取消（与 v1 同口径）；
//   · 多一个「图片格式转换」页签：v2 的 Rust 支持图片族，v1 新版四页签里没有它，
//     去掉会让已有能力倒退，所以作为 v2 追加项放在最后。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/convert.dart';
import '../platform/shell_actions.dart';
import '../theme.dart';

/// v1 的四个页签 + v2 追加的图片页签。
class _TabDef {
  const _TabDef(this.key, this.label, this.kinds, this.targets);

  final String key;
  final String label;

  /// 这个页签接受哪些源文件族（`ConvertPlan.kind`）。
  final Set<String> kinds;

  /// 这个页签能输出什么（v1 `TABS[].targets`）。
  final List<String> targets;
}

const List<_TabDef> _tabs = [
  _TabDef('pdf', 'PDF 转换', {'pdf'}, ['docx', 'pptx', 'xlsx', 'png']),
  _TabDef('video', '视频格式转换', {'video'}, ['mp4', 'mkv', 'webm', 'avi', 'mov', 'gif']),
  _TabDef('extract', '视频音频提取', {'video'}, ['mp3', 'wav', 'flac', 'm4a', 'aac', 'ogg']),
  _TabDef('audio', '音频格式转换', {'audio'}, ['mp3', 'wav', 'flac', 'm4a', 'aac', 'ogg']),
  _TabDef('image', '图片格式转换', {'image'}, ['png', 'jpg', 'webp', 'bmp', 'tiff', 'gif']),
];

class ConvertPanel extends StatefulWidget {
  const ConvertPanel({
    super.key,
    required this.onClose,
    this.attachments = const [],
    this.windowControls,
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 替换标题栏里的「返回」；null ⇒ 内嵌模式，保留返回。
  final Widget? windowControls;

  /// 搜索栏里的附件（每个都是一次待转换的源文件）。
  final List<String> attachments;

  @override
  State<ConvertPanel> createState() => _ConvertPanelState();
}

/// 一个源文件 + 它的转换计划（目标格式由页签决定）。
class _ConvertItem {
  _ConvertItem({required this.path, required this.plan});

  final String path;
  final ConvertPlan? plan;

  String get kind => plan?.kind ?? 'unknown';
}

/// 一次转换的结果（成功带输出路径，失败带引擎 stderr 摘要）。
class _ConvertResult {
  const _ConvertResult({
    required this.path,
    required this.target,
    this.outputPath,
    this.error,
  });

  final String path;
  final String target;
  final String? outputPath;
  final String? error;

  bool get ok => outputPath != null;
}

class _ConvertPanelState extends State<ConvertPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'convert-panel',
    onKeyEvent: _onKey,
  );

  late final List<_ConvertItem> _items = [
    for (final path in widget.attachments)
      _ConvertItem(path: path, plan: loadConvertPlan(path)),
  ];

  /// 当前页签（缺省跟第一个附件走，没有附件时是 v1 的缺省「视频」）。
  late String _tabKey = _initialTab();

  /// 每个页签当前选中的目标格式（v1 `targetByTab`）。
  final Map<String, String> _targetByTab = {
    for (final tab in _tabs)
      tab.key: tab.targets.isNotEmpty ? tab.targets.first : '',
  };

  /// 保存目录；空 = 与源文件同目录（v1 `outDir` 的缺省口径）。
  String _outDir = '';

  bool _busy = false;
  final List<_ConvertResult> _results = <_ConvertResult>[];

  /// 当前转换进度（v1 `convert-progress` 事件的 v2 等价物：300ms 轮询）。
  ConvertProgress? _progress;
  Timer? _progressTimer;

  /// 队列里的第几个 / 一共几个（1 起；面板显示「2/3」）。
  int _jobIndex = 0;
  int _jobCount = 0;

  /// 用户点了「取消」：当前文件结束后不再继续后面的文件。
  bool _cancelRequested = false;

  String? _message;
  bool _messageError = false;
  bool _confirmClear = false;

  _TabDef get _tab => _tabs.firstWhere(
        (tab) => tab.key == _tabKey,
        orElse: () => _tabs[1],
      );

  String _initialTab() {
    if (widget.attachments.isEmpty) {
      return 'video';
    }
    final plan = loadConvertPlan(widget.attachments.first);
    final kind = plan?.kind ?? '';
    for (final tab in _tabs) {
      if (tab.kinds.contains(kind)) {
        return tab.key;
      }
    }
    return 'video';
  }

  /// 当前页签里的源文件。
  List<_ConvertItem> get _tabItems =>
      [for (final item in _items) if (_tab.kinds.contains(item.kind)) item];

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
    _progressTimer?.cancel();
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

  /// 桥接不可用：所有文件都拿不到计划。
  bool get _unavailable =>
      _items.isNotEmpty && _items.every((item) => item.plan == null);

  /// 当前页签的队列能不能转（v1 引擎缺失时禁用）。
  bool get _canStart {
    final items = _tabItems;
    if (_busy || items.isEmpty) {
      return false;
    }
    final target = _targetByTab[_tabKey] ?? '';
    if (target.isEmpty) {
      return false;
    }
    return items.every((item) {
      final plan = item.plan;
      return plan != null &&
          plan.engineFound &&
          plan.error.isEmpty &&
          plan.targets.contains(target);
    });
  }

  /// 顶部引擎状态（v1 `#convert-engine`）。
  String get _engineStatus {
    if (_items.isEmpty) {
      return '引擎：未选择文件';
    }
    if (_unavailable) {
      return '不可用（rust 桥接未加载）';
    }
    for (final item in _items) {
      final plan = item.plan;
      if (plan == null || plan.error.isNotEmpty || plan.engineFound) {
        continue;
      }
      return plan.engine == 'libreoffice'
          ? '未找到 LibreOffice（PDF 转换不可用）'
          : '未找到 ffmpeg，请先安装';
    }
    return '引擎已就绪';
  }

  String? get _engineHint {
    for (final item in _items) {
      final plan = item.plan;
      if (plan == null || plan.error.isNotEmpty || plan.engineFound) {
        continue;
      }
      return plan.engine == 'libreoffice'
          ? '未找到 LibreOffice，PDF 转换不可用。PDF 转换需要本机安装 LibreOffice。'
          : '未找到 ffmpeg，请先安装。装一个 ffmpeg 就能用：在终端执行 '
              'winget install Gyan.FFmpeg，然后重启 Lunac。也可以把 ffmpeg.exe '
              '放到 Lunac 安装目录下。';
    }
    return null;
  }

  void _selectTab(String key) {
    if (_busy || key == _tabKey) {
      return;
    }
    setState(() {
      _tabKey = key;
      _message = null;
      _confirmClear = false;
    });
  }

  void _selectTarget(String target) {
    if (_busy) {
      return;
    }
    setState(() => _targetByTab[_tabKey] = target);
  }

  void _pickInputs() {
    if (_busy) {
      return;
    }
    final picked = pickConvertFiles(_tabKey);
    if (!mounted || picked.isEmpty) {
      return;
    }
    final added = <_ConvertItem>[];
    final skipped = <String>[];
    for (final path in picked) {
      if (_items.any((item) => item.path == path)) {
        continue;
      }
      final plan = loadConvertPlan(path);
      if (plan == null || !_tab.kinds.contains(plan.kind)) {
        skipped.add(path);
        continue;
      }
      added.add(_ConvertItem(path: path, plan: plan));
    }
    setState(() {
      _items.addAll(added);
      _messageError = skipped.isNotEmpty;
      _message = skipped.isEmpty
          ? null
          : '有 ${skipped.length} 个文件不属于当前页签，已跳过';
    });
  }

  void _pickOutDir() {
    final dir = pickConvertOutDir();
    if (!mounted || dir.isEmpty) {
      return;
    }
    setState(() {
      _outDir = dir;
      _message = null;
    });
  }

  void _removeItem(_ConvertItem item) {
    if (_busy) {
      return;
    }
    setState(() {
      _items.removeWhere((entry) => entry.path == item.path);
      _results.removeWhere((result) => result.path == item.path);
    });
  }

  void _clearTab() {
    if (_busy) {
      return;
    }
    if (!_confirmClear) {
      setState(() {
        _confirmClear = true;
        _messageError = false;
        _message = '再点一次「清空」清掉当前页签的文件';
      });
      return;
    }
    final paths = {for (final item in _tabItems) item.path};
    setState(() {
      _confirmClear = false;
      _items.removeWhere((item) => paths.contains(item.path));
      _results.removeWhere((result) => paths.contains(result.path));
      _message = null;
    });
  }

  Future<void> _start() async {
    if (!_canStart) {
      return;
    }
    final target = _targetByTab[_tabKey]!;
    final jobs = [for (final item in _tabItems) item.path];
    final paths = jobs.toSet();
    setState(() {
      _busy = true;
      _message = null;
      _messageError = false;
      _progress = null;
      _jobIndex = 0;
      _jobCount = jobs.length;
      _cancelRequested = false;
      _results.removeWhere((result) => paths.contains(result.path));
    });
    _startProgressPolling();
    final results = <_ConvertResult>[];
    for (var index = 0; index < jobs.length; index++) {
      final path = jobs[index];
      if (_cancelRequested) {
        break;
      }
      setState(() => _jobIndex = index + 1);
      final reply = await runConvert(path, target, outDir: _outDir);
      // 被用户取消的那一个不进结果列（v1 会把它从条目里移除）。
      final cancelled = reply.error != null && reply.error!.contains('已取消');
      if (!cancelled) {
        results.add(
          _ConvertResult(
            path: path,
            target: target,
            outputPath: reply.outputPath,
            error: reply.error,
          ),
        );
      }
      // 取消后不再继续队列里的下一个文件（v1 也是逐文件取消）。
      if (_cancelRequested) {
        break;
      }
    }
    _stopProgressPolling();
    if (!mounted) {
      return;
    }
    setState(() {
      _busy = false;
      _progress = null;
      _results.addAll(results);
      if (_cancelRequested) {
        _messageError = false;
        _message = '已取消';
        return;
      }
      final failed = results.where((result) => !result.ok).length;
      _messageError = failed > 0;
      _message = failed == 0
          ? '完成 ${results.length} 个'
          : '完成 ${results.length - failed}/${results.length}，失败 $failed 个';
    });
  }

  void _startProgressPolling() {
    _progressTimer?.cancel();
    _progressTimer = Timer.periodic(const Duration(milliseconds: 300), (_) {
      if (!mounted || !_busy) {
        return;
      }
      final progress = readConvertProgress();
      if (progress != null) {
        setState(() => _progress = progress);
      }
    });
  }

  void _stopProgressPolling() {
    _progressTimer?.cancel();
    _progressTimer = null;
  }

  /// 「取消」：后端杀掉 ffmpeg 进程树 + 删半成品；当前文件会以「已取消转换」返回，
  /// 队列里剩下的文件不再开始。PDF（LibreOffice）不可取消，后端 no-op。
  Future<void> _cancel() async {
    if (!_busy || _cancelRequested) {
      return;
    }
    setState(() {
      _cancelRequested = true;
      _message = '正在取消…';
      _messageError = false;
    });
    final reply = await cancelConvert();
    if (!mounted) {
      return;
    }
    if (reply.isError) {
      setState(() {
        _message = reply.error;
        _messageError = true;
      });
    }
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
          borderRadius: BorderRadius.circular(12),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            _buildTopBar(),
            Divider(height: 1, color: LunacColors.border),
            Expanded(child: _buildMain()),
            Divider(height: 1, color: LunacColors.border),
            _buildMessageLine(),
          ],
        ),
      ),
    );
  }

  /// v1 `convert-top`：引擎状态 + 保存目录 + 选择目录。
  Widget _buildTopBar() {
    final engineHint = _engineHint;
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 6, 10, 6),
      child: Row(
        children: [
          // 内嵌模式才画「返回」；独立窗的三个窗口键放**右上角**（用户 2026-11-20 口径）。
          if (widget.windowControls == null) ...[
            IconButton(
              tooltip: '关闭（Esc）',
              icon: const Icon(Icons.arrow_back, size: 18),
              color: LunacColors.textDim,
              visualDensity: VisualDensity.compact,
              onPressed: widget.onClose,
            ),
            const SizedBox(width: 2),
          ],
          Text(
            _engineStatus,
            style: TextStyle(
              color: engineHint != null ? LunacColors.danger : LunacColors.textDim,
              fontSize: 11,
            ),
          ),
          const Spacer(),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 360),
            child: Text(
              _outDir.isEmpty ? '保存到：与源文件同目录' : _outDir,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: LunacColors.textMuted,
                fontSize: 11,
              ),
            ),
          ),
          const SizedBox(width: 4),
          IconButton(
            tooltip: '选择保存目录',
            icon: const Icon(Icons.folder_outlined, size: 16),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: _busy ? null : _pickOutDir,
          ),
          // 独立窗：置顶 / 最小化 / 关闭 常驻右上角。
          if (widget.windowControls != null) ...[
            const SizedBox(width: 4),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  /// v1 `convert-main`：左页签 + 输入列 + 中间 + 输出列。
  Widget _buildMain() {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(width: 104, child: _buildTabs()),
        VerticalDivider(width: 1, color: LunacColors.border),
        Expanded(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(10, 10, 10, 8),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(child: _buildInputColumn()),
                SizedBox(width: 58, child: _buildMiddle()),
                Expanded(child: _buildOutputColumn()),
              ],
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildTabs() {
    return ListView(
      padding: const EdgeInsets.symmetric(vertical: 8),
      children: [
        for (final tab in _tabs)
          _buildTabButton(tab),
      ],
    );
  }

  Widget _buildTabButton(_TabDef tab) {
    final selected = tab.key == _tabKey;
    final count = _items.where((item) => tab.kinds.contains(item.kind)).length;
    return Material(
      color: selected ? LunacColors.surfaceHover : Colors.transparent,
      child: InkWell(
        onTap: () => _selectTab(tab.key),
        child: Container(
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(
                width: 2,
                color: selected ? LunacColors.accent : Colors.transparent,
              ),
            ),
          ),
          padding: const EdgeInsets.fromLTRB(10, 8, 6, 8),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  tab.label,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: selected ? LunacColors.text : LunacColors.textDim,
                    fontSize: 11.5,
                    height: 1.35,
                    fontWeight: selected ? FontWeight.w600 : FontWeight.w400,
                  ),
                ),
              ),
              if (count > 0)
                Text(
                  '$count',
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
  }

  /// 输入列：目标格式胶囊 + 输入框 + 文件泡泡（v1 左列）。
  Widget _buildInputColumn() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildTargetRow(),
        const SizedBox(height: 6),
        Expanded(child: _buildDropZone()),
        const SizedBox(height: 6),
        Flexible(child: _buildBubbles(isOutput: false)),
      ],
    );
  }

  Widget _buildTargetRow() {
    return SizedBox(
      height: 26,
      child: Row(
        children: [
          Text(
            '转换为',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: _tab.targets.length,
              separatorBuilder: (_, _) => const SizedBox(width: 5),
              itemBuilder: (context, index) {
                final target = _tab.targets[index];
                final selected = _targetByTab[_tabKey] == target;
                return _buildChip(
                  target,
                  selected: selected,
                  onTap: () => _selectTarget(target),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildChip(
    String label, {
    required bool selected,
    required VoidCallback onTap,
  }) {
    return InkWell(
      onTap: _busy ? null : onTap,
      borderRadius: BorderRadius.circular(6),
      child: Container(
        alignment: Alignment.center,
        padding: const EdgeInsets.symmetric(horizontal: 9),
        decoration: BoxDecoration(
          color: selected ? LunacColors.surfaceHover : LunacColors.background,
          border: Border.all(
            color: selected ? LunacColors.divider : LunacColors.border,
          ),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: selected ? LunacColors.accent : LunacColors.textDim,
            fontSize: 11,
          ),
        ),
      ),
    );
  }

  /// 输入框（v1 `.convert-drop`）：点击 = 系统多选框。
  Widget _buildDropZone() {
    final count = _tabItems.length;
    return InkWell(
      onTap: _busy ? null : _pickInputs,
      borderRadius: BorderRadius.circular(8),
      child: Container(
        decoration: BoxDecoration(
          color: LunacColors.background,
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Stack(
          children: [
            Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    Icons.upload_file_outlined,
                    size: 26,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    count == 0 ? '点击选择文件' : '已选 $count 个文件',
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 12,
                    ),
                  ),
                  const SizedBox(height: 4),
                  Text(
                    _tab.key == 'pdf' ? '支持 PDF' : '支持${_tab.label.replaceAll('格式转换', '').replaceAll('视频音频提取', '视频')}',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ],
              ),
            ),
            if (count > 0)
              Positioned(
                top: 4,
                right: 4,
                child: TextButton(
                  onPressed: _busy ? null : _clearTab,
                  style: TextButton.styleFrom(
                    foregroundColor: _confirmClear
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    visualDensity: VisualDensity.compact,
                    minimumSize: const Size(0, 22),
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                  ),
                  child: Text(
                    _confirmClear ? '确认清空' : '清空',
                    style: const TextStyle(fontSize: 10),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  /// v1 `convert-mid`：换向图标 + 进度条。
  Widget _buildMiddle() {
    final progress = _progress;
    final determinate = progress != null && progress.determinate;
    final percent = determinate ? progress.percent.clamp(0, 100).round() : null;
    final job = _jobCount > 0 ? ' · $_jobIndex/$_jobCount' : '';
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Icon(
          Icons.swap_horiz,
          size: 26,
          color: _busy ? LunacColors.accent : LunacColors.textMuted,
        ),
        const SizedBox(height: 8),
        SizedBox(
          width: 64,
          child: _busy
              ? LinearProgressIndicator(
                  value: determinate ? (progress.percent / 100).clamp(0.0, 1.0) : null,
                  minHeight: 3,
                  backgroundColor: LunacColors.border,
                  color: LunacColors.accent,
                )
              : Container(
                  height: 3,
                  decoration: BoxDecoration(
                    color: LunacColors.border,
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
        ),
        const SizedBox(height: 6),
        Text(
          _busy
              ? '转换中${percent == null ? '' : ' $percent%'}$job'
              : '就绪',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10,
          ),
        ),
        if (_busy && progress?.running == true && !_cancelRequested)
          TextButton(
            onPressed: _cancel,
            style: TextButton.styleFrom(
              minimumSize: Size.zero,
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
              visualDensity: VisualDensity.compact,
              foregroundColor: LunacColors.danger,
              textStyle: const TextStyle(fontSize: 10),
            ),
            child: const Text('取消'),
          ),
        if (_busy &&
            _tabKey == 'pdf' &&
            !_cancelRequested &&
            progress?.running != true)
          Text(
            'PDF 由 LibreOffice 一次完成，不能中途取消',
            textAlign: TextAlign.center,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 9),
          ),
      ],
    );
  }

  /// 输出列：占位目标行 + 输出框 + 结果泡泡（v1 右列）。
  Widget _buildOutputColumn() {
    final results = [
      for (final result in _results)
        if (_tabItems.any((item) => item.path == result.path)) result,
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // 与左侧目标行等高，保证两个框的上沿齐平（v1 `convert-targets-ghost`）。
        const SizedBox(height: 26),
        const SizedBox(height: 6),
        Expanded(
          child: Container(
            decoration: BoxDecoration(
              color: LunacColors.background,
              border: Border.all(color: LunacColors.border),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    results.isEmpty
                        ? Icons.inventory_2_outlined
                        : Icons.check_circle_outline,
                    size: 26,
                    color: results.isEmpty
                        ? LunacColors.textMuted
                        : LunacColors.accent,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    results.isEmpty ? '转换结果会出现在这里' : '已产出 ${results.length} 个文件',
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 12,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: 6),
        Flexible(child: _buildBubbles(isOutput: true)),
      ],
    );
  }

  /// v1 `.convert-bubbles`：输入泡泡可移除 / 打开；输出泡泡点击打开。
  Widget _buildBubbles({required bool isOutput}) {
    final entries = isOutput
        ? [
            for (final result in _results)
              if (_tabItems.any((item) => item.path == result.path)) result,
          ]
        : _tabItems;
    if (entries.isEmpty) {
      return const SizedBox.shrink();
    }
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        for (final entry in entries)
          isOutput
              ? _buildOutputBubble(entry as _ConvertResult)
              : _buildInputBubble(entry as _ConvertItem),
      ],
    );
  }

  Widget _buildInputBubble(_ConvertItem item) {
    return _bubbleShell(
      icon: Icons.description_outlined,
      text: _basename(item.path),
      tooltip: item.path,
      onTap: () => openPath(item.path),
      trailing: IconButton(
        tooltip: '移除',
        icon: const Icon(Icons.close, size: 13),
        color: LunacColors.textMuted,
        visualDensity: VisualDensity.compact,
        onPressed: _busy ? null : () => _removeItem(item),
      ),
    );
  }

  Widget _buildOutputBubble(_ConvertResult result) {
    final ok = result.ok;
    return _bubbleShell(
      icon: ok ? Icons.check_circle_outline : Icons.error_outline,
      iconColor: ok ? LunacColors.accent : LunacColors.danger,
      text: ok
          ? _basename(result.outputPath!)
          : '${_basename(result.path)} → ${result.target} 失败',
      tooltip: ok ? result.outputPath! : (result.error ?? ''),
      onTap: ok ? () => openPath(result.outputPath!) : null,
    );
  }

  Widget _bubbleShell({
    required IconData icon,
    required String text,
    required String tooltip,
    Color? iconColor,
    VoidCallback? onTap,
    Widget? trailing,
  }) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 5),
      child: Material(
        color: LunacColors.background,
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(6),
          child: Tooltip(
            message: tooltip,
            child: Container(
              padding: const EdgeInsets.fromLTRB(8, 5, 4, 5),
              decoration: BoxDecoration(
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.circular(6),
              ),
              child: Row(
                children: [
                  Icon(icon, size: 13, color: iconColor ?? LunacColors.textMuted),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      text,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textDim,
                        fontSize: 11,
                      ),
                    ),
                  ),
                  ?trailing,
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// 底部消息行（v1 `convert-msg`）+「开始转换」。
  Widget _buildMessageLine() {
    final hint = _engineHint;
    final text = _message ?? hint ?? '';
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 4, 10, 8),
      child: Row(
        children: [
          Expanded(
            child: Text(
              text,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: _messageError || hint != null
                    ? LunacColors.danger
                    : LunacColors.textMuted,
                fontSize: 10.5,
                height: 1.3,
              ),
            ),
          ),
          const SizedBox(width: 8),
          SizedBox(
            height: 28,
            child: TextButton(
              onPressed: _canStart ? _start : null,
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.accent,
                backgroundColor: LunacColors.surfaceHover,
                visualDensity: VisualDensity.compact,
                padding: const EdgeInsets.symmetric(horizontal: 14),
              ),
              child: Text(
                _busy ? '转换中…' : '开始转换',
                style: const TextStyle(fontSize: 12),
              ),
            ),
          ),
        ],
      ),
    );
  }

  static String _basename(String path) {
    final index = path.lastIndexOf(RegExp(r'[\\/]'));
    return index >= 0 && index + 1 < path.length ? path.substring(index + 1) : path;
  }
}
