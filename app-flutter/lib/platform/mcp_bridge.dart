// 宿主 MCP 桥（`lunac.exe --mcp-server`）的 Dart 入口。
//
// agent 把宿主可执行文件当 stdio MCP 服务器拉起，协议与实现都在 Rust 侧
// （`rust/src/mcp_bridge.rs`）；这里只负责「在 Flutter 起来之前」调用那一层阻塞循环 ——
// stdout 必须独占给 JSON-RPC，所以调用方拿到退出码后要立刻 exit。
import 'dart:ffi';

import 'native_bridge.dart';

typedef _ServeC = Int32 Function();
typedef _ServeDart = int Function();

/// 阻塞跑完 stdin 上的 MCP 会话；返回进程退出码。
///
/// 桥接库不可用（未构建 / ABI 不匹配）时返回 1：agent 侧会如实报「桥连不上」，
/// 不会出现「工具看着在、点了没反应」。
int serveMcpBridge() {
  final library = openLunacNative().library;
  if (library == null) {
    return 1;
  }
  try {
    final serve = library.lookupFunction<_ServeC, _ServeDart>('lunac_mcp_serve');
    return serve();
  } catch (_) {
    return 1;
  }
}
