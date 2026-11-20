// VSCode 桥的 Dart 侧契约：解码状态 + 桥接缺失时的降级。
//
// HTTP / CORS / SSE 的判定在 Rust 单测里（`app-flutter/rust/src/vscode_bridge.rs`，
// 含一条真的在 127.0.0.1:8789 上打 `/health` 的用例）；这里只测解码与「没有 DLL 时不许抛」。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/vscode_bridge.dart';

void main() {
  test('status decodes running / addr / subscribers', () {
    final status = VscodeBridgeStatus.fromRaw(
      '{"running":true,"addr":"127.0.0.1:8789","subscribers":2}',
    );
    expect(status, isNotNull);
    expect(status!.running, isTrue);
    expect(status.addr, '127.0.0.1:8789');
    expect(status.subscribers, 2);

    final idle = VscodeBridgeStatus.fromRaw(
      '{"running":false,"addr":"127.0.0.1:8789","subscribers":0}',
    );
    expect(idle!.running, isFalse);
  });

  test('missing or broken payloads degrade instead of throwing', () {
    expect(VscodeBridgeStatus.fromRaw(null), isNull);
    expect(VscodeBridgeStatus.fromRaw('[]'), isNull);
    expect(VscodeBridgeStatus.fromRaw('not json'), isNull);
    // 字段缺失时给安全默认值（不因为少一个键就整块不可用）。
    final partial = VscodeBridgeStatus.fromRaw('{"running":true}');
    expect(partial!.addr, isEmpty);
    expect(partial.subscribers, 0);
  });

  test('without the native bridge, start / status / stop stay quiet', () {
    // 测试进程没有 `lunac_core.dll`：起桥必须返回一句原因（不抛），状态为 null。
    final reason = startVscodeBridge();
    expect(reason, isNotNull);
    expect(reason, contains('桥接未加载'));
    expect(readVscodeBridgeStatus(), isNull);
    stopVscodeBridge(); // 不抛即通过
  });
}
