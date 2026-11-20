// 平台薄层：插件运行时（sidecar）—— v1 `plugin_sidecar.rs` 的 v2 等价物。
//
// 它是什么：插件目录里带一个**自己的可执行文件**（Python / Node / Go / Rust 都行），
// Lunac 负责「起进程 + NDJSON 通信 + 收尾」，界面由宿主画（`plugin_console_panel.dart`）。
// 这就是「AI 开发的插件可以用其他框架 / 语言」的落点；**不是沙箱**，信任卡是
// 「如实告知 + 用户裁决」。
//
// 纪律：
//   · 起进程 / 等回包 / 代请求都会阻塞 ⇒ 一律 `Isolate.run`，不冻 UI；
//   · 读状态（只读清单）与拉事件（读内存里的环形日志）是同步的，开销可忽略；
//   · 坏 JSON / 桥接缺失 → 明确状态或一句人话，**不抛异常**。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _String1C = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _String1Dart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _String2C = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _String2Dart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _String3C = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _String3Dart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _HttpC = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Uint16,
);
typedef _HttpDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
  int,
);
typedef _TrustC = Pointer<Utf8> Function(Pointer<Utf8>, Uint8);
typedef _TrustDart = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _EventsC = Pointer<Utf8> Function(Pointer<Utf8>, Uint64);
typedef _EventsDart = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _String0C = Pointer<Utf8> Function();
typedef _String0Dart = Pointer<Utf8> Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

// ── 数据形状 ──────────────────────────────────────────────────────

/// sidecar 清单（信任卡上原样展示；与 v1 `SidecarSpec` 同字段）。
class PluginSidecarSpec {
  const PluginSidecarSpec({
    required this.command,
    required this.args,
    required this.transport,
    required this.port,
    required this.cwd,
    required this.autostart,
    required this.restart,
    required this.sha256,
    required this.allowLocalPorts,
    required this.allowLocalAny,
    required this.readyTimeoutSec,
    required this.memoryLimitMiB,
    required this.processLimit,
  });

  final String command;
  final List<String> args;
  final String transport;
  final int port;
  final String cwd;
  final bool autostart;
  final String restart;
  final String sha256;
  final List<int> allowLocalPorts;
  final bool allowLocalAny;
  final int readyTimeoutSec;
  final int memoryLimitMiB;
  final int processLimit;

  /// `http` / `both` 才有本机端口。
  bool get usesHttp => transport == 'http' || transport == 'both';

  static PluginSidecarSpec? fromJson(Object? raw) {
    if (raw is! Map) {
      return null;
    }
    return PluginSidecarSpec(
      command: raw['command'] is String ? raw['command'] as String : '',
      args: [
        for (final a in (raw['args'] as List? ?? const []))
          if (a is String) a,
      ],
      transport: raw['transport'] is String ? raw['transport'] as String : '',
      port: raw['port'] is int ? raw['port'] as int : 0,
      cwd: raw['cwd'] is String ? raw['cwd'] as String : '',
      autostart: raw['autostart'] == true,
      restart: raw['restart'] is String ? raw['restart'] as String : '',
      sha256: raw['sha256'] is String ? raw['sha256'] as String : '',
      allowLocalPorts: [
        for (final p in (raw['allowLocalPorts'] as List? ?? const []))
          if (p is int) p,
      ],
      allowLocalAny: raw['allowLocalAny'] == true,
      readyTimeoutSec: raw['readyTimeoutSec'] is int
          ? raw['readyTimeoutSec'] as int
          : 10,
      // 后端键名沿用 v1 的大写 B；这里两种都认，避免界面因键名差异空白。
      memoryLimitMiB: raw['memoryLimitMiB'] is int
          ? raw['memoryLimitMiB'] as int
          : (raw['memoryLimitMib'] is int ? raw['memoryLimitMib'] as int : 0),
      processLimit: raw['processLimit'] is int ? raw['processLimit'] as int : 0,
    );
  }
}

/// 运行时状态（`lunac_plugin_runtime_status`）。
class PluginSidecarStatus {
  const PluginSidecarStatus({
    required this.ok,
    required this.pluginId,
    required this.hasSidecar,
    this.spec,
    this.dir = '',
    this.trusted = false,
    this.denied = false,
    this.running = false,
    this.port,
    this.exitReason = '',
    this.cursor = 0,
    this.error = '',
  });

  final bool ok;
  final String pluginId;
  final bool hasSidecar;
  final PluginSidecarSpec? spec;
  final String dir;
  final bool trusted;
  final bool denied;
  final bool running;

  /// http 通道端口（stdio-only 时为 null）。
  final int? port;

  /// 最近一次退出原因（空 = 没退过 / 已重启）。
  final String exitReason;
  final int cursor;
  final String error;

  static PluginSidecarStatus fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const PluginSidecarStatus(
        ok: false,
        pluginId: '',
        hasSidecar: false,
        error: '运行时状态不是合法 JSON',
      );
    }
    if (decoded is! Map) {
      return const PluginSidecarStatus(
        ok: false,
        pluginId: '',
        hasSidecar: false,
        error: '运行时状态格式不对',
      );
    }
    return PluginSidecarStatus(
      ok: decoded['ok'] == true,
      pluginId: decoded['pluginId'] is String ? decoded['pluginId'] as String : '',
      hasSidecar: decoded['hasSidecar'] == true,
      spec: PluginSidecarSpec.fromJson(decoded['spec']),
      dir: decoded['dir'] is String ? decoded['dir'] as String : '',
      trusted: decoded['trusted'] == true,
      denied: decoded['denied'] == true,
      running: decoded['running'] == true,
      port: decoded['port'] is int ? decoded['port'] as int : null,
      exitReason:
          decoded['exitReason'] is String ? decoded['exitReason'] as String : '',
      cursor: decoded['cursor'] is int ? decoded['cursor'] as int : 0,
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  }
}

/// 起 sidecar 的结果；`status == 'needs_trust'` 时先弹信任卡。
class PluginSidecarStart {
  const PluginSidecarStart({
    required this.ok,
    required this.status,
    required this.pluginId,
    this.port,
    this.command = '',
    this.args = const [],
    this.sha256 = '',
    this.transport = '',
    this.allowLocalPorts = const [],
    this.allowLocalAny = false,
    this.error = '',
  });

  final bool ok;
  final String status;
  final String pluginId;
  final int? port;
  final String command;
  final List<String> args;
  final String sha256;
  final String transport;
  final List<int> allowLocalPorts;
  final bool allowLocalAny;
  final String error;

  bool get needsTrust => ok && status == 'needs_trust';
  bool get started => ok && status == 'started';

  static PluginSidecarStart fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const PluginSidecarStart(
        ok: false,
        status: '',
        pluginId: '',
        error: '启动结果不是合法 JSON',
      );
    }
    if (decoded is! Map) {
      return const PluginSidecarStart(
        ok: false,
        status: '',
        pluginId: '',
        error: '启动结果格式不对',
      );
    }
    return PluginSidecarStart(
      ok: decoded['ok'] == true,
      status: decoded['status'] is String ? decoded['status'] as String : '',
      pluginId: decoded['pluginId'] is String ? decoded['pluginId'] as String : '',
      port: decoded['port'] is int ? decoded['port'] as int : null,
      command: decoded['command'] is String ? decoded['command'] as String : '',
      args: [
        for (final a in (decoded['args'] as List? ?? const []))
          if (a is String) a,
      ],
      sha256: decoded['sha256'] is String ? decoded['sha256'] as String : '',
      transport:
          decoded['transport'] is String ? decoded['transport'] as String : '',
      allowLocalPorts: [
        for (final p in (decoded['allowLocalPorts'] as List? ?? const []))
          if (p is int) p,
      ],
      allowLocalAny: decoded['allowLocalAny'] == true,
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  }
}

/// 一次普通调用（信任 / 拒绝 / 停止）的结果。
class PluginRuntimeReply {
  const PluginRuntimeReply({required this.ok, this.error = ''});

  final bool ok;
  final String error;

  static PluginRuntimeReply fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const PluginRuntimeReply(ok: false, error: '应答不是合法 JSON');
    }
    if (decoded is! Map) {
      return const PluginRuntimeReply(ok: false, error: '应答格式不对');
    }
    return PluginRuntimeReply(
      ok: decoded['ok'] == true,
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  }
}

/// stdio 请求的回包（`result` 原样保留，界面按 JSON 显示）。
class PluginRuntimeCall {
  const PluginRuntimeCall({required this.ok, this.result, this.error = ''});

  final bool ok;
  final Object? result;
  final String error;

  /// 结果的一行摘要（界面在列表里用；完整内容在详情里）。
  String get resultText {
    if (!ok) {
      return error;
    }
    try {
      return const JsonEncoder.withIndent('  ').convert(result);
    } catch (_) {
      return '$result';
    }
  }

  static PluginRuntimeCall fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const PluginRuntimeCall(ok: false, error: '回包不是合法 JSON');
    }
    if (decoded is! Map) {
      return const PluginRuntimeCall(ok: false, error: '回包格式不对');
    }
    return PluginRuntimeCall(
      ok: decoded['ok'] == true,
      result: decoded['result'],
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  }
}

/// 宿主代请求的回执（只回状态码 + 文本体，插件代码拿不到 socket）。
class PluginRuntimeHttp {
  const PluginRuntimeHttp({
    required this.ok,
    this.status = 0,
    this.body = '',
    this.error = '',
  });

  final bool ok;
  final int status;
  final String body;
  final String error;

  static PluginRuntimeHttp fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const PluginRuntimeHttp(ok: false, error: '回执不是合法 JSON');
    }
    if (decoded is! Map) {
      return const PluginRuntimeHttp(ok: false, error: '回执格式不对');
    }
    return PluginRuntimeHttp(
      ok: decoded['ok'] == true,
      status: decoded['status'] is int ? decoded['status'] as int : 0,
      body: decoded['body'] is String ? decoded['body'] as String : '',
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  }
}

/// 轮询拿到的事件（`payload` 是 sidecar 原样的 JSON 行）。
class PluginRuntimeEvents {
  const PluginRuntimeEvents({
    required this.cursor,
    required this.events,
    required this.running,
    this.exitReason = '',
  });

  final int cursor;
  final List<Map<String, Object?>> events;
  final bool running;
  final String exitReason;

  static PluginRuntimeEvents fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const PluginRuntimeEvents(cursor: 0, events: [], running: false);
    }
    if (decoded is! Map) {
      return const PluginRuntimeEvents(cursor: 0, events: [], running: false);
    }
    return PluginRuntimeEvents(
      cursor: decoded['cursor'] is int ? decoded['cursor'] as int : 0,
      events: [
        for (final e in (decoded['events'] as List? ?? const []))
          if (e is Map) Map<String, Object?>.from(e),
      ],
      running: decoded['running'] == true,
      exitReason:
          decoded['exitReason'] is String ? decoded['exitReason'] as String : '',
    );
  }
}

// ── C ABI 调用 ────────────────────────────────────────────────────

/// 读状态（只读清单 + 内存里的会话表，不发网络、不起进程）。
PluginSidecarStatus readPluginRuntimeStatus(String pluginId) {
  final raw = _call1('lunac_plugin_runtime_status', pluginId);
  if (raw == null) {
    return PluginSidecarStatus(
      ok: false,
      pluginId: pluginId,
      hasSidecar: false,
      error: '不可用（rust 桥接未加载）',
    );
  }
  return PluginSidecarStatus.fromJson(raw);
}

/// 起 sidecar（会阻塞到 ready：Dart 侧放后台 isolate）。
Future<PluginSidecarStart> startPluginSidecar(
  String pluginId, {
  required String owner,
}) async {
  final raw = await Isolate.run(
    () => _call2('lunac_plugin_runtime_start', pluginId, owner),
  );
  if (raw == null) {
    return PluginSidecarStart(
      ok: false,
      status: '',
      pluginId: pluginId,
      error: '不可用（rust 桥接未加载）',
    );
  }
  return PluginSidecarStart.fromJson(raw);
}

/// 停止 sidecar。
Future<PluginRuntimeReply> stopPluginSidecar(String pluginId) async {
  final raw = await Isolate.run(
    () => _call1('lunac_plugin_runtime_stop', pluginId),
  );
  return _reply(raw);
}

/// 按 owner 收掉它起的全部 sidecar（面板 / 窗口关闭时调）。
Future<PluginRuntimeReply> stopPluginSidecarsForOwner(String owner) async {
  final raw = await Isolate.run(
    () => _call1('lunac_plugin_runtime_stop_owner', owner),
  );
  return _reply(raw);
}

/// 信任一次（`remember = true` 落盘）。
Future<PluginRuntimeReply> trustPluginSidecar(
  String pluginId, {
  required bool remember,
}) async {
  final raw = await Isolate.run(
    () => _callTrust(pluginId, remember),
  );
  return _reply(raw);
}

/// 拒绝（本会话内不再询问）。
Future<PluginRuntimeReply> denyPluginSidecar(String pluginId) async {
  final raw = await Isolate.run(
    () => _call1('lunac_plugin_runtime_deny', pluginId),
  );
  return _reply(raw);
}

/// stdio 请求（30s 超时）。
Future<PluginRuntimeCall> requestPluginSidecar(
  String pluginId,
  String method,
  String paramsJson,
) async {
  final raw = await Isolate.run(
    () => _call3('lunac_plugin_runtime_request', pluginId, method, paramsJson),
  );
  if (raw == null) {
    return const PluginRuntimeCall(
      ok: false,
      error: '不可用（rust 桥接未加载）',
    );
  }
  return PluginRuntimeCall.fromJson(raw);
}

/// 宿主代请求本机端口（`port = 0` 用插件自己的端口）。
Future<PluginRuntimeHttp> httpPluginSidecar(
  String pluginId,
  String method,
  String path,
  String body,
  int port,
) async {
  final raw = await Isolate.run(
    () => _callHttp(pluginId, method, path, body, port),
  );
  if (raw == null) {
    return const PluginRuntimeHttp(
      ok: false,
      error: '不可用（rust 桥接未加载）',
    );
  }
  return PluginRuntimeHttp.fromJson(raw);
}

/// 拉取 `since` 之后的事件（面板每秒轮询一次）。
PluginRuntimeEvents pollPluginRuntimeEvents(String pluginId, int since) {
  final raw = _callEvents(pluginId, since);
  if (raw == null) {
    return const PluginRuntimeEvents(cursor: 0, events: [], running: false);
  }
  return PluginRuntimeEvents.fromJson(raw);
}

/// 应用退出前收掉全部 sidecar（宿主崩溃路径由 Job Object 兜底）。
Future<void> killAllPluginSidecars() async {
  await Isolate.run(_callKillAll);
}

PluginRuntimeReply _reply(String? raw) {
  if (raw == null) {
    return const PluginRuntimeReply(
      ok: false,
      error: '不可用（rust 桥接未加载）',
    );
  }
  return PluginRuntimeReply.fromJson(raw);
}

String? _call1(String symbol, String a) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  final pa = a.toNativeUtf8();
  try {
    raw = library.lookupFunction<_String1C, _String1Dart>(symbol)(pa);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    malloc.free(pa);
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

String? _call2(String symbol, String a, String b) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  final pa = a.toNativeUtf8();
  final pb = b.toNativeUtf8();
  try {
    raw = library.lookupFunction<_String2C, _String2Dart>(symbol)(pa, pb);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    malloc.free(pa);
    malloc.free(pb);
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

String? _call3(String symbol, String a, String b, String c) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  final pa = a.toNativeUtf8();
  final pb = b.toNativeUtf8();
  final pc = c.toNativeUtf8();
  try {
    raw = library.lookupFunction<_String3C, _String3Dart>(symbol)(pa, pb, pc);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    malloc.free(pa);
    malloc.free(pb);
    malloc.free(pc);
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

String? _callHttp(
  String pluginId,
  String method,
  String path,
  String body,
  int port,
) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  final pa = pluginId.toNativeUtf8();
  final pb = method.toNativeUtf8();
  final pc = path.toNativeUtf8();
  final pd = body.toNativeUtf8();
  try {
    raw = library.lookupFunction<_HttpC, _HttpDart>(
      'lunac_plugin_runtime_http',
    )(pa, pb, pc, pd, port);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    malloc.free(pa);
    malloc.free(pb);
    malloc.free(pc);
    malloc.free(pd);
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

String? _callTrust(String pluginId, bool remember) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  final pa = pluginId.toNativeUtf8();
  try {
    raw = library.lookupFunction<_TrustC, _TrustDart>(
      'lunac_plugin_runtime_trust',
    )(pa, remember ? 1 : 0);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    malloc.free(pa);
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

String? _callEvents(String pluginId, int since) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  final pa = pluginId.toNativeUtf8();
  try {
    raw = library.lookupFunction<_EventsC, _EventsDart>(
      'lunac_plugin_runtime_events',
    )(pa, since);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    malloc.free(pa);
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

void _callKillAll() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_String0C, _String0Dart>(
      'lunac_plugin_runtime_kill_all',
    )();
  } catch (_) {
    return;
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

void _free(DynamicLibrary library, Pointer<Utf8> pointer) {
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}

/// 面板依赖的最小运行时接口（单测注入假实现，不加载 DLL、不起进程）。
abstract class PluginRuntimeBridge {
  PluginSidecarStatus readStatus(String pluginId);

  Future<PluginSidecarStart> start(String pluginId, String owner);

  Future<PluginRuntimeReply> stop(String pluginId);

  Future<PluginRuntimeReply> stopOwner(String owner);

  Future<PluginRuntimeReply> trust(String pluginId, {required bool remember});

  Future<PluginRuntimeReply> deny(String pluginId);

  Future<PluginRuntimeCall> request(
    String pluginId,
    String method,
    String paramsJson,
  );

  Future<PluginRuntimeHttp> http(
    String pluginId,
    String method,
    String path,
    String body,
    int port,
  );

  PluginRuntimeEvents poll(String pluginId, int since);
}

/// 真实实现（走上面的 C ABI）。
class NativePluginRuntimeBridge implements PluginRuntimeBridge {
  const NativePluginRuntimeBridge();

  @override
  PluginSidecarStatus readStatus(String pluginId) =>
      readPluginRuntimeStatus(pluginId);

  @override
  Future<PluginSidecarStart> start(String pluginId, String owner) =>
      startPluginSidecar(pluginId, owner: owner);

  @override
  Future<PluginRuntimeReply> stop(String pluginId) =>
      stopPluginSidecar(pluginId);

  @override
  Future<PluginRuntimeReply> stopOwner(String owner) =>
      stopPluginSidecarsForOwner(owner);

  @override
  Future<PluginRuntimeReply> trust(String pluginId, {required bool remember}) =>
      trustPluginSidecar(pluginId, remember: remember);

  @override
  Future<PluginRuntimeReply> deny(String pluginId) =>
      denyPluginSidecar(pluginId);

  @override
  Future<PluginRuntimeCall> request(
    String pluginId,
    String method,
    String paramsJson,
  ) => requestPluginSidecar(pluginId, method, paramsJson);

  @override
  Future<PluginRuntimeHttp> http(
    String pluginId,
    String method,
    String path,
    String body,
    int port,
  ) => httpPluginSidecar(pluginId, method, path, body, port);

  @override
  PluginRuntimeEvents poll(String pluginId, int since) =>
      pollPluginRuntimeEvents(pluginId, since);
}
