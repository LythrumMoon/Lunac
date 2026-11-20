// 代理平台薄层与面板的单测（不加载 DLL、不读写系统代理）。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/proxy.dart';
import 'package:lunac/search/proxy_panel.dart';
import 'package:lunac/search/search_bridge.dart';
import 'package:lunac/search/search_page.dart';
import 'package:lunac/search/search_result.dart';
import 'package:lunac/theme.dart';

class _FakeBackend extends SearchBackend {
  _FakeBackend(this.items);

  final List<SearchResult> items;
  final List<String> actions = [];
  SearchActionOutcome outcome = SearchActionOutcome.uiHandled;

  @override
  String? get unavailableReason => null;

  @override
  List<SearchResult> search(String query) => items;

  @override
  SearchActionOutcome runAction(String id) {
    actions.add(id);
    return outcome;
  }
}

Widget _wrap(SearchPage page) =>
    MaterialApp(theme: buildLunacTheme(), home: page);

Finder _row(String text) =>
    find.descendant(of: find.byType(ListView), matching: find.text(text));

void _useTallSurface(WidgetTester tester) {
  tester.view.physicalSize = const Size(1000, 900);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

void main() {
  group('输入拆分', () {
    test('多行 Clash YAML 整段保留，不按行拼坏', () {
      const yaml = '''
port: 7890
proxies:
  - name: "a"
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: pw
rules:
  - MATCH,PROXY
''';
      final split = splitProxyInput(yaml);
      expect(split.links, isEmpty);
      expect(split.subscription, yaml.trim());
      expect(split.subscription, contains('\n  - name: "a"'));
    });

    test('分享链接与 base64 订阅可以混着贴', () {
      final split = splitProxyInput(
        'ss://YWVzLTI1Ni1nY206cHc@a.com:1#A\n'
        'dHVpYzovL3B3QGIuY29tOjI=\n',
      );
      expect(split.links, ['ss://YWVzLTI1Ni1nY206cHc@a.com:1#A']);
      expect(split.subscription, 'dHVpYzovL3B3QGIuY29tOjI=');
    });
  });
  test('status parsing maps system, local and kernel', () {
    final status = parseProxyStatus(
      '{"supported":true,'
      '"system":{"enabled":true,"server":"http://127.0.0.1:8080","bypass":"<local>;localhost"},'
      '"backup":true,'
      '"local":{"supported":false,"running":false,"port":0,"reason":"本版未接入本地代理服务"},'
      '"kernel":{"available":false,"installed":false,"path":"","searchPaths":["C:\\\\A","C:\\\\B","C:\\\\C"],'
      '"reason":"未找到 mihomo 内核"}}',
    );
    expect(status.error, isNull);
    expect(status.supported, isTrue);
    expect(status.system.enabled, isTrue);
    expect(status.system.server, 'http://127.0.0.1:8080');
    expect(status.system.bypass, contains('<local>'));
    expect(status.backup, isTrue);
    expect(status.local.supported, isFalse);
    expect(status.local.reason, contains('未接入'));
    expect(status.kernel.available, isFalse);
    expect(status.kernel.installed, isFalse);
    expect(status.kernel.searchPaths, hasLength(3));
    expect(status.kernel.reason, contains('mihomo'));
  });

  test('status parsing tolerates broken json and missing fields', () {
    expect(parseProxyStatus('{not json').error, '代理状态不是合法 JSON');
    expect(parseProxyStatus('[]').error, '代理状态格式不对');
    expect(parseProxyStatus('{}').error, '代理状态缺少 system 字段');

    // 只有 system：local / kernel 用默认值，不报错。
    final minimal = parseProxyStatus('{"system":{"enabled":false}}');
    expect(minimal.error, isNull);
    expect(minimal.system.enabled, isFalse);
    expect(minimal.local.supported, isFalse);
    expect(minimal.kernel.searchPaths, isEmpty);
  });

  test('engine status parsing maps nodes, active index and skipped reasons', () {
    final status = LocalProxyStatus.parse(
      '{"supported":true,"running":true,"port":7890,"active":1,'
      '"nodes":['
      '{"index":0,"name":"香港 01","kind":"ss","server":"a.com","port":8388,"implemented":true},'
      '{"index":1,"name":"日本 02","kind":"hysteria2","server":"b.com","port":443,"implemented":false}],'
      '"skipped":["tuic://x@c.com:443：不支持的协议"],"error":""}',
    );
    expect(status.error, isEmpty);
    expect(status.supported, isTrue);
    expect(status.running, isTrue);
    expect(status.port, 7890);
    expect(status.active, 1);
    expect(status.nodes, hasLength(2));
    expect(status.nodes.first.name, '香港 01');
    expect(status.nodes.first.endpoint, 'a.com:8388');
    expect(status.nodes.first.implemented, isTrue);
    expect(status.nodes.last.implemented, isFalse);
    expect(status.skipped.single, contains('不支持的协议'));
    // 抽屉要显示的三列：传输 / 来源（老应答缺字段时给空串，不崩）。
    expect(status.nodes.first.transport, isEmpty);
    expect(status.nodes.first.source, isEmpty);
    expect(status.mode, 'rule');

    expect(LocalProxyStatus.parse('{not json').error, '引擎状态不是合法 JSON');
    expect(LocalProxyStatus.parse('[]').error, '引擎状态格式不对');
    // 缺字段：保持默认值，不报错。
    final empty = LocalProxyStatus.parse('{}');
    expect(empty.running, isFalse);
    expect(empty.nodes, isEmpty);
  });

  test('engine status carries mode, traffic and the drawer columns', () {
    final status = LocalProxyStatus.parse(
      '{"supported":true,"running":true,"port":7890,"active":0,"mode":"global",'
      '"traffic":{"up":1024,"down":2048},'
      '"nodes":[{"index":0,"name":"n","kind":"vless","server":"a.com","port":443,'
      '"transport":"ws","source":"订阅","implemented":true}],'
      '"skipped":[],"error":""}',
    );
    expect(status.mode, 'global');
    expect(status.trafficUp, 1024);
    expect(status.trafficDown, 2048);
    expect(status.nodes.single.transport, 'ws');
    expect(status.nodes.single.source, '订阅');
    // 坏 traffic / 缺 mode ⇒ 回落默认，不崩。
    final minimal = LocalProxyStatus.parse('{"running":true,"traffic":"x"}');
    expect(minimal.mode, 'rule');
    expect(minimal.trafficUp, 0);
    expect(minimal.trafficDown, 0);
  });

  test('request log parsing keeps the panel line format', () {
    final entries = parseProxyRequests(
      '[{"host":"example.com","port":443,"route":"node","detail":"订阅 · 香港 01"},'
      '{"host":"lan.test","port":80,"route":"direct","detail":"没有规则命中"},'
      '{"host":"bad.test","port":0,"route":"rejected","detail":"规则要求走节点，但引擎里没有节点"}]',
    );
    expect(entries, hasLength(3));
    expect(entries.first.line, 'example.com:443 → 订阅 · 香港 01');
    expect(entries[1].route, 'direct');
    expect(entries.last.route, 'rejected');
    // 坏 JSON / 不是数组 ⇒ 空表（面板显示「暂无请求」）。
    expect(parseProxyRequests('{not json'), isEmpty);
    expect(parseProxyRequests('{"a":1}'), isEmpty);
    // 混进非对象项时只丢那一条。
    expect(parseProxyRequests('[1,{"host":"a","port":1,"route":"direct"}]'), hasLength(1));
  });

  test('panel config round-trips and normalises bad values', () {
    final config = parseProxyPanelConfig({
      'linksText': 'ss://a@b.com:1#A',
      'port': 1080,
      'mode': 'global',
      'ruleMode': 'whitelist',
      'ruleDomains': ['baidu.com', '  # 注释', 'qq.com'],
      'extraRules': ['DOMAIN-SUFFIX,example.com,DIRECT', 'MATCH,PROXY'],
    });
    expect(config.linksText, 'ss://a@b.com:1#A');
    expect(config.port, 1080);
    expect(config.mode, 'global');
    expect(config.ruleMode, 'whitelist');
    expect(config.ruleDomains, ['baidu.com', '# 注释', 'qq.com']);
    expect(config.extraRules, [
      'DOMAIN-SUFFIX,example.com,DIRECT',
      'MATCH,PROXY',
    ]);
    // 缺字段 ⇒ 空数组（不是默认规则），保证老配置不会被塞进规则。
    expect(parseProxyPanelConfig({'port': 1080}).extraRules, isEmpty);
    expect(config.toJson()['extraRules'], config.extraRules);

    // 脏数据：端口越界 / 模式不认识 / 缺规则清单 ⇒ 回默认。
    final bad = parseProxyPanelConfig({
      'port': 70000,
      'mode': 'direct',
      'ruleMode': 'x',
    });
    expect(bad.port, 7890);
    expect(bad.mode, 'rule');
    expect(bad.ruleMode, 'blacklist');
    expect(bad.ruleDomains, kDefaultRuleDomains);
    expect(parseProxyPanelConfig(null).mode, 'rule');

    // 清单文本 ↔ 列表：空行 / 注释行忽略；显式空清单保持为空。
    final edited = ProxyPanelConfig()
      ..setDomainsFromText('a.com\n\n# 注释\n// 注释\nb.com');
    expect(edited.ruleDomains, ['a.com', 'b.com']);
    expect(parseProxyPanelConfig({'ruleDomains': <String>[]}).ruleDomains, isEmpty);
    expect(edited.domainsText, 'a.com\nb.com');

    // 自定义规则：同一套忽略规则；空文本 ⇒ 空规则表。
    final withRules = ProxyPanelConfig()
      ..setExtraRulesFromText('DOMAIN,a.com,DIRECT\n\n# 注释\nIP-CIDR,10.0.0.0/8,DIRECT');
    expect(withRules.extraRules, [
      'DOMAIN,a.com,DIRECT',
      'IP-CIDR,10.0.0.0/8,DIRECT',
    ]);
    expect(withRules.rulesText, 'DOMAIN,a.com,DIRECT\nIP-CIDR,10.0.0.0/8,DIRECT');
    withRules.setExtraRulesFromText('');
    expect(withRules.extraRules, isEmpty);
  });

  test('custom server reply maps the generated link and reasons', () {
    final ok = ProxyCustomLinkReply.parse(
      '{"ok":true,"link":"ss://YWVzLTI1Ni1nY206cHc@a.com:8388#n","name":"n",'
      '"kind":"ss","error":""}',
    );
    expect(ok.ok, isTrue);
    expect(ok.link, startsWith('ss://'));
    expect(ok.name, 'n');
    expect(ok.kind, 'ss');

    final bad = ProxyCustomLinkReply.parse(
      '{"ok":false,"link":"","error":"端口要在 1–65535 之间"}',
    );
    expect(bad.ok, isFalse);
    expect(bad.error, contains('端口'));

    expect(
      ProxyCustomLinkReply.parse('{not json').error,
      '自定义服务器应答不是合法 JSON',
    );
    expect(ProxyCustomLinkReply.parse('[]').error, '自定义服务器应答格式不对');
  });

  test('engine reply carries ok, error and the nested status', () {
    final started = LocalProxyReply.parse(
      '{"ok":true,"error":"","status":{"running":true,"port":7891,"active":0,'
      '"nodes":[{"index":0,"name":"n","kind":"trojan","server":"t.com","port":443,"implemented":true}],'
      '"skipped":[]}}',
    );
    expect(started.ok, isTrue);
    expect(started.status!.running, isTrue);
    expect(started.status!.port, 7891);
    expect(started.status!.nodes.single.kind, 'trojan');

    final failed = LocalProxyReply.parse('{"ok":false,"error":"端口 7890 被占用"}');
    expect(failed.ok, isFalse);
    expect(failed.error, contains('占用'));
    expect(failed.status, isNull);

    expect(LocalProxyReply.parse('{not json').error, '引擎应答不是合法 JSON');
    expect(LocalProxyReply.parse('[]').error, '引擎应答格式不对');
  });

  test('delay reply keeps the probe scope honest', () {
    final ok = ProxyDelayReply.parse('{"ok":true,"error":"","ms":42,"scope":"http"}');
    expect(ok.ok, isTrue);
    expect(ok.ms, 42);
    expect(ok.scope, 'http');

    final failed = ProxyDelayReply.parse(
      '{"ok":false,"error":"经节点取 http://www.baidu.com/ 失败：timeout","ms":12000,"scope":"http"}',
    );
    expect(failed.ok, isFalse);
    expect(failed.error, contains('经节点'));
    // 老应答的 tcp 口径仍要能读（不把缺字段当成 http）。
    expect(
      ProxyDelayReply.parse('{"ok":true,"ms":7,"scope":"tcp"}').scope,
      'tcp',
    );

    expect(ProxyDelayReply.parse('{not json').error, '时延应答不是合法 JSON');
  });

  test('proxy status exposes engine nodes through the local field', () {
    final status = parseProxyStatus(
      '{"system":{"enabled":false},'
      '"local":{"supported":true,"running":true,"port":7890,"active":0,'
      '"nodes":[{"index":0,"name":"n","kind":"ss","server":"a.com","port":8388,"implemented":true}],'
      '"skipped":[],"reason":""}}',
    );
    expect(status.error, isNull);
    expect(status.local.supported, isTrue);
    expect(status.local.running, isTrue);
    expect(status.local.port, 7890);
    expect(status.local.nodes.single.name, 'n');
  });

  test('set reply maps ok, error and the actual system state', () {
    final ok = decodeProxySetReply(
      '{"ok":true,"error":"","system":{"enabled":true,"server":"http://127.0.0.1:8080","bypass":"<local>"}}',
    );
    expect(ok.ok, isTrue);
    expect(ok.error, isEmpty);
    expect(ok.system!.enabled, isTrue);

    final failed = decodeProxySetReply(
      '{"ok":false,"error":"系统代理不是由 Lunac 开启的（没有备份），未改动系统设置","system":{"enabled":true,"server":"http://old:8080","bypass":""}}',
    );
    expect(failed.ok, isFalse);
    expect(failed.error, contains('没有备份'));
    expect(failed.system!.server, 'http://old:8080');

    expect(decodeProxySetReply('{not json').error, '设置结果不是合法 JSON');
    expect(decodeProxySetReply('[]').error, '设置结果格式不对');
    expect(decodeProxySetReply('{"ok":true}').system, isNull);
  });

  testWidgets('proxy entry opens the 640 panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend(const [
      SearchResult(
        id: 'proxy',
        title: '代理',
        subtitle: 'Windows 系统代理开关（本地服务 / 内核未接入）',
        kind: 'command',
      ),
    ]);
    final heights = <double>[];
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          onResize: (height) async => heights.add(height),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(_row('代理'));
    await tester.pumpAndSettle();

    // 面板：640 固定档；单测没有 lunac_core.dll，如实显示「不可用」。
    expect(backend.actions, contains('proxy'));
    expect(heights.last, 640);
    expect(find.text('代理'), findsOneWidget);
    expect(find.text('系统代理：关'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    expect(find.text('不可用（rust 桥接未加载）'), findsNothing);
    expect(heights.last, 126);
  });
}
