// 插件运行时（sidecar）的平台解析与面板单测（不加载 DLL、不起进程）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/plugin_runtime.dart';
import 'package:lunac/search/plugin_console_panel.dart';
import 'package:lunac/theme.dart';

const PluginSidecarSpec _spec = PluginSidecarSpec(
  command: 'bin/llama-server.exe',
  args: ['-m', 'models/x.gguf'],
  transport: 'http',
  port: 8090,
  cwd: '',
  autostart: false,
  restart: 'on-failure',
  sha256: '',
  allowLocalPorts: [11434],
  allowLocalAny: false,
  readyTimeoutSec: 180,
  memoryLimitMiB: 8192,
  processLimit: 8,
);

/// 假桥：记录调用、按预设回包（单测不碰 FFI）。
class _FakeBridge implements PluginRuntimeBridge {
  _FakeBridge({PluginSidecarStatus? status})
    : _status = status ?? _idleStatus();

  PluginSidecarStatus _status;
  final PluginSidecarStart startReply = const PluginSidecarStart(
    ok: true,
    status: 'started',
    pluginId: 'demo',
    port: 8090,
  );
  final List<String> calls = [];

  static PluginSidecarStatus _idleStatus() => const PluginSidecarStatus(
    ok: true,
    pluginId: 'demo',
    hasSidecar: true,
    spec: _spec,
    dir: 'C:/lunac/Modules/demo',
  );

  @override
  PluginSidecarStatus readStatus(String pluginId) => _status;

  @override
  Future<PluginSidecarStart> start(String pluginId, String owner) async {
    calls.add('start:$owner');
    if (startReply.started) {
      _status = const PluginSidecarStatus(
        ok: true,
        pluginId: 'demo',
        hasSidecar: true,
        spec: _spec,
        dir: 'C:/lunac/Modules/demo',
        trusted: true,
        running: true,
        port: 8090,
      );
    }
    return startReply;
  }

  @override
  Future<PluginRuntimeReply> stop(String pluginId) async {
    calls.add('stop');
    return const PluginRuntimeReply(ok: true);
  }

  @override
  Future<PluginRuntimeReply> stopOwner(String owner) async {
    calls.add('stopOwner:$owner');
    return const PluginRuntimeReply(ok: true);
  }

  @override
  Future<PluginRuntimeReply> trust(String pluginId, {required bool remember}) async {
    calls.add('trust:$remember');
    return const PluginRuntimeReply(ok: true);
  }

  @override
  Future<PluginRuntimeReply> deny(String pluginId) async {
    calls.add('deny');
    return const PluginRuntimeReply(ok: true);
  }

  @override
  Future<PluginRuntimeCall> request(
    String pluginId,
    String method,
    String paramsJson,
  ) async {
    calls.add('request:$method:$paramsJson');
    return const PluginRuntimeCall(
      ok: true,
      result: {'ok': true, 'echo': 'yes'},
    );
  }

  @override
  Future<PluginRuntimeHttp> http(
    String pluginId,
    String method,
    String path,
    String body,
    int port,
  ) async {
    calls.add('http:$method:$path:$port');
    return const PluginRuntimeHttp(ok: true, status: 200, body: '{"status":"ok"}');
  }

  @override
  PluginRuntimeEvents poll(String pluginId, int since) => PluginRuntimeEvents(
    cursor: 2,
    events: const [
      {
        'seq': 1,
        'payload': {'method': 'event', 'params': {'n': 1}},
      },
    ],
    running: _status.running,
  );
}

Widget _wrap(Widget child) {
  return MaterialApp(
    theme: buildLunacTheme(),
    home: Scaffold(backgroundColor: LunacColors.background, body: child),
  );
}

void main() {
  test('运行时状态解析：spec / 信任 / 运行 / 退出原因', () {
    final status = PluginSidecarStatus.fromJson('''
    {"ok":true,"pluginId":"demo","hasSidecar":true,
     "spec":{"command":"bin/x.exe","args":["-a"],"transport":"http","port":8090,
             "readyTimeoutSec":15,"memoryLimitMiB":512,"processLimit":4,
             "allowLocalPorts":[11434],"allowLocalAny":false,"restart":"on-failure"},
     "trusted":false,"denied":false,"running":true,"port":8090,
     "exitReason":"","cursor":3,"dir":"C:/Modules/demo","error":""}
    ''');
    expect(status.ok, true);
    expect(status.hasSidecar, true);
    expect(status.running, true);
    expect(status.spec?.command, 'bin/x.exe');
    expect(status.spec?.usesHttp, true);
    expect(status.spec?.memoryLimitMiB, 512);
    expect(status.spec?.allowLocalPorts, [11434]);
    expect(status.cursor, 3);
  });

  test('运行时状态解析：坏 JSON 与缺 sidecar 都给人话', () {
    expect(PluginSidecarStatus.fromJson('{oops').ok, false);
    expect(PluginSidecarStatus.fromJson('{oops').error, '运行时状态不是合法 JSON');
    final none = PluginSidecarStatus.fromJson(
      '{"ok":true,"pluginId":"demo","hasSidecar":false}',
    );
    expect(none.ok, true);
    expect(none.hasSidecar, false);
    expect(none.spec, isNull);
  });

  test('启动结果解析：needs_trust 带信任卡内容', () {
    final start = PluginSidecarStart.fromJson('''
    {"ok":true,"status":"needs_trust","pluginId":"demo","port":null,
     "command":"bin/x.exe","args":["-m","x.gguf"],"sha256":"",
     "transport":"http","allowLocalPorts":[11434],"allowLocalAny":false,"error":""}
    ''');
    expect(start.needsTrust, true);
    expect(start.command, 'bin/x.exe');
    expect(start.args, ['-m', 'x.gguf']);
    expect(start.allowLocalPorts, [11434]);
  });

  test('事件解析：cursor / events / running', () {
    final events = PluginRuntimeEvents.fromJson('''
    {"ok":true,"cursor":7,"events":[{"seq":7,"payload":{"method":"ready"}}],
     "running":true,"exitReason":""}
    ''');
    expect(events.cursor, 7);
    expect(events.events.single['seq'], 7);
    expect(events.running, true);
  });

  testWidgets('面板：未信任时先弹信任卡，允许后启动', (tester) async {
    final bridge = _FakeBridge(
      status: const PluginSidecarStatus(
        ok: true,
        pluginId: 'demo',
        hasSidecar: true,
        spec: _spec,
        dir: 'C:/lunac/Modules/demo',
        cursor: 0,
      ),
    );
    await tester.pumpWidget(
      _wrap(
        PluginConsolePanel(
          onClose: () {},
          pluginId: 'demo',
          bridge: bridge,
          refreshInterval: null,
        ),
      ),
    );
    expect(find.text('信任卡'), findsOneWidget);
    expect(find.text('bin/llama-server.exe'), findsWidgets);
    expect(find.text('11434'), findsWidgets);
    expect(find.text('允许并记住'), findsOneWidget);
    expect(find.text('仅本次允许'), findsOneWidget);
    expect(find.text('拒绝'), findsOneWidget);

    await tester.tap(find.text('仅本次允许'));
    await tester.pumpAndSettle();
    expect(bridge.calls, contains('trust:false'));
    // 信任后会自动再启动一次（start 已写入 running 状态）。
    expect(bridge.calls, contains('start:$kPluginRuntimeOwner'));
    expect(find.text('已启动（http 通道，端口 8090）'), findsOneWidget);
    expect(find.text('停止'), findsOneWidget);
  });

  testWidgets('面板：运行中可发 stdio 调用并看到回包', (tester) async {
    final bridge = _FakeBridge(
      status: const PluginSidecarStatus(
        ok: true,
        pluginId: 'demo',
        hasSidecar: true,
        spec: _spec,
        dir: 'C:/lunac/Modules/demo',
        trusted: true,
        running: true,
        port: 8090,
        cursor: 0,
      ),
    );
    await tester.pumpWidget(
      _wrap(
        PluginConsolePanel(
          onClose: () {},
          pluginId: 'demo',
          bridge: bridge,
          refreshInterval: null,
        ),
      ),
    );
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();
    expect(bridge.calls, contains('request:ping:'));
    expect(find.textContaining('"echo": "yes"'), findsOneWidget);
  });

  testWidgets('面板：没声明 sidecar 的插件如实说明，不放空按钮', (tester) async {
    final bridge = _FakeBridge(
      status: const PluginSidecarStatus(
        ok: true,
        pluginId: 'web-only',
        hasSidecar: false,
      ),
    );
    await tester.pumpWidget(
      _wrap(
        PluginConsolePanel(
          onClose: () {},
          pluginId: 'web-only',
          bridge: bridge,
          refreshInterval: null,
        ),
      ),
    );
    expect(find.textContaining('没有在清单里声明 sidecar'), findsOneWidget);
    expect(find.text('启动'), findsNothing);
    expect(find.text('信任卡'), findsNothing);
  });

  testWidgets('面板：关闭时按 owner 收掉 sidecar', (tester) async {
    final bridge = _FakeBridge(
      status: const PluginSidecarStatus(
        ok: true,
        pluginId: 'demo',
        hasSidecar: true,
        spec: _spec,
        trusted: true,
        running: true,
      ),
    );
    await tester.pumpWidget(
      _wrap(
        PluginConsolePanel(
          onClose: () {},
          pluginId: 'demo',
          bridge: bridge,
          refreshInterval: null,
        ),
      ),
    );
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    expect(bridge.calls, contains('stopOwner:$kPluginRuntimeOwner'));
  });
}
