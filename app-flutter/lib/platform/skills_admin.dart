// 平台薄层：`<exe 根>/skills` 的宿主侧管理（v1「设置 · AI · 技能」组）。
//
// 边界：目录定位 / key 白名单 / 大小上限 / 原子写 / 删除的越界闸都在
// `rust/src/skills_admin.rs`；本文件只解析回执，不猜路径、不自己拼文件。
// agent 侧读取同一个目录（`LUNAC_SKILLS_DIR`），改完要重启 agent 才生效。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条技能（`lunac_skills` 的 `entries[]`）。
class SkillEntry {
  const SkillEntry({
    required this.key,
    this.name = '',
    this.description = '',
    this.fork = false,
    this.bytes = 0,
    this.valid = true,
    this.sha256 = '',
    this.allowedTools = const [],
  });

  /// 目录名（agent 的 `key`）。
  final String key;
  final String name;
  final String description;

  /// `context: fork` ⇒ 子代理执行（清单里带 `[subagent]`）。
  final bool fork;
  final int bytes;

  /// 目录存在但 `SKILL.md` 读不了时的标记（不隐藏问题）。
  final bool valid;

  /// 正文的 SHA-256（小写十六进制；读不了时为空串）。
  final String sha256;

  /// frontmatter 的 `allowed-tools`（空 = 不限制）。
  final List<String> allowedTools;

  String get title => name.isNotEmpty ? name : key;

  String get hashShort => sha256.length <= 12 ? sha256 : sha256.substring(0, 12);

  static SkillEntry fromJson(Map<String, dynamic> json) {
    return SkillEntry(
      key: json['key'] is String ? json['key'] as String : '',
      name: json['name'] is String ? json['name'] as String : '',
      description: json['description'] is String ? json['description'] as String : '',
      fork: json['fork'] == true,
      bytes: json['bytes'] is num ? (json['bytes'] as num).toInt() : 0,
      valid: json['valid'] != false,
      sha256: json['sha256'] is String ? json['sha256'] as String : '',
      allowedTools: _stringList(json['allowedTools']),
    );
  }
}

List<String> _stringList(dynamic raw) =>
    raw is List ? raw.whereType<String>().toList(growable: false) : const [];

/// 列表快照。
class SkillsSnapshot {
  const SkillsSnapshot({this.entries = const [], this.error = ''});

  final List<SkillEntry> entries;
  final String error;

  static const unavailable = SkillsSnapshot(error: '当前环境没有原生桥');
}

/// 读一份技能的原文。
class SkillContent {
  const SkillContent({
    required this.ok,
    this.key = '',
    this.content = '',
    this.error = '',
    this.sha256 = '',
    this.allowedTools = const [],
  });

  final bool ok;
  final String key;
  final String content;
  final String error;

  /// 正文的 SHA-256（与清单里的 `sha256` 同一口径）。
  final String sha256;
  final List<String> allowedTools;
}

/// 一次安装的权限面（§待办 3-28.9 ③：升级时的 `allowed-tools` diff）。
class SkillPermissions {
  const SkillPermissions({
    this.tools = const [],
    this.unrestricted = false,
    this.added = const [],
    this.removed = const [],
    this.broadens = false,
  });

  /// 安装后的工具名单（空 = 不限制）。
  final List<String> tools;
  final bool unrestricted;

  /// 相比本地旧版本新增 / 移除的工具。
  final List<String> added;
  final List<String> removed;

  /// 这次升级是否扩大权限（界面据此弹确认）。
  final bool broadens;

  static SkillPermissions? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    return SkillPermissions(
      tools: _stringList(json['tools']),
      unrestricted: json['unrestricted'] == true,
      added: _stringList(json['added']),
      removed: _stringList(json['removed']),
      broadens: json['broadens'] == true,
    );
  }

  /// 一句话描述权限面（空名单 = 任意工具）。
  String get summary => unrestricted ? '任意工具' : tools.join('、');
}

/// 安全扫描的一条命中（§待办 3-28.9）。
class SkillFinding {
  const SkillFinding({
    required this.code,
    required this.severity,
    required this.line,
    required this.excerpt,
    required this.why,
  });

  /// `destructive-delete` / `remote-exec` / `credential-exfiltration` / `prompt-injection` …
  final String code;

  /// `danger` / `warn`。
  final String severity;

  /// 命中的行号（`0` = 跨行的组合判定）。
  final int line;

  /// 命中的原文片段（已截断）。
  final String excerpt;

  /// 一句人话：为什么这行可疑。
  final String why;

  bool get danger => severity == 'danger';

  static SkillFinding? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    return SkillFinding(
      code: json['code'] is String ? json['code'] as String : '',
      severity: json['severity'] is String ? json['severity'] as String : 'warn',
      line: json['line'] is num ? (json['line'] as num).toInt() : 0,
      excerpt: json['excerpt'] is String ? json['excerpt'] as String : '',
      why: json['why'] is String ? json['why'] as String : '',
    );
  }
}

/// 一份技能的安全检查结果（判定在 Rust 侧，本文件只解析）。
class SkillScan {
  const SkillScan({
    required this.risk,
    this.bytes = 0,
    this.findings = const [],
  });

  /// `safe` / `warn` / `danger`。
  final String risk;
  final int bytes;
  final List<SkillFinding> findings;

  bool get danger => risk == 'danger';
  bool get clean => risk == 'safe';

  String get label => switch (risk) {
    'danger' => '危险',
    'warn' => '需留意',
    _ => '未发现问题',
  };

  /// 安装闸拦下时用于摘要（前 3 条，其余折叠成「还有 N 条」）。
  String get summary {
    if (findings.isEmpty) {
      return label;
    }
    final head = findings.take(3).map((f) => f.why).join('；');
    final rest = findings.length - 3;
    return rest > 0 ? '$head（还有 $rest 条）' : head;
  }

  static const unavailable = SkillScan(risk: 'warn');

  static SkillScan? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final findings = <SkillFinding>[];
    final raw = json['findings'];
    if (raw is List) {
      for (final item in raw) {
        final finding = SkillFinding.fromJson(item);
        if (finding != null) {
          findings.add(finding);
        }
      }
    }
    return SkillScan(
      risk: json['risk'] is String ? json['risk'] as String : 'warn',
      bytes: json['bytes'] is num ? (json['bytes'] as num).toInt() : 0,
      findings: findings,
    );
  }
}

/// 写入 / 删除 / 导入的回执。
class SkillResult {
  const SkillResult({
    required this.ok,
    this.key = '',
    this.error = '',
    this.exists = false,
    this.bytes = 0,
    this.name = '',
    this.scanBlocked = false,
    this.scan,
    this.hashMismatch = false,
    this.permissionBlocked = false,
    this.sha256 = '',
    this.permissions,
  });

  final bool ok;
  final String key;
  final String error;

  /// 导入时同名技能已存在（界面据此弹二次确认，带 `overwrite: true` 重发）。
  final bool exists;

  /// 导入落盘的字节数。
  final int bytes;

  /// 导入内容 frontmatter 里的 `name`（没有就空串）。
  final String name;

  /// 导入被安全闸拦下（命中 `danger`）：界面必须列检查结果，用户确认后带
  /// `acceptScan: true` 重发。
  final bool scanBlocked;

  /// 这次操作带回来的检查结果（安装时总有；其它操作为 null）。
  final SkillScan? scan;

  /// 清单里的 `sha256` 与下载内容不符（硬拒绝，没有「仍然安装」）。
  final bool hashMismatch;

  /// 覆盖安装扩大了 `allowed-tools`：用户确认后带 `acceptPermissions: true` 重发。
  final bool permissionBlocked;

  /// 下载内容的实际 SHA-256（核对清单用）。
  final String sha256;

  /// 安装后的权限面（只有安装回执带）。
  final SkillPermissions? permissions;

  String get hashShort => sha256.length <= 12 ? sha256 : sha256.substring(0, 12);

  static SkillResult fromRaw(String? text) {
    if (text == null || text.isEmpty) {
      return const SkillResult(ok: false, error: '当前环境没有原生桥');
    }
    try {
      final value = jsonDecode(text);
      if (value is! Map) {
        return const SkillResult(ok: false, error: '技能操作的返回格式不对');
      }
      return SkillResult(
        ok: value['ok'] == true,
        key: value['key'] is String ? value['key'] as String : '',
        error: value['error'] is String ? value['error'] as String : '',
        exists: value['exists'] == true,
        bytes: value['bytes'] is num ? (value['bytes'] as num).toInt() : 0,
        name: value['name'] is String ? value['name'] as String : '',
        scanBlocked: value['scanBlocked'] == true,
        scan: SkillScan.fromJson(value['scan']),
        hashMismatch: value['hashMismatch'] == true,
        permissionBlocked: value['permissionBlocked'] == true,
        sha256: value['sha256'] is String ? value['sha256'] as String : '',
        permissions: SkillPermissions.fromJson(value['permissions']),
      );
    } catch (_) {
      return const SkillResult(ok: false, error: '技能操作失败');
    }
  }
}

/// 列出全部技能；桥接缺失时返回 [SkillsSnapshot.unavailable]。
SkillsSnapshot listSkills() {
  final text = _call0('lunac_skills');
  if (text == null) {
    return SkillsSnapshot.unavailable;
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map) {
      return const SkillsSnapshot(error: '技能列表的返回格式不对');
    }
    final entries = <SkillEntry>[];
    final list = value['entries'];
    if (list is List) {
      for (final item in list) {
        if (item is Map<String, dynamic>) {
          entries.add(SkillEntry.fromJson(item));
        }
      }
    }
    return SkillsSnapshot(
      entries: entries,
      error: value['error'] is String ? value['error'] as String : '',
    );
  } catch (_) {
    return const SkillsSnapshot(error: '技能列表解析失败');
  }
}

// ── 「精选」索引（§待办 3-28.9 ①）──────────────────────────────────
//
// 清单随包分发（`skills/curated.json`，哈希由打包脚本钉住）；本层只解析回执。
// 安装复用 URL 安装那条闸（哈希 / 危险扫描 / 权限 diff），差异只在来源（不联网）。

/// 「精选」里的一条，带「与本地已装版本」的比对结果。
class CuratedSkill {
  const CuratedSkill({
    required this.key,
    this.description = '',
    this.sha256 = '',
    this.installedSha256 = '',
    this.files = const [],
    this.status = 'missing',
  });

  final String key;
  final String description;

  /// 清单钉住的指纹。
  final String sha256;

  /// 本地已装版本的指纹（没装 = 空串）。
  final String installedSha256;
  final List<String> files;

  /// `installed` / `outdated` / `missing`。
  final String status;

  bool get installed => status == 'installed';
  bool get outdated => status == 'outdated';
  bool get missing => status == 'missing';

  String get statusLabel => switch (status) {
    'installed' => '已安装',
    'outdated' => '与精选版本不同',
    _ => '未安装',
  };

  /// 按钮文案：没装 = 安装；装的是别的版本 = 覆盖为精选版；一致 = 无需操作。
  String get actionLabel => installed ? '已一致' : (missing ? '安装' : '恢复精选版');

  String get hashShort => sha256.length <= 12 ? sha256 : sha256.substring(0, 12);

  static CuratedSkill? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final key = json['key'];
    if (key is! String || key.isEmpty) {
      return null;
    }
    return CuratedSkill(
      key: key,
      description: json['description'] is String ? json['description'] as String : '',
      sha256: json['sha256'] is String ? json['sha256'] as String : '',
      installedSha256:
          json['installedSha256'] is String ? json['installedSha256'] as String : '',
      files: _stringList(json['files']),
      status: json['status'] is String ? json['status'] as String : 'missing',
    );
  }
}

class CuratedSnapshot {
  const CuratedSnapshot({
    this.ok = false,
    this.updated = '',
    this.entries = const [],
    this.error = '',
  });

  final bool ok;
  final String updated;
  final List<CuratedSkill> entries;
  final String error;

  /// 清单本身可用（有文件、schema 认）。`entries` 为空也可能是「一条都没钉哈希」。
  bool get available => ok;

  static const unavailable = CuratedSnapshot(error: '当前环境没有原生桥');
}

/// 读「精选」清单并跟本地已装技能逐条比哈希。
CuratedSnapshot readCuratedSkills() {
  final text = _call0('lunac_skills_curated');
  if (text == null) {
    return CuratedSnapshot.unavailable;
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map) {
      return const CuratedSnapshot(error: '「精选」清单返回格式不对');
    }
    final entries = <CuratedSkill>[];
    final list = value['entries'];
    if (list is List) {
      for (final item in list) {
        final entry = CuratedSkill.fromJson(item);
        if (entry != null) {
          entries.add(entry);
        }
      }
    }
    return CuratedSnapshot(
      ok: value['ok'] == true,
      updated: value['updated'] is String ? value['updated'] as String : '',
      entries: entries,
      error: value['error'] is String ? value['error'] as String : '',
    );
  } catch (_) {
    return const CuratedSnapshot(error: '「精选」清单解析失败');
  }
}

/// 从「精选」安装 / 修复：走与 URL 安装同一条闸，只是不联网。
///
/// `overwrite` = 本地已有同名技能、确认覆盖；`acceptScan` / `acceptPermissions` =
/// 用户看过检查结果 / 权限 diff 后确认继续（回执会说明是哪一种）。
SkillResult installCuratedSkill({
  required String key,
  bool overwrite = false,
  bool acceptScan = false,
  bool acceptPermissions = false,
}) => SkillResult.fromRaw(
  _call1(
    'lunac_skills_install_curated',
    jsonEncode({
      'key': key,
      'overwrite': overwrite,
      'acceptScan': acceptScan,
      'acceptPermissions': acceptPermissions,
    }),
  ),
);

/// 读一份技能的 `SKILL.md` 原文。
SkillContent readSkill(String key) {
  final text = _call1('lunac_skills_read', key);
  if (text == null) {
    return const SkillContent(ok: false, error: '当前环境没有原生桥');
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map) {
      return const SkillContent(ok: false, error: '读取返回格式不对');
    }
    return SkillContent(
      ok: value['ok'] == true,
      key: value['key'] is String ? value['key'] as String : key,
      content: value['content'] is String ? value['content'] as String : '',
      error: value['error'] is String ? value['error'] as String : '',
      sha256: value['sha256'] is String ? value['sha256'] as String : '',
      allowedTools: _stringList(value['allowedTools']),
    );
  } catch (_) {
    return const SkillContent(ok: false, error: '读取失败');
  }
}

/// 新建 / 覆盖一份技能。
SkillResult saveSkill({required String key, required String content}) => SkillResult.fromRaw(
  _call1('lunac_skills_save', jsonEncode({'key': key, 'content': content})),
);

/// 删除一份技能（整个目录）。
SkillResult deleteSkill(String key) =>
    SkillResult.fromRaw(_call1('lunac_skills_delete', key));

/// 从 raw URL 安装一份技能（只收 https；同名要 `overwrite: true` 才覆盖）。
///
/// `acceptScan` = 用户已经看过 `scanBlocked` 的检查结果并选择继续。
/// `sha256` = 清单里钉的指纹（可空；给了就必须完全一致）。
/// `acceptPermissions` = 用户确认过「升级扩大了 allowed-tools」。
SkillResult installSkillFromUrl({
  required String key,
  required String url,
  bool overwrite = false,
  bool acceptScan = false,
  String sha256 = '',
  bool acceptPermissions = false,
}) => SkillResult.fromRaw(
  _call1(
    'lunac_skills_install',
    jsonEncode({
      'key': key,
      'url': url,
      'overwrite': overwrite,
      'acceptScan': acceptScan,
      'sha256': sha256.trim(),
      'acceptPermissions': acceptPermissions,
    }),
  ),
);

/// 扫一份技能正文（与安装闸同一份判定，§待办 3-28.9）。
SkillScan scanSkill(String content) {
  final text = _call1('lunac_skill_scan', content);
  if (text == null) {
    return SkillScan.unavailable;
  }
  try {
    return SkillScan.fromJson(jsonDecode(text)) ??
        const SkillScan(risk: 'warn');
  } catch (_) {
    return const SkillScan(risk: 'warn');
  }
}

typedef _TextC = Pointer<Utf8> Function();
typedef _TextDart = Pointer<Utf8> Function();
typedef _TextWithArgC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextWithArgDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

String? _call0(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_TextC, _TextDart>(symbol)();
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

String? _call1(String symbol, String argument) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextWithArgC, _TextWithArgDart>(symbol);
    arg = argument.toNativeUtf8();
    raw = call(arg);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (arg != null) {
      malloc.free(arg);
    }
    _free(library, raw);
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? raw) {
  if (raw == null || raw == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
  } catch (_) {
    // 旧桥没有这个符号：忽略。
  }
}
