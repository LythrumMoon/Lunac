// 沙箱盘：JSON 解码契约 + 面板空态（无桥接时如实显示不可用）。
//
// 统计 / 预览 / 清空的真实行为由 Rust 单测覆盖
// （`app-flutter/rust/src/sandbox.rs`）。
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/change_review.dart';
import 'package:lunac/platform/sandbox.dart';
import 'package:lunac/search/sandbox_panel.dart';
import 'package:lunac/theme.dart';

void main() {
  test('status decodes usage, quota and the over-quota flag', () {
    final status = SandboxStatus.fromJson(const {
      'path': r'D:\Lunac\temp\transStorage',
      'exists': true,
      'usedBytes': 256,
      'quotaBytes': 512,
      'fileCount': 3,
      'dirCount': 1,
      'truncated': false,
      'error': '',
    });
    expect(status.path, endsWith('transStorage'));
    expect(status.exists, isTrue);
    expect(status.usageRatio, 0.5);
    expect(status.overQuota, isFalse);
    final over = SandboxStatus.fromJson(const {
      'usedBytes': 600,
      'quotaBytes': 512,
    });
    expect(over.usageRatio, 1.0);
    expect(over.overQuota, isTrue);
  });

  test('listing decodes nested entries and depth', () {
    final listing = SandboxListing.fromJson(const {
      'path': r'D:\Lunac\temp\transStorage',
      'entries': [
        {'path': 'demo', 'name': 'demo', 'isDir': true, 'size': 0},
        {
          'path': 'demo/main.py',
          'name': 'main.py',
          'isDir': false,
          'size': 42,
          'modifiedMs': 1700000000000,
        },
      ],
      'usedBytes': 42,
      'quotaBytes': 1024,
    });
    expect(listing.entries, hasLength(2));
    expect(listing.entries[0].isDir, isTrue);
    expect(listing.entries[1].depth, 1);
    expect(listing.entries[1].size, 42);
  });

  test('file preview decodes text / image / errors', () {
    final text = SandboxFile.fromJson(const {
      'ok': true,
      'kind': 'text',
      'name': 'a.txt',
      'size': 2,
      'text': 'hi',
    });
    expect(text.ok, isTrue);
    expect(text.kind, 'text');
    expect(text.text, 'hi');
    final image = SandboxFile.fromJson(const {
      'ok': true,
      'kind': 'image',
      'path': r'D:\Lunac\temp\transStorage\a.png',
    });
    expect(image.kind, 'image');
    final denied = SandboxFile.fromJson(const {
      'ok': false,
      'kind': 'binary',
      'error': '路径越出沙箱盘，已拒绝',
    });
    expect(denied.ok, isFalse);
    expect(denied.error, contains('越出'));
  });

  test('status decodes recent files and tolerates junk entries', () {
    final status = SandboxStatus.fromJson(const {
      'exists': true,
      'recent': [
        {
          'path': 'demo/main.py',
          'name': 'main.py',
          'size': 42,
          'modifiedMs': 1700000000000,
        },
        {'name': 'no-path.txt'},
        'junk',
      ],
    });
    expect(status.recent.length, 1, reason: '没有路径的条目要丢掉');
    expect(status.recent.first.path, 'demo/main.py');
    expect(status.recent.first.size, 42);
    expect(status.recent.first.modifiedMs, 1700000000000);
    // 缺 recent 字段时是空表，不抛。
    expect(SandboxStatus.fromJson(const {'exists': true}).recent, isEmpty);
  });

  test('workspace fingerprints diff into created and updated files', () {
    const before = {
      r'a.txt': '3:100',
      r'sub\keep.md': '1:100',
      r'gone.txt': '9:100',
    };
    const after = {
      r'a.txt': '7:200', // 改过（大小 / 时间变化）
      r'sub\keep.md': '1:100', // 没动
      r'new.txt': '5:300', // 新增
    };
    expect(
      diffWorkspaceFingerprints(before, after),
      [r'a.txt', r'new.txt'],
      reason: '删除不算产出；排序稳定',
    );
    expect(diffWorkspaceFingerprints(after, after), isEmpty);
  });

  testWidgets('panel lists recent files and previews one on tap', (
    tester,
  ) async {
    const listing = SandboxListing(
      path: r'D:\Lunac\temp\transStorage',
      entries: [
        SandboxEntry(
          path: 'demo/main.py',
          name: 'main.py',
          isDir: false,
          size: 42,
          modifiedMs: 1700000000000,
        ),
      ],
      usedBytes: 42,
      quotaBytes: 1024,
    );
    final opened = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: SandboxPanel(
            loadStatus: () => SandboxStatus.fromJson(const {
              'path': r'D:\Lunac\temp\transStorage',
              'exists': true,
              'usedBytes': 42,
              'quotaBytes': 1024,
              'fileCount': 1,
              'recent': [
                {
                  'path': 'demo/main.py',
                  'name': 'main.py',
                  'size': 42,
                  'modifiedMs': 1700000000000,
                },
              ],
            }),
            loadListing: () => listing,
            loadFile: (path) {
              opened.add(path);
              return const SandboxFile(
                ok: true,
                kind: 'text',
                name: 'main.py',
                size: 42,
                text: 'print(1)',
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();

    // 工作区标签里不再列「最近改动」（用户 2026-11-14 要求挪到对话面板），
    // 文件在左侧资源管理器里。
    expect(find.text('工作区'), findsNWidgets(2));
    await tester.tap(find.text('main.py').first);
    await tester.pump();

    expect(opened, ['demo/main.py']);
    expect(find.text('print(1)'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('panel shows the honest empty state without a bridge', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: const Scaffold(body: SandboxPanel()),
      ),
    );
    await tester.pump();
    expect(find.text('沙箱盘'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsWidgets);
    // 定时器（2 秒轮询）要随面板销毁一起取消，否则测试会报 pending timer。
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('preview tabs from the agent land beside the workspace tree', (
    tester,
  ) async {
    var delivered = false;
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: SandboxPanel(
            loadStatus: () => SandboxStatus.fromJson(const {
              'path': r'D:\Lunac\temp\transStorage',
              'exists': true,
              'usedBytes': 0,
              'quotaBytes': 1024,
            }),
            loadListing: () => const SandboxListing(
              path: r'D:\Lunac\temp\transStorage',
              entries: [],
              usedBytes: 0,
              quotaBytes: 1024,
            ),
            // 只交一次，模拟 Rust 队列的 take 语义。
            pullTabs: () {
              if (delivered) {
                return const [];
              }
              delivered = true;
              return const [
                PreviewTabRequest(
                  // 模型只给应用名时，面板要补成「预览: …」。
                  title: '记事本',
                  kind: 'app',
                  target: 'notepad.exe',
                ),
              ];
            },
          ),
        ),
      ),
    );
    await tester.pump();

    // 左侧资源管理器 + 右侧常驻「工作区」标签。
    expect(find.text('工作区'), findsNWidgets(2));

    await tester.pump(const Duration(seconds: 1));
    expect(find.text('预览: 记事本'), findsWidgets);
    expect(find.text('打开应用'), findsOneWidget);
    expect(find.text('接管'), findsOneWidget);
    // 抓帧 / 停靠都拿不到时如实说明，不假装渲染成功。
    expect(find.textContaining('缩略图是只读画面'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('workspace tab can be closed and comes back when no tab remains', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: SandboxPanel(
            loadStatus: () => SandboxStatus.fromJson(const {
              'path': r'D:\Lunac\temp\transStorage',
              'exists': true,
              'usedBytes': 0,
              'quotaBytes': 1024,
            }),
            loadListing: () => const SandboxListing(
              path: r'D:\Lunac\temp\transStorage',
              entries: [],
              usedBytes: 0,
              quotaBytes: 1024,
            ),
            pullTabs: () => const [],
          ),
        ),
      ),
    );
    await tester.pump();

    // 左侧资源管理器标题 + 标签条里的「工作区」。
    expect(find.text('工作区'), findsNWidgets(2));
    await tester.tap(find.byIcon(Icons.close).first);
    await tester.pump();

    // 用户 2026-11-14：标签可以关到空，但空的时候自动把「工作区」放回来。
    expect(find.text('工作区'), findsNWidgets(2));

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('change tab shows red deletions, green additions and apply/undo', (
    tester,
  ) async {
    var delivered = false;
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: SandboxPanel(
            loadStatus: () => SandboxStatus.fromJson(const {
              'path': r'D:\Lunac\temp\transStorage',
              'exists': true,
              'usedBytes': 0,
              'quotaBytes': 1024,
            }),
            loadListing: () => const SandboxListing(
              path: r'D:\Lunac\temp\transStorage',
              entries: [],
              usedBytes: 0,
              quotaBytes: 1024,
            ),
            pullTabs: () {
              if (delivered) {
                return const [];
              }
              delivered = true;
              return const [
                PreviewTabRequest(
                  title: 'a.txt',
                  kind: 'change',
                  target: r'D:\work\a.txt',
                ),
              ];
            },
            loadChangeDiff: (path) => const ChangeDiff(
              ok: true,
              existed: true,
              added: 1,
              removed: 1,
              lines: [
                ChangeDiffLine(kind: 'same', text: 'one'),
                ChangeDiffLine(kind: 'del', text: 'two'),
                ChangeDiffLine(kind: 'add', text: 'TWO'),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();

    expect(find.text('变更已完成 · 是否应用？'), findsOneWidget);
    expect(find.text('全部应用'), findsOneWidget);
    expect(find.text('撤销'), findsOneWidget);
    expect(find.text('−two'), findsOneWidget);
    expect(find.text('+TWO'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('tab bar shows a scrollbar and follows the mouse wheel', (
    tester,
  ) async {
    var delivered = false;
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLunacTheme(),
        home: Scaffold(
          body: SandboxPanel(
            loadStatus: () => SandboxStatus.fromJson(const {
              'path': r'D:\Lunac\temp\transStorage',
              'exists': true,
              'usedBytes': 0,
              'quotaBytes': 1024,
            }),
            loadListing: () => const SandboxListing(
              path: r'D:\Lunac\temp\transStorage',
              entries: [],
              usedBytes: 0,
              quotaBytes: 1024,
            ),
            // 一次塞满，触发标签条横向溢出。
            pullTabs: () {
              if (delivered) {
                return const [];
              }
              delivered = true;
              return [
                for (var i = 0; i < 20; i++)
                  PreviewTabRequest(
                    title: '网页$i',
                    kind: 'url',
                    target: 'https://example.com/$i',
                  ),
              ];
            },
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pump(const Duration(milliseconds: 200));

    // 底部横向滚动条存在。
    expect(find.byType(Scrollbar), findsWidgets);

    final scrollable = find.byWidgetPredicate(
      (widget) => widget is Scrollable && widget.axisDirection == AxisDirection.right,
    );
    expect(scrollable, findsOneWidget);
    final position = tester.state<ScrollableState>(scrollable).position;
    expect(position.maxScrollExtent, greaterThan(0), reason: '标签必须真的溢出才可滚');

    // 竖直滚轮应当驱动横向标签条（默认 ListView 会丢掉 dy）。
    position.jumpTo(0);
    await tester.pump();
    await tester.sendEventToBinding(
      PointerScrollEvent(
        position: tester.getCenter(scrollable),
        scrollDelta: const Offset(0, 120),
        kind: PointerDeviceKind.mouse,
      ),
    );
    await tester.pump();

    expect(position.pixels, greaterThan(0));

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });
}
