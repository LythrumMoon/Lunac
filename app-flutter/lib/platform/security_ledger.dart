// 白箱边界账本（W0，v2 移植）：`docs/sandbox-research.md` §6 的前端唯一出口。
//
// 口径（与 v1 `app/src/security-ledger.ts` 一致）：
//   · 账本只记录「判定」，不执行任何拦截 —— 真正的拦截仍在审批卡 / agent / 工作区锁；
//   · 原因码是稳定契约（与 core-agent 的分析结论、将来 MXC 的 denials 映射共用）；
//   · 写盘失败绝不打断对话（fire-and-forget：宿主只回 `{ok:false}`，这里静默丢弃）；
//   · 落盘位置与上限由 Rust 侧负责（`ModuleData/security/decisions-YYYYMMDD.ndjson`）。
//
// 本文件只做三件事：**算原因码**（`describeBoundary`）、**写一行**、**读 / 导出**。
// 不碰 UI；「更多设置 → 安全」页在 `search/chat_panel.dart`。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';
import 'usage.dart' show localDateKey;

/// 判定枚举（与 Rust `security_ledger::DECISIONS` 一一对应）。
abstract final class SecurityDecision {
  static const allow = 'allow';
  static const ask = 'ask';
  static const deny = 'deny';
  static const auto = 'auto';
}

/// 原因码唯一枚举（`docs/sandbox-research.md` §6.2）。
///
/// UI、账本、导出、测试都只认它；**不许在调用点拼中文字符串**。
const List<String> kSecurityReasonCodes = [
  'AUTO_READONLY',
  'RUN_MODE_AUTO',
  'WHITELIST_TOOL',
  'WHITELIST_CMD',
  'ASK_DEFAULT',
  'DANGER_RULE',
  'OPAQUE_COMMAND',
  'SECRET_IN_PAYLOAD',
  'PLAN_MODE_BLOCK',
  'PLAN_APPROVAL',
  'ASK_QUESTION',
  'WORKSPACE_LOCK',
  'PROTECTED_DIR',
  'USER_ALLOW',
  'USER_DENY',
  'USER_ALWAYS',
  'ENGINE_TIMEOUT',
  'ENGINE_OOM',
  'MXC_FILE_DENY',
  'MXC_NET_DENY',
  'MXC_CAP_DENY',
  'MXC_UI_DENY',
  'BACKEND_UNAVAILABLE',
  // 技能市场（§待办 3-28.9 ②③）：哈希不符 / 升级扩大工具权限。
  'HASH_MISMATCH',
  'PERMISSION_EXPAND',
];

/// 一条判定的原因：`code` 是稳定码，`detail` 只写「命中哪条规则 / 哪个命令」。
typedef BoundaryWhy = ({String code, String detail});

/// 把审批请求的字段翻译成**稳定原因码**（与 `shouldAutoApprove` 同一套优先级）。
///
/// 顺序与 v1 `describeRequest()` 一致：危险 > 不可判定 > 凭据 > 计划 > 提问 >
/// 运行方式。v2 暂无用户白名单，因此不会产出 `WHITELIST_TOOL` / `WHITELIST_CMD`。
BoundaryWhy describeBoundary({
  required String toolName,
  Map<String, dynamic>? analysis,
  required String runMode,
}) {
  if (_hasItems(analysis?['dangerous'])) {
    return (code: 'DANGER_RULE', detail: _labels(analysis?['dangerous']));
  }
  // 「判不出来」只在非自动档拦（v1 `classifyRequest()` ②）：自动档的规范语义就是
  // 不再弹卡，opaque 在那一档不构成拒绝理由，因此也不该成为账本里的原因码。
  if (runMode != 'auto' && _hasItems(analysis?['opaque'])) {
    return (code: 'OPAQUE_COMMAND', detail: _labels(analysis?['opaque']));
  }
  if (_hasItems(analysis?['secrets'])) {
    return (code: 'SECRET_IN_PAYLOAD', detail: _labels(analysis?['secrets']));
  }
  if (toolName == 'ExitPlanMode') {
    return (code: 'PLAN_APPROVAL', detail: '');
  }
  if (toolName == 'AskUserQuestion') {
    return (code: 'ASK_QUESTION', detail: '');
  }
  // `RunScript`（§待办 3-28.15）：模型写的代码，**永不自动放行**（与 `shouldAutoApprove`
  // 同一份口径）⇒ 自动档下它仍然走「默认询问」，不能记成 `RUN_MODE_AUTO`。
  if (toolName == 'RunScript') {
    return (code: 'ASK_DEFAULT', detail: '');
  }
  if (runMode == 'auto') {
    return (code: 'RUN_MODE_AUTO', detail: '');
  }
  if (runMode != 'manual' &&
      (toolName == 'Cmd' || toolName == 'PowerShell') &&
      analysis?['readonly'] == true) {
    return (code: 'AUTO_READONLY', detail: '');
  }
  return (code: 'ASK_DEFAULT', detail: '');
}

/// 账本里记什么「目标」：命令 > 文件路径 > 其他路径 / URL > 工具名（v1 同口径）。
String securityTarget(String toolName, Map<String, dynamic>? input) {
  for (final key in const ['command', 'file_path', 'path', 'url']) {
    final value = input?[key];
    if (value is String && value.trim().isNotEmpty) {
      return value;
    }
  }
  return toolName;
}

/// 原因码 → 用户可读文案（唯一出口；别在调用点拼字符串）。
///
/// 文案取 v1 i18n 的 zh-CN；`detail` 只用于「命中哪条规则」，不做二次解释。
String reasonTextFor(String code, [String detail = '']) {
  final d = detail.trim();
  switch (code) {
    case 'AUTO_READONLY':
      return '可证只读，自动放行';
    case 'RUN_MODE_AUTO':
      return '自动档放行';
    case 'WHITELIST_TOOL':
    case 'WHITELIST_CMD':
      return d.isEmpty ? '命中白名单' : '命中白名单：$d';
    case 'ASK_DEFAULT':
      return '默认需要确认';
    case 'DANGER_RULE':
      return d.isEmpty ? '命中危险规则' : '命中危险规则：$d';
    case 'OPAQUE_COMMAND':
      return d.isEmpty ? '无法静态判定，按需要确认处理' : '无法静态判定：$d';
    case 'SECRET_IN_PAYLOAD':
      return d.isEmpty ? '写入内容里疑似凭据' : '写入内容里疑似凭据：$d';
    case 'PLAN_MODE_BLOCK':
      return '计划模式下的写入被拦下';
    case 'PLAN_APPROVAL':
      return '待批准的计划';
    case 'ASK_QUESTION':
      return '需要用户作答';
    case 'WORKSPACE_LOCK':
      return '越出工作区';
    case 'PROTECTED_DIR':
      return '受保护目录';
    case 'USER_ALLOW':
      return '用户点了允许';
    case 'USER_DENY':
      return '用户点了拒绝';
    case 'USER_ALWAYS':
      return '用户点了始终允许';
    case 'ENGINE_TIMEOUT':
      return '插件引擎超时';
    case 'ENGINE_OOM':
      return '插件引擎内存超限';
    case 'MXC_FILE_DENY':
      return 'MXC 拒绝了文件访问';
    case 'MXC_NET_DENY':
      return 'MXC 拒绝了网络访问';
    case 'MXC_CAP_DENY':
      return 'MXC 拒绝了能力调用';
    case 'MXC_UI_DENY':
      return 'MXC 拒绝了界面操作';
    case 'BACKEND_UNAVAILABLE':
      return 'MXC 不可用，回落到现有边界';
    case 'HASH_MISMATCH':
      return '技能内容与清单哈希不符，拒绝安装';
    case 'PERMISSION_EXPAND':
      return d.isEmpty ? '技能升级扩大了工具权限' : '技能升级扩大了工具权限：$d';
    default:
      return '其他判定';
  }
}

/// 账本里的一行（字段固定；`v` / `ts` 由宿主生成）。
class SecurityEntry {
  const SecurityEntry({
    required this.ts,
    required this.session,
    required this.actor,
    required this.tool,
    required this.target,
    required this.decision,
    required this.reasonCode,
    required this.reasonText,
    required this.policySource,
    required this.scope,
    required this.evidence,
    required this.reversible,
    this.v = 1,
  });

  final int v;
  final String ts;
  final String session;
  final String actor;
  final String tool;
  final String target;
  final String decision;
  final String reasonCode;
  final String reasonText;
  final String policySource;
  final String scope;
  final Map<String, dynamic> evidence;
  final bool reversible;

  static SecurityEntry fromJson(Map<String, dynamic> json) {
    final evidence = json['evidence'];
    return SecurityEntry(
      v: json['v'] is int ? json['v'] as int : 1,
      ts: _str(json['ts']),
      session: _str(json['session']),
      actor: _str(json['actor']),
      tool: _str(json['tool']),
      target: _str(json['target']),
      decision: _str(json['decision']),
      reasonCode: _str(json['reasonCode']),
      reasonText: _str(json['reasonText']),
      policySource: _str(json['policySource']),
      scope: _str(json['scope']),
      evidence: evidence is Map<String, dynamic> ? evidence : const {},
      reversible: json['reversible'] == true,
    );
  }

  /// 进盘载荷：**不带** `v` / `ts`（宿主生成，前端传了也会被忽略）。
  Map<String, Object?> toPayload() => {
    'session': session,
    'actor': actor,
    'tool': tool,
    'target': target,
    'decision': decision,
    'reasonCode': reasonCode,
    'reasonText': reasonText,
    'policySource': policySource,
    'scope': scope,
    'evidence': evidence,
    'reversible': reversible,
  };
}

/// 写一条判定。**不 await**：账本是旁路，不能因为写盘慢拖住审批。
///
/// 失败静默（宿主已记 warn；前端不打扰用户）—— 与 v1 `logSecurityDecision` 同语义。
void logSecurityDecision({
  required String tool,
  Map<String, dynamic>? input,
  required String decision,
  required BoundaryWhy why,
  required String session,
  required String scope,
  String actor = 'agent',
  String policySource = 'auto_approval.dart',
}) {
  final entry = SecurityEntry(
    ts: '',
    session: session,
    actor: actor,
    tool: tool,
    target: securityTarget(tool, input),
    decision: decision,
    reasonCode: why.code,
    reasonText: reasonTextFor(why.code, why.detail),
    policySource: policySource,
    scope: scope,
    evidence: why.detail.isEmpty ? const {} : {'detail': why.detail},
    reversible: const ['Write', 'Edit', 'NotebookEdit'].contains(tool),
  );
  _callJson('lunac_security_ledger_append', {
    'date': localDateKey(DateTime.now()),
    'entry': entry.toPayload(),
  });
}

/// 读最近的判定（新 → 旧）。桥接缺失时返回空表。
List<SecurityEntry> readRecentSecurityDecisions({int limit = 200}) {
  final raw = _callJson('lunac_security_ledger_recent', {'limit': limit});
  final list = raw == null ? null : _decodeList(raw);
  if (list == null) {
    return const [];
  }
  return [
    for (final item in list)
      if (item is Map<String, dynamic>) SecurityEntry.fromJson(item),
  ];
}

/// 导出复盘包。成功给 `path`，失败给一句原因。
({String? path, String? error}) exportSecurityLedger({
  required Map<String, Object?> policy,
  required String appVersion,
}) {
  final raw = _callJson('lunac_security_ledger_export', {
    'policy': policy,
    'appVersion': appVersion,
  });
  final value = raw == null ? null : _decode(raw);
  if (value == null) {
    return (path: null, error: '账本不可用（未加载 rust 桥接）');
  }
  if (value['ok'] == true) {
    return (path: _str(value['path']), error: null);
  }
  final error = _str(value['error']);
  return (path: null, error: error.isEmpty ? '导出账本失败' : error);
}

/// 判定预演用的静态分析：调 `agent.exe --analyze`（core-agent `bash_safety::analyze`），
/// 只分析、**不执行**。
///
/// 放进 `Isolate.run`：进程启动 + 最长 3 秒的等待不能冻住 UI。失败时 `error` 非空，
/// 调用方据此如实说明「这次预演只按运行方式判」。
Future<({Map<String, dynamic>? analysis, String? error})> analyzeCommand(
  String command,
) async {
  final raw = await Isolate.run(
    () => _callText('lunac_security_analyze', command),
  );
  final value = raw == null ? null : _decode(raw);
  if (value == null) {
    return (analysis: null, error: '判定分析不可用（未加载 rust 桥接）');
  }
  if (value['ok'] == true) {
    final analysis = value['analysis'];
    return (
      analysis: analysis is Map<String, dynamic> ? analysis : null,
      error: null,
    );
  }
  final error = _str(value['error']);
  return (analysis: null, error: error.isEmpty ? '判定分析失败' : error);
}

// ── FFI ───────────────────────────────────────────────────────────

typedef _JsonInC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _JsonInDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 调一个「入参 JSON / 出参 JSON」的 FFI；桥接缺失或抛错时返回 null。
String? _callJson(String symbol, Map<String, Object?> payload) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_JsonInC, _JsonInDart>(symbol);
    input = jsonEncode(payload).toNativeUtf8();
    raw = call(input);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    if (raw != null && raw != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 调一个「入参纯文本 / 出参 JSON」的 FFI；桥接缺失或抛错时返回 null。
String? _callText(String symbol, String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_JsonInC, _JsonInDart>(symbol);
    input = text.toNativeUtf8();
    raw = call(input);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    if (raw != null && raw != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// `analysis` 里的标签数组 → 「a、b」；非数组 / 空串一律当没有。
///
/// 形状有两种：命令分析给字符串（`["递归强制删除"]`），写入内容分析给
/// `{rule, line}` 对象（core-agent `content_safety::json_hits()`）。
String _labels(Object? value) {
  if (value is! List) {
    return '';
  }
  final parts = <String>[];
  for (final item in value) {
    if (item is String) {
      final text = item.trim();
      if (text.isNotEmpty) {
        parts.add(text);
      }
      continue;
    }
    if (item is Map) {
      final rule = item['rule'];
      if (rule is! String || rule.trim().isEmpty) {
        continue;
      }
      final line = item['line'];
      parts.add(
        line is int && line > 0 ? '${rule.trim()}（第 $line 行）' : rule.trim(),
      );
    }
  }
  return parts.join('、');
}

bool _hasItems(Object? value) => value is List && value.isNotEmpty;

String _str(Object? value) => value is String ? value : '';

Map<String, dynamic>? _decode(String raw) {
  try {
    final value = jsonDecode(raw);
    return value is Map<String, dynamic> ? value : null;
  } catch (_) {
    return null;
  }
}

List<dynamic>? _decodeList(String raw) {
  try {
    final value = jsonDecode(raw);
    return value is List ? value : null;
  } catch (_) {
    return null;
  }
}
