// 平台薄层：开机自启状态与开关（v1 `auto_start.rs` 的 C ABI 包装）。
//
// 边界：注册表 / 计划任务 / 提权全部在 Rust（`rust/src/auto_start.rs`）；
// 本文件只负责「读状态、拨开关、识别提权子进程参数」。
//
// 开关是**同步阻塞**的（非管理员下会弹一次 UAC），界面侧用 `Isolate.run` 调用，
// 避免 UAC 等待期间冻结 UI。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 与 Rust 的 `ELEVATED_FLAG` 必须一致（`lunac.exe --lunac-auto-start-task=create|delete`）。
const String kElevatedAutoStartFlag = '--lunac-auto-start-task=';

/// 开机自启状态（与 Rust `AutoStartInfo::to_json` 一一对应）。
class AutoStartInfo {
  const AutoStartInfo({
    required this.enabled,
    required this.mechanism,
    required this.stale,
    required this.dev,
    required this.supported,
  });

  /// 是否已开启（计划任务或 Run 键任一存在）。
  final bool enabled;

  /// `task`（登录时计划任务）/ `run`（HKCU Run 键）/ `both` / `none`。
  final String mechanism;

  /// 已注册项指向的不是当前 exe（exe 被移动 / 升级 / 曾被别的构建注册过）。
  final bool stale;

  /// 当前是开发构建（不允许注册；面板禁用开关并说明原因）。
  final bool dev;

  /// 当前平台是否支持（Linux 暂不支持）。
  final bool supported;

  /// 给界面看的一句话机制说明（不暴露「注册表 / 计划任务」术语的部分只在 hint 里）。
  String get mechanismLabel => switch (mechanism) {
        'task' => '登录时启动（较快）',
        'run' => '登录时启动（较慢）',
        'both' => '登录时启动（两条都已注册）',
        _ => '未开启',
      };

  static AutoStartInfo? fromJson(Map<String, dynamic> json) {
    final enabled = json['enabled'];
    final mechanism = json['mechanism'];
    if (enabled is! bool || mechanism is! String) {
      return null;
    }
    return AutoStartInfo(
      enabled: enabled,
      mechanism: mechanism,
      stale: json['stale'] == true,
      dev: json['dev'] == true,
      supported: json['supported'] == true,
    );
  }
}

typedef _InfoC = Pointer<Utf8> Function();
typedef _InfoDart = Pointer<Utf8> Function();
typedef _SetC = Pointer<Utf8> Function(Int32);
typedef _SetDart = Pointer<Utf8> Function(int);
typedef _ElevatedC = Int32 Function(Pointer<Utf8>);
typedef _ElevatedDart = int Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读自启状态；桥接缺失返回 null（界面显示「不可用」而不是崩掉）。
AutoStartInfo? readAutoStart() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library
        .lookupFunction<_InfoC, _InfoDart>('lunac_auto_start_info')();
    if (raw == nullptr) {
      return null;
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic>) {
      return null;
    }
    return AutoStartInfo.fromJson(value);
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

/// 开启 / 关闭；返回 null = 已生效，否则是给用户看的原因（开关应弹回原状态）。
///
/// **可能长时间阻塞**（非管理员下弹 UAC 并等待用户确认）⇒ 调用方应在独立 isolate 里跑。
String? setAutoStart(bool enabled) {
  final library = openLunacNative().library;
  if (library == null) {
    return 'rust bridge not loaded';
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_SetC, _SetDart>('lunac_auto_start_set')(
      enabled ? 1 : 0,
    );
    if (raw == nullptr) {
      return '设置开机自启失败';
    }
    final reason = raw.toDartString().trim();
    return reason.isEmpty ? null : reason;
  } catch (_) {
    return '设置开机自启失败：rust 桥接不可用';
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

/// 提权子进程入口：执行一次计划任务动作并返回进程退出码（0 = 成功）。
int handleAutoStartElevated(String action) {
  final library = openLunacNative().library;
  if (library == null) {
    return 1;
  }
  Pointer<Utf8>? pointer;
  try {
    pointer = action.toNativeUtf8();
    return library
        .lookupFunction<_ElevatedC, _ElevatedDart>(
          'lunac_auto_start_elevated',
        )(pointer);
  } catch (_) {
    return 1;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 从命令行参数里认出提权动作（`create` / `delete`）；不是提权子进程返回 null。
String? parseElevatedAction(List<String> args) {
  for (final arg in args) {
    if (arg.startsWith(kElevatedAutoStartFlag)) {
      final action = arg.substring(kElevatedAutoStartFlag.length).trim();
      if (action.isNotEmpty) {
        return action;
      }
    }
  }
  return null;
}
