// 平台薄层：应用自更新（检查 → 下载 → sha256 校验 → 静默安装交棒）。
//
// v1（`app/src-tauri/src/updater.rs`）的完整链路是「检查 → 下载 → sha256 校验 →
// 静默安装 → 退出」。v2 按同一口径接通：清单校验（版本可解析 / 直链 https 且落在
// GitHub 域 / sha256 64 位十六进制）→ 流式下载 + 增量 sha256 → `Setup.exe /S` 交棒，
// 由安装器覆盖 exe 并重新拉起。开发构建（`flutter run` / `flutter test`）拒绝自更新，
// 与 v1 的 `is_dev_build` 同一条判据。
//
// 网络用 `dart:io` 的 HttpClient：不引第三方 HTTP 依赖，也不把这一步塞进 Rust 薄层
// （它不涉及平台差异）。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart' show kDebugMode;

import 'native_bridge.dart';
import 'sha256.dart';

/// 当前版本。发布前与 v1 同一套版本号（`app/src-tauri/Cargo.toml`）；M6 打包时改成
/// 构建注入，不再手改这里。
const String kAppVersion = '0.9.45';

/// 版本清单的固定入口（GitHub「最新一个非预发布 Release」的资产）。
const String kUpdateManifestUrl =
    'https://github.com/LythrumMoon/Lunac/releases/latest/download/latest.json';

/// 发布页（检查到更新后「前往下载」打开这里；静默安装接上后这个入口仍保留）。
const String kReleasePageUrl =
    'https://github.com/LythrumMoon/Lunac/releases/latest';

/// 清单是小文件；超了说明拿到的不是清单（例如被某个首页顶替了）。
const int kMaxManifestBytes = 64 * 1024;

/// 检查 / 下载超时：建连与读数据都不该挂太久（与 v1 的 20s 口径一致）。
const Duration kUpdateCheckTimeout = Duration(seconds: 20);

/// 安装包上限：512MB。超了直接拒绝，别把磁盘写满（v1 同一条硬上限）。
const int kMaxInstallerBytes = 512 * 1024 * 1024;

/// 自动更新配置（v1 `updater::UpdateConfig`，`config/update.json` 同一格式）。
///
/// 设置面板只有一个开关：两个字段同真同假才算开（v1 2026-10-05 的合并语义）。
class UpdateConfig {
  const UpdateConfig({
    required this.checkOnStartup,
    required this.autoInstall,
  });

  final bool checkOnStartup;
  final bool autoInstall;

  /// 面板开关的取值。
  bool get enabled => checkOnStartup && autoInstall;

  static const UpdateConfig defaults = UpdateConfig(
    checkOnStartup: true,
    autoInstall: true,
  );
}

typedef _UpdateConfigC = Pointer<Utf8> Function();
typedef _UpdateConfigDart = Pointer<Utf8> Function();
typedef _SetUpdateAutoC = Pointer<Utf8> Function(Int32);
typedef _SetUpdateAutoDart = Pointer<Utf8> Function(int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

void _freeString(Pointer<Utf8> pointer) {
  try {
    final library = openLunacNative().library;
    library?.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}

/// 读自动更新配置；桥接不可用 / 读坏时按默认（开）处理，与 v1 同一条判据。
UpdateConfig readUpdateConfig() {
  final library = openLunacNative().library;
  if (library == null) {
    return UpdateConfig.defaults;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_UpdateConfigC, _UpdateConfigDart>(
      'lunac_update_config',
    )();
    if (raw == nullptr) {
      return UpdateConfig.defaults;
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic>) {
      return UpdateConfig.defaults;
    }
    return UpdateConfig(
      checkOnStartup: value['checkOnStartup'] is bool
          ? value['checkOnStartup'] as bool
          : true,
      autoInstall: value['autoInstall'] is bool
          ? value['autoInstall'] as bool
          : true,
    );
  } catch (_) {
    return UpdateConfig.defaults;
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeString(pointer);
    }
  }
}

/// 写「自动更新」开关；返回 null = 已保存，否则是一句给用户看的原因。
String? setUpdateAuto(bool on) {
  final library = openLunacNative().library;
  if (library == null) {
    return '当前环境不支持保存更新设置';
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_SetUpdateAutoC, _SetUpdateAutoDart>(
      'lunac_set_update_auto',
    )(on ? 1 : 0);
    if (raw == nullptr) {
      return '保存更新设置失败';
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic> || value['ok'] != true) {
      final reason = value is Map<String, dynamic> ? value['error'] : null;
      return reason is String && reason.isNotEmpty ? reason : '保存更新设置失败';
    }
    return null;
  } catch (_) {
    return '保存更新设置失败';
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      _freeString(pointer);
    }
  }
}

/// 启动时的静默自更新（v1 `update_check_silent`：检查 → 下载 → 拉起安装器）。
///
/// 返回 true = 已经交棒给安装器，调用方应当退出应用；开发构建 / 关着开关 /
/// 已是最新 / 任何失败都返回 false（启动时的失败不打扰用户，v1 同口径）。
Future<bool> autoUpdateOnStartup({required String installDir}) async {
  if (_isDevBuild() || !readUpdateConfig().enabled) {
    return false;
  }
  final check = await checkForUpdate();
  final status = check.status;
  if (status == null || !status.hasUpdate) {
    return false;
  }
  final download = await downloadInstaller(status, installDir: installDir);
  final installer = download.installerPath;
  if (installer == null) {
    return false;
  }
  return await launchInstaller(installer, installDir: installDir) == null;
}

/// 下载进度回调的最小步长（字节）：每 256KB 推一次，进度够平滑又不刷爆界面。
const int _progressStep = 256 * 1024;

/// 最新版本信息（字段名与 `latest.json` 一一对应）。
class UpdateStatus {
  const UpdateStatus({
    required this.current,
    required this.hasUpdate,
    required this.latest,
    required this.notes,
    required this.size,
    required this.url,
    required this.sha256,
  });

  final String current;
  final bool hasUpdate;
  final String latest;
  final String notes;
  final int size;
  final String url;

  /// 安装包的 sha256（小写十六进制，64 位）；下载完必须与它一致才允许安装。
  final String sha256;
}

/// 下载安装包的结果：`installerPath` 与 `error` 互斥。
class UpdateDownloadResult {
  const UpdateDownloadResult.ok(String this.installerPath) : error = null;

  const UpdateDownloadResult.failed(String this.error) : installerPath = null;

  final String? installerPath;
  final String? error;
}

/// 检查结果：`status` 与 `error` 互斥（失败时不假装「已是最新」）。
class UpdateCheck {
  const UpdateCheck.ok(UpdateStatus this.status) : error = null;

  const UpdateCheck.failed(String this.error) : status = null;

  final UpdateStatus? status;
  final String? error;
}

/// 检查更新（手动触发；启动时的静默检查等 M6 接安装器后再开）。
Future<UpdateCheck> checkForUpdate() async {
  final client = HttpClient()
    ..connectionTimeout = kUpdateCheckTimeout
    ..userAgent = 'Lunac/$kAppVersion';
  try {
    final request = await client
        .getUrl(Uri.parse(kUpdateManifestUrl))
        .timeout(kUpdateCheckTimeout);
    final response = await request.close().timeout(kUpdateCheckTimeout);
    if (response.statusCode == HttpStatus.notFound) {
      return const UpdateCheck.failed('还没有发布过可更新的版本');
    }
    if (response.statusCode != HttpStatus.ok) {
      return const UpdateCheck.failed('获取更新信息失败 —— 请稍后重试');
    }
    final bytes = <int>[];
    await for (final chunk in response.timeout(kUpdateCheckTimeout)) {
      bytes.addAll(chunk);
      if (bytes.length > kMaxManifestBytes) {
        return const UpdateCheck.failed('更新信息异常 —— 请到 GitHub 手动下载');
      }
    }
    final text = utf8.decode(bytes, allowMalformed: true);
    return parseUpdateManifest(text);
  } on TimeoutException {
    return const UpdateCheck.failed('连接超时 —— 请检查网络后重试');
  } on SocketException {
    return const UpdateCheck.failed('连不上更新服务器 —— 请检查网络或代理');
  } on FormatException {
    return const UpdateCheck.failed('更新信息格式不对 —— 请到 GitHub 手动下载');
  } catch (_) {
    return const UpdateCheck.failed('获取更新信息失败 —— 请稍后重试');
  } finally {
    client.close(force: true);
  }
}

/// 解析 + 校验清单文本（与网络解耦：真实清单可落盘后单测 / 探针复用）。
UpdateCheck parseUpdateManifest(String text) {
  final Object? value;
  try {
    value = jsonDecode(text.trimLeft().replaceFirst('\uFEFF', ''));
  } on FormatException {
    return const UpdateCheck.failed('更新信息格式不对 —— 请到 GitHub 手动下载');
  }
  if (value is! Map<String, dynamic>) {
    return const UpdateCheck.failed('更新信息格式不对 —— 请到 GitHub 手动下载');
  }
  final manifest = UpdateManifest.fromJson(value);
  final problem = validateManifest(manifest);
  if (problem != null) {
    return UpdateCheck.failed(problem);
  }
  return UpdateCheck.ok(
    UpdateStatus(
      current: kAppVersion,
      hasUpdate: isNewerVersion(kAppVersion, manifest.version),
      latest: manifest.version,
      notes: manifest.notes,
      size: manifest.size,
      url: manifest.url,
      sha256: manifest.sha256,
    ),
  );
}

/// `latest.json` 的形状（与 `scripts\publish-release.ps1` 生成的那份一一对应）。
class UpdateManifest {
  const UpdateManifest({
    required this.version,
    required this.notes,
    required this.url,
    required this.sha256,
    required this.size,
  });

  final String version;
  final String notes;
  final String url;
  final String sha256;
  final int size;

  static UpdateManifest fromJson(Map<String, dynamic> json) {
    return UpdateManifest(
      version: json['version'] is String ? json['version'] as String : '',
      notes: json['notes'] is String ? json['notes'] as String : '',
      url: json['url'] is String ? json['url'] as String : '',
      sha256: json['sha256'] is String ? json['sha256'] as String : '',
      size: json['size'] is int ? json['size'] as int : 0,
    );
  }
}

/// 清单是指向可执行文件的指令 ⇒ 下之前先按最小必要校验一遍（与 v1 同口径）。
/// 返回 null = 通过；否则是一句给用户看的原因。
String? validateManifest(UpdateManifest manifest) {
  if (parseVersion(manifest.version) == null) {
    return '更新信息异常（版本号不对）—— 请到 GitHub 手动下载';
  }
  if (!manifest.url.startsWith('https://')) {
    return '更新信息异常（下载地址不安全）—— 请到 GitHub 手动下载';
  }
  // 只认 GitHub 自己的域：清单若被改动过，也换不到别的站点去。
  const hosts = [
    'https://github.com/',
    'https://objects.githubusercontent.com/',
    'https://githubusercontent.com/',
  ];
  if (!hosts.any(manifest.url.startsWith)) {
    return '更新信息异常（下载来源不对）—— 请到 GitHub 手动下载';
  }
  final hex = RegExp(r'^[0-9a-fA-F]{64}$');
  if (!hex.hasMatch(manifest.sha256)) {
    return '更新信息异常（缺少校验值）—— 请到 GitHub 手动下载';
  }
  return null;
}

/// `x.y.z` → `[x, y, z]`；允许前置 `v`、允许只写两段（第三段补 0）；
/// 带后缀（`0.9.8-rc1`）或非数字返回 null。
List<int>? parseVersion(String version) {
  final text = version.trim().replaceFirst(RegExp(r'^[vV]'), '');
  final parts = text.split('.');
  if (parts.length < 2 || parts.length > 3) {
    return null;
  }
  final numbers = <int>[];
  for (final part in parts) {
    if (!RegExp(r'^\d+$').hasMatch(part)) {
      return null;
    }
    numbers.add(int.parse(part));
  }
  while (numbers.length < 3) {
    numbers.add(0);
  }
  return numbers;
}

/// `latest` 是否比 `current` 新；任一侧解析不出来一律 false（宁可漏报一次，
/// 也不要因为上游写了个奇怪版本号就反复弹更新）。
bool isNewerVersion(String current, String latest) {
  final a = parseVersion(current);
  final b = parseVersion(latest);
  if (a == null || b == null) {
    return false;
  }
  for (var i = 0; i < 3; i++) {
    if (b[i] != a[i]) {
      return b[i] > a[i];
    }
  }
  return false;
}

/// 是否开发构建（Flutter 侧判据，与 v1 `auto_start::is_dev_build` 同一条理由）。
///
/// 开发构建自更新会把安装版装进 `target\debug` / `runner\debug`，把开发环境搅乱；
/// 而 `flutter test` 下 `kDebugMode` 也为真，顺带拦住测试里的误触。
bool _isDevBuild() {
  final executable =
      Platform.resolvedExecutable.toLowerCase().replaceAll('/', '\\');
  return kDebugMode ||
      executable.contains('\\runner\\debug\\') ||
      executable.contains('\\runner\\release\\') ||
      executable.contains('\\target\\debug\\') ||
      executable.contains('\\target\\release\\');
}

/// 下载目录：`<installDir>/temp/update`（与 v1 同一位置；整目录只服务自更新一件事）。
String _updateDirectory(String installDir) {
  final root = installDir.replaceAll(RegExp(r'[\\/]+$'), '');
  return [root, 'temp', 'update'].join(Platform.pathSeparator);
}

/// 下载安装包到 `<installDir>/temp/update`，边下边算 sha256，下完校验。
///
/// 不复用旧文件：目录里可能躺着上次中断的半成品或旧版本安装包，先清空再下。
Future<UpdateDownloadResult> downloadInstaller(
  UpdateStatus status, {
  required String installDir,
  void Function(int downloaded, int total)? onProgress,
}) async {
  if (_isDevBuild()) {
    return const UpdateDownloadResult.failed('开发构建不支持自更新 —— 请改用安装版');
  }
  final client = HttpClient()
    ..connectionTimeout = kUpdateCheckTimeout
    ..userAgent = 'Lunac/$kAppVersion';
  final directory = Directory(_updateDirectory(installDir));
  File? destination;
  IOSink? sink;
  var succeeded = false;
  try {
    await directory.create(recursive: true);
    await for (final entity in directory.list()) {
      if (entity is File) {
        try {
          await entity.delete();
        } catch (_) {
          // 删不掉旧文件不影响本次下载：目标文件按版本命名，覆盖写即可。
        }
      }
    }
    final installer = File(
      '${directory.path}${Platform.pathSeparator}Lunac-${status.latest}-Setup.exe',
    );
    destination = installer;
    final request = await client
        .getUrl(Uri.parse(status.url))
        .timeout(kUpdateCheckTimeout);
    final response = await request.close().timeout(kUpdateCheckTimeout);
    if (response.statusCode != HttpStatus.ok) {
      return const UpdateDownloadResult.failed(
        '安装包暂时取不到 —— 请稍后重试，或到 GitHub 手动下载',
      );
    }

    // Content-Length 优先（上游实际给的），没有才退回清单里的 size；0 = 未知。
    final declared = response.contentLength;
    final total = declared > 0 ? declared : status.size;
    if (total > kMaxInstallerBytes) {
      return const UpdateDownloadResult.failed('安装包体积异常 —— 请到 GitHub 手动下载');
    }

    final output = installer.openWrite();
    sink = output;
    final hasher = Sha256();
    var downloaded = 0;
    var lastReported = 0;
    await for (final chunk in response.timeout(kUpdateCheckTimeout)) {
      downloaded += chunk.length;
      // Content-Length 可以撒谎、也可能没给：读的时候再兜一次上限。
      if (downloaded > kMaxInstallerBytes) {
        return const UpdateDownloadResult.failed('安装包体积异常 —— 请到 GitHub 手动下载');
      }
      hasher.update(chunk);
      output.add(chunk);
      if (downloaded - lastReported >= _progressStep) {
        lastReported = downloaded;
        onProgress?.call(downloaded, total);
      }
    }
    sink = null;
    await output.flush();
    await output.close();
    // 最后一次必调：不足一个步长的尾巴也要让界面走到 100%。
    onProgress?.call(downloaded, total);

    // 清单写了 size 就用它先兜一道；sha256 才是硬判据。
    if (status.size > 0 && downloaded != status.size) {
      return const UpdateDownloadResult.failed('安装包不完整 —— 请重试');
    }
    if (hasher.hexDigest().toLowerCase() != status.sha256.toLowerCase()) {
      return const UpdateDownloadResult.failed(
        '安装包校验失败 —— 文件可能不完整，请重试',
      );
    }
    succeeded = true;
    return UpdateDownloadResult.ok(destination.path);
  } on TimeoutException {
    return const UpdateDownloadResult.failed('连接超时 —— 请检查网络后重试');
  } on SocketException {
    return const UpdateDownloadResult.failed('连不上更新服务器 —— 请检查网络或代理');
  } on FileSystemException {
    return const UpdateDownloadResult.failed(
      '无法写入安装包 —— 请检查安装目录权限与磁盘空间',
    );
  } on FormatException {
    return const UpdateDownloadResult.failed('更新信息异常 —— 请到 GitHub 手动下载');
  } catch (_) {
    return const UpdateDownloadResult.failed('下载安装包失败 —— 请稍后重试');
  } finally {
    final openSink = sink;
    if (openSink != null) {
      try {
        await openSink.close();
      } catch (_) {
        // 失败路径上关闭写流出错不再叠加错误。
      }
    }
    // 失败时删掉半成品；成功时保留（安装器要用它）。
    final path = destination;
    if (!succeeded && path != null) {
      try {
        if (await path.exists()) {
          await path.delete();
        }
      } catch (_) {
        // 删不掉也不影响已经确定的失败原因。
      }
    }
    client.close(force: true);
  }
}

/// 拉起静默安装：`Process.start(installerPath, ['/S', '/D=$installDir'])`。
///
/// NSIS 的静默开关是 `/S`（**大小写敏感**）；`/D=<目录>` 直接设定 `$INSTDIR`，
/// **必须是最后一个参数且不能带引号**（路径含空格也一样，v1 结论见
/// `app/src-tauri/src/updater.rs`）。所以这里用参数列表交给系统拼命令行，不自己拼字符串。
///
/// 返回 null = 已拉起（随后调用方负责退出应用交棒）；否则是一句给用户看的原因。
Future<String?> launchInstaller(
  String installerPath, {
  required String installDir,
}) async {
  try {
    await Process.start(installerPath, ['/S', '/D=$installDir']);
    return null;
  } on ProcessException {
    return '无法启动安装程序 —— 请手动运行那个安装包';
  } catch (_) {
    return '无法启动安装程序 —— 请手动运行那个安装包';
  }
}
