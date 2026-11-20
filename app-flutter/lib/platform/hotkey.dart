// 平台薄层：单实例 / 全局热键 / 事件驱动的唤出 / 设置面板改键。
//
// 与 runner 的约定（`windows/runner/flutter_window.cpp`）：
//   · `lunac/wake` 通道：Dart 调 `hwnd` 取主窗句柄 → 交给 Rust（`lunac_set_host_window`）；
//     runner 收到 WM_APP+1 / +2 时回调 Dart 的 `wake` / `activate`；
//   · 这样热键线程 / 第二个实例只需 `PostMessage`，后台**零轮询**；
//   · 旧 DLL 或通道不可用时返回 false，`main.dart` 退回 `startWakeWatcher`（2 秒轮询计数）。
//
// 改键（设置面板「通用」）：`lunac_hotkey_combo` / `lunac_hotkey_set` —— Rust 在热键线程上
// 注销旧键、注册新键（失败回滚旧键），成功后写 `<exe 根>/config/hotkey.json`。
import 'dart:async';
import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';

import 'native_bridge.dart';

typedef _I32C = Int32 Function();
typedef _I32Dart = int Function();
typedef _U32C = Uint32 Function();
typedef _U32Dart = int Function();
typedef _IsizeC = IntPtr Function(IntPtr);
typedef _IsizeDart = int Function(int);
typedef _GetC = Pointer<Utf8> Function();
typedef _GetDart = Pointer<Utf8> Function();
typedef _SetC = Int32 Function(Pointer<Utf8>);
typedef _SetDart = int Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 默认唤醒热键（与 Rust / v1 同一份默认值；桥接缺失时用它显示）。
const String defaultHotkeyCombo = 'Ctrl+Alt+Space';

/// 与 runner 的唤出通道（`hwnd` 请求 + `wake` / `activate` 回调）。
const MethodChannel _wakeChannel = MethodChannel('lunac/wake');

/// 本进程是不是第一个实例；false = 已有实例在跑（调用方 signal 后立即退出）。
bool acquireSingleInstance() {
  final library = openLunacNative().library;
  if (library == null) {
    // 桥接缺失时不要误杀当前进程：当作首个实例继续起窗。
    return true;
  }
  try {
    final acquire = library.lookupFunction<_I32C, _I32Dart>(
      'lunac_single_instance_acquire',
    );
    return acquire() == 1;
  } catch (_) {
    return true;
  }
}

/// 通知已有实例把窗口唤出（第二个实例调用后自己退出）。
void signalExistingInstance() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<Void Function(), void Function()>(
      'lunac_single_instance_signal',
    )();
  } catch (_) {
    // 符号缺失时忽略。
  }
}

/// 启动全局热键线程（幂等；Rust 侧读 `config/hotkey.json`）。
void startGlobalHotkey() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<Void Function(), void Function()>(
      'lunac_hotkey_start',
    )();
  } catch (_) {
    // 符号缺失时忽略。
  }
}

/// 接上事件驱动的唤出：取主窗句柄交给 Rust，并订阅 `wake` / `activate`。
///
/// 返回 false = 桥接 / 通道不可用，调用方应退回 [startWakeWatcher]。
Future<bool> startWakeEvents({
  required VoidCallback onHotkey,
  required VoidCallback onActivate,
}) async {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    final hwnd = await _wakeChannel.invokeMethod<int>('hwnd');
    if (hwnd == null || hwnd == 0) {
      return false;
    }
    library.lookupFunction<_IsizeC, _IsizeDart>('lunac_set_host_window')(hwnd);
  } catch (_) {
    return false;
  }
  _wakeChannel.setMethodCallHandler((call) async {
    switch (call.method) {
      case 'wake':
        onHotkey();
      case 'activate':
        onActivate();
    }
    return null;
  });
  return true;
}

/// 兜底：2 秒轮询 Rust 的计数（事件通道不可用时才用）。
Timer? _wakeTimer;

void startWakeWatcher({
  required VoidCallback onHotkey,
  required VoidCallback onActivate,
}) {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  _U32Dart? takeWake;
  _U32Dart? takeActivate;
  try {
    takeWake = library.lookupFunction<_U32C, _U32Dart>('lunac_wake_take');
    takeActivate = library.lookupFunction<_U32C, _U32Dart>(
      'lunac_activate_take',
    );
  } catch (_) {
    return;
  }
  _wakeTimer?.cancel();
  _wakeTimer = Timer.periodic(const Duration(seconds: 2), (_) {
    if (takeWake!() > 0) {
      onHotkey();
    }
    if (takeActivate!() > 0) {
      onActivate();
    }
  });
}

/// 读当前热键；桥接缺失 = 默认组合。
String readHotkeyCombo() {
  final library = openLunacNative().library;
  if (library == null) {
    return defaultHotkeyCombo;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_GetC, _GetDart>('lunac_hotkey_combo')();
    if (raw == nullptr) {
      return defaultHotkeyCombo;
    }
    final combo = raw.toDartString().trim();
    return combo.isEmpty ? defaultHotkeyCombo : combo;
  } catch (_) {
    return defaultHotkeyCombo;
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

/// 改热键；成功返回 null，失败返回给用户看的原因。
///
/// 错误码（Rust）：1 组合非法 / 2 注册失败（被占用）/ 3 热键线程不可用。
String? setHotkeyCombo(String combo) {
  final library = openLunacNative().library;
  if (library == null) {
    return 'rust bridge not loaded';
  }
  Pointer<Utf8>? pointer;
  try {
    final set = library.lookupFunction<_SetC, _SetDart>('lunac_hotkey_set');
    pointer = combo.toNativeUtf8();
    final code = set(pointer);
    switch (code) {
      case 0:
        return null;
      case 1:
        return '组合键格式不正确：至少一个修饰键 + 一个主键';
      case 2:
        return '注册失败：该组合可能已被其它程序占用';
      case 3:
        return '热键线程不可用（Linux 暂未实现全局热键）';
      default:
        return '改键失败（错误码 $code）';
    }
  } catch (_) {
    return '改键失败：rust 桥接不可用';
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}
