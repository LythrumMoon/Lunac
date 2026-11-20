// 平台薄层：对话历史的读写（v2 自管，见 `rust/src/chat_history.rs`）。
//
// 存的是会话快照：`id / title / createdAt / updatedAt / messages[{role,content}] /
// steps[]`（`messages` 与 v1 `ChatSession` 同名同义；`steps` 是过程快照 ——
// 思考块 / 工具卡 / 工具输出 / 已修改的文件，见 `ChatModel.stepsJson`）。
// 审批卡只对当前进程有效，不进历史；恢复时把消息回灌给 agent（`set_history`），
// 过程块按 `steps` 原顺序重建。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import '../search/chat_model.dart';
import 'native_bridge.dart';

typedef _LoadC = Pointer<Utf8> Function();
typedef _LoadDart = Pointer<Utf8> Function();
typedef _SaveC = Int32 Function(Pointer<Utf8>);
typedef _SaveDart = int Function(Pointer<Utf8>);
typedef _DeleteC = Int32 Function(Pointer<Utf8>);
typedef _DeleteDart = int Function(Pointer<Utf8>);
typedef _SessionC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SessionDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 一条历史消息（只保留 role / content 纯文本）。
class ChatHistoryMessage {
  const ChatHistoryMessage(this.role, this.content);

  final String role;
  final String content;

  static ChatHistoryMessage? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final role = json['role'];
    final content = json['content'];
    if (role is! String || content is! String) {
      return null;
    }
    return ChatHistoryMessage(role, content);
  }
}

/// 历史列表的一行元数据（不含 messages / steps，首屏只读这一份）。
class ChatHistoryMeta {
  const ChatHistoryMeta({
    required this.id,
    required this.title,
    required this.createdAt,
    required this.updatedAt,
    required this.messageCount,
  });

  final String id;
  final String title;
  final DateTime? createdAt;
  final DateTime? updatedAt;
  final int messageCount;

  static ChatHistoryMeta? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final id = json['id'];
    if (id is! String || id.isEmpty) {
      return null;
    }
    DateTime? time(dynamic value) => value is int && value > 0
        ? DateTime.fromMillisecondsSinceEpoch(value)
        : null;
    return ChatHistoryMeta(
      id: id,
      title: json['title'] is String ? json['title'] as String : '未命名对话',
      createdAt: time(json['createdAt']),
      updatedAt: time(json['updatedAt']),
      messageCount: json['messageCount'] is num
          ? (json['messageCount'] as num).toInt()
          : 0,
    );
  }
}

/// 一条历史会话。
class ChatHistorySession {
  const ChatHistorySession({
    required this.id,
    required this.title,
    required this.createdAt,
    required this.updatedAt,
    required this.messages,
    this.steps = const [],
    this.turnDurations = const [],
    this.systemChars = 0,
    this.systemSha = '',
    this.contextPresent = false,
    this.contextChars = 0,
    this.contextSha = '',
    this.promptTimeline = const [],
  });

  final String id;
  final String title;
  final DateTime? createdAt;
  final DateTime? updatedAt;
  final List<ChatHistoryMessage> messages;

  /// 过程快照（按回合分组的思考 / 工具调用，见 `ChatModel.stepsJson`）。
  /// 旧记录没有该字段 ⇒ 空列表（正文照常恢复，只是没有过程块）。
  final List<dynamic> steps;

  /// 每轮墙钟耗时（毫秒，按回合序号；回合信息条的「耗时」用）。
  final List<int> turnDurations;

  /// SYSTEM / CONTEXT 两栏（§待办 3-28.12 ①）：只有字符数与哈希，没有正文。
  final int systemChars;
  final String systemSha;
  final bool contextPresent;
  final int contextChars;
  final String contextSha;

  /// 逐轮 SYSTEM / CONTEXT 时间线（§待办 3-28.12 ①）。
  ///
  /// 老会话（只有 `system` / `context` 两个键）在这里合成**一条**快照 —— 不假装
  /// 有逐轮数据；新会话由 `ChatModel.historyJson()` 写完整的 `promptTimeline`。
  final List<PromptSnapshot> promptTimeline;

  static ChatHistorySession? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final id = json['id'];
    if (id is! String || id.isEmpty) {
      return null;
    }
    final system = json['system'];
    final context = json['context'];
    int chars(dynamic raw) => raw is num ? raw.toInt() : 0;
    String sha(dynamic raw) => raw is String ? raw : '';
    final timeline = <PromptSnapshot>[
      for (final item in (json['promptTimeline'] as List? ?? const []))
        ?PromptSnapshot.fromJson(item),
    ];
    // 老会话没有时间线：把最新一次快照合成一条，界面与导出仍能看到同样的信息。
    if (timeline.isEmpty &&
        ((system is Map && sha(system['sha256']).isNotEmpty) ||
            (context is Map && context['present'] == true))) {
      timeline.add(
        PromptSnapshot(
          turn: 1,
          systemChars: system is Map ? chars(system['chars']) : 0,
          systemSha: system is Map ? sha(system['sha256']) : '',
          contextPresent: context is Map && context['present'] == true,
          contextChars: context is Map ? chars(context['chars']) : 0,
          contextSha: context is Map ? sha(context['sha256']) : '',
        ),
      );
    }
    return ChatHistorySession(
      id: id,
      title: json['title'] is String ? json['title'] as String : '未命名对话',
      createdAt: _time(json['createdAt']),
      updatedAt: _time(json['updatedAt']),
      messages: [
        for (final item in (json['messages'] as List? ?? const []))
          ?ChatHistoryMessage.fromJson(item),
      ],
      steps: json['steps'] is List ? json['steps'] as List : const [],
      turnDurations: [
        for (final item in (json['turnDurations'] as List? ?? const []))
          if (item is num) item.toInt(),
      ],
      systemChars: system is Map ? chars(system['chars']) : 0,
      systemSha: system is Map ? sha(system['sha256']) : '',
      contextPresent: context is Map && context['present'] == true,
      contextChars: context is Map ? chars(context['chars']) : 0,
      contextSha: context is Map ? sha(context['sha256']) : '',
      promptTimeline: timeline,
    );
  }

  static DateTime? _time(dynamic value) => value is int && value > 0
      ? DateTime.fromMillisecondsSinceEpoch(value)
      : null;
}

/// 读全部历史（按最后活动降序；文件不存在 = 空列表）。
List<ChatHistorySession> loadChatHistory() {
  final library = openLunacNative().library;
  if (library == null) {
    return const [];
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_LoadC, _LoadDart>('lunac_chat_history')();
    if (raw == nullptr) {
      return const [];
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return const [];
    }
    final sessions = [
      for (final item in value) ?ChatHistorySession.fromJson(item),
    ];
    sessions.sort((a, b) {
      final left = (a.updatedAt ?? a.createdAt)?.millisecondsSinceEpoch ?? 0;
      final right = (b.updatedAt ?? b.createdAt)?.millisecondsSinceEpoch ?? 0;
      return right.compareTo(left);
    });
    return sessions;
  } catch (_) {
    return const [];
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 读历史列表元数据（轻量；点开某条时才读完整 payload）。
List<ChatHistoryMeta> loadChatHistoryMeta() {
  final library = openLunacNative().library;
  if (library == null) {
    return const [];
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_LoadC, _LoadDart>(
      'lunac_chat_history_meta',
    )();
    if (raw == nullptr) {
      return const [];
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return const [];
    }
    return [for (final item in value) ?ChatHistoryMeta.fromJson(item)];
  } catch (_) {
    return const [];
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 按 id 读完整会话；不存在 / 桥接缺失 = null。
ChatHistorySession? loadChatHistorySession(String id) {
  if (id.isEmpty) {
    return null;
  }
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    input = id.toNativeUtf8();
    raw = library.lookupFunction<_SessionC, _SessionDart>(
      'lunac_chat_history_session',
    )(input);
    if (raw == nullptr) {
      return null;
    }
    final text = raw.toDartString();
    if (text == 'null') {
      return null;
    }
    return ChatHistorySession.fromJson(jsonDecode(text));
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeQuietly(library, pointer);
    }
  }
}

/// 写入 / 覆盖一条会话；返回 false = 桥接缺失或写失败。
bool saveChatHistory(Map<String, Object?> session) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final save = library.lookupFunction<_SaveC, _SaveDart>(
      'lunac_chat_history_save',
    );
    pointer = jsonEncode(session).toNativeUtf8();
    return save(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 删除一条会话；不存在也算成功。
bool deleteChatHistory(String id) {
  if (id.isEmpty) {
    return false;
  }
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final remove = library.lookupFunction<_DeleteC, _DeleteDart>(
      'lunac_chat_history_delete',
    );
    pointer = id.toNativeUtf8();
    return remove(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

void _freeQuietly(DynamicLibrary library, Pointer<Utf8> pointer) {
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}
