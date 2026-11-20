// 音乐平台薄层与面板的单测（不加载 DLL、不建音频设备）。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/music.dart';
import 'package:lunac/search/music_panel.dart';
import 'package:lunac/search/search_bridge.dart';
import 'package:lunac/search/search_page.dart';
import 'package:lunac/search/search_result.dart';
import 'package:lunac/theme.dart';

/// 播放引擎替身：只接流、不建平台通道（单测进程里没有音频设备）。
class _FakeMusicEngine implements MusicAudioEngine {
  final StreamController<Duration> _position =
      StreamController<Duration>.broadcast();
  final StreamController<Duration?> _duration =
      StreamController<Duration?>.broadcast();
  final StreamController<bool> _playing = StreamController<bool>.broadcast();
  final StreamController<void> _completed = StreamController<void>.broadcast();

  @override
  Stream<Duration> get positionStream => _position.stream;

  @override
  Stream<Duration?> get durationStream => _duration.stream;

  @override
  Stream<bool> get playingStream => _playing.stream;

  @override
  Stream<void> get completedStream => _completed.stream;

  @override
  Future<Duration?> open(String path) async => null;

  @override
  Future<void> play() async {}

  @override
  Future<void> pause() async {}

  @override
  Future<void> stop() async {}

  @override
  Future<void> seek(Duration position) async {}

  @override
  Future<void> setVolume(double volume) async {}

  @override
  Future<void> dispose() async {
    await _position.close();
    await _duration.close();
    await _playing.close();
    await _completed.close();
  }
}

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
  test('music root reply maps ok and readable failures', () {
    final ok = MusicRootResult.fromJson(
      r'{"ok":true,"path":"D:\\Music","error":""}',
    );
    expect(ok.ok, isTrue);
    expect(ok.path, r'D:\Music');

    final failed = MusicRootResult.fromJson(
      r'{"ok":false,"path":"","error":"目录不存在：D:\\nope"}',
    );
    expect(failed.ok, isFalse);
    expect(failed.error, contains('目录不存在'));

    expect(MusicRootResult.fromJson('{oops').ok, isFalse);
    expect(MusicRootResult.fromJson('[]').error, '曲库结果格式不对');
  });

  test('library parsing keeps tracks, roots and readable errors', () {
    final library = parseMusicLibrary(
      '{"tracks":['
      '{"path":"C:\\\\Music\\\\a.mp3","title":"晴天","artist":"周杰伦",'
      '"album":"叶惠美","duration_ms":269000,"cover":"C:\\\\covers\\\\a.jpg"},'
      '{"path":"C:\\\\Music\\\\b.flac","title":"夜曲","artist":"","album":"",'
      '"durationMs":226000,"cover":""},'
      '{"title":"缺路径"}'
      '],"roots":["C:\\\\Music"],"error":null}',
    );
    expect(library.tracks, hasLength(2));
    expect(library.tracks.first.title, '晴天');
    expect(library.tracks.first.durationMs, 269000);
    expect(library.tracks[1].durationMs, 226000);
    expect(library.tracks[1].subtitle, isEmpty);
    expect(library.roots, ['C:\\Music']);
    expect(library.error, isNull);

    final broken = parseMusicLibrary('{not json');
    expect(broken.tracks, isEmpty);
    expect(broken.error, '曲库结果不是合法 JSON');

    final wrongShape = parseMusicLibrary('[]');
    expect(wrongShape.error, '曲库结果格式不对');

    final failed = parseMusicLibrary(
      '{"tracks":[],"roots":[],"error":"读取目录失败：D:\\\\Music（拒绝访问）"}',
    );
    expect(failed.error, contains('读取目录失败'));
  });

  test('lyrics parsing sorts by time and tolerates broken json', () {
    final lyrics = parseMusicLyrics(
      '{"lines":[{"timeMs":3000,"text":"第二句"},'
      '{"time_ms":1200,"text":"第一句"},{"text":"缺时间"}],"error":null}',
    );
    expect(lyrics.lines, hasLength(2));
    expect(lyrics.lines.first.timeMs, 1200);
    expect(lyrics.lines.last.text, '第二句');
    expect(lyrics.error, isNull);

    expect(parseMusicLyrics('{not json').error, '歌词结果不是合法 JSON');
    expect(parseMusicLyrics('[]').error, '歌词结果格式不对');

    final missing = parseMusicLyrics('{"lines":[],"error":null}');
    expect(missing.lines, isEmpty);
    expect(missing.isEmpty, isTrue);
  });

  test('lyrics error payload wins over an empty line list', () {
    final lyrics = parseMusicLyrics(
      '{"lines":[],"error":"读取歌词文件失败：C:\\\\a.lrc（拒绝访问）"}',
    );
    expect(lyrics.lines, isEmpty);
    expect(lyrics.error, contains('读取歌词文件失败'));
  });

  test('online lyrics carry source, found and instrumental flags', () {
    final online = parseMusicLyrics(
      '{"found":true,"instrumental":false,"source":"netease",'
      '"track":"Creep","artist":"Radiohead","album":"Pablo Honey","duration":238.0,'
      '"lines":[{"timeMs":12000,"text":"When you were here before"}],'
      '"plain":"When you were here before","error":null}',
    );
    expect(online.found, isTrue);
    expect(online.instrumental, isFalse);
    expect(online.source, 'netease');
    expect(online.sourceLabel, '网易云');
    expect(online.lines.single.timeMs, 12000);
    expect(online.plain, 'When you were here before');
    expect(online.error, isNull);

    // 纯音乐：命中但没有任何词行 —— 不是失败。
    final instrumental = parseMusicLyrics(
      '{"found":true,"instrumental":true,"source":"lrclib","lines":[],"error":null}',
    );
    expect(instrumental.found, isTrue);
    expect(instrumental.instrumental, isTrue);
    expect(instrumental.isEmpty, isTrue);
    expect(instrumental.sourceLabel, 'LRCLIB');

    // 三级全落空：found=false 且 error 为 null（面板显示「没找到歌词」）。
    final none = parseMusicLyrics('{"found":false,"lines":[],"error":null}');
    expect(none.found, isFalse);
    expect(none.error, isNull);
  });

  test('rate limited lyrics turn into a readable sentence', () {
    final limited = parseMusicLyrics(
      '{"lines":[],"found":false,"error":"ERR_RATE_LIMITED:7"}',
    );
    expect(limited.error, '歌词服务限流了，等 7 秒再试');

    final unknown = parseMusicLyrics(
      '{"lines":[],"found":false,"error":"ERR_RATE_LIMITED:?"}',
    );
    expect(unknown.error, '歌词服务限流了，等几秒再试');

    // 其它错误原样保留（不吞真正的失败原因）。
    final broken = parseMusicLyrics(
      '{"lines":[],"found":false,"error":"请求 LRCLIB 失败：dns error"}',
    );
    expect(broken.error, contains('dns error'));
  });

  test('music queue json maps id, name and paths', () {
    final queue = MusicQueue.fromJson(<String, dynamic>{
      'id': 'queue-1',
      'name': '通勤',
      'paths': [r'D:\a.mp3', '', r'D:\b.flac'],
    });
    expect(queue, isNotNull);
    expect(queue!.id, 'queue-1');
    expect(queue.name, '通勤');
    expect(queue.paths, [r'D:\a.mp3', r'D:\b.flac']);
    expect(queue.copyWith(name: '新名字').name, '新名字');
    expect(queue.toJson()['paths'], hasLength(2));

    expect(MusicQueue.fromJson(<String, dynamic>{'name': '缺 id'}), isNull);
    expect(MusicQueue.fromJson('nope'), isNull);
  });

  test('Spotify state marks the built-in client id and defaults to it', () {
    final custom = SpotifyState.fromJson(<String, dynamic>{
      'configured': true,
      'connected': false,
      'builtin': false,
      'clientId': 'my-own-client',
      'port': 8899,
      'redirectUri': 'http://127.0.0.1:8899/callback',
    });
    expect(custom, isNotNull);
    expect(custom!.builtin, isFalse);
    expect(custom.clientId, 'my-own-client');
    expect(custom.port, 8899);

    final missing = SpotifyState.fromJson(<String, dynamic>{'connected': true});
    expect(missing, isNotNull);
    expect(missing!.builtin, isTrue, reason: '缺字段按内置值处理（面板收起凭据字段）');
  });

  testWidgets('queue section lists queues and filters the track list', (
    tester,
  ) async {
    _useTallSurface(tester);
    const library = MusicLibrary(
      tracks: [
        MusicTrack(
          path: r'C:\MusicA\a.mp3',
          title: '晴天',
          artist: '周杰伦',
          album: '叶惠美',
          durationMs: 269000,
          cover: '',
        ),
        MusicTrack(
          path: r'C:\MusicA\b.flac',
          title: '夜曲',
          artist: '周杰伦',
          album: '',
          durationMs: 226000,
          cover: '',
        ),
      ],
      roots: [r'C:\MusicA'],
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 1000,
              height: 640,
              child: MusicPanel(
                onClose: () {},
                loadLibrary: () async => library,
                loadQueues: () => const [
                  MusicQueue(
                    id: 'q1',
                    name: '通勤歌单',
                    paths: [r'C:\MusicA\b.flac'],
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('自定义队列'), findsOneWidget);
    expect(find.text('通勤歌单'), findsOneWidget);

    // 点队列 → 主区只留队列里的曲目，且标题栏显示队列名。
    await tester.tap(find.text('通勤歌单'));
    await tester.pumpAndSettle();
    expect(find.text('夜曲'), findsOneWidget);
    expect(find.text('晴天'), findsNothing);
    expect(find.text('通勤歌单'), findsNWidgets(2));

    // 回「全部曲目」→ 队列过滤解除。
    await tester.tap(find.text('全部曲目'));
    await tester.pumpAndSettle();
    expect(find.text('晴天'), findsOneWidget);
  });

  testWidgets('add-queue dialog mirrors v1 and guards an empty draft', (
    tester,
  ) async {
    _useTallSurface(tester);
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 1000,
              height: 640,
              child: MusicPanel(
                onClose: () {},
                loadLibrary: () async =>
                    const MusicLibrary(tracks: [], roots: []),
                loadQueues: () => const [],
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('添加队列（名字 + 选文件）'));
    await tester.pumpAndSettle();

    // v1 `#music-queue-dlg`：标题 / 队列名称 / 添加文件 / 空清单 / 取消 + 确定。
    expect(find.text('添加队列'), findsOneWidget);
    expect(find.text('队列名称'), findsOneWidget);
    expect(find.text('可跨不同文件夹多选，重复的自动去重'), findsOneWidget);
    expect(find.text('还没有选择文件'), findsOneWidget);

    // 空草稿点「确定」→ 小窗不关，并给出 v1 那句提示。
    await tester.tap(find.text('确定'));
    await tester.pumpAndSettle();
    expect(find.text('请先添加至少一个文件'), findsOneWidget);
    expect(find.text('队列名称'), findsOneWidget);

    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(find.text('队列名称'), findsNothing);
  });

  testWidgets('music entry opens the 640 panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend(const [
      SearchResult(
        id: 'music',
        title: '音乐',
        subtitle: '本地媒体库 · 歌词同步（在线搜索未接入）',
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

    await tester.tap(_row('音乐'));
    await tester.pumpAndSettle();
    // 曲库读取在 isolate 里：等它真的回落后再断言界面。
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pumpAndSettle();

    // 面板：640 固定档；单测没有 lunac_core.dll，如实显示「不可用」。
    expect(backend.actions, contains('music'));
    expect(heights.last, 640);
    expect(find.text('音乐'), findsOneWidget);
    expect(find.text('本地媒体库 · 在线歌词（LRCLIB / 网易云）'), findsOneWidget);
    // 歌词已接在线；Spotify 在线库的入口在标题栏（点开是指挥台弹窗）。
    expect(find.text('Spotify'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);
    // v1 本地页的左栏骨架：本地音乐 / 全部曲目（没有曲目时没有底部播放条）。
    expect(find.text('本地音乐'), findsOneWidget);
    expect(find.text('全部曲目'), findsOneWidget);
    expect(find.byTooltip('播放 / 暂停'), findsNothing);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    expect(find.text('不可用（rust 桥接未加载）'), findsNothing);
    expect(heights.last, 126);
  });

  testWidgets(
    'local shell follows v1: sidebar roots, filter, nowbar and player view',
    (tester) async {
      _useTallSurface(tester);
      final engine = _FakeMusicEngine();
      const library = MusicLibrary(
        tracks: [
          MusicTrack(
            path: r'C:\MusicA\a.mp3',
            title: '晴天',
            artist: '周杰伦',
            album: '叶惠美',
            durationMs: 269000,
            cover: '',
          ),
          MusicTrack(
            path: r'C:\MusicA\b.flac',
            title: '夜曲',
            artist: '周杰伦',
            album: '',
            durationMs: 226000,
            cover: '',
          ),
          MusicTrack(
            path: r'C:\MusicB\c.mp3',
            title: '后来',
            artist: '刘若英',
            album: '',
            durationMs: 300000,
            cover: '',
          ),
        ],
        roots: [r'C:\MusicA', r'C:\MusicB'],
      );
      await tester.pumpWidget(
        MaterialApp(
          theme: buildLunacTheme(),
          home: Scaffold(
            body: Center(
              child: SizedBox(
                width: 1000,
                height: 640,
                child: MusicPanel(
                  onClose: () {},
                  createEngine: () => engine,
                  loadLibrary: () async => library,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // 左栏：本地音乐 / 全部曲目 / 两个目录（各带曲目数）。
      expect(find.text('本地音乐'), findsOneWidget);
      expect(find.text('全部曲目'), findsOneWidget);
      expect(find.text('MusicA'), findsOneWidget);
      expect(find.text('MusicB'), findsOneWidget);
      expect(find.text('2 首'), findsOneWidget);
      expect(find.text('1 首'), findsOneWidget);
      expect(find.text('晴天'), findsOneWidget);
      expect(find.text('后来'), findsOneWidget);

      // 点左栏 MusicB → 主区只留它下面的曲目。
      await tester.tap(find.text('MusicB'));
      await tester.pumpAndSettle();
      expect(find.text('后来'), findsOneWidget);
      expect(find.text('晴天'), findsNothing);

      // 点一首 → 出现 v1 的底部播放条（时间 / 音量 / 五个控制键）。
      await tester.tap(find.text('后来'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.byTooltip('播放 / 暂停'), findsOneWidget);
      expect(find.byTooltip('停止播放'), findsOneWidget);
      expect(find.text('0:00 / 5:00'), findsOneWidget);

      // 点「正在播放」进入播放界面，再返回歌单。
      await tester.tap(find.byTooltip('打开播放界面'));
      await tester.pumpAndSettle();
      expect(find.text('正在播放'), findsOneWidget);
      await tester.tap(find.byTooltip('返回歌单（Esc）'));
      await tester.pumpAndSettle();
      expect(find.text('正在播放'), findsNothing);
      // 回到歌单：列表行 + 底部播放条的「正在播放」块各有一份。
      expect(find.text('后来'), findsNWidgets(2));
    },
  );

  testWidgets('playback queue dropdown lists the round of playing tracks', (
    tester,
  ) async {
    _useTallSurface(tester);
    final engine = _FakeMusicEngine();
    const library = MusicLibrary(
      tracks: [
        MusicTrack(
          path: r'C:\MusicA\a.mp3',
          title: '晴天',
          artist: '周杰伦',
          album: '',
          durationMs: 269000,
          cover: '',
        ),
        MusicTrack(
          path: r'C:\MusicA\b.flac',
          title: '夜曲',
          artist: '周杰伦',
          album: '',
          durationMs: 226000,
          cover: '',
        ),
      ],
      roots: [r'C:\MusicA'],
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 1000,
              height: 640,
              child: MusicPanel(
                onClose: () {},
                createEngine: () => engine,
                loadLibrary: () async => library,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // 点播一首 → `_queue` 是点播那一刻的列表快照（v1 localQueue 同口径）。
    await tester.tap(find.text('晴天'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.tap(find.byTooltip('播放队列'));
    await tester.pumpAndSettle();
    // 浮层里第二首也在（列表行 + 浮层行各一份）；正在播的那首多一份播放条标题。
    expect(find.text('夜曲'), findsNWidgets(2));
    expect(find.text('晴天'), findsNWidgets(3));

    // 再点一次收起（v1 `#music-lq-dd` 的开关语义）。
    await tester.tap(find.byTooltip('播放队列'));
    await tester.pumpAndSettle();
    expect(find.text('夜曲'), findsOneWidget);
    expect(find.text('晴天'), findsNWidgets(2));
  });

  testWidgets('queue pick mode seeds the add-queue dialog', (tester) async {
    _useTallSurface(tester);
    const library = MusicLibrary(
      tracks: [
        MusicTrack(
          path: r'C:\MusicA\a.mp3',
          title: '晴天',
          artist: '周杰伦',
          album: '',
          durationMs: 269000,
          cover: '',
        ),
        MusicTrack(
          path: r'C:\MusicA\b.flac',
          title: '夜曲',
          artist: '周杰伦',
          album: '',
          durationMs: 226000,
          cover: '',
        ),
      ],
      roots: [r'C:\MusicA'],
    );
    final saved = <List<MusicQueue>>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 1000,
              height: 640,
              child: MusicPanel(
                onClose: () {},
                loadLibrary: () async => library,
                loadQueues: () => const [],
                saveQueues: (queues) {
                  saved.add(queues);
                  return null;
                },
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // 进勾选态：点行 = 勾选（不是播放），工具栏出现「添加到队列 (n)」。
    await tester.tap(find.byTooltip('自定义队列'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('晴天'));
    await tester.pumpAndSettle();
    expect(find.text('添加到队列 (1)'), findsOneWidget);
    expect(find.byTooltip('播放 / 暂停'), findsNothing);

    // 勾中的歌作为小窗草稿带入（v1 `openQueueDialog(seedFiles)`）。
    await tester.tap(find.text('添加到队列 (1)'));
    await tester.pumpAndSettle();
    expect(find.text('已选 1 个文件'), findsOneWidget);
    expect(find.text('a.mp3'), findsOneWidget);

    await tester.tap(find.text('确定'));
    await tester.pumpAndSettle();
    expect(saved, hasLength(1));
    expect(saved.single.single.name, '未命名队列');
    expect(saved.single.single.paths, [r'C:\MusicA\a.mp3']);
    expect(find.text('已创建队列「未命名队列」'), findsOneWidget);
    expect(find.text('队列名称'), findsNothing);
  });

  testWidgets('queue detail removes a track in edit mode', (tester) async {
    _useTallSurface(tester);
    const library = MusicLibrary(
      tracks: [
        MusicTrack(
          path: r'C:\MusicA\a.mp3',
          title: '晴天',
          artist: '周杰伦',
          album: '',
          durationMs: 269000,
          cover: '',
        ),
        MusicTrack(
          path: r'C:\MusicA\b.flac',
          title: '夜曲',
          artist: '周杰伦',
          album: '',
          durationMs: 226000,
          cover: '',
        ),
      ],
      roots: [r'C:\MusicA'],
    );
    final saved = <List<MusicQueue>>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 1000,
              height: 640,
              child: MusicPanel(
                onClose: () {},
                loadLibrary: () async => library,
                loadQueues: () => const [
                  MusicQueue(
                    id: 'q1',
                    name: '通勤歌单',
                    paths: [r'C:\MusicA\a.mp3', r'C:\MusicA\b.flac'],
                  ),
                ],
                saveQueues: (queues) {
                  saved.add(queues);
                  return null;
                },
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('通勤歌单'));
    await tester.pumpAndSettle();
    // 「修改列表」编辑态才出现逐首移除（v1 `mediaEditMode` 同口径）。
    expect(find.byTooltip('从队列移除'), findsNothing);
    await tester.tap(find.byTooltip('修改列表'));
    await tester.pumpAndSettle();
    expect(find.byTooltip('从队列移除'), findsNWidgets(2));

    await tester.tap(find.byTooltip('从队列移除').first);
    await tester.pumpAndSettle();
    expect(saved, hasLength(1));
    expect(saved.single.single.paths, [r'C:\MusicA\b.flac']);
  });
}
