// 平台薄层：VSCode 扩展的本机 HTTP / SSE 桥（v1 `agent_server.rs` + `cli_bridge.rs`）。
//
// 桥本身全在 `rust/src/vscode_bridge.rs`（端点 / CORS / SSE / 广播）；这里只有三个 C ABI：
// 起桥、读状态、停桥。**它只是个监听器** —— 真正的 agent 进程仍由 `agent.dart` 那条链路
// 管理（`lunac_agent_start` / `lunac_agent_send`），不在这里再造一份。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';
import 'package:lunac/platform/host_log.dart';

/// 桥的诊断状态。
class VscodeBridgeStatus {
  const VscodeBridgeStatus({
    required this.running,
    required this.addr,
    required this.subscribers,
  });

  final bool running;
  final String addr;
  final int subscribers;

  static VscodeBridgeStatus? fromRaw(String? raw) {
    if (raw == null) {
      return null;
    }
    try {
      final value = jsonDecode(raw);
      if (value is! Map<String, dynamic>) {
        return null;
      }
      return VscodeBridgeStatus(
        running: value['running'] == true,
        addr: value['addr'] is String ? value['addr'] as String : '',
        subscribers: value['subscribers'] is int
            ? value['subscribers'] as int
            : 0,
      );
    } catch (_) {
      return null;
    }
  }
}

/// 起桥（幂等；失败只写日志，绝不阻塞启动）。返回失败原因，成功返回 null。
///
/// 为什么吞掉失败：桥是给 VSCode 扩展用的可选通道 —— 端口被别的程序占着时，
/// 桌面端本身必须照常可用（v1 也是只 `eprintln!` 一行）。
String? startVscodeBridge() {
  final raw = _callString0('lunac_vscode_bridge_start');
  if (raw == null) {
    return '不可用（rust 桥接未加载）';
  }
  try {
    final value = jsonDecode(raw);
    if (value is Map && value['ok'] == true) {
      return null;
    }
    final reason = value is Map && value['error'] is String
        ? value['error'] as String
        : '未知原因';
    hostLogWarn('VSCode 桥未启动：$reason');
    return reason;
  } catch (e) {
    return '返回不是合法 JSON：$e';
  }
}

/// 读桥状态（不阻塞；桥接缺失返回 null）。
VscodeBridgeStatus? readVscodeBridgeStatus() =>
    VscodeBridgeStatus.fromRaw(_callString0('lunac_vscode_bridge_status'));

/// 停桥（只关监听与 SSE，不杀 agent）。
void stopVscodeBridge() {
  final library = openLunacNative().library;
  if (library == null) {
    return;
  }
  try {
    library.lookupFunction<Void Function(), void Function()>(
      'lunac_vscode_bridge_stop',
    )();
  } catch (_) {
    // 符号缺失（旧 DLL）时忽略。
  }
}

typedef _String0C = Pointer<Utf8> Function();
typedef _String0Dart = Pointer<Utf8> Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

String? _callString0(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_String0C, _String0Dart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null) {
      _free(library, raw);
    }
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? pointer) {
  if (pointer == null || pointer == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}
