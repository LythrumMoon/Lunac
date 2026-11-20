// 插件市场的 JSON 解码契约（FFI 返回值 → 界面可用的形状）。
//
// 这里只覆盖 Dart 侧的解码；下载 / 解压 / sha256 由 Rust 单测覆盖
// （`app-flutter/rust/src/plugin_market.rs`）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/plugin_market.dart';

void main() {
  test('index decodes entries and keeps the index error', () {
    final index = MarketIndex.fromRaw(
      '{"entries":[{"id":"music","name":"音乐","description":"歌词",'
      '"version":"0.9.6","url":"https://example.com/music.zip",'
      '"sha256":"","icon":"🎵","keywords":["音乐"]}],"error":""}',
    );
    expect(index.error, isEmpty);
    expect(index.entries, hasLength(1));
    final entry = index.entries.single;
    expect(entry.id, 'music');
    expect(entry.url, 'https://example.com/music.zip');
    expect(entry.keywords, ['音乐']);
  });

  test('index without a bridge or with a broken body reports the reason', () {
    expect(MarketIndex.fromRaw(null).entries, isEmpty);
    expect(MarketIndex.fromRaw(null).error, contains('rust 桥接未加载'));
    final broken = MarketIndex.fromRaw('[]');
    expect(broken.entries, isEmpty);
    expect(broken.error, isNotEmpty);
    final indexError = MarketIndex.fromRaw('{"entries":[],"error":"HTTP 404"}');
    expect(indexError.error, 'HTTP 404');
  });

  test('install / uninstall results decode ok and errors', () {
    final ok = MarketActionResult.fromRaw(
      '{"ok":true,"id":"music","error":""}',
      'fallback',
    );
    expect(ok.ok, isTrue);
    expect(ok.id, 'music');
    final failed = MarketActionResult.fromRaw(
      '{"ok":false,"id":"","error":"sha256 不匹配"}',
      'fallback',
    );
    expect(failed.ok, isFalse);
    expect(failed.error, contains('sha256'));
    expect(MarketActionResult.fromRaw(null, 'fallback').ok, isFalse);
  });

  test('install state decodes progress, pause and the dependency index', () {
    final state = MarketInstallState.fromRaw(
      '{"busy":true,"state":{"active":true,"kind":"deps","id":"ocr",'
      '"url":"https://example.com/model.bin","phase":"download",'
      '"downloaded":768,"total":1024,"paused":true,"resumable":true,'
      '"index":2,"count":3}}',
    );
    expect(state.busy, isTrue);
    expect(state.active, isTrue);
    expect(state.kind, 'deps');
    expect(state.id, 'ocr');
    expect(state.index, 2);
    expect(state.count, 3);
    expect(state.paused, isTrue);
    expect(state.fraction, closeTo(0.75, 1e-9));
    // 总长未知（服务端没给 Content-Length）时不编造百分比。
    const unknown = MarketInstallState(downloaded: 100);
    expect(unknown.fraction, isNull);
    // 桥接缺失 / 坏 JSON ⇒ 全部默认值（界面显示「没有在装」，不假装在跑）。
    expect(MarketInstallState.fromRaw(null).active, isFalse);
    expect(MarketInstallState.fromRaw('[]').active, isFalse);
  });

  test('pending downloads decode and skip entries without a url', () {
    final list = MarketPendingDownload.fromJson({
      'url': 'https://example.com/local-model.zip',
      'id': 'local-model',
      'downloaded': 37,
      'total': 100,
    });
    expect(list, isNotNull);
    expect(list!.fraction, closeTo(0.37, 1e-9));
    expect(MarketPendingDownload.fromJson({'id': 'x'}), isNull);
  });

  test('progress formatting never invents numbers', () {
    expect(percentText(null), isEmpty);
    expect(percentText(0), '0%');
    expect(percentText(0.3712), '37%');
    // 99.9% 不四舍五入成 100%（那会让人以为下完了）。
    expect(percentText(0.999), '99%');
    expect(percentText(1), '100%');

    expect(formatByteSize(0), '0 B');
    expect(formatByteSize(1023), '1023 B');
    expect(formatByteSize(1024), '1.0 KB');
    expect(formatByteSize(1536), '1.5 KB');
    expect(formatByteSize(1024 * 1024), '1.0 MB');
    expect(formatByteSize(1258291), '1.2 MB');
    // 三位数以上不带小数（进度条上位置有限）。
    expect(formatByteSize(200 * 1024 * 1024), '200 MB');

    expect(urlTail('https://example.com/a/b/local-model.zip'), 'local-model.zip');
    expect(urlTail('https://example.com/x.zip?token=1#frag'), 'x.zip');
    expect(urlTail('https://example.com'), 'example.com');
  });
}
