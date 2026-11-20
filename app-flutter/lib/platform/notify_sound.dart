// 系统提示音（与 v1 `notify_sound` 同一个 MessageBeep「星号」音）。
//
// 平台差异只出现在这一层：Dart 只表达「响一声」，真正出声在 Rust
// （user32!MessageBeep）。没有桥接（单测 / 非 Windows）时静默返回 false ——
// 提示音是锦上添花，失败不上报，也不该影响对话收尾。
import 'dart:ffi';

import 'native_bridge.dart';

typedef _NotifySoundC = Int32 Function();
typedef _NotifySoundDart = int Function();

/// 播一次系统提示音；返回 false = 当前环境没有可用的提示音（不抛异常）。
bool playNotifySound() {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    library.lookupFunction<_NotifySoundC, _NotifySoundDart>(
      'lunac_notify_sound',
    )();
    return true;
  } catch (_) {
    return false;
  }
}
