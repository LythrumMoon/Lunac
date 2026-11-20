// 平台薄层：读取 Rust 侧 agent 状态（M0 起）。
//
// 库加载与 ABI 校验统一在 `native_bridge.dart`，本文件只保留状态读取一件事。
// 任何失败都返回 `agent: …` 形式的说明文本，不抛异常。
import 'dart:convert';
import 'dart:ffi';

import 'native_bridge.dart';

typedef _StatusC = Pointer<Uint8> Function();
typedef _StatusDart = Pointer<Uint8> Function();

/// 读取 Rust 侧 agent 状态（失败时返回 `agent: …` 形式的说明文本）。
String readAgentStatus() {
  final opened = openLunacNative();
  final library = opened.library;
  if (library == null) {
    return 'agent: ${opened.reason}';
  }
  try {
    final status =
        library.lookupFunction<_StatusC, _StatusDart>('lunac_agent_status')();
    return _readCString(status);
  } catch (e) {
    return 'agent: rust bridge error ($e)';
  }
}

/// 读 NUL 结尾的 UTF-8 串。静态串不会很长；额外加 4096 字节上限，
/// 防止坏指针把进程拖死在无界循环里。
String _readCString(Pointer<Uint8> pointer) {
  if (pointer == nullptr) {
    return '';
  }
  final bytes = <int>[];
  for (var i = 0; i < 4096; i++) {
    final byte = pointer[i];
    if (byte == 0) {
      break;
    }
    bytes.add(byte);
  }
  return utf8.decode(bytes, allowMalformed: true);
}
