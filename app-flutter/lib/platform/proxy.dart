// 平台薄层：代理（v1 `proxy` 插件的原生等价物）。
//
// 边界：系统代理（注册表读写 / 备份还原 / 地址规范化）在 `rust/src/proxy.rs`；
// 本地代理服务（自研引擎）在 `rust/src/native_proxy/`，由这里转调这些 C ABI：
// `lunac_proxy_status` / `_system_set` / `_local_status` / `_local_start` / `_local_stop` /
// `_local_select` / `_local_delay` / `_local_mode` / `_nodes` / `_requests` / `_exit_info`。
// 本文件只负责调用与容错解析。
//
// 状态 JSON 形状（Rust `proxy::status_json`）：
//   · `system`（enabled / server / bypass）+ `backup`（有没有系统原值备份）；
//   · `local`（supported / running / port / active / nodes / skipped / reason）；
//   · `kernel`（available / installed / path / searchPaths / reason）。
// 引擎状态（`lunac_proxy_local_status`）另有 `mode`（rule / global）与
// `traffic`（累计 up / down 字节，界面按两次采样差算速率）。
//
// 纪律：坏 JSON / 桥接缺符号 → 明确状态或一句人话，**不抛异常、不崩面板**。
// 返回字符串由 Rust 分配，读完统一用 `lunac_free_string` 释放。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 系统代理的实际状态（`ProxyEnable` / `ProxyServer` / `ProxyOverride`）。
class ProxySystemState {
  const ProxySystemState({
    required this.enabled,
    required this.server,
    required this.bypass,
  });

  final bool enabled;
  final String server;
  final String bypass;

  static ProxySystemState? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    final enabled = json['enabled'];
    if (enabled is! bool) {
      return null;
    }
    final server = json['server'];
    final bypass = json['bypass'];
    return ProxySystemState(
      enabled: enabled,
      server: server is String ? server : '',
      bypass: bypass is String ? bypass : '',
    );
  }
}

/// 自研引擎里的一个节点（订阅 / 分享链接解析结果）。
class ProxyNode {
  const ProxyNode({
    required this.index,
    required this.name,
    required this.kind,
    required this.server,
    required this.port,
    required this.implemented,
    this.transport = '',
    this.source = '',
  });

  final int index;
  final String name;

  /// `ss` / `trojan` / `vless` / `vmess` / `anytls` / `hysteria2` / `tuic`。
  final String kind;
  final String server;
  final int port;

  /// 本版是否已有该协议的拨号实现（没有的会在切换时给一句人话）。
  final bool implemented;

  /// 传输层：`tcp` / `tls` / `ws` / `http` / `h2` / `grpc` / `xhttp` / `quic` / `obfs-*`。
  final String transport;

  /// 来源：`订阅` / `分享链接`（抽屉里展示）。
  final String source;

  String get endpoint => '$server:$port';

  static ProxyNode? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    final index = json['index'];
    final port = json['port'];
    final name = json['name'];
    final kind = json['kind'];
    final server = json['server'];
    final transport = json['transport'];
    final source = json['source'];
    return ProxyNode(
      index: index is int ? index : 0,
      name: name is String && name.isNotEmpty ? name : '未命名节点',
      kind: kind is String ? kind : '',
      server: server is String ? server : '',
      port: port is int ? port : 0,
      implemented: json['implemented'] == true,
      transport: transport is String ? transport : '',
      source: source is String ? source : '',
    );
  }

  static List<ProxyNode> listFromJson(Object? json) => [
    for (final item in (json is List ? json : const []))
      ?ProxyNode.fromJson(item),
  ];
}

/// 本地代理服务状态（自研引擎）。
class ProxyLocalState {
  const ProxyLocalState({
    this.supported = false,
    this.running = false,
    this.port = 0,
    this.active = 0,
    this.reason = '',
    this.nodes = const [],
    this.skipped = const [],
  });

  final bool supported;
  final bool running;
  final int port;

  /// 当前选中节点的下标。
  final int active;
  final List<ProxyNode> nodes;

  /// 解析时被跳过的链接 / 规则（原样展示，附原因）。
  final List<String> skipped;

  /// 为什么没有接入 / 为什么起不来（面板原样展示）。
  final String reason;

  static ProxyLocalState fromJson(Object? json) {
    if (json is! Map) {
      return const ProxyLocalState();
    }
    final port = json['port'];
    final active = json['active'];
    final reason = json['reason'];
    final skipped = json['skipped'];
    return ProxyLocalState(
      supported: json['supported'] == true,
      running: json['running'] == true,
      port: port is int ? port : 0,
      active: active is int ? active : 0,
      reason: reason is String ? reason : '',
      nodes: ProxyNode.listFromJson(json['nodes']),
      skipped: [
        for (final item in (skipped is List ? skipped : const []))
          if (item is String && item.isNotEmpty) item,
      ],
    );
  }
}

/// 引擎自身的状态（`lunac_proxy_local_status` 的解析结果）。
class LocalProxyStatus {
  const LocalProxyStatus({
    this.supported = false,
    this.running = false,
    this.port = 0,
    this.active = 0,
    this.mode = 'rule',
    this.trafficUp = 0,
    this.trafficDown = 0,
    this.nodes = const [],
    this.skipped = const [],
    this.error = '',
  });

  final bool supported;
  final bool running;
  final int port;
  final int active;

  /// `rule` 规则模式 / `global` 全局代理（`direct` 只为兼容旧配置）。
  final String mode;

  /// 累计上行 / 下行字节（界面按两次采样差算速率）。
  final int trafficUp;
  final int trafficDown;

  final List<ProxyNode> nodes;
  final List<String> skipped;
  final String error;

  /// 解析引擎状态 JSON（纯函数，单测不加载 DLL）。
  static LocalProxyStatus parse(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const LocalProxyStatus(error: '引擎状态不是合法 JSON');
    }
    if (decoded is! Map) {
      return const LocalProxyStatus(error: '引擎状态格式不对');
    }
    final error = decoded['error'];
    final port = decoded['port'];
    final active = decoded['active'];
    final mode = decoded['mode'];
    final traffic = decoded['traffic'];
    final skipped = decoded['skipped'];
    final up = traffic is Map ? traffic['up'] : null;
    final down = traffic is Map ? traffic['down'] : null;
    return LocalProxyStatus(
      supported: decoded['supported'] == true,
      running: decoded['running'] == true,
      port: port is int ? port : 0,
      active: active is int ? active : 0,
      mode: mode is String && mode.isNotEmpty ? mode : 'rule',
      trafficUp: up is int ? up : 0,
      trafficDown: down is int ? down : 0,
      nodes: ProxyNode.listFromJson(decoded['nodes']),
      skipped: [
        for (final item in (skipped is List ? skipped : const []))
          if (item is String && item.isNotEmpty) item,
      ],
      error: error is String ? error : '',
    );
  }
}

/// 一条请求日志（`lunac_proxy_requests` 的一项）。
class ProxyRequestEntry {
  const ProxyRequestEntry({
    required this.host,
    required this.port,
    required this.route,
    required this.detail,
  });

  final String host;
  final int port;

  /// `node` 走节点 / `direct` 直连 / `rejected` 拒绝。
  final String route;

  /// 节点名 / 命中的规则 / 拒绝原因（人话）。
  final String detail;

  /// 日志区一行的读法：`example.com:443 → 订阅 · 香港 01`。
  String get line => '$host:$port → $detail';

  static ProxyRequestEntry? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    final host = json['host'];
    final route = json['route'];
    final detail = json['detail'];
    final port = json['port'];
    return ProxyRequestEntry(
      host: host is String ? host : '',
      port: port is int ? port : 0,
      route: route is String ? route : 'direct',
      detail: detail is String ? detail : '',
    );
  }
}

/// 解析请求日志 JSON（纯函数，单测不加载 DLL）。
List<ProxyRequestEntry> parseProxyRequests(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const [];
  }
  if (decoded is! List) {
    return const [];
  }
  return [
    for (final item in decoded) ?ProxyRequestEntry.fromJson(item),
  ];
}

/// 启停 / 切节点 / 测速的通用应答。
class LocalProxyReply {
  const LocalProxyReply({required this.ok, this.error = '', this.status});

  final bool ok;
  final String error;
  final LocalProxyStatus? status;

  static LocalProxyReply parse(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const LocalProxyReply(ok: false, error: '引擎应答不是合法 JSON');
    }
    if (decoded is! Map) {
      return const LocalProxyReply(ok: false, error: '引擎应答格式不对');
    }
    final error = decoded['error'];
    final status = decoded['status'];
    return LocalProxyReply(
      ok: decoded['ok'] == true,
      error: error is String ? error : '',
      status: status is Map
          ? LocalProxyStatus.parse(jsonEncode(status))
          : null,
    );
  }
}

/// 节点时延应答（经节点的 HTTP 探针，`scope` 如实标注口径）。
class ProxyDelayReply {
  const ProxyDelayReply({required this.ok, this.error = '', this.ms = 0, this.scope = ''});

  final bool ok;
  final String error;
  final int ms;

  /// `http` = 经节点取 `http://www.baidu.com/` 的首个响应字节（与 v1 同口径）；
  /// `tcp` = 只测到节点的 TCP 连通（旧口径，仅老应答会出现）。
  final String scope;

  static ProxyDelayReply parse(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const ProxyDelayReply(ok: false, error: '时延应答不是合法 JSON');
    }
    if (decoded is! Map) {
      return const ProxyDelayReply(ok: false, error: '时延应答格式不对');
    }
    final error = decoded['error'];
    final ms = decoded['ms'];
    final scope = decoded['scope'];
    return ProxyDelayReply(
      ok: decoded['ok'] == true,
      error: error is String ? error : '',
      ms: ms is int ? ms : 0,
      scope: scope is String ? scope : '',
    );
  }
}

/// 出口地区探测的结果（经当前节点请求 `ipinfo.io/country`）。
class ProxyExitReply {
  const ProxyExitReply({required this.ok, this.country = '', this.error = ''});

  final bool ok;

  /// 两个字母的国家代码（`JP` / `US`）；失败时为空。
  final String country;
  final String error;

  static ProxyExitReply parse(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const ProxyExitReply(ok: false, error: '出口探测应答不是合法 JSON');
    }
    if (decoded is! Map) {
      return const ProxyExitReply(ok: false, error: '出口探测应答格式不对');
    }
    final country = decoded['country'];
    final error = decoded['error'];
    return ProxyExitReply(
      ok: decoded['ok'] == true,
      country: country is String ? country : '',
      error: error is String ? error : '',
    );
  }
}

/// 自定义服务器 → 分享链接的结果（格式与校验全在 Rust）。
class ProxyCustomLinkReply {
  const ProxyCustomLinkReply({
    required this.ok,
    this.link = '',
    this.name = '',
    this.kind = '',
    this.error = '',
  });

  final bool ok;
  final String link;

  /// 解析回来的备注 / 协议（与订阅同一个 `parse_link`）。
  final String name;
  final String kind;
  final String error;

  static ProxyCustomLinkReply parse(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const ProxyCustomLinkReply(
        ok: false,
        error: '自定义服务器应答不是合法 JSON',
      );
    }
    if (decoded is! Map) {
      return const ProxyCustomLinkReply(ok: false, error: '自定义服务器应答格式不对');
    }
    final link = decoded['link'];
    final name = decoded['name'];
    final kind = decoded['kind'];
    final error = decoded['error'];
    return ProxyCustomLinkReply(
      ok: decoded['ok'] == true,
      link: link is String ? link : '',
      name: name is String ? name : '',
      kind: kind is String ? kind : '',
      error: error is String ? error : '',
    );
  }
}

/// mihomo 内核状态（本版只报告是否找到了可执行文件，不能启动）。
class ProxyKernelState {
  const ProxyKernelState({
    this.available = false,
    this.installed = false,
    this.path = '',
    this.searchPaths = const [],
    this.reason = '',
  });

  final bool available;
  final bool installed;
  final String path;

  /// Rust 侧搜索过的路径（未安装时展示给用户）。
  final List<String> searchPaths;
  final String reason;

  static ProxyKernelState fromJson(Object? json) {
    if (json is! Map) {
      return const ProxyKernelState();
    }
    final path = json['path'];
    final reason = json['reason'];
    final searchPaths = json['searchPaths'];
    return ProxyKernelState(
      available: json['available'] == true,
      installed: json['installed'] == true,
      path: path is String ? path : '',
      searchPaths: [
        for (final item in (searchPaths is List ? searchPaths : const []))
          if (item is String && item.isNotEmpty) item,
      ],
      reason: reason is String ? reason : '',
    );
  }
}

/// 一次状态读取的完整结果。
class ProxyStatus {
  const ProxyStatus({
    this.supported = false,
    this.system = const ProxySystemState(
      enabled: false,
      server: '',
      bypass: '',
    ),
    this.local = const ProxyLocalState(),
    this.kernel = const ProxyKernelState(),
    this.backup = false,
    this.error,
  });

  /// 当前平台是否支持系统代理（只支持 Windows）。
  final bool supported;
  final ProxySystemState system;
  final ProxyLocalState local;
  final ProxyKernelState kernel;

  /// 是否已经备份过系统原值（关闭时会还原）。
  final bool backup;

  /// 解析失败时的一句人话（正常时 null）。
  final String? error;
}

/// 解析 Rust 返回的状态 JSON（纯函数，单测不加载 DLL）。
///
/// 坏 JSON / 缺 `system` 都收敛成可显示的错误，绝不抛异常。
ProxyStatus parseProxyStatus(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const ProxyStatus(error: '代理状态不是合法 JSON');
  }
  if (decoded is! Map) {
    return const ProxyStatus(error: '代理状态格式不对');
  }
  final system = ProxySystemState.fromJson(decoded['system']);
  if (system == null) {
    return const ProxyStatus(error: '代理状态缺少 system 字段');
  }
  return ProxyStatus(
    supported: decoded['supported'] == true,
    system: system,
    local: ProxyLocalState.fromJson(decoded['local']),
    kernel: ProxyKernelState.fromJson(decoded['kernel']),
    backup: decoded['backup'] == true,
  );
}

/// 开关系统代理的结果：`ok` / `error` / 动作后的实际状态。
class ProxySetReply {
  const ProxySetReply({required this.ok, required this.error, this.system});

  final bool ok;

  /// 失败原因（只说「哪一步没成 + 用户能做什么」）。
  final String error;

  /// 动作后的实际系统状态（失败时也尽量带回，界面据此回弹开关）。
  final ProxySystemState? system;
}

/// 解析 Rust 返回的设置结果（纯函数，单测不加载 DLL）。
ProxySetReply decodeProxySetReply(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const ProxySetReply(ok: false, error: '设置结果不是合法 JSON');
  }
  if (decoded is! Map) {
    return const ProxySetReply(ok: false, error: '设置结果格式不对');
  }
  final error = decoded['error'];
  return ProxySetReply(
    ok: decoded['ok'] == true,
    error: error is String ? error : '',
    system: ProxySystemState.fromJson(decoded['system']),
  );
}

typedef _StatusC = Pointer<Utf8> Function();
typedef _StatusDart = Pointer<Utf8> Function();
typedef _SetC = Pointer<Utf8> Function(Int32, Pointer<Utf8>);
typedef _SetDart = Pointer<Utf8> Function(int, Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读代理状态；桥接不可用 / 旧版库缺符号时返回 null（面板显示「不可用」）。
ProxyStatus? readProxyStatus() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    final status = library.lookupFunction<_StatusC, _StatusDart>(
      'lunac_proxy_status',
    );
    raw = status();
    if (raw == nullptr) {
      return null;
    }
    return parseProxyStatus(raw.toDartString());
  } catch (_) {
    return null;
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 开关 Windows 系统代理（同步阻塞；只碰注册表，耗时极短）。
ProxySetReply setSystemProxy({required bool enabled, String server = ''}) {
  final library = openLunacNative().library;
  if (library == null) {
    return const ProxySetReply(ok: false, error: '代理不可用（rust 桥接未加载）');
  }
  Pointer<Utf8>? serverPointer;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_SetC, _SetDart>(
      'lunac_proxy_system_set',
    );
    serverPointer = server.toNativeUtf8();
    raw = call(enabled ? 1 : 0, serverPointer);
    if (raw == nullptr) {
      return const ProxySetReply(ok: false, error: '设置系统代理没有返回内容');
    }
    return decodeProxySetReply(raw.toDartString());
  } catch (e) {
    return ProxySetReply(ok: false, error: '设置系统代理失败：$e');
  } finally {
    _free(library, raw);
    if (serverPointer != null) {
      malloc.free(serverPointer);
    }
  }
}

// ── 本地代理服务（自研引擎）────────────────────────────────────────

typedef _StringC = Pointer<Utf8> Function();
typedef _StringDart = Pointer<Utf8> Function();
typedef _StringArgC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _StringArgDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _UintC = Pointer<Utf8> Function(Uint32);
typedef _UintDart = Pointer<Utf8> Function(int);

void _free(dynamic library, Pointer<Utf8>? pointer) {
  if (pointer == null || pointer == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}

/// 调一个无参的 `Pointer<Utf8> f()`；不可用时返回 null。
String? _invokeString(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_StringC, _StringDart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

/// 调一个 `Pointer<Utf8> f(u32)`；不可用时返回 null。
String? _invokeUint(String symbol, int value) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_UintC, _UintDart>(symbol)(value);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

/// 调一个 `Pointer<Utf8> f(const char*)`；不可用时返回 null。
String? _invokeStringArg(String symbol, String argument) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  Pointer<Utf8>? argumentPointer;
  try {
    argumentPointer = argument.toNativeUtf8();
    raw = library.lookupFunction<_StringArgC, _StringArgDart>(symbol)(
      argumentPointer,
    );
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
    if (argumentPointer != null) {
      malloc.free(argumentPointer);
    }
  }
}

/// 读自研引擎状态；桥接不可用时返回 null（面板显示「不可用」）。
LocalProxyStatus? readLocalProxyStatus() {
  final raw = _invokeString('lunac_proxy_local_status');
  return raw == null ? null : LocalProxyStatus.parse(raw);
}

/// 启动引擎。
///
/// [links] 是分享链接（`ss://` / `trojan://` / `vless://` …）；
/// [subscription] 是整段 base64 订阅（与 [links] 可以同时给）。
LocalProxyReply startLocalProxy({
  List<String> links = const [],
  String subscription = '',
  int port = 7890,
  int active = 0,
  List<String> rules = const [],
  String mode = 'rule',
  String ruleMode = 'blacklist',
  List<String> ruleDomains = const [],
}) {
  final payload = jsonEncode({
    'links': links,
    'subscription': subscription,
    'port': port,
    'active': active,
    'rules': rules,
    'mode': mode,
    'ruleMode': ruleMode,
    'ruleDomains': ruleDomains,
  });
  final raw = _invokeStringArg('lunac_proxy_local_start', payload);
  return raw == null
      ? const LocalProxyReply(ok: false, error: '引擎不可用（rust 桥接未加载）')
      : LocalProxyReply.parse(raw);
}

/// 切换模式（`rule` 规则模式 / `global` 全局代理）。运行中即刻生效。
LocalProxyReply setLocalProxyMode(String mode) {
  final raw = _invokeStringArg('lunac_proxy_local_mode', mode);
  return raw == null
      ? const LocalProxyReply(ok: false, error: '引擎不可用（rust 桥接未加载）')
      : LocalProxyReply.parse(raw);
}

/// 出口地区（经当前节点请求 `ipinfo.io/country`）。
ProxyExitReply probeExitCountry() {
  final raw = _invokeString('lunac_proxy_exit_info');
  return raw == null
      ? const ProxyExitReply(ok: false, error: '引擎不可用（rust 桥接未加载）')
      : ProxyExitReply.parse(raw);
}

/// 由字段拼分享链接（v1 `mihomo_node_add_custom` 的等价物）。
///
/// 链接格式与校验都在 Rust（与订阅同一个 `parse_link`）；这里只负责传字段。
ProxyCustomLinkReply buildCustomProxyLink({
  required String kind,
  required String name,
  required String server,
  required int port,
  String password = '',
  String method = '',
  String uuid = '',
  bool mux = false,
}) {
  final payload = jsonEncode({
    'kind': kind,
    'name': name,
    'server': server,
    'port': port,
    'password': password,
    'method': method,
    'uuid': uuid,
    // Xray 的 mux.cool（只对 trojan / vless / vmess 生效；其它协议由 Rust 侧忽略）。
    'mux': mux,
  });
  final raw = _invokeStringArg('lunac_proxy_custom_link', payload);
  return raw == null
      ? const ProxyCustomLinkReply(
          ok: false,
          error: '引擎不可用（rust 桥接未加载）',
        )
      : ProxyCustomLinkReply.parse(raw);
}

/// 最近 [limit] 条请求日志（面板每 2s 取 200 条）；不可用时返回空表。
List<ProxyRequestEntry> readProxyRequests({int limit = 200}) {
  final raw = _invokeUint('lunac_proxy_requests', limit);
  return raw == null ? const [] : parseProxyRequests(raw);
}

/// 停止引擎（已监听端口会释放；已建立的长连接下个读周期退出）。
LocalProxyReply stopLocalProxy() {
  final raw = _invokeString('lunac_proxy_local_stop');
  return raw == null
      ? const LocalProxyReply(ok: false, error: '引擎不可用（rust 桥接未加载）')
      : LocalProxyReply.parse(raw);
}

/// 切换节点（运行中即刻生效，不必重启监听）。
LocalProxyReply selectLocalProxyNode(int index) {
  final raw = _invokeUint('lunac_proxy_local_select', index);
  return raw == null
      ? const LocalProxyReply(ok: false, error: '引擎不可用（rust 桥接未加载）')
      : LocalProxyReply.parse(raw);
}

/// 节点时延（重建期口径：TCP 连通耗时）。
ProxyDelayReply probeLocalProxyDelay(int index) {
  final raw = _invokeUint('lunac_proxy_local_delay', index);
  return raw == null
      ? const ProxyDelayReply(ok: false, error: '引擎不可用（rust 桥接未加载）')
      : ProxyDelayReply.parse(raw);
}

/// 节点清单（`lunac_proxy_nodes`）；不可用时返回空表。
List<ProxyNode> readLocalProxyNodes() {
  final raw = _invokeString('lunac_proxy_nodes');
  if (raw == null) {
    return const [];
  }
  try {
    return ProxyNode.listFromJson(jsonDecode(raw));
  } catch (_) {
    return const [];
  }
}
