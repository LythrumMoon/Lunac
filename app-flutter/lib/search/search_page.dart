// M1 搜索主页面：搜索框 + 结果列表 + 键盘导航 + 动作执行。
//
// 边界（docs/flutter-migration.md §一）：
//   · 搜索与动作的业务逻辑在 Rust（`search_bridge.dart` 背后的 C ABI），本文件只管表现
//     与交互；数据源经 [SearchBackend] 注入，单测不碰 FFI。
//   · 平台动作（退出 / 拖拽）走 `platform/app_window.dart`，本文件不出现 `Platform.isXxx`。
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/app_window.dart';
import '../platform/appearance.dart';
import '../platform/attachment_window.dart';
import '../platform/clipboard.dart';
import '../platform/drop.dart';
import '../platform/file_preview.dart';
import '../platform/icons.dart';
import '../platform/index_status.dart';
import '../platform/pet.dart';
import '../platform/plugin_window.dart';
import '../platform/sandbox.dart';
import '../platform/settings.dart';
import '../platform/shell_actions.dart';
import '../platform/wake.dart';
import '../theme.dart';
import 'attachment_chip.dart';
import 'chat_panel.dart';
import 'clipboard_panel.dart';
import 'convert_panel.dart';
import 'drawer_metrics.dart';
import 'memo_panel.dart';
import 'music_panel.dart';
import 'ocr_panel.dart';
import 'pet_panel.dart';
import 'proxy_panel.dart';
import 'quick_launch_panel.dart';
import 'search_bridge.dart';
import 'search_result.dart';
import 'settings_panel.dart';
import 'tools_panel.dart';
import 'translate_panel.dart';

/// 结果行高度：固定值，键盘翻页时按它算滚动位置。
const double _rowExtent = 56;

/// 搜索栏高度。
const double _searchBarHeight = 52;

/// 搜索栏右端内嵌的圆形设置按钮直径（中空，后续放自定义图片）。
const double _settingsOrbSize = 40;

/// 圆形设置按钮距搜索栏右边缘的距离。
const double _orbRightInset = 6;

/// 页面左右留白。
const double _pageInsetX = 12;

/// 页面上下留白。
const double _pageInsetY = 8;

/// 结果面板的 1px 上下描边。
const double _resultsBorderExtent = 2;

/// 列表区最多显示 5 行，更多结果在列表内滚动。
const int _maxVisibleRows = 5;

/// 详细搜索大界面的窗口高度（与 v1 的 `DETAIL_HEIGHT` 一致，固定档、不做高度动画）。
const double _detailWindowHeight = 640;

/// 详细搜索的分类（与 v1 的六个 Tab 对齐）。
enum _DetailTab {
  all('全部'),
  apps('应用'),
  files('文件'),
  settings('设置'),
  actions('系统动作'),
  commands('命令'),
  web('网络搜索');

  const _DetailTab(this.label);
  final String label;
}

/// 详细搜索的一行：结果本体 + 目录条目的附加属性（危险 / 可提权）。
class _DetailRow {
  const _DetailRow(
    this.result, {
    this.danger = false,
    this.elevatable = false,
    this.group,
  });

  final SearchResult result;
  final bool danger;
  final bool elevatable;

  /// 非空 = 这一行前面要画一条分组标题（v1 `detail-group-title`）。
  final String? group;
}

/// 详细搜索的分组标题高度（选中滚动定位按它算偏移）。
const double _detailHeaderExtent = 24;

/// 「插件窗口已达上限」确认卡片的高度（与搜索栏拼成一档窗口高度）。
const double _confirmPanelHeight = 150;

/// 一个等待用户裁决的插件窗口打开请求（第 3 个窗口被上限挡住时）。
class _PendingLaunch {
  const _PendingLaunch({
    required this.id,
    required this.attachments,
    required this.query,
    required this.oldestId,
  });

  /// 用户想打开的插件。
  final String id;
  final List<String> attachments;
  final String query;

  /// 最早打开、需要被关掉的那个（空串 = 宿主没报上来）。
  final String oldestId;
}

/// 详细搜索的文件类型胶囊（值与 `indexer` 的 `Kind::as_str()` 一一对应）。
const List<(String?, String)> _fileKinds = [
  (null, '全部类型'),
  ('folder', '文件夹'),
  ('document', '文档'),
  ('image', '图片'),
  ('video', '视频'),
  ('audio', '音频'),
  ('archive', '压缩包'),
  ('program', '程序'),
  ('other', '其他'),
];

class SearchPage extends StatefulWidget {
  const SearchPage({
    super.key,
    this.backend,
    this.debounce = const Duration(milliseconds: 60),
    this.initialAttachments = const [],
    this.focusOnMount = true,
    this.revealDelay = Duration.zero,
    this.onQuit,
    this.onHide,
    this.onResize,
    this.onDrag,
    this.pollPetEvents = false,
  });

  /// 数据源；null 时使用 Rust 薄层（`NativeSearchBackend`）。
  final SearchBackend? backend;

  /// 输入去抖；单测可传 [Duration.zero]。
  final Duration debounce;

  /// 初始附件（文件路径）；运行中还可通过粘贴 / 拖放追加。单测用它注入。
  final List<String> initialAttachments;

  /// 挂载后是否自动聚焦搜索框。
  ///
  /// 生产环境传 false（首次启动只有圆形按钮，结果抽屉保持收起）；
  /// 唤出窗口时由 `searchRefreshTick` 流程主动聚焦。单测默认 true。
  final bool focusOnMount;

  /// 首次启动时「只显示圆形按钮」的停留时长（生产 1500ms；单测默认 0）。
  final Duration revealDelay;

  /// 退出动作；null 时关闭窗口。
  final Future<void> Function()? onQuit;

  /// 空查询时按 Esc 的隐藏动作；null 时隐藏主窗口（与 v1 一致）。
  final Future<void> Function()? onHide;

  /// 窗口高度同步；null 时走 `resizeMainWindowHeight`（单测可注入以断言尺寸）。
  final Future<void> Function(double height)? onResize;

  /// 搜索栏背景拖拽；null 时走 `startWindowDrag`（单测可注入以断言命中）。
  final Future<void> Function()? onDrag;

  /// 是否轮询桌宠事件队列（右键菜单「打开控制台」/ 渲染错误）。
  ///
  /// 生产实例必须为 true；单测默认 false —— 常驻定时器会让 `pumpAndSettle` 永不收敛。
  final bool pollPetEvents;

  @override
  State<SearchPage> createState() => _SearchPageState();
}

class _SearchPageState extends State<SearchPage> with TickerProviderStateMixin {
  late final SearchBackend _backend = widget.backend ?? NativeSearchBackend();
  final TextEditingController _controller = TextEditingController();
  final ScrollController _scroll = ScrollController();
  late final FocusNode _inputFocus = FocusNode(onKeyEvent: _onKey);

  /// 附件（文件 / 图片路径）。非空时结果区切成「附件优先」视图（v1 同语义）。
  late final List<String> _attachments = [...widget.initialAttachments];

  /// 首次启动：搜索框从左边拉开的动画（唤醒窗口时 widget 不重建，所以不会重播）。
  late final AnimationController _revealController = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 420),
  );
  late final Animation<double> _reveal = CurvedAnimation(
    parent: _revealController,
    curve: Curves.easeOutCubic,
  );

  /// 启动序列未走完时，结果区先不展开（等搜索框拉出来以后再决定）。
  bool _booting = true;

  Timer? _debounceTimer;

  /// 已下发的窗口高度（用来判断是"变高"还是"变矮"）。
  double _appliedWindowHeight = 0;

  /// 变矮时的延迟任务（等收起动画结束再缩窗口）。
  Timer? _shrinkTimer;

  /// 首次启动「只显示圆形按钮」的等待计时器。
  Timer? _revealDelayTimer;

  /// 预热抽屉第二个 engine 的计时器（可取消，单测销毁时不留下 pending timer）。
  Timer? _warmupTimer;

  List<SearchResult> _results = const [];

  /// 当前选中行；-1 = 无结果。
  int _selected = -1;

  /// 鼠标停留的行；副标题只在这一行上以动画显示。
  int? _hoveredIndex;

  /// 详细搜索大界面是否打开（双击搜索栏进入，Esc / 返回退出）。
  bool _detailOpen = false;
  _DetailTab _detailTab = _DetailTab.all;

  /// 设置面板是否打开（搜索栏右侧圆钮 / 「设置」结果项；与详细搜索互斥）。
  bool _settingsOpen = false;

  /// AI 对话面板是否打开（「AI 对话」/「AI 分析附件」入口）。
  bool _chatOpen = false;
  bool _clipboardOpen = false;
  bool _memoOpen = false;
  bool _quickOpen = false;
  bool _translateOpen = false;
  bool _ocrOpen = false;
  bool _convertOpen = false;
  bool _musicOpen = false;
  bool _petOpen = false;

  /// 桌宠控制台是否在**独立窗口**里开着（2026-11-19）：与内嵌兜底的 `_petOpen` 互斥。
  /// 独立窗口打开期间主窗不再轮询桌宠事件（窗口里的 `PetPanel` 自己轮询）。
  bool _petWindowOpen = false;
  bool _toolsOpen = false;
  bool _proxyOpen = false;
  String? _memoInitialId;
  String _translateQuery = '';
  List<String> _chatAttachments = const [];
  String _chatQuery = '';

  /// 插件窗口已达上限（两个）时待用户确认的请求；null = 没有待确认项。
  _PendingLaunch? _pendingLaunch;

  /// 插件窗口被关掉的回推（只用来恢复主窗置顶）。
  StreamSubscription<String>? _pluginClosedSubscription;

  /// 详细搜索里的文件类型过滤（null = 全部类型）。
  String? _detailFileKind;

  List<_DetailRow> _detailRows = const [];
  int _detailSelected = -1;
  int? _detailHovered;
  final ScrollController _detailScroll = ScrollController();

  /// 危险动作的二次确认：第一次 Enter / 点击只「上膛」，3 秒内再来一次才执行。
  String? _pendingDangerId;
  DateTime? _pendingDangerAt;

  /// 文件索引状态（详细搜索底栏）：进入大界面时读一次，之后每 2 秒刷新。
  FileIndexStatus? _indexStatus;
  Timer? _indexTimer;

  /// 桌宠事件轮询（只有生产实例会启动，见 [SearchPage.pollPetEvents]）。
  Timer? _petPollTimer;

  /// 预览窗「添加到对话」队列的轮询（没有对话面板/窗口时由主窗接管）。
  Timer? _attachPollTimer;

  /// 「重建索引」已发起：界面上先显示转圈，等 Rust 侧 status().scanning 接管。
  bool _indexRefreshing = false;

  /// 双击判定的上一次点击（`Listener` 只观察，不参与手势竞技场，不会干扰输入框选词）。
  DateTime? _lastClickAt;
  Offset? _lastClickPos;

  @override
  void initState() {
    super.initState();
    _results = _attachments.isEmpty
        ? _backend.search('')
        : _attachmentResults();
    _selected = _results.isEmpty ? -1 : 0;
    // 结果区只在「搜索框被选中」时展开（用户 2026-11-08 裁决）。
    _inputFocus.addListener(_onFocusChanged);
    // 附件抽屉是独立窗口：点它里面的 × 时由原生转回这里。
    startAttachmentDrawerBridge(_removeAttachment, onClear: _clearAttachments);
    // 插件窗与主窗的层级：主窗常驻置顶，有插件窗时让位（否则会把插件窗盖住）。
    _pluginClosedSubscription = pluginWindowClosed.listen((id) {
      if (id == 'pet' && mounted) {
        setState(() => _petWindowOpen = false);
      }
      unawaited(_syncMainTopmost());
    });
    _syncWindowHeight();
    // 系统文件拖放：runner 的 WM_DROPFILES → `lunac/drop` 通道。
    addFileDropListener(_addAttachments);
    // 唤出时带入的剪贴板文本：可能在界面挂载前就写好了，所以这里先消费一次。
    pendingSearchQuery.addListener(_applyPendingQuery);
    pendingAttachments.addListener(_applyPendingAttachments);
    searchRefreshTick.addListener(_onWakeRefresh);
    if (pendingSearchQuery.value.isNotEmpty) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _applyPendingQuery());
    }
    if (pendingAttachments.value.isNotEmpty) {
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => _applyPendingAttachments(),
      );
    }
    // 桌宠右键菜单的「打开控制台」经 C ABI 事件队列上来；面板自己开着时由面板轮询，
    // 这里只在面板关着时兜底（避免两边抢同一个队列）。
    if (widget.pollPetEvents) {
      _petPollTimer = Timer.periodic(
        const Duration(milliseconds: 900),
        (_) => _pollPetEvents(),
      );
    }
    // 预览窗右键「添加到对话」：对话面板/窗口开着时由它自己轮询（它知道要加进哪条会话），
    // 都没开时由主窗接管 —— 加进附件栏并直接开对话。
    _attachPollTimer = Timer.periodic(
      const Duration(seconds: 1),
      (_) => unawaited(_pollChatAttachments()),
    );
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _playReveal();
      // 初始附件（例如单测注入）也要同步到独立抽屉窗口。
      unawaited(syncAttachmentWindow(_attachments));
      // 预热抽屉的第二个 engine（窗口宽 0 = 隐藏）：首次粘贴文件时不必现建
      // 引擎，避免那一下卡顿。放在首帧之后，不拖慢主窗首帧与揭示动画。
      _warmupTimer = Timer(const Duration(milliseconds: 600), () {
        if (mounted) {
          unawaited(warmUpAttachmentDrawer());
        }
      });
    });
  }

  /// 桌宠事件轮询（面板关着时）：右键菜单「打开控制台」/ 渲染错误。
  void _pollPetEvents() {
    if (!mounted || _petOpen || _petWindowOpen) {
      return;
    }
    for (final event in pollPetEvents()) {
      if (event.type == 'console') {
        _openPet();
        return;
      }
      if (event.type == 'error' && event.message != null) {
        _toast('桌宠：${event.message}');
      }
    }
  }

  /// 预览窗右键「添加到对话」的兜底消费：对话没开着时才由主窗接手。
  ///
  /// 对话面板开着时由它自己的 1 秒轮询消费（同一队列 `take` 即排空，不会两边都加）。
  Future<void> _pollChatAttachments() async {
    if (!mounted || _chatOpen) {
      return;
    }
    final windows = await listPluginWindows();
    if (!mounted || windows.contains('chat')) {
      return;
    }
    final paths = takeChatAttachments();
    if (paths.isEmpty) {
      return;
    }
    _addAttachments(paths);
    _openChat(attachments: [..._attachments], query: '请分析这些文件');
  }

  /// 首次启动动画：先只展示圆形按钮 [SearchPage.revealDelay]，再拉开搜索栏，
  /// 最后按焦点决定结果抽屉。
  void _playReveal() {
    if (!mounted) {
      return;
    }
    if (widget.revealDelay <= Duration.zero) {
      unawaited(_runReveal());
      return;
    }
    // 用可取消的 Timer：widget 在等待期间被销毁时不能留下 pending timer。
    _revealDelayTimer = Timer(widget.revealDelay, () {
      if (mounted) {
        unawaited(_runReveal());
      }
    });
  }

  Future<void> _runReveal() async {
    await _revealController.forward();
    if (!mounted) {
      return;
    }
    setState(() => _booting = false);
    _syncWindowHeight();
  }

  @override
  void dispose() {
    removeFileDropListener(_addAttachments);
    stopAttachmentDrawerBridge();
    pendingSearchQuery.removeListener(_applyPendingQuery);
    pendingAttachments.removeListener(_applyPendingAttachments);
    searchRefreshTick.removeListener(_onWakeRefresh);
    _inputFocus.removeListener(_onFocusChanged);
    _debounceTimer?.cancel();
    _shrinkTimer?.cancel();
    _revealDelayTimer?.cancel();
    _warmupTimer?.cancel();
    _indexTimer?.cancel();
    _petPollTimer?.cancel();
    _attachPollTimer?.cancel();
    _pluginClosedSubscription?.cancel();
    _revealController.dispose();
    _controller.dispose();
    _scroll.dispose();
    _detailScroll.dispose();
    _inputFocus.dispose();
    _backend.dispose();
    super.dispose();
  }

  /// 搜索框获得焦点时展开结果抽屉；**失焦不收起**。
  ///
  /// 早期是「失焦就收起」，但桌面端点结果行 / 设置按钮都会让输入框短暂失焦，
  /// 面板先收起、点击就落空了（用户 2026-11-09 反馈）。现在展开一次就保持，
  /// 收起只发生在窗口隐藏 / Esc 空查询（下次显示时由唤出流程重新聚焦再展开）。
  void _onFocusChanged() {
    if (!mounted || !_inputFocus.hasFocus || _resultsOpen) {
      return;
    }
    setState(() => _resultsOpen = true);
    _syncWindowHeight();
  }

  /// 唤出时重跑当前查询（v1 在 `lunac-window-shown` 里做同一件事）。
  ///
  /// 首次启动刻意不聚焦（只留圆形按钮）；从这里开始才把焦点交还搜索框，
  /// 于是结果抽屉随之展开。
  void _onWakeRefresh() {
    if (!mounted) {
      return;
    }
    // 与 v1 一致：大界面 / 各面板不跨「隐藏 → 唤出」存活，唤出后回到简洁搜索。
    _exitDetail(silent: true);
    if (_settingsOpen) {
      _settingsOpen = false;
    }
    if (_chatOpen) {
      _chatOpen = false;
    }
    if (_clipboardOpen) {
      _clipboardOpen = false;
    }
    if (_memoOpen) {
      _memoOpen = false;
      _memoInitialId = null;
    }
    if (_quickOpen) {
      _quickOpen = false;
    }
    if (_translateOpen) {
      _translateOpen = false;
      _translateQuery = '';
    }
    if (_ocrOpen) {
      _ocrOpen = false;
    }
    if (_convertOpen) {
      _convertOpen = false;
    }
    if (_musicOpen) {
      _musicOpen = false;
    }
    if (_petOpen) {
      _petOpen = false;
    }
    if (_toolsOpen) {
      _toolsOpen = false;
    }
    if (_proxyOpen) {
      _proxyOpen = false;
    }
    // 待确认的「关最早窗口再开下一个」不跨唤出存活。
    _pendingLaunch = null;
    _runSearch();
    // 面板关掉后搜索栏要等重建才恢复可编辑，焦点延到下一帧再交还。
    _focusInputAfterUnlock();
  }

  /// 把唤出时读到的剪贴板文本填进搜索框。
  ///
  /// 只在**搜索框为空**时填：用户已经打了一半的字不该被覆盖（v1 是无条件填，
  /// 这里刻意收窄，避免热键切回窗口时把正在输入的查询冲掉）。
  void _applyPendingQuery() {
    final text = pendingSearchQuery.value;
    if (text.isEmpty) {
      return;
    }
    pendingSearchQuery.value = '';
    if (!mounted || _controller.text.isNotEmpty) {
      return;
    }
    _debounceTimer?.cancel();
    _controller.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
    _inputFocus.requestFocus();
    _runSearch();
  }

  /// 把唤出时读到的剪贴板文件 / 图片并入附件栏。
  ///
  /// 去重、上限与附件抽屉同步都复用 `_addAttachments`（粘贴 / 拖放同一条路）。
  void _applyPendingAttachments() {
    final paths = pendingAttachments.value;
    if (paths.isEmpty) {
      return;
    }
    pendingAttachments.value = const [];
    if (!mounted) {
      return;
    }
    _addAttachments(paths);
  }

  // ── 查询 ───────────────────────────────────────────────────────

  void _onQueryChanged(String _) {
    _debounceTimer?.cancel();
    _debounceTimer = Timer(widget.debounce, _runSearch);
    // 输入变了先回到第一行，旧行上的 hover 也一并清掉。
    setState(() {
      _selected = _results.isEmpty ? -1 : 0;
      _hoveredIndex = null;
    });
  }

  void _runSearch() {
    if (_detailOpen) {
      _runDetailSearch();
      return;
    }
    final results = _attachments.isEmpty
        ? _backend.search(_controller.text)
        : _attachmentResults();
    if (!mounted) {
      return;
    }
    setState(() {
      _results = results;
      _selected = results.isEmpty ? -1 : 0;
      _hoveredIndex = null;
    });
    _syncWindowHeight();
  }

  // ── 详细搜索大界面（双击搜索栏进入）─────────────────────────────

  /// 搜索栏上的双击检测：`Listener` 只观察指针，不参与手势竞技场，
  /// 因此不会抢走输入框的双击选词。
  void _onRowPointerDown(PointerDownEvent event) {
    // 面板打开时搜索栏是锁定态，双击不再进入详细搜索（v1 同理：`!pluginActive`）。
    if (event.buttons != kPrimaryButton || _anyPanelOpen) {
      return;
    }
    final now = DateTime.now();
    final lastAt = _lastClickAt;
    final lastPos = _lastClickPos;
    if (lastAt != null &&
        lastPos != null &&
        now.difference(lastAt) <= const Duration(milliseconds: 400) &&
        (event.localPosition - lastPos).distance <= 6) {
      _lastClickAt = null;
      _lastClickPos = null;
      _enterDetail();
      return;
    }
    _lastClickAt = now;
    _lastClickPos = event.localPosition;
  }

  void _enterDetail() {
    if (_detailOpen) {
      return;
    }
    setState(() {
      _detailOpen = true;
      _detailTab = _DetailTab.all;
      _detailFileKind = null;
    });
    _runDetailSearch();
    _syncWindowHeight();
    _inputFocus.requestFocus();
    // 底栏的索引状态：先读一次，之后 2 秒一轮（v1 同频率）。
    _refreshIndexStatus();
    _indexTimer?.cancel();
    _indexTimer = Timer.periodic(
      const Duration(seconds: 2),
      (_) => _refreshIndexStatus(),
    );
  }

  void _exitDetail({bool silent = false}) {
    if (!_detailOpen) {
      return;
    }
    _indexTimer?.cancel();
    _indexTimer = null;
    setState(() => _detailOpen = false);
    if (silent) {
      return;
    }
    _runSearch();
    _syncWindowHeight();
    _inputFocus.requestFocus();
  }

  /// 读一次索引状态；从「未就绪 / 扫描中」变成「有数据且不在扫」时补跑一次查询
  /// （v1 同口径：首次进大界面时索引常常还没载入，不补跑就一直是空列表）。
  void _refreshIndexStatus() {
    final status = readFileIndexStatus();
    if (!mounted || !_detailOpen) {
      return;
    }
    final wasPending =
        _indexStatus == null ||
        _indexStatus!.scanning ||
        _indexStatus!.notLoaded;
    setState(() {
      _indexStatus = status;
      // Rust 侧已经不在扫 = 我们这边的「刚发起」标记也结束。
      if (status != null && !status.scanning) {
        _indexRefreshing = false;
      }
    });
    if (wasPending && status != null && !status.scanning && status.count > 0) {
      _runDetailSearch();
    }
  }

  /// 底栏刷新按钮 = 重建文件索引（后台扫盘，立即返回）。
  void _rebuildIndex() {
    if (!rebuildFileIndex()) {
      _toast('文件索引不可用（rust 桥接未加载）');
      return;
    }
    setState(() => _indexRefreshing = true);
    _toast('正在重建文件索引…');
  }

  void _selectDetailTab(_DetailTab tab) {
    if (_detailTab == tab) {
      return;
    }
    setState(() => _detailTab = tab);
    _runDetailSearch();
  }

  void _selectDetailFileKind(String? kind) {
    if (_detailFileKind == kind) {
      return;
    }
    setState(() => _detailFileKind = kind);
    _runDetailSearch();
  }

  /// 详细搜索的数据源：应用 / 文件走「条数更多」的查询，设置 / 系统动作走静态目录，
  /// 其余复用紧凑搜索的目录。
  void _runDetailSearch() {
    final query = _controller.text;
    final catalog = _backend.systemCatalog(query);
    final settings = [
      for (final entry in catalog)
        if (entry.id.startsWith('setting:')) _catalogRow(entry),
    ];
    final actions = [
      for (final entry in catalog)
        if (entry.id.startsWith('action:')) _catalogRow(entry),
    ];
    List<_DetailRow> asRows(Iterable<SearchResult> results, {String? group}) {
      final rows = [for (final result in results) _DetailRow(result)];
      if (rows.isEmpty || group == null) {
        return rows;
      }
      return [_DetailRow(rows.first.result, group: group), ...rows.skip(1)];
    }

    List<_DetailRow> grouped(List<_DetailRow> rows, String group) {
      if (rows.isEmpty) {
        return rows;
      }
      return [
        _DetailRow(
          rows.first.result,
          danger: rows.first.danger,
          elevatable: rows.first.elevatable,
          group: group,
        ),
        ...rows.skip(1),
      ];
    }

    final List<_DetailRow> rows;
    switch (_detailTab) {
      case _DetailTab.apps:
        rows = asRows(_backend.searchApps(query, limit: 60), group: '应用');
      case _DetailTab.files:
        rows = asRows(
          _backend.searchFiles(query, kind: _detailFileKind, limit: 80),
          group: '文件',
        );
      case _DetailTab.settings:
        rows = grouped(settings, 'Windows 设置');
      case _DetailTab.actions:
        rows = grouped(actions, '系统动作');
      case _DetailTab.commands:
        rows = asRows(
          _backend
              .search(query)
              .where((r) => r.kind == 'command' || r.kind == 'folder'),
          group: '命令',
        );
      case _DetailTab.web:
        rows = asRows(
          _backend.search(query).where((r) => r.kind == 'web'),
          group: '网络搜索',
        );
      case _DetailTab.all:
        // 与 v1 同序：应用 → 文件 → 设置 → 系统动作 → 命令 → 网络搜索。
        final commands = <SearchResult>[];
        final web = <SearchResult>[];
        for (final result in _backend.search(query)) {
          if (result.kind == 'app' || result.kind == 'file') {
            continue;
          }
          if (result.kind == 'web') {
            web.add(result);
          } else {
            commands.add(result);
          }
        }
        rows = [
          ...asRows(_backend.searchApps(query, limit: 20), group: '应用'),
          ...asRows(
            _backend.searchFiles(query, kind: _detailFileKind, limit: 30),
            group: '文件',
          ),
          ...grouped(settings, 'Windows 设置'),
          ...grouped(actions, '系统动作'),
          ...asRows(commands, group: '命令'),
          ...asRows(web, group: '网络搜索'),
        ];
    }
    if (!mounted) {
      return;
    }
    setState(() {
      _detailRows = rows;
      _detailSelected = rows.isEmpty ? -1 : 0;
      _detailHovered = null;
      _pendingDangerId = null;
      _pendingDangerAt = null;
    });
  }

  /// 目录条目 → 可渲染的一行（`kind` 统一用 command，图标走类型图标）。
  _DetailRow _catalogRow(SystemCatalogEntry entry) {
    return _DetailRow(
      SearchResult(
        id: entry.id,
        title: entry.title,
        subtitle: entry.subtitle,
        kind: 'command',
      ),
      danger: entry.danger,
      elevatable: entry.elevatable,
    );
  }

  void _moveDetailSelection(int delta) {
    if (_detailRows.isEmpty) {
      return;
    }
    final next = _detailSelected + delta;
    setState(() {
      _detailSelected = next < 0
          ? 0
          : (next >= _detailRows.length ? _detailRows.length - 1 : next);
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _ensureDetailSelectedVisible();
    });
  }

  void _ensureDetailSelectedVisible() {
    if (!_detailScroll.hasClients || _detailSelected < 0) {
      return;
    }
    final position = _detailScroll.position;
    // 列表项含可变高的分组标题，选中行偏移得逐项累加（不能再按固定行高乘）。
    var itemTop = 0.0;
    for (var i = 0; i < _detailSelected; i++) {
      itemTop += _rowExtent;
      if (_detailRows[i].group != null) {
        itemTop += _detailHeaderExtent;
      }
    }
    final header = _detailRows[_detailSelected].group != null
        ? _detailHeaderExtent
        : 0.0;
    final top = itemTop + header;
    final bottom = top + _rowExtent;
    final visibleTop = position.pixels;
    final visibleBottom = visibleTop + position.viewportDimension;
    double? target;
    if (top < visibleTop) {
      target = top;
    } else if (bottom > visibleBottom) {
      target = bottom - position.viewportDimension;
    }
    if (target == null) {
      return;
    }
    _detailScroll.animateTo(
      target.clamp(position.minScrollExtent, position.maxScrollExtent),
      duration: const Duration(milliseconds: 120),
      curve: Curves.easeOut,
    );
  }

  /// 详细搜索里回车 / 点击：与紧凑搜索同一套动作执行链。
  ///
  /// · Shift+Enter = 以管理员身份运行（应用 / 文件 / 白名单动作；v1 同语义）；
  /// · 当前分类没有结果时回车 = 直接网页搜索（v1 的兜底）；
  /// · 关机 / 重启这类危险动作要二次确认：第一次只「上膛」并提示，3 秒内再来一次
  ///   才真的执行（与 v1 同语义；提权分支按 v1 顺序放在确认之前）。
  void _activateDetailSelected({bool elevate = false}) {
    if (_detailSelected < 0 || _detailSelected >= _detailRows.length) {
      final query = _controller.text.trim();
      if (query.isEmpty) {
        return;
      }
      if (_backend.runAction('web:$query') == SearchActionOutcome.executed) {
        _afterExecuted();
      } else {
        _toast('无法打开网页搜索');
      }
      return;
    }
    final row = _detailRows[_detailSelected];
    final result = row.result;
    if (result.id == 'ai-attachments') {
      _openChat(attachments: [..._attachments], query: '请分析这些文件');
      return;
    }
    if (elevate) {
      final canElevate =
          row.elevatable ||
          result.id.startsWith('app:') ||
          result.id.startsWith('file:');
      if (!canElevate) {
        _toast('该结果不支持以管理员身份运行');
        return;
      }
      _runElevated(result);
      return;
    }
    if (row.danger) {
      final now = DateTime.now();
      final armed =
          _pendingDangerId == result.id &&
          _pendingDangerAt != null &&
          now.difference(_pendingDangerAt!) <= const Duration(seconds: 3);
      if (!armed) {
        setState(() {
          _pendingDangerId = result.id;
          _pendingDangerAt = now;
        });
        _toast('再按一次 Enter 确认：${result.title}');
        return;
      }
      _pendingDangerId = null;
      _pendingDangerAt = null;
    }
    switch (_backend.runAction(result.id)) {
      case SearchActionOutcome.executed:
        _afterExecuted();
      case SearchActionOutcome.uiHandled:
        _handleUiAction(result);
      case SearchActionOutcome.unknown:
        _toast('未知动作：${result.id}');
      case SearchActionOutcome.failed:
        _toast('执行失败：${result.title}');
    }
  }

  /// 以管理员身份运行（会弹 UAC）。id 原样带前缀交给 Rust：`admin:action:<id>` /
  /// `admin:app:<path>` / `admin:file:<path>`。
  void _runElevated(SearchResult result) {
    switch (_backend.runAction('admin:${result.id}')) {
      case SearchActionOutcome.executed:
        _afterExecuted();
      case SearchActionOutcome.uiHandled:
        _handleUiAction(result);
      case SearchActionOutcome.unknown:
      case SearchActionOutcome.failed:
        _toast('提权启动失败：${result.title}');
    }
  }

  /// 「附件优先」视图（v1：有泡泡时不再列应用 / 插件结果）。
  ///
  /// 当前接的是「打开附件 + AI 分析占位」：AI 分析要等 M2 的对话链路，
  /// 文件本身可以像普通结果一样回车打开 / 右键定位。
  List<SearchResult> _attachmentResults() {
    final needle = _controller.text.trim().toLowerCase();
    final rows = <SearchResult>[];
    for (final path in _attachments) {
      final name = fileNameOf(path);
      if (needle.isNotEmpty && !name.toLowerCase().contains(needle)) {
        continue;
      }
      rows.add(
        SearchResult(
          id: 'file:$path',
          title: name,
          subtitle: path,
          kind: 'file',
        ),
      );
    }
    rows.add(
      const SearchResult(
        id: 'ai-attachments',
        title: 'AI 分析附件',
        subtitle: '让 Lunac 助手分析这些文件',
        kind: 'command',
      ),
    );
    rows.add(
      const SearchResult(
        id: 'quick-launch',
        title: '用快捷启动打开',
        subtitle: '把附件注册为自定义启动项',
        kind: 'command',
      ),
    );
    return rows;
  }

  /// 追加附件（粘贴 / 拖放都会走这里）；按小写路径去重。
  void _addAttachments(List<String> paths) {
    if (paths.isEmpty) {
      return;
    }
    final existing = _attachments.map((path) => path.toLowerCase()).toSet();
    var added = false;
    for (final path in paths) {
      if (path.isEmpty) {
        continue;
      }
      final key = path.toLowerCase();
      if (existing.contains(key)) {
        continue;
      }
      existing.add(key);
      _attachments.add(path);
      added = true;
    }
    if (added) {
      unawaited(syncAttachmentWindow(_attachments));
      _runSearch();
    }
  }

  void _removeAttachment(int index) {
    if (index < 0 || index >= _attachments.length) {
      return;
    }
    _attachments.removeAt(index);
    unawaited(syncAttachmentWindow(_attachments));
    _runSearch();
  }

  /// 附件抽屉顶部「一键删除」：清空全部附件并回到普通结果视图。
  void _clearAttachments() {
    if (_attachments.isEmpty) {
      return;
    }
    _attachments.clear();
    unawaited(syncAttachmentWindow(_attachments));
    _runSearch();
    _syncWindowHeight();
  }

  /// 可见结果行数；0 条结果时按一行空状态算。
  int get _visibleRows => _results.isEmpty
      ? 1
      : (_results.length > _maxVisibleRows ? _maxVisibleRows : _results.length);

  /// 结果抽屉是否已展开：搜索框第一次拿到焦点后保持展开（见 `_onFocusChanged`）。
  bool _resultsOpen = false;

  /// 是否有任一 640 大面板打开（详细搜索也在内）。
  bool get _anyPanelOpen =>
      _detailOpen ||
      _settingsOpen ||
      _chatOpen ||
      _clipboardOpen ||
      _memoOpen ||
      _quickOpen ||
      _translateOpen ||
      _ocrOpen ||
      _convertOpen ||
      _musicOpen ||
      _petOpen ||
      _toolsOpen ||
      _proxyOpen;

  /// 搜索栏锁定态：面板打开时输入框只读、不接收焦点（v1 `plugin-locked`）。
  ///
  /// 详细搜索除外：v1 大界面有独立的 `detail-input`，v2 则是复用这条搜索栏
  /// 做过滤（`_runDetailSearch` 读 `_controller.text`），锁上就没法搜索了。
  bool get _searchBarLocked =>
      _settingsOpen ||
      _chatOpen ||
      _clipboardOpen ||
      _memoOpen ||
      _quickOpen ||
      _translateOpen ||
      _ocrOpen ||
      _convertOpen ||
      _musicOpen ||
      _petOpen ||
      _toolsOpen ||
      _proxyOpen ||
      _pendingLaunch != null;

  /// 面板开合时同步平台层：失焦不隐藏 + 输入框让出焦点。
  void _syncPanelLock() {
    setAutoHideSuspended(_anyPanelOpen);
    if (_searchBarLocked && _inputFocus.hasFocus) {
      _inputFocus.unfocus();
    }
  }

  /// 面板关闭后把焦点交还搜索框。
  ///
  /// 锁定态下 `canRequestFocus` 为 false；它要等这一帧重建（`TextField` 在
  /// `didUpdateWidget` 里写回 FocusNode）才恢复，所以焦点必须延到下一帧再要。
  void _focusInputAfterUnlock() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_searchBarLocked) {
        _inputFocus.requestFocus();
      }
    });
  }

  /// 结果区只在「启动动画走完 + 面板已展开（或有查询词 / 附件）」时展开。
  bool get _resultsExpanded =>
      !_booting &&
      (_resultsOpen || _attachments.isNotEmpty || _controller.text.isNotEmpty);

  /// 窗口高度 = 上下留白 + 搜索栏 + 结果面板（展开时）。
  ///
  /// 附件不在主窗口里画：它是独立渲染的抽屉窗口（见 `lib/attachment_drawer_main.dart`），
  /// 由 runner 的原生层负责吸附在主窗口左端。
  double get _windowHeight {
    if (_pendingLaunch != null) {
      // 「插件窗口已达上限」：搜索栏 + 一张确认卡片（不展开结果抽屉）。
      return _pageInsetY * 2 + _searchBarHeight + _confirmPanelHeight;
    }
    if (_detailOpen ||
        _settingsOpen ||
        _chatOpen ||
        _clipboardOpen ||
        _memoOpen ||
        _quickOpen ||
        _translateOpen ||
        _ocrOpen ||
        _convertOpen ||
        _musicOpen ||
        _petOpen ||
        _toolsOpen ||
        _proxyOpen) {
      // 详细搜索 / 设置 / 对话 / 剪贴板 / 备忘录 / 快捷启动 / 翻译 / OCR / 音乐 /
      // 工具编辑器 / 代理面板都是固定档（v1 DETAIL_HEIGHT）。
      return _detailWindowHeight;
    }
    var height = _pageInsetY * 2 + _searchBarHeight;
    if (_resultsExpanded) {
      height += _resultsBorderExtent + _rowExtent * _visibleRows;
    }
    return height;
  }

  /// 抽屉是否正好顶到结果面板的底边。
  ///
  /// 抽屉从搜索栏顶部向下排，文件多到把面板撑到上限时，它的底边与结果面板
  /// 的底边重合；此时结果面板左下角必须让位成直角，否则接缝处会露出缺口。
  bool get _drawerCoversPanelBottom {
    if (_attachments.isEmpty) {
      return false;
    }
    final drawerBottom =
        _pageInsetY + drawerPanelHeightFor(_attachments.length);
    return drawerBottom >= _windowHeight - _pageInsetY - 0.5;
  }

  /// 把窗口高度贴合到当前内容。
  ///
  /// 变高立即下发；**变矮延迟 140ms**，等结果抽屉的收起动画走完再缩窗口 ——
  /// 否则内容比窗口高，RenderFlex 会画黄色 overflow 警告条（切换焦点时的问题）。
  void _syncWindowHeight() {
    _syncPanelLock();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) {
        return;
      }
      final target = _windowHeight;
      _shrinkTimer?.cancel();
      if (target >= _appliedWindowHeight) {
        _appliedWindowHeight = target;
        _applyWindowHeight(target);
        return;
      }
      _shrinkTimer = Timer(const Duration(milliseconds: 140), () {
        if (!mounted) {
          return;
        }
        _appliedWindowHeight = _windowHeight;
        _applyWindowHeight(_appliedWindowHeight);
      });
    });
  }

  void _applyWindowHeight(double height) {
    final resize = widget.onResize;
    if (resize != null) {
      resize(height);
    } else {
      unawaited(resizeMainWindowHeight(height));
    }
  }

  /// 搜索栏背景的拖动 = 移动窗口（输入框里的拖动由输入框自己处理）。
  Future<void> _startDrag() async {
    final drag = widget.onDrag;
    if (drag != null) {
      await drag();
    } else {
      await startWindowDrag();
    }
  }

  // ── 键盘 ───────────────────────────────────────────────────────

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyUpEvent) {
      return KeyEventResult.ignored;
    }
    // 「插件窗口已达上限」是一张二选一的卡片：Enter = 关最早并继续，Esc = 取消。
    if (_pendingLaunch != null) {
      if (event.logicalKey == LogicalKeyboardKey.enter ||
          event.logicalKey == LogicalKeyboardKey.numpadEnter) {
        unawaited(_confirmPendingLaunch());
        return KeyEventResult.handled;
      }
      if (event.logicalKey == LogicalKeyboardKey.escape) {
        _cancelPendingLaunch();
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    // 设置面板开着时 Esc 关闭面板（焦点可能落在面板里的按钮上，这里兜一层）。
    if (_settingsOpen) {
      if (event.logicalKey == LogicalKeyboardKey.escape) {
        _closeSettings();
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    // 对话面板同理（面板自己也会处理 Esc，这里是焦点回到搜索框时的兜底）。
    if (_chatOpen) {
      if (event.logicalKey == LogicalKeyboardKey.escape) {
        _closeChat();
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    // 剪贴板 / 备忘录 / 快捷启动 / 翻译 / OCR / 音乐 / 桌宠 / 工具编辑器 / 代理面板的
    // Esc 兜底（焦点回到搜索框时）。
    if (_clipboardOpen ||
        _memoOpen ||
        _quickOpen ||
        _translateOpen ||
        _convertOpen ||
        _ocrOpen ||
        _musicOpen ||
        _petOpen ||
        _toolsOpen ||
        _proxyOpen) {
      if (event.logicalKey == LogicalKeyboardKey.escape) {
        if (_clipboardOpen) {
          _closeClipboard();
        }
        if (_memoOpen) {
          _closeMemo();
        }
        if (_quickOpen) {
          _closeQuickLaunch();
        }
        if (_translateOpen) {
          _closeTranslate();
        }
        if (_convertOpen) {
          _closeConvert();
        }
        if (_ocrOpen) {
          _closeOcr();
        }
        if (_musicOpen) {
          _closeMusic();
        }
        if (_petOpen) {
          _closePet();
        }
        if (_toolsOpen) {
          _closeTools();
        }
        if (_proxyOpen) {
          _closeProxy();
        }
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    // 详细搜索有自己的一套导航（Esc 退出 / Tab 切分类 / 上下选行）。
    if (_detailOpen) {
      return _onDetailKey(event);
    }
    // 输入框空白时按退格：先删最后一个附件（v1 的泡泡删除）。
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.backspace &&
        _controller.text.isEmpty &&
        _attachments.isNotEmpty) {
      _removeAttachment(_attachments.length - 1);
      return KeyEventResult.handled;
    }
    // Ctrl+V：剪贴板里是文件时加附件并吞掉；否则交给输入框做普通文本粘贴。
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.keyV &&
        HardwareKeyboard.instance.isControlPressed) {
      final files = readClipboardFilePaths();
      if (files.isNotEmpty) {
        _addAttachments(files);
        return KeyEventResult.handled;
      }
    }
    switch (event.logicalKey) {
      case LogicalKeyboardKey.arrowDown:
        _moveSelection(1);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.arrowUp:
        _moveSelection(-1);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.enter:
      case LogicalKeyboardKey.numpadEnter:
        if (event is KeyDownEvent) {
          _activateSelected();
        }
        return KeyEventResult.handled;
      case LogicalKeyboardKey.escape:
        if (event is KeyDownEvent) {
          _onEscape();
        }
        return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  /// 详细搜索的键盘：Esc 退出、↑↓ 选行、Enter 执行、Tab / Shift+Tab 切分类。
  KeyEventResult _onDetailKey(KeyEvent event) {
    if (event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    switch (event.logicalKey) {
      case LogicalKeyboardKey.escape:
        _exitDetail();
        return KeyEventResult.handled;
      case LogicalKeyboardKey.arrowDown:
        _moveDetailSelection(1);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.arrowUp:
        _moveDetailSelection(-1);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.enter:
      case LogicalKeyboardKey.numpadEnter:
        _activateDetailSelected(
          elevate: HardwareKeyboard.instance.isShiftPressed,
        );
        return KeyEventResult.handled;
      case LogicalKeyboardKey.tab:
        final tabs = _DetailTab.values;
        final step = HardwareKeyboard.instance.isShiftPressed ? -1 : 1;
        final next = (_detailTab.index + step + tabs.length) % tabs.length;
        _selectDetailTab(tabs[next]);
        return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void _moveSelection(int delta) {
    if (_results.isEmpty) {
      return;
    }
    final next = _selected + delta;
    setState(() {
      _selected = next < 0
          ? 0
          : (next >= _results.length ? _results.length - 1 : next);
    });
    WidgetsBinding.instance.addPostFrameCallback(
      (_) => _ensureSelectedVisible(),
    );
  }

  void _ensureSelectedVisible() {
    if (!_scroll.hasClients || _selected < 0) {
      return;
    }
    final position = _scroll.position;
    final top = _selected * _rowExtent;
    final bottom = top + _rowExtent;
    final visibleTop = position.pixels;
    final visibleBottom = visibleTop + position.viewportDimension;
    double? target;
    if (top < visibleTop) {
      target = top;
    } else if (bottom > visibleBottom) {
      target = bottom - position.viewportDimension;
    }
    if (target == null) {
      return;
    }
    _scroll.animateTo(
      target.clamp(position.minScrollExtent, position.maxScrollExtent),
      duration: const Duration(milliseconds: 120),
      curve: Curves.easeOut,
    );
  }

  void _clearQuery() {
    if (_controller.text.isEmpty) {
      return;
    }
    _controller.clear();
    _debounceTimer?.cancel();
    _runSearch();
  }

  /// Esc 语义与 v1 一致：先删附件（有泡泡时），再清空查询词，最后才隐藏窗口。
  void _onEscape() {
    if (_pendingLaunch != null) {
      _cancelPendingLaunch();
      return;
    }
    if (_attachments.isNotEmpty) {
      _removeAttachment(_attachments.length - 1);
      return;
    }
    if (_controller.text.isNotEmpty) {
      _clearQuery();
      return;
    }
    final hide = widget.onHide;
    if (hide != null) {
      hide();
    } else {
      hideMainWindow();
    }
  }

  // ── 动作 ───────────────────────────────────────────────────────

  void _activateSelected() {
    if (_selected < 0 || _selected >= _results.length) {
      // v1 语义：没有可执行项时把当前查询交给 AI；空查询就只打开对话面板。
      final query = _controller.text.trim();
      if (query.isEmpty) {
        _openChat();
      } else {
        _openChat(query: query);
      }
      return;
    }

    final result = _results[_selected];
    if (result.id == 'ai-attachments') {
      _openChat(attachments: [..._attachments], query: '请分析这些文件');
      return;
    }
    final outcome = _backend.runAction(result.id);
    switch (outcome) {
      case SearchActionOutcome.executed:
        _afterExecuted();
      case SearchActionOutcome.uiHandled:
        _handleUiAction(result);
      case SearchActionOutcome.unknown:
        _toast('未知动作：${result.id}');
      case SearchActionOutcome.failed:
        _toast('执行失败：${result.title}');
    }
  }

  /// 动作已执行（启动应用 / 打开目录 / Web 搜索）：清空查询并收起窗口（与 v1 一致）。
  void _afterExecuted() {
    _detailOpen = false;
    _indexTimer?.cancel();
    _indexTimer = null;
    _attachments.clear();
    unawaited(syncAttachmentWindow(_attachments));
    _controller.clear();
    _debounceTimer?.cancel();
    _runSearch();
    final hide = widget.onHide;
    if (hide != null) {
      hide();
    } else {
      hideMainWindow();
    }
  }

  void _handleUiAction(SearchResult result) {
    // 「用 AI 提问」兜底行：id = `ai:<查询词>`，打开对话面板并预填。
    if (result.id.startsWith('ai:')) {
      _openChat(query: result.id.substring('ai:'.length));
      return;
    }
    switch (result.id) {
      case 'quit':
        final quit = widget.onQuit;
        if (quit != null) {
          quit();
        } else {
          quitApp();
        }
      case 'ai':
        _openChat();
      case 'settings':
        _openSettings();
      case 'clipboard-history':
        _openClipboard();
      case 'memo':
        _openMemo();
      case 'quick-launch':
        _openQuickLaunch();
      case 'translate':
        // 搜索栏里的原文带进来（面板负责剥掉「翻译」调用词后预填）。
        _openTranslate(query: _controller.text);
      case 'ocr':
        _openOcr();
      case 'convert':
        _openConvert();
      case 'music':
        _openMusic();
      case 'pet':
        _openPet();
      case 'proxy':
        _openProxy();
      case 'tool-editor':
        _openTools();
      default:
        if (result.id.startsWith('memo:')) {
          _openMemo(memoId: result.id.substring('memo:'.length));
        } else {
          _toast('该动作尚未接入：${result.id}');
        }
    }
  }

  /// 打开设置面板（搜索栏右侧圆钮 / 「设置」结果项）。
  ///
  /// 与详细搜索 / 对话面板互斥：先静默退出其它大面板，再切面板并同步窗口高度。
  void _openSettings() {
    if (_settingsOpen) {
      return;
    }
    _exitDetail(silent: true);
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _convertOpen = false;
    _ocrOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    setState(() => _settingsOpen = true);
    _syncWindowHeight();
  }

  void _closeSettings() {
    if (!_settingsOpen) {
      return;
    }
    setState(() => _settingsOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开 AI 对话（「AI 对话」/「AI 分析附件」入口）。
  ///
  /// [attachments] 从附件泡芙带进来；[query] 预填输入框（不自动发送，用户确认后再发）。
  /// 首选独立窗口；没有窗口宿主（单测 / Linux）时回退到主窗内的 640 档面板。
  void _openChat({List<String> attachments = const [], String query = ''}) {
    unawaited(
      _launchPlugin('chat', attachments: attachments, query: query).then((
        opened,
      ) {
        if (!opened && mounted) {
          _showChatPanel(attachments: attachments, query: query);
        }
      }),
    );
  }

  void _showChatPanel({
    List<String> attachments = const [],
    String query = '',
  }) {
    if (_chatOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _convertOpen = false;
    _ocrOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    _chatAttachments = attachments;
    _chatQuery = query;
    setState(() => _chatOpen = true);
    _syncWindowHeight();
  }

  void _closeChat() {
    if (!_chatOpen) {
      return;
    }
    setState(() => _chatOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  // ── 插件独立窗口（2026-11-13 用户裁决）────────────────────────────
  //
  // 最多同时两个窗口（C++ 侧硬上限）。第三个请求会带 `limit` 回来，这里先问用户：
  // 「关闭最早打开的 X 并继续」还是「取消」——两个选择都不隐式丢数据。

  /// 打开一个插件窗口。返回 false = 没有窗口宿主（单测 / Linux），调用方回退内嵌面板。
  Future<bool> _launchPlugin(
    String id, {
    List<String> attachments = const [],
    String query = '',
  }) async {
    final result = await openPluginWindow(
      id,
      attachments: attachments,
      query: query,
    );
    if (!mounted) {
      return true;
    }
    switch (result) {
      case PluginOpenResult.opened:
        unawaited(_syncMainTopmost());
        return true;
      case PluginOpenResult.unavailable:
        return false;
      case PluginOpenResult.limitReached:
        final oldest = await oldestPluginWindow();
        if (!mounted) {
          return true;
        }
        setState(() {
          _pendingLaunch = _PendingLaunch(
            id: id,
            attachments: attachments,
            query: query,
            oldestId: oldest,
          );
        });
        _syncWindowHeight();
        return true;
    }
  }

  /// 主窗置顶只在「没有插件窗」时成立：有插件窗时让位，否则 680 宽的主窗会盖住它。
  Future<void> _syncMainTopmost() async {
    final count = await pluginWindowCount();
    await setMainWindowTopmost(count == 0);
  }

  void _cancelPendingLaunch() {
    if (_pendingLaunch == null) {
      return;
    }
    setState(() => _pendingLaunch = null);
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 「关闭最早的窗口并继续」：等 C++ 回推 `closed` 再开下一个，避免又撞上限。
  Future<void> _confirmPendingLaunch() async {
    final pending = _pendingLaunch;
    if (pending == null) {
      return;
    }
    setState(() => _pendingLaunch = null);
    _syncWindowHeight();
    if (pending.oldestId.isNotEmpty) {
      // 自己订阅、自己在超时后取消：`Future.firstWhere` 的超时不会释放监听。
      final closed = Completer<void>();
      late final StreamSubscription<String> subscription;
      subscription = pluginWindowClosed.listen((id) {
        if (id == pending.oldestId && !closed.isCompleted) {
          closed.complete();
        }
      });
      await closePluginWindow(pending.oldestId);
      try {
        await closed.future.timeout(const Duration(seconds: 2));
      } on TimeoutException {
        // 关窗没有回推（宿主异常）：不再硬开，如实提示。
        _toast('关闭「${_pluginTitle(pending.oldestId)}」失败，请稍后重试');
        return;
      } finally {
        await subscription.cancel();
      }
    }
    if (!mounted) {
      return;
    }
    await openPluginWindow(
      pending.id,
      attachments: pending.attachments,
      query: pending.query,
    );
    await _syncMainTopmost();
  }

  /// 插件 id → 中文标题（窗口 / 确认卡片共用，未知 id 原样显示）。
  String _pluginTitle(String id) => kPluginWindowSpecs[id]?.title ?? id;

  /// 打开剪贴板历史面板（「剪贴板历史」结果项）。
  void _openClipboard() {
    if (_clipboardOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _ocrOpen = false;
    _convertOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    setState(() => _clipboardOpen = true);
    _syncWindowHeight();
  }

  void _closeClipboard() {
    if (!_clipboardOpen) {
      return;
    }
    setState(() => _clipboardOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开备忘录面板（「备忘录」结果项 / 「编辑备忘录 · <标识>」）。
  void _openMemo({String? memoId}) {
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _convertOpen = false;
    _ocrOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    _memoInitialId = memoId;
    setState(() => _memoOpen = true);
    _syncWindowHeight();
  }

  void _closeMemo() {
    if (!_memoOpen) {
      return;
    }
    setState(() {
      _memoOpen = false;
      _memoInitialId = null;
    });
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开快捷启动面板（「快捷启动」结果项；把当前附件带进去以便一键注册）。
  void _openQuickLaunch() {
    if (_quickOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _translateOpen = false;
    _convertOpen = false;
    _ocrOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    _memoInitialId = null;
    setState(() => _quickOpen = true);
    _syncWindowHeight();
  }

  void _closeQuickLaunch() {
    if (!_quickOpen) {
      return;
    }
    setState(() => _quickOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开翻译面板（「翻译」结果项；把搜索栏原文带进去预填）。
  void _openTranslate({String query = ''}) {
    if (_translateOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _convertOpen = false;
    _ocrOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    _translateQuery = query;
    setState(() => _translateOpen = true);
    _syncWindowHeight();
  }

  void _closeTranslate() {
    if (!_translateOpen) {
      return;
    }
    setState(() {
      _translateOpen = false;
      _translateQuery = '';
    });
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开 OCR（「OCR 文字识别」结果项；把当前附件里的图片带进去）。
  ///
  /// 首选独立窗口（v1 是 420×560 的插件窗）；没有宿主时回退主窗内面板。
  void _openOcr() {
    unawaited(
      _launchPlugin('ocr', attachments: [..._attachments]).then((opened) {
        if (!opened && mounted) {
          _showOcrPanel();
        }
      }),
    );
  }

  void _showOcrPanel() {
    if (_ocrOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _convertOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    setState(() => _ocrOpen = true);
    _syncWindowHeight();
  }

  void _closeOcr() {
    if (!_ocrOpen) {
      return;
    }
    setState(() => _ocrOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开文件转换（「文件转换」结果项；把搜索栏附件带进去当源文件）。
  ///
  /// 首选独立窗口（v1 是 820×470 / min 780×430 的插件窗）；没有宿主时回退面板。
  void _openConvert() {
    unawaited(
      _launchPlugin('convert', attachments: [..._attachments]).then((opened) {
        if (!opened && mounted) {
          _showConvertPanel();
        }
      }),
    );
  }

  void _showConvertPanel() {
    if (_convertOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _ocrOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    setState(() => _convertOpen = true);
    _syncWindowHeight();
  }

  void _closeConvert() {
    if (!_convertOpen) {
      return;
    }
    setState(() => _convertOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开音乐（「音乐」结果项；本地曲库 / 歌词 / 出声全在 Rust，见 `rust/src/player.rs`）。
  ///
  /// 首选独立窗口（v1 是 1280×720 的独立插件窗）；没有宿主时回退主窗内面板。
  void _openMusic() {
    unawaited(
      _launchPlugin('music').then((opened) {
        if (!opened && mounted) {
          _showMusicPanel();
        }
      }),
    );
  }

  void _showMusicPanel() {
    if (_musicOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _ocrOpen = false;
    _convertOpen = false;
    _proxyOpen = false;
    _toolsOpen = false;
    setState(() => _musicOpen = true);
    _syncWindowHeight();
  }

  void _closeMusic() {
    if (!_musicOpen) {
      return;
    }
    setState(() => _musicOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开桌宠控制台（「桌宠」结果项；窗口 / 形象在 Rust 的 `lunac_pet_*`）。
  ///
  /// 2026-11-19 用户裁决：控制台与其余插件一致走**独立窗口** —— 主窗的搜索栏不允许
  /// 出现在插件界面上。没有窗口宿主（单测 / Linux）时才回退到主窗内嵌面板。
  ///
  /// 与 v1 `pet.ts:3569-3577` 对齐：**配了 Live2D 模型而桌宠窗没开时，打开控制台直接拉起**
  /// （模型装载时会自行应用已持久化的参数）。图片模式不自动拉起 —— v1 的判据就是
  /// `loadConfig().model.trim()`。
  void _openPet() {
    if (_petOpen || _petWindowOpen) {
      return;
    }
    _exitDetail(silent: true);
    unawaited(_launchPetConsole());
  }

  Future<void> _launchPetConsole() async {
    final result = await openPluginWindow('pet');
    if (!mounted) {
      return;
    }
    switch (result) {
      case PluginOpenResult.opened:
        setState(() => _petWindowOpen = true);
        unawaited(_syncMainTopmost());
        _maybeShowPetWindow();
        return;
      case PluginOpenResult.unavailable:
        // 测试环境 / 没有窗口宿主：退回主窗内嵌面板。
        setState(() => _petOpen = true);
        _syncWindowHeight();
        _maybeShowPetWindow();
        return;
      case PluginOpenResult.limitReached:
        _toast('最多同时打开两个插件窗口，先关掉一个再打开桌宠');
        return;
    }
  }

  /// 配了 Live2D 模型而形象窗没开时，顺手把它拉起来。
  void _maybeShowPetWindow() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) {
        return;
      }
      final config = readPetConfig();
      if (config.isLive2D && !readPetStatus().open) {
        showPetWindow();
      }
    });
  }

  void _closePet() {
    if (!_petOpen) {
      return;
    }
    setState(() => _petOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开代理（「代理」结果项；系统代理读写都在 Rust 侧）。
  ///
  /// 首选独立窗口（v1 是 1280×720 / min 960×600 的插件窗）；没有宿主时回退面板。
  void _openProxy() {
    unawaited(
      _launchPlugin('proxy').then((opened) {
        if (!opened && mounted) {
          _showProxyPanel();
        }
      }),
    );
  }

  void _showProxyPanel() {
    if (_proxyOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _ocrOpen = false;
    _convertOpen = false;
    _toolsOpen = false;
    _musicOpen = false;
    setState(() => _proxyOpen = true);
    _syncWindowHeight();
  }

  void _closeProxy() {
    if (!_proxyOpen) {
      return;
    }
    setState(() => _proxyOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  /// 打开工具编辑器面板（「工具编辑器」结果项；工具定义读写都在 Rust 侧）。
  void _openTools() {
    if (_toolsOpen) {
      return;
    }
    _exitDetail(silent: true);
    _settingsOpen = false;
    _chatOpen = false;
    _clipboardOpen = false;
    _memoOpen = false;
    _quickOpen = false;
    _translateOpen = false;
    _ocrOpen = false;
    _convertOpen = false;
    _proxyOpen = false;
    _musicOpen = false;
    setState(() => _toolsOpen = true);
    _syncWindowHeight();
  }

  void _closeTools() {
    if (!_toolsOpen) {
      return;
    }
    setState(() => _toolsOpen = false);
    _runSearch();
    _syncWindowHeight();
    _focusInputAfterUnlock();
  }

  void _toast(String message) {
    final messenger = ScaffoldMessenger.of(context);
    messenger
      ..hideCurrentSnackBar()
      ..showSnackBar(
        SnackBar(content: Text(message), duration: const Duration(seconds: 2)),
      );
  }

  /// 结果行右键菜单：打开 / 打开所在位置 / 复制路径。
  Future<void> _showRowMenu(int index, Offset position) async {
    if (index < 0 || index >= _results.length) {
      return;
    }
    await _showResultMenu(
      _results[index],
      position,
      onOpen: () {
        setState(() => _selected = index);
        _activateSelected();
      },
    );
  }

  /// 右键菜单本体（紧凑搜索与详细搜索共用）。
  Future<void> _showResultMenu(
    SearchResult result,
    Offset position, {
    required VoidCallback onOpen,
    List<PopupMenuEntry<String>> extraItems = const [],
    void Function(String action)? onExtra,
  }) async {
    final path = _pathOf(result);
    final isWeb = result.kind == 'web';
    final currentEngine = isWeb ? webEngineName() : null;
    final overlay =
        Overlay.of(context).context.findRenderObject() as RenderBox?;
    if (overlay == null) {
      return;
    }
    final action = await showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(position.dx, position.dy, 1, 1),
        Offset.zero & overlay.size,
      ),
      items: [
        const PopupMenuItem<String>(value: 'open', child: Text('打开')),
        if (path != null)
          const PopupMenuItem<String>(value: 'reveal', child: Text('打开所在位置')),
        if (path != null)
          const PopupMenuItem<String>(value: 'copy', child: Text('复制路径')),
        // 「用 AI 询问」：把该结果标题作为提问预填进对话面板（附件一并带入）。
        const PopupMenuItem<String>(value: 'ask-ai', child: Text('用 AI 询问')),
        // 与 v1 一致：右键 Web 搜索行可以切换默认搜索引擎。
        if (isWeb)
          for (final entry in webEngines.entries)
            PopupMenuItem<String>(
              value: 'engine:${entry.key}',
              child: Text(
                entry.key == currentEngine
                    ? '${entry.value}（当前）'
                    : '切换为 ${entry.value}',
              ),
            ),
        // 详细搜索追加的项（例如系统动作的「以管理员身份运行」）。
        ...extraItems,
      ],
    );
    if (!mounted || action == null) {
      return;
    }
    switch (action) {
      case 'open':
        onOpen();
      case 'reveal':
        if (!revealInExplorer(path!)) {
          _toast('无法定位：$path');
        }
      case 'copy':
        await Clipboard.setData(ClipboardData(text: path!));
        _toast('已复制路径');
      case 'ask-ai':
        _openChat(query: result.title, attachments: [..._attachments]);
      default:
        if (action.startsWith('engine:')) {
          final name = action.substring('engine:'.length);
          if (setWebEngine(name)) {
            // 引擎变了，Web 行的标题 / 打开的地址都要重算。
            _runSearch();
            _toast('已切换为 ${webEngines[name] ?? name}');
          } else {
            _toast('切换搜索引擎失败');
          }
        } else if (onExtra != null) {
          onExtra(action);
        }
    }
  }

  /// `app:` / `file:` 结果里带的绝对路径；其它（内置命令 / Web 搜索）返回 null。
  static String? _pathOf(SearchResult result) {
    if (result.id.startsWith('app:')) {
      return result.id.substring(4);
    }
    if (result.id.startsWith('file:')) {
      return result.id.substring(5);
    }
    return null;
  }

  // ── 构建 ───────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    // 设置面板独占整窗（v1 同款：进设置后搜索栏整个消失），其余面板仍在搜索栏下方。
    if (_settingsOpen) {
      return Scaffold(
        backgroundColor: Colors.transparent,
        body: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: _pageInsetX,
            vertical: _pageInsetY,
          ),
          child: _buildSettingsPanel(),
        ),
      );
    }
    return Scaffold(
      // 窗口面透明：只画「猫 / 搜索栏 / 结果区」，不铺整窗底色。
      backgroundColor: Colors.transparent,
      body: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: _pageInsetX,
          vertical: _pageInsetY,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [_buildSearchRow(), _buildResultsSection()],
        ),
      ),
    );
  }

  /// 搜索栏这一行：首次启动时只有圆形按钮，搜索栏再从圆钮左端向左拉开。
  /// 之后唤醒窗口时 widget 不重建，所以动画不会再播。
  Widget _buildSearchRow() {
    return Listener(
      // 双击搜索栏 → 详细搜索大界面（v1 §2.1.2）。Listener 只观察不消费，
      // 输入框的双击选词照旧。
      onPointerDown: _onRowPointerDown,
      child: LayoutBuilder(
        builder: (context, constraints) {
          // 圆钮 right 6 + 直径 40：它的左缘就是搜索栏展开的起点。
          final revealOrigin =
              constraints.maxWidth - _settingsOrbSize - _orbRightInset;
          return SizedBox(
            height: _searchBarHeight,
            child: Stack(
              clipBehavior: Clip.none,
              children: [
                Positioned.fill(
                  child: AnimatedBuilder(
                    animation: _reveal,
                    builder: (context, _) {
                      return Stack(
                        children: [
                          Positioned.fill(
                            child: ClipRect(
                              clipper: _RevealClipper(
                                progress: _reveal.value,
                                origin: revealOrigin,
                              ),
                              child: _buildSearchBarBody(),
                            ),
                          ),
                          Positioned(
                            right: _orbRightInset,
                            top: (_searchBarHeight - _settingsOrbSize) / 2,
                            child: _buildSettingsOrb(),
                          ),
                        ],
                      );
                    },
                  ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }

  /// 搜索栏本体：既是输入区，也是无边框窗口的拖拽区。
  Widget _buildSearchBarBody() {
    // 下面是否接着一块内容（结果抽屉 / 640 面板 / 启动上限卡片）。
    // 没有接的时候搜索栏就是**一整块独立面板**，四个角都收圆 —— 否则简要搜索
    // 看起来像被切掉了底边（用户 2026-11-14 报障）。
    final attachedBelow =
        _pendingLaunch != null || _anyPanelOpen || _resultsExpanded;
    return Container(
      key: const ValueKey('search-bar-body'),
      decoration: BoxDecoration(
        color: LunacColors.surface,
        // 圆角与描边必须同色（Flutter 限制）；底边与结果面板的上沿直接接缝。
        //
        // 左上角：附件抽屉是另一个窗口、右缘为直角并贴在 x = 12 处，如果这里
        // 保留 12px 圆角就会在接缝顶部留下一个楔形透明缺口。抽屉在时改为直角，
        // 由抽屉自己的左端圆角收口，两块面板的上下边缘因此严丝合缝。
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.only(
          topLeft: Radius.circular(_attachments.isEmpty ? 12 : 0),
          topRight: const Radius.circular(12),
          bottomLeft: attachedBelow
              ? Radius.zero
              : Radius.circular(_attachments.isEmpty ? 12 : 0),
          bottomRight: attachedBelow ? Radius.zero : const Radius.circular(12),
        ),
      ),
      child: Stack(
        children: [
          Positioned.fill(
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onPanStart: (_) => _startDrag(),
            ),
          ),
          Positioned.fill(
            child: Row(
              children: [
                const SizedBox(width: 14),
                Icon(Icons.search, size: 16, color: LunacColors.textMuted),
                const SizedBox(width: 8),
                Expanded(
                  child: TextField(
                    controller: _controller,
                    focusNode: _inputFocus,
                    autofocus: widget.focusOnMount,
                    // 面板打开时的「锁定态」（v1 `plugin-locked`）：只读且不接受焦点，
                    // 点搜索栏不会收起面板、也不会改变结果列表。
                    readOnly: _searchBarLocked,
                    canRequestFocus: !_searchBarLocked,
                    cursorColor: LunacColors.accent,
                    style: TextStyle(color: LunacColors.text, fontSize: 15),
                    decoration: InputDecoration(
                      isDense: true,
                      border: InputBorder.none,
                      hintText: '搜索应用、文件与命令',
                      hintStyle: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 15,
                      ),
                    ),
                    onChanged: _onQueryChanged,
                    // 桌面端 TextField 默认在「点到别处」时调用 unfocus；点结果行
                    // 会先收起结果面板，点击就落空了（用户 2026-11-09 反馈）。
                    // 这里保持焦点：结果面板常驻，点击 / 回车都能正常执行。
                    // 窗口真正失焦（切到别的应用）由平台的自动隐藏负责。
                    onTapOutside: (_) {},
                    // Enter 统一在 _onKey 里处理（FocusNode 级），不走 onSubmitted，
                    // 否则真实设备上一次回车可能执行两遍动作。
                  ),
                ),
                if (_controller.text.isNotEmpty)
                  _WindowButton(
                    tooltip: '清空',
                    icon: Icons.backspace_outlined,
                    onTap: _clearQuery,
                  ),
                // 给右端圆形设置按钮留位（按钮在 Stack 顶层绘制，负责命中）。
                const SizedBox(width: _settingsOrbSize + 10),
              ],
            ),
          ),
        ],
      ),
    );
  }

  /// 圆形设置按钮：默认放一张图（用户给的 jpg，256×256 居中裁切），
  /// 后续可替换成自定义图片；外圈保留 1px 描边。
  Widget _buildSettingsOrb() {
    return Tooltip(
      message: '设置',
      child: SizedBox(
        width: _settingsOrbSize,
        height: _settingsOrbSize,
        child: Material(
          color: Colors.transparent,
          shape: const CircleBorder(),
          child: InkWell(
            customBorder: const CircleBorder(),
            onTap: _openSettings,
            child: Container(
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                image: const DecorationImage(
                  image: AssetImage('assets/orb_default.jpg'),
                  fit: BoxFit.cover,
                ),
                border: Border.all(color: LunacColors.divider),
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// 结果区：对话 / 设置 / 详细搜索打开时换成固定高度的大面板；否则用抽屉动画
  /// （用户 2026-11-08 裁决）。
  Widget _buildResultsSection() {
    if (_pendingLaunch != null) {
      return _buildPluginConfirmPanel();
    }
    if (_chatOpen) {
      return _buildChatPanel();
    }
    if (_clipboardOpen) {
      return _buildClipboardPanel();
    }
    if (_quickOpen) {
      return _buildQuickLaunchPanel();
    }
    if (_memoOpen) {
      return _buildMemoPanel();
    }
    if (_translateOpen) {
      return _buildTranslatePanel();
    }
    if (_convertOpen) {
      return _buildConvertPanel();
    }
    if (_ocrOpen) {
      return _buildOcrPanel();
    }
    if (_musicOpen) {
      return _buildMusicPanel();
    }
    if (_petOpen) {
      return _buildPetPanel();
    }
    if (_proxyOpen) {
      return _buildProxyPanel();
    }
    if (_toolsOpen) {
      return _buildToolsPanel();
    }
    if (_detailOpen) {
      return _buildDetailPanel();
    }
    return AnimatedSize(
      duration: const Duration(milliseconds: 130),
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: _resultsExpanded
          ? SizedBox(
              height: _resultsBorderExtent + _rowExtent * _visibleRows,
              child: Container(
                decoration: BoxDecoration(
                  color: LunacColors.surface,
                  // 圆角与描边必须同色（Flutter 限制）。
                  border: Border.all(color: LunacColors.border),
                  // 左下角：抽屉文件多到顶住面板底边时改为直角，否则接缝处
                  // 会露出楔形缺口（与左上角同一口径，见 `_buildSearchBarBody`）。
                  borderRadius: BorderRadius.only(
                    bottomLeft: Radius.circular(
                      _drawerCoversPanelBottom ? 0 : 12,
                    ),
                    bottomRight: const Radius.circular(12),
                  ),
                ),
                // 上沿 1px 接缝盖在描边上：搜索栏与结果区的分界线。
                foregroundDecoration: BoxDecoration(
                  border: Border(top: BorderSide(color: LunacColors.divider)),
                ),
                clipBehavior: Clip.antiAlias,
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    // 自定义背景图严格铺在结果区内部（跟随面板圆角 / 裁剪），
                    // 不再作为整窗最底层的一层。
                    Positioned.fill(child: appearancePanelBackdrop()),
                    _buildResults(),
                  ],
                ),
              ),
            )
          : const SizedBox(height: 0),
    );
  }

  Widget _buildResults() {
    if (_results.isEmpty) {
      final reason = _backend.unavailableReason;
      return Center(
        child: Text(
          reason ?? '没有匹配的结果',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    return ListView.builder(
      controller: _scroll,
      itemExtent: _rowExtent,
      itemCount: _results.length,
      itemBuilder: (context, index) => _ResultTile(
        result: _results[index],
        selected: index == _selected,
        showSubtitle: index == _hoveredIndex,
        onHoverChange: (hovering) {
          // 与 v1 一致：鼠标 hover 只显示副标题 / 视觉高亮，**不改变键盘选中行**
          // （否则移开鼠标后回车会命中鼠标停留过的那一行，和键盘操作打架）。
          setState(() {
            if (hovering) {
              _hoveredIndex = index;
            } else if (_hoveredIndex == index) {
              _hoveredIndex = null;
            }
          });
        },
        onTap: () {
          setState(() => _selected = index);
          _activateSelected();
        },
        onSecondaryTap: (position) => _showRowMenu(index, position),
      ),
    );
  }

  /// 插件窗口已达上限：让用户选「关掉最早那个并继续」还是「取消」。
  ///
  /// 画在结果区的位置（不是系统模态框）：主窗高度只有几百像素，系统对话框会被裁掉。
  Widget _buildPluginConfirmPanel() {
    final pending = _pendingLaunch;
    if (pending == null) {
      return const SizedBox.shrink();
    }
    final oldestTitle = pending.oldestId.isEmpty
        ? '最早打开的插件窗口'
        : '「${_pluginTitle(pending.oldestId)}」';
    final nextTitle = '「${_pluginTitle(pending.id)}」';
    return SizedBox(
      height: _confirmPanelHeight,
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
        padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.window_outlined,
                  size: 16,
                  color: LunacColors.textDim,
                ),
                const SizedBox(width: 8),
                Text(
                  '最多同时打开 2 个插件窗口',
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              '要关闭最早打开的$oldestTitle并继续打开$nextTitle吗？',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
            ),
            const Spacer(),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton(
                  onPressed: _cancelPendingLaunch,
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.textDim,
                    visualDensity: VisualDensity.compact,
                  ),
                  child: const Text('取消'),
                ),
                const SizedBox(width: 8),
                FilledButton(
                  onPressed: () => unawaited(_confirmPendingLaunch()),
                  style: FilledButton.styleFrom(
                    backgroundColor: LunacColors.accent,
                    foregroundColor: LunacColors.background,
                    visualDensity: VisualDensity.compact,
                  ),
                  child: Text('关闭$oldestTitle并继续'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  // ── AI 对话面板的界面 ──────────────────────────────────────────
  /// 对话面板：与详细搜索同高（640 档），协议与折叠都在面板内部。
  Widget _buildChatPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: ChatPanel(
        onClose: _closeChat,
        initialAttachments: _chatAttachments,
        initialQuery: _chatQuery,
      ),
    );
  }

  // ── 设置面板的界面 ──────────────────────────────────────────────

  /// 设置面板：独占整窗（进设置后搜索栏消失），内容与读写都在 [SettingsPanel] 里。
  Widget _buildSettingsPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2;
    return SizedBox(
      height: height,
      child: SettingsPanel(
        onClose: _closeSettings,
        // 引擎切换后 Web 搜索行的标题 / 地址要重算。
        onEngineChanged: _runSearch,
        // 设置面板独占整窗，标题栏拖动也要能移动窗口（2026-11-20 用户报障）。
        onDrag: _startDrag,
      ),
    );
  }

  /// 剪贴板历史面板：与设置 / 对话面板同高（640 档）。
  Widget _buildClipboardPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: ClipboardPanel(onClose: _closeClipboard),
    );
  }

  /// 备忘录面板：与设置 / 对话面板同高（640 档）。
  Widget _buildMemoPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: MemoPanel(onClose: _closeMemo, initialMemoId: _memoInitialId),
    );
  }

  /// 翻译面板：与设置 / 对话 / 备忘录面板同高（640 档）。
  Widget _buildTranslatePanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: TranslatePanel(
        onClose: _closeTranslate,
        initialQuery: _translateQuery,
      ),
    );
  }

  /// 文件转换面板：与设置 / 对话 / 翻译面板同高（640 档）。
  Widget _buildConvertPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: ConvertPanel(onClose: _closeConvert, attachments: _attachments),
    );
  }

  /// OCR 面板：与设置 / 对话 / 翻译面板同高（640 档）。
  Widget _buildOcrPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: OcrPanel(onClose: _closeOcr, attachments: _attachments),
    );
  }

  /// 音乐面板：与设置 / 对话 / 翻译面板同高（640 档）。
  Widget _buildMusicPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: MusicPanel(onClose: _closeMusic),
    );
  }

  /// 桌宠控制台：与设置 / 对话 / 翻译面板同高（640 档）。
  Widget _buildPetPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: PetPanel(onClose: _closePet),
    );
  }

  /// 代理面板：与设置 / 对话 / 翻译面板同高（640 档）。
  Widget _buildProxyPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: ProxyPanel(onClose: _closeProxy),
    );
  }

  /// 工具编辑器面板：与设置 / 对话 / 翻译面板同高（640 档）。
  Widget _buildToolsPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: ToolsPanel(onClose: _closeTools),
    );
  }

  /// 快捷启动面板：与设置 / 对话 / 剪贴板面板同高（640 档）。
  Widget _buildQuickLaunchPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: QuickLaunchPanel(
        onClose: _closeQuickLaunch,
        attachments: _attachments,
      ),
    );
  }

  // ── 详细搜索大界面的界面 ────────────────────────────────────────

  Widget _buildDetailPanel() {
    final height = _detailWindowHeight - _pageInsetY * 2 - _searchBarHeight;
    return SizedBox(
      height: height,
      child: Container(
        decoration: BoxDecoration(
          color: LunacColors.surface,
          border: Border.all(color: LunacColors.border),
          borderRadius: const BorderRadius.vertical(
            bottom: Radius.circular(12),
          ),
        ),
        // 上沿 1px 接缝盖在描边上：与搜索栏的分界线（与紧凑结果区同口径）。
        foregroundDecoration: BoxDecoration(
          border: Border(top: BorderSide(color: LunacColors.divider)),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            _buildDetailHeader(),
            if (_detailTab == _DetailTab.files) _buildDetailFileKinds(),
            Expanded(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(child: _buildDetailResults()),
                  _buildDetailPreview(),
                ],
              ),
            ),
            _buildDetailFooter(),
          ],
        ),
      ),
    );
  }

  Widget _buildDetailHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 6),
      child: Row(
        children: [
          _WindowButton(
            tooltip: '返回简洁搜索（Esc）',
            icon: Icons.arrow_back,
            onTap: () => _exitDetail(),
          ),
          const SizedBox(width: 4),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  for (final tab in _DetailTab.values)
                    Padding(
                      padding: const EdgeInsets.only(right: 6),
                      child: _DetailChip(
                        label: tab.label,
                        selected: _detailTab == tab,
                        onTap: () => _selectDetailTab(tab),
                      ),
                    ),
                ],
              ),
            ),
          ),
          // 当前分类的条数（v1 大界面头部的计数）。
          Text(
            '${_detailRows.length} 项',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          const SizedBox(width: 6),
          _WindowButton(
            tooltip: '重建文件索引',
            icon: Icons.refresh,
            onTap: _rebuildIndex,
          ),
        ],
      ),
    );
  }

  /// 详细搜索底栏：左侧快捷键提示，右侧文件索引状态（v1 `#detail-foot` 同口径）。
  ///
  /// 索引「还没载入 / 正在扫」时只给转圈、不给文字（用户 2026-09-22 的明确要求），
  /// 就绪后才显示「已索引 N 项」。
  Widget _buildDetailFooter() {
    final status = _indexStatus;
    final Widget trailing;
    if (status == null) {
      // 桥接缺失（极少数：DLL 没跟上）：如实说，不假装在扫盘。
      trailing = Text(
        '文件索引不可用',
        style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
      );
    } else if (_indexRefreshing || status.scanning || status.notLoaded) {
      trailing = SizedBox(
        width: 12,
        height: 12,
        child: CircularProgressIndicator(
          strokeWidth: 1.5,
          color: LunacColors.accent,
        ),
      );
    } else {
      trailing = Text(
        status.truncated
            ? '已索引 ${status.count} 项（已截断）'
            : '已索引 ${status.count} 项',
        style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
      );
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 4, 12, 6),
      child: Row(
        children: [
          Expanded(
            child: Text(
              '↑↓ 选择 · Enter 执行 · Shift+Enter 提权 · Esc 返回',
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
          const SizedBox(width: 8),
          trailing,
        ],
      ),
    );
  }

  Widget _buildDetailFileKinds() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 0, 10, 6),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Row(
          children: [
            for (final (kind, label) in _fileKinds)
              Padding(
                padding: const EdgeInsets.only(right: 6),
                child: _DetailChip(
                  label: label,
                  selected: _detailFileKind == kind,
                  onTap: () => _selectDetailFileKind(kind),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildDetailResults() {
    if (_detailRows.isEmpty) {
      final reason = _backend.unavailableReason;
      return Center(
        child: Text(
          reason ?? '没有匹配的结果',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    return ListView.builder(
      controller: _detailScroll,
      itemCount: _detailRows.length,
      itemBuilder: (context, index) {
        final row = _detailRows[index];
        final tile = _ResultTile(
          result: row.result,
          selected: index == _detailSelected,
          showSubtitle: index == _detailHovered,
          danger: row.danger,
          onHoverChange: (hovering) {
            setState(() {
              if (hovering) {
                _detailHovered = index;
              } else if (_detailHovered == index) {
                _detailHovered = null;
              }
            });
          },
          onTap: () {
            setState(() => _detailSelected = index);
            _activateDetailSelected();
          },
          onSecondaryTap: (position) => _showDetailRowMenu(index, position),
        );
        final group = row.group;
        if (group == null) {
          return tile;
        }
        // 分组标题（v1 `detail-group-title`）：与结果行同属一个列表项。
        return Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            SizedBox(
              height: _detailHeaderExtent,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(12, 8, 12, 2),
                child: Text(
                  group,
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
            tile,
          ],
        );
      },
    );
  }

  // ── 详细搜索右侧预览（v1 `#detail-preview`）────────────────────
  //
  // 只看选中行的**只读元信息**：图片走 Flutter 自带的 `Image.file`（缩到 192px），
  // 其余用 Rust 抽的类型图标；目录 / 文件都能打开、定位、复制路径。
  // 目录类 / 设置 / 系统动作这类没有本地路径的行不占位（列表恢复整宽）。

  /// 预览区宽度。
  static const double _detailPreviewWidth = 232;

  String? get _detailSelectedPath {
    if (_detailSelected < 0 || _detailSelected >= _detailRows.length) {
      return null;
    }
    return _pathOf(_detailRows[_detailSelected].result);
  }

  Widget _buildDetailPreview() {
    final path = _detailSelectedPath;
    if (path == null) {
      return const SizedBox.shrink();
    }
    final info = readFilePreview(path);
    final icon = info != null && info.isImage ? null : appIcon(path);
    return Container(
      width: _detailPreviewWidth,
      decoration: BoxDecoration(
        border: Border(left: BorderSide(color: LunacColors.divider)),
      ),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Center(child: _buildPreviewThumb(info, icon)),
          const SizedBox(height: 8),
          Text(
            info?.name ?? fileNameOfPath(path),
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(color: LunacColors.text, fontSize: 13),
          ),
          const SizedBox(height: 6),
          if (info == null)
            Text(
              '文件不可用（已移动或删除）',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            )
          else ...[
            _previewRow('类型', info.kindLabel),
            if (!info.isDirectory)
              _previewRow('大小', formatBytes(info.sizeBytes)),
            _previewRow('修改时间', formatModified(info.modified)),
            _previewRow('路径', path),
          ],
          const Spacer(),
          Row(
            children: [
              _previewAction('打开', _activateDetailSelected),
              _previewAction('定位', () => revealInExplorer(path)),
              _previewAction('复制', () => _copyPath(path)),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildPreviewThumb(FilePreviewInfo? info, String? icon) {
    final fallback = _previewGlyph(icon);
    if (info == null || !info.isImage) {
      return SizedBox(width: 96, height: 96, child: Center(child: fallback));
    }
    return ClipRRect(
      borderRadius: BorderRadius.circular(8),
      child: Image.file(
        File(info.path),
        width: 96,
        height: 96,
        fit: BoxFit.cover,
        cacheWidth: 192,
        errorBuilder: (_, _, _) => fallback,
      ),
    );
  }

  Widget _previewGlyph(String? icon) {
    if (icon == null) {
      return Icon(
        Icons.insert_drive_file_outlined,
        size: 44,
        color: LunacColors.textMuted,
      );
    }
    final comma = icon.indexOf(',');
    try {
      return Image.memory(
        base64Decode(comma >= 0 ? icon.substring(comma + 1) : ''),
        width: 44,
        height: 44,
        fit: BoxFit.contain,
        gaplessPlayback: true,
        errorBuilder: (_, _, _) => Icon(
          Icons.insert_drive_file_outlined,
          size: 44,
          color: LunacColors.textMuted,
        ),
      );
    } catch (_) {
      return Icon(
        Icons.insert_drive_file_outlined,
        size: 44,
        color: LunacColors.textMuted,
      );
    }
  }

  Widget _previewRow(String key, String value) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 52,
            child: Text(
              key,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
          Expanded(
            child: Text(
              value,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: LunacColors.textDim,
                fontSize: 11,
                height: 1.35,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _previewAction(String label, VoidCallback onTap) {
    return Expanded(
      child: TextButton(
        onPressed: onTap,
        style: TextButton.styleFrom(
          foregroundColor: LunacColors.textDim,
          padding: EdgeInsets.zero,
          minimumSize: const Size(0, 26),
          visualDensity: VisualDensity.compact,
        ),
        child: Text(label, style: const TextStyle(fontSize: 11)),
      ),
    );
  }

  void _copyPath(String path) {
    Clipboard.setData(ClipboardData(text: path));
    _toast('已复制路径');
  }

  Future<void> _showDetailRowMenu(int index, Offset position) async {
    if (index < 0 || index >= _detailRows.length) {
      return;
    }
    setState(() => _detailSelected = index);
    final row = _detailRows[index];
    // 提权项判据与 Shift+Enter 同源：白名单动作看 `elevatable`，应用 / 文件都能提权。
    final canElevate =
        row.elevatable ||
        row.result.id.startsWith('app:') ||
        row.result.id.startsWith('file:');
    await _showResultMenu(
      row.result,
      position,
      onOpen: _activateDetailSelected,
      // 右键多一项「以管理员身份运行」（与 v1 的同源判据一致）。
      extraItems: [
        if (canElevate)
          const PopupMenuItem<String>(
            value: 'elevate',
            child: Text('以管理员身份运行'),
          ),
      ],
      onExtra: (action) {
        if (action != 'elevate') {
          return;
        }
        _runElevated(row.result);
      },
    );
  }
}

class _ResultTile extends StatefulWidget {
  const _ResultTile({
    required this.result,
    required this.selected,
    required this.showSubtitle,
    required this.onHoverChange,
    required this.onTap,
    required this.onSecondaryTap,
    this.danger = false,
  });

  final SearchResult result;
  final bool selected;

  /// 副标题默认隐藏，鼠标停在这一行时才以动画显示。
  final bool showSubtitle;
  final ValueChanged<bool> onHoverChange;
  final VoidCallback onTap;

  /// 右键（secondary tap）位置，用于弹出「打开 / 打开所在位置」菜单。
  final ValueChanged<Offset> onSecondaryTap;

  /// 危险动作（关机 / 重启）：标题标红，提示需要二次确认。
  final bool danger;

  @override
  State<_ResultTile> createState() => _ResultTileState();
}

class _ResultTileState extends State<_ResultTile> {
  /// 真实图标（data URL）；null = 还没取到 / 取不到，先显示类型图标。
  String? _icon;

  @override
  void initState() {
    super.initState();
    _loadIcon();
  }

  @override
  void didUpdateWidget(covariant _ResultTile oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.result.id != widget.result.id) {
      _icon = null;
      _loadIcon();
    }
  }

  /// 图标抽取是同步 FFI（内部有串行 GDI 闸）：放到下一次事件循环再调，别卡当前帧。
  /// 路径级缓存见 `platform/icons.dart`，同一应用只抽一次。
  void _loadIcon() {
    final path = _SearchPageState._pathOf(widget.result);
    // 应用与文件都取系统图标（文件那类是类型图标，如 PDF / 图片 / 压缩包）。
    if (path == null ||
        (widget.result.kind != 'app' && widget.result.kind != 'file')) {
      return;
    }
    Future<void>(() {
      final icon = appIcon(path);
      if (!mounted || icon == null) {
        return;
      }
      setState(() => _icon = icon);
    });
  }

  static IconData _iconFor(String kind) {
    return switch (kind) {
      'folder' => Icons.folder_outlined,
      'app' => Icons.apps,
      'file' => Icons.description_outlined,
      'web' => Icons.public,
      _ => Icons.terminal,
    };
  }

  Widget _buildLeading(bool highlight) {
    final icon = _icon;
    if (icon != null) {
      final comma = icon.indexOf(',');
      final base64Text = comma >= 0 ? icon.substring(comma + 1) : '';
      return ClipRRect(
        borderRadius: BorderRadius.circular(6),
        child: Image.memory(
          base64Decode(base64Text),
          width: 22,
          height: 22,
          fit: BoxFit.contain,
          gaplessPlayback: true,
          // 图标数据坏了也不能让整行渲染失败：退回类型图标。
          errorBuilder: (context, error, stack) =>
              Icon(_iconFor(widget.result.kind), size: 16),
        ),
      );
    }
    return Icon(
      _iconFor(widget.result.kind),
      size: 16,
      color: highlight ? LunacColors.accent : LunacColors.textDim,
    );
  }

  @override
  Widget build(BuildContext context) {
    final result = widget.result;
    // 视觉高亮 = 键盘选中 或 鼠标停留（v1 里 hover 是独立于 .selected 的样式）。
    final highlight = widget.selected || widget.showSubtitle;
    return MouseRegion(
      onEnter: (_) => widget.onHoverChange(true),
      onExit: (_) => widget.onHoverChange(false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        onSecondaryTapDown: (details) =>
            widget.onSecondaryTap(details.globalPosition),
        child: Container(
          color: highlight ? const Color(0x1FCFC8BB) : Colors.transparent,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Row(
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  color: LunacColors.surfaceHover,
                  borderRadius: BorderRadius.circular(8),
                ),
                child: Center(child: _buildLeading(highlight)),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: SizedBox(
                  height: _rowExtent,
                  child: Stack(
                    children: [
                      // 平时标题垂直居中；hover 时上移，把位置让给副标题。
                      AnimatedPositioned(
                        duration: const Duration(milliseconds: 150),
                        curve: Curves.easeOut,
                        left: 0,
                        right: 0,
                        top: widget.showSubtitle ? 8 : 18,
                        child: Text(
                          result.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            // 危险动作（关机 / 重启）标红，提醒需要二次确认。
                            color: widget.danger
                                ? const Color(0xFFE06C6C)
                                : LunacColors.text,
                            fontSize: 14.5,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ),
                      AnimatedPositioned(
                        duration: const Duration(milliseconds: 150),
                        curve: Curves.easeOut,
                        left: 0,
                        right: 0,
                        top: widget.showSubtitle ? 28 : 34,
                        child: AnimatedOpacity(
                          opacity: widget.showSubtitle ? 1 : 0,
                          duration: const Duration(milliseconds: 150),
                          curve: Curves.easeOut,
                          child: Text(
                            result.subtitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              color: LunacColors.textDim,
                              fontSize: 12,
                            ),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 首次启动的揭示裁切：从圆形按钮左缘（[origin]）起，向左铺满整条搜索栏。
class _RevealClipper extends CustomClipper<Rect> {
  _RevealClipper({required this.progress, required this.origin});

  final double progress;
  final double origin;

  @override
  Rect getClip(Size size) {
    final left = origin * (1 - progress);
    final right = origin + (size.width - origin) * progress;
    return Rect.fromLTRB(left, 0, right, size.height);
  }

  @override
  bool shouldReclip(_RevealClipper oldClipper) =>
      oldClipper.progress != progress || oldClipper.origin != origin;
}

/// 搜索框内的小图标按钮（清空），24×24，圆角 6。
class _WindowButton extends StatelessWidget {
  const _WindowButton({
    required this.tooltip,
    required this.icon,
    required this.onTap,
  });

  final String tooltip;
  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(6),
        child: SizedBox(
          width: 24,
          height: 24,
          child: Icon(icon, size: 14, color: LunacColors.textDim),
        ),
      ),
    );
  }
}

/// 详细搜索的分类 / 文件类型胶囊。
class _DetailChip extends StatelessWidget {
  const _DetailChip({
    required this.label,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(6),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
        decoration: BoxDecoration(
          color: selected ? LunacColors.surfaceHover : Colors.transparent,
          borderRadius: BorderRadius.circular(6),
          border: Border.all(
            color: selected ? LunacColors.accent : LunacColors.divider,
          ),
        ),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 12,
            color: selected ? LunacColors.accent : LunacColors.textDim,
          ),
        ),
      ),
    );
  }
}
