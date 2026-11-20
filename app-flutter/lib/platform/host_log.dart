// 宿主落盘日志的 Dart 薄层（对齐 v1 `temp\logs\lunac-*.log`）。
//
// 边界：**不在这里拼文件路径 / 不做轮转** —— 目录、文件名、保留天数、密钥打码全在
// Rust 侧（`app-flutter/rust/src/log.rs`）。Dart 只做两件事：
//   1. App 启动时把 Rust 的 logger 打开（`lunac_log_init`）；
//   2. 把 Dart 侧无人接收的异常（Flutter framework 错误 / isolate 未捕获错误）
//      转成一条 ERROR 落盘 —— release 是 GUI 子系统，没有控制台，不落盘就彻底丢了。
//
// 开关与级别由环境变量控制（与 Rust 同一口径）：`LUNAC_LOG=off` 关闭，
// `LUNAC_LOG_LEVEL=error|warn|info|debug`（缺省 info）。日志关着时本文件的调用全部是空操作。
import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';

import 'native_bridge.dart';

/// 级别取值必须与 `rust/src/lib.rs` 的 `lunac_log_write` 分派一致。
const int _levelError = 1;
const int _levelWarn = 2;
const int _levelInfo = 3;
const int _levelDebug = 4;

typedef _InitC = Void Function();
typedef _InitDart = void Function();
typedef _WriteC = Void Function(Uint32, Pointer<Utf8>);
typedef _WriteDart = void Function(int, Pointer<Utf8>);

/// 日志是否已经初始化过（桥接库缺失时也置 true，避免每次写都重试查符号）。
bool _initialized = false;
/// 桥接可用但函数查不到（旧 DLL）：后续调用直接放弃。
bool _unavailable = false;

/// 打开 Rust 侧 logger（幂等）。桥接不可用时静默返回 —— 骨架窗口仍要能起来。
void initHostLog() {
  if (_initialized) {
    return;
  }
  _initialized = true;
  final library = openLunacNative().library;
  if (library == null) {
    _unavailable = true;
    return;
  }
  try {
    library.lookupFunction<_InitC, _InitDart>('lunac_log_init')();
  } catch (_) {
    // 旧桥没有这个符号：整体降级为「不落盘」，不抛给调用方。
    _unavailable = true;
  }
}

/// 写一条 INFO。
void hostLogInfo(String message) => _write(_levelInfo, message);

/// 写一条 WARN。
void hostLogWarn(String message) => _write(_levelWarn, message);

/// 写一条 ERROR。
void hostLogError(String message) => _write(_levelError, message);

/// 写一条 DEBUG（`LUNAC_LOG_LEVEL` 没到 debug 时 Rust 侧会丢弃）。
void hostLogDebug(String message) => _write(_levelDebug, message);

/// 接管 Dart 侧无人接收的异常。
///
/// 必须在 `WidgetsFlutterBinding.ensureInitialized()` **之后**调用：绑定初始化会重置
/// `FlutterError.onError`。两类错误都只在 debug 下额外 `debugPrint` 一份，release 只落盘。
void installDartLogHooks() {
  final previous = FlutterError.onError;
  FlutterError.onError = (details) {
    _write(
      _levelError,
      'flutter error: ${details.exceptionAsString()}'
      '${details.context == null ? '' : ' (${details.context})'}'
      '${details.library == null ? '' : ' lib=${details.library}'}'
      '${details.stack == null ? '' : '\n${details.stack}'}',
    );
    if (kDebugMode) {
      debugPrint('[host-log] flutter error: ${details.exceptionAsString()}');
    }
    // 交回原处理器（debug 下仍会在控制台打红屏信息）。
    previous?.call(details);
  };
  PlatformDispatcher.instance.onError = (error, stack) {
    _write(_levelError, 'uncaught: $error\n$stack');
    if (kDebugMode) {
      debugPrint('[host-log] uncaught: $error');
    }
    // false = 没有完全消费：debug 下仍让 Flutter 自己报出来。
    return false;
  };
}

void _write(int level, String message) {
  if (!_initialized) {
    initHostLog();
  }
  if (_unavailable) {
    return;
  }
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  Pointer<Utf8>? text;
  try {
    text = message.toNativeUtf8();
    library.lookupFunction<_WriteC, _WriteDart>('lunac_log_write')(level, text);
  } catch (_) {
    _unavailable = true;
  } finally {
    if (text != null) {
      malloc.free(text);
    }
  }
}
