// M2 对话面板：用户输入 → agent（stream-json）→ 文本 / 思考 / 工具卡 / 审批卡。
//
// 边界：协议报文与事件折叠分别在 `platform/agent.dart` 与 `search/chat_model.dart`；
// 本文件只负责画与收集输入。自 M5v/M5w 起过程块的版式（思考折叠、工具卡、用户气泡、
// 消息动作条）逐项对齐 v1 `index.html` / `styles.css` / `main.ts` 的当前形态。
import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/agent.dart';
import '../platform/appearance.dart';
import '../platform/change_review.dart';
import '../platform/chat_history.dart';
import '../platform/clipboard_files.dart';
import '../platform/cron.dart';
import '../platform/drop.dart';
import '../platform/file_preview.dart';
import '../platform/memory.dart';
import '../platform/notify_sound.dart';
import '../platform/plans.dart';
import '../platform/sandbox.dart';
import '../platform/security_ledger.dart';
import '../platform/session_export.dart';
import '../platform/shell_actions.dart';
import '../platform/task_queue.dart';
import '../platform/updater.dart' show kAppVersion;
import '../platform/usage.dart';
import '../theme.dart';
import 'ansi.dart';
import 'attachment_tokens.dart';
import 'auto_approval.dart';
import 'chat_markdown.dart';
import 'chat_model.dart';
import 'usage_dashboard.dart';

/// 对话面板：与详细搜索 / 设置同高（640 固定档）。
class ChatPanel extends StatefulWidget {
  const ChatPanel({
    super.key,
    required this.onClose,
    this.initialAttachments = const [],
    this.initialQuery = '',
    this.title = 'AI 对话',
    this.windowControls,
  });

  /// 关闭面板（Esc / 返回按钮）；agent 进程保持常驻（与 v1 一致）。
  final VoidCallback onClose;

  /// 从主页面带进来的附件（文件路径）。
  final List<String> initialAttachments;

  /// 从主页面带进来的初始输入（例如「请分析这些文件」）。
  final String initialQuery;

  /// 头部标题：独立窗里用窗口标题（v1 `#detached-title`），内嵌时是「AI 对话」。
  final String title;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 窗口栏与面板头部**合并成一行**（v1 的聊天窗只有一个 `#detached-header`），
  /// 面板不再画「返回」；null ⇒ 内嵌模式，保留「返回 + 标题」。
  final Widget? windowControls;

  @override
  State<ChatPanel> createState() => _ChatPanelState();
}

class _ChatPanelState extends State<ChatPanel> {
  /// 会话级单例：关面板再打开时对话还在（agent 也常驻）。
  ChatModel get _model => chatSession;
  late final TextEditingController _input = AttachmentTextEditingController(
    text: widget.initialQuery,
  );
  late final List<String> _attachments = [...widget.initialAttachments];
  final ScrollController _scroll = ScrollController();
  late final FocusNode _inputFocus = FocusNode(onKeyEvent: _onInputKey);

  /// 面板级焦点：桥接缺失时面板里没有输入框，Esc 只能靠它兜底。
  late final FocusNode _panelFocus = FocusNode(onKeyEvent: _onKey);

  StreamSubscription<Map<String, dynamic>>? _subscription;
  String? _startError;

  /// 流式过程中的落盘节流（2026-11-21）：一轮可能跑几分钟，只靠 `result` 落盘
  /// 时，强制结束 / 闪退会把整个新会话丢掉。这里每 900ms 最多写一次快照；
  /// 用户提问在 `_send` 里先同步落一次，保证「问题本身」一定在盘上。
  Timer? _persistTimer;

  /// 历史列表是否打开（面板内切换，不新开窗口）。
  bool _historyOpen = false;
  List<ChatHistoryMeta> _historyMeta = const [];

  /// 生成中按发送的两种语义（§待办 3-28.1）：
  /// `false` = **排队**（默认，回合结束后按原语义执行）；
  /// `true` = **立即插入**（带 `steer:"insert"`，agent 在下一个轮首安全点注入本回合）。
  /// 「暂停」是另一颗键（`_stopTurn`），三态各走各的，不互相代替。
  bool _insertWhileBusy = false;

  /// 斜杠命令候选（当前输入处于「命令位置」时的过滤结果）。
  List<_SlashItem> _slashMatches = const [];
  int _slashIndex = 0;
  bool _slashHidden = false;

  /// 待二次确认的破坏性命令：`rewind` / `memory_clear`；null = 无。
  String? _pendingConfirm;

  /// 上一次告诉宿主调度器的「前端忙」值（只在与 `_model.busy` 不同步时才过 FFI）。
  bool _cronBusy = false;

  /// 待办抽屉是否展开（清单非空时常驻在输入框上方）。
  bool _todoOpen = false;

  /// 「更多设置」弹出层（v1 `#chat-more-menu`）是否打开。
  bool _moreOpen = false;

  /// 当前页签：`profile` / `workspace` / `tools` / `persona` / `security`。
  String _moreTab = 'profile';

  /// 对话设置快照（`config/chat.json` + `config/persona.md`）。
  ChatSettings _settings = const ChatSettings();

  /// 工具黑名单的**编辑态**（点「保存并重启」才写回）。
  final Set<String> _toolsOff = <String>{};

  /// 人格编辑区是否展开（v1 `#chat-persona-box`）。
  bool _personaEditing = false;
  final TextEditingController _personaText = TextEditingController();

  /// 弹出层底部的一行反馈（保存结果 / 重启提示）。
  String? _moreMessage;
  bool _moreMessageError = false;

  /// 「自动审批」需要二次确认（v1 `askRunModeAutoConfirm`）。
  bool _runModeConfirm = false;

  /// 「安全」页（白箱边界账本 W0）：最近判定 / 预演输入 / 一行反馈。
  List<SecurityEntry> _securityRows = const [];
  final TextEditingController _securityPreviewText = TextEditingController();
  String _securityPreviewResult = '';
  String? _securityNote;

  /// 上一次提示音的时间（审批卡 / 回合结束共用一个 600ms 去抖窗口）。
  DateTime? _lastNotifyAt;

  /// 任务抽屉当前页（v1 `#todo-drawer-tabs` 三页互斥）：`tasks` / `files` / `bg`。
  String _todoPage = 'tasks';

  /// 已扫到哪条 entry：只增量扫描新出现的写类工具。
  int _changeScanIndex = 0;

  /// 已经采过快照的 tool_use_id（同一张卡只采一次）。
  final Set<String> _snapshottedTools = <String>{};

  /// 命令类工具执行前抓过的工作区指纹（tool_use_id → 相对路径 → `大小:改动ms`）。
  ///
  /// §待办 3-28.13 ②：`Cmd` / `PowerShell` 里产生的文件无法静态判定，只能前后各抓
  /// 一次目录元数据做差。**有上限**（同时最多 4 条在飞），避免长批量命令把抓帧堆起来。
  final Map<String, Map<String, String>> _cmdFingerprints = {};

  /// 已采过「执行前指纹」的命令 id（防止同一个 tool_use 抓两次）。
  final Set<String> _cmdTrackedTools = <String>{};

  /// 由命令产出（而不是 Write / Edit）的文件：界面标「命令产出」，且**不提供还原**
  /// —— 抓帧只有元数据，没有前像，声称可还原就是假的。
  final Set<String> _cmdOutputs = <String>{};

  /// 改动文件的 diff 统计缓存（打开「改动文件」页时算一次）。
  final Map<String, ChangeDiff?> _changeStats = <String, ChangeDiff?>{};

  /// 历史会话恢复出来的、**已经没有回退快照**的文件（v1 的列表能重建，v2 不假装可还原）。
  final Set<String> _noSnapshot = <String>{};

  /// 上一次看到的「仍有快照」集合：预览窗里确认 / 撤销后据此刷新抽屉。
  Set<String> _pendingChanges = <String>{};

  /// 每秒比对一次快照状态（预览窗在另一个 engine，靠落盘标记跨窗口同步）。
  Timer? _changeTimer;

  /// 预览窗「添加到对话」队列的轮询（跨 engine，同一 `lunac_core.dll` 静态区）。
  /// 面板开着时由这里消费；主窗 `search_page` 只在对话窗未开时才取。
  Timer? _attachTimer;

  /// 定时任务的缓存（§待办 3-28.16：活动面板里要能看到「下次什么时候跑 / 上次结果」）。
  List<CronJob> _cronJobs = const [];
  Timer? _cronTimer;

  /// 正在「立即运行」的那条定时任务 id（防连点；同一时刻只放一条）。
  String? _cronRunning;

  /// 正在切换启用 / 停用的定时任务 id（与「运行中」分开，按钮文案不能混）。
  String? _cronToggling;

  /// 后台任务队列的缓存（§待办 3-28.16 ①：任务状态与取消入口）。
  List<TaskItem> _taskItems = const [];
  Timer? _taskTimer;

  /// 正在取消的那条排队任务 id（防连点）。
  String? _taskCancelling;

  /// 待二次确认的逐条回退（v1 消息动作条：点一次变「确认回退」，再点才执行）。
  ChatEntry? _armedRewind;

  @override
  void initState() {
    super.initState();
    _settings = readChatSettings();
    _toolsOff.addAll(_settings.toolBlacklist);
    _personaText.text = _settings.persona;
    _input.addListener(_onInputChanged);
    // §待办 3-28.18：外部带进来的附件同样以「胶囊」落到正文里（不再单独堆一行）。
    if (_attachments.isNotEmpty) {
      final tokens = _attachments.map(attachToken).join(' ');
      final text = _input.text;
      _input.text = text.trim().isEmpty ? '$tokens ' : '$text $tokens ';
      _input.selection = TextSelection.collapsed(offset: _input.text.length);
    }
    _start();
    // 预览窗在另一个 engine：它在预览里「全部应用 / 撤销」后，抽屉靠落盘标记
    // 每秒比对一次（没有事件通道，也不该为一个开关加跨窗协议）。
    _changeTimer = Timer.periodic(
      const Duration(seconds: 1),
      (_) => _refreshChangeState(),
    );
    // 预览窗在另一个 engine，它在文件树 / 标签上「添加到对话」后靠同一份
    // 静态队列转交；1 秒取一次，`take` 即排空。
    _attachTimer = Timer.periodic(
      const Duration(seconds: 1),
      (_) => _pollAttachments(),
    );
    // 定时任务只读两个小 JSON（定义 + 运行态），5 秒刷一次足够；桥缺失时返回空表。
    _refreshCronJobs();
    _cronTimer = Timer.periodic(
      const Duration(seconds: 5),
      (_) => _refreshCronJobs(),
    );
    // 后台任务队列：同一份 5 秒节奏（queue.json 也是小文件）。
    _refreshTaskQueue();
    _taskTimer = Timer.periodic(
      const Duration(seconds: 5),
      (_) => _refreshTaskQueue(),
    );
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        // 没有输入框（启动失败）时焦点落在面板上，Esc 才有人接。
        (_startError == null ? _inputFocus : _panelFocus).requestFocus();
      }
    });
    // 输入区边框随聚焦变色（只重画边框；不改任何输入语义）。
    _inputFocus.addListener(_onInputFocusChanged);
    // 系统文件拖放：对话面板打开时优先接住拖进来的文件（后注册优先，见 `drop.dart`）。
    addFileDropListener(_attachPaths);
  }

  void _onInputFocusChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  @override
  void dispose() {
    // 流式过程中的节流落盘（见 `_schedulePersist`）。
    _persistTimer?.cancel();
    _persistTimer = null;
    _persist();
    // 面板关掉就不再收事件：忙态必须放掉，否则调度器会一直以为前端在流式跑。
    if (_cronBusy) {
      setCronBusy(false);
    }
    _input.removeListener(_onInputChanged);
    _inputFocus.removeListener(_onInputFocusChanged);
    removeFileDropListener(_attachPaths);
    _subscription?.cancel();
    _changeTimer?.cancel();
    _attachTimer?.cancel();
    _cronTimer?.cancel();
    _taskTimer?.cancel();
    _input.dispose();
    _scroll.dispose();
    _inputFocus.dispose();
    _panelFocus.dispose();
    _personaText.dispose();
    _securityPreviewText.dispose();
    super.dispose();
  }

  void _start() {
    // 旧进程刚死 / 首次启动时，历史里可能还挂着上一轮没答过的审批卡：
    // 那些 request_id 在新进程里不存在，点了只会「看起来没反应」。
    final wasRunning = agentRunning;
    final reason = startAgent();
    if (!mounted) {
      return;
    }
    if (reason != null) {
      setState(() => _startError = reason);
      return;
    }
    if (!wasRunning) {
      final stale = _model.invalidateApprovals('agent 已重启，这张卡已失效');
      if (stale > 0) {
        _model.addNotice('上一轮有 $stale 张审批卡已失效（agent 已重启）。');
      }
    }
    // 面板关闭期间 agent 仍可能在跑：先把积压事件折叠进模型，再接实时流。
    drainAgentEvents(_consumeEvent);
    if (_model.busy) {
      // 这些事件可能是崩溃前最后一段输出，别等下一轮 result 才落盘。
      _schedulePersist();
    }
    _listen();
    setState(() => _startError = null);
  }

  void _listen() {
    _subscription?.cancel();
    _subscription = agentEvents.listen(_onEvent);
  }

  /// 折叠一条事件 + 记账。**事件只从 Rust 队列消费一次**（drain 或实时二选一），
  /// 所以在这里追加用量日志不会重复。
  void _consumeEvent(Map<String, dynamic> event) {
    _model.apply(event);
    // 被丢弃的「立即插入」：正文还回输入框（气泡已由模型撤回，用户直接回车重发）。
    final retracted = _model.takeDroppedSteer();
    if (retracted != null) {
      _input.value = TextEditingValue(
        text: retracted,
        selection: TextSelection.collapsed(offset: retracted.length),
      );
    }
    // 写类工具的 tool_use 在**工具执行之前**到达 ⇒ 此刻读到的就是「改动前」原文。
    _captureChanges();
    _maybeAutoApprove(event);
    _maybeNotify(event);
    _recordUsage(event);
    _deliverPreviewRequests();
  }

  /// `runMode == auto` / `allowlist` 时直接替用户回「允许」，不弹卡（v1 同语义）。
  void _maybeAutoApprove(Map<String, dynamic> event) {
    if (event['type'] != 'control_request') {
      return;
    }
    final request = event['request'];
    if (request is! Map || request['subtype'] != 'can_use_tool') {
      return;
    }
    final requestId = event['request_id'];
    if (requestId is! String || requestId.isEmpty) {
      return;
    }
    final toolName = request['tool_name'] is String
        ? request['tool_name'] as String
        : '';
    final rawAnalysis = request['analysis'];
    final analysis = rawAnalysis is Map
        ? Map<String, dynamic>.from(rawAnalysis)
        : null;
    final rawInput = request['input'];
    final input = rawInput is Map ? Map<String, dynamic>.from(rawInput) : null;
    // 白箱账本（W0）：先用同一条判定流水线定下「为什么」，自动放行与弹卡记同一种原因码。
    final why = describeBoundary(
      toolName: toolName,
      analysis: analysis,
      runMode: _settings.runMode,
    );
    if (!shouldAutoApprove(
      toolName: toolName,
      analysis: analysis,
      runMode: _settings.runMode,
    )) {
      // 弹卡前先记一条 ask；用户答复会再记一条 USER_*，两条构成完整证据链（v1 同语义）。
      _logBoundary(toolName, input, SecurityDecision.ask, why);
      return;
    }
    final toolUseId = request['tool_use_id'] is String
        ? request['tool_use_id'] as String
        : '';
    final sent = sendAgentLine(
      jsonEncode(approvalResponse(requestId, toolUseId, allow: true)),
    );
    if (!sent) {
      // 送不出去就保留卡片，让用户手动处理（不能假装已放行）。
      return;
    }
    _logBoundary(toolName, input, SecurityDecision.auto, why);
    for (final entry in _model.entries) {
      if (entry.kind == ChatEntryKind.approval &&
          entry.requestId == requestId &&
          !entry.handled) {
        entry.handled = true;
        entry.detail = '${entry.detail ?? ''}\n已自动放行（${_settings.runModeLabel}）'
            .trim();
      }
    }
  }

  /// 往白箱账本落一行（旁路；写盘失败静默）。
  ///
  /// `scope` 与 v1 同口径：有工作区就记工作区，否则 `system`。
  void _logBoundary(
    String toolName,
    Map<String, dynamic>? input,
    String decision,
    BoundaryWhy why,
  ) {
    if (toolName.trim().isEmpty) {
      return; // 没有工具名的一行在 Rust 侧会被拒；不要制造噪声
    }
    logSecurityDecision(
      tool: toolName,
      input: input,
      decision: decision,
      why: why,
      session: _model.sessionId ?? '',
      scope: _settings.workspace.isEmpty ? 'system' : _settings.workspace,
    );
  }

  /// 审批卡真的出现在界面上 / 一轮结束时响一声（与 v1 `notify_sound` 同一个音）。
  ///
  /// 自动放行不响（没有卡）；一批多条命令也只响一次（600ms 去抖）。
  void _maybeNotify(Map<String, dynamic> event) {
    final type = event['type'];
    if (type == 'control_request') {
      final hasPending = _model.entries.any(
        (entry) => entry.kind == ChatEntryKind.approval && !entry.handled,
      );
      if (!hasPending) {
        return;
      }
    } else if (type != 'result') {
      return;
    }
    final now = DateTime.now();
    final last = _lastNotifyAt;
    if (last != null &&
        now.difference(last) < const Duration(milliseconds: 600)) {
      return;
    }
    _lastNotifyAt = now;
    playNotifySound();
  }

  /// agent 的 `Preview` 工具事件 → 预览窗标签（跨 FlutterEngine 走 Rust 进程内队列）。
  ///
  /// 入队成功后把预览窗打开（已在则提到前台）；入队失败就静默放弃 —— 这条是纯展示，
  /// 不该因为预览窗不可用而在对话流里制造错误。
  void _deliverPreviewRequests() {
    if (_model.previewRequests.isEmpty) {
      return;
    }
    final requests = List<Map<String, dynamic>>.of(_model.previewRequests);
    _model.previewRequests.clear();
    var pushed = false;
    for (final request in requests) {
      if (pushPreviewTab(request) == null) {
        pushed = true;
      }
    }
    if (pushed) {
      unawaited(openSandboxWindow());
    }
  }

  // ── 改动文件（会话内快照 / 审阅）────────────────────────────────

  /// 增量扫描 tool_use：写类工具立刻采一份「改动前」快照（工具还没执行）。
  ///
  /// 快照按**回合**分档（`turn` = 采集时已完成的助手消息条数）：回退到某一轮时，
  /// 只还原那一轮及之后改过的文件（v1 `revertFileSnapshots` 同口径）。
  void _captureChanges() {
    if (_changeScanIndex > _model.entries.length) {
      // 回退 / 恢复历史会把条目截短，游标要回到能扫到的位置（重复由 id 集合挡住）。
      _changeScanIndex = 0;
    }
    final workdir = readSandboxStatus()?.path ?? '';
    final turn = _model.messages.where((m) => m.role == 'assistant').length;
    var added = false;
    while (_changeScanIndex < _model.entries.length) {
      final entry = _model.entries[_changeScanIndex++];
      // 命令类工具收口：用「执行前指纹」做差（§待办 3-28.13 ②）。
      if (entry.kind == ChatEntryKind.toolResult) {
        _finalizeCommandOutput(entry);
        continue;
      }
      if (entry.kind != ChatEntryKind.toolUse) {
        continue;
      }
      final id = entry.toolUseId;
      // 命令类：执行前抓一次工作区指纹。**抓帧被预算截断就不做** —— 半份指纹
      // 比出来的差会漏报，宁可如实不列。
      if (id != null &&
          id.isNotEmpty &&
          (entry.toolName == 'Cmd' || entry.toolName == 'PowerShell') &&
          !_cmdTrackedTools.contains(id) &&
          _cmdFingerprints.length < 4) {
        _cmdTrackedTools.add(id);
        final fingerprint = readWorkspaceFingerprint();
        if (fingerprint.ok && !fingerprint.truncated) {
          _cmdFingerprints[id] = fingerprint.entries;
        }
      }
      final field = writeToolPathField[entry.toolName];
      if (field == null) {
        continue;
      }
      if (id == null || id.isEmpty || !_snapshottedTools.add(id)) {
        continue;
      }
      final path = toolArgPath(entry.detail, field);
      if (path == null) {
        continue;
      }
      final absolute = _absoluteAgentPath(path, workdir);
      if (snapshotChange(absolute, turn: turn, session: _sessionKey)) {
        _model.noteChangedFile(absolute);
        _pendingChanges.add(absolute);
        _changeStats.remove(absolute);
        added = true;
      }
    }
    if (added && _todoOpen && _todoPage == 'files') {
      _computeChangeStats();
    }
  }

  /// 命令类工具收口：再抓一次指纹，把新增 / 改动的文件并进「改动文件」。
  ///
  /// **只列出来、不承诺还原**（没有前像）：这些路径进 `_cmdOutputs`，行上标「命令产出」。
  void _finalizeCommandOutput(ChatEntry entry) {
    final id = entry.toolUseId;
    if (id == null || id.isEmpty) {
      return;
    }
    final before = _cmdFingerprints.remove(id);
    if (before == null) {
      return;
    }
    final after = readWorkspaceFingerprint();
    if (!after.ok || after.truncated) {
      return;
    }
    final changed = diffWorkspaceFingerprints(before, after.entries);
    if (changed.isEmpty) {
      return;
    }
    final workspace = _settings.workspace;
    var added = false;
    for (final relative in changed) {
      final absolute = _absoluteAgentPath(relative, workspace);
      if (_cmdOutputs.add(absolute)) {
        _model.noteChangedFile(absolute);
        added = true;
      }
    }
    if (added && _todoOpen && _todoPage == 'files') {
      _computeChangeStats();
    }
  }

  /// 相对路径按 agent 的 cwd（沙箱盘）解析成绝对路径；盘符 / UNC 原样返回。
  String _absoluteAgentPath(String path, String workdir) {
    final normalized = path.replaceAll('/', r'\');
    final absolute =
        RegExp(r'^[A-Za-z]:\\').hasMatch(normalized) ||
        normalized.startsWith(r'\\');
    if (absolute || workdir.isEmpty) {
      return path;
    }
    return '$workdir\\$normalized';
  }

  /// 打开「改动文件」页时算一次增删行数（读盘 + LCS，别放在每帧里）。
  ///
  /// 读不到 diff（没有快照 / 无桥接）不是错误：历史会话的文件照样列出来，
  /// 只是行上标「已无快照」，不假装还能还原（与「命令产出」同一条口径）。
  void _computeChangeStats() {
    for (final path in _model.changedFiles) {
      final diff = readChangeDiff(path, session: _sessionKey);
      if (diff == null) {
        _changeStats.remove(path);
        _noSnapshot.add(path);
      } else {
        _noSnapshot.remove(path);
        _changeStats[path] = diff;
      }
    }
  }

  /// 预览窗里「全部应用 / 撤销」后，抽屉靠落盘标记每秒比对一次。
  void _refreshChangeState() {
    if (!mounted || _model.changedFiles.isEmpty) {
      return;
    }
    final pending = <String>{
      for (final path in _model.changedFiles)
        if (isChangePending(path, session: _sessionKey)) path,
    };
    final same =
        pending.length == _pendingChanges.length &&
        pending.every(_pendingChanges.contains);
    if (same) {
      return;
    }
    setState(() {
      _pendingChanges = pending;
      _changeStats.clear();
    });
    if (_todoOpen && _todoPage == 'files') {
      _computeChangeStats();
    }
  }

  /// 预览窗「添加到对话」：同一个 `lunac_core.dll` 静态队列，`take` 即排空。
  ///
  /// 与主窗 `search_page` 的轮询共用一个队列（那边只在对话窗未开时才取），
  /// 所以不会两边都加；已存在的路径直接跳过，避免重复气泡。
  void _pollAttachments() {
    if (!mounted) {
      return;
    }
    _attachPaths(takeChatAttachments());
  }

  /// 把一批路径加进附件并插到光标处（拖放 / 粘贴文件 / 预览窗「添加到对话」共用）。
  void _attachPaths(List<String> paths) {
    if (!mounted || paths.isEmpty) {
      return;
    }
    final existing = {for (final path in _attachments) path.toLowerCase()};
    final added = <String>[
      for (final path in paths)
        if (path.isNotEmpty && existing.add(path.toLowerCase())) path,
    ];
    if (added.isEmpty) {
      return;
    }
    setState(() {
      _attachments.addAll(added);
      _insertAttachmentTokens(added);
    });
    _scrollToEnd(force: true);
  }

  /// Ctrl+V：先看剪贴板里有没有文件（`CF_HDROP`），没有就按普通文本粘贴。
  ///
  /// 为什么自己粘文本：按键处理器里拿剪贴板是异步的，不能「先放行、发现是文件再撤」，
  /// 所以统一接管 —— 有文件插胶囊，没文件就把文本插到光标处（行为与默认粘贴一致）。
  Future<void> _pasteIntoInput() async {
    final files = await readClipboardFiles();
    if (!mounted) {
      return;
    }
    if (files.isNotEmpty) {
      _attachPaths(files);
      return;
    }
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (!mounted || text == null || text.isEmpty) {
      return;
    }
    final selection = _input.selection;
    final base = selection.isValid
        ? selection
        : TextSelection.collapsed(offset: _input.text.length);
    final next = _input.text.replaceRange(base.start, base.end, text);
    _input.value = TextEditingValue(
      text: next,
      selection: TextSelection.collapsed(offset: base.start + text.length),
    );
  }

  /// 把附件占位符插到光标处（§待办 3-28.18）：位置即用户想要的锚点。
  void _insertAttachmentTokens(Iterable<String> paths) {
    final tokens = [for (final path in paths) attachToken(path)];
    if (tokens.isEmpty) {
      return;
    }
    final selection = _input.selection;
    final base = selection.isValid
        ? selection
        : TextSelection.collapsed(offset: _input.text.length);
    final insert = ' ${tokens.join(' ')} ';
    final next = _input.text.replaceRange(base.start, base.end, insert);
    _input.value = TextEditingValue(
      text: next,
      selection: TextSelection.collapsed(offset: base.start + insert.length),
    );
  }

  // ── 定时任务（§待办 3-28.16：让 cron 进活动面板）────────────────────
  //
  // 「查看 / 下次运行时刻 / 立即运行」都在前端；**创建与删除仍归 agent**（`CronCreate` /
  // `CronDelete` 写 `ModuleData/tasks/cron.json`），前端不直接改定义文件。

  /// 刷新定时任务缓存（5 秒一次；内容没变就不 setState）。
  void _refreshCronJobs() {
    final jobs = readCronJobs().jobs;
    if (!mounted) {
      return;
    }
    if (_cronSignature(_cronJobs) == _cronSignature(jobs)) {
      return;
    }
    setState(() => _cronJobs = jobs);
  }

  /// 只比较界面上会变的东西：增删 / 启停 / 下次时间 / 上次结果。
  String _cronSignature(List<CronJob> jobs) => jobs
      .map(
        (job) =>
            '${job.id}|${job.enabled}|${job.autoDisabled}|'
            '${job.nextRunAtLocal}|${job.lastStatus}|${job.lastError}|${job.runCount}',
      )
      .join('\n');

  /// 立即跑一次（阻塞到跑完 ⇒ 桥接里已经放进 isolate）。
  ///
  /// 回合进行中禁用：定时任务是「另一个 agent 进程」，与当前回合并发跑会白白多烧一份
  /// token，也容易和文件快照互相踩。
  Future<void> _runCronNow(CronJob job) async {
    if (_cronRunning != null || _model.busy) {
      return;
    }
    setState(() => _cronRunning = job.id);
    final result = await runCronJobNow(job.id);
    if (!mounted) {
      return;
    }
    setState(() => _cronRunning = null);
    if (result.ok) {
      _model.addNotice('已运行定时任务：${_oneLine(job.prompt)}');
    } else {
      _model.addError('定时任务运行失败：${result.error}');
    }
    _refreshCronJobs();
  }

  /// 启用 / 停用一条定时任务（与设置页同一个 ABI；重新启用会清掉自动停用）。
  Future<void> _toggleCronEnabled(CronJob job) async {
    if (_cronToggling != null) {
      return;
    }
    final enabled = !job.enabled;
    setState(() => _cronToggling = job.id);
    final result = await setCronEnabled(job.id, enabled);
    if (!mounted) {
      return;
    }
    setState(() => _cronToggling = null);
    if (result.ok) {
      _model.addNotice(
        enabled
            ? '已启用定时任务：${_oneLine(job.prompt)}'
            : '已停用定时任务：${_oneLine(job.prompt)}',
      );
    } else {
      _model.addError('定时任务启停失败：${result.error}');
    }
    _refreshCronJobs();
  }

  // ── 后台任务队列（§待办 3-28.16 ①：让队列进活动面板）──────────────────
  //
  // 入队仍归 agent（`TaskEnqueue` 写 `ModuleData/tasks/queue.json`）；前端只做
  // 「看状态 / 取消 pending」，**不直接改队列文件**（与 cron 同一条分工）。

  /// 刷新任务队列缓存（5 秒一次；内容没变就不 setState）。
  void _refreshTaskQueue() {
    final items = readTaskQueue().items;
    if (!mounted) {
      return;
    }
    if (_taskSignature(_taskItems) == _taskSignature(items)) {
      return;
    }
    setState(() => _taskItems = items);
  }

  /// 只比较界面上会变的东西：状态 / 轮次 / 摘要 / 错误。
  String _taskSignature(List<TaskItem> items) => items
      .map(
        (item) =>
            '${item.id}|${item.status}|${item.round}|'
            '${item.summary}|${item.error}|${item.updatedAt}',
      )
      .join('\n');

  /// 取消一条排队中的任务（running 会被 Rust 侧按拒绝处理）。
  Future<void> _cancelTask(TaskItem item) async {
    if (_taskCancelling != null) {
      return;
    }
    setState(() => _taskCancelling = item.id);
    final result = await cancelTask(item.id);
    if (!mounted) {
      return;
    }
    setState(() => _taskCancelling = null);
    if (result.ok) {
      _model.addNotice('已取消排队任务：${_oneLine(item.prompt)}');
    } else {
      _model.addError('取消任务失败：${result.error}');
    }
    _refreshTaskQueue();
  }

  /// 新对话 / 切换历史 / 删除当前会话：扫描游标与统计一起清掉。
  ///
  /// **不清盘上的快照**（v1 `snapshots.rs` 的 per-session frames 语义）：每个会话
  /// 一份快照，切回历史会话时还要能回退它改过的文件；真正该丢的只有「会话被删了」
  /// 那一种情况，走 `pruneChangeSessions`。
  void _resetChangeTracking() {
    _changeScanIndex = 0;
    _snapshottedTools.clear();
    _changeStats.clear();
    _noSnapshot.clear();
    _pendingChanges = <String>{};
    // 命令产出是**会话内**的派生视图：换会话 / 新对话时一并清掉（换会话不该继承
    // 上一个会话的「命令产出」标签）。
    _cmdFingerprints.clear();
    _cmdTrackedTools.clear();
    _cmdOutputs.clear();
  }

  /// 点「改动文件」里的一行：只在预览区打开这个文件（不打开资源管理器）。
  void _openChangePreview(String path) {
    final title = path.split(RegExp(r'[\\/]')).last;
    final error = pushPreviewTab({
      'title': title.isEmpty ? path : title,
      'kind': 'change',
      'target': path,
    });
    if (error == null) {
      unawaited(openSandboxWindow());
    } else {
      _model.addNotice('打不开预览区：$error');
      setState(() {});
    }
  }

  /// 点正文 / 工具输出里的 URL：在预览区打开对应 URL 标签。
  ///
  /// 这是纯展示通道：入队失败只在对话流里给一条旁注，不把错误抛给 agent。
  void _openUrlPreview(String url) {
    final error = _pushUrlPreview(url);
    if (error == null) {
      unawaited(openSandboxWindow());
    } else {
      _model.addNotice('打不开预览区：$error');
      setState(() {});
    }
  }

  /// 一轮结束（`result`）把用量追加进本地日志（v1 `appendUsageLog` 同口径）。
  /// 没有模型名时宁可漏记，也不写一条「—」的错账。
  void _recordUsage(Map<String, dynamic> event) {
    if (event['type'] != 'result') {
      return;
    }
    final usage = event['usage'];
    final model = _model.modelName;
    if (usage is! Map || model == null || model.isEmpty) {
      return;
    }
    appendUsageRecord(
      model: model,
      usage: Map<String, dynamic>.from(usage),
      sessionId: event['session_id'] is String
          ? event['session_id'] as String
          : '',
    );
  }

  void _onEvent(Map<String, dynamic> event) {
    if (!mounted) {
      return;
    }
    final wasBusy = _model.busy;
    setState(() => _consumeEvent(event));
    // 流式忙态同步给宿主调度器：忙时定时任务记 `skipped_busy`，不排队、不并发跑 agent。
    _syncCronBusy();
    // 一轮结束（busy true → false）时立刻落盘一次，历史列表里马上能看到。
    if (wasBusy && !_model.busy) {
      _persistTimer?.cancel();
      _persistTimer = null;
      _persist();
    } else if (wasBusy || _model.busy) {
      // 流式中：节流落盘，防止强制结束 / 闪退把本回合整体吞掉。
      _schedulePersist();
    }
    if (wasBusy || _model.busy) {
      _scrollToEnd();
    }
  }

  /// 把「本轮是否还在流式跑」告诉宿主调度器（`cron.rs` 的 `BUSY` 标记）。
  ///
  /// 只在值变化时过 FFI：每个 stream 事件都调一次，但绝大多数是空操作。
  void _syncCronBusy() {
    if (_model.busy == _cronBusy) {
      return;
    }
    _cronBusy = _model.busy;
    setCronBusy(_cronBusy);
  }

  // ── 斜杠命令（只在输入框生效，不进模型、不写历史）────────────────
  //
  // 口径与 v1 `parseSlashCommand` 一致：trim 后首字符 `/`、命令名到第一个空白为止、
  // 大小写不敏感；**命中已知集合才拦截**，未识别的 `/xxx` 当普通文本发送。

  static const List<_SlashItem> _slashItems = [
    _SlashItem('/help', 'help', '显示命令清单'),
    _SlashItem('/compact', 'compact', '立即压缩上下文'),
    _SlashItem('/rewind', 'rewind', '回退最近一轮提问'),
    _SlashItem('/memory ', 'memory', '查看长期记忆'),
    _SlashItem('/memory clear', 'memory', '清空长期记忆'),
    _SlashItem('/longtask ', 'longtask', '长期任务（定时任务）'),
  ];

  static const Set<String> _slashNames = {
    'help',
    'compact',
    'rewind',
    'memory',
    'longtask',
  };

  /// 光标前文本是否处于「命令位置」；是则返回待过滤的命令名前缀（小写）。
  ///
  /// 单行输入框：整段文本即光标前文本（`^\s*\/\S*$`，出现空格即收起菜单）。
  static String? _slashQuery(String text) {
    final match = RegExp(r'^\s*/(\S*)$').firstMatch(text);
    return match?.group(1)?.toLowerCase();
  }

  void _onInputChanged() {
    final query = _slashQuery(_input.text);
    final matches = query == null
        ? const <_SlashItem>[]
        : [
            for (final item in _slashItems)
              if (item.name.startsWith(query)) item,
          ];
    if (!mounted) {
      return;
    }
    if (matches.isEmpty && _slashMatches.isEmpty) {
      return;
    }
    setState(() {
      _slashMatches = matches;
      if (_slashIndex >= matches.length) {
        _slashIndex = 0;
      }
      _slashHidden = false;
    });
  }

  KeyEventResult _onInputKey(FocusNode node, KeyEvent event) {
    // 剪贴板里可能是**文件**（资源管理器里 Ctrl+C 复制文件）：Flutter 默认粘贴只认
    // 文本，这里整段接管 —— 有文件就插胶囊，没有文件再按文本粘到光标处。
    if (event is KeyDownEvent &&
        HardwareKeyboard.instance.isControlPressed &&
        event.logicalKey == LogicalKeyboardKey.keyV) {
      unawaited(_pasteIntoInput());
      return KeyEventResult.handled;
    }
    if (event is! KeyDownEvent || _slashHidden || _slashMatches.isEmpty) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      setState(() => _slashHidden = true);
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.arrowDown ||
        key == LogicalKeyboardKey.arrowUp) {
      final delta = key == LogicalKeyboardKey.arrowDown ? 1 : -1;
      setState(() {
        final count = _slashMatches.length;
        _slashIndex = (_slashIndex + delta + count) % count;
      });
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.tab ||
        (key == LogicalKeyboardKey.enter &&
            !HardwareKeyboard.instance.isShiftPressed)) {
      _adoptSlash();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  /// 采纳候选：把输入框整体替换成命令、光标置尾，**不执行**（再按一次 Enter 才发送）。
  void _adoptSlash() {
    if (_slashMatches.isEmpty) {
      return;
    }
    final item = _slashMatches[_slashIndex.clamp(0, _slashMatches.length - 1)];
    _input.value = TextEditingValue(
      text: item.fill,
      selection: TextSelection.collapsed(offset: item.fill.length),
    );
    setState(() => _slashHidden = true);
    _inputFocus.requestFocus();
  }

  /// 解析 `/name args`；不认识 → null（不拦截）。
  static ({String name, List<String> args})? _parseSlash(String text) {
    if (!text.startsWith('/')) {
      return null;
    }
    final body = text.substring(1);
    final match = RegExp(r'\s').firstMatch(body);
    final name = (match == null ? body : body.substring(0, match.start))
        .toLowerCase();
    if (!_slashNames.contains(name)) {
      return null;
    }
    final rest = match == null ? '' : body.substring(match.start + 1);
    return (
      name: name,
      args: rest.split(RegExp(r'\s+')).where((arg) => arg.isNotEmpty).toList(),
    );
  }

  void _runSlash(String name, List<String> args) {
    switch (name) {
      case 'help':
        _model.addNotice(
          '斜杠命令（只在此输入框生效，不会发送给模型）：\n'
          '/help — 显示本帮助\n'
          '/rewind — 撤销最近一轮提问（需确认；该轮之后改过的文件一起回退）\n'
          '/memory — 查看长期记忆\n'
          '/memory clear — 清空长期记忆（需确认）\n'
          '/longtask — 查看长期任务；/longtask <描述> 创建一条\n'
          '/compact — 立即压缩上下文',
        );
      case 'compact':
        if (!agentRunning) {
          _model.addError('压缩请求发送失败：agent 未启动');
          break;
        }
        _model.markCompactRequested();
        if (!sendAgentLine(jsonEncode(compactPayload()))) {
          _model.addError('压缩请求发送失败（agent 未就绪）');
        }
      case 'rewind':
        if (_model.busy) {
          _model.addNotice('当前回合还在进行，先停止再回退。');
        } else if (!_model.messages.any((message) => message.role == 'user')) {
          _model.addNotice('没有可回退的用户提问。');
        } else {
          _pendingConfirm = 'rewind';
        }
      case 'memory':
        final sub = args.isEmpty ? '' : args.first.toLowerCase();
        if (sub.isEmpty || sub == 'show') {
          final text = readMemoryText();
          _model.addNotice(
            text.trim().isEmpty ? '长期记忆为空。' : '长期记忆（当前内容）：\n$text',
          );
        } else if (sub == 'clear') {
          _pendingConfirm = 'memory_clear';
        } else {
          _model.addNotice('用法：/memory 查看长期记忆；/memory clear 清空。');
        }
      case 'longtask':
        // 定义在 `ModuleData/tasks/cron.json`，由 agent 的 `CronCreate` 写入、
        // 宿主调度器执行；本命令只负责「查看清单」与「代用户发起创建」。
        if (args.isEmpty) {
          final jobs = readCronJobs().jobs;
          final lines = <String>[
            '长期任务（定时任务，由宿主按本地时间调度）：',
            if (jobs.isEmpty)
              '（还没有定义）'
            else
              for (final job in jobs)
                '· ${job.scheduleText} — ${job.prompt}'
                    '${job.enabled ? '' : '（已停用）'}',
            '',
            '用法：/longtask <任务描述>（例如：/longtask 每天早上 9 点汇总昨天的改动）',
          ];
          _model.addNotice(lines.join('\n'));
          break;
        }
        final body = '请用 CronCreate 创建一个长期任务：${args.join(' ')}';
        _model.addUser(body);
        if (!sendAgentLine(jsonEncode(userMessagePayload(body)))) {
          _model.addError('发送失败：agent 未启动（点击右上角「重试」）');
        }
    }
    setState(() {});
    _scrollToEnd(force: true);
  }

  /// 二次确认后的实际执行（`/rewind` 与 `/memory clear`）。
  void _confirmPending() {
    final pending = _pendingConfirm;
    if (pending == null) {
      return;
    }
    setState(() => _pendingConfirm = null);
    switch (pending) {
      case 'rewind':
        final kept = _keptAssistantCountBeforeLastUser();
        final removed = _model.rewindLastUser();
        if (removed == null) {
          _model.addNotice('回退失败：没有可回退的用户提问。');
        } else {
          _input.value = TextEditingValue(
            text: removed,
            selection: TextSelection.collapsed(offset: removed.length),
          );
          sendAgentLine(
            jsonEncode(
              setHistoryPayload([
                for (final message in _model.messages)
                  {'role': message.role, 'content': message.content},
              ]),
            ),
          );
          // 与消息气泡上的「回退到此处」同一条语义：文件也回到那一刻。
          _revertFilesSince(kept);
          _persist();
        }
      case 'memory_clear':
        if (clearMemoryText()) {
          _model.addNotice('已清空长期记忆。');
        } else {
          _model.addError('清空长期记忆失败（详情见日志）。');
        }
    }
    setState(() {});
    _scrollToEnd(force: true);
  }

  void _send() {
    final raw = _input.text;
    final text = raw.trim();
    // §待办 3-28.18：正文里的 `[[file:…]]` 占位符就是附件锚点，按出现顺序取。
    final tokenPaths = attachmentPathsIn(raw);
    final paths = <String>[];
    for (final path in [...tokenPaths, ..._attachments]) {
      if (!paths.contains(path)) {
        paths.add(path);
      }
    }
    if (text.isEmpty && paths.isEmpty) {
      return;
    }
    // 斜杠命令：命中已知集合才拦截（未识别的 `/xxx` 照常当文本发送）。
    final slash = _parseSlash(text);
    if (slash != null) {
      _input.clear();
      _runSlash(slash.name, slash.args);
      return;
    }
    // 占位符 → 可读片段，位置保持不动；早期没有占位符的附件补在末尾（不丢文件）。
    var body = expandAttachmentTokens(raw).trim();
    final extras = [
      for (final path in _attachments)
        if (!tokenPaths.contains(path)) path,
    ];
    for (final path in extras) {
      body = body.isEmpty
          ? attachSegment(path)
          : '$body\n${attachSegment(path)}';
    }
    if (body.isEmpty) {
      body = '请分析这些文件。';
    }
    // 生成中的两种语义（§待办 3-28.1）：排队 vs 立即插入。
    final steerInsert = _model.busy && _insertWhileBusy;
    _model.addUser(body, steerInsert: steerInsert);
    // 先落盘再发：即使发送过程中进程被强制结束，用户这条提问也已经在历史里。
    _persist();
    // vision 缺省 = 开（与 v1 `storage::default_vision` 一致）。
    final vision = readAiConfig()['vision'] != false;
    final sent = sendAgentLine(
      jsonEncode(
        userMessagePayload(
          body,
          attachments: paths,
          vision: vision,
          steerInsert: steerInsert,
        ),
      ),
    );
    if (!sent) {
      _model.addError('发送失败：agent 未启动（点击右上角「重试」）');
    }
    _input.clear();
    _attachments.clear();
    setState(() {});
    _syncCronBusy();
    _scrollToEnd(force: true);
  }

  void _stopTurn() {
    sendAgentLine(jsonEncode(pausePayload()));
  }

  /// 面板内的一句即时提示（不写进对话流）。
  void _showTransient(String text) {
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(
      SnackBar(content: Text(text), duration: const Duration(seconds: 2)),
    );
  }

  /// 回退后把 agent 的上下文同步成界面剩下的消息（v1 `set_history` 同形）。
  void _syncAgentHistory() {
    sendAgentLine(
      jsonEncode(
        setHistoryPayload([
          for (final message in _model.messages)
            {'role': message.role, 'content': message.content},
        ]),
      ),
    );
  }

  /// v1 消息动作条「回退到此处」：第一次点只上膛，第二次才执行（不可撤销）。
  ///
  /// 与 v1 一致：除了回退对话与 agent 上下文，还会把**这次回退点之后**改过的磁盘文件
  /// 一起还原（快照按回合分档，见 `_captureChanges`）。
  void _rewindTo(ChatEntry entry) {
    if (!identical(_armedRewind, entry)) {
      setState(() => _armedRewind = entry);
      return;
    }
    if (_model.busy) {
      _showTransient('回合进行中，先停止再回退');
      return;
    }
    final kept = _keptAssistantCount(entry);
    final removed = _model.rewindTo(entry);
    if (removed == null) {
      setState(() => _armedRewind = null);
      return;
    }
    _syncAgentHistory();
    _revertFilesSince(kept);
    _persist();
    setState(() => _armedRewind = null);
  }

  /// 回退点之前**已完成的助手消息条数**（v1 `keptAssistantCountFor` 同口径）。
  int _keptAssistantCount(ChatEntry entry) {
    var count = 0;
    for (final item in _model.entries) {
      if (identical(item, entry)) {
        break;
      }
      if (item.kind == ChatEntryKind.assistant) {
        count++;
      }
    }
    return count;
  }

  /// 最后一条用户提问之前已完成的助手消息条数（`/rewind` 用）。
  int _keptAssistantCountBeforeLastUser() {
    ChatEntry? last;
    for (final item in _model.entries) {
      if (item.kind == ChatEntryKind.user) {
        last = item;
      }
    }
    return last == null ? 0 : _keptAssistantCount(last);
  }

  /// 回退到第 [kept] 回合之前：把该回合及之后改过的文件还原（v1 `revertFileSnapshots`）。
  void _revertFilesSince(int kept) {
    final result = revertChangesSince(kept, session: _sessionKey);
    if (result.paths.isEmpty) {
      return;
    }
    setState(() {
      for (final path in result.paths) {
        if (isChangePending(path, session: _sessionKey)) {
          continue;
        }
        _model.changedFiles.removeWhere(
          (item) => item.toLowerCase() == path.toLowerCase(),
        );
        _pendingChanges.removeWhere(
          (item) => item.toLowerCase() == path.toLowerCase(),
        );
        _changeStats.removeWhere(
          (key, _) => key.toLowerCase() == path.toLowerCase(),
        );
      }
    });
    _showTransient(
      result.failed > 0
          ? '文件回退有 ${result.failed} 条失败（见日志）'
          : '已回退 ${result.paths.length} 个文件的改动',
    );
  }

  /// §待办 3-28.6：从这条回答分叉 —— 新建一条会话（到这条回答为止的正文），原会话不动。
  ///
  /// 与「回退到此处」的区别：回退会**删掉**原会话后面的内容；分叉把当前这段复制出去，
  /// 原会话保持逐字节不变，两条可以各自继续跑。
  void _forkFrom(ChatEntry entry) {
    if (_model.busy) {
      _showTransient('回合进行中，先停止再分叉');
      return;
    }
    final sessions = loadChatHistoryMeta();
    final snapshot = _model.forkSnapshotAt(
      entry,
      existingTitles: {for (final session in sessions) session.title},
    );
    if (snapshot == null) {
      _showTransient('这条回答不在会话正文里（可能还在流式），无法分叉');
      return;
    }
    // 原会话先落盘，再写分支 —— 这样分支里永远不会有原会话「之后」的内容。
    _persist();
    if (!saveChatHistory(snapshot)) {
      _showTransient('分叉失败：历史写入不可用');
      return;
    }
    final branch = ChatHistorySession.fromJson(snapshot);
    if (branch == null) {
      _showTransient('分叉失败：分支记录无法读回');
      return;
    }
    setState(() => _historyMeta = loadChatHistoryMeta());
    _restoreSession(branch);
    _model.addNotice('已从这条回答分叉成新会话「${branch.title}」；原会话未改动，可在历史里切回。');
    _persist();
    setState(() {});
  }

  /// v1 消息动作条「重试」：回退到这条提问并原样重发。
  void _retryFrom(ChatEntry entry) {
    if (_model.busy) {
      _showTransient('回合进行中，先停止再重试');
      return;
    }
    final kept = _keptAssistantCount(entry);
    final removed = _model.rewindTo(entry);
    if (removed == null) {
      setState(() => _armedRewind = null);
      return;
    }
    _syncAgentHistory();
    // v1 的「重试」也走同一条回退路径（`retryChat` → `rollbackChat`），文件一起还原。
    _revertFilesSince(kept);
    setState(() => _armedRewind = null);
    _input.text = removed;
    _send();
  }

  /// 把当前会话落一次盘（按 `sessionId` 覆盖，幂等）。
  void _persist() {
    final snapshot = _model.historyJson();
    if (snapshot != null) {
      saveChatHistory(snapshot);
    }
  }

  /// 流式期间节流落盘：900ms 内最多一次，避免每个 delta 都序列化 `steps`。
  ///
  /// 触发点在 `_onEvent`（思考 / 正文 / 工具卡都算），因此即使进程被强制结束，
  /// 最多丢最后 0.9 秒的输出；用户提问由 `_send` 同步落盘，不会丢。
  void _schedulePersist() {
    if (_persistTimer?.isActive ?? false) {
      return;
    }
    _persistTimer = Timer(const Duration(milliseconds: 900), () {
      _persistTimer = null;
      _persist();
    });
  }

  /// 打开沙箱盘预览窗（AI 的受控工作目录，`temp/transStorage`）。
  ///
  /// 窗口由宿主建（复用插件窗口基础设施，id = `sandbox`）；没有窗口宿主
  /// （单测 / Linux）时如实提示，不假装打开了。
  Future<void> _openSandbox() async {
    // 同一颗按钮第二次点击 = 关闭预览窗（用户 2026-11-21 口径）。
    if (await isSandboxWindowOpen()) {
      await closeSandboxWindow();
      return;
    }
    final opened = await openSandboxWindow();
    if (opened || !mounted) {
      return;
    }
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(
      const SnackBar(
        content: Text('沙箱预览窗不可用（当前环境没有窗口宿主）'),
        duration: Duration(seconds: 2),
      ),
    );
  }

  // ── 更多设置（v1 `#chat-more-menu` 的五页签）────────────────────────
  //
  // 接有真实后端的五页：权限 / 工作区 / 工具 / 人格 / 安全（白箱边界账本 W0，
  // 口径见 docs/sandbox-research.md §6）。v1 的「其他」（本地模型 / 阶段浮标 /
  // 发送方式 / 定时任务）v2 还没有对应通道，不画空页签（登记在 docs/todo.md）。

  static const List<String> _runModeOrder = ['manual', 'allowlist', 'auto'];
  static const List<String> _profileOrder = ['safe', 'project', 'full'];

  void _toggleMore() {
    if (_moreOpen) {
      setState(() {
        _moreOpen = false;
        _runModeConfirm = false;
      });
      return;
    }
    setState(() {
      _settings = readChatSettings();
      _toolsOff
        ..clear()
        ..addAll(_settings.toolBlacklist);
      _personaText.text = _settings.persona;
      _moreMessage = null;
      _runModeConfirm = false;
      // 停在「安全」页再打开菜单：记录可能又多了，重新读一次。
      if (_moreTab == 'security') {
        _securityRows = readRecentSecurityDecisions();
      }
      _moreOpen = true;
    });
  }

  void _closeMore() {
    setState(() {
      _moreOpen = false;
      _runModeConfirm = false;
    });
  }

  void _reloadSettings() {
    _settings = readChatSettings();
    _toolsOff
      ..clear()
      ..addAll(_settings.toolBlacklist);
    _personaText.text = _settings.persona;
  }

  /// 保存设置并重启 agent（这几项都只在 spawn 时生效，v1 同语义）。
  void _applySettings(Map<String, Object?> patch, String okMessage) {
    final error = saveChatSettings(patch);
    if (error != null) {
      setState(() {
        _moreMessage = error;
        _moreMessageError = true;
      });
      return;
    }
    setState(() {
      _reloadSettings();
      _moreMessage = okMessage;
      _moreMessageError = false;
      _runModeConfirm = false;
    });
    _restartAgent();
  }

  /// 重启常驻 agent（不换会话、不重开面板）。
  void _restartAgent() {
    stopAgent();
    _start();
  }

  /// 选工作区目录（v1 语义：选了才启用工作区锁；切换会重启 agent）。
  Future<void> _pickWorkspace() async {
    final picked = pickWorkspaceDirectory();
    if (picked.isEmpty) {
      return; // 用户取消
    }
    _applySettings({'workspace': picked}, '工作区：$picked');
  }

  /// 恢复默认工作区（应用数据目录，不启用工作区锁）。
  void _resetWorkspace() {
    _applySettings({'workspace': ''}, '工作区：已恢复默认');
  }

  String _runModeLabel(String mode) => switch (mode) {
    'manual' => '手动',
    'auto' => '自动',
    _ => '白名单',
  };

  String _profileLabel(String profile) => switch (profile) {
    'safe' => '只读',
    'full' => '系统',
    _ => '工作区',
  };

  void _cycleRunMode() {
    final index = _runModeOrder.indexOf(_settings.runMode);
    final next = _runModeOrder[(index + 1) % _runModeOrder.length];
    if (next == 'auto') {
      setState(() => _runModeConfirm = true);
      return;
    }
    _applySettings({'runMode': next}, '命令审批方式：${_runModeLabel(next)}');
  }

  void _cycleProfile() {
    final index = _profileOrder.indexOf(_settings.profile);
    final next = _profileOrder[(index + 1) % _profileOrder.length];
    _applySettings({'profile': next}, '权限：${_profileLabel(next)}');
  }

  /// 折叠时机两档循环（§待办 3-28.17）。
  ///
  /// **刻意不走 `_applySettings`**：那个出口每次都会 `_restartAgent()`，而折叠时机是
  /// 纯界面偏好 —— 为它重启 agent 会白白掐掉正在跑的回合。这里只落盘 + 重读设置。
  void _cycleFoldTiming() {
    final next = _settings.foldTiming == 'nextMessage'
        ? 'reply'
        : 'nextMessage';
    final error = saveChatSettings({'foldTiming': next});
    if (error != null) {
      setState(() {
        _moreMessage = error;
        _moreMessageError = true;
      });
      return;
    }
    setState(() {
      _reloadSettings();
      _moreMessage = next == 'nextMessage' ? '细节折叠：等下一条消息' : '细节折叠：回复结束';
      _moreMessageError = false;
    });
  }

  void _saveTools() {
    final candidates = _settings.toolCandidates;
    // 保留用户手写、但不在候选表里的名字（v1 也允许任意自定义名）。
    final list = <String>[
      for (final name in _toolsOff)
        if (!candidates.contains(name)) name,
      for (final name in candidates)
        if (_toolsOff.contains(name)) name,
    ];
    _applySettings({'toolBlacklist': list}, '已保存，AI Agent 已重启');
  }

  void _savePersonaText({bool reset = false}) {
    final text = reset ? '' : _personaText.text;
    final error = savePersona(text);
    if (error != null) {
      setState(() {
        _moreMessage = error;
        _moreMessageError = true;
      });
      return;
    }
    if (reset) {
      _personaText.text = '';
    }
    setState(() {
      _settings = readChatSettings();
      _moreMessage = reset ? '已恢复内置人格 · 重启 AI 后生效' : '已保存 · 重启 AI 后生效';
      _moreMessageError = false;
    });
  }

  /// 展开 / 收起历史列表（面板内切换，不新开窗口）。
  void _toggleHistory() {
    if (_historyOpen) {
      setState(() => _historyOpen = false);
      return;
    }
    setState(() {
      _historyMeta = loadChatHistoryMeta();
      _historyOpen = true;
      _moreOpen = false;
      _runModeConfirm = false;
      _armedRewind = null;
    });
  }

  /// 点历史行：只在这一刻读取完整 payload，避免首屏解析全部会话。
  void _restoreSessionById(ChatHistoryMeta meta) {
    final session = loadChatHistorySession(meta.id);
    if (session == null) {
      _showTransient('读取历史会话失败（记录可能已被删除）');
      return;
    }
    _restoreSession(session);
  }

  /// 恢复一份历史会话：先落盘当前会话，再替换界面条目并回灌 agent 上下文。
  void _restoreSession(ChatHistorySession session) {
    _persist();
    // 改动快照只活在本次进程（v1 同语义），切会话时一起清掉；
    // 但过程快照（思考 / 工具卡 / 已修改的文件）随会话落盘，这里要按回合插回去。
    _resetChangeTracking();
    setState(() {
      _model.restore(
        id: session.id,
        messages: [
          for (final message in session.messages)
            ChatMessage(message.role, message.content),
        ],
        createdAt: session.createdAt,
        steps: session.steps,
        turnDurations: session.turnDurations,
        promptTimeline: session.promptTimeline,
      );
      _seedChangedFilesFromSteps(session.steps);
      _historyOpen = false;
      _armedRewind = null;
    });
    if (!agentRunning) {
      _start();
    }
    // 只换历史、不触发模型调用（协议见 docs/flutter-migration.md）。
    sendAgentLine(
      jsonEncode(
        setHistoryPayload([
          for (final message in session.messages)
            {'role': message.role, 'content': message.content},
        ]),
      ),
    );
    _scrollToEnd(force: true);
  }

  /// 历史会话的「更改文件」列表：从过程快照里的写类工具重建（v1 `SessionStep.path` 同口径）。
  ///
  /// 只还原**列表**：这些文件有没有可回退的快照由 `_computeChangeStats` 现查，查不到就
  /// 标「已无快照」—— 不把「列出来」当成「还能还原」（`forbidden.md` §禁用 1）。
  void _seedChangedFilesFromSteps(List<dynamic> steps) {
    final workdir = _settings.workspace;
    for (final path in changedFilesFromSteps(
      steps,
      resolve: (path) => _absoluteAgentPath(path, workdir),
    )) {
      _model.noteChangedFile(path);
    }
  }

  void _deleteSession(ChatHistoryMeta session) {
    if (!deleteChatHistory(session.id)) {
      return;
    }
    // 会话没了 ⇒ 它在 temp/change-review 下的文件副本也不该留着（v1 `prune_frames`）。
    final survivors = [
      for (final item in loadChatHistoryMeta()) item.id,
      if (_model.sessionId != null && _model.sessionId != session.id)
        _model.sessionId!,
    ];
    pruneChangeSessions(survivors);
    final wasCurrent = _model.sessionId == session.id;
    setState(() {
      _historyMeta = [
        for (final item in _historyMeta)
          if (item.id != session.id) item,
      ];
    });
    if (wasCurrent) {
      // 删的是当前这条：把对话区按「新对话」重置 —— 否则抽屉关掉后，
      // 界面还留着已经删掉的那段内容（用户 2026-11-14 报障）。
      //
      // 不能先 `_persist()`：那会把刚删掉的会话原样写回去。
      stopAgent();
      _resetChangeTracking();
      setState(() {
        _model.clear();
        _historyOpen = false;
        _armedRewind = null;
        _startError = null;
      });
      _start();
    }
  }

  /// 会话轨迹（§待办 3-28.12）：把一条已落盘的会话拍平成 NDJSON 事件行，只读预览，
  /// 需要时再导出成 `.jsonl`。
  ///
  /// 三件事刻意不做：**不发请求**（导出 ≠ 重放）、**不在前端脱敏**（宿主 `mask_secrets` 是
  /// 唯一规则）、**不在前端截断**（单字段 4000 字 / 2000 行 / 预览 256 KiB 都由宿主管）。
  void _showSessionTrace(ChatHistorySession session) =>
      _showTrace('轨迹：${session.title}', _sessionForExport(session));

  /// 历史列表只有元数据；看轨迹时才按 id 读完整会话。
  void _showSessionTraceMeta(ChatHistoryMeta meta) {
    final session = loadChatHistorySession(meta.id);
    if (session == null) {
      _showTransient('读取历史会话失败（记录可能已被删除）');
      return;
    }
    _showSessionTrace(session);
  }

  /// 当前会话的轨迹（§待办 3-28.12 ②）：**不要求先落盘 / 先聊完一轮** ——
  /// 直接用内存里的 `historyJson()` 快照，进行中的回合也能看。
  void _showCurrentTrace() {
    final snapshot = _model.historyJson();
    if (snapshot == null) {
      _showTransient('本会话还没有可看的内容（先发一条消息）');
      return;
    }
    _showTrace('轨迹：当前会话', snapshot);
  }

  void _showTrace(String title, Map<String, Object?> session) {
    final trace = readSessionTrace(session);
    // §待办 3-28.12 ①：SYSTEM / CONTEXT 两栏（只有字符数与哈希，没有正文）。
    final system = session['system'];
    final systemChars = system is Map && system['chars'] is num
        ? (system['chars'] as num).toInt()
        : 0;
    final systemSha = system is Map && system['sha256'] is String
        ? system['sha256'] as String
        : '';
    final contextBlock = session['context'];
    final contextPresent =
        contextBlock is Map && contextBlock['present'] == true;
    final contextChars = contextBlock is Map && contextBlock['chars'] is num
        ? (contextBlock['chars'] as num).toInt()
        : 0;
    final contextSha = contextBlock is Map && contextBlock['sha256'] is String
        ? contextBlock['sha256'] as String
        : '';
    // §待办 3-28.12 ①：逐轮时间线的摘要（明细就在下面的轨迹里）。
    final timelineRaw = session['promptTimeline'];
    final snapshots = <PromptSnapshot>[
      for (final item in (timelineRaw is List ? timelineRaw : const []))
        ?PromptSnapshot.fromJson(item),
    ];
    String? timelineSummary;
    if (snapshots.isNotEmpty) {
      final systemHashes = snapshots
          .map((snapshot) => snapshot.systemSha)
          .where((sha) => sha.isNotEmpty)
          .toSet();
      final injected = snapshots
          .where((snapshot) => snapshot.contextPresent)
          .length;
      timelineSummary =
          '逐轮快照 ${snapshots.length} 条（第 ${snapshots.first.turn}–${snapshots.last.turn} 轮）'
          ' · SYSTEM ${systemHashes.length} 种哈希 · CONTEXT 注入 $injected 次'
          ' · 逐轮明细见下方轨迹';
    }
    String? note;
    showDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setDialogState) => AlertDialog(
          backgroundColor: LunacColors.background,
          title: Text(
            title,
            style: TextStyle(color: LunacColors.text, fontSize: 13.5),
          ),
          content: SizedBox(
            width: 720,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  trace.ok
                      ? '${trace.count} 行 · ${trace.bytes} 字节'
                            '${trace.truncated ? ' · 已截断（单字段 4000 字 / 2000 行上限）' : ''}'
                            ' · 已脱敏（密钥 / Bearer 都打码）'
                      : (trace.error.isEmpty ? '读不了这条会话的轨迹' : trace.error),
                  style: TextStyle(
                    color: trace.ok
                        ? LunacColors.textMuted
                        : LunacColors.danger,
                    fontSize: 11,
                    height: 1.4,
                  ),
                ),
                const SizedBox(height: 8),
                // §待办 3-28.12 ①：SYSTEM / CONTEXT 两栏（只有字符数与哈希，正文不发）。
                Row(
                  children: [
                    Expanded(
                      child: _traceColumn(
                        'SYSTEM',
                        systemSha.isEmpty
                            ? '没有上报（老版本 agent / 本会话还没聊过）'
                            : '$systemChars 字符 · sha256 '
                                  '${_shortHash(systemSha)}',
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: _traceColumn(
                        'CONTEXT',
                        contextPresent
                            ? '$contextChars 字符 · sha256 '
                                  '${_shortHash(contextSha)}'
                            : '本轮没有注入长期记忆',
                      ),
                    ),
                  ],
                ),
                if (timelineSummary != null) ...[
                  const SizedBox(height: 6),
                  Text(
                    timelineSummary,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10.5,
                      height: 1.4,
                    ),
                  ),
                ],
                const SizedBox(height: 8),
                Container(
                  height: 340,
                  padding: const EdgeInsets.all(8),
                  decoration: BoxDecoration(
                    color: LunacColors.surface,
                    borderRadius: BorderRadius.circular(6),
                    border: Border.all(color: LunacColors.divider),
                  ),
                  child: SingleChildScrollView(
                    child: SelectableText(
                      trace.ok ? trace.lines.join('\n') : '',
                      style: TextStyle(
                        color: LunacColors.textDim,
                        fontSize: 10.5,
                        height: 1.45,
                        fontFamily: 'Consolas',
                        fontFamilyFallback: const ['Courier New', 'monospace'],
                      ),
                    ),
                  ),
                ),
                if (note != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: SelectableText(
                      note!,
                      style: TextStyle(
                        color: note!.startsWith('已导出')
                            ? LunacColors.textMuted
                            : LunacColors.danger,
                        fontSize: 11,
                        height: 1.4,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(),
              child: const Text('关闭'),
            ),
            TextButton(
              onPressed: trace.ok
                  ? () {
                      final result = exportSessionJsonl(session);
                      setDialogState(() {
                        note = result.ok
                            ? '已导出：${result.path}（${result.count} 行'
                                  '${result.truncated ? '，已截断' : ''}）'
                            : result.error;
                      });
                    }
                  : null,
              child: const Text('导出 .jsonl'),
            ),
          ],
        ),
      ),
    );
  }

  /// 导出 / 预览用的会话形状：与 `chat_history` 落盘的一致（steps 原样透传）。
  Map<String, Object?> _sessionForExport(ChatHistorySession session) => {
    'id': session.id,
    'title': session.title,
    'createdAt': session.createdAt?.millisecondsSinceEpoch,
    'updatedAt': session.updatedAt?.millisecondsSinceEpoch,
    'messages': [
      for (final message in session.messages)
        {'role': message.role, 'content': message.content},
    ],
    'steps': session.steps,
    if (session.turnDurations.isNotEmpty)
      'turnDurations': session.turnDurations,
    // §待办 3-28.12 ①：逐轮 SYSTEM / CONTEXT 时间线（只有哈希与字符数）。
    if (session.promptTimeline.isNotEmpty)
      'promptTimeline': [
        for (final snapshot in session.promptTimeline) snapshot.toJson(),
      ],
    // §待办 3-28.12 ①：SYSTEM / CONTEXT 两栏（只有哈希与字符数，没有正文）。
    if (session.systemSha.isNotEmpty)
      'system': {'chars': session.systemChars, 'sha256': session.systemSha},
    if (session.contextPresent)
      'context': {
        'present': true,
        'chars': session.contextChars,
        'sha256': session.contextSha,
      },
  };

  /// 轨迹弹窗的两栏（SYSTEM / CONTEXT）：小标题 + 一行说明。
  Widget _traceColumn(String title, String detail) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
    decoration: BoxDecoration(
      color: LunacColors.surface,
      borderRadius: BorderRadius.circular(6),
      border: Border.all(color: LunacColors.divider),
    ),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          title,
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10,
            letterSpacing: 0.8,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 2),
        Text(
          detail,
          style: TextStyle(
            color: LunacColors.textDim,
            fontSize: 10.5,
            height: 1.4,
          ),
        ),
      ],
    ),
  );

  String _shortHash(String hash) =>
      hash.length <= 12 ? hash : hash.substring(0, 12);

  /// 历史行的时间：近期用相对时间，超过 30 天直接给日期。
  String _relativeTime(DateTime? time) {
    if (time == null) {
      return '未知时间';
    }
    final diff = DateTime.now().difference(time);
    if (diff.inMinutes < 1) {
      return '刚刚';
    }
    if (diff.inHours < 1) {
      return '${diff.inMinutes} 分钟前';
    }
    if (diff.inDays < 1) {
      return '${diff.inHours} 小时前';
    }
    if (diff.inDays < 30) {
      return '${diff.inDays} 天前';
    }
    final local = time.toLocal();
    final month = local.month.toString().padLeft(2, '0');
    final day = local.day.toString().padLeft(2, '0');
    return '${local.year}-$month-$day';
  }

  void _respond(
    ChatEntry entry,
    bool allow, {
    Map<String, Object?>? updatedInput,
    String? note,
  }) {
    final requestId = entry.requestId;
    if (requestId == null || entry.handled) {
      return;
    }
    final sent = sendAgentLine(
      jsonEncode(
        approvalResponse(
          requestId,
          entry.toolUseId ?? '',
          allow: allow,
          updatedInput: updatedInput,
        ),
      ),
    );
    if (!sent) {
      // 不标记 handled：卡片留着，用户可以重试；否则会变成「点了没反应」。
      _model.addNotice('审批没有送达 agent（进程未启动或已退出），请点右上角「重试」后再试。');
      setState(() {});
      return;
    }
    setState(() {
      entry.handled = true;
      final label = note ?? (allow ? '已允许' : '已拒绝');
      entry.detail = '${entry.detail ?? ''}\n$label'.trim();
    });
    // 白箱账本（W0）：用户点的那一下单独记一条（与弹卡前的 ask 组成完整链）。
    _logBoundary(
      entry.toolName ?? '',
      entry.input,
      allow ? SecurityDecision.allow : SecurityDecision.deny,
      (code: allow ? 'USER_ALLOW' : 'USER_DENY', detail: ''),
    );
  }

  /// 批量答复一组命令审批（全部允许 / 全部取消）。
  ///
  /// agent 是**按序**逐个阻塞等回包的（见 core-agent `await_approval`），所以每一条都要答，
  /// 少答一条整批就会一直挂着。
  void _respondAll(Iterable<ChatEntry> targets, bool allow) {
    final label = allow ? '已允许' : '已拒绝';
    for (final entry in targets) {
      final requestId = entry.requestId;
      if (requestId == null || entry.handled) {
        continue;
      }
      final sent = sendAgentLine(
        jsonEncode(
          approvalResponse(requestId, entry.toolUseId ?? '', allow: allow),
        ),
      );
      if (!sent) {
        _model.addNotice('审批没有送达 agent（进程未启动或已退出），请点右上角「重试」后再试。');
        break;
      }
      entry.handled = true;
      entry.detail = '${entry.detail ?? ''}\n$label'.trim();
      // 白箱账本（W0）：批量答复按条记（每条都能独立复盘）。
      _logBoundary(
        entry.toolName ?? '',
        entry.input,
        allow ? SecurityDecision.allow : SecurityDecision.deny,
        (code: allow ? 'USER_ALLOW' : 'USER_DENY', detail: ''),
      );
    }
    setState(() {});
  }

  /// 新对话：停掉旧进程 → 清空模型 → 重新拉起（v1 的「新会话」同语义）。
  void _restart() {
    _persist();
    stopAgent();
    _resetChangeTracking();
    setState(() {
      _model.clear();
      _historyOpen = false;
      _startError = null;
      _armedRewind = null;
    });
    _start();
  }

  void _scrollToEnd({bool force = false}) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scroll.hasClients) {
        return;
      }
      final position = _scroll.position;
      if (!force && position.maxScrollExtent - position.pixels > 160) {
        return; // 用户正在往回翻，别抢滚动条
      }
      _scroll.animateTo(
        position.maxScrollExtent,
        duration: const Duration(milliseconds: 160),
        curve: Curves.easeOut,
      );
    });
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      if (_armedRewind != null) {
        setState(() => _armedRewind = null);
        return KeyEventResult.handled;
      }
      if (_moreOpen) {
        _closeMore();
        return KeyEventResult.handled;
      }
      if (_pendingConfirm != null) {
        setState(() => _pendingConfirm = null);
        return KeyEventResult.handled;
      }
      if (_historyOpen) {
        setState(() => _historyOpen = false);
        return KeyEventResult.handled;
      }
      // 关面板会把带焦点的输入框一起卸载：放到当前按键派发之后再做，
      // 否则 focus_manager 会在迭代中看到列表被改（日志里的 `Concurrent modification`）。
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
          widget.onClose();
        }
      });
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      focusNode: _panelFocus,
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
        child: Stack(
          children: [
            // AI 对话背景：自定义背景图铺在面板内部（严格跟随本面板的圆角 / 裁剪），
            // 标题栏 / 输入栏 / 正文都浮在它上面。
            Positioned.fill(child: appearancePanelBackdrop()),
            Column(
              children: [
                _buildHeader(),
                // 用户 2026-11-20 口径：耗时 / 失败 / 产出文件常驻对话顶端（不再在末尾）。
                if (_startError == null &&
                    !_historyOpen &&
                    _model.entries.isNotEmpty)
                  _buildTopStats(),
                Expanded(child: _buildBody()),
                if (_startError == null && !_historyOpen) _buildInputBar(),
              ],
            ),
            // v1 的「更多」菜单是输入栏右上方弹出的浮层；点面板空白处关闭。
            if (_moreOpen) ...[
              Positioned.fill(
                child: GestureDetector(
                  behavior: HitTestBehavior.translucent,
                  onTap: _closeMore,
                ),
              ),
              Positioned(
                left: 8,
                right: 8,
                bottom: 52,
                child: Align(
                  alignment: Alignment.bottomRight,
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(
                      maxWidth: 336,
                      maxHeight: 420,
                    ),
                    child: _buildMoreMenu(),
                  ),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    final model = _model.modelName;
    final usage = _model.inputTokens + _model.outputTokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 8, 12, 6),
      child: Row(
        children: [
          // 独立窗里「返回」= 关窗口，窗口栏的关闭键已经承担；内嵌时才画返回。
          if (widget.windowControls == null) ...[
            IconButton(
              tooltip: '返回（Esc）',
              icon: const Icon(Icons.arrow_back, size: 18),
              color: LunacColors.textDim,
              visualDensity: VisualDensity.compact,
              onPressed: widget.onClose,
            ),
            const SizedBox(width: 2),
          ],
          Text(
            widget.title,
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          if (model != null) ...[
            const SizedBox(width: 8),
            Text(
              model,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ],
          const Spacer(),
          if (usage > 0)
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: Text(
                'tokens $usage',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            ),
          IconButton(
            tooltip: '沙箱盘（AI 的工作目录）',
            icon: const Icon(Icons.terminal, size: 17),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: _openSandbox,
          ),
          // 输入栏在 agent 起不来时整条隐藏（v2 的失败态），这时历史入口退到标题栏 ——
          // 否则用户没有路打开「对话记录」。正常态下它和 v1 一样在输入栏里。
          if (_startError != null && !_historyOpen)
            IconButton(
              tooltip: '聊天历史',
              icon: const Icon(Icons.history, size: 17),
              color: LunacColors.textDim,
              visualDensity: VisualDensity.compact,
              onPressed: _toggleHistory,
            ),
          if (widget.windowControls != null) ...[
            const SizedBox(width: 4),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  Widget _buildBody() {
    if (_historyOpen) {
      return _buildHistory();
    }
    final error = _startError;
    if (error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.cloud_off, size: 28, color: LunacColors.textMuted),
              const SizedBox(height: 10),
              Text(
                error,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 13,
                  height: 1.5,
                ),
              ),
              const SizedBox(height: 6),
              Text(
                '需要 agent.exe；点搜索栏右侧圆钮 →「设置 → AI 模型」填 Base URL / API Key 后即可对话。',
                textAlign: TextAlign.center,
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
              const SizedBox(height: 12),
              TextButton(
                onPressed: _start,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.accent,
                ),
                child: const Text('重试'),
              ),
            ],
          ),
        ),
      );
    }
    // v1 的会话日志第一行永远是一条系统提示（`chat.new_conversation_msg`：
    // 「新对话 · {mode}」，`.chat-msg-system` 居中 / 0.72rem / text-dim / 0.6 透明）。
    if (_model.entries.isEmpty) {
      return _buildSystemLine();
    }
    // v1 的工具结果并进对应的工具卡（`<details class="tool-card">` 里的 `tool-out`），
    // 不单独占一行；配对键就是 `tool_use_id`。
    final results = <String, ChatEntry>{};
    final toolIds = <String>{};
    for (final entry in _model.entries) {
      switch (entry.kind) {
        case ChatEntryKind.toolUse:
          if (entry.toolUseId != null) {
            toolIds.add(entry.toolUseId!);
          }
        case ChatEntryKind.toolResult:
          if (entry.toolUseId != null) {
            results[entry.toolUseId!] = entry;
          }
        default:
          break;
      }
    }
    // 2026-11-20（用户口径）：对话流按「待办任务大抽屉」归类。
    //   · 每个任务段把**思考过程 / 工具调用 / 命令执行 / 审批**收进一个默认折叠的
    //     大抽屉（运行中也不自动摊开，点标题才展开）；
    //   · 模型输出的正文仍平铺可见 —— 阶段小总结不再带复制 / 分叉，
    //     只有**全对话最后一条**总结正文带；
    //   · 耗时 / 失败 / 产出文件由对话顶端常驻统计条承载（见 `_buildTopStats`）。
    final blocks = <Widget>[];
    final taskBody = <Widget>[];
    final taskTools = <ChatEntry>[];
    final group = <ChatEntry>[];
    final approvals = <ChatEntry>[];
    var taskThinking = 0;
    var taskPending = 0;

    void flushGroup() {
      if (group.isEmpty) {
        return;
      }
      final uses = List<ChatEntry>.of(group);
      group.clear();
      taskTools.addAll(uses);
      taskBody.add(
        _ToolGroup(
          key: ValueKey(
            'tool-group-${uses.first.toolUseId ?? taskBody.length}',
          ),
          tools: uses,
          results: results,
          busy: _model.busy,
          // §待办 3-28.17：折叠时机取自对话设置（默认「回复结束」= 改造前行为）；
          // `userTurns` 是「下一条用户消息到达」这个时机的判据（用户条目数）。
          foldTiming: _settings.foldTiming,
          userTurns: _model.entries
              .where((e) => e.kind == ChatEntryKind.user)
              .length,
        ),
      );
    }

    // 多条命令同时等审批时只画**一张**大卡（用户 2026-11-14 要求）：
    // 每条一行「工具名 + 省略的命令」+ 勾 / ×，顶部再给「全部允许 / 全部取消 / 拒绝并暂停」。
    void flushApprovals() {
      if (approvals.isEmpty) {
        return;
      }
      final items = List<ChatEntry>.of(approvals);
      approvals.clear();
      final pending = items.where((item) => !item.handled).toList();
      if (pending.isEmpty) {
        return; // 全部处理完 ⇒ 整块消失
      }
      taskPending += pending.length;
      if (pending.length == 1) {
        // 只剩一条时回到详细卡（命令原文看得全）。
        taskBody.add(
          _buildEntry(pending.first, results: results, streaming: false),
        );
        return;
      }
      taskBody.add(
        _ApprovalGroup(
          key: ValueKey(
            'approval-group-${pending.first.requestId ?? taskBody.length}',
          ),
          entries: pending,
          onAllow: (entry) => _respond(entry, true),
          onDeny: (entry) => _respond(entry, false),
          onAllowAll: () => _respondAll(pending, true),
          onDenyAll: () => _respondAll(pending, false),
        ),
      );
    }

    final last = _model.entries.last;
    final lastText = _lastAssistantText();
    // 第几条用户提问（回合序号）：与 `turnDurationsMs` 同口径（0 起）。
    var turnIndex = -1;
    var segment = 0;
    // 当前任务的主要内容概括（用户提问的首行）。2026-11-20 起抽屉标题只用它，
    // 不再显示「任务 N」；同一回合被正文分成多段时沿用同一标题。
    var taskTitle = '';

    /// 收掉当前大抽屉（正文 / 用户气泡之前的进程块都要落进抽屉）。
    void flushTask({bool endOfTurn = false}) {
      flushGroup();
      flushApprovals();
      if (taskBody.isEmpty && !endOfTurn) {
        return;
      }
      final children = List<Widget>.of(taskBody);
      taskBody.clear();
      final tools = List<ChatEntry>.of(taskTools);
      taskTools.clear();
      final thinking = taskThinking;
      taskThinking = 0;
      final pending = taskPending;
      taskPending = 0;
      // 本轮产出物（§待办 3-28.13）跟着本轮的最后一个抽屉走。
      if (endOfTurn && turnIndex >= 0) {
        final artifacts = _artifactsBlock(turnIndex, results);
        if (artifacts != null) {
          children.add(artifacts);
        }
      }
      if (children.isEmpty) {
        return;
      }
      segment++;
      // 2026-11-21：标题优先用该轮 TodoWrite 的待办内容；没有待办时才回落到
      // 用户提问概括（旧会话 / 模型没写待办的情况）。
      final title = turnIndex >= 0
          ? (_model.todoTitleForTurn(turnIndex) ??
                (taskTitle.isEmpty ? '过程记录' : taskTitle))
          : '过程记录';
      blocks.add(
        _TaskDrawer(
          key: ValueKey('task-$turnIndex-$segment'),
          title: title,
          thinkingChars: thinking,
          tools: tools,
          results: results,
          // 只有「本回合最后一段」在跑时才挂进度点（前面的段已经结束）。
          busy: _model.busy && endOfTurn,
          pendingApprovals: pending,
          children: children,
        ),
      );
    }

    for (final entry in _model.entries) {
      // 已被工具卡消费 ⇒ 跳过（不单独占行）。
      if (entry.kind == ChatEntryKind.toolResult &&
          entry.toolUseId != null &&
          toolIds.contains(entry.toolUseId)) {
        continue;
      }
      // 命令审批：连续的一段合并；已处理的直接消失。
      if (entry.kind == ChatEntryKind.approval &&
          entry.subtype == 'can_use_tool') {
        if (entry.handled) {
          continue;
        }
        flushGroup();
        approvals.add(entry);
        continue;
      }
      if (entry.kind == ChatEntryKind.toolUse) {
        flushApprovals();
        group.add(entry);
        continue;
      }
      if (entry.kind == ChatEntryKind.user) {
        flushTask(endOfTurn: true);
        turnIndex++;
        segment = 0;
        taskTitle = summarizeTaskTitle(entry.text);
        blocks.add(_buildEntry(entry, results: results, streaming: false));
        continue;
      }
      if (entry.kind == ChatEntryKind.thinking) {
        // 思考过程进抽屉（不单独占对话流）。
        flushGroup();
        flushApprovals();
        taskThinking += entry.text.length;
        taskBody.add(
          _ThinkingBlock(
            text: entry.text,
            streaming: _model.busy && identical(entry, last),
          ),
        );
        continue;
      }
      if (entry.kind == ChatEntryKind.assistant) {
        // 正文（含阶段小总结）平铺；只有全对话最后一条总结带复制 / 分叉。
        flushTask();
        blocks.add(
          _buildEntry(
            entry,
            results: results,
            streaming: _model.busy && identical(entry, last),
            showActions: identical(entry, lastText) && !_model.busy,
          ),
        );
        continue;
      }
      // 其它（通知 / 错误 / 计划审批 / 文件卡）：不进抽屉，单独一块。
      flushTask();
      blocks.add(
        _buildEntry(
          entry,
          results: results,
          streaming: _model.busy && identical(entry, last),
        ),
      );
    }
    flushTask(endOfTurn: true);
    return ListView.builder(
      controller: _scroll,
      padding: const EdgeInsets.fromLTRB(14, 4, 14, 10),
      // 第 0 行固定是「新对话 · 思考开 / 关」，与 v1 的日志同构（系统行永不消失）。
      itemCount: blocks.length + 1,
      itemBuilder: (context, index) =>
          index == 0 ? _buildSystemLine() : blocks[index - 1],
    );
  }

  /// 全对话**最后一条**非空模型正文（只有它带复制 / 分叉，用户 2026-11-20 口径）。
  ChatEntry? _lastAssistantText() {
    ChatEntry? found;
    for (final entry in _model.entries) {
      if (entry.kind == ChatEntryKind.assistant &&
          entry.text.trim().isNotEmpty) {
        found = entry;
      }
    }
    return found;
  }

  /// 某一轮的产出物块（§待办 3-28.13）；没有写类工具或拿不到路径时返回 null。
  ///
  /// 数据与「改动文件」抽屉同源：`_changeStats` 由 `readChangeDiff` 算，不另算一份。
  Widget? _artifactsBlock(int turn, Map<String, ChatEntry> results) {
    if (turn < 0) {
      return null;
    }
    final tools = <ChatEntry>[];
    var index = -1;
    for (final entry in _model.entries) {
      if (entry.kind == ChatEntryKind.user) {
        index++;
        continue;
      }
      if (index == turn && entry.kind == ChatEntryKind.toolUse) {
        tools.add(entry);
      }
    }
    final groups = groupToolsByCategory(tools);
    final paths = changedFilesOf([...?groups['create'], ...?groups['edit']]);
    if (paths.isEmpty) {
      return null;
    }
    return _ArtifactsBlock(
      key: ValueKey('artifacts-$turn'),
      paths: paths,
      diffStats: _changeStats,
      onOpenFile: _openChangePreview,
    );
  }

  /// 对话顶端常驻统计条（用户 2026-11-20 口径：耗时 / 失败 / 产出文件要一直在顶，
  /// 不再挂在每轮末尾）。
  Widget _buildTopStats() {
    final stats = _sessionStats();
    final parts = <String>[];
    if (stats.durationMs != null) {
      parts.add('耗时 ${_durationText(stats.durationMs!)}');
    }
    if (stats.fails > 0) {
      parts.add('${stats.fails} 次失败');
    }
    if (stats.created > 0) {
      parts.add('创建 ${stats.created} 个文件');
    }
    if (stats.edited > 0) {
      parts.add('修改 ${stats.edited} 个文件');
    }
    if (parts.isEmpty) {
      parts.add(_model.busy ? '任务进行中…' : '暂无产出');
    }
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 4, 14, 4),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: LunacColors.divider)),
      ),
      child: Row(
        children: [
          Icon(Icons.insights_outlined, size: 12, color: LunacColors.textMuted),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              parts.join(' · '),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
        ],
      ),
    );
  }

  /// 全对话的耗时 / 失败 / 产出文件（与工具卡、抽屉头同一批 `ChatEntry`）。
  _SessionStats _sessionStats() {
    final results = <String, ChatEntry>{};
    final tools = <ChatEntry>[];
    for (final entry in _model.entries) {
      if (entry.kind == ChatEntryKind.toolResult && entry.toolUseId != null) {
        results[entry.toolUseId!] = entry;
      } else if (entry.kind == ChatEntryKind.toolUse) {
        tools.add(entry);
      }
    }
    var fails = 0;
    for (final tool in tools) {
      final id = tool.toolUseId;
      if (id != null && (results[id]?.text ?? '').startsWith('⚠')) {
        fails++;
      }
    }
    final created = changedFilesOf(
      tools.where((tool) => toolCategoryOf(tool.toolName) == 'create'),
    ).length;
    final edited = changedFilesOf(
      tools.where((tool) => toolCategoryOf(tool.toolName) == 'edit'),
    ).length;
    int? duration;
    if (_model.turnDurationsMs.isNotEmpty) {
      duration = _model.turnDurationsMs.fold<int>(0, (sum, ms) => sum + ms);
    }
    return _SessionStats(
      durationMs: duration,
      fails: fails,
      created: created,
      edited: edited,
    );
  }

  /// 会话开头的系统提示（v1 `.chat-msg-system`：居中 / 0.72rem / text-dim / 0.6 透明）。
  Widget _buildSystemLine() {
    final mode = _settings.thinking ? '思考开' : '思考关';
    return Padding(
      padding: const EdgeInsets.only(top: 12, bottom: 4),
      child: Center(
        child: Opacity(
          opacity: 0.6,
          child: Text(
            '新对话 · $mode',
            style: TextStyle(color: LunacColors.textDim, fontSize: 11.5),
          ),
        ),
      ),
    );
  }

  Widget _buildHistory() {
    // v1 的对话记录是抽屉：顶部一行「对话记录」+ 右上角关闭（`#chat-drawer-header`）。
    final body = _historyMeta.isEmpty
        ? Center(
            child: Text(
              '暂无对话记录。开始对话后会自动保存到这里。',
              textAlign: TextAlign.center,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
            ),
          )
        : ListView.builder(
            padding: const EdgeInsets.fromLTRB(10, 2, 10, 8),
            itemCount: _historyMeta.length,
            itemBuilder: (context, index) =>
                _buildHistoryRow(_historyMeta[index]),
          );
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 6, 6, 4),
          child: Row(
            children: [
              Text(
                '对话记录',
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Spacer(),
              IconButton(
                tooltip: '关闭历史',
                icon: const Icon(Icons.close, size: 16),
                color: LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                onPressed: _toggleHistory,
              ),
            ],
          ),
        ),
        Expanded(child: body),
      ],
    );
  }

  Widget _buildHistoryRow(ChatHistoryMeta session) {
    final active = session.id == _model.sessionId;
    final updated = session.updatedAt ?? session.createdAt;
    return Container(
      margin: const EdgeInsets.only(bottom: 4),
      decoration: BoxDecoration(
        color: active ? LunacColors.surfaceHover : null,
        borderRadius: BorderRadius.circular(8),
      ),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: () => _restoreSessionById(session),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(10, 7, 4, 7),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      session.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(color: LunacColors.text, fontSize: 13),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '${_relativeTime(updated)} · ${session.messageCount} 条'
                      '${active ? ' · 当前' : ''}',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 11,
                      ),
                    ),
                  ],
                ),
              ),
              IconButton(
                tooltip: '轨迹 / 导出 JSONL',
                icon: const Icon(Icons.account_tree_outlined, size: 15),
                color: LunacColors.textMuted,
                visualDensity: VisualDensity.compact,
                onPressed: () => _showSessionTraceMeta(session),
              ),
              IconButton(
                tooltip: '删除会话',
                icon: const Icon(Icons.delete_outline, size: 15),
                color: LunacColors.textMuted,
                visualDensity: VisualDensity.compact,
                onPressed: () => _deleteSession(session),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildEntry(
    ChatEntry entry, {
    required Map<String, ChatEntry> results,
    required bool streaming,

    /// 只有全对话最后一条总结正文为真（用户 2026-11-20 口径：阶段小总结不给
    /// 复制 / 分叉，避免把中间过程当成可复用的结论）。
    bool showActions = false,
  }) {
    switch (entry.kind) {
      case ChatEntryKind.user:
        // v1 `.chat-msg-user`：左侧留 1/6 空档、其余满宽（不是右对齐气泡）；
        // 1px 描边 + 8% 主题色底、圆角 10、padding 8×12、正文 0.82rem 左对齐。
        // 悬浮时右下角出现「复制 / 回退 / 重试」（v1 `.msg-actions`）。
        return _UserBubble(
          text: entry.text,
          armed: identical(_armedRewind, entry),
          onRewind: () => _rewindTo(entry),
          onRetry: () => _retryFrom(entry),
        );
      case ChatEntryKind.assistant:
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // 轻量 markdown：标题 / 列表 / 表格 / 围栏代码 + 行内加粗、代码、
              // 本机路径小泡泡（v1 的 pre-wrap 会把 `**` 和 `|` 原样显示出来）。
              ChatMarkdownView(text: entry.text, onOpenUrl: _openUrlPreview),
              // v1 `.flow-actions`：回答下方的图标动作条（复制 / 分叉）。
              // 2026-11-20（用户口径）：只有**全对话最后一条**总结正文才给这两颗键，
              // 阶段小总结（一段思考之后的自动输出）不再出现。
              if (showActions && entry.text.trim().isNotEmpty)
                Align(
                  alignment: Alignment.centerRight,
                  child: Padding(
                    padding: const EdgeInsets.only(top: 2),
                    // §待办 3-28.6：从这条回答分叉（复制到这条为止的正文到新会话，
                    // 原会话保持不变）。
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        _CopyButton(text: entry.text),
                        _ForkButton(onTap: () => _forkFrom(entry)),
                      ],
                    ),
                  ),
                ),
            ],
          ),
        );
      case ChatEntryKind.thinking:
        // v1 的思考块是 `<details>`：跑的时候展开（「思考中… (n 字)」），
        // 收尾后自动收起并改成「思考过程 (n 字)」，点开才看到正文。
        return _ThinkingBlock(text: entry.text, streaming: streaming);
      case ChatEntryKind.toolUse:
        {
          final id = entry.toolUseId;
          final result = id == null ? null : results[id];
          return _ToolCard(
            name: entry.toolName ?? '工具',
            command: entry.detail ?? '',
            output: result?.text ?? '',
            running: result == null && _model.busy,
            toolUseId: id,
          );
        }
      case ChatEntryKind.toolResult:
        // 走到这里 = 没有对应的 tool_use（异常 / 历史残缺）⇒ 单独成卡。
        if (entry.text.trim().isEmpty) {
          return const SizedBox.shrink();
        }
        return _ToolCard(
          name: entry.toolName ?? '工具',
          command: '',
          output: entry.text,
          running: false,
        );
      case ChatEntryKind.notice:
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 3),
          child: Text(
            entry.text,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        );
      case ChatEntryKind.error:
        // §待办 3-28.4：能归类就给三要素，原始错误默认收起（§禁用 59）。
        return _ErrorReport(text: entry.text);
      case ChatEntryKind.approval:
        return _buildApproval(entry);
      case ChatEntryKind.file:
        return _buildFileCard(entry);
    }
  }

  /// agent 通过 `SendUserFile` 呈递的文件卡片（元信息 + 预览 + 打开 / 定位）。
  Widget _buildFileCard(ChatEntry entry) {
    final meta = [
      if (entry.fileSize > 0) formatBytes(entry.fileSize),
      _fileKindLabel(entry.fileKind),
    ].join(' · ');
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 6),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                Icons.insert_drive_file_outlined,
                size: 14,
                color: LunacColors.accent,
              ),
              const SizedBox(width: 6),
              Text(
                '向你发送了一个文件',
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            entry.fileName,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(color: LunacColors.text, fontSize: 13),
          ),
          const SizedBox(height: 2),
          Text(
            entry.filePath,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          if (meta.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                meta,
                style: TextStyle(color: LunacColors.textDim, fontSize: 11),
              ),
            ),
          if (entry.filePreview.trim().isNotEmpty)
            Container(
              margin: const EdgeInsets.only(top: 6),
              padding: const EdgeInsets.all(8),
              decoration: BoxDecoration(
                color: LunacColors.surface,
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(
                entry.filePreview,
                maxLines: 6,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 11,
                  height: 1.45,
                ),
              ),
            ),
          Row(
            children: [
              TextButton(
                onPressed: () => openPath(entry.filePath),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('打开'),
              ),
              TextButton(
                onPressed: () => revealInExplorer(entry.filePath),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.textMuted,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('在资源管理器中显示'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  String _fileKindLabel(String kind) {
    switch (kind) {
      case 'image':
        return '图片';
      case 'text':
        return '文本';
      case 'code':
        return '代码';
      case 'pdf':
        return 'PDF';
      case 'archive':
        return '压缩包';
      case 'audio':
        return '音频';
      case 'video':
        return '视频';
      default:
        return '文件';
    }
  }

  Widget _buildApproval(ChatEntry entry) {
    switch (entry.subtype) {
      case 'elicitation':
        if (entry.handled) {
          return _handledCard(entry, entry.elicitMessage);
        }
        return _ElicitationCard(
          entry: entry,
          onSubmit: (content) =>
              _respond(entry, true, updatedInput: content, note: '已提交'),
          onCancel: () => _respond(entry, false),
        );
      case 'mcp_trust':
        if (entry.handled) {
          return _handledCard(entry, '项目 MCP 服务器');
        }
        return _TrustCard(
          entry: entry,
          onDecide: (allow, remember) => _respond(
            entry,
            allow,
            updatedInput: remember ? const {'remember': true} : null,
            note: allow ? (remember ? '已允许（始终信任）' : '已允许（仅本次）') : '已拒绝',
          ),
        );
      case 'mcp_sampling':
        if (entry.handled) {
          return _handledCard(entry, 'MCP 服务器请求借用本机模型');
        }
        return _SamplingCard(
          entry: entry,
          onDecide: (allow) =>
              _respond(entry, allow, note: allow ? '已允许（本会话）' : '已拒绝'),
        );
    }
    // §待办 3-28.3：计划卡可逐条改 —— 批准回包里带 `plan` + `edited: true`，
    // agent 把改后的那一份当成权威任务清单（core-agent 的 ExitPlanMode）。
    if (entry.subtype == 'can_use_tool' && entry.toolName == 'ExitPlanMode') {
      if (entry.handled) {
        return _handledCard(entry, '计划');
      }
      return _PlanApprovalCard(
        key: ValueKey(entry.requestId ?? entry.toolUseId ?? 'plan'),
        originalPlan: (entry.input?['plan'] is String)
            ? entry.input!['plan'] as String
            : (entry.detail ?? ''),
        onApprove: (edited, note) {
          // 留档失败不打断批准（只是事后取证，见 `platform/plans.dart`）。
          archivePlan(
            original: (entry.input?['plan'] is String)
                ? entry.input!['plan'] as String
                : (entry.detail ?? ''),
            edited: edited,
            note: note,
          );
          _respond(
            entry,
            true,
            updatedInput: {'plan': edited, 'edited': true},
            note: '已批准计划（${edited.split('\n').length} 行）',
          );
        },
        onDeny: () => _respond(entry, false),
      );
    }
    // 命令审批处理完直接消失（用户 2026-11-14 要求）：留一张「已处理」的空卡只是噪音。
    if (entry.handled) {
      return const SizedBox.shrink();
    }
    return _buildToolApproval(entry);
  }

  /// 处理过的审批卡：只留标题与结论，不再占按钮。
  Widget _handledCard(ChatEntry entry, String title) {
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        children: [
          Icon(
            Icons.check_circle_outline,
            size: 14,
            color: LunacColors.textMuted,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              title.isEmpty ? '审批' : title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ),
          Text(
            entry.detail ?? '已处理',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        ],
      ),
    );
  }

  Widget _buildToolApproval(ChatEntry entry) {
    // 白箱账本（W0）：卡片上直接回答「为什么问我」——不让用户去翻日志（v1 `.approval-why` 同位置）。
    final why = describeBoundary(
      toolName: entry.toolName ?? '',
      analysis: entry.analysis,
      runMode: _settings.runMode,
    );
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.accent),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '需要你的确认：${entry.toolName ?? '工具调用'}',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          if ((entry.detail ?? '').isNotEmpty) ...[
            const SizedBox(height: 6),
            Text(
              entry.detail!,
              maxLines: 6,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: LunacColors.textDim,
                fontSize: 12,
                height: 1.45,
              ),
            ),
          ],
          const SizedBox(height: 6),
          Text(
            '为什么问我：${reasonTextFor(why.code, why.detail)}',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.4,
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              TextButton(
                onPressed: () => _respond(entry, true),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('允许'),
              ),
              TextButton(
                onPressed: () => _respond(entry, false),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.danger,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('拒绝'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  // ── 「更多设置」浮层内容（v1 `#chat-more-menu`）────────────────────

  Widget _buildMoreMenu() {
    return Container(
      decoration: BoxDecoration(
        color: LunacColors.surface,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(10),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.42),
            blurRadius: 18,
            offset: const Offset(0, 6),
          ),
        ],
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 7, 4, 5),
            child: Row(
              children: [
                Expanded(child: _buildMoreTabs()),
                IconButton(
                  tooltip: '关闭',
                  icon: const Icon(Icons.close, size: 14),
                  color: LunacColors.textMuted,
                  visualDensity: VisualDensity.compact,
                  onPressed: _closeMore,
                ),
              ],
            ),
          ),
          Divider(height: 1, color: LunacColors.divider),
          Flexible(
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
              child: _buildMorePage(),
            ),
          ),
          if (_moreMessage != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(10, 0, 10, 8),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  _moreMessage!,
                  style: TextStyle(
                    color: _moreMessageError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 10.5,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildMoreTabs() {
    const tabs = <(String, String)>[
      ('profile', '权限'),
      ('workspace', '工作区'),
      ('tools', '工具'),
      ('persona', '人格'),
      ('security', '安全'),
    ];
    return Wrap(
      spacing: 4,
      runSpacing: 4,
      children: [
        for (final (id, label) in tabs)
          InkWell(
            borderRadius: BorderRadius.circular(999),
            onTap: () => setState(() {
              _moreTab = id;
              _moreMessage = null;
              _runModeConfirm = false;
              if (id == 'security') {
                // 进页就读一次盘：记录可能在上一次会话里就写下了。
                _securityRows = readRecentSecurityDecisions();
                _securityNote = null;
              }
            }),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
              decoration: BoxDecoration(
                color: _moreTab == id
                    ? LunacColors.accent.withValues(alpha: 0.18)
                    : Colors.transparent,
                border: Border.all(
                  color: _moreTab == id
                      ? LunacColors.accent.withValues(alpha: 0.5)
                      : LunacColors.border,
                ),
                borderRadius: BorderRadius.circular(999),
              ),
              child: Text(
                label,
                style: TextStyle(
                  color: _moreTab == id
                      ? LunacColors.text
                      : LunacColors.textMuted,
                  fontSize: 11,
                ),
              ),
            ),
          ),
      ],
    );
  }

  Widget _buildMorePage() {
    switch (_moreTab) {
      case 'workspace':
        return _buildMoreWorkspacePage();
      case 'tools':
        return _buildMoreToolsPage();
      case 'persona':
        return _buildMorePersonaPage();
      case 'security':
        return _buildMoreSecurityPage();
      default:
        return _buildMoreProfilePage();
    }
  }

  Widget _buildMoreProfilePage() {
    final auto = _settings.runMode == 'auto';
    final level = _runModeOrder.indexOf(_settings.runMode) + 1;
    final modeColor = auto ? LunacColors.danger : LunacColors.accent;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // 命令审批方式：图标 + 三格档位点阵 + 一句功能简述（v1 `.rm-dots` 同形）。
        InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: _cycleRunMode,
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 7, horizontal: 4),
            child: Row(
              children: [
                Icon(
                  auto ? Icons.gpp_bad_outlined : Icons.shield_outlined,
                  size: 16,
                  color: auto ? LunacColors.danger : LunacColors.textDim,
                ),
                const SizedBox(width: 8),
                for (var i = 0; i < 3; i++)
                  Padding(
                    padding: const EdgeInsets.only(right: 3),
                    child: Container(
                      width: 5,
                      height: 5,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: i < level ? modeColor : LunacColors.border,
                      ),
                    ),
                  ),
                const SizedBox(width: 8),
                Text(
                  '命令审批方式：${_settings.runModeLabel}',
                  style: TextStyle(color: LunacColors.text, fontSize: 12),
                ),
              ],
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.only(left: 28, right: 4),
          child: Text(
            _settings.runModeHint,
            style: TextStyle(
              color: auto ? LunacColors.danger : LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.35,
            ),
          ),
        ),
        if (_runModeConfirm) _buildRunModeConfirm(),
        const SizedBox(height: 10),
        // 权限（文件边界）：点一下循环 只读 → 工作区 → 系统（v1 同交互）。
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '权限',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            Tooltip(
              message:
                  '权限：${_settings.profileLabel} — 决定能否写文件、是否受工作区限制；'
                  '切换会重启 Agent。',
              child: _MoreTextButton(
                label: _settings.profileLabel,
                onTap: _cycleProfile,
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        // 思考开关（v1 从「其他」页挪到「权限」页的两态开关）。
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '思考',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            _MoreToggle(
              value: _settings.thinking,
              onChanged: (value) => _applySettings({
                'thinking': value,
              }, value ? '思考已开启' : '思考已关闭'),
            ),
            const SizedBox(width: 8),
            Text(
              _settings.thinking ? '开' : '关',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ],
        ),
        const SizedBox(height: 10),
        // 第二模型预审（§待办 3-28.14）：只 deny / escalate，永不 allow。
        // 预审模型端点由环境变量决定（`LUNAC_SECOND_REVIEW_URL` 或 `LUNAC_LOCAL_MODEL_URL`）；
        // 没配端点时 agent 只在日志里说明一句，然后照旧弹人工卡 —— 开关不会假装审过。
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '第二模型预审',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            _MoreToggle(
              value: _settings.secondReview,
              onChanged: (value) => _applySettings({
                'secondReview': value,
              }, value ? '第二模型预审已开启' : '第二模型预审已关闭'),
            ),
            const SizedBox(width: 8),
            Text(
              _settings.secondReview ? '开' : '关',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ],
        ),
        Padding(
          padding: const EdgeInsets.only(left: 28, right: 4),
          child: Text(
            '只可能「拒绝」或「升级到人工卡」，永不自动放行；'
            '需要预审模型端点（LUNAC_SECOND_REVIEW_URL / LUNAC_LOCAL_MODEL_URL），'
            '没配时照旧弹卡。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.35,
            ),
          ),
        ),
        const SizedBox(height: 10),
        // 技能级记忆（§待办 3-28.10 ①）：只影响「技能 fork 出来的子代理」，
        // 主对话前缀不动；开关关着时 agent 完全不碰技能记忆目录。
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '技能记忆',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            _MoreToggle(
              value: _settings.skillMemory,
              onChanged: (value) => _applySettings({
                'skillMemory': value,
              }, value ? '技能级记忆已开启' : '技能级记忆已关闭'),
            ),
            const SizedBox(width: 8),
            Text(
              _settings.skillMemory ? '开' : '关',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ],
        ),
        Padding(
          padding: const EdgeInsets.only(left: 28, right: 4),
          child: Text(
            '打开后，某个技能跑在子代理里时会带上它自己的笔记，'
            '它记下的东西也写回该技能（不动主对话的长期记忆）。'
            '可在「技能」页查看与清空。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.35,
            ),
          ),
        ),
        const SizedBox(height: 10),
        // PTC / Code Mode（§待办 3-28.15）：关着时模型看不到 RunScript。
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '脚本批处理',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            _MoreToggle(
              value: _settings.ptcMode,
              onChanged: (value) => _applySettings({
                'ptcMode': value,
              }, value ? '脚本批处理已开启（RunScript 可用）' : '脚本批处理已关闭'),
            ),
            const SizedBox(width: 8),
            Text(
              _settings.ptcMode ? '开' : '关',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ],
        ),
        Padding(
          padding: const EdgeInsets.only(left: 28, right: 4),
          child: Text(
            '开启后模型可用 RunScript 把多次只读调用写成一段脚本一次跑完；'
            '每次调用仍逐次弹审批，超步数 / 超时 / 输出超限即终止。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.35,
            ),
          ),
        ),
        const SizedBox(height: 10),
        // 细节折叠时机（§待办 3-28.17）：两档，点一下切换并落盘。
        // 「回复结束」= 改造前的行为；「下一条消息」= 回合跑完后过程还摊着，
        // 直到用户发下一条才收起（长回合后能回头看命令输出）。
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '细节折叠',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            Tooltip(
              message:
                  '折叠时机：${_settings.foldTimingLabel} — 选「下一条消息」时，'
                  '回合结束后命令组不会立刻收起，等你发下一条消息才折叠。',
              child: _MoreTextButton(
                label: _settings.foldTimingLabel,
                onTap: _cycleFoldTiming,
              ),
            ),
          ],
        ),
      ],
    );
  }

  /// 自动审批的二次确认（v1 `askRunModeAutoConfirm`：就地问，不另开弹窗）。
  Widget _buildRunModeConfirm() {
    return Container(
      margin: const EdgeInsets.only(left: 28, top: 5, right: 4),
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
      decoration: BoxDecoration(
        color: LunacColors.danger.withValues(alpha: 0.08),
        border: Border.all(color: LunacColors.danger.withValues(alpha: 0.4)),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        spacing: 6,
        runSpacing: 4,
        children: [
          Text(
            '切换到自动审批？工具调用将不再询问。',
            style: TextStyle(color: LunacColors.danger, fontSize: 10.5),
          ),
          _MoreTextButton(
            label: '开启自动审批',
            onTap: () => _applySettings({'runMode': 'auto'}, '命令审批方式：自动'),
          ),
          _MoreTextButton(
            label: '取消',
            onTap: () => setState(() => _runModeConfirm = false),
          ),
        ],
      ),
    );
  }

  Widget _buildMoreWorkspacePage() {
    final path = _settings.workspace;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Icon(Icons.folder_outlined, size: 14, color: LunacColors.textDim),
            SizedBox(width: 8),
            Text(
              '工作区',
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ],
        ),
        const SizedBox(height: 6),
        Container(
          padding: const EdgeInsets.all(8),
          decoration: BoxDecoration(
            color: LunacColors.background,
            border: Border.all(color: LunacColors.border),
            borderRadius: BorderRadius.circular(6),
          ),
          child: SelectableText(
            path.isEmpty ? '未知路径' : path,
            style: TextStyle(
              color: LunacColors.textDim,
              fontSize: 11,
              height: 1.4,
              fontFamily: 'Consolas',
              fontFamilyFallback: ['Courier New', 'monospace'],
            ),
          ),
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            _MoreTextButton(
              label: '打开目录',
              onTap: () {
                if (openPath(path)) {
                  return;
                }
                setState(() {
                  _moreMessage = '打开目录失败：$path';
                  _moreMessageError = true;
                });
              },
            ),
            const SizedBox(width: 8),
            _MoreTextButton(label: '选择文件夹', onTap: _pickWorkspace),
            if (_settings.workspaceConfigured) ...[
              const SizedBox(width: 8),
              _MoreTextButton(label: '恢复默认', onTap: _resetWorkspace),
            ],
          ],
        ),
        const SizedBox(height: 8),
        Text(
          _settings.workspaceConfigured
              ? '已选定工作区：agent 的文件读写被限制在这个目录内（越界直接拒绝，'
                    '「系统」权限档会忽略该限制）；写入仍逐次走审批卡。切换会重启 Agent。'
              : '当前是默认的应用数据目录（未启用工作区锁）：agent 可访问任意路径，'
                    '写操作逐次询问。选择文件夹后会启用工作区锁，越界读写被直接拒绝。',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10.5,
            height: 1.4,
          ),
        ),
      ],
    );
  }

  Widget _buildMoreToolsPage() {
    final candidates = _settings.toolCandidates;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '工具黑名单（禁用后从请求中剔除，可减少缓存丢失；保存后 AI Agent 将重启）',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10.5,
            height: 1.4,
          ),
        ),
        const SizedBox(height: 4),
        if (candidates.isEmpty)
          Padding(
            padding: EdgeInsets.symmetric(vertical: 8),
            child: Text(
              '工具清单不可用（Rust 桥未加载）',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
        for (final name in candidates) _buildToolRow(name),
        const SizedBox(height: 8),
        Row(
          children: [_MoreTextButton(label: '保存并重启', onTap: _saveTools)],
        ),
      ],
    );
  }

  Widget _buildToolRow(String name) {
    final enabled = !_toolsOff.contains(name);
    return InkWell(
      borderRadius: BorderRadius.circular(4),
      onTap: () => setState(() {
        if (!_toolsOff.remove(name)) {
          _toolsOff.add(name);
        }
      }),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 3),
        child: Row(
          children: [
            Icon(
              enabled ? Icons.check : Icons.close,
              size: 13,
              color: enabled ? LunacColors.textMuted : LunacColors.danger,
            ),
            const SizedBox(width: 8),
            Text(
              name,
              style: TextStyle(
                color: enabled ? LunacColors.text : LunacColors.textMuted,
                fontSize: 11.5,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildMorePersonaPage() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(
                '人格',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
            _MoreTextButton(
              label: _personaEditing ? '收起' : '编辑',
              onTap: () => setState(() => _personaEditing = !_personaEditing),
            ),
          ],
        ),
        if (_personaEditing) ...[
          const SizedBox(height: 8),
          TextField(
            controller: _personaText,
            minLines: 5,
            maxLines: 8,
            maxLength: _settings.personaMaxChars,
            buildCounter: (
              context, {
              required currentLength,
              required isFocused,
              maxLength,
            }) => null,
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 11.5,
              height: 1.45,
            ),
            cursorColor: LunacColors.accent,
            decoration: InputDecoration(
              isDense: true,
              contentPadding: const EdgeInsets.all(8),
              filled: true,
              fillColor: LunacColors.background,
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
                borderSide: BorderSide(
                  color: LunacColors.accent.withValues(alpha: 0.6),
                ),
              ),
            ),
          ),
          const SizedBox(height: 6),
          Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              _MoreTextButton(label: '保存', onTap: () => _savePersonaText()),
              _MoreTextButton(
                label: '恢复内置人格',
                onTap: () => _savePersonaText(reset: true),
              ),
              _MoreTextButton(
                label: '立即重启 AI',
                onTap: () {
                  _restartAgent();
                  setState(() {
                    _moreMessage = 'AI 已重启，新人格已生效';
                    _moreMessageError = false;
                  });
                },
              ),
            ],
          ),
        ],
      ],
    );
  }

  // ── 安全页：白箱边界账本（W0）────────────────────────────────────

  /// 刷新最近判定（同步读，默认 200 条；桥接缺失时返回空表）。
  void _refreshSecurity() {
    setState(() {
      _securityRows = readRecentSecurityDecisions();
      _securityNote = null;
    });
  }

  /// 导出复盘包（自包含 JSON：账本 + policy + 版本 / OS build，不含凭据）。
  void _exportSecurity() {
    final result = exportSecurityLedger(
      policy: {
        'runMode': _settings.runMode,
        'profile': _settings.profile,
        'workspace': _settings.workspace,
        'toolBlacklist': _settings.toolBlacklist.length,
        // v2 暂无用户白名单（v1 的「始终允许」未移植），如实写 0 而不是假装有。
        'whitelistTools': 0,
        'whitelistCommands': 0,
      },
      appVersion: kAppVersion,
    );
    setState(() {
      _securityNote = result.path != null
          ? '已导出：${result.path}'
          : (result.error ?? '导出失败');
    });
  }

  /// 判定预演：只跑判定流水线，**不执行**命令（v1 `doSecurityPreview` 同口径）。
  ///
  /// 静态分析来自 `agent.exe --analyze`（与真实审批同一套 `bash_safety`），所以命令原文里
  /// 的危险 / 不可判定 / 可证只读都能在预演里出现；agent 不可用时如实说明「只按运行方式判」。
  Future<void> _previewSecurity() async {
    final command = _securityPreviewText.text.trim();
    if (command.isEmpty) {
      return;
    }
    const toolName = 'Cmd';
    setState(() {
      _securityPreviewResult = '正在分析…';
      _securityNote = null;
    });
    final scanned = await analyzeCommand(command);
    if (!mounted) {
      return;
    }
    final why = describeBoundary(
      toolName: toolName,
      analysis: scanned.analysis,
      runMode: _settings.runMode,
    );
    final auto = shouldAutoApprove(
      toolName: toolName,
      analysis: scanned.analysis,
      runMode: _settings.runMode,
    );
    setState(() {
      _securityPreviewResult =
          '${auto ? '自动放行' : '会弹卡确认'} · ${why.code} · '
          '${reasonTextFor(why.code, why.detail)}';
      _securityNote = scanned.error == null
          ? null
          : '静态分析不可用（${scanned.error}），本次只按运行方式判';
    });
  }

  Widget _buildMoreSecurityPage() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '边界账本 · 本地记录每次自动放行 / 人工确认 / 拒绝；不上传。',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10.5,
            height: 1.4,
          ),
        ),
        const SizedBox(height: 6),
        Wrap(
          spacing: 6,
          runSpacing: 4,
          children: [
            _MoreTextButton(label: '刷新', onTap: _refreshSecurity),
            _MoreTextButton(label: '导出', onTap: _exportSecurity),
          ],
        ),
        if (_securityNote != null)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Text(
              _securityNote!,
              style: TextStyle(color: LunacColors.textDim, fontSize: 10.5),
            ),
          ),
        const SizedBox(height: 6),
        if (_securityRows.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text(
              '还没有记录',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          )
        else
          for (final row in _securityRows.take(200)) _buildSecurityRow(row),
        const SizedBox(height: 10),
        Text(
          '判定预演',
          style: TextStyle(color: LunacColors.textDim, fontSize: 12),
        ),
        const SizedBox(height: 4),
        TextField(
          controller: _securityPreviewText,
          onSubmitted: (_) => unawaited(_previewSecurity()),
          style: TextStyle(color: LunacColors.text, fontSize: 11.5),
          cursorColor: LunacColors.accent,
          decoration: InputDecoration(
            isDense: true,
            contentPadding: const EdgeInsets.all(8),
            hintText: '粘贴一条命令（不会执行）',
            hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            filled: true,
            fillColor: LunacColors.background,
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
              borderSide: BorderSide(
                color: LunacColors.accent.withValues(alpha: 0.6),
              ),
            ),
          ),
        ),
        const SizedBox(height: 6),
        Row(
          children: [
            _MoreTextButton(
              label: '预演',
              onTap: () => unawaited(_previewSecurity()),
            ),
          ],
        ),
        if (_securityPreviewResult.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Text(
              _securityPreviewResult,
              style: TextStyle(
                color: LunacColors.text,
                fontSize: 11,
                height: 1.4,
              ),
            ),
          ),
        const SizedBox(height: 6),
        Text(
          '预演只跑判定、不执行；静态分析来自 agent（与审批同一套规则），'
          'agent 不可用时只按运行方式判。',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10.5,
            height: 1.4,
          ),
        ),
      ],
    );
  }

  Widget _buildSecurityRow(SecurityEntry row) {
    final time = row.ts.replaceFirst('T', ' ').replaceFirst('Z', '');
    final color = switch (row.decision) {
      'deny' => LunacColors.danger,
      'ask' => LunacColors.textDim,
      _ => LunacColors.accent,
    };
    return Tooltip(
      message: row.target.isEmpty
          ? row.scope
          : '${row.target}${row.scope.isEmpty ? '' : '\n${row.scope}'}',
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 3),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 5,
                    vertical: 1,
                  ),
                  decoration: BoxDecoration(
                    color: color.withValues(alpha: 0.12),
                    border: Border.all(color: color.withValues(alpha: 0.4)),
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: Text(
                    row.reasonCode,
                    style: TextStyle(color: color, fontSize: 9.5),
                  ),
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    time,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 9.5,
                      fontFamily: 'Consolas',
                      fontFamilyFallback: ['Courier New', 'monospace'],
                    ),
                  ),
                ),
                Text(
                  row.tool,
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 9.5),
                ),
              ],
            ),
            Padding(
              padding: const EdgeInsets.only(top: 2, left: 1),
              child: Text(
                row.reasonText,
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 10.5,
                  height: 1.35,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildInputBar() {
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 6, 10, 8),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: LunacColors.divider)),
      ),
      child: Column(
        children: [
          if (_model.todos.isNotEmpty ||
              _model.changedFiles.isNotEmpty ||
              _model.backgroundCommands.isNotEmpty ||
              _cronJobs.isNotEmpty ||
              _taskItems.isNotEmpty ||
              _activityCount > 0)
            _buildTodoStrip(),
          if (_pendingConfirm != null) _buildConfirmRow(),
          if (!_slashHidden && _slashMatches.isNotEmpty) _buildSlashMenu(),
          // 输入框（2026-11-20 用户口径）：明显的边框 + 与面板底色不同的内部底色。
          // 外层 `surface` 是面板底，输入区用 `background` 才能「看得出是一块输入区」；
          // 聚焦时边框转主题色（rest 态仍是中性边框，不闪）。
          Container(
            decoration: BoxDecoration(
              color: LunacColors.background,
              borderRadius: BorderRadius.circular(10),
              border: Border.all(
                color: _inputFocus.hasFocus
                    ? LunacColors.accent
                    : LunacColors.border,
                width: _inputFocus.hasFocus ? 1.2 : 1,
              ),
            ),
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
            child: Row(
              children: [
                // v1 输入栏左侧就是「新建对话」（`#chat-new-btn`）。
                IconButton(
                  tooltip: '新建对话',
                  icon: const Icon(Icons.add_comment_outlined, size: 18),
                  color: LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                  onPressed: _restart,
                ),
                Expanded(
                  child: TextField(
                    controller: _input,
                    focusNode: _inputFocus,
                    style: TextStyle(color: LunacColors.text, fontSize: 13),
                    cursorColor: LunacColors.accent,
                    decoration: InputDecoration(
                      isDense: true,
                      border: InputBorder.none,
                      hintText: '随便问...',
                      hintStyle: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 13,
                      ),
                    ),
                    onSubmitted: (_) => _send(),
                  ),
                ),
                if (_model.busy)
                  IconButton(
                    tooltip: '停止生成',
                    icon: const Icon(Icons.stop_circle_outlined, size: 18),
                    color: LunacColors.textMuted,
                    visualDensity: VisualDensity.compact,
                    onPressed: _stopTurn,
                  ),
                // v1 的「更多」菜单按钮在输入栏里（`#chat-more-btn`，位于历史键左侧）。
                IconButton(
                  tooltip: '更多设置',
                  icon: const Icon(Icons.more_horiz, size: 18),
                  color: _moreOpen
                      ? LunacColors.accent
                      : (_settings.runMode == 'auto'
                            ? LunacColors.danger
                            : LunacColors.textDim),
                  visualDensity: VisualDensity.compact,
                  onPressed: _toggleMore,
                ),
                // v1 的「历史记录」按钮在输入栏里（`#chat-history-btn` 紧挨发送键）。
                IconButton(
                  tooltip: _historyOpen ? '返回对话' : '聊天历史',
                  icon: Icon(
                    _historyOpen ? Icons.chat_bubble_outline : Icons.history,
                    size: 18,
                  ),
                  color: LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                  onPressed: _toggleHistory,
                ),
                // 生成中的「排队 ⇄ 立即插入」开关（§待办 3-28.1 的前端半边；
                // 「暂停」是左边那颗停止键，三态各走各的）。
                if (_model.busy)
                  IconButton(
                    tooltip: _insertWhileBusy
                        ? '立即插入：改回「排队」'
                        : '排队中（回合结束后发送）：改为「立即插入」',
                    icon: Icon(
                      _insertWhileBusy ? Icons.bolt : Icons.schedule,
                      size: 18,
                    ),
                    color: _insertWhileBusy
                        ? LunacColors.accent
                        : LunacColors.textDim,
                    visualDensity: VisualDensity.compact,
                    onPressed: () =>
                        setState(() => _insertWhileBusy = !_insertWhileBusy),
                  ),
                IconButton(
                  tooltip: '发送',
                  icon: const Icon(Icons.send, size: 18),
                  color: LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                  onPressed: _send,
                ),
              ],
            ),
          ),
          // 底部一行（用户 2026-11-20 口径）：上下文水位吐在**同一行**上，不再上下各占一行。
          Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              // 上下文水位（§待办 3-28.8）：实测体积 / 预算，60% 起提示「可能压缩」。
              _ContextWaterBar(
                level: contextLevelOf(
                  _model.contextWaterLevel,
                  elideRatio: _model.elideRatio,
                  dropRatio: _model.dropRatio,
                ),
                ratio: _model.contextWaterLevel,
                tokens: _model.lastContextTokens,
                limit: _model.contextLimit,
                elideRatio: _model.elideRatio,
                dropRatio: _model.dropRatio,
              ),
              const Spacer(),
              // v1 `#status-bar` 的表盘：缓存命中率 + 累计 tokens，点开是「用量与成本」数据面板。
              UsageDashboard(
                session: UsageSessionStats(
                  hit: _model.hitTokens,
                  miss: _model.missTokens,
                  output: _model.totalOutputTokens,
                  elided: _model.elidedItems,
                  dropped: _model.droppedItems,
                  revision: _model.usageRevision,
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  /// 活动面板要显示「有事在发生」的条数（在跑的工具 + 待审批 + 在跑的后台子代理）。
  ///
  /// 只做计数，用于抽屉是否出现与页签上的角标；正文全部来自 `_buildActivityPage`。
  /// 后台子代理算进实时活动：它跨回合继续烧 token，用户必须一眼看到它还在跑。
  int get _activityCount =>
      _model.runningTools.length +
      _model.pendingApprovals.length +
      _model.runningBackgroundTasks;

  /// 当前会话 id（改动快照按会话隔离；还没有 id 时用空串 = 老布局的根目录）。
  String get _sessionKey => _model.sessionId ?? '';

  /// 任务抽屉（v1 输入框上方那条，四页互斥：活动 / 待办 / 改动文件 / 后台命令）。
  ///
  /// 「活动」= §待办 3-28.2 的活动面板：正在跑的工具 / 待审批 / 子代理 /
  /// 后台命令 / 改动文件，**同一份事件流**，不新建状态源。
  /// 位置与 v1 相同（输入框正上方）；「改动文件」点一行**只在预览区打开**该文件，
  /// 不在对话流里画 diff（用户 2026-11-14 口径）。
  Widget _buildTodoStrip() {
    final todos = _model.todos;
    final files = _model.changedFiles;
    final background = _model.backgroundCommands;
    final activity = _activityCount;
    final cron = _cronJobs;
    final tasks = _taskItems;
    // 排队 / 运行中的任务才算「活动」；完成态留在列表里回顾，不凑计数。
    final liveTasks = tasks
        .where((item) => item.isPending || item.isRunning)
        .length;
    final done = todos.where((todo) => todo.status == 'completed').length;
    // 「活动」页同时承载定时任务（3-28.16）与后台任务队列（3-28.16 ①）：
    // 没有在跑的工具时，页签按「日程 + 在排任务」计数，不把静态的日程算进「活动 N」。
    final activityLabel = activity > 0
        ? '活动 $activity'
        : cron.isEmpty && liveTasks == 0
        ? '活动'
        : liveTasks > 0
        ? '任务 ${cron.length + liveTasks}'
        : '定时 ${cron.length}';
    final page = switch (_todoPage) {
      'activity' when activity > 0 || cron.isNotEmpty || liveTasks > 0 =>
        'activity',
      'files' when files.isNotEmpty => 'files',
      'bg' when background.isNotEmpty => 'bg',
      'tasks' when todos.isNotEmpty => 'tasks',
      'trace' => 'trace',
      _ =>
        activity > 0 || cron.isNotEmpty || liveTasks > 0
            ? 'activity'
            : files.isNotEmpty
            ? 'files'
            : background.isNotEmpty
            ? 'bg'
            : 'tasks',
    };
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 4, 4, 4),
            child: Row(
              children: [
                Icon(Icons.checklist, size: 13, color: LunacColors.textMuted),
                const SizedBox(width: 4),
                if (activity > 0 || cron.isNotEmpty || liveTasks > 0) ...[
                  _drawerTab('activity', activityLabel),
                  const SizedBox(width: 2),
                ],
                _drawerTab(
                  'tasks',
                  todos.isEmpty ? '待办' : '待办 $done/${todos.length}',
                ),
                const SizedBox(width: 2),
                _drawerTab('files', '改动文件 ${files.length}'),
                const SizedBox(width: 2),
                _drawerTab('bg', '后台 ${background.length}'),
                const SizedBox(width: 2),
                // §待办 3-28.12 ②：当前会话的轨迹（不必先落盘 / 先聊完一轮）。
                _drawerTab('trace', '轨迹'),
                const Spacer(),
                InkWell(
                  borderRadius: BorderRadius.circular(4),
                  onTap: () => setState(() => _todoOpen = !_todoOpen),
                  child: Padding(
                    padding: const EdgeInsets.all(4),
                    child: Icon(
                      _todoOpen ? Icons.expand_less : Icons.expand_more,
                      size: 15,
                      color: LunacColors.textMuted,
                    ),
                  ),
                ),
              ],
            ),
          ),
          if (_todoOpen)
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 170),
              child: ListView(
                shrinkWrap: true,
                padding: const EdgeInsets.fromLTRB(10, 0, 10, 6),
                children: switch (page) {
                  'activity' => _buildActivityPage(files, background),
                  'files' => [
                    if (files.isEmpty) _drawerEmpty('本次会话还没有改动文件'),
                    for (final path in files) _buildChangeRow(path),
                  ],
                  'bg' => [
                    if (background.isEmpty) _drawerEmpty('没有正在后台运行的命令'),
                    for (final command in background)
                      _buildBackgroundRow(command),
                  ],
                  'trace' => _buildTracePage(),
                  _ => [
                    if (todos.isEmpty) _drawerEmpty('暂无待办'),
                    for (final todo in todos) _buildTodoRow(todo),
                  ],
                },
              ),
            ),
        ],
      ),
    );
  }

  /// 活动面板正文（§待办 3-28.2）：一页看全「正在发生的事」。
  ///
  /// 五段全部是**同一份事件流的派生视图**：正在跑的工具 / 待审批 / 子代理 /
  /// 后台命令 / 改动文件。抽屉收起时事件照常进模型，展开即最新
  /// （没有「折叠后停止更新」的第二套状态）。
  List<Widget> _buildActivityPage(
    List<String> files,
    List<BackgroundCommand> background,
  ) {
    final running = _model.runningTools;
    final approvals = _model.pendingApprovals;
    final agents = _model.agentCalls;
    final bgTasks = _model.backgroundTasks;
    final cron = _cronJobs;
    final tasks = _taskItems;
    if (running.isEmpty &&
        approvals.isEmpty &&
        agents.isEmpty &&
        bgTasks.isEmpty &&
        background.isEmpty &&
        cron.isEmpty &&
        tasks.isEmpty &&
        files.isEmpty) {
      return [_drawerEmpty('当前没有进行中的活动')];
    }
    final finished = _model.finishedToolIds;
    return [
      if (running.isNotEmpty) ...[
        _drawerSection('正在跑'),
        for (final tool in running)
          _buildActivityRow(
            Icons.play_arrow_outlined,
            tool.toolName ?? '工具',
            _oneLine(tool.detail ?? ''),
          ),
      ],
      if (approvals.isNotEmpty) ...[
        _drawerSection('待审批'),
        for (final approval in approvals)
          _buildActivityRow(
            Icons.gpp_maybe_outlined,
            approval.toolName ?? '工具',
            '等你决定是否放行',
          ),
      ],
      if (agents.isNotEmpty) ...[
        _drawerSection('子代理${_subagentUsageLabel()}'),
        for (final agent in agents)
          _buildActivityRow(
            Icons.account_tree_outlined,
            'Agent',
            _oneLine(agent.detail ?? ''),
            trailing: finished.contains(agent.toolUseId) ? '已回' : '在跑',
          ),
      ],
      if (background.isNotEmpty) ...[
        _drawerSection('后台命令'),
        for (final command in background) _buildBackgroundRow(command),
      ],
      if (bgTasks.isNotEmpty) ...[
        // 后台子代理（§待办 3-28.7）：跨回合还在跑的那些 —— 显示角色 / 当前工具 /
        // 已跑时长，并提供一键终止（取消在下一个检查点生效，不中断正在飞的请求）。
        _drawerSection('后台子代理'),
        for (final task in bgTasks) _buildBackgroundTaskRow(task),
      ],
      if (cron.isNotEmpty) ...[
        // 定时任务（3-28.16）：静态日程也放进活动面板 —— 创建 / 删除仍归 agent。
        _drawerSection('定时任务'),
        for (final job in cron) _buildCronRow(job),
      ],
      if (tasks.isNotEmpty) ...[
        // 后台任务队列（3-28.16 ①）：入队归 agent（`TaskEnqueue`），这里只看状态 / 取消。
        _drawerSection('后台任务'),
        for (final item in tasks.take(8)) _buildTaskRow(item),
      ],
      if (files.isNotEmpty) ...[
        _drawerSection('改动文件'),
        for (final path in files) _buildChangeRow(path),
      ],
    ];
  }

  /// 活动面板的一条定时任务：调度描述 + 提示词 + 下次时间 / 上次结果 + 「立即运行」。
  Widget _buildCronRow(CronJob job) {
    final running = _cronRunning == job.id;
    final canRun = _cronRunning == null && !_model.busy;
    final state = job.autoDisabled
        ? '已自动停用'
        : !job.enabled
        ? '已停用'
        : job.nextRunAtLocal.isNotEmpty
        ? '下次 ${job.nextRunAtLocal}'
        : job.lastStatus.isNotEmpty
        ? '上次 ${job.lastStatus}'
        : '';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2, horizontal: 2),
      child: Row(
        children: [
          Icon(
            Icons.schedule,
            size: 12,
            color: job.autoDisabled || !job.enabled
                ? LunacColors.danger
                : LunacColors.textMuted,
          ),
          const SizedBox(width: 6),
          Text(
            job.scheduleText.isEmpty ? '定时' : job.scheduleText,
            style: TextStyle(color: LunacColors.text, fontSize: 11.5),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              _oneLine(job.prompt),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
          if (state.isNotEmpty) ...[
            const SizedBox(width: 6),
            Text(
              state,
              style: TextStyle(
                color: job.autoDisabled || !job.enabled
                    ? LunacColors.danger
                    : LunacColors.textMuted,
                fontSize: 10,
              ),
            ),
          ],
          const SizedBox(width: 2),
          TextButton(
            onPressed: _cronToggling == null
                ? () => _toggleCronEnabled(job)
                : null,
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textMuted,
              visualDensity: VisualDensity.compact,
              minimumSize: const Size(0, 22),
              padding: const EdgeInsets.symmetric(horizontal: 6),
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            child: Text(
              _cronToggling == job.id ? '…' : (job.enabled ? '停用' : '启用'),
              style: const TextStyle(fontSize: 10.5),
            ),
          ),
          TextButton(
            onPressed: canRun ? () => _runCronNow(job) : null,
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.accent,
              visualDensity: VisualDensity.compact,
              minimumSize: const Size(0, 22),
              padding: const EdgeInsets.symmetric(horizontal: 6),
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            child: Text(
              running ? '运行中…' : '运行',
              style: const TextStyle(fontSize: 10.5),
            ),
          ),
        ],
      ),
    );
  }

  /// 活动面板的一条后台任务：状态 + 提示词 + 轮次 / 摘要 + 「取消」（只对 pending）。
  ///
  /// 状态颜色：运行中 = 强调色，失败 = 危险色，完成 / 取消 = 弱化色。
  /// `round / maxRounds` 只在这一条任务会续跑时才有意义（只读档 + 清单未完成）。
  Widget _buildTaskRow(TaskItem item) {
    final color = item.isRunning
        ? LunacColors.accent
        : item.status == 'failed'
        ? LunacColors.danger
        : LunacColors.textMuted;
    final detail = item.isRunning
        ? '第 ${item.round}/${item.maxRounds} 轮'
        : item.status == 'failed' && item.error.isNotEmpty
        ? _oneLine(item.error)
        : item.summary.isNotEmpty
        ? _oneLine(item.summary)
        : '';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2, horizontal: 2),
      child: Row(
        children: [
          Icon(
            switch (item.status) {
              'running' => Icons.sync,
              'done' => Icons.check_circle_outline,
              'failed' => Icons.error_outline,
              'cancelled' => Icons.cancel_outlined,
              _ => Icons.schedule,
            },
            size: 12,
            color: color,
          ),
          const SizedBox(width: 6),
          Text(
            item.statusLabel,
            style: TextStyle(color: color, fontSize: 11.5),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              _oneLine(item.prompt),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
          if (detail.isNotEmpty) ...[
            const SizedBox(width: 6),
            Text(detail, style: TextStyle(color: color, fontSize: 10)),
          ],
          const SizedBox(width: 2),
          TextButton(
            onPressed: item.isPending && _taskCancelling == null
                ? () => _cancelTask(item)
                : null,
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textMuted,
              visualDensity: VisualDensity.compact,
              minimumSize: const Size(0, 22),
              padding: const EdgeInsets.symmetric(horizontal: 6),
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            child: Text(
              _taskCancelling == item.id ? '…' : '取消',
              style: const TextStyle(fontSize: 10.5),
            ),
          ),
        ],
      ),
    );
  }

  /// 子代理的累计用量（`result.usage.subagent`，**其中**来自子代理的部分）。
  ///
  /// 事件流里只有回合级的归因，没有「每个子代理烧了多少」⇒ 这里只报累计，
  /// 不假装逐个子代理的数字。
  String _subagentUsageLabel() {
    if (_model.subagentRequests <= 0) {
      return '';
    }
    final tokens = _model.subagentTokens;
    final tokenText = tokens >= 1000
        ? '${(tokens / 1000).toStringAsFixed(1)}k'
        : '$tokens';
    return ' · 累计 ${_model.subagentRequests} 次请求 / $tokenText tokens';
  }

  /// 单行摘要：折叠换行、截到 80 字（面板宽度有限，长参数没有信息量）。
  String _oneLine(String text) {
    final flat = text.replaceAll(RegExp(r'\s+'), ' ').trim();
    return flat.length <= 80 ? flat : '${flat.substring(0, 80)}…';
  }

  Widget _drawerSection(String title) {
    return Padding(
      padding: const EdgeInsets.only(top: 4, bottom: 2),
      child: Text(
        title,
        style: TextStyle(
          color: LunacColors.textMuted,
          fontSize: 10.5,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
  }

  /// 活动面板的一行（工具 / 审批 / 子代理）：图标 + 名称 + 一行摘要 + 可选状态。
  Widget _buildActivityRow(
    IconData icon,
    String name,
    String detail, {
    String? trailing,
  }) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2.5, horizontal: 2),
      child: Row(
        children: [
          Icon(icon, size: 12, color: LunacColors.textMuted),
          const SizedBox(width: 6),
          Text(name, style: TextStyle(color: LunacColors.text, fontSize: 11.5)),
          if (detail.isNotEmpty) ...[
            const SizedBox(width: 6),
            Expanded(
              child: Text(
                detail,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            ),
          ] else
            const Spacer(),
          if (trailing != null) ...[
            const SizedBox(width: 6),
            Text(
              trailing,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          ],
        ],
      ),
    );
  }

  /// 「轨迹」页（§待办 3-28.12 ②）：当前会话的 SYSTEM / CONTEXT 摘要 + 完整视图入口。
  ///
  /// 抽屉只有 170px 高，整段 NDJSON 放不下 —— 完整视图仍是 720×340 的对话框
  /// （与历史行的「轨迹」同一份渲染 / 同一份导出）。
  List<Widget> _buildTracePage() {
    final hasBody = _model.messages.isNotEmpty;
    final system = _model.systemSha.isEmpty
        ? '未上报'
        : '${_model.systemChars} 字符 · sha256 ${_model.systemHashShort}…';
    final contextText = _model.contextPresent
        ? '${_model.contextChars} 字符 · sha256 ${_model.contextHashShort}…'
        : '本轮没有注入长期记忆';
    return [
      _drawerHint('SYSTEM：$system'),
      _drawerHint('CONTEXT：$contextText'),
      Row(
        children: [
          _drawerHint(
            hasBody
                ? '${_model.messages.length} 条消息 · '
                      '${_model.stepsJson().length} 个过程步骤'
                : '还没有内容',
          ),
          const Spacer(),
          TextButton(
            onPressed: hasBody ? _showCurrentTrace : null,
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textMuted,
              visualDensity: VisualDensity.compact,
              minimumSize: const Size(0, 22),
              padding: const EdgeInsets.symmetric(horizontal: 6),
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            child: const Text('打开完整轨迹', style: TextStyle(fontSize: 10.5)),
          ),
        ],
      ),
      if (!hasBody) _drawerEmpty('先发一条消息，轨迹才有东西可看'),
    ];
  }

  Widget _drawerTab(String page, String label) {
    final active = page == _todoPage;
    return InkWell(
      borderRadius: BorderRadius.circular(5),
      onTap: () {
        setState(() {
          _todoPage = page;
          _todoOpen = true;
        });
        if (page == 'files') {
          _computeChangeStats();
        }
      },
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
        decoration: BoxDecoration(
          color: active ? LunacColors.surfaceHover : Colors.transparent,
          borderRadius: BorderRadius.circular(5),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: active ? LunacColors.text : LunacColors.textMuted,
            fontSize: 11,
          ),
        ),
      ),
    );
  }

  Widget _drawerEmpty(String text) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Text(
        text,
        style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
      ),
    );
  }

  /// 抽屉里的一行说明（比 `_drawerEmpty` 更紧凑；用于轨迹页的 SYSTEM / CONTEXT）。
  Widget _drawerHint(String text) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: Text(
        text,
        style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
      ),
    );
  }

  /// 改动文件一行：文件名 + 目录 + `+N −M`（已确认的显示「已处理」）。
  ///
  /// 点击 → 预览区打开该文件的 diff（红色删除 / 绿色新增）。
  Widget _buildChangeRow(String path) {
    final name = path.split(RegExp(r'[\\/]')).last;
    final dir = path.length > name.length
        ? path.substring(0, path.length - name.length - 1)
        : '';
    final stat = _changeStats[path];
    final pending = _pendingChanges.contains(path);
    final cmdOutput = _cmdOutputs.contains(path);
    final noSnapshot = _noSnapshot.contains(path);
    return InkWell(
      borderRadius: BorderRadius.circular(4),
      onTap: () => _openChangePreview(path),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 3, horizontal: 2),
        child: Row(
          children: [
            Icon(
              Icons.description_outlined,
              size: 12,
              color: LunacColors.textMuted,
            ),
            const SizedBox(width: 6),
            Expanded(
              child: Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text: name,
                      style: TextStyle(color: LunacColors.text, fontSize: 11.5),
                    ),
                    if (dir.isNotEmpty)
                      TextSpan(
                        text: '  $dir',
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 10,
                        ),
                      ),
                  ],
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (pending && stat != null && stat.ok)
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: Text(
                  '+${stat.added} −${stat.removed}',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              ),
            if (cmdOutput)
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: Tooltip(
                  message: '命令运行产生的文件（只列出来；命令执行前没有快照，不能还原）',
                  child: Text(
                    '命令产出',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ),
              )
            else if (!pending && noSnapshot)
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: Tooltip(
                  message: '历史会话恢复出来的文件：当次运行的快照已经不在了，不能还原',
                  child: Text(
                    '已无快照',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ),
              )
            else if (!pending)
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: Text(
                  '已处理',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              ),
            const SizedBox(width: 4),
            Icon(Icons.open_in_new, size: 11, color: LunacColors.textMuted),
          ],
        ),
      ),
    );
  }

  /// 待办抽屉里的一条后台命令（v1 任务抽屉的 `bg` 段）：摘要 + PID + 取消。
  Widget _buildBackgroundRow(BackgroundCommand command) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          Icon(Icons.terminal, size: 12, color: LunacColors.textMuted),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              command.label.isEmpty ? command.id : command.label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          if (command.pid > 0)
            Padding(
              padding: const EdgeInsets.only(left: 6),
              child: Text(
                'PID ${command.pid}',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
              ),
            ),
          const SizedBox(width: 6),
          Tooltip(
            message: '取消这条后台命令',
            child: InkWell(
              borderRadius: BorderRadius.circular(4),
              onTap: command.toolUseId.isEmpty
                  ? null
                  : () => sendAgentLine(
                      jsonEncode(toolControlPayload(command.toolUseId, 'stop')),
                    ),
              child: Padding(
                padding: EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                child: Text(
                  '取消',
                  style: TextStyle(color: LunacColors.danger, fontSize: 10),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 活动面板里的一条后台子代理（§待办 3-28.7）。
  ///
  /// 跑着时给「已跑 Ns · 第 k 轮 · 当前工具」+「终止」；收口后给结果与花费
  /// （`spent` 是 agent 报的加权单位 —— 只报实测值，不估算）。
  Widget _buildBackgroundTaskRow(BackgroundTask task) {
    final running = task.running;
    final parts = <String>[
      task.role,
      if (running && task.round > 0) '第 ${task.round} 轮',
      if (running && task.startedMs > 0)
        '${((DateTime.now().millisecondsSinceEpoch - task.startedMs) / 1000).round()}s',
      if (!running && task.elapsedMs > 0)
        '${(task.elapsedMs / 1000).toStringAsFixed(1)}s',
      if (!running && task.spent > 0) '${task.spent} 单位',
      if (!running)
        task.statusText
      else if (task.tool != null && task.tool!.isNotEmpty)
        task.tool!,
    ];
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2, horizontal: 2),
      child: Row(
        children: [
          Icon(
            running ? Icons.smart_toy_outlined : Icons.smart_toy,
            size: 12,
            color: running
                ? LunacColors.accent
                : task.status == 'failed' || task.status == 'cancelled'
                ? LunacColors.danger
                : LunacColors.textMuted,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              task.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.text, fontSize: 11.5),
            ),
          ),
          if (parts.isNotEmpty) ...[
            const SizedBox(width: 6),
            Text(
              parts.join(' · '),
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ],
          const SizedBox(width: 6),
          Tooltip(
            message: running ? '终止这个后台子代理' : '已结束',
            child: InkWell(
              borderRadius: BorderRadius.circular(4),
              onTap: running ? () => _stopBackgroundTask(task) : null,
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                child: Text(
                  running ? '终止' : '已结束',
                  style: TextStyle(
                    color: running ? LunacColors.danger : LunacColors.textMuted,
                    fontSize: 10,
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 终止一个后台子代理：置位它的取消标志（agent 在下一个检查点收工）。
  void _stopBackgroundTask(BackgroundTask task) {
    sendAgentLine(jsonEncode(stopBackgroundTaskPayload(task.id)));
  }

  Widget _buildTodoRow(TodoItem todo) {
    final done = todo.status == 'completed';
    final active = todo.status == 'in_progress';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            done
                ? Icons.check_circle
                : (active
                      ? Icons.radio_button_checked
                      : Icons.radio_button_unchecked),
            size: 13,
            color: done
                ? LunacColors.textMuted
                : (active ? LunacColors.accent : LunacColors.textDim),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              todo.content,
              style: TextStyle(
                color: done ? LunacColors.textMuted : LunacColors.textDim,
                fontSize: 11,
                height: 1.4,
                decoration: done ? TextDecoration.lineThrough : null,
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 斜杠候选菜单：↑↓ 选择、Enter / Tab 采纳、Esc 收起、点条目采纳。
  Widget _buildSlashMenu() {
    return Container(
      margin: const EdgeInsets.only(bottom: 4),
      padding: const EdgeInsets.symmetric(vertical: 2),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var index = 0; index < _slashMatches.length; index++)
            InkWell(
              onTap: () {
                setState(() => _slashIndex = index);
                _adoptSlash();
              },
              child: Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 5,
                ),
                color: index == _slashIndex
                    ? LunacColors.surfaceHover
                    : Colors.transparent,
                child: Row(
                  children: [
                    SizedBox(
                      width: 110,
                      child: Text(
                        _slashMatches[index].fill.trim(),
                        style: TextStyle(
                          color: LunacColors.accent,
                          fontSize: 12,
                        ),
                      ),
                    ),
                    Expanded(
                      child: Text(
                        _slashMatches[index].description,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 11,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// 破坏性命令的就地二次确认（`/rewind`、`/memory clear`）。
  Widget _buildConfirmRow() {
    final rewind = _pendingConfirm == 'rewind';
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.danger),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              rewind
                  ? '其后的对话将被移除，该轮之后改过的文件也会回退（不可撤销）—— 确认回退？'
                  : '将清空全部长期记忆（不可撤销）。确认吗？',
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          TextButton(
            onPressed: _confirmPending,
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.danger,
              visualDensity: VisualDensity.compact,
            ),
            child: const Text('确认'),
          ),
          TextButton(
            onPressed: () => setState(() => _pendingConfirm = null),
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textMuted,
              visualDensity: VisualDensity.compact,
            ),
            child: const Text('取消'),
          ),
        ],
      ),
    );
  }
}

/// 斜杠命令候选的一条。
class _SlashItem {
  const _SlashItem(this.fill, this.name, this.description);

  /// 采纳后填入输入框的内容（`/memory ` 带尾空格，方便接着补 `clear`）。
  final String fill;
  final String name;
  final String description;
}

/// 计划审批卡（§待办 3-28.3）：逐条开关 + 一行备注，批准时把**改后**的计划交回 agent。
///
/// 与普通审批卡的区别：`ExitPlanMode` 的入参就是整份计划，批准前得能改。
/// 关掉的步骤不会出现在回包里（`editedPlanText` 按 1..N 重排）；
/// 全部关掉时批准键不可点 —— 交一份空计划出去等于让 agent 随便发挥。
/// 键盘/焦点安全：卡片不设「回车即批准」，批准必须是一次明确的点击。
class _PlanApprovalCard extends StatefulWidget {
  const _PlanApprovalCard({
    super.key,
    required this.originalPlan,
    required this.onApprove,
    required this.onDeny,
  });

  /// 模型给出的原始计划全文。
  final String originalPlan;

  /// 批准：`edited` 是去掉关掉的步骤、重新编号后的正文；`note` 是那一行备注。
  final void Function(String edited, String note) onApprove;
  final VoidCallback onDeny;

  @override
  State<_PlanApprovalCard> createState() => _PlanApprovalCardState();
}

class _PlanApprovalCardState extends State<_PlanApprovalCard> {
  late final List<PlanStep> _steps = planStepsOf(widget.originalPlan);
  final TextEditingController _note = TextEditingController();

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  int get _kept => _steps.where((step) => step.enabled).length;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.accent),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                Icons.assignment_outlined,
                size: 14,
                color: LunacColors.accent,
              ),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  '计划待批准',
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              Text(
                '${_steps.length} 步 · 保留 $_kept 步',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
              ),
            ],
          ),
          const SizedBox(height: 4),
          Text(
            '关掉的步骤不会交给 AI；批准后它会按下面的顺序执行。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.4,
            ),
          ),
          if (_steps.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Text(
                '这份计划没有可拆分的步骤，将整体批准。',
                style: TextStyle(color: LunacColors.textDim, fontSize: 12),
              ),
            ),
          for (var i = 0; i < _steps.length; i++)
            InkWell(
              borderRadius: BorderRadius.circular(4),
              onTap: () =>
                  setState(() => _steps[i].enabled = !_steps[i].enabled),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 3, horizontal: 2),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(
                      _steps[i].enabled
                          ? Icons.check_box_outlined
                          : Icons.check_box_outline_blank,
                      size: 15,
                      color: _steps[i].enabled
                          ? LunacColors.accent
                          : LunacColors.textMuted,
                    ),
                    const SizedBox(width: 6),
                    Expanded(
                      child: Text(
                        _steps[i].title,
                        style: TextStyle(
                          color: _steps[i].enabled
                              ? LunacColors.text
                              : LunacColors.textMuted,
                          fontSize: 12,
                          height: 1.4,
                          decoration: _steps[i].enabled
                              ? null
                              : TextDecoration.lineThrough,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          const SizedBox(height: 6),
          TextField(
            controller: _note,
            style: TextStyle(color: LunacColors.text, fontSize: 12),
            decoration: InputDecoration(
              isDense: true,
              hintText: '一行备注（可选，会一起交给 AI）',
              hintStyle: TextStyle(
                color: LunacColors.textMuted,
                fontSize: 11.5,
              ),
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 8,
                vertical: 8,
              ),
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(6),
                borderSide: BorderSide(color: LunacColors.border),
              ),
            ),
            onChanged: (_) => setState(() {}),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              TextButton(
                onPressed: _kept == 0 && _note.text.trim().isEmpty
                    ? null
                    : () => widget.onApprove(
                        editedPlanText(_steps, note: _note.text),
                        _note.text,
                      ),
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('批准并执行'),
              ),
              TextButton(
                onPressed: widget.onDeny,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.danger,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('拒绝'),
              ),
              const Spacer(),
              if (_kept < _steps.length)
                Text(
                  '已关掉 ${_steps.length - _kept} 步',
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10.5,
                  ),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

/// 审批卡外框：四类审批共用同一视觉（深底 + 描边）。
class _ApprovalFrame extends StatelessWidget {
  const _ApprovalFrame({
    required this.title,
    required this.children,
    this.borderColor,
  });

  final String title;
  final List<Widget> children;
  final Color? borderColor;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: borderColor ?? LunacColors.accent),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 8),
          ...children,
        ],
      ),
    );
  }
}

/// elicitation 表单卡（MCP 服务端在 `tools/call` 前向用户收集输入）。
///
/// 回包复用审批通道：`updatedInput` 就是用户填的表单值（MCP 规范要的 `content`）。
class _ElicitationCard extends StatefulWidget {
  const _ElicitationCard({
    required this.entry,
    required this.onSubmit,
    required this.onCancel,
  });

  final ChatEntry entry;
  final ValueChanged<Map<String, Object?>> onSubmit;
  final VoidCallback onCancel;

  @override
  State<_ElicitationCard> createState() => _ElicitationCardState();
}

class _ElicitationCardState extends State<_ElicitationCard> {
  late final Map<String, TextEditingController> _text = {
    for (final field in widget.entry.elicitFields)
      if (field.type != 'boolean' && field.options.isEmpty)
        field.key: TextEditingController(text: field.defaultValue ?? ''),
  };
  late final Map<String, String> _choice = {
    for (final field in widget.entry.elicitFields)
      if (field.options.isNotEmpty)
        field.key: field.options.contains(field.defaultValue)
            ? field.defaultValue!
            : field.options.first,
  };
  late final Map<String, bool> _checked = {
    for (final field in widget.entry.elicitFields)
      if (field.type == 'boolean') field.key: field.defaultValue == 'true',
  };

  @override
  void dispose() {
    for (final controller in _text.values) {
      controller.dispose();
    }
    super.dispose();
  }

  void _submit() {
    final content = <String, Object?>{};
    for (final field in widget.entry.elicitFields) {
      if (field.type == 'boolean') {
        content[field.key] = _checked[field.key] ?? false;
      } else if (field.options.isNotEmpty) {
        content[field.key] = _choice[field.key];
      } else if (field.type == 'number' || field.type == 'integer') {
        content[field.key] =
            num.tryParse(_text[field.key]?.text.trim() ?? '') ?? 0;
      } else {
        content[field.key] = _text[field.key]?.text ?? '';
      }
    }
    widget.onSubmit(content);
  }

  @override
  Widget build(BuildContext context) {
    final message = widget.entry.elicitMessage;
    return _ApprovalFrame(
      title: message.isEmpty ? '需要你补充信息' : message,
      children: [
        for (final field in widget.entry.elicitFields) _buildField(field),
        Row(
          children: [
            TextButton(
              onPressed: _submit,
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.accent,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('提交'),
            ),
            TextButton(
              onPressed: widget.onCancel,
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.danger,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('取消'),
            ),
          ],
        ),
      ],
    );
  }

  Widget _buildField(ElicitField field) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Row(
        children: [
          SizedBox(
            width: 120,
            child: Text(
              '${field.label}${field.required ? ' *' : ''}',
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ),
          Expanded(child: _buildControl(field)),
        ],
      ),
    );
  }

  Widget _buildControl(ElicitField field) {
    if (field.type == 'boolean') {
      return Align(
        alignment: Alignment.centerLeft,
        child: SizedBox(
          height: 24,
          child: Checkbox(
            value: _checked[field.key] ?? false,
            visualDensity: VisualDensity.compact,
            materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
            activeColor: LunacColors.accent,
            onChanged: (value) =>
                setState(() => _checked[field.key] = value ?? false),
          ),
        ),
      );
    }
    if (field.options.isNotEmpty) {
      return DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: _choice[field.key],
          isDense: true,
          dropdownColor: LunacColors.surface,
          style: TextStyle(color: LunacColors.text, fontSize: 12),
          items: [
            for (final option in field.options)
              DropdownMenuItem(value: option, child: Text(option)),
          ],
          onChanged: (value) =>
              setState(() => _choice[field.key] = value ?? _choice[field.key]!),
        ),
      );
    }
    final numeric = field.type == 'number' || field.type == 'integer';
    return SizedBox(
      height: 26,
      child: TextField(
        controller: _text[field.key],
        style: TextStyle(color: LunacColors.text, fontSize: 12),
        cursorColor: LunacColors.accent,
        keyboardType: numeric ? TextInputType.number : TextInputType.text,
        decoration: InputDecoration(
          isDense: true,
          filled: true,
          fillColor: LunacColors.surfaceHover,
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(6),
            borderSide: BorderSide.none,
          ),
          hintText: field.description,
          hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          contentPadding: const EdgeInsets.symmetric(
            horizontal: 8,
            vertical: 6,
          ),
        ),
      ),
    );
  }
}

/// 项目 MCP 信任卡：`.mcp.json` 要在本机拉起这些命令，用户先点头。
class _TrustCard extends StatelessWidget {
  const _TrustCard({required this.entry, required this.onDecide});

  final ChatEntry entry;

  /// `(是否放行, 是否始终信任)`。
  final void Function(bool allow, bool remember) onDecide;

  @override
  Widget build(BuildContext context) {
    return _ApprovalFrame(
      title: '项目要启动这些 MCP 服务器',
      children: [
        if (entry.trustProject.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: Text(
              entry.trustProject,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
        for (final server in entry.trustServers)
          Padding(
            padding: const EdgeInsets.only(bottom: 4),
            child: Row(
              children: [
                Text(
                  server.name.isEmpty ? '未命名' : server.name,
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  server.transport,
                  style: TextStyle(color: LunacColors.accent, fontSize: 10),
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    server.target,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: LunacColors.textDim, fontSize: 11),
                  ),
                ),
              ],
            ),
          ),
        Padding(
          padding: EdgeInsets.only(top: 2),
          child: Text(
            '这些命令会在本机运行，请确认来源可信。',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        ),
        Row(
          children: [
            TextButton(
              onPressed: () => onDecide(true, false),
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.accent,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('仅本次'),
            ),
            TextButton(
              onPressed: () => onDecide(true, true),
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.accent,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('始终信任'),
            ),
            TextButton(
              onPressed: () => onDecide(false, false),
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.danger,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('拒绝'),
            ),
          ],
        ),
      ],
    );
  }
}

/// MCP sampling 授权卡：服务器要借本机模型；只有「允许（本会话）/ 拒绝」两档。
class _SamplingCard extends StatelessWidget {
  const _SamplingCard({required this.entry, required this.onDecide});

  final ChatEntry entry;
  final ValueChanged<bool> onDecide;

  @override
  Widget build(BuildContext context) {
    return _ApprovalFrame(
      title: 'MCP 服务器请求借用本机模型',
      borderColor: entry.samplingViaHttp ? LunacColors.danger : null,
      children: [
        if (entry.samplingViaHttp)
          Padding(
            padding: EdgeInsets.only(bottom: 6),
            child: Text(
              '这条请求由服务器经 HTTP 主动推来，请确认来源。',
              style: TextStyle(color: LunacColors.danger, fontSize: 11),
            ),
          ),
        Text(
          entry.samplingServer.isEmpty ? 'MCP' : entry.samplingServer,
          style: TextStyle(
            color: LunacColors.text,
            fontSize: 12,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 2),
        Text(
          '消息 ${entry.samplingMessages} 条 · 最多 ${entry.samplingMaxTokens} tokens',
          style: TextStyle(color: LunacColors.textDim, fontSize: 11),
        ),
        Padding(
          padding: EdgeInsets.only(top: 4),
          child: Text(
            '允许后本会话内不再询问；重启后需重新确认。',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        ),
        Row(
          children: [
            TextButton(
              onPressed: () => onDecide(true),
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.accent,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('允许（本会话）'),
            ),
            TextButton(
              onPressed: () => onDecide(false),
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.danger,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('拒绝'),
            ),
          ],
        ),
      ],
    );
  }
}

/// 思考块（v1 `.think-block`）：跑的时候展开显示「思考中… (n 字)」，收尾后自动收起成
/// 「思考过程 (n 字)」，点开才看正文 —— 与 v1 的 `<details>` + `dataset.done` 同口径。
class _ThinkingBlock extends StatefulWidget {
  const _ThinkingBlock({required this.text, required this.streaming});

  final String text;

  /// 这一块还在流式增长（只有最后一条思考条目会为真）。
  final bool streaming;

  @override
  State<_ThinkingBlock> createState() => _ThinkingBlockState();
}

class _ThinkingBlockState extends State<_ThinkingBlock> {
  /// null = 跟随 [widget.streaming]（运行中展开、收尾后收起）；用户点过就固定住。
  bool? _manualOpen;

  bool get _open => _manualOpen ?? widget.streaming;

  @override
  Widget build(BuildContext context) {
    final count = widget.text.length;
    final label = widget.streaming ? '思考中… ($count 字)' : '思考过程 ($count 字)';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            borderRadius: BorderRadius.circular(6),
            onTap: () => setState(() => _manualOpen = !_open),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 3),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    Icons.psychology_outlined,
                    size: 14,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(width: 6),
                  Text(
                    label,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 12,
                    ),
                  ),
                  const SizedBox(width: 4),
                  Icon(
                    _open ? Icons.expand_less : Icons.expand_more,
                    size: 14,
                    color: LunacColors.textMuted,
                  ),
                ],
              ),
            ),
          ),
          if (_open)
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 2, 4, 4),
              child: Text(
                widget.text,
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 12,
                  height: 1.5,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// 批量命令审批卡（2026-11-14，用户要求）。
///
/// 形态：**一张**大卡片，顶部一行「N 个命令需要你的确认」+「全部允许 / 全部取消」；
/// 下面每行一个命令 —— 工具名 + 省略成一行的命令原文 + ✓ / ×。
/// 勾掉 / 叉掉某一条后，那一条从列表里消失（由外层过滤 `handled` 实现）。
class _ApprovalGroup extends StatelessWidget {
  const _ApprovalGroup({
    super.key,
    required this.entries,
    required this.onAllow,
    required this.onDeny,
    required this.onAllowAll,
    required this.onDenyAll,
  });

  final List<ChatEntry> entries;
  final ValueChanged<ChatEntry> onAllow;
  final ValueChanged<ChatEntry> onDeny;
  final VoidCallback onAllowAll;
  final VoidCallback onDenyAll;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 8),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 8),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.accent),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Icon(Icons.shield_outlined, size: 14, color: LunacColors.accent),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  '${entries.length} 个命令需要你的确认',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 4),
          Wrap(
            spacing: 2,
            alignment: WrapAlignment.end,
            children: [
              TextButton(
                onPressed: onAllowAll,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.accent,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('全部允许', style: TextStyle(fontSize: 12)),
              ),
              TextButton(
                onPressed: onDenyAll,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.danger,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('全部取消', style: TextStyle(fontSize: 12)),
              ),
            ],
          ),
          Divider(height: 10, color: LunacColors.divider),
          for (final entry in entries) _buildRow(entry),
        ],
      ),
    );
  }

  Widget _buildRow(ChatEntry entry) {
    final name = switch (entry.toolName) {
      'Cmd' => '命令',
      final other when other != null && other.isNotEmpty => other,
      _ => '工具',
    };
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 66,
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11.5),
            ),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Tooltip(
              message: entry.detail ?? '',
              child: Text(
                _approvalSummary(entry),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11.5),
              ),
            ),
          ),
          IconButton(
            tooltip: '允许',
            icon: const Icon(Icons.check, size: 16),
            color: LunacColors.accent,
            visualDensity: VisualDensity.compact,
            constraints: const BoxConstraints(minWidth: 30, minHeight: 28),
            padding: EdgeInsets.zero,
            onPressed: () => onAllow(entry),
          ),
          IconButton(
            tooltip: '取消',
            icon: const Icon(Icons.close, size: 16),
            color: LunacColors.danger,
            visualDensity: VisualDensity.compact,
            constraints: const BoxConstraints(minWidth: 30, minHeight: 28),
            padding: EdgeInsets.zero,
            onPressed: () => onDeny(entry),
          ),
        ],
      ),
    );
  }
}

/// 审批行的一行摘要：命令类取 `command`，其余取第一个非空字符串值，最后压成一行。
String _approvalSummary(ChatEntry entry) {
  final detail = (entry.detail ?? '').trim();
  if (detail.isEmpty) {
    return '';
  }
  String oneLine(String text) => text.trim().replaceAll(RegExp(r'\s+'), ' ');
  try {
    final value = jsonDecode(detail);
    if (value is Map) {
      final command = value['command'];
      if (command is String && command.trim().isNotEmpty) {
        return oneLine(command);
      }
      for (final item in value.values) {
        if (item is String && item.trim().isNotEmpty) {
          return oneLine(item);
        }
      }
    }
  } catch (_) {
    // 不是 JSON：原样压成一行。
  }
  return oneLine(detail);
}

/// 命令组（v1 `details.cmd-group`）：一段连续工具调用的折叠容器。
///
/// 折叠态只留组头一行「已执行 N 条命令 · M 次失败」；运行中默认展开，回合结束后
/// 自动收起（v1 的 `.flow-folded` 作用在组头上，而不是把工具卡整块藏掉）。
class _ToolGroup extends StatefulWidget {
  const _ToolGroup({
    super.key,
    required this.tools,
    required this.results,
    required this.busy,
    this.foldTiming = 'reply',
    this.userTurns = 0,
  });

  final List<ChatEntry> tools;
  final Map<String, ChatEntry> results;

  /// 当前回合是否还在跑（回合结束后自动收起）。
  final bool busy;

  /// `reply` / `nextMessage`（§待办 3-28.17：折叠时机）。
  final String foldTiming;

  /// 对话流里用户条目的数量 —— 「下一条用户消息到达」这个时机的判据。
  final int userTurns;

  @override
  State<_ToolGroup> createState() => _ToolGroupState();
}

class _ToolGroupState extends State<_ToolGroup> {
  late bool _open = _autoOpen;

  /// 用户手动点过之后就尊重用户的选择，不再自动开合。
  bool _manual = false;

  ChatEntry? _resultOf(ChatEntry tool) {
    final id = tool.toolUseId;
    return id == null ? null : widget.results[id];
  }

  bool get _anyRunning =>
      widget.tools.any((tool) => _resultOf(tool) == null) && widget.busy;

  bool get _autoOpen => widget.busy || _anyRunning;

  @override
  void didUpdateWidget(covariant _ToolGroup oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_manual && !oldWidget.busy && widget.busy) {
      _open = true; // 新的一轮开始：进行中摊开
    } else if (!_manual && oldWidget.busy && !widget.busy) {
      // 回合结束：`reply` 档立刻收起；`nextMessage` 档先留着（等下一条消息到达）。
      _open = widget.foldTiming == 'nextMessage';
    }
    // 「下一条用户消息到达」：那一刻收起（新消息同时会让 busy 变真、上面先把 _open 置真，
    // 所以这条判断必须放在它之后 —— 折叠时机优先于「进行中摊开」）。
    if (!_manual &&
        widget.foldTiming == 'nextMessage' &&
        widget.userTurns > oldWidget.userTurns) {
      _open = false;
    }
  }

  bool _failed(ChatEntry tool) => (_resultOf(tool)?.text ?? '').startsWith('⚠');

  /// 本组里**文件类工具**改过的文件（完整路径，按出现顺序去重）。
  ///
  /// 只认 `Write` / `Edit` / `NotebookEdit` 的精确路径；命令类不冒充（见
  /// `chat_model.dart` 的 `writeToolPathField`）。用户要的就是「这一轮到底动了
  /// 哪些文件」，猜错比不显示更糟。
  List<String> get _changedFiles => changedFilesOf(widget.tools);

  /// 组头末尾那一段文件文案：一个文件写名字，多个写「已修改 N 个文件」。
  String get _filePart => changedFilesLabel(_changedFiles);

  String get _label {
    final n = widget.tools.length;
    final other = widget.tools
        .where(
          (tool) => tool.toolName != 'Cmd' && tool.toolName != 'PowerShell',
        )
        .length;
    final fails = widget.tools.where(_failed).length;
    final base = other == 0 ? '已执行 $n 条命令' : '已执行 $n 个工具调用';
    final head = fails > 0 ? '$base · $fails 次失败' : base;
    final files = _filePart;
    return files.isEmpty ? head : '$head · $files';
  }

  @override
  Widget build(BuildContext context) {
    final changed = _changedFiles;
    final label = Text(
      _label,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyle(color: LunacColors.textDim, fontSize: 11),
    );
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 3),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InkWell(
            borderRadius: BorderRadius.circular(6),
            onTap: () => setState(() {
              _open = !_open;
              _manual = true;
            }),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
              decoration: BoxDecoration(
                color: LunacColors.surfaceHover.withValues(alpha: 0.14),
                borderRadius: BorderRadius.circular(6),
              ),
              child: Row(
                children: [
                  Icon(
                    _open ? Icons.expand_more : Icons.chevron_right,
                    size: 12,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(width: 4),
                  Icon(Icons.terminal, size: 12, color: LunacColors.textMuted),
                  const SizedBox(width: 6),
                  Expanded(
                    // 文件多的组只显示「已修改 N 个文件」；完整路径放 tooltip。
                    child: changed.isEmpty
                        ? label
                        : Tooltip(
                            message: changed.join('\n'),
                            waitDuration: const Duration(milliseconds: 400),
                            child: label,
                          ),
                  ),
                ],
              ),
            ),
          ),
          if (_open)
            Container(
              margin: const EdgeInsets.only(top: 3),
              padding: const EdgeInsets.only(left: 8),
              decoration: BoxDecoration(
                border: Border(
                  left: BorderSide(
                    color: LunacColors.text.withValues(alpha: 0.10),
                    width: 2,
                  ),
                ),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final tool in widget.tools)
                    _ToolCard(
                      name: tool.toolName ?? '工具',
                      command: tool.detail ?? '',
                      output: _resultOf(tool)?.text ?? '',
                      running: _resultOf(tool) == null && widget.busy,
                      toolUseId: tool.toolUseId,
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// 工具类别 → 中文（回合信息条的六类，顺序见 `toolCategoryOrder`）。
const Map<String, String> _kCategoryLabels = {
  'create': '创建',
  'edit': '编辑',
  'read': '读取',
  'search': '搜索',
  'web': '网页',
  'command': '命令',
};

/// 每类的计数文案（v1 `turnMetaText` 的「N 个文件 / N 条命令」）。
String _categoryCountText(String category, int count) {
  switch (category) {
    case 'command':
      return '执行 $count 条命令';
    case 'web':
      return '抓取 $count 个网页';
    case 'search':
      // Grep / Glob 是**次数**，不是命中的文件数 —— 不编造文件数（v1 口径的诚实降级）。
      return '搜索 $count 次';
    default:
      return '${_kCategoryLabels[category] ?? category} $count 个文件';
  }
}

/// 耗时文案：不到 1 秒给毫秒（短回合显示「0.0 秒」没有意义）。
String _durationText(int ms) {
  if (ms < 1000) {
    return '$ms 毫秒';
  }
  return '${(ms / 1000).toStringAsFixed(1)} 秒';
}

/// 全对话统计（对话顶端常驻条用）。
class _SessionStats {
  const _SessionStats({
    required this.durationMs,
    required this.fails,
    required this.created,
    required this.edited,
  });

  final int? durationMs;
  final int fails;
  final int created;
  final int edited;
}

/// 「待办任务大抽屉」（用户 2026-11-20 口径）。
///
/// 一个任务段把**思考过程 / 工具调用 / 命令执行 / 审批**收进同一个可折叠容器：
///   · 默认折叠（思考进行中也不自动摊开，避免整页被过程刷屏）；
///   · 有待审批的命令时强制摊开 —— 用户必须看得到确认入口；
///   · 标题比正文大一号（正文 13 ⇒ 这里 14），点标题开合。
class _TaskDrawer extends StatefulWidget {
  const _TaskDrawer({
    super.key,
    required this.title,
    required this.thinkingChars,
    required this.tools,
    required this.results,
    required this.busy,
    required this.pendingApprovals,
    required this.children,
  });

  /// 任务标题：该任务里主要内容的首行概括（不再用「任务 1 / 2」编号）。
  final String title;

  /// 本抽屉里的思考字数（标题摘要用）。
  final int thinkingChars;

  /// 本抽屉里的工具调用（与 `children` 同源，只用于统计）。
  final List<ChatEntry> tools;
  final Map<String, ChatEntry> results;

  /// 当前回合是否还在跑（标题右侧的小进度点）。
  final bool busy;

  /// 待审批指令条数：> 0 时强制摊开。
  final int pendingApprovals;

  /// 抽屉内容（思考块 / 命令组 / 审批卡 / 产出物）。
  final List<Widget> children;

  @override
  State<_TaskDrawer> createState() => _TaskDrawerState();
}

class _TaskDrawerState extends State<_TaskDrawer> {
  /// null = 跟随默认（折叠）；用户点过就固定住。
  bool? _manualOpen;

  bool get _open => widget.pendingApprovals > 0 || (_manualOpen ?? false);

  int get _fails => widget.tools.where((tool) {
    final id = tool.toolUseId;
    return id != null && (widget.results[id]?.text ?? '').startsWith('⚠');
  }).length;

  /// 只显示内容概括；空内容（只有过程块）才回落到「过程记录」。
  String get _label =>
      widget.title.trim().isEmpty ? '过程记录' : widget.title.trim();

  /// 标题摘要：思考字数 + 六类计数 + 失败 / 待审批（与 `_categoryCountText` 同口径）。
  String get _summary {
    final parts = <String>[];
    if (widget.thinkingChars > 0) {
      parts.add('思考 ${widget.thinkingChars} 字');
    }
    final groups = groupToolsByCategory(widget.tools);
    for (final category in toolCategoryOrder) {
      final items = groups[category];
      if (items == null || items.isEmpty) {
        continue;
      }
      parts.add(_categoryCountText(category, items.length));
    }
    if (_fails > 0) {
      parts.add('$_fails 次失败');
    }
    if (widget.pendingApprovals > 0) {
      parts.add('${widget.pendingApprovals} 个待审批');
    }
    return parts.join(' · ');
  }

  @override
  Widget build(BuildContext context) {
    final summary = _summary;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: () => setState(() => _manualOpen = !_open),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
              decoration: BoxDecoration(
                color: LunacColors.surfaceHover.withValues(alpha: 0.12),
                border: Border.all(color: LunacColors.divider),
                borderRadius: BorderRadius.circular(8),
              ),
              child: Row(
                children: [
                  Icon(
                    _open ? Icons.expand_more : Icons.chevron_right,
                    size: 14,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(width: 4),
                  Icon(
                    Icons.checklist_rtl,
                    size: 14,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(width: 6),
                  Text(
                    _label,
                    style: TextStyle(
                      color: LunacColors.text,
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  if (summary.isEmpty)
                    const Spacer()
                  else ...[
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        summary,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.textDim,
                          fontSize: 11.5,
                        ),
                      ),
                    ),
                  ],
                  if (widget.busy) ...[
                    const SizedBox(width: 6),
                    SizedBox(
                      width: 10,
                      height: 10,
                      child: CircularProgressIndicator(
                        strokeWidth: 1.4,
                        color: LunacColors.accent,
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
          if (_open)
            Container(
              margin: const EdgeInsets.only(top: 4),
              padding: const EdgeInsets.only(left: 8),
              decoration: BoxDecoration(
                border: Border(
                  left: BorderSide(
                    color: LunacColors.text.withValues(alpha: 0.10),
                    width: 2,
                  ),
                ),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: widget.children,
              ),
            ),
        ],
      ),
    );
  }
}

/// 本轮产出物块（§待办 3-28.13）。
///
/// 2026-11-20：耗时 / 失败 / 六类计数搬到对话顶端统计条与任务抽屉头，这里只剩
/// 「这一轮落盘了什么」：文件名（点开 diff）+ `+N −M` + 还原 / 打开。
/// 数据与「改动文件」抽屉同源（同一份 `readChangeDiff` 结果）。
class _ArtifactsBlock extends StatefulWidget {
  const _ArtifactsBlock({
    super.key,
    required this.paths,
    this.diffStats = const {},
    this.onOpenFile,
  });

  /// 本轮写类工具（create / edit）改过的完整路径（`changedFilesOf` 去重后）。
  final List<String> paths;

  /// diff 摘要：键 = 完整路径，值 = `readChangeDiff` 结果；`ok=false` ⇒ 不编数字。
  final Map<String, ChangeDiff?> diffStats;

  /// 点文件名 → 在预览区打开 diff（对话流里不内联渲染 diff，用户 2026-11-14 口径）。
  final void Function(String path)? onOpenFile;

  @override
  State<_ArtifactsBlock> createState() => _ArtifactsBlockState();
}

class _ArtifactsBlockState extends State<_ArtifactsBlock> {
  /// 产出物：标题行只给总数与 diff 汇总，文件本身直接铺成可点击卡片。
  ///
  /// 用户 2026-11-21 口径：不再用「产出物 N 个文件 ▸」折一层，点文件卡片就是
  /// 在预览区打开 diff；拿不到 diff 的文件也保留卡片并标「无快照 / 二进制」。
  Widget _buildArtifacts(
    List<String> paths,
    int added,
    int removed,
    int measured,
  ) {
    final base = TextStyle(color: LunacColors.textDim, fontSize: 11);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(top: 2, left: 2),
          child: Text(
            '产出物 ${paths.length} 个文件'
            '${measured > 0 ? ' · +$added −$removed' : ''}',
            style: base,
          ),
        ),
        const SizedBox(height: 4),
        Wrap(
          spacing: 6,
          runSpacing: 4,
          children: [for (final path in paths) _buildArtifactChip(path)],
        ),
      ],
    );
  }

  /// 一张文件卡片：图标 + 文件名 + diff 统计；点击在预览区打开。
  Widget _buildArtifactChip(String path) {
    final stat = widget.diffStats[path];
    final hasStat = stat != null && stat.ok;
    return Tooltip(
      message: path,
      waitDuration: const Duration(milliseconds: 400),
      child: InkWell(
        borderRadius: BorderRadius.circular(6),
        onTap: widget.onOpenFile == null
            ? null
            : () => widget.onOpenFile!(path),
        child: Container(
          constraints: const BoxConstraints(maxWidth: 260),
          padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 4),
          decoration: BoxDecoration(
            color: LunacColors.surfaceHover.withValues(alpha: 0.55),
            border: Border.all(color: LunacColors.border),
            borderRadius: BorderRadius.circular(6),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                Icons.description_outlined,
                size: 12,
                color: LunacColors.textMuted,
              ),
              const SizedBox(width: 5),
              Flexible(
                child: Text(
                  fileBaseName(path),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.text, fontSize: 11),
                ),
              ),
              const SizedBox(width: 5),
              Text(
                hasStat
                    ? '+${stat.added} −${stat.removed}'
                    : (stat?.binary == true ? '二进制' : '无快照'),
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
              ),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    var added = 0;
    var removed = 0;
    var measured = 0;
    for (final path in widget.paths) {
      final stat = widget.diffStats[path];
      if (stat == null || !stat.ok) {
        continue;
      }
      added += stat.added;
      removed += stat.removed;
      measured++;
    }
    return Padding(
      padding: const EdgeInsets.only(left: 2, right: 8, top: 2, bottom: 4),
      child: _buildArtifacts(widget.paths, added, removed, measured),
    );
  }
}

/// 入队一条 URL 预览标签；返回 null = 入队成功，否则是失败原因。
String? _pushUrlPreview(String url) {
  final uri = Uri.tryParse(url);
  final label = uri == null
      ? '预览: URL'
      : (uri.path.isEmpty ? '预览: ${uri.host}' : '预览: ${uri.host}${uri.path}');
  return pushPreviewTab({'title': label, 'kind': 'url', 'target': url});
}

/// 工具输出里的 URL：入队后把预览窗打开；失败不打扰对话流。
void _openUrlFromTool(String url) {
  if (_pushUrlPreview(url) == null) {
    unawaited(openSandboxWindow());
  }
}

/// 工具卡（v1 `.tool-card`）：summary = 图标 + 工具名 + 入参 + 状态（执行中 / 成功 /
/// ✗ 工具失败: …）；展开才看完整入参与输出。结果按 `tool_use_id` 并进同一张卡。
class _ToolCard extends StatefulWidget {
  const _ToolCard({
    required this.name,
    required this.command,
    required this.output,
    required this.running,
    this.toolUseId,
  });

  final String name;

  /// 工具入参（v2 用紧凑 JSON 代替 v1 的可读参数行）。
  final String command;

  /// 工具输出；错误输出以 `⚠ ` 开头（见 `chat_model.dart` 的 tool_result 处理）。
  final String output;

  final bool running;

  /// 命令类工具（Cmd / PowerShell）的 `tool_use_id`：运行中可下「后台运行 / 停止」指令。
  final String? toolUseId;

  @override
  State<_ToolCard> createState() => _ToolCardState();
}

class _ToolCardState extends State<_ToolCard> {
  bool _open = false;

  /// 已点过「后台运行 / 停止」：这两个动作不可逆，点完就换成状态文字（v1 同口径）。
  bool _background = false;
  bool _stopped = false;

  /// 终端（v1 xterm 卡的等价物）：可聚焦、逐键把字符经 `tool_control` 灌进子进程 stdin，
  /// 运行中在输出尾部画一个闪烁光标块；输出增长时自动滚到底（用户往上翻时不打断）。
  final FocusNode _termFocus = FocusNode(debugLabel: 'tool-terminal');
  final ScrollController _termScroll = ScrollController();
  Timer? _blink;
  bool _cursorOn = true;
  bool _termFocused = false;

  /// 鼠标按住期间的输出快照（v1 xterm「选中时暂停输出」）。
  String? _frozenOutput;

  static const TextStyle _termBase = TextStyle(
    fontSize: 11,
    height: 1.4,
    fontFamily: 'Consolas',
    fontFamilyFallback: ['Courier New', 'monospace'],
  );

  @override
  void initState() {
    super.initState();
    if (widget.running) {
      _startBlink();
    }
  }

  @override
  void didUpdateWidget(covariant _ToolCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.running) {
      _startBlink();
    } else {
      _stopBlink();
    }
    if (oldWidget.output != widget.output) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _scrollToBottom());
    }
  }

  @override
  void dispose() {
    _stopBlink();
    _termFocus.dispose();
    _termScroll.dispose();
    super.dispose();
  }

  void _startBlink() {
    if (_blink != null) {
      return;
    }
    _blink = Timer.periodic(const Duration(milliseconds: 530), (_) {
      if (mounted) {
        setState(() => _cursorOn = !_cursorOn);
      }
    });
  }

  void _stopBlink() {
    _blink?.cancel();
    _blink = null;
    if (!_cursorOn) {
      _cursorOn = true;
    }
  }

  void _scrollToBottom() {
    if (_frozenOutput != null) {
      // 用户正在拖选：不把视图拽走（v1 xterm 同口径）。
      return;
    }
    if (!_termScroll.hasClients) {
      return;
    }
    final position = _termScroll.position;
    // 只在「本来就贴着底部」时跟着滚，避免用户向上翻看时被拽回。
    if (position.maxScrollExtent - position.pixels < 40) {
      position.jumpTo(position.maxScrollExtent);
    }
  }

  /// 按下 = 冻结当前输出（之后的新输出不再替换文本，选区不会被打断）。
  void _freezeTerminal() {
    if (_frozenOutput != null) {
      return;
    }
    setState(() => _frozenOutput = widget.output);
  }

  /// 松开 = 恢复实时输出，并把视图贴回底部（若本来就贴着底）。
  void _resumeTerminal() {
    if (_frozenOutput == null) {
      return;
    }
    setState(() => _frozenOutput = null);
    WidgetsBinding.instance.addPostFrameCallback((_) => _scrollToBottom());
  }

  bool get _failed => widget.output.startsWith('⚠');

  /// 失败三要素（§待办 3-28.4）：认不出具体类别时也给通用三段（工具卡必须可行动）。
  FailureHint? get _hint => _failed
      ? failureHintFor(toolName: widget.name, text: widget.output)
      : null;

  /// 三要素的展示形态：标签加粗、值同色，收起状态下也常显（不用展开才看得到）。
  Widget _failureHintBlock(FailureHint hint) => Padding(
    padding: const EdgeInsets.fromLTRB(10, 0, 10, 7),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: _failureLines(hint),
    ),
  );

  /// 只有命令类工具才有「跑一半可以打断」这回事（agent 侧同判据）。
  bool get _shell => widget.name == 'Cmd' || widget.name == 'PowerShell';

  bool get _showLive => widget.running && _shell && widget.toolUseId != null;

  /// 运行中的命令卡就是 v1 的 xterm：能键入。
  bool get _interactive => widget.running && _shell && widget.toolUseId != null;

  void _sendControl(String action) {
    final id = widget.toolUseId;
    if (id == null) {
      return;
    }
    sendAgentLine(jsonEncode(toolControlPayload(id, action)));
    setState(() {
      if (action == 'background') {
        _background = true;
      } else if (action == 'stop') {
        _stopped = true;
      }
    });
  }

  /// 逐键输入（v1 xterm 的 `onData`）：键位映射见 `platform/agent.dart` 的
  /// `terminalKeyData`（纯函数，单测覆盖）。
  KeyEventResult _onTermKey(FocusNode node, KeyEvent event) {
    final id = widget.toolUseId;
    if (!_interactive || id == null || event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    final data = terminalKeyData(
      key: event.logicalKey,
      character: event.character,
    );
    if (data == null) {
      return KeyEventResult.ignored;
    }
    sendAgentLine(jsonEncode(toolControlPayload(id, 'stdin', data: data)));
    return KeyEventResult.handled;
  }

  String get _stateText {
    if (widget.running) {
      if (_stopped) {
        return '已停止';
      }
      if (_background) {
        return '已在后台运行';
      }
      return '执行中';
    }
    if (!_failed) {
      return '成功';
    }
    // 失败时**不把原始错误首行贴进标题**（状态码 / JSON 体，`forbidden.md` §禁用 59）：
    // 具体原因与下一步由卡里的三要素给，原文留在展开后的输出里。
    return '✗ 工具失败';
  }

  @override
  Widget build(BuildContext context) {
    final stateColor = _failed
        ? LunacColors.danger
        : (widget.running && _stopped
              ? LunacColors.warning
              : LunacColors.textMuted);
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 3),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(
          color: _failed ? LunacColors.danger : LunacColors.border,
        ),
        borderRadius: BorderRadius.circular(8),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InkWell(
            onTap: () => setState(() => _open = !_open),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
              child: Row(
                children: [
                  Icon(Icons.terminal, size: 13, color: LunacColors.textMuted),
                  const SizedBox(width: 6),
                  Text(
                    widget.name,
                    style: TextStyle(color: LunacColors.text, fontSize: 12),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      widget.command,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 11,
                      ),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    _stateText,
                    style: TextStyle(color: stateColor, fontSize: 11),
                  ),
                  const SizedBox(width: 2),
                  Icon(
                    _open ? Icons.expand_less : Icons.expand_more,
                    size: 14,
                    color: LunacColors.textMuted,
                  ),
                ],
              ),
            ),
          ),
          // 失败三要素（§待办 3-28.4）：收起时也常显，展开只是看原始输出。
          if (_hint != null) _failureHintBlock(_hint!),
          // v1 `.tool-live`：只在命令还在跑时给「后台运行 / 停止」，点完换成状态文字。
          if (_showLive)
            Padding(
              padding: const EdgeInsets.fromLTRB(10, 0, 10, 7),
              child: _background || _stopped
                  ? Text(
                      _background ? '已在后台运行' : '已停止',
                      style: TextStyle(
                        color: _stopped
                            ? LunacColors.warning
                            : LunacColors.textMuted,
                        fontSize: 10.5,
                      ),
                    )
                  : Row(
                      children: [
                        _ToolLiveButton(
                          label: '后台运行',
                          onTap: () => _sendControl('background'),
                        ),
                        const SizedBox(width: 6),
                        _ToolLiveButton(
                          label: '停止',
                          danger: true,
                          onTap: () => _sendControl('stop'),
                        ),
                      ],
                    ),
            ),
          if (_open)
            Padding(
              padding: const EdgeInsets.fromLTRB(10, 0, 10, 8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (widget.command.isNotEmpty)
                    _mono(widget.command, LunacColors.textDim),
                  if (widget.output.isNotEmpty) ...[
                    const SizedBox(height: 6),
                    // 命令类工具 = v1 的 xterm 卡（深色底 + 等宽）；其它工具仍是普通文本。
                    if (_shell)
                      _terminal(widget.output)
                    else
                      ConstrainedBox(
                        constraints: const BoxConstraints(maxHeight: 220),
                        child: SingleChildScrollView(
                          child: _mono(widget.output, LunacColors.textMuted),
                        ),
                      ),
                  ],
                  // v1 xterm 可键入（经 `tool_control` 的 `stdin`）；键入区就在终端里，
                  // 点击输出区后直接敲键盘，Enter 发送（不再有单独的输入行）。
                ],
              ),
            ),
        ],
      ),
    );
  }

  /// 命令输出区（v1 xterm 卡的视觉等价物：`#0b0b0c` + 等宽 + ANSI 着色）。
  ///
  /// 运行中：可聚焦、逐键 stdin、尾部闪烁光标块、输出增长自动滚到底；
  /// 结束后：可选中复制（`SelectableText`）。
  ///
  /// **鼠标按住时暂停输出**（v1 xterm 的选区行为）：按下瞬间冻结当前文本，松开恢复 ——
  /// 否则新输出会把正在拖选的文字换掉。
  Widget _terminal(String output) {
    final interactive = _interactive;
    final shown = _frozenOutput ?? output;
    final root = ansiSpansToTextSpan(parseAnsi(shown), base: _termBase);
    final Widget body = interactive
        ? Text.rich(
            TextSpan(
              children: [
                root,
                TextSpan(
                  text: _cursorOn ? '▍' : ' ',
                  style: TextStyle(color: LunacColors.accent),
                ),
              ],
            ),
          )
        : SelectableText.rich(root);
    final terminal = Container(
      decoration: BoxDecoration(
        color: LunacColors.terminal,
        border: Border.all(
          color: _termFocused ? LunacColors.accent : LunacColors.border,
        ),
        borderRadius: BorderRadius.circular(6),
      ),
      padding: const EdgeInsets.all(8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 220),
            child: Listener(
              onPointerDown: (_) => _freezeTerminal(),
              onPointerUp: (_) => _resumeTerminal(),
              onPointerCancel: (_) => _resumeTerminal(),
              child: SingleChildScrollView(
                controller: _termScroll,
                child: Align(alignment: Alignment.topLeft, child: body),
              ),
            ),
          ),
          if (_frozenOutput != null)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                '输出已暂停（松开鼠标继续）',
                style: TextStyle(color: LunacColors.warning, fontSize: 9.5),
              ),
            ),
          if (interactive)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                _termFocused ? '正在输入…（Enter 发送）' : '点击这里后直接键入（Enter 发送）',
                style: TextStyle(
                  color: _termFocused
                      ? LunacColors.accent
                      : LunacColors.textMuted,
                  fontSize: 9.5,
                ),
              ),
            ),
        ],
      ),
    );
    if (!interactive) {
      return terminal;
    }
    return Focus(
      focusNode: _termFocus,
      onFocusChange: (focused) => setState(() => _termFocused = focused),
      onKeyEvent: _onTermKey,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => _termFocus.requestFocus(),
        child: terminal,
      ),
    );
  }

  Widget _mono(String text, Color color) {
    return LinkifiedText(
      text,
      style: TextStyle(
        color: color,
        fontSize: 11,
        height: 1.45,
        fontFamily: 'Consolas',
        fontFamilyFallback: const ['Courier New', 'monospace'],
      ),
      onOpenUrl: _openUrlFromTool,
    );
  }
}

/// v1 `.tool-live-btn`：工具卡里的小指令按钮（后台运行 / 停止）。
class _ToolLiveButton extends StatelessWidget {
  const _ToolLiveButton({
    required this.label,
    required this.onTap,
    this.danger = false,
  });

  final String label;
  final VoidCallback onTap;
  final bool danger;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      borderRadius: BorderRadius.circular(5),
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        decoration: BoxDecoration(
          color: LunacColors.surfaceHover.withValues(alpha: 0.5),
          border: Border.all(
            color: danger
                ? LunacColors.danger.withValues(alpha: 0.5)
                : LunacColors.border,
          ),
          borderRadius: BorderRadius.circular(5),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: danger ? LunacColors.danger : LunacColors.textDim,
            fontSize: 10.5,
          ),
        ),
      ),
    );
  }
}

/// v1 `.chat-more-actions button`：细描边小按钮（设置 / 打开目录 / 保存并重启）。
class _MoreTextButton extends StatelessWidget {
  const _MoreTextButton({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      borderRadius: BorderRadius.circular(5),
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
        decoration: BoxDecoration(
          color: LunacColors.surfaceHover.withValues(alpha: 0.6),
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(5),
        ),
        child: Text(
          label,
          style: TextStyle(color: LunacColors.textDim, fontSize: 11),
        ),
      ),
    );
  }
}

/// v1 `.settings-toggle`：36×20 轨道 + 14px 圆点（对话「更多设置」里的思考开关）。
class _MoreToggle extends StatelessWidget {
  const _MoreToggle({required this.value, required this.onChanged});

  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => onChanged(!value),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        width: 36,
        height: 20,
        padding: const EdgeInsets.all(3),
        decoration: BoxDecoration(
          color: value
              ? LunacColors.accent.withValues(alpha: 0.35)
              : LunacColors.surfaceHover,
          border: Border.all(
            color: value ? LunacColors.accent : LunacColors.border,
          ),
          borderRadius: BorderRadius.circular(10),
        ),
        child: Align(
          alignment: value ? Alignment.centerRight : Alignment.centerLeft,
          child: Container(
            width: 14,
            height: 14,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: value ? LunacColors.accent : LunacColors.textMuted,
            ),
          ),
        ),
      ),
    );
  }
}

/// 用户气泡（v1 `.chat-msg-user`）：左留 1/6 空档、其余满宽；悬浮时右下角出现复制。
/// 输入框控制器：把 `[[file:…]]` 渲染成可见胶囊（§待办 3-28.18）。
///
/// 内部文本仍是占位符（插入 / 删除 / 移动都按普通文字处理），只改**显示**。
/// IME 组字区间存在时退回默认实现，避免中文输入被拆坏。
class AttachmentTextEditingController extends TextEditingController {
  AttachmentTextEditingController({super.text});

  static final RegExp _pattern = RegExp(r'\[\[file:([^\]]+)\]\]');

  @override
  TextSpan buildTextSpan({
    required BuildContext context,
    TextStyle? style,
    required bool withComposing,
  }) {
    if (!value.isComposingRangeValid || !withComposing) {
      final spans = <InlineSpan>[];
      var cursor = 0;
      for (final match in _pattern.allMatches(text)) {
        if (match.start > cursor) {
          spans.add(TextSpan(text: text.substring(cursor, match.start)));
        }
        spans.add(
          TextSpan(
            text: ' ${attachmentName(match.group(1)!)} ',
            style: TextStyle(
              color: LunacColors.accent,
              backgroundColor: LunacColors.accent.withValues(alpha: 0.14),
              fontWeight: FontWeight.w600,
            ),
          ),
        );
        cursor = match.end;
      }
      if (cursor < text.length) {
        spans.add(TextSpan(text: text.substring(cursor)));
      }
      return TextSpan(style: style, children: spans);
    }
    return super.buildTextSpan(
      context: context,
      style: style,
      withComposing: withComposing,
    );
  }
}

/// 用户气泡正文：`[Attached file] "path"` 在**原位**渲染成文件胶囊。
class _AttachmentAwareText extends StatelessWidget {
  const _AttachmentAwareText({required this.text, required this.style});

  final String text;
  final TextStyle style;

  @override
  Widget build(BuildContext context) {
    final segments = splitAttachments(text);
    if (segments.every((segment) => !segment.isFile)) {
      return Text(text, style: style);
    }
    final spans = <InlineSpan>[];
    for (final segment in segments) {
      if (!segment.isFile) {
        if (segment.text.isNotEmpty) {
          spans.add(TextSpan(text: segment.text));
        }
        continue;
      }
      final path = segment.path!;
      spans.add(
        WidgetSpan(
          alignment: PlaceholderAlignment.middle,
          child: Tooltip(
            message: path,
            child: Container(
              margin: const EdgeInsets.symmetric(horizontal: 2),
              padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
              decoration: BoxDecoration(
                color: LunacColors.accent.withValues(alpha: 0.14),
                borderRadius: BorderRadius.circular(999),
                border: Border.all(
                  color: LunacColors.accent.withValues(alpha: 0.25),
                ),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    Icons.insert_drive_file_outlined,
                    size: 11,
                    color: LunacColors.accent,
                  ),
                  const SizedBox(width: 3),
                  Text(
                    attachmentName(path),
                    style: TextStyle(
                      color: LunacColors.accent,
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
    }
    return Text.rich(TextSpan(style: style, children: spans));
  }
}

class _UserBubble extends StatefulWidget {
  const _UserBubble({
    required this.text,
    this.onRewind,
    this.onRetry,
    this.armed = false,
  });

  final String text;

  /// 「回退到此处」：第一次点只上膛（`armed=true` 显示确认），第二次才执行。
  final VoidCallback? onRewind;

  /// 「重试」：回退到这条提问并原样重发。
  final VoidCallback? onRetry;

  /// 回退已上膛：动作条常驻并显示红色「确认回退」。
  final bool armed;

  @override
  State<_UserBubble> createState() => _UserBubbleState();
}

class _UserBubbleState extends State<_UserBubble> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    return FractionallySizedBox(
      widthFactor: 5 / 6,
      alignment: Alignment.centerRight,
      child: MouseRegion(
        onEnter: (_) => setState(() => _hovering = true),
        onExit: (_) => setState(() => _hovering = false),
        child: Container(
          margin: const EdgeInsets.symmetric(vertical: 6),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          decoration: BoxDecoration(
            color: LunacColors.accent.withValues(alpha: 0.08),
            border: Border.all(
              color: LunacColors.accent.withValues(alpha: 0.15),
            ),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Stack(
            children: [
              _AttachmentAwareText(
                text: widget.text,
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 13,
                  height: 1.5,
                ),
              ),
              if (_hovering || widget.armed)
                Positioned(
                  right: 0,
                  bottom: 0,
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (widget.onRewind != null)
                        widget.armed
                            ? Tooltip(
                                message: '再点一次确认回退（对话与该轮之后改过的文件一起回退）',
                                child: InkWell(
                                  borderRadius: BorderRadius.circular(4),
                                  onTap: widget.onRewind,
                                  child: Container(
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 6,
                                      vertical: 2,
                                    ),
                                    decoration: BoxDecoration(
                                      border: Border.all(
                                        color: LunacColors.danger.withValues(
                                          alpha: 0.5,
                                        ),
                                      ),
                                      borderRadius: BorderRadius.circular(4),
                                    ),
                                    child: Text(
                                      '确认回退',
                                      style: TextStyle(
                                        color: LunacColors.danger,
                                        fontSize: 10,
                                      ),
                                    ),
                                  ),
                                ),
                              )
                            : _BubbleIconButton(
                                tooltip: '回退到此处（移除其后消息，仅回退对话）',
                                icon: Icons.undo,
                                onTap: widget.onRewind!,
                              ),
                      if (widget.onRetry != null)
                        _BubbleIconButton(
                          tooltip: '重试此对话并重新发送',
                          icon: Icons.refresh,
                          onTap: widget.onRetry!,
                        ),
                      _CopyButton(text: widget.text, size: 18),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 气泡动作条上的一枚小图标键（v1 `.msg-actions`）。
class _BubbleIconButton extends StatelessWidget {
  const _BubbleIconButton({
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
        borderRadius: BorderRadius.circular(4),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(3),
          child: Icon(icon, size: 14, color: LunacColors.textMuted),
        ),
      ),
    );
  }
}

/// 三要素的三行（工具卡与出错旁注**共用同一形态**，不写两份）。
List<Widget> _failureLines(FailureHint hint) {
  Widget line(String label, String text) => Padding(
    padding: const EdgeInsets.only(top: 2),
    child: RichText(
      text: TextSpan(
        style: TextStyle(color: LunacColors.danger, fontSize: 11, height: 1.45),
        children: [
          TextSpan(
            text: '$label：',
            style: const TextStyle(fontWeight: FontWeight.w600),
          ),
          TextSpan(text: text),
        ],
      ),
    ),
  );
  return [
    line('哪一步', hint.step),
    line('原因', hint.cause),
    line('下一步', hint.next),
  ];
}

/// 出错旁注（§待办 3-28.4 + `forbidden.md` §禁用 59）：
///
/// 能归类的失败显示三要素，**原始错误（状态码 / URL / JSON 体）默认收起** ——
/// 它只在本机可见，点开才显示；认不出类别时原样显示那句话（不硬套模板）。
class _ErrorReport extends StatefulWidget {
  const _ErrorReport({required this.text});

  final String text;

  @override
  State<_ErrorReport> createState() => _ErrorReportState();
}

class _ErrorReportState extends State<_ErrorReport> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final hint = failureHintFor(
      toolName: '',
      text: widget.text,
      genericFallback: false,
    );
    if (hint == null) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Text(
          widget.text,
          style: TextStyle(
            color: LunacColors.danger,
            fontSize: 12,
            height: 1.5,
          ),
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Container(
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: LunacColors.background,
          border: Border.all(color: LunacColors.danger),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            ..._failureLines(hint),
            const SizedBox(height: 6),
            InkWell(
              onTap: () => setState(() => _open = !_open),
              child: Text(
                _open ? '收起原始信息' : '查看原始信息（只在本机显示）',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
              ),
            ),
            if (_open)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: SelectableText(
                  widget.text,
                  style: TextStyle(
                    color: LunacColors.textDim,
                    fontSize: 11,
                    height: 1.4,
                    fontFamily: 'Consolas',
                    fontFamilyFallback: const ['Courier New', 'monospace'],
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// 上下文水位条（§待办 3-28.8）：常显「当前上下文占预算多少」。
///
/// 数值全部是**实测**：预算与两档阈值来自 `system/init`（agent 的
/// `max_context_tokens` / `ELIDE_RATIO` / `DROP_RATIO`），当前体积来自
/// `usage_delta` 的 `in + read + create`。不估算、不发额外请求。
///
/// `level == none`（老 agent 没上报预算 / 本会话还没发过请求）时**不占位** ——
/// 画一根没有数据的条比不画更糟。
class _ContextWaterBar extends StatelessWidget {
  const _ContextWaterBar({
    required this.level,
    required this.ratio,
    required this.tokens,
    required this.limit,
    required this.elideRatio,
    required this.dropRatio,
  });

  final ContextLevel level;
  final double? ratio;
  final int tokens;
  final int limit;
  final double elideRatio;
  final double dropRatio;

  @override
  Widget build(BuildContext context) {
    if (level == ContextLevel.none || ratio == null) {
      return const SizedBox.shrink();
    }
    final percent = (ratio! * 100).floor();
    final color = switch (level) {
      ContextLevel.drop => LunacColors.danger,
      ContextLevel.elide => LunacColors.warning,
      _ => LunacColors.textMuted,
    };
    final hint = switch (level) {
      ContextLevel.drop =>
        '已过丢弃水位（${(dropRatio * 100).round()}%）：下一次请求会把旧消息换成摘要，前缀缓存整段作废',
      ContextLevel.elide =>
        '已过瘦身水位（${(elideRatio * 100).round()}%）：下一次请求可能压缩，会改写前缀缓存',
      ContextLevel.watch => '接近压缩水位：下一次可能压缩，会改写前缀缓存',
      _ => '离瘦身水位（${(elideRatio * 100).round()}%）还有余量',
    };
    final label = '上下文 $percent%';
    return Tooltip(
      message:
          '$hint\n'
          '实测 $tokens / $limit tokens（drop=${(dropRatio * 100).round()}% · '
          'elide=${(elideRatio * 100).round()}%）',
      waitDuration: const Duration(milliseconds: 400),
      child: Padding(
        padding: const EdgeInsets.only(left: 8),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(label, style: TextStyle(color: color, fontSize: 11)),
            const SizedBox(width: 6),
            // 40×4 的细条：宽度就是水位，不加过渡动画（每 500ms 一跳已经够平滑）。
            SizedBox(
              width: 40,
              height: 4,
              child: ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: LinearProgressIndicator(
                  value: ratio,
                  minHeight: 4,
                  backgroundColor: LunacColors.border,
                  valueColor: AlwaysStoppedAnimation<Color>(color),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 「从此分叉」图标按钮（§待办 3-28.6）：把到这条回答为止的正文复制成一条新会话。
///
/// 与「回退到此处」相邻但语义相反 —— 回退销毁后面的内容，分叉保留原会话。
class _ForkButton extends StatelessWidget {
  const _ForkButton({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return IconButton(
      tooltip: '从此分叉（原会话保持不变）',
      icon: const Icon(Icons.call_split, size: 13),
      color: LunacColors.textMuted,
      visualDensity: VisualDensity.compact,
      padding: EdgeInsets.zero,
      constraints: const BoxConstraints(minWidth: 22, minHeight: 22),
      onPressed: onTap,
    );
  }
}

/// 一个「复制」图标按钮（v1 `.msg-actions` / `.flow-actions` 的复制键）：
/// 点了之后 1.2 秒内换成对勾，避免「点了没反应」。
class _CopyButton extends StatefulWidget {
  const _CopyButton({required this.text, this.size = 22});

  final String text;
  final double size;

  @override
  State<_CopyButton> createState() => _CopyButtonState();
}

class _CopyButtonState extends State<_CopyButton> {
  bool _copied = false;
  bool _hovering = false;
  Timer? _timer;

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _copy() async {
    try {
      await Clipboard.setData(ClipboardData(text: widget.text));
    } catch (_) {
      return; // 无剪贴板（极少见）：静默，不弹假成功。
    }
    if (!mounted) {
      return;
    }
    setState(() => _copied = true);
    _timer?.cancel();
    _timer = Timer(const Duration(milliseconds: 1200), () {
      if (mounted) {
        setState(() => _copied = false);
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final alpha = _copied || _hovering ? 1.0 : 0.45;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        onTap: _copy,
        child: Tooltip(
          message: _copied ? '已复制' : '复制',
          child: SizedBox(
            width: widget.size,
            height: widget.size,
            child: Icon(
              _copied ? Icons.check : Icons.copy_rounded,
              size: 13,
              color: _copied
                  ? LunacColors.accent
                  : LunacColors.textDim.withValues(alpha: alpha),
            ),
          ),
        ),
      ),
    );
  }
}
