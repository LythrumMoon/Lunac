// 平台薄层：M2 对话主链路 —— agent 进程与 stream-json 通道。
//
// 边界（docs/flutter-migration.md §一 / M2）：
//   · 进程、参数与凭据注入都在 Rust（`rust/src/agent.rs`），协议与 v1 完全一致；
//   · 本文件只做四件事：启动 / 停止、写一行 JSON、轮询事件、拼协议报文；
//   · 事件语义（文本增量 / 工具卡 / 审批卡）由 `search/chat_model.dart` 解释。
//
// 轮询：每 40ms 调一次 `lunac_agent_poll`（Rust 侧只是一次队列读取，队列空时无 IO）；
// 没有监听者时定时器停掉，后台不空转。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;

import 'native_bridge.dart';
import 'paths.dart';

typedef _StartC = Pointer<Utf8> Function();
typedef _StartDart = Pointer<Utf8> Function();
typedef _StopC = Void Function();
typedef _StopDart = void Function();
typedef _RunningC = Int32 Function();
typedef _RunningDart = int Function();
typedef _SendC = Int32 Function(Pointer<Utf8>);
typedef _SendDart = int Function(Pointer<Utf8>);
typedef _PollC = Pointer<Utf8> Function();
typedef _PollDart = Pointer<Utf8> Function();
typedef _AiConfigC = Pointer<Utf8> Function();
typedef _AiConfigDart = Pointer<Utf8> Function();
typedef _SetAiConfigC = Int32 Function(Pointer<Utf8>);
typedef _SetAiConfigDart = int Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 轮询间隔：40ms ≈ 25fps 的流式观感，队列空时开销可忽略。
const Duration _pollInterval = Duration(milliseconds: 40);

final StreamController<Map<String, dynamic>> _events =
    StreamController<Map<String, dynamic>>.broadcast(
  onListen: _startPolling,
  onCancel: _stopPolling,
);

Timer? _pollTimer;
DynamicLibrary? _library;
PollFn? _poll;
FreeFn? _free;

typedef PollFn = Pointer<Utf8> Function();
typedef FreeFn = void Function(Pointer<Utf8>);

/// 一条 stream-json 事件（stdout 的一行 JSON）。
Stream<Map<String, dynamic>> get agentEvents => _events.stream;

/// 启动常驻 agent（幂等）。返回 null = 已启动；否则是可读的失败原因。
String? startAgent() {
  final opened = openLunacNative();
  final library = opened.library;
  if (library == null) {
    return opened.reason ?? 'rust bridge not loaded';
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_StartC, _StartDart>('lunac_agent_start')();
    if (raw == nullptr) {
      return 'agent 启动失败（空返回）';
    }
    final reason = raw.toDartString();
    return reason.isEmpty ? null : reason;
  } catch (e) {
    return 'agent 启动失败（$e）';
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 停掉 agent（退出应用时调用；只关对话面板不调，保持常驻与 v1 一致）。
void stopAgent() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<_StopC, _StopDart>('lunac_agent_stop')();
  } catch (_) {
    // 旧版库没有该符号：忽略。
  }
}

/// agent 是否在运行。
bool get agentRunning {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    return library.lookupFunction<_RunningC, _RunningDart>(
          'lunac_agent_running',
        )() ==
        1;
  } catch (_) {
    return false;
  }
}

/// 往 agent 的 stdin 写一行 JSON（Rust 补换行）。
bool sendAgentLine(String line) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final send = library.lookupFunction<_SendC, _SendDart>('lunac_agent_send');
    pointer = line.toNativeUtf8();
    return send(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 发送一条用户消息（协议与 v1 `startAIChat` 一致）。
///
/// 非图片附件拼进提示词（`[Attached files]` + `[User query]`）；图片附件在 [vision]
/// 为真时**额外**追加 `image` 块（只带路径，agent 自己读字节并转 base64）——
/// 与 v1 一致，图片路径同时保留在提示词文本里。
Map<String, Object?> userMessagePayload(
  String text, {
  List<String> attachments = const [],
  bool vision = true,
  bool steerInsert = false,
}) {
  final body = attachments.isEmpty
      ? text
      : '[Attached files]\n'
          '${attachments.map((path) => '- $path').join('\n')}\n\n'
          '[User query]\n$text';
  final images = vision ? attachments.where(isImagePath) : const <String>[];
  return {
    'type': 'user',
    'session_id': '',
    // §待办 3-28.1 的「立即插入」：只有显式带 `steer:"insert"` 的消息才会被
    // agent 在**轮首安全点**注入 history（工具批中途不插，否则拆坏配对）；
    // 不带这个键 = 排队（回合结束后照原语义执行）。
    if (steerInsert) 'steer': 'insert',
    'message': {
      'role': 'user',
      'content': [
        {'type': 'text', 'text': body},
        for (final path in images)
          {
            'type': 'image',
            'source': {'type': 'file', 'path': path},
          },
      ],
    },
    'parent_tool_use_id': null,
  };
}

/// 审批回包（`control_response`）；`allow=false` 时带默认拒因。
///
/// `updatedInput` 非空 = 覆盖工具入参。四类审批共用这条通道：
/// `can_use_tool` 传空（按原参数执行）、`elicitation` 传表单值、
/// `mcp_trust` 传 `{'remember': true}`（始终信任）。
Map<String, Object?> approvalResponse(
  String requestId,
  String toolUseId, {
  required bool allow,
  String? message,
  Map<String, Object?>? updatedInput,
  bool interrupt = false,
}) {
  return {
    'type': 'control_response',
    'response': {
      'subtype': 'success',
      'request_id': requestId,
      'response': allow
          ? {
              'behavior': 'allow',
              'updatedInput': updatedInput ?? <String, Object?>{},
              'toolUseID': toolUseId,
            }
          : {
              'behavior': 'deny',
              'message': message ?? 'User denied this action in Lunac',
              'interrupt': interrupt,
              'toolUseID': toolUseId,
            },
    },
  };
}

/// 暂停当前回合（v1 的「停止」）。
Map<String, Object?> pausePayload() => {'type': 'pause'};

/// 对**正在执行**的命令下一条实时指令（v1 `sendToolControl`）。
///
/// `action`：`background` = 转后台；`stop` = 杀掉；`stdin` = 往子进程 stdin 灌字符
/// （v2 暂未接终端，只发前两种）。agent 侧按 `tool_use_id` 找回那条命令，
/// 这两种动作都会让模型的 tool_result 变成「已转后台 / 已被用户停止」。
Map<String, Object?> toolControlPayload(
  String toolUseId,
  String action, {
  String? data,
}) => {
  'type': 'tool_control',
  'action': action,
  'tool_use_id': toolUseId,
  'data': ?data,
};

/// 终止一个**后台子代理**（§待办 3-28.7）。
///
/// 与 `toolControlPayload` 分开：后台子代理按 `task_id` 寻址（`task-3`），不是
/// `tool_use_id` —— 它已经活过了发起它的那次工具调用。agent 收到后在下一个检查点
/// （下一轮开头 / 下一件工具之前）收工，**不会**硬中断正在飞的 API 请求。
Map<String, Object?> stopBackgroundTaskPayload(String taskId) => {
  'type': 'tool_control',
  'action': 'stop_task',
  'task_id': taskId,
};

/// v1 xterm 的逐键输入 → 子进程 stdin 数据（纯函数；没有映射时返回 null）。
///
/// 与 xterm.js 的默认键位一致：可打印字符原样、Enter `\r`、Backspace `\b`、
/// Tab `\t`、Esc `\u001b`、方向键 / Home / End / Delete 发 ANSI 序列。
String? terminalKeyData({
  required LogicalKeyboardKey key,
  String? character,
}) {
  if (key == LogicalKeyboardKey.enter || key == LogicalKeyboardKey.numpadEnter) {
    return '\r';
  }
  if (key == LogicalKeyboardKey.backspace) {
    return '\b';
  }
  if (key == LogicalKeyboardKey.tab) {
    return '\t';
  }
  if (key == LogicalKeyboardKey.escape) {
    return '\u001b';
  }
  if (key == LogicalKeyboardKey.arrowUp) {
    return '\u001b[A';
  }
  if (key == LogicalKeyboardKey.arrowDown) {
    return '\u001b[B';
  }
  if (key == LogicalKeyboardKey.arrowRight) {
    return '\u001b[C';
  }
  if (key == LogicalKeyboardKey.arrowLeft) {
    return '\u001b[D';
  }
  if (key == LogicalKeyboardKey.home) {
    return '\u001b[H';
  }
  if (key == LogicalKeyboardKey.end) {
    return '\u001b[F';
  }
  if (key == LogicalKeyboardKey.delete) {
    return '\u001b[3~';
  }
  if (character == null ||
      character.isEmpty ||
      character.contains(RegExp(r'[\x00-\x1f\x7f]'))) {
    return null;
  }
  return character;
}

/// 手动压缩一次上下文（`/compact`；不是一次提问，不产生回答）。
Map<String, Object?> compactPayload() => {'type': 'compact'};

/// 回灌 / 替换会话历史（v1 `set_history` 同形，不触发模型调用）。
///
/// 恢复旧会话时先发这一条，agent 的上下文才和界面显示一致。
Map<String, Object?> setHistoryPayload(List<Map<String, String>> messages) => {
      'type': 'set_history',
      'messages': messages,
    };

// ── AI 配置（设置面板用）────────────────────────────────────────

/// 读 AI 配置原始 JSON（没有配置文件 = 空 Map）。
///
/// 字段与 v1 `<exe 根>/config/ai.json` 同名：`provider` / `url` / `key` / `model` /
/// `agent_url` / `vision` / `search_provider` / `search_key` …
Map<String, dynamic> readAiConfig() {
  final library = openLunacNative().library;
  if (library == null) {
    return const {};
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_AiConfigC, _AiConfigDart>('lunac_ai_config')();
    if (raw == nullptr) {
      return const {};
    }
    final value = jsonDecode(raw.toDartString());
    return value is Map<String, dynamic> ? value : const {};
  } catch (_) {
    return const {};
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 保存 AI 配置（与现有文件合并）。返回 null = 已保存，否则是可读原因。
String? saveAiConfig(Map<String, Object?> config) {
  final library = openLunacNative().library;
  if (library == null) {
    return 'rust bridge not loaded';
  }
  Pointer<Utf8>? pointer;
  try {
    final save =
        library.lookupFunction<_SetAiConfigC, _SetAiConfigDart>('lunac_set_ai_config');
    pointer = jsonEncode(config).toNativeUtf8();
    final code = save(pointer);
    return switch (code) {
      0 => null,
      1 => '配置格式非法',
      _ => '写入 config/ai.json 失败',
    };
  } catch (e) {
    return '保存失败（$e）';
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

// ── 对话「更多设置」（v1 localStorage 的 v2 对应物）────────────────────
//
// 落点：`<exe 根>/config/chat.json`（thinking / runMode / profile / toolBlacklist）
// 与 `config/persona.md`（人格原文）。agent 在 spawn 时读取，所以保存后要重启才生效。

typedef _ChatSettingsC = Pointer<Utf8> Function();
typedef _ChatSettingsDart = Pointer<Utf8> Function();
typedef _SetChatSettingsC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SetChatSettingsDart = Pointer<Utf8> Function(Pointer<Utf8>);

/// 对话设置快照（字段与 `chat_settings::snapshot_json` 一一对应）。
class ChatSettings {
  const ChatSettings({
    this.thinking = true,
    this.runMode = 'allowlist',
    this.profile = 'project',
    this.foldTiming = 'reply',
    this.secondReview = false,
    this.skillMemory = false,
    this.ptcMode = false,
    this.toolBlacklist = const {},
    this.toolCandidates = const [],
    this.persona = '',
    this.personaMaxChars = 8000,
    this.workspace = '',
    this.workspaceConfigured = false,
    this.defaultWorkspace = '',
  });

  final bool thinking;

  /// `manual` / `allowlist` / `auto`（命令审批方式）。
  final String runMode;

  /// `safe` / `project` / `full`（权限 / 文件边界）。
  final String profile;

  /// `reply` / `nextMessage`（工作细节的折叠时机，§待办 3-28.17）。
  final String foldTiming;

  /// 第二模型预审（§待办 3-28.14）：对应 agent 的 `LUNAC_SECOND_REVIEW`。
  /// 打开后「本来就要弹卡」的调用会先过一遍预审模型 —— 只可能被拒或被升级到人工卡。
  final bool secondReview;

  /// 技能级记忆（§待办 3-28.10 ①）：对应 agent 的 `LUNAC_SKILL_MEMORY`。
  /// 打开后，某个技能 fork 出来的子代理会读到 `ModuleData/memory/skills/<key>/MEMORY.md`，
  /// 它写的 `Remember` 也落在这个技能自己的命名空间（不进主对话前缀）。
  final bool skillMemory;

  /// PTC / Code Mode（§待办 3-28.15）：对应 agent 的 `LUNAC_PTC`。
  /// 关着时 `RunScript` 根本不出现在工具表里。
  final bool ptcMode;

  final Set<String> toolBlacklist;
  final List<String> toolCandidates;
  final String persona;
  final int personaMaxChars;

  /// **生效中的**工作区（用户选过就是它，否则是应用数据目录）。
  final String workspace;

  /// 工作区是否为用户选定（只有 `true` 才启用 agent 侧的工作区锁）。
  final bool workspaceConfigured;

  /// 没选工作区时用的默认目录（面板上显示「默认」时用它做说明）。
  final String defaultWorkspace;

  /// 权限档的展示名（v1 `settings.security_profile_*`）。
  String get profileLabel => switch (profile) {
    'safe' => '只读',
    'full' => '系统',
    _ => '工作区',
  };

  String get runModeLabel => switch (runMode) {
    'manual' => '手动',
    'auto' => '自动',
    _ => '白名单',
  };

  /// 折叠时机的展示名（§待办 3-28.17）。
  String get foldTimingLabel =>
      foldTiming == 'nextMessage' ? '下一条消息' : '回复结束';

  /// 功能简述（v1 `agent.run_mode_*_hint`；自动档用警示文案）。
  String get runModeHint => switch (runMode) {
    'manual' => '每次工具调用都先询问（含白名单命中）',
    'auto' => '自动审批中：不再询问；「系统」权限会忽略工作区限制',
    _ => '只读操作自动放行，其余先询问',
  };

  factory ChatSettings.fromJson(Map<String, dynamic> json) {
    final candidates = json['toolCandidates'];
    final blacklist = json['toolBlacklist'];
    return ChatSettings(
      thinking: json['thinking'] is bool ? json['thinking'] as bool : true,
      runMode: json['runMode'] is String
          ? json['runMode'] as String
          : 'allowlist',
      profile: json['profile'] is String ? json['profile'] as String : 'project',
      foldTiming: json['foldTiming'] is String
          ? json['foldTiming'] as String
          : 'reply',
      secondReview: json['secondReview'] is bool
          ? json['secondReview'] as bool
          : false,
      skillMemory: json['skillMemory'] is bool
          ? json['skillMemory'] as bool
          : false,
      ptcMode: json['ptcMode'] is bool ? json['ptcMode'] as bool : false,
      toolBlacklist: {
        if (blacklist is List)
          for (final item in blacklist)
            if (item is String && item.isNotEmpty) item,
      },
      toolCandidates: [
        if (candidates is List)
          for (final item in candidates)
            if (item is String) item,
      ],
      persona: json['persona'] is String ? json['persona'] as String : '',
      personaMaxChars: json['personaMaxChars'] is int
          ? json['personaMaxChars'] as int
          : 8000,
      workspace: json['workspace'] is String ? json['workspace'] as String : '',
      workspaceConfigured: json['workspaceConfigured'] == true,
      defaultWorkspace: json['defaultWorkspace'] is String
          ? json['defaultWorkspace'] as String
          : '',
    );
  }
}

/// 弹系统的「选择文件夹」（AI 工作区）；取消 / 桥接缺失返回空串。
String pickWorkspaceDirectory() {
  final library = openLunacNative().library;
  if (library == null) {
    return '';
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_ChatSettingsC, _ChatSettingsDart>(
      'lunac_pick_workspace',
    )();
    if (raw == nullptr) {
      return '';
    }
    return raw.toDartString().trim();
  } catch (_) {
    return '';
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 读对话设置；没有桥接（单测 / 非 Windows）时给缺省值。
ChatSettings readChatSettings() {
  final library = openLunacNative().library;
  if (library == null) {
    return const ChatSettings();
  }
  Pointer<Utf8>? raw;
  try {
    raw =
        library.lookupFunction<_ChatSettingsC, _ChatSettingsDart>(
          'lunac_chat_settings',
        )();
    if (raw == nullptr) {
      return const ChatSettings();
    }
    final value = jsonDecode(raw.toDartString());
    return value is Map<String, dynamic>
        ? ChatSettings.fromJson(value)
        : const ChatSettings();
  } catch (_) {
    return const ChatSettings();
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 合并保存对话设置；返回 null = 已保存，否则是一句人话。
String? saveChatSettings(Map<String, Object?> patch) {
  final library = openLunacNative().library;
  if (library == null) {
    return 'rust bridge not loaded';
  }
  Pointer<Utf8>? pointer;
  Pointer<Utf8>? raw;
  try {
    final save = library.lookupFunction<_SetChatSettingsC, _SetChatSettingsDart>(
      'lunac_chat_set_settings',
    );
    pointer = jsonEncode(patch).toNativeUtf8();
    raw = save(pointer);
    return _readOk(raw);
  } catch (e) {
    return '保存失败（$e）';
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
    final result = raw;
    if (result != null && result != nullptr) {
      _freeQuietly(library, result);
    }
  }
}

/// 保存人格（空文本 = 恢复内置人格）；返回 null = 已保存。
String? savePersona(String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return 'rust bridge not loaded';
  }
  Pointer<Utf8>? pointer;
  Pointer<Utf8>? raw;
  try {
    final save = library.lookupFunction<_SetChatSettingsC, _SetChatSettingsDart>(
      'lunac_set_persona',
    );
    pointer = text.toNativeUtf8();
    raw = save(pointer);
    return _readOk(raw);
  } catch (e) {
    return '保存失败（$e）';
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
    final result = raw;
    if (result != null && result != nullptr) {
      _freeQuietly(library, result);
    }
  }
}

/// `{"ok":true}` / `{"ok":false,"error":"…"}` → null / 一句人话。
String? _readOk(Pointer<Utf8> raw) {
  if (raw == nullptr) {
    return '保存失败（空返回）';
  }
  try {
    final value = jsonDecode(raw.toDartString());
    if (value is Map && value['ok'] == true) {
      return null;
    }
    final error = value is Map ? value['error'] : null;
    return error is String && error.isNotEmpty ? error : '保存失败';
  } catch (_) {
    return '保存失败（返回不可解析）';
  }
}

// ── 事件轮询 ────────────────────────────────────────────────────

void _startPolling() {
  _pollTimer ??= Timer.periodic(_pollInterval, (_) => _drain());
}

void _stopPolling() {
  _pollTimer?.cancel();
  _pollTimer = null;
}

bool _ensureApi() {
  if (_poll != null && _free != null) {
    return true;
  }
  final library = _library ?? openLunacNative().library;
  if (library == null) {
    return false;
  }
  _library = library;
  try {
    _poll = library.lookupFunction<_PollC, _PollDart>('lunac_agent_poll');
    _free = library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string');
    return true;
  } catch (_) {
    _poll = null;
    _free = null;
    return false;
  }
}

void _drain() {
  for (var i = 0; i < 200; i++) {
    final event = _popEvent();
    if (event == null) {
      return;
    }
    _events.add(event);
  }
}

/// 同步取走积压事件并交给 [handler]。
///
/// 面板关闭期间 agent 仍可能在跑：再打开时先把这段事件补齐，界面才接得上。
void drainAgentEvents(void Function(Map<String, dynamic> event) handler) {
  for (var i = 0; i < 4000; i++) {
    final event = _popEvent();
    if (event == null) {
      return;
    }
    handler(event);
  }
}

Map<String, dynamic>? _popEvent() {
  if (!_ensureApi()) {
    return null;
  }
  final Pointer<Utf8> raw;
  try {
    raw = _poll!();
  } catch (_) {
    return null;
  }
  if (raw == nullptr) {
    return null;
  }
  final text = raw.toDartString();
  try {
    _free!(raw);
  } catch (_) {
    // 释放失败不影响解析；ABI 检查已保证符号存在。
  }
  if (text.isEmpty) {
    return null;
  }
  return _decode(text);
}

Map<String, dynamic>? _decode(String text) {
  try {
    final value = jsonDecode(text);
    return value is Map<String, dynamic> ? value : null;
  } catch (_) {
    return null;
  }
}

void _freeQuietly(DynamicLibrary library, Pointer<Utf8> pointer) {
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}
