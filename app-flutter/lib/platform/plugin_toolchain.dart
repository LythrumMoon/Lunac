// 平台薄层：插件开发工具链（轨道 H，契约见 `agent-templates/modules/README.md` §开发工具链）。
//
// 边界：授权账本 / 下载校验 / 构建 / 停止都在 `rust/src/plugin_toolchain.rs`；本文件只做
// 数据形状与 C ABI。三件事**不在前端做**：
//   · 授权判定（`granted` 由宿主按 dev 段指纹算，前端不缓存结论）；
//   · 白名单与路径边界（`plugin_market` 一处定义，装插件时就校验）；
//   · 进程与停止（Job Object 由宿主持有，前端只能发「停」这个意图）。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _JsonC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _JsonDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 一个工具链 profile 的状态（`dev.toolchains[]` + 装没装）。
class ToolchainProfileStatus {
  const ToolchainProfileStatus({
    required this.id,
    required this.version,
    required this.url,
    required this.sha256,
    required this.kind,
    required this.dest,
    required this.binDir,
    required this.installed,
  });

  final String id;
  final String version;
  final String url;
  final String sha256;

  /// `file` / `archive`。
  final String kind;
  final String dest;
  final String binDir;

  /// 标记存在 + sha256 与清单一致 + 落点还在（宿主判定）。
  final bool installed;

  String get label => version.isEmpty ? id : '$id $version';

  static ToolchainProfileStatus? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final id = json['id'];
    if (id is! String || id.isEmpty) {
      return null;
    }
    return ToolchainProfileStatus(
      id: id,
      version: json['version'] is String ? json['version'] as String : '',
      url: json['url'] is String ? json['url'] as String : '',
      sha256: json['sha256'] is String ? json['sha256'] as String : '',
      kind: json['kind'] is String ? json['kind'] as String : 'file',
      dest: json['dest'] is String ? json['dest'] as String : '',
      binDir: json['binDir'] is String ? json['binDir'] as String : '',
      installed: json['installed'] == true,
    );
  }
}

/// `dev.build` 声明。
class ToolchainBuildSpec {
  const ToolchainBuildSpec({
    required this.command,
    required this.args,
    required this.cwd,
    required this.label,
  });

  final String command;
  final List<String> args;
  final String cwd;

  /// 宿主拼好的一行（`command args…`），界面直接显示，不自己拼。
  final String label;

  static ToolchainBuildSpec? fromJson(dynamic json) {
    if (json is! Map) {
      return null;
    }
    final command = json['command'];
    if (command is! String || command.trim().isEmpty) {
      return null;
    }
    return ToolchainBuildSpec(
      command: command,
      args: [
        for (final item in (json['args'] as List? ?? const []))
          if (item is String) item,
      ],
      cwd: json['cwd'] is String ? json['cwd'] as String : '',
      label: json['label'] is String ? json['label'] as String : command,
    );
  }
}

/// 一个插件的工具链状态（`lunac_plugin_toolchain_status`）。
class ToolchainStatus {
  const ToolchainStatus({
    required this.ok,
    required this.plugin,
    required this.name,
    required this.fingerprint,
    required this.toolchains,
    required this.build,
    required this.running,
    required this.granted,
    required this.error,
  });

  final bool ok;
  final String plugin;
  final String name;

  /// dev 段指纹（前 12 位在界面上显示；改任何 dev 字段都会变）。
  final String fingerprint;
  final List<ToolchainProfileStatus> toolchains;
  final ToolchainBuildSpec? build;

  /// 有构建在跑（宿主注册表里的一项）。
  final bool running;

  /// dev 段在当前指纹下有授权（下载与构建的闸）。
  final bool granted;
  final String error;

  /// 有 `dev` 段（清单声明了工具链或构建）。
  bool get hasDev => toolchains.isNotEmpty || build != null;

  static const unavailable = ToolchainStatus(
    ok: false,
    plugin: '',
    name: '',
    fingerprint: '',
    toolchains: [],
    build: null,
    running: false,
    granted: false,
    error: '本地组件未就绪，读不了工具链状态',
  );

  static ToolchainStatus fromJson(dynamic json) {
    if (json is! Map) {
      return unavailable;
    }
    return ToolchainStatus(
      ok: json['ok'] == true,
      plugin: json['plugin'] is String ? json['plugin'] as String : '',
      name: json['name'] is String ? json['name'] as String : '',
      fingerprint: json['fingerprint'] is String
          ? json['fingerprint'] as String
          : '',
      toolchains: [
        for (final item in (json['toolchains'] as List? ?? const []))
          ?ToolchainProfileStatus.fromJson(item),
      ],
      build: ToolchainBuildSpec.fromJson(json['build']),
      running: json['running'] == true,
      granted: json['granted'] == true,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 安装一个 profile 的结果。
class ToolchainStep {
  const ToolchainStep({
    required this.id,
    required this.ok,
    required this.bytes,
    required this.dest,
  });

  final String id;
  final bool ok;
  final int bytes;
  final String dest;

  static ToolchainStep fromJson(dynamic json) {
    if (json is! Map) {
      return const ToolchainStep(id: '', ok: false, bytes: 0, dest: '');
    }
    return ToolchainStep(
      id: json['id'] is String ? json['id'] as String : '',
      ok: json['ok'] == true,
      bytes: json['bytes'] is num ? (json['bytes'] as num).toInt() : 0,
      dest: json['dest'] is String ? json['dest'] as String : '',
    );
  }
}

/// 安装结果（`lunac_plugin_toolchain_install`）。
class ToolchainInstallResult {
  const ToolchainInstallResult({
    required this.ok,
    required this.steps,
    required this.error,
  });

  final bool ok;
  final List<ToolchainStep> steps;
  final String error;

  static ToolchainInstallResult fromJson(dynamic json) {
    if (json is! Map) {
      return const ToolchainInstallResult(ok: false, steps: [], error: '安装没有返回结果');
    }
    return ToolchainInstallResult(
      ok: json['ok'] == true,
      steps: [
        for (final item in (json['steps'] as List? ?? const []))
          ToolchainStep.fromJson(item),
      ],
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 构建结果（`lunac_plugin_toolchain_build`）。
class ToolchainBuildResult {
  const ToolchainBuildResult({
    required this.ok,
    required this.code,
    required this.durationMs,
    required this.timedOut,
    required this.stopped,
    required this.output,
    required this.error,
  });

  final bool ok;
  final int? code;
  final int durationMs;
  final bool timedOut;
  final bool stopped;
  final String output;
  final String error;

  static ToolchainBuildResult fromJson(dynamic json) {
    if (json is! Map) {
      return const ToolchainBuildResult(
        ok: false,
        code: null,
        durationMs: 0,
        timedOut: false,
        stopped: false,
        output: '',
        error: '构建没有返回结果',
      );
    }
    return ToolchainBuildResult(
      ok: json['ok'] == true,
      code: json['code'] is num ? (json['code'] as num).toInt() : null,
      durationMs: json['durationMs'] is num
          ? (json['durationMs'] as num).toInt()
          : 0,
      timedOut: json['timedOut'] == true,
      stopped: json['stopped'] == true,
      output: json['output'] is String ? json['output'] as String : '',
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 授权 / 撤销 / 停止的通用回执。
class ToolchainActionResult {
  const ToolchainActionResult({
    required this.ok,
    required this.removed,
    required this.stopped,
    required this.deleted,
    required this.error,
  });

  final bool ok;

  /// 撤销时是否真的删掉了授权记录。
  final bool removed;

  /// 是否有在跑的构建被停掉。
  final bool stopped;

  /// 撤销时是否把 `.toolchain\` 也删了。
  final bool deleted;
  final String error;

  static ToolchainActionResult fromJson(dynamic json) {
    if (json is! Map) {
      return const ToolchainActionResult(
        ok: false,
        removed: false,
        stopped: false,
        deleted: false,
        error: '操作没有返回结果',
      );
    }
    return ToolchainActionResult(
      ok: json['ok'] == true,
      removed: json['removed'] == true,
      stopped: json['stopped'] == true,
      deleted: json['deleted'] == true,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 读工具链状态（含 toolchains / build / running / granted）。
ToolchainStatus readToolchainStatus(String plugin) {
  final raw = _callJson('lunac_plugin_toolchain_status', {'plugin': plugin});
  return raw == null ? ToolchainStatus.unavailable : ToolchainStatus.fromJson(raw);
}

/// 授权整个 `dev` 段（**只有用户点「授权」才该调它**）。
ToolchainActionResult grantToolchain(String plugin) {
  final raw = _callJson('lunac_plugin_toolchain_grant', {'plugin': plugin});
  return raw == null
      ? const ToolchainActionResult(
          ok: false,
          removed: false,
          stopped: false,
          deleted: false,
          error: 'rust 桥接不可用',
        )
      : ToolchainActionResult.fromJson(raw);
}

/// 撤销授权并停掉在跑的构建；[deleteToolchain] = 同时删 `.toolchain\`。
ToolchainActionResult revokeToolchain(
  String plugin, {
  bool deleteToolchain = false,
}) {
  final raw = _callJson('lunac_plugin_toolchain_revoke', {
    'plugin': plugin,
    'deleteToolchain': deleteToolchain,
  });
  return raw == null
      ? const ToolchainActionResult(
          ok: false,
          removed: false,
          stopped: false,
          deleted: false,
          error: 'rust 桥接不可用',
        )
      : ToolchainActionResult.fromJson(raw);
}

/// 下载并安装全部工具链（同步、慢）：界面里要用 `Isolate.run` 包起来。
ToolchainInstallResult installToolchain(String plugin) {
  final raw = _callJson('lunac_plugin_toolchain_install', {'plugin': plugin});
  return raw == null
      ? const ToolchainInstallResult(ok: false, steps: [], error: 'rust 桥接不可用')
      : ToolchainInstallResult.fromJson(raw);
}

/// 跑 `dev.build`（同步、慢）：同样用 `Isolate.run`。
ToolchainBuildResult buildToolchain(String plugin) {
  final raw = _callJson('lunac_plugin_toolchain_build', {'plugin': plugin});
  return raw == null
      ? const ToolchainBuildResult(
          ok: false,
          code: null,
          durationMs: 0,
          timedOut: false,
          stopped: false,
          output: '',
          error: 'rust 桥接不可用',
        )
      : ToolchainBuildResult.fromJson(raw);
}

/// 停止在跑的构建（整棵进程树）。
ToolchainActionResult stopToolchain(String plugin) {
  final raw = _callJson('lunac_plugin_toolchain_stop', {'plugin': plugin});
  return raw == null
      ? const ToolchainActionResult(
          ok: false,
          removed: false,
          stopped: false,
          deleted: false,
          error: 'rust 桥接不可用',
        )
      : ToolchainActionResult.fromJson(raw);
}

dynamic _callJson(String symbol, Map<String, Object?> request) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? pointer;
  Pointer<Utf8>? out;
  try {
    final call = library.lookupFunction<_JsonC, _JsonDart>(symbol);
    pointer = jsonEncode(request).toNativeUtf8();
    out = call(pointer);
    if (out == nullptr) {
      return null;
    }
    return jsonDecode(out.toDartString());
  } catch (_) {
    return null;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
    if (out != null && out != nullptr) {
      _freeQuietly(library, out);
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
