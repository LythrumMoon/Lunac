// M2 对话流的纯数据模型：把 stream-json 事件折叠成可渲染的条目。
//
// 为什么单独一层（不在 UI 里做）：
//   · 事件形状（增量 / 整包 / 工具输出 / 审批）只在协议一处定义，UI 只管画；
//   · 纯 Dart、不碰 FFI，单测可以直接喂事件断言折叠结果。
//
// 关键去重：`--include-partial-messages` 会先给 `stream_event` 增量、随后再给
// `assistant` 整包。文本 / 思考以整包为准（覆盖增量），工具调用只在整包里出现一次，
// 因此界面不会出现「同一段话说两遍」。
import 'dart:convert';

// ── 上下文水位（§待办 3-28.8）─────────────────────────────────────────
//
// 三档的**真实数值**只有 agent 一处定义（`core-agent/src/main.rs` 的
// `ELIDE_RATIO = 0.85` / `DROP_RATIO = 0.95`），经 `system/init` 下发给界面；
// 这里的两个 fallback 只在「agent 太旧、没上报」时兜底，且只在 `contextLimit > 0`
// 的前提下才会被用到（老 agent 连预算都不报 ⇒ 水位条根本不画）。
const double contextElideFallback = 0.85;
const double contextDropFallback = 0.95;

/// 预警档（纯界面语义，**不是**压缩触发点）：到这条线就先告诉用户
/// 「下一次可能压缩，会改写前缀缓存」，压缩本身仍只看 agent 的 0.85 / 0.95。
const double contextWatchRatio = 0.60;

/// 水位的四档（`none` = 没有预算或还没发过请求）。
enum ContextLevel { none, calm, watch, elide, drop }

/// 按实测水位定档。`ratio` 传 null = 无数据。
///
/// 顺序必须是 **drop → elide → watch**：阈值相等时（比如 agent 把两档配成同一个数）
/// 取更危险的那一档，宁可提前提示也不要「明明要丢了还显示平静」。
ContextLevel contextLevelOf(
  double? ratio, {
  double elideRatio = contextElideFallback,
  double dropRatio = contextDropFallback,
}) {
  if (ratio == null) {
    return ContextLevel.none;
  }
  if (ratio >= dropRatio) {
    return ContextLevel.drop;
  }
  if (ratio >= elideRatio) {
    return ContextLevel.elide;
  }
  if (ratio >= contextWatchRatio) {
    return ContextLevel.watch;
  }
  return ContextLevel.calm;
}

/// 失败三要素（§待办 3-28.4）：**哪一步没成 / 最可能的原因 / 用户可以做什么**。
///
/// 为什么要有这一层：以前工具失败只显示结果首行（`✗ 工具失败: xxx`），系统旁注也只有
/// 一句结论 —— 用户既不知道「哪一步」也不知道「下一步做什么」。三要素是同一个形状，
/// 失败卡与出错旁注共用。
class FailureHint {
  const FailureHint({
    required this.step,
    required this.cause,
    required this.next,
  });

  /// 哪一步没成（含工具名）。
  final String step;

  /// 最可能的原因（**只写能判定的**，判不出就给笼统说法，不猜）。
  final String cause;

  /// 用户可以做什么。
  final String next;
}

/// 把一条失败文本归到「三要素」；认不出时给通用三段（不返回 null，除非根本没有文本）。
///
/// **原始状态码 / URL / JSON 不进来**：这里只做关键词判定（`forbidden.md` §禁用 59
/// 要求原始错误只进日志）；判定用的字符串只在内部用，不出现在返回文案里。
/// 顺序是「越具体越先」：审批被拒 → 工作区锁 → 超时 → 限流 → 上下文超限 → 鉴权 →
/// 网络 → 权限 → 命令不存在 → 文件不存在 → 服务端 → 通用。
FailureHint? failureHintFor({
  required String toolName,
  required String text,
  bool genericFallback = true,
}) {
  final raw = text.trim();
  if (raw.isEmpty) {
    return null;
  }
  final body = raw.startsWith('⚠') ? raw.substring(1).trim() : raw;
  final lower = body.toLowerCase();
  final tool = toolName.trim().isEmpty ? '这一步' : toolName.trim();
  bool has(List<String> needles) =>
      needles.any((n) => lower.contains(n.toLowerCase()));

  if (has(['user denied', 'denied this action', '用户拒绝', '审批被拒'])) {
    return FailureHint(
      step: '$tool 没有执行（审批被拒）',
      cause: '这次调用需要你批准，而它被拒绝了',
      next: '确认无误后重发，并在审批卡上点「允许」；只读操作可以先让我解释清楚再做',
    );
  }
  if (has(['工作区锁', 'workspace lock', 'workspace-lock', '另一个 lunac'])) {
    return FailureHint(
      step: '$tool 没有拿到工作区',
      cause: '这个工作区正被另一个会话 / 进程占用（同一个工作区只允许一个写者）',
      next: '关掉另一个正在跑的会话，或在设置里换一个工作区后重试',
    );
  }
  if (has(['timed out', 'timeout', 'etimedout', '超时'])) {
    return FailureHint(
      step: '$tool 没有在时限内完成',
      cause: '对端迟迟没有响应（网络慢 / 服务端排队 / 命令本身卡住）',
      next: '稍后重试；命令类的可以拆小或先只读试跑一次，确认是慢还是卡死',
    );
  }
  if (has(['429', 'rate limit', 'rate_limit', 'too many requests', '限流'])) {
    return FailureHint(
      step: '$tool 的请求被端点挡回',
      cause: '短时间内请求太多（触发了端点的速率限制）',
      next: '等几十秒再重试；连续出现就降低并发、或换一个模型 / 端点',
    );
  }
  if (has([
    'context length',
    'context_length_exceeded',
    'prompt is too long',
    'too many tokens',
    '上下文超',
    '超出上下文',
  ])) {
    return FailureHint(
      step: '这次请求没能发出去',
      cause: '上下文已经超过模型的窗口上限',
      next: '用「更多 → 压缩上下文」压一次，或开一个新会话只带必要文件',
    );
  }
  if (has([
        'unauthorized',
        'invalid api key',
        'invalid_api_key',
        'incorrect api key',
        'token is not set',
        'api key is not set',
        'key is not set',
        '鉴权失败',
        '密钥无效',
      ]) ||
      // 只认独立的 401 / 403（工具输出里的数字太多，`contains('401')` 会误判）。
      RegExp(r'\b40[13]\b').hasMatch(lower)) {
    return FailureHint(
      step: '这次请求没有发出去（鉴权失败）',
      cause: '端点不接受当前的密钥 / 模型配置（或根本没配）',
      next: '到「设置 → AI 模型」检查 Base URL / API Key / 模型名，保存后点「重试」',
    );
  }
  if (has([
    'econnrefused',
    'econnreset',
    'enotfound',
    'getaddrinfo',
    'failed to lookup',
    '连接被拒绝',
    '网络不可达',
    '连接失败',
  ])) {
    return FailureHint(
      step: '$tool 连不上目标',
      cause: '网络不通、域名解析失败，或对端端口没有在监听',
      next: '确认地址与端口正确、网络可达；本地服务类先看它是否已经启动',
    );
  }
  if (has([
    'eacces',
    'eperm',
    'access is denied',
    'permission denied',
    '拒绝访问',
    '权限不足',
  ])) {
    return FailureHint(
      step: '$tool 被系统拒绝',
      cause: '当前用户对目标文件 / 目录没有权限（或文件被占用）',
      next: '换一个有权限的路径，或先关掉占用该文件的程序；不要用提权来绕过',
    );
  }
  if (has([
    'command not found',
    '不是内部或外部命令',
    'not recognized as an internal or external command',
    '无法识别',
    'no such command',
  ])) {
    return FailureHint(
      step: '$tool 要跑的命令不存在',
      cause: '命令名拼错，或这个工具没有安装 / 不在 PATH 里',
      next: '确认命令名，或在机器上装好它再重试（也可以让我先列一下可用命令）',
    );
  }
  if (has([
    'no such file',
    'enoent',
    'cannot find path',
    '找不到路径',
    '找不到文件',
    '系统找不到指定的',
    'does not exist',
  ])) {
    return FailureHint(
      step: '$tool 找不到目标文件 / 目录',
      cause: '路径写错、文件被移动或删除（也可能是工作区换过）',
      next: '让我先列一下目录确认路径，再重试这一步',
    );
  }
  if (has([
    'internal server error',
    'bad gateway',
    'service unavailable',
    'gateway timeout',
    'overloaded',
    '服务端错误',
  ])) {
    return FailureHint(
      step: '$tool 的请求被服务端拒绝',
      cause: '端点自己出错了（过载 / 临时故障），不是你的操作有问题',
      next: '过一会儿重试；持续出现就换端点或稍后再用',
    );
  }
  if (!genericFallback) {
    // 认不出具体类别时**不编**三段：调用方（比如出错旁注）本来就有一句人话可显示。
    return null;
  }
  return FailureHint(
    step: '$tool 没有成功',
    cause: '工具返回了错误（具体原因见这张卡里的原始输出）',
    next: '先看原始输出定位；修好后重发这一步，连续失败时换一个更小的操作验证',
  );
}

/// 任务抽屉的标题：取该任务里**主要内容**的首行，压掉 Markdown 标记与多余空白。
///
/// 2026-11-20 用户口径：不再用「任务 1 / 任务 2」这种编号，只保留这一条任务在做什么
/// 的概括。空内容回落到 [fallback]（例如只有过程、还没有用户提问的那一段）。
String summarizeTaskTitle(String text, {String fallback = '过程记录'}) {
  var line = text
      .split(RegExp(r'\r?\n'))
      .map((item) => item.trim())
      .firstWhere((item) => item.isNotEmpty, orElse: () => '');
  line = line
      .replaceFirst(RegExp(r'^[#>*`\-\d.\s]+'), '')
      .replaceAll(RegExp(r'\s+'), ' ')
      .trim();
  if (line.isEmpty) {
    line = fallback.trim().isEmpty ? '过程记录' : fallback.trim();
  }
  if (line.runes.length > 48) {
    line = '${String.fromCharCodes(line.runes.take(48))}…';
  }
  return line;
}

/// 一条可渲染的对话条目。
enum ChatEntryKind {
  user,
  assistant,
  thinking,
  toolUse,
  toolResult,
  notice,
  error,
  approval,
  file,
}

class ChatEntry {
  ChatEntry({
    required this.kind,
    this.text = '',
    this.toolName,
    this.toolUseId,
    this.requestId,
    this.detail,
    this.input,
    this.analysis,
    this.handled = false,
    this.subtype,
    this.elicitMessage = '',
    this.elicitFields = const [],
    this.trustProject = '',
    this.trustServers = const [],
    this.samplingServer = '',
    this.samplingMessages = 0,
    this.samplingMaxTokens = 0,
    this.samplingViaHttp = false,
    this.filePath = '',
    this.fileName = '',
    this.fileKind = 'other',
    this.fileSize = 0,
    this.filePreview = '',
  });

  final ChatEntryKind kind;

  /// 正文（用户消息 / 助手文本 / 思考 / 工具结果 / 提示 / 错误）。
  String text;

  /// 工具名（`toolUse` / `approval`）。
  final String? toolName;
  final String? toolUseId;

  /// 审批请求 id（`approval`）。
  final String? requestId;

  /// 工具入参 / 审批详情的紧凑 JSON。
  String? detail;

  /// 审批请求的原始入参（只给边界账本取 `target` / `evidence` 用，不参与渲染）。
  final Map<String, dynamic>? input;

  /// agent 随 `can_use_tool` 附上的静态分析（危险 / 不可判定 / 凭据 / 可证只读）。
  ///
  /// 只给审批卡上的「为什么问我」与账本原因码用；没有它就没法解释真实判定。
  final Map<String, dynamic>? analysis;

  /// 审批是否已处理（处理过就不再显示按钮）。
  bool handled;

  /// 审批子类型：`can_use_tool` / `elicitation` / `mcp_trust` / `mcp_sampling`。
  final String? subtype;

  /// `elicitation`：服务端要求用户补充说明的提示与表单字段。
  final String elicitMessage;
  final List<ElicitField> elicitFields;

  /// `mcp_trust`：项目路径 + 待信任的服务器清单。
  final String trustProject;
  final List<McpTrustServer> trustServers;

  /// `mcp_sampling`：借用本机模型的服务器与请求概要。
  final String samplingServer;
  final int samplingMessages;
  final int samplingMaxTokens;
  final bool samplingViaHttp;

  /// `file`：agent 通过 `SendUserFile` 呈递的文件（只带元信息，不读内容）。
  final String filePath;
  final String fileName;
  final String fileKind;
  final int fileSize;
  final String filePreview;
}

/// elicitation 表单的一个字段（JSON Schema 子集：enum / boolean / number / string）。
class ElicitField {
  const ElicitField({
    required this.key,
    required this.label,
    required this.type,
    this.options = const [],
    this.description,
    this.defaultValue,
    this.required = false,
  });

  final String key;

  /// 显示名（`title`，缺省用 key）。
  final String label;

  /// `string` / `number` / `integer` / `boolean`。
  final String type;

  /// `enum` 值（非空时渲染下拉框）。
  final List<String> options;

  /// 输入框占位提示（`description`）。
  final String? description;

  final String? defaultValue;
  final bool required;
}

/// `mcp_trust` 卡片里的一台服务器。
class McpTrustServer {
  const McpTrustServer({
    required this.name,
    required this.transport,
    required this.target,
  });

  final String name;
  final String transport;
  final String target;
}

/// 一条进历史的纯文本消息（工具卡 / 审批卡不进历史）。
class ChatMessage {
  const ChatMessage(this.role, this.content);

  /// `user` / `assistant`。
  final String role;
  final String content;
}

/// 一条已在后台运行的命令（agent `system/background_started`）。
class BackgroundCommand {
  const BackgroundCommand({
    required this.id,
    required this.toolUseId,
    required this.label,
    this.pid = 0,
  });

  /// agent 给的后台 id（`bg_<pid>_<seq>`；`background_done` 按它移除）。
  final String id;

  /// 原来那条 `tool_use` 的 id（取消时经 `tool_control` 的 `stop` 发回去）。
  final String toolUseId;

  /// 命令摘要（agent 截到 120 字）。
  final String label;

  /// 子进程 PID（0 = agent 没给）。
  final int pid;
}

/// 一条后台子代理（`task_started` / `task_done` 的 `background: true` 那一支）。
///
/// 这是 §待办 3-28.7 的数据面：跨回合还在跑的子代理必须能被看见（活动面板）、
/// 能被终止（`stop_task`）、完成后能回答「跑了多久 / 花了多少」。
class BackgroundTask {
  BackgroundTask({
    required this.id,
    required this.label,
    required this.role,
    required this.startedMs,
  });

  /// agent 的 `task_id`（`task-<n>`）；终止时按它寻址。
  final String id;

  /// `Agent` 的 `description`（短标签）。
  final String label;

  /// `general` / `explore` / `coding` / `review` / `research`。
  final String role;

  /// 起点（agent 的 `started_ms`；没有就是 0 ⇒ 不显示「已跑」）。
  final int startedMs;

  /// 最近一次 `task_progress` 的工具名（空 = 还没有）。
  String? tool;

  /// 最近一次的轮次（0 = 还没收到）。
  int round = 0;

  /// 还在跑（`task_done` 到达后为 false）。
  bool running = true;

  /// 收口结果：`done` / `partial` / `failed` / `cancelled`。
  String status = '';

  /// 总耗时（`task_done.ms`）。
  int elapsedMs = 0;

  /// 烧掉的加权单位（`task_done.spent`；只有后台子代理给）。
  int spent = 0;

  /// 面板主文案（与 v1 一样用角色 + 标签）。
  String get title => label.isEmpty ? id : label;

  /// 收口后的状态词（跑着时为空）。
  String get statusText {
    if (running) {
      return tool == null || tool!.isEmpty ? '在跑' : '在跑 · $tool';
    }
    return switch (status) {
      'partial' => '部分完成',
      'failed' => '失败',
      'cancelled' => '已终止',
      _ => '已完成',
    };
  }
}

/// 一条待办（`TodoWrite` 入参里的 `todos[]`）。
class TodoItem {
  const TodoItem({required this.content, required this.status});

  final String content;

  /// `pending` / `in_progress` / `completed`。
  final String status;
}

/// 一次 `system/init` 的 SYSTEM / CONTEXT 快照（§待办 3-28.12 ① 的逐轮时间线）。
///
/// 只存字符数与哈希，**不存正文** —— 系统提示词可能含人格 / 记忆原文，界面没有逐字
/// 阅读它的需求；哈希让「前缀缓存为什么失效」在轨迹里可对照。
class PromptSnapshot {
  const PromptSnapshot({
    required this.turn,
    required this.systemChars,
    required this.systemSha,
    required this.contextPresent,
    required this.contextChars,
    required this.contextSha,
  });

  /// 第几轮（从 1 起；时间线被裁剪后仍然单调递增，不重编号）。
  final int turn;
  final int systemChars;
  final String systemSha;
  final bool contextPresent;
  final int contextChars;
  final String contextSha;

  String get systemHashShort =>
      systemSha.length <= 12 ? systemSha : systemSha.substring(0, 12);
  String get contextHashShort =>
      contextSha.length <= 12 ? contextSha : contextSha.substring(0, 12);

  Map<String, Object?> toJson() => {
    'turn': turn,
    'system': {'chars': systemChars, 'sha256': systemSha},
    'context': {
      'present': contextPresent,
      'chars': contextChars,
      'sha256': contextSha,
    },
  };

  /// 解析失败（缺 turn / 没有可读的哈希）⇒ `null`，由调用方决定降级。
  static PromptSnapshot? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final system = json['system'];
    final context = json['context'];
    final systemSha = system is Map && system['sha256'] is String
        ? system['sha256'] as String
        : '';
    final contextSha = context is Map && context['sha256'] is String
        ? context['sha256'] as String
        : '';
    if (systemSha.isEmpty && contextSha.isEmpty) {
      return null;
    }
    final turn = json['turn'] is num ? (json['turn'] as num).toInt() : 0;
    return PromptSnapshot(
      turn: turn > 0 ? turn : 1,
      systemChars: system is Map && system['chars'] is num
          ? (system['chars'] as num).toInt()
          : 0,
      systemSha: systemSha,
      contextPresent: context is Map && context['present'] == true,
      contextChars: context is Map && context['chars'] is num
          ? (context['chars'] as num).toInt()
          : 0,
      contextSha: contextSha,
    );
  }
}

/// 会话级单例：面板关闭 / 重开时保留对话（agent 进程本身也常驻，见 v1 同语义）。
final ChatModel chatSession = ChatModel();

class ChatModel {
  final List<ChatEntry> entries = <ChatEntry>[];

  /// 会话标识（第一次发消息时生成；历史落盘按它覆盖）。
  String? sessionId;
  DateTime? createdAt;

  /// 纯文本对话（历史落盘 / `set_history` 回灌用）。
  final List<ChatMessage> messages = <ChatMessage>[];

  /// 待办清单：`TodoWrite` 每轮发的是完整清单，整体替换（与 v1 任务抽屉同口径）。
  final List<TodoItem> todos = <TodoItem>[];

  /// 后台运行中的命令（v1 `backgroundCmds`）：只由 `background_started` /
  /// `background_done` 两条事件驱动，前端不自己推断「跑完没有」。
  final List<BackgroundCommand> backgroundCommands = <BackgroundCommand>[];

  /// 后台子代理（§待办 3-28.7）：`task_started(background:true)` 加、
  /// `task_progress` 更新、`task_done` 收口。同样只由事件驱动。
  ///
  /// 与 `backgroundCommands` 分开：那条通路是「一条 shell 命令」，这条是「一个会继续
  /// 烧 token 的子代理」，取消动作、状态文案、预算归因都不同。
  final List<BackgroundTask> backgroundTasks = <BackgroundTask>[];

  /// 还在跑的后台子代理数（活动面板的角标 / 页签文案用）。
  int get runningBackgroundTasks =>
      backgroundTasks.where((task) => task.running).length;

  /// `system/init` 上报的模型名。
  String? modelName;

  /// 本次会话 AI 改过的文件（绝对路径，按首次出现顺序去重；v1 任务抽屉的「更改文件」页）。
  final List<String> changedFiles = <String>[];

  /// 记一笔文件改动（同一路径只留一次）。
  void noteChangedFile(String path) {
    if (path.isNotEmpty && !changedFiles.contains(path)) {
      changedFiles.add(path);
    }
  }

  /// agent 的 `Preview` 工具事件（`system/preview_open`）：由面板取出后交给预览窗。
  final List<Map<String, dynamic>> previewRequests = <Map<String, dynamic>>[];

  /// 是否有一轮正在跑（用户消息发出 → `result` 到达）。
  bool busy = false;

  /// 用量（`usage_delta` 累加；仅展示，不参与计费）。
  int inputTokens = 0;
  int outputTokens = 0;

  /// 会话累计用量（v1 `usageTotals`）：命中缓存 / 未命中 / 缓存写入 / 输出 / 请求数。
  /// 正常收尾以 `result.usage` 为准（权威值，还含子代理 / 复盘）；异常收尾用本轮
  /// `usage_delta` 的累计值兜底 —— 与 v1 同一口径（2026-10-06）。
  int hitTokens = 0;
  int missTokens = 0;
  int cacheCreateTokens = 0;
  int totalOutputTokens = 0;
  int requestCount = 0;

  /// 压缩统计：解释「命中率为什么掉了」（v1 表盘的 compact note）。
  int elidedItems = 0;
  int droppedItems = 0;

  /// 上下文水位（§待办 3-28.8）：**预算与阈值都来自 `system/init`**（agent 的
  /// `max_context_tokens` / `ELIDE_RATIO` / `DROP_RATIO`），前端不复制数字。
  /// `contextLimit == 0` = 老版本 agent 没上报 ⇒ 水位条不画（不估一个假预算）。
  int contextLimit = 0;
  double elideRatio = contextElideFallback;
  double dropRatio = contextDropFallback;

  /// 上一次请求的实测上下文体积（`usage_delta` 的 `in + read + create`）。
  ///
  /// 这是「当前上下文占预算多少」的唯一实测来源 —— 不按字符数估算，
  /// 也不为它多发一次请求（压缩判断本来就要算这个数，见 agent 的 `measured_tokens`）。
  int lastContextTokens = 0;

  /// 技能清单统计（§待办 3-28.11）：来自 `system/init.skills_listing`。
  ///
  /// `omitted > 0` = 清单超预算，那些技能**没进系统提示词**（模型不知道它们存在）。
  /// 本轮先只在对话里给一次旁注；v2 还没有技能设置页（见 `§待办 3-31`）。
  int skillTotal = 0;
  int skillListed = 0;
  bool _skillBudgetNoticed = false;

  /// SYSTEM / CONTEXT 两栏（§待办 3-28.12 ①）：来自 `system/init`，**只有字符数与哈希**，
  /// 没有正文 —— agent 的提示词可能含人格 / 记忆原文，界面不需要逐字读它。
  /// 同一份提示词 ⇒ 同一串哈希，所以「前缀缓存为什么失效」在界面上看得见。
  int systemChars = 0;
  String systemSha = '';
  bool contextPresent = false;
  int contextChars = 0;
  String contextSha = '';

  /// 逐轮时间线（§待办 3-28.12 ①）：每次 `system/init` 追加一条，最多保留
  /// [maxPromptSnapshots] 条（丢最旧；`turn` 编号继续递增，不重编）。
  final List<PromptSnapshot> promptTimeline = <PromptSnapshot>[];
  static const int maxPromptSnapshots = 50;

  String get systemHashShort =>
      systemSha.length <= 12 ? systemSha : systemSha.substring(0, 12);
  String get contextHashShort =>
      contextSha.length <= 12 ? contextSha : contextSha.substring(0, 12);

  int get skillOmitted =>
      skillTotal > skillListed ? skillTotal - skillListed : 0;

  /// 当前水位（0..1）；没有预算或还没发过请求时返回 null。
  double? get contextWaterLevel => contextLimit > 0 && lastContextTokens > 0
      ? (lastContextTokens / contextLimit).clamp(0.0, 1.0)
      : null;

  /// 用量快照版本：每累计一次 +1，表盘据此重读。
  int usageRevision = 0;

  // 本轮逐请求累计（`usage_delta`），异常收尾（拿不到 `result.usage`）时兜底。
  int _turnInput = 0;
  int _turnRead = 0;
  int _turnCreate = 0;
  int _turnOutput = 0;
  int _turnRequests = 0;

  /// 正在接收增量的助手文本 / 思考条目（整包到达时覆盖）。
  ChatEntry? _openAssistant;
  ChatEntry? _openThinking;

  /// 工具输出按 `tool_use_id` 追加到同一条结果条目。
  final Map<String, ChatEntry> _toolResults = <String, ChatEntry>{};

  /// 「准备中」的工具卡（§待办 3-28.17）：`content_block_start` 先建一张，
  /// 整包 `assistant` 到达时**复用它**（按 `tool_use_id` 对上），不再新增第二条。
  final Map<String, ChatEntry> _draftToolCards = <String, ChatEntry>{};

  /// 正在流式接收的 `input_json_delta`：按流事件里的 block `index` 累积。
  final Map<int, StringBuffer> _toolJsonBuffers = <int, StringBuffer>{};

  /// `index` → 那张准备中的卡（delta 不带 `tool_use_id`，只能按 index 找回）。
  final Map<int, ChatEntry> _draftByIndex = <int, ChatEntry>{};

  /// `TodoWrite` 的 tool_use id：它的结果不进对话流（清单只在任务抽屉里）。
  final Set<String> _todoToolIds = <String>{};

  /// `/compact` 的占位旁注：agent 回 `context_compacted` 时原地改成结果（不新建节点）。
  ChatEntry? _compactNote;

  /// 计划模式横幅（单例，与 v1 同语义）：反复进出只更新同一条，不刷屏。
  ChatEntry? _planNote;

  /// 「立即插入」但 agent 还没确认消费的消息条目（FIFO）。
  ///
  /// `steer_inserted` = 轮首安全点注入成功 ⇒ 出队；`steer_dropped` = 回合已结束没赶上
  /// ⇒ 出队并**撤掉那个气泡**，正文放回输入框（用户按发送时它已经在界面上了，
  /// 不撤回就是一条「发出去却永远没回音」的消息）。
  final List<ChatEntry> _pendingSteers = <ChatEntry>[];

  /// 最近一条被丢弃的插入正文；面板 `takeDroppedSteer()` 取一次填回输入框。
  String? _droppedSteer;

  /// 子代理的累计用量（`result.usage.subagent` 的归因，**不**再计入上面的总量）。
  ///
  /// agent 在 `result.usage` 里另外报一份「其中来自子代理 / 复盘的部分」——
  /// 只用于活动面板的归因展示，加起来会把钱算两遍。
  int subagentTokens = 0;
  int subagentRequests = 0;

  /// 每轮墙钟耗时（毫秒，按回合序号；v1 `SessionProcess.durationMs` 同口径）。
  ///
  /// 回合信息条要显示「耗时 X 秒」，而条目里没有时间信息 ⇒ 随会话落盘。
  final List<int> turnDurationsMs = <int>[];

  /// 本轮开始时刻（`addUser` 记，`result` 收口时结算）。
  DateTime? _turnStartedAt;

  int _assistantSinceUser = 0;

  /// 单条正文上限：工具输出可能很大，界面只要尾部的关键信息。
  static const int _maxEntryChars = 4000;

  /// 过程快照落盘的体积上限（单条 `_maxEntryChars`，整会话 `_maxStepsChars`）。
  static const int _maxStepsChars = 200000;

  void clear() {
    entries.clear();
    messages.clear();
    _pendingSteers.clear();
    _droppedSteer = null;
    todos.clear();
    changedFiles.clear();
    backgroundCommands.clear();
    turnDurationsMs.clear();
    _turnStartedAt = null;
    sessionId = null;
    createdAt = null;
    modelName = null;
    busy = false;
    inputTokens = 0;
    outputTokens = 0;
    hitTokens = 0;
    missTokens = 0;
    cacheCreateTokens = 0;
    totalOutputTokens = 0;
    requestCount = 0;
    elidedItems = 0;
    droppedItems = 0;
    usageRevision = 0;
    subagentTokens = 0;
    subagentRequests = 0;
    _turnInput = 0;
    _turnRead = 0;
    _turnCreate = 0;
    _turnOutput = 0;
    _turnRequests = 0;
    _openAssistant = null;
    _openThinking = null;
    // 准备中的工具卡（§待办 3-28.17）：整包 assistant 到达后这些表已经空了；
    // 异常收尾（没有整包）时剩下的残影必须清掉，否则下一轮的 delta 会写进上一轮的卡。
    _draftToolCards.clear();
    _toolJsonBuffers.clear();
    _draftByIndex.clear();
    _toolResults.clear();
    _todoToolIds.clear();
    _compactNote = null;
    _planNote = null;
    _assistantSinceUser = 0;
    promptTimeline.clear();
    systemChars = 0;
    systemSha = '';
    contextPresent = false;
    contextChars = 0;
    contextSha = '';
  }

  /// 落一条用户提问。[steerInsert] = 这条是「立即插入」（进待确认队列，
  /// 由 `steer_inserted` / `steer_dropped` 二选一出队）。
  void addUser(String text, {bool steerInsert = false}) {
    entries.add(ChatEntry(kind: ChatEntryKind.user, text: text));
    messages.add(ChatMessage('user', text));
    if (steerInsert) {
      _pendingSteers.add(entries.last);
    }
    _turnStartedAt = DateTime.now();
    sessionId ??= 'sess-${DateTime.now().microsecondsSinceEpoch}';
    createdAt ??= DateTime.now();
    _openAssistant = null;
    _openThinking = null;
    _assistantSinceUser = 0;
    // 新一轮：逐请求累计归零（上一轮已在 `_applyResult` 结账）。
    _turnInput = 0;
    _turnRead = 0;
    _turnCreate = 0;
    _turnOutput = 0;
    _turnRequests = 0;
    busy = true;
  }

  /// 删除当前会话后调用：下次落盘重新分配 id，删掉的那条不会被写回来（v1 同语义）。
  void forgetSessionId() {
    sessionId = null;
    createdAt = DateTime.now();
  }

  /// §待办 3-28.6：从某条**助手消息**分叉出一份新会话的落盘快照（原会话逐字节不变）。
  ///
  /// 只带「到这条回答为止」的纯文本 `messages`：过程快照按回合归属，跨会话截取容易
  /// 出现「工具卡配不上结果」的半截回合，宁可不带（正文完整，过程块空着）。
  /// 标题沿用首条提问 + 「（分支）」；与已有标题重名时加编号（`existingTitles`）。
  /// 这条回答不在正文里（流式半截 / 已被裁剪）⇒ null。
  Map<String, Object?>? forkSnapshotAt(
    ChatEntry entry, {
    Set<String> existingTitles = const {},
  }) {
    if (entry.kind != ChatEntryKind.assistant) {
      return null;
    }
    var index = -1;
    for (var i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role == 'assistant' &&
          messages[i].content == entry.text) {
        index = i;
        break;
      }
    }
    if (index < 0) {
      return null;
    }
    final kept = messages.sublist(0, index + 1);
    final firstUser = kept.firstWhere(
      (message) => message.role == 'user',
      orElse: () => kept.first,
    );
    var base = firstUser.content.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (base.length > 60) {
      base = '${base.substring(0, 60)}…';
    }
    if (base.isEmpty) {
      base = '未命名对话';
    }
    var title = '$base（分支）';
    if (existingTitles.contains(title)) {
      var n = 2;
      while (existingTitles.contains('$base（分支 $n）')) {
        n++;
      }
      title = '$base（分支 $n）';
    }
    final now = DateTime.now();
    return {
      'id': 'sess-${now.microsecondsSinceEpoch}',
      'title': title,
      'createdAt': now.millisecondsSinceEpoch,
      'updatedAt': now.millisecondsSinceEpoch,
      'messages': [
        for (final message in kept)
          {'role': message.role, 'content': message.content},
      ],
    };
  }

  /// 历史落盘用的快照；还没有任何消息时返回 null。
  Map<String, Object?>? historyJson() {
    final id = sessionId;
    if (id == null || messages.isEmpty) {
      return null;
    }
    final firstUser = messages.firstWhere(
      (message) => message.role == 'user',
      orElse: () => messages.first,
    );
    var title = firstUser.content.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (title.length > 60) {
      title = '${title.substring(0, 60)}…';
    }
    if (title.isEmpty) {
      title = '未命名对话';
    }
    final steps = stepsJson();
    return {
      'id': id,
      'title': title,
      'createdAt':
          createdAt?.millisecondsSinceEpoch ??
          DateTime.now().millisecondsSinceEpoch,
      'updatedAt': DateTime.now().millisecondsSinceEpoch,
      'messages': [
        for (final message in messages)
          {'role': message.role, 'content': message.content},
      ],
      if (steps.isNotEmpty) 'steps': steps,
      if (turnDurationsMs.isNotEmpty) 'turnDurations': turnDurationsMs,
      // §待办 3-28.12 ①：逐轮时间线 + 最新一次的 SYSTEM / CONTEXT 两个兼容字段
      // （老宿主只认这两个键）。两者同源：都取时间线的最后一条。
      if (promptTimeline.isNotEmpty) ...{
        'promptTimeline': [
          for (final snapshot in promptTimeline) snapshot.toJson(),
        ],
        if (promptTimeline.last.systemSha.isNotEmpty)
          'system': {
            'chars': promptTimeline.last.systemChars,
            'sha256': promptTimeline.last.systemSha,
          },
        if (promptTimeline.last.contextPresent)
          'context': {
            'present': true,
            'chars': promptTimeline.last.contextChars,
            'sha256': promptTimeline.last.contextSha,
          },
      },
    };
  }

  /// 过程快照：对话流里除审批卡 / 文件卡外的**全部条目，按原顺序**落盘。
  ///
  /// 为什么不能只存正文：思考块、工具卡、工具输出、「已修改 …」都在条目里
  /// （v1 靠 `SessionStep[]` 落盘，这里同理）。审批卡只对当前进程有效，文件卡是
  /// 一次性呈递，两者都不还原。总量上限见 `_maxStepsChars`（超出丢掉更早的回合）。
  List<Map<String, Object?>> stepsJson() {
    final chips = <Map<String, Object?>>[];
    var total = 0;
    for (final entry in entries.reversed) {
      if (entry.kind == ChatEntryKind.approval ||
          entry.kind == ChatEntryKind.file) {
        continue;
      }
      final chip = <String, Object?>{'kind': entry.kind.name};
      if (entry.toolName != null) {
        chip['name'] = entry.toolName;
      }
      if (entry.toolUseId != null) {
        chip['id'] = entry.toolUseId;
      }
      if (entry.detail != null) {
        chip['detail'] = _clipStep(entry.detail!);
      }
      if (entry.text.isNotEmpty) {
        chip['text'] = _clipStep(entry.text);
      }
      final size =
          '${chip['text'] ?? ''}'.length + '${chip['detail'] ?? ''}'.length;
      // 2026-11-20：用户提问永不因体积上限被丢。历史回写后「用户的问题消失」的根因
      // 之一就是这里按总量截断时把最早那条提问切掉了；过程块可以丢，任务锚点不行。
      final isUser = entry.kind == ChatEntryKind.user;
      if (!isUser && total + size > _maxStepsChars) {
        break;
      }
      total += size;
      chips.add(chip);
    }
    var ordered = chips.reversed.toList();
    // 从完整回合开始（半截回合的工具结果会配不上工具卡）。
    final firstUser = ordered.indexWhere(
      (chip) => chip['kind'] == ChatEntryKind.user.name,
    );
    if (firstUser > 0) {
      ordered = ordered.sublist(firstUser);
    }
    // 正文被宿主裁剪过（超过 200 条）时，`messages` 里的提问比条目少：丢掉前面
    // 多出来的**整段**回合（到它的下一条提问为止），否则恢复出的提问在 `messages`
    // 里找不到（回退 / 重发会错位）。
    final userIndexes = <int>[
      for (var index = 0; index < ordered.length; index++)
        if (ordered[index]['kind'] == ChatEntryKind.user.name) index,
    ];
    final extra =
        userIndexes.length -
        messages.where((message) => message.role == 'user').length;
    final start = extra > 0 ? userIndexes[extra] : 0;
    return start == 0 ? ordered : ordered.sublist(start);
  }

  /// 落盘时的单条截断（保留尾部：工具输出真正有用的是结尾）。
  static String _clipStep(String text) {
    if (text.length <= _maxEntryChars) {
      return text;
    }
    return '…${text.substring(text.length - _maxEntryChars)}';
  }

  /// 恢复一份历史会话：界面条目与纯文本消息一起替换。
  ///
  /// [steps] 是落盘的过程快照（见 [stepsJson]）：按原顺序重建条目，
  /// 这样切回历史会话时思考块 / 工具卡 / 「已修改 …」都还在（v1 `historyStepHtml` 同口径）。
  /// 旧记录没有快照 ⇒ 只按正文（`messages`）重建，如实降级。
  void restore({
    required String id,
    required List<ChatMessage> messages,
    DateTime? createdAt,
    List<dynamic> steps = const [],
    List<int> turnDurations = const [],
    List<PromptSnapshot> promptTimeline = const [],
  }) {
    clear();
    sessionId = id;
    this.createdAt = createdAt;
    this.messages.addAll(messages);
    turnDurationsMs.addAll(turnDurations);
    this.promptTimeline.addAll(promptTimeline);
    if (promptTimeline.isNotEmpty) {
      final last = promptTimeline.last;
      systemChars = last.systemChars;
      systemSha = last.systemSha;
      contextPresent = last.contextPresent;
      contextChars = last.contextChars;
      contextSha = last.contextSha;
    }
    for (final step in steps) {
      final entry = stepEntryOf(step);
      if (entry != null) {
        entries.add(entry);
      }
    }
    if (entries.isEmpty) {
      for (final message in messages) {
        entries.add(
          ChatEntry(
            kind: message.role == 'user'
                ? ChatEntryKind.user
                : ChatEntryKind.assistant,
            text: message.content,
          ),
        );
      }
    } else {
      // 旧会话的 `steps` 可能因体积上限缺了最早的提问。正文 `messages` 是权威：
      // 进程块照旧保留，但每条用户提问都必须回到界面（否则回写后「问题消失」）。
      _restoreMissingQuestions(messages);
    }
    _assistantSinceUser = messages.any((m) => m.role == 'assistant') ? 1 : 0;
  }

  /// 把 `messages` 里有、`entries` 里没有的用户提问按顺序插回去。
  ///
  /// 只插入**真正缺失**的提问：同一句话可能是被 `stepsJson` 截断（4000 字）过的，
  /// 用前缀比较避免把同一条又问一遍。插入位置跟着下一条用户条目的位置走，
  /// 因此顺序仍与历史一致。
  void _restoreMissingQuestions(List<ChatMessage> messages) {
    final expected = [
      for (final message in messages)
        if (message.role == 'user' && message.content.trim().isNotEmpty)
          message.content,
    ];
    if (expected.isEmpty) {
      return;
    }
    bool same(String left, String right) {
      final a = left.trim();
      final b = right.trim();
      return a == b ||
          (a.length >= 8 && a.startsWith(b)) ||
          (b.length >= 8 && b.startsWith(a));
    }

    var expectedIndex = 0;
    final merged = <ChatEntry>[];
    for (final entry in entries) {
      if (entry.kind == ChatEntryKind.user) {
        while (expectedIndex < expected.length &&
            !same(entry.text, expected[expectedIndex])) {
          merged.add(
            ChatEntry(kind: ChatEntryKind.user, text: expected[expectedIndex]),
          );
          expectedIndex++;
        }
        if (expectedIndex < expected.length) {
          expectedIndex++;
        }
      }
      merged.add(entry);
    }
    while (expectedIndex < expected.length) {
      merged.add(
        ChatEntry(kind: ChatEntryKind.user, text: expected[expectedIndex]),
      );
      expectedIndex++;
    }
    entries
      ..clear()
      ..addAll(merged);
  }

  void addNotice(String text) {
    entries.add(ChatEntry(kind: ChatEntryKind.notice, text: text));
  }

  /// 已收到 `tool_result` 的 `tool_use_id`（活动面板判「还在跑」的唯一依据）。
  Set<String> get finishedToolIds {
    final ids = <String>{};
    for (final entry in entries) {
      if (entry.kind == ChatEntryKind.toolResult && entry.toolUseId != null) {
        ids.add(entry.toolUseId!);
      }
    }
    return ids;
  }

  /// 活动面板「正在跑」：有 `tool_use`、还没收到对应 `tool_result` 的工具。
  ///
  /// §待办 3-28.2：面板**只读同一份事件流**（`entries`），不新建状态源 ——
  /// 对话流里的工具卡与本面板永远是同一批对象，不会出现「面板说在跑、卡片说完了」。
  List<ChatEntry> get runningTools {
    final finished = finishedToolIds;
    return [
      for (final entry in entries)
        if (entry.kind == ChatEntryKind.toolUse &&
            entry.toolUseId != null &&
            !finished.contains(entry.toolUseId))
          entry,
    ];
  }

  /// 活动面板「待审批」：还没处理掉的审批卡。
  List<ChatEntry> get pendingApprovals => [
    for (final entry in entries)
      if (entry.kind == ChatEntryKind.approval && !entry.handled) entry,
  ];

  /// 活动面板「子代理」：`Agent` 工具的调用（结果有没有回来决定「在跑 / 已回」）。
  List<ChatEntry> get agentCalls => [
    for (final entry in entries)
      if (entry.kind == ChatEntryKind.toolUse && entry.toolName == 'Agent')
        entry,
  ];

  /// 取走上一条被 agent 丢弃的插入正文（只取一次；没有则 null）。
  ///
  /// 面板拿它填回输入框：撤回的气泡如果连正文都不还，用户就得重打一遍。
  String? takeDroppedSteer() {
    final text = _droppedSteer;
    _droppedSteer = null;
    return text;
  }

  /// `/compact` 的占位旁注：立刻给一条反馈，压缩完成后原地改文字（与 v1 同口径）。
  void markCompactRequested() {
    _compactNote = ChatEntry(kind: ChatEntryKind.notice, text: '已请求压缩上下文，稍候…');
    entries.add(_compactNote!);
  }

  /// `/rewind`：删掉最近一轮用户提问及其后的条目，返回被删的提问文本（给输入框回填）。
  ///
  /// 没有用户消息 / 正在跑回合时返回 null（面板先判 `busy`）。
  String? rewindLastUser() {
    final entryIndex = entries.lastIndexWhere(
      (entry) => entry.kind == ChatEntryKind.user,
    );
    if (entryIndex < 0) {
      return null;
    }
    return rewindTo(entries[entryIndex]);
  }

  /// 回退到**某一条用户消息**（v1 消息动作条的「回退到此处」/「重试」）：
  /// 删掉它及其后的条目与纯文本消息，返回该提问文本（「重试」用它直接重发）。
  ///
  /// 只回退对话，不碰磁盘文件 —— 与 v1 的「连同文件快照一起还原」不同，
  /// 面板上如实写「仅回退对话」。
  String? rewindTo(ChatEntry entry) {
    if (busy || entry.kind != ChatEntryKind.user) {
      return null;
    }
    final entryIndex = entries.indexOf(entry);
    if (entryIndex < 0) {
      return null;
    }
    // 该条目是第几条 user 条目 ⇒ 对应 `messages` 里第几条 user 消息
    // （思考 / 工具 / 旁注不进 messages，不会让两边错位）。
    final ordinal = entries
        .take(entryIndex)
        .where((item) => item.kind == ChatEntryKind.user)
        .length;
    var messageIndex = -1;
    var seen = 0;
    for (var i = 0; i < messages.length; i++) {
      if (messages[i].role != 'user') {
        continue;
      }
      if (seen == ordinal) {
        messageIndex = i;
        break;
      }
      seen++;
    }
    if (messageIndex < 0) {
      return null;
    }
    final removed = messages[messageIndex].content;
    messages.removeRange(messageIndex, messages.length);
    entries.removeRange(entryIndex, entries.length);
    // 被回退掉的条目里可能有还没确认的插入 ⇒ 队列只留还在界面上的那些。
    _pendingSteers.removeWhere((pending) => !entries.contains(pending));
    // 回退到第 `ordinal` 轮之前 ⇒ 耗时表也砍到同一位置（信息条不能留着已删回合的数字）。
    if (turnDurationsMs.length > ordinal) {
      turnDurationsMs.removeRange(ordinal, turnDurationsMs.length);
    }
    _toolResults.clear();
    _compactNote = null;
    _openAssistant = null;
    _openThinking = null;
    _assistantSinceUser = 0;
    return removed;
  }

  void addError(String text) {
    entries.add(ChatEntry(kind: ChatEntryKind.error, text: text));
    busy = false;
  }

  /// 折叠一条 stdout 事件。
  void apply(Map<String, dynamic> event) {
    switch (event['type']) {
      case 'system':
        _applySystem(event);
      case 'stream_event':
        _applyStreamEvent(event['event']);
      case 'assistant':
        _applyAssistant(event['message']);
      case 'user':
        _applyUserMessage(event['message']);
      case 'tool_output':
        _applyToolOutput(event);
      case 'usage_delta':
        final usage = event['usage'];
        if (usage is Map) {
          // 一次请求的上下文体积 = in + read + create（与 agent 的水位口径同源；
          // 输出不属于「已占用上下文」，不算进来）。
          final context =
              _asInt(usage['in']) +
              _asInt(usage['read']) +
              _asInt(usage['create']);
          if (context > 0) {
            lastContextTokens = context;
          }
          inputTokens += _asInt(usage['in']);
          outputTokens += _asInt(usage['out']);
          // 逐请求累计：`result.usage` 缺失（回合被取消 / 被强杀）时用它结账。
          _turnInput += _asInt(usage['in']);
          _turnRead += _asInt(usage['read']);
          _turnCreate += _asInt(usage['create']);
          _turnOutput += _asInt(usage['out']);
          _turnRequests += 1;
        }
      case 'control_request':
        _applyControlRequest(event);
      case 'control_cancel_request':
        _cancelApproval(event['request_id']);
      case 'result':
        _applyResult(event);
    }
  }

  void _applySystem(Map<String, dynamic> event) {
    switch (event['subtype']) {
      case 'init':
        final model = event['model'];
        if (model is String && model.isNotEmpty) {
          modelName = model;
        }
        // §待办 3-28.8：预算与两档阈值都来自 agent 的同名常量（不复制数字）。
        final limit = _asInt(event['max_context_tokens']);
        if (limit > 0) {
          contextLimit = limit;
        }
        final elide = event['elide_ratio'];
        if (elide is num && elide > 0 && elide < 1) {
          elideRatio = elide.toDouble();
        }
        final drop = event['drop_ratio'];
        if (drop is num && drop > 0 && drop <= 1) {
          dropRatio = drop.toDouble();
        }
        // §待办 3-28.11：清单超预算的技能不会进系统提示词 —— 只给一次旁注
        // （init 每轮都发，重复提示就是噪音）。
        final listing = event['skills_listing'];
        if (listing is Map) {
          skillTotal = _asInt(listing['total']);
          skillListed = _asInt(listing['listed']);
          if (skillOmitted > 0 && !_skillBudgetNoticed) {
            _skillBudgetNoticed = true;
            addNotice(
              '技能清单超出预算：共 $skillTotal 个，只有 $skillListed 个进了系统提示词'
              '（其余 $skillOmitted 个模型看不到）。缩短技能描述即可全部列出。',
            );
          }
        }
        // §待办 3-28.12 ①：SYSTEM / CONTEXT 两栏（只有哈希与字符数，正文不发）。
        final prompt = event['system_prompt'];
        if (prompt is Map) {
          systemChars = _asInt(prompt['chars']);
          final sha = prompt['sha256'];
          if (sha is String) {
            systemSha = sha;
          }
        }
        final context = event['context_block'];
        if (context is Map) {
          contextPresent = context['present'] == true;
          contextChars = _asInt(context['chars']);
          final sha = context['sha256'];
          if (sha is String) {
            contextSha = sha;
          }
        }
        // 逐轮时间线：每次 init 一条（agent 每轮都发）。老 agent 两项都没有 ⇒ 不追加，
        // 免得在轨迹里造出一串「全是 0」的假轮次。
        if (prompt is Map || context is Map) {
          final turn = promptTimeline.isEmpty
              ? 1
              : promptTimeline.last.turn + 1;
          promptTimeline.add(
            PromptSnapshot(
              turn: turn,
              systemChars: systemChars,
              systemSha: systemSha,
              contextPresent: contextPresent,
              contextChars: contextChars,
              contextSha: contextSha,
            ),
          );
          if (promptTimeline.length > maxPromptSnapshots) {
            promptTimeline.removeRange(
              0,
              promptTimeline.length - maxPromptSnapshots,
            );
          }
        }
      case 'api_retry':
        addNotice('接口重试中（第 ${_asInt(event['attempt'])} 次）…');
      case 'steer_inserted':
        // §待办 3-28.1「立即插入」的落地回执：agent 只在**轮首安全点**注入
        // （工具批中途不插，否则拆坏 `tool_use` / `tool_result` 配对）。
        // 这句话是「真正生效」的时刻 —— 不做这一条，用户会以为消息丢了。
        if (_pendingSteers.isNotEmpty) {
          _pendingSteers.removeAt(0);
        }
        final chars = _asInt(event['chars']);
        addNotice('已插入本回合（轮首生效${chars > 0 ? '，$chars 字' : ''}）');
      case 'steer_dropped':
        // 回合已被预算 / 轮次硬收口收尾，这条插入赶不上安全点 ⇒ agent 直接丢弃。
        // 界面必须跟着撤回：那条气泡从没进过 agent 的 history，留着就是假的。
        String? retracted;
        if (_pendingSteers.isNotEmpty) {
          final entry = _pendingSteers.removeAt(0);
          retracted = entry.text;
          entries.remove(entry);
          // 同文提问可能有多条 ⇒ 从尾部摘第一条 user（就是刚撤的这条）。
          for (var i = messages.length - 1; i >= 0; i--) {
            if (messages[i].role == 'user' &&
                messages[i].content == retracted) {
              messages.removeAt(i);
              break;
            }
          }
          _droppedSteer = retracted;
        }
        addNotice(
          '这条插入没赶上本轮（回合已结束），已撤回'
          '${retracted == null ? '' : '并放回输入框'} —— 请重新发送。',
        );
      case 'context_compacted':
        // 手动 `/compact` 的占位旁注原地改成结果，其余情况补一条（与 v1 同口径）。
        final elided = _asInt(event['elided']);
        final dropped = _asInt(event['dropped']);
        elidedItems += elided;
        droppedItems += dropped;
        usageRevision++;
        final text = '上下文已压缩：省略 $elided 个工具结果，丢弃 $dropped 条早期消息';
        final note = _compactNote;
        if (note != null) {
          note.text = text;
          _compactNote = null;
        } else {
          addNotice(text);
        }
      case 'budget_extended':
        addNotice('上下文已压缩，继续执行');
      case 'budget_exhausted':
        addNotice('本轮花费已达上限，AI 正在收尾作答');
      case 'rounds_exhausted':
        addNotice('本轮工具调用已达上限，AI 正在收尾作答');
      case 'plan_mode':
        _applyPlanMode(event['state'], event['reason']);
      case 'background_started':
        _applyBackgroundStarted(event);
      case 'background_done':
        final id = event['id'] is String ? event['id'] as String : '';
        backgroundCommands.removeWhere((command) => command.id == id);
      case 'task_started':
        // §待办 3-28.7：只有 `background: true` 这一支进面板 —— 前台子代理已经有
        // 自己的 `Agent` 工具卡（结果回没回来决定「在跑 / 已回」），再列一遍就是两份状态。
        if (event['background'] != true) {
          break;
        }
        final startId = event['task_id'] is String
            ? event['task_id'] as String
            : '';
        if (startId.isEmpty) {
          break;
        }
        backgroundTasks.removeWhere((task) => task.id == startId);
        backgroundTasks.add(
          BackgroundTask(
            id: startId,
            label: event['description'] is String
                ? event['description'] as String
                : '',
            role: event['role'] is String ? event['role'] as String : 'general',
            startedMs: _asInt(event['started_ms']),
          ),
        );
      case 'task_progress':
        final progressId = event['task_id'] is String
            ? event['task_id'] as String
            : '';
        for (final task in backgroundTasks) {
          if (task.id != progressId) {
            continue;
          }
          task.round = _asInt(event['round']);
          final tool = event['tool'];
          if (tool is String && tool.isNotEmpty) {
            task.tool = tool;
          }
        }
      case 'task_done':
        final doneId = event['task_id'] is String
            ? event['task_id'] as String
            : '';
        if (event['background'] != true || doneId.isEmpty) {
          break;
        }
        final index = backgroundTasks.indexWhere((task) => task.id == doneId);
        if (index < 0) {
          break;
        }
        final task = backgroundTasks[index];
        task.running = false;
        task.elapsedMs = _asInt(event['ms']);
        task.spent = _asInt(event['spent']);
        final cancelled = event['cancelled'] == true;
        final ok = event['ok'] == true;
        final partial = event['partial'] == true;
        task.status = cancelled
            ? 'cancelled'
            : !ok
            ? 'failed'
            : partial
            ? 'partial'
            : 'done';
        // 用户必须知道「它跑完了、报告在哪」—— 报告本体进下一轮的 tool_result，
        // 不在对话流里复制一遍（复制就是第二份真相）。
        final seconds = (task.elapsedMs / 1000).toStringAsFixed(1);
        addNotice(
          cancelled
              ? '后台子代理「${task.title}」已终止'
              : !ok
              ? '后台子代理「${task.title}」失败'
              : partial
              ? '后台子代理「${task.title}」部分完成（${seconds}s，撞了轮数或预算上限），'
                    '报告将在下一轮并入'
              : '后台子代理「${task.title}」已完成（${seconds}s），报告将在下一轮并入',
        );
      case 'hook_note':
        _applyHookNote(event);
      case 'user_file':
        _applyUserFile(event['file']);
      case 'preview_open':
        // 预览窗标签：不在对话流里落卡片，交给面板转给预览窗（见 ChatPanel）。
        final tab = event['tab'];
        if (tab is Map) {
          previewRequests.add(Map<String, dynamic>.from(tab));
        }
      case 'push_notification':
        _applyPushNotification(event['notification']);
      case 'mcp_config_changed':
        // v2 还没有 MCP 配置界面 / 热重启；如实提示，不假装已生效。
        addNotice('MCP 配置已变更：重启 agent 后生效');
      case 'attachment_note':
        final skipped = event['skipped'];
        if (skipped is List && skipped.isNotEmpty) {
          final names = [
            for (final item in skipped)
              if (item is Map && item['path'] is String) item['path'] as String,
          ];
          if (names.isNotEmpty) {
            addNotice('附件被跳过：${names.join('、')}');
          }
        }
    }
  }

  /// 计划模式横幅：只跟着 agent 的 `system/plan_mode` 广播走（不自行推断），单例复用。
  void _applyPlanMode(dynamic state, dynamic reason) {
    final on = state == 'on';
    final label = on ? '计划模式：只读，模型会先给出计划再动手' : '计划已批准，写权限恢复';
    final suffix = reason is String && reason.trim().isNotEmpty
        ? ' · ${reason.trim()}'
        : '';
    final text = '$label$suffix';
    final note = _planNote;
    if (note != null) {
      note.text = text;
      return;
    }
    _planNote = ChatEntry(kind: ChatEntryKind.notice, text: text);
    entries.add(_planNote!);
  }

  /// 后台命令登记（v1 `backgroundCmds.set`）：同一个后台 id 只留一条。
  void _applyBackgroundStarted(Map<String, dynamic> event) {
    final id = event['id'] is String ? event['id'] as String : '';
    if (id.isEmpty) {
      return;
    }
    final toolUseId = event['tool_use_id'] is String
        ? event['tool_use_id'] as String
        : '';
    final label = event['label'] is String ? event['label'] as String : '';
    final pid = event['pid'] is int ? event['pid'] as int : 0;
    backgroundCommands.removeWhere((command) => command.id == id);
    backgroundCommands.add(
      BackgroundCommand(id: id, toolUseId: toolUseId, label: label, pid: pid),
    );
  }

  /// 权限 hook 的裁决 / 输出 / 失败（v1 `renderHookNote` 的等价物）。
  void _applyHookNote(Map<String, dynamic> event) {
    final items = event['items'];
    if (items is! List || items.isEmpty) {
      return;
    }
    final lines = <String>[];
    for (final item in items) {
      if (item is! Map) {
        continue;
      }
      final command = item['command'];
      lines.add(
        '[${item['kind'] ?? '?'}] ${item['text'] ?? ''}'
        '${command is String && command.isNotEmpty ? ' — $command' : ''}',
      );
    }
    if (lines.isEmpty) {
      return;
    }
    final hookEvent = event['hook_event'] is String
        ? event['hook_event'] as String
        : 'hook';
    final tool = event['tool_name'] is String
        ? event['tool_name'] as String
        : '';
    addNotice(
      'hook $hookEvent${tool.isEmpty ? '' : ' · $tool'} · ${lines.length} 条\n'
      '${lines.join('\n')}',
    );
  }

  void _applyUserFile(dynamic file) {
    if (file is! Map) {
      return;
    }
    final path = file['path'] is String ? file['path'] as String : '';
    if (path.isEmpty) {
      return;
    }
    final name = file['name'];
    entries.add(
      ChatEntry(
        kind: ChatEntryKind.file,
        filePath: path,
        fileName: name is String && name.trim().isNotEmpty ? name : path,
        fileKind: file['kind'] is String ? file['kind'] as String : 'other',
        fileSize: _asInt(file['size']),
        filePreview: file['preview'] is String ? file['preview'] as String : '',
      ),
    );
  }

  /// `TodoWrite` 的完整清单：整体替换（同一轮内多次写入只留最后一次）。
  void _applyTodoWrite(dynamic input) {
    if (input is! Map) {
      return;
    }
    final list = input['todos'];
    if (list is! List) {
      return;
    }
    todos
      ..clear()
      ..addAll([
        for (final item in list)
          if (item is Map)
            TodoItem(
              content: item['content'] is String
                  ? item['content'] as String
                  : '',
              status: item['status'] is String
                  ? item['status'] as String
                  : 'pending',
            ),
      ]);
  }

  void _applyPushNotification(dynamic notification) {
    if (notification is! Map) {
      return;
    }
    final title = notification['title'] is String
        ? notification['title'] as String
        : '';
    final body = notification['body'] is String
        ? notification['body'] as String
        : '';
    final text = [
      title,
      body,
    ].where((part) => part.trim().isNotEmpty).join(' — ');
    if (text.isNotEmpty) {
      addNotice('通知：$text');
    }
  }

  void _applyStreamEvent(dynamic payload) {
    if (payload is! Map) {
      return;
    }
    switch (payload['type']) {
      case 'content_block_start':
        // §待办 3-28.17：工具调用还没发完就先摆一张卡（工具名立刻可见），
        // 参数在 `input_json_delta` 里逐段解析（解析不出就只报进度，不猜）。
        final block = payload['content_block'];
        if (block is! Map || block['type'] != 'tool_use') {
          return;
        }
        final id = block['id'];
        if (id is! String || id.isEmpty) {
          return;
        }
        if (block['name'] == 'TodoWrite') {
          // TodoWrite 不占命令卡（清单在任务抽屉里）；这里也不建，免得闪一下又消失。
          return;
        }
        final entry = ChatEntry(
          kind: ChatEntryKind.toolUse,
          toolName: block['name'] is String ? block['name'] as String : '工具调用',
          toolUseId: id,
          detail: '准备参数…',
        );
        entries.add(entry);
        _draftToolCards[id] = entry;
        final index = _asInt(payload['index']);
        _toolJsonBuffers[index] = StringBuffer();
        _draftByIndex[index] = entry;
      case 'content_block_delta':
        final delta = payload['delta'];
        if (delta is! Map) {
          return;
        }
        switch (delta['type']) {
          case 'text_delta':
            final text = delta['text'];
            if (text is String && text.isNotEmpty) {
              _appendStreaming(ChatEntryKind.assistant, text);
            }
          case 'thinking_delta':
            final thinking = delta['thinking'];
            if (thinking is String && thinking.isNotEmpty) {
              _appendStreaming(ChatEntryKind.thinking, thinking);
            }
          case 'input_json_delta':
            final chunk = delta['partial_json'];
            final buffer = _toolJsonBuffers[_asInt(payload['index'])];
            if (chunk is String && buffer != null) {
              buffer.write(chunk);
              final card = _draftByIndex[_asInt(payload['index'])];
              if (card != null) {
                card.detail = partialToolDetail(buffer.toString());
              }
            }
        }
      case 'message_stop':
        // 这里**不能**清 `_openAssistant` / `_openThinking`：core-agent 的
        // `message_stop` 之后紧跟着整包 `assistant` 消息，清掉就会让整包再建一条，
        // 于是同一段小输出在界面上出现两次（用户 2026-11-15 报障）。
        // 合并在 `_applyAssistant` 里做，合完再收口。
        break;
    }
  }

  void _appendStreaming(ChatEntryKind kind, String chunk) {
    final entry = kind == ChatEntryKind.assistant
        ? (_openAssistant ??= _newEntry(kind))
        : (_openThinking ??= _newEntry(kind));
    _append(entry, chunk);
    if (kind == ChatEntryKind.assistant) {
      _assistantSinceUser = 1;
    }
  }

  ChatEntry _newEntry(ChatEntryKind kind) {
    final entry = ChatEntry(kind: kind);
    entries.add(entry);
    return entry;
  }

  void _applyAssistant(dynamic message) {
    if (message is! Map) {
      return;
    }
    final content = message['content'];
    if (content is! List) {
      return;
    }
    final texts = <String>[];
    final thinkings = <String>[];
    for (final block in content) {
      if (block is! Map) {
        continue;
      }
      switch (block['type']) {
        case 'text':
          final text = block['text'];
          if (text is String && text.isNotEmpty) {
            texts.add(text);
          }
        case 'thinking':
          final thinking = block['thinking'];
          if (thinking is String && thinking.isNotEmpty) {
            thinkings.add(thinking);
          }
        case 'tool_use':
          final id = block['id'];
          final name = block['name'] is String
              ? block['name'] as String
              : '工具调用';
          // §待办 3-28.17：流式阶段已经摆过一张「准备中」的卡 ⇒ 原地补全参数，
          // 不再新增第二条（否则同一个工具会出现两张卡）。
          final draft = id is String ? _draftToolCards.remove(id) : null;
          // TodoWrite 只更新待办清单，不占命令卡（清单画在输入框上方的抽屉里）。
          if (name == 'TodoWrite') {
            if (draft != null) {
              entries.remove(draft);
            }
            _applyTodoWrite(block['input']);
            if (id is String) {
              _todoToolIds.add(id);
            }
            break;
          }
          if (draft != null) {
            draft.detail = compactJson(block['input']);
            break;
          }
          entries.add(
            ChatEntry(
              kind: ChatEntryKind.toolUse,
              toolName: name,
              toolUseId: id is String ? id : null,
              detail: compactJson(block['input']),
            ),
          );
      }
    }
    if (texts.isNotEmpty) {
      final joined = texts.join('\n');
      final open = _openAssistant;
      if (open != null) {
        open.text = joined; // 整包覆盖增量，避免重复
      } else {
        entries.add(ChatEntry(kind: ChatEntryKind.assistant, text: joined));
      }
      messages.add(ChatMessage('assistant', joined));
      _assistantSinceUser = 1;
    }
    if (thinkings.isNotEmpty) {
      final joined = thinkings.join('\n');
      final open = _openThinking;
      if (open != null) {
        open.text = joined;
      } else {
        entries.add(ChatEntry(kind: ChatEntryKind.thinking, text: joined));
      }
    }
    _openAssistant = null;
    _openThinking = null;
  }

  void _applyUserMessage(dynamic message) {
    if (message is! Map) {
      return;
    }
    final content = message['content'];
    if (content is! List) {
      return;
    }
    for (final block in content) {
      if (block is! Map || block['type'] != 'tool_result') {
        continue;
      }
      final id = block['tool_use_id'] is String
          ? block['tool_use_id'] as String
          : null;
      if (id != null && _todoToolIds.contains(id)) {
        continue; // 待办清单的结果不进对话流（与 v1 同口径）。
      }
      final text = _contentText(block['content']);
      final isError = block['is_error'] == true;
      final entry = _resultEntry(id);
      if (text.isNotEmpty) {
        if (entry.text.isNotEmpty) {
          _append(entry, '\n');
        }
        _append(entry, isError ? '⚠ $text' : text);
      }
    }
  }

  void _applyToolOutput(Map<String, dynamic> event) {
    final chunk = event['chunk'];
    if (chunk is! String || chunk.isEmpty) {
      return;
    }
    final id = event['tool_use_id'] is String
        ? event['tool_use_id'] as String
        : null;
    if (id != null && _todoToolIds.contains(id)) {
      return;
    }
    _append(_resultEntry(id), chunk);
  }

  ChatEntry _resultEntry(String? toolUseId) {
    final existing = toolUseId == null ? null : _toolResults[toolUseId];
    if (existing != null) {
      return existing;
    }
    final entry = ChatEntry(
      kind: ChatEntryKind.toolResult,
      toolUseId: toolUseId,
    );
    entries.add(entry);
    if (toolUseId != null) {
      _toolResults[toolUseId] = entry;
    }
    return entry;
  }

  void _applyControlRequest(Map<String, dynamic> event) {
    final request = event['request'];
    if (request is! Map) {
      return;
    }
    final subtype = request['subtype'] is String
        ? request['subtype'] as String
        : '';
    final requestId = event['request_id'] is String
        ? event['request_id'] as String
        : null;
    // 四类审批共用一条通道，载荷形状不同（见 core-agent 的 open_* 函数）。
    switch (subtype) {
      case 'can_use_tool':
        busy = true;
        entries.add(
          ChatEntry(
            kind: ChatEntryKind.approval,
            subtype: subtype,
            toolName: request['tool_name'] is String
                ? request['tool_name'] as String
                : '工具调用',
            toolUseId: request['tool_use_id'] is String
                ? request['tool_use_id'] as String
                : null,
            requestId: requestId,
            detail: compactJson(request['input']),
            input: request['input'] is Map
                ? Map<String, dynamic>.from(request['input'] as Map)
                : null,
            analysis: request['analysis'] is Map
                ? Map<String, dynamic>.from(request['analysis'] as Map)
                : null,
          ),
        );
      case 'elicitation':
        busy = true;
        entries.add(
          ChatEntry(
            kind: ChatEntryKind.approval,
            subtype: subtype,
            toolName: 'elicitation',
            requestId: requestId,
            elicitMessage: request['message'] is String
                ? request['message'] as String
                : '',
            elicitFields: _parseElicitFields(request['requestedSchema']),
          ),
        );
      case 'mcp_trust':
        busy = true;
        entries.add(
          ChatEntry(
            kind: ChatEntryKind.approval,
            subtype: subtype,
            toolName: 'mcp_trust',
            requestId: requestId,
            trustProject: request['project'] is String
                ? request['project'] as String
                : '',
            trustServers: _parseTrustServers(request['servers']),
          ),
        );
      case 'mcp_sampling':
        busy = true;
        entries.add(
          ChatEntry(
            kind: ChatEntryKind.approval,
            subtype: subtype,
            toolName: 'mcp_sampling',
            requestId: requestId,
            samplingServer: request['server'] is String
                ? request['server'] as String
                : 'MCP',
            samplingMessages: _asInt(request['messageCount']),
            samplingMaxTokens: _asInt(request['maxTokens']),
            samplingViaHttp: request['viaHttp'] == true,
          ),
        );
      default:
        // 未知子类型：如实提示，不假装支持（也不推状态机）。
        addNotice('agent 请求确认：$subtype（该类型审批尚未接入）');
    }
  }

  /// `requestedSchema` → 表单字段（JSON Schema 子集，与 v1 `showElicitationCard` 同口径）。
  List<ElicitField> _parseElicitFields(dynamic schema) {
    if (schema is! Map) {
      return const [];
    }
    final properties = schema['properties'];
    if (properties is! Map) {
      return const [];
    }
    final required = schema['required'] is List
        ? (schema['required'] as List).map((item) => '$item').toList()
        : const <String>[];
    final fields = <ElicitField>[];
    for (final entry in properties.entries) {
      final def = entry.value;
      if (def is! Map) {
        continue;
      }
      final key = '${entry.key}';
      final type = def['type'] is String ? def['type'] as String : 'string';
      final options = def['enum'] is List
          ? (def['enum'] as List).map((item) => '$item').toList()
          : const <String>[];
      final rawDefault = def['default'];
      fields.add(
        ElicitField(
          key: key,
          label: def['title'] is String ? def['title'] as String : key,
          type: type,
          options: options,
          description: def['description'] is String
              ? def['description'] as String
              : null,
          defaultValue: rawDefault is String
              ? rawDefault
              : (rawDefault is num || rawDefault is bool)
              ? '$rawDefault'
              : null,
          required: required.contains(key),
        ),
      );
    }
    return fields;
  }

  List<McpTrustServer> _parseTrustServers(dynamic servers) {
    if (servers is! List) {
      return const [];
    }
    return [
      for (final item in servers)
        if (item is Map)
          McpTrustServer(
            name: item['name'] is String ? item['name'] as String : '',
            transport: item['transport'] is String
                ? item['transport'] as String
                : '',
            target: item['target'] is String ? item['target'] as String : '',
          ),
    ];
  }

  void _cancelApproval(dynamic requestId) {
    if (requestId is! String) {
      return;
    }
    for (final entry in entries) {
      if (entry.kind == ChatEntryKind.approval &&
          entry.requestId == requestId &&
          !entry.handled) {
        entry.handled = true;
        entry.detail = '${entry.detail ?? ''}\n（已取消 / 超时）'.trim();
      }
    }
  }

  /// 把还没答过的审批卡全部作废（agent 重启 / 回合结束）。
  ///
  /// 这些卡的 `request_id` 已经不可能再被接收方认领：留着按钮只会让用户
  /// 「点了没反应」，比直接消失更糟。返回被作废的张数。
  int invalidateApprovals(String note) {
    var count = 0;
    for (final entry in entries) {
      if (entry.kind == ChatEntryKind.approval && !entry.handled) {
        entry.handled = true;
        entry.detail = '${entry.detail ?? ''}\n（$note）'.trim();
        count++;
      }
    }
    return count;
  }

  void _applyResult(Map<String, dynamic> event) {
    busy = false;
    // 回合耗时（信息条显示用）：收口时结算一次，`clear` / 恢复历史各自维护这份表。
    final started = _turnStartedAt;
    if (started != null) {
      turnDurationsMs.add(
        DateTime.now()
            .difference(started)
            .inMilliseconds
            .clamp(0, 86400000)
            .toInt(),
      );
      _turnStartedAt = null;
    }
    // 先用「本轮权威值」结账，再收口条目：即使收尾异常（没有整包文本），金额也要记上。
    _settleUsage(event['usage']);
    // 回合收口：正常时 `_applyAssistant` 已经清过；异常收尾（没有整包）也要清，
    // 否则下一轮的增量会并进上一轮那条半截条目。
    _openAssistant = null;
    _openThinking = null;
    // 准备中的工具卡（§待办 3-28.17）：没有整包时这些表不会自然清空，
    // 留着会让下一轮的 delta 写进上一轮的卡。
    _draftToolCards.clear();
    _toolJsonBuffers.clear();
    _draftByIndex.clear();
    // 回合已经结束 ⇒ 不可能还有能被认领的审批请求。
    invalidateApprovals('本轮已结束');
    final isError =
        event['is_error'] == true ||
        event['subtype'] == 'error_during_execution';
    final text = event['result'];
    if (isError) {
      addError(
        text is String && text.trim().isNotEmpty ? text.trim() : '本轮执行失败',
      );
      return;
    }
    // §待办 3-28.1 的「暂停」：agent 在轮首安全点停下并发 `subtype=paused`。
    // 必须让用户看见「停在哪、怎么继续」，否则和正常收尾一模一样（也会误当成成功）。
    if (event['subtype'] == 'paused') {
      addNotice('已暂停本回合（轮首安全点）。已完成的工具不会重跑，直接发下一条消息即可继续。');
    }
    // 非流式（或模型只给了最终文本）时补一条助手消息。
    if (_assistantSinceUser == 0 && text is String && text.trim().isNotEmpty) {
      final finalText = text.trim();
      entries.add(ChatEntry(kind: ChatEntryKind.assistant, text: finalText));
      messages.add(ChatMessage('assistant', finalText));
      _assistantSinceUser = 1;
    }
  }

  /// 一轮结束结账（v1 `addUsageToTotals`）：优先用 `result.usage`（权威值，含子代理 /
  /// 复盘），拿不到才退回本轮 `usage_delta` 的累计值 —— 回合被取消 / 被强杀时平台照计费，
  /// 账不能一条都不落。
  void _settleUsage(Object? usage) {
    var input = _turnInput;
    var read = _turnRead;
    var create = _turnCreate;
    var output = _turnOutput;
    var requests = _turnRequests;
    if (usage is Map) {
      input = _asInt(usage['input_tokens']);
      read = _asInt(usage['cache_read_input_tokens']);
      create = _asInt(usage['cache_creation_input_tokens']);
      output = _asInt(usage['output_tokens']);
      final perRequest = usage['requests'];
      if (perRequest is List) {
        requests = perRequest.length;
      }
      // 子代理归因：**其中**的一部分（已含在上面四个数里），只累计展示值。
      final subagent = usage['subagent'];
      if (subagent is Map) {
        subagentTokens +=
            _asInt(subagent['input_tokens']) +
            _asInt(subagent['output_tokens']) +
            _asInt(subagent['cache_read_input_tokens']) +
            _asInt(subagent['cache_creation_input_tokens']);
        subagentRequests += _asInt(subagent['requests']);
      }
    }
    hitTokens += read;
    missTokens += input + create;
    cacheCreateTokens += create;
    totalOutputTokens += output;
    requestCount += requests;
    usageRevision++;
    _turnInput = 0;
    _turnRead = 0;
    _turnCreate = 0;
    _turnOutput = 0;
    _turnRequests = 0;
  }

  static void _append(ChatEntry entry, String chunk) {
    final next = entry.text + chunk;
    if (next.length <= _maxEntryChars) {
      entry.text = next;
      return;
    }
    entry.text = '…${next.substring(next.length - _maxEntryChars)}';
  }

  static String _contentText(dynamic content) {
    if (content is String) {
      return content;
    }
    if (content is List) {
      final parts = <String>[];
      for (final block in content) {
        if (block is Map &&
            block['type'] == 'text' &&
            block['text'] is String) {
          parts.add(block['text'] as String);
        }
      }
      return parts.join('\n');
    }
    return '';
  }

  static int _asInt(dynamic value) => value is num ? value.toInt() : 0;
}

/// 工具入参 / 审批详情的单行展示：紧凑 JSON，超长截断。
String compactJson(dynamic value) {
  if (value == null) {
    return '';
  }
  String text;
  try {
    text = value is String ? value : jsonEncode(value);
  } catch (_) {
    text = value.toString();
  }
  final single = text.replaceAll(RegExp(r'\s+'), ' ').trim();
  if (single.length <= 400) {
    return single;
  }
  return '${single.substring(0, 400)}…';
}

/// 「工具卡准备中」时给用户看的一行（§待办 3-28.17）。
///
/// `input_json_delta` 流过来的是**还没闭合的 JSON**，直接 `jsonDecode` 会失败。
/// 这里按优先级做两件事，**解析不出就不猜**：
///   ① 已经闭合 ⇒ 走 `compactJson`，与收完参数后的显示逐字一致；
///   ② 只在半截里找**第一个已闭合的字符串值**（先认命令 / 路径 / 描述这类字段名），
///      用 `jsonDecode` 反转义后截到 120 字 —— 比「准备参数…」有信息量，且绝不编造。
/// 两者都不成立时返回「准备参数…（已收到 N 字）」。
String partialToolDetail(String buffer) {
  final raw = buffer.trim();
  if (raw.isEmpty) {
    return '准备参数…';
  }
  try {
    return compactJson(jsonDecode(raw));
  } catch (_) {
    // 还没闭合，继续往下走。
  }
  const preferred = [
    'command',
    'file_path',
    'path',
    'notebook_path',
    'pattern',
    'query',
    'url',
    'description',
    'prompt',
  ];
  for (final field in preferred) {
    final value = _closedStringValue(raw, field);
    if (value == null) {
      continue;
    }
    final single = value.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (single.isEmpty) {
      continue;
    }
    return single.length <= 120 ? single : '${single.substring(0, 120)}…';
  }
  return '准备参数…（已收到 ${raw.length} 字）';
}

/// 从**半截 JSON** 里取 `"<field>": "<已闭合的字符串>"`；没有就返回 null。
///
/// 只认完整的转义序列：遇到反斜杠时连同下一个字符一起跳过，找第一个未转义的 `"`。
/// 反转义交给 `jsonDecode`（不手写 unescape），解析失败按「没找到」处理。
String? _closedStringValue(String raw, String field) {
  final needle = '"$field"';
  var from = 0;
  while (true) {
    final start = raw.indexOf(needle, from);
    if (start < 0) {
      return null;
    }
    var cursor = start + needle.length;
    // 跳过空白与冒号。
    while (cursor < raw.length &&
        (raw[cursor] == ' ' ||
            raw[cursor] == '\n' ||
            raw[cursor] == '\r' ||
            raw[cursor] == '\t')) {
      cursor++;
    }
    if (cursor >= raw.length || raw[cursor] != ':') {
      from = start + needle.length;
      continue;
    }
    cursor++;
    while (cursor < raw.length &&
        (raw[cursor] == ' ' ||
            raw[cursor] == '\n' ||
            raw[cursor] == '\r' ||
            raw[cursor] == '\t')) {
      cursor++;
    }
    if (cursor >= raw.length || raw[cursor] != '"') {
      from = start + needle.length;
      continue;
    }
    final valueStart = ++cursor;
    var escaped = false;
    while (cursor < raw.length) {
      final ch = raw[cursor];
      if (escaped) {
        escaped = false;
      } else if (ch == r'\') {
        escaped = true;
      } else if (ch == '"') {
        final literal = raw.substring(valueStart, cursor);
        try {
          final decoded = jsonDecode('"$literal"');
          return decoded is String ? decoded : null;
        } catch (_) {
          return null;
        }
      }
      cursor++;
    }
    // 字符串还没闭合（正在流式接收）⇒ 没有可显示的值。
    return null;
  }
}

/// 写类工具 → 入参里的路径字段（与 v1 `WRITE_TOOLS` 同口径）。
///
/// **只有这三类能精确知道改了哪个文件**：`Cmd` / `PowerShell` 一律不推断 ——
/// `cargo fmt` / 脚本 / `git` 都可能改文件，从命令文本猜出来的「已修改」就是撒谎。
const Map<String, String> writeToolPathField = {
  'Write': 'file_path',
  'Edit': 'file_path',
  'NotebookEdit': 'notebook_path',
};

/// 从工具入参 JSON 里取那个路径字段（分片 / 非法 JSON 时返回 null）。
String? toolArgPath(String? detail, String field) {
  if (detail == null || detail.isEmpty) {
    return null;
  }
  try {
    final value = jsonDecode(detail);
    if (value is Map) {
      final raw = value[field];
      if (raw is String && raw.trim().isNotEmpty) {
        return raw.trim();
      }
    }
  } catch (_) {
    // 分片 / 非法 JSON：这一拍先跳过。
  }
  return null;
}

/// 从落盘的过程快照重建一个条目（缺字段 / 未知 kind 一律跳过，不猜）。
ChatEntry? stepEntryOf(dynamic item) {
  if (item is! Map) {
    return null;
  }
  final kind = item['kind'];
  final kindName = kind is String ? kind : '';
  final text = item['text'] is String ? item['text'] as String : '';
  final detail = item['detail'] is String ? item['detail'] as String : null;
  final name = item['name'] is String ? item['name'] as String : null;
  final id = item['id'] is String ? item['id'] as String : null;
  for (final value in ChatEntryKind.values) {
    if (value.name != kindName) {
      continue;
    }
    switch (value) {
      case ChatEntryKind.user:
      case ChatEntryKind.assistant:
      case ChatEntryKind.thinking:
      case ChatEntryKind.notice:
      case ChatEntryKind.error:
        return ChatEntry(kind: value, text: text);
      case ChatEntryKind.toolUse:
        return ChatEntry(
          kind: value,
          toolName: name,
          toolUseId: id,
          detail: detail,
        );
      case ChatEntryKind.toolResult:
        return ChatEntry(kind: value, toolUseId: id, text: text);
      default:
        return null;
    }
  }
  return null;
}

/// 历史会话恢复时的「更改文件」列表：从过程快照里的写类工具还原（v1 `SessionStep.path` 同口径）。
///
/// **只还原列表**，不承诺可还原内容 —— 有没有快照由运行时现查（面板查不到就标「已无快照」）。
List<String> changedFilesFromSteps(
  List<dynamic> steps, {
  String Function(String path)? resolve,
}) {
  final tools = <ChatEntry>[];
  for (final step in steps) {
    final entry = stepEntryOf(step);
    if (entry != null && entry.kind == ChatEntryKind.toolUse) {
      tools.add(entry);
    }
  }
  return changedFilesOf(tools, resolve: resolve);
}

/// 一组成员里**文件类工具**改过的文件（完整路径，按出现顺序去重）。
///
/// 用户要的是「这一轮到底动了哪些文件」，所以只认精确路径，猜错比不显示更糟。
/// [resolve] 用来把工具参数里的相对路径换成绝对路径（历史会话恢复时按当时的 cwd 口径）。
List<String> changedFilesOf(
  Iterable<ChatEntry> tools, {
  String Function(String path)? resolve,
}) {
  final seen = <String>{};
  final files = <String>[];
  for (final tool in tools) {
    final field = writeToolPathField[tool.toolName];
    if (field == null) {
      continue;
    }
    final path = toolArgPath(tool.detail, field);
    if (path == null) {
      continue;
    }
    final full = resolve == null ? path : resolve(path);
    if (seen.add(full.toLowerCase())) {
      files.add(full);
    }
  }
  return files;
}

/// 过程块组头末尾的文件文案：一个文件写**文件名**，多个写「已修改 N 个文件」。
///
/// 没有文件类工具时返回空串（组头只显示命令数）。完整路径不进组头 —— 太长，
/// 由 UI 放进 tooltip。
String changedFilesLabel(List<String> files) {
  if (files.isEmpty) {
    return '';
  }
  if (files.length == 1) {
    return '已修改 ${fileBaseName(files.first)}';
  }
  return '已修改 ${files.length} 个文件';
}

/// 路径的最后一段（`\` 与 `/` 都认）。
String fileBaseName(String path) {
  final parts = path.split(RegExp(r'[\\/]'));
  return parts.isEmpty ? path : parts.last;
}

/// 回合信息条的六类工具（v1 `turnMetaText` 同顺序：创建 / 编辑 / 读取 / 搜索 / 网页 / 命令）。
const List<String> toolCategoryOrder = [
  'create',
  'edit',
  'read',
  'search',
  'web',
  'command',
];

/// 工具名 → 信息条类别；未列入的工具（TodoWrite / MCP 自定义工具等）不计入。
///
/// 只按**工具语义**归类，不看参数内容 —— 归类错了会让「创建 / 编辑几个文件」撒谎。
String? toolCategoryOf(String? toolName) {
  switch (toolName) {
    case 'Write':
      return 'create';
    case 'Edit':
    case 'NotebookEdit':
      return 'edit';
    case 'Read':
      return 'read';
    case 'Grep':
    case 'Glob':
      return 'search';
    case 'WebSearch':
    case 'WebFetch':
      return 'web';
    case 'Cmd':
    case 'PowerShell':
      return 'command';
  }
  return null;
}

/// 计划里的一步（§待办 3-28.3：批准前可逐条开关）。
class PlanStep {
  PlanStep({required this.title, this.enabled = true});

  /// 步骤正文（原计划里去掉项目符号 / 编号之后的那一句）。
  final String title;

  /// 是否保留（关掉的步骤不会交给 agent，也不会出现在留档的计划正文里）。
  bool enabled;

  @override
  String toString() => 'PlanStep($title, enabled: $enabled)';
}

/// 项目符号 / 编号行：`- xxx`、`* xxx`、`1. xxx`、`2) xxx`。
final RegExp _planStepPattern = RegExp(r'^\s*(?:[-*+]|\d{1,2}[.)])\s+(.+)$');

/// 把 `ExitPlanMode` 的计划文本拆成可编辑的步骤。
///
/// 优先认项目符号 / 编号行（计划的常见写法，标题与说明段落不算步骤）；
/// 一条都认不出来时退化成「每个非空、非标题行是一步」—— 卡片上永远有东西可编辑，
/// 不会出现「计划有正文但一个开关都没有」。
List<PlanStep> planStepsOf(String plan) {
  final numbered = <PlanStep>[];
  for (final line in plan.split(RegExp(r'\r?\n'))) {
    final match = _planStepPattern.firstMatch(line);
    if (match == null) {
      continue;
    }
    final title = match.group(1)!.trim();
    if (title.isNotEmpty) {
      numbered.add(PlanStep(title: title));
    }
  }
  if (numbered.isNotEmpty) {
    return numbered;
  }
  return [
    for (final line in plan.split(RegExp(r'\r?\n')))
      if (line.trim().isNotEmpty && !line.trim().startsWith('#'))
        PlanStep(title: line.trim()),
  ];
}

/// 编辑后的计划正文：只保留开着的步骤，按 1..N 重新编号，末尾附一行备注。
String editedPlanText(List<PlanStep> steps, {String note = ''}) {
  final kept = steps.where((step) => step.enabled).toList();
  final buffer = StringBuffer();
  for (var i = 0; i < kept.length; i++) {
    buffer.writeln('${i + 1}. ${kept[i].title}');
  }
  if (note.trim().isNotEmpty) {
    if (buffer.isNotEmpty) {
      buffer.writeln();
    }
    buffer.writeln('补充说明：${note.trim()}');
  }
  return buffer.toString().trimRight();
}

/// 把一轮里的工具调用按六类归组（保持出现顺序；不认识的一律丢掉）。
Map<String, List<ChatEntry>> groupToolsByCategory(Iterable<ChatEntry> tools) {
  final groups = <String, List<ChatEntry>>{};
  for (final tool in tools) {
    final category = toolCategoryOf(tool.toolName);
    if (category == null) {
      continue;
    }
    (groups[category] ??= <ChatEntry>[]).add(tool);
  }
  return groups;
}
