// M1 搜索主页面的交互测试。
//
// 数据源经 `SearchBackend` 注入假实现，因此不依赖 Rust 动态库；这里只验证
// 「查询 → 结果 → 键盘 → 动作」这条表现层链路。
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/updater.dart';
import 'package:lunac/platform/wake.dart';
import 'package:lunac/search/chat_model.dart';
import 'package:lunac/search/search_bridge.dart';
import 'package:lunac/search/search_page.dart';
import 'package:lunac/search/search_result.dart';
import 'package:lunac/theme.dart';

class _FakeBackend implements SearchBackend {
  _FakeBackend(this.items, {this.files = const [], this.catalog = const []});

  final List<SearchResult> items;
  final List<SearchResult> files;
  final List<SystemCatalogEntry> catalog;
  final List<String> actions = [];
  SearchActionOutcome outcome = SearchActionOutcome.executed;
  String? unavailable;
  int searchCalls = 0;

  @override
  String? get unavailableReason => unavailable;

  @override
  List<SearchResult> search(String query) {
    searchCalls++;
    final needle = query.trim();
    if (needle.isEmpty) {
      return items;
    }
    return items
        .where((e) => e.title.contains(needle) || e.subtitle.contains(needle))
        .toList();
  }

  @override
  List<SearchResult> searchApps(String query, {int limit = 60}) =>
      items.where((e) => e.kind == 'app').toList();

  @override
  List<SearchResult> searchFiles(
    String query, {
    String? kind,
    int limit = 60,
  }) => files;

  @override
  List<SystemCatalogEntry> systemCatalog(String query) => catalog;

  @override
  SearchActionOutcome runAction(String id) {
    actions.add(id);
    return outcome;
  }

  @override
  void dispose() {}
}

SearchResult _item(String id, String title) =>
    SearchResult(id: id, title: title, subtitle: 'sub-$id', kind: 'command');

Widget _wrap(SearchPage page) =>
    MaterialApp(theme: buildLunacTheme(), home: page);

/// 只看结果列表里的文本：搜索框自身的 EditableText 也会命中同名文本。
Finder _row(String text) =>
    find.descendant(of: find.byType(ListView), matching: find.text(text));

/// 详细搜索是 640 高的固定档：把测试画布放大，否则会按 600 高的默认画布溢出。
void _useTallSurface(WidgetTester tester) {
  tester.view.physicalSize = const Size(1000, 900);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

void main() {
  testWidgets('lists all entries and filters by query', (tester) async {
    final backend = _FakeBackend([_item('a', 'Alpha'), _item('b', 'Beta')]);
    await tester.pumpWidget(
      _wrap(SearchPage(backend: backend, debounce: Duration.zero)),
    );
    await tester.pumpAndSettle();

    expect(_row('Alpha'), findsOneWidget);
    expect(_row('Beta'), findsOneWidget);

    await tester.enterText(find.byType(TextField), 'Beta');
    await tester.pumpAndSettle();

    expect(_row('Alpha'), findsNothing);
    expect(_row('Beta'), findsOneWidget);
  });

  testWidgets('window height hugs the visible result rows', (tester) async {
    final backend = _FakeBackend([
      _item('a', 'Alpha'),
      _item('b', 'Beta'),
      _item('c', 'Gamma'),
    ]);
    final heights = <double>[];
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          debounce: Duration.zero,
          onResize: (height) async => heights.add(height),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // 68 基础（上下留白 + 搜索栏）+ 2 面板描边 + 3 × 56 行。
    expect(heights.last, 238);
    // 启动序列：结果抽屉收起，只剩搜索栏一行。
    expect(heights.first, 68);

    await tester.enterText(find.byType(TextField), 'Beta');
    await tester.pumpAndSettle();
    expect(heights.last, 126);
  });

  testWidgets('boot reveal ends with the search bar fully visible', (
    tester,
  ) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    // 揭示裁切在动画结束后必须铺满整条（起点是圆形按钮左缘）。
    final clip = tester
        .widgetList<ClipRect>(find.byType(ClipRect))
        .firstWhere((clip) => clip.clipper != null);
    expect(
      clip.clipper!.getClip(const Size(656, 52)),
      const Rect.fromLTRB(0, 0, 656, 52),
    );
  });

  testWidgets('results stay open after the search field loses focus', (
    tester,
  ) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
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
    expect(heights.last, 126);
    expect(_row('Alpha'), findsOneWidget);

    tester.binding.focusManager.primaryFocus?.unfocus();
    await tester.pumpAndSettle();

    // 展开一次就保持（用户 2026-11-09 裁决）：桌面端点结果行会让输入框短暂
    // 失焦，若这时收起面板，点击就落空了。
    expect(heights.last, 126);
    expect(_row('Alpha'), findsOneWidget);
  });

  testWidgets('window height caps at five visible rows', (tester) async {
    final many = [for (var i = 0; i < 8; i++) _item('i$i', 'Item $i')];
    final heights = <double>[];
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: _FakeBackend(many),
          onResize: (height) async => heights.add(height),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // 超过 5 行后窗口高度封顶（68 + 2 + 5×56），列表内部滚动。
    expect(heights.last, 350);
  });

  testWidgets('search bar background drags the window, the field does not', (
    tester,
  ) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    var drags = 0;
    await tester.pumpWidget(
      _wrap(SearchPage(backend: backend, onDrag: () async => drags++)),
    );
    await tester.pumpAndSettle();

    final field = tester.getRect(find.byType(TextField));

    // 左内侧留白（输入框图标之前）拖动 = 移动窗口。
    await tester.dragFrom(
      Offset(field.left - 30, field.center.dy),
      const Offset(40, 20),
    );
    await tester.pumpAndSettle();
    expect(drags, 1);

    // 输入框内横向拖动 = 文本选择，不应变成窗口拖拽。
    await tester.dragFrom(field.center, const Offset(40, 0));
    await tester.pumpAndSettle();
    expect(drags, 1);
  });

  testWidgets('subtitle is hidden until the row is hovered', (tester) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    AnimatedOpacity subtitleOpacity() => tester.widget<AnimatedOpacity>(
      find
          .ancestor(
            of: find.text('sub-a'),
            matching: find.byType(AnimatedOpacity),
          )
          .first,
    );

    expect(subtitleOpacity().opacity, 0);

    final gesture = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await gesture.addPointer(location: Offset.zero);
    addTearDown(gesture.removePointer);
    await gesture.moveTo(tester.getCenter(find.text('Alpha')));
    await tester.pumpAndSettle();
    expect(subtitleOpacity().opacity, 1);

    await gesture.moveTo(const Offset(2, 2));
    await tester.pumpAndSettle();
    expect(subtitleOpacity().opacity, 0);
  });

  testWidgets('arrow keys move the selection and Enter runs it', (
    tester,
  ) async {
    final backend = _FakeBackend([
      _item('a', 'Alpha'),
      _item('b', 'Beta'),
      _item('c', 'Gamma'),
    ]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(TextField));
    await tester.pump();

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(backend.actions, ['c']);
  });

  testWidgets('selection stops at the first and last row', (tester) async {
    final backend = _FakeBackend([_item('a', 'Alpha'), _item('b', 'Beta')]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(TextField));
    await tester.pump();

    // 已经在第一行：向上不越界。
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(backend.actions, ['a']);

    // 越过最后一行后回落到最后一行。
    backend.actions.clear();
    await tester.pump(const Duration(milliseconds: 200));
    for (var i = 0; i < 4; i++) {
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.pump();
    }
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(backend.actions, ['b']);
  });

  testWidgets('Escape clears the query and restores the full list', (
    tester,
  ) async {
    final backend = _FakeBackend([_item('a', 'Alpha'), _item('b', 'Beta')]);
    var hideCalls = 0;
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          debounce: Duration.zero,
          onHide: () async => hideCalls++,
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), 'Beta');
    await tester.pumpAndSettle();
    expect(_row('Alpha'), findsNothing);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();

    expect(_row('Alpha'), findsOneWidget);
    final field = tester.widget<TextField>(find.byType(TextField));
    expect(field.controller?.text, isEmpty);
    // 有查询词时 Esc 只清空，不隐藏窗口。
    expect(hideCalls, 0);
  });

  testWidgets('Escape on an empty query hides the window', (tester) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    var hideCalls = 0;
    await tester.pumpWidget(
      _wrap(SearchPage(backend: backend, onHide: () async => hideCalls++)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byType(TextField));
    await tester.pump();

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();

    expect(hideCalls, 1);
  });

  testWidgets('Quit action is delegated to the host callback', (tester) async {
    final backend = _FakeBackend([_item('quit', 'Quit Lunac')]);
    backend.outcome = SearchActionOutcome.uiHandled;
    var quitCalls = 0;
    await tester.pumpWidget(
      _wrap(SearchPage(backend: backend, onQuit: () async => quitCalls++)),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('Quit Lunac'));
    await tester.pumpAndSettle();

    expect(quitCalls, 1);
  });

  testWidgets('unavailable backend shows the reason instead of results', (
    tester,
  ) async {
    final backend = _FakeBackend(const [])
      ..unavailable = 'rust bridge not loaded';
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    expect(find.text('rust bridge not loaded'), findsWidgets);
  });

  testWidgets('wake-up clipboard text fills an empty search box', (
    tester,
  ) async {
    final backend = _FakeBackend([_item('a', 'Alpha'), _item('b', 'Beta')]);
    await tester.pumpWidget(
      _wrap(SearchPage(backend: backend, debounce: Duration.zero)),
    );
    await tester.pumpAndSettle();
    addTearDown(() => pendingSearchQuery.value = '');

    pendingSearchQuery.value = 'Beta';
    await tester.pumpAndSettle();

    expect(_row('Alpha'), findsNothing);
    expect(_row('Beta'), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      'Beta',
    );

    // 用户已经输入的内容不被剪贴板覆盖。
    await tester.enterText(find.byType(TextField), 'Alpha');
    await tester.pumpAndSettle();
    pendingSearchQuery.value = 'Beta';
    await tester.pumpAndSettle();
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      'Alpha',
    );
  });

  testWidgets('running a result hides the window and clears the query', (
    tester,
  ) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    var hideCalls = 0;
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          debounce: Duration.zero,
          onHide: () async => hideCalls++,
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();

    expect(backend.actions, ['a']);
    expect(hideCalls, 1);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      isEmpty,
    );
  });

  testWidgets('tapping a result on Windows runs it instead of collapsing', (
    tester,
  ) async {
    // 桌面端 TextField 默认「点到别处就 unfocus」：点结果行会先收起面板，
    // 点击因此落空（用户 2026-11-09 反馈）。这里用 Windows 平台复现并守住。
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    try {
      final backend = _FakeBackend([_item('a', 'Alpha')]);
      var hideCalls = 0;
      await tester.pumpWidget(
        _wrap(SearchPage(backend: backend, onHide: () async => hideCalls++)),
      );
      await tester.pumpAndSettle();

      await tester.tap(find.text('Alpha'));
      await tester.pumpAndSettle();

      expect(backend.actions, ['a']);
      expect(hideCalls, 1);
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('double clicking the search bar opens the detailed view', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([_item('a', 'Alpha')]);
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

    await tester.tap(find.byType(TextField));
    await tester.pump(const Duration(milliseconds: 60));
    await tester.tap(find.byType(TextField));
    await tester.pumpAndSettle();

    // 详细搜索是固定档 640 高。
    expect(heights.last, 640);
    expect(find.text('网络搜索'), findsOneWidget);
  });

  testWidgets('Escape leaves the detailed view', (tester) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([_item('a', 'Alpha')]);
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

    await tester.tap(find.byType(TextField));
    await tester.pump(const Duration(milliseconds: 60));
    await tester.tap(find.byType(TextField));
    await tester.pumpAndSettle();
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    // Escape 后先落一帧（`_syncWindowHeight` 在这个帧末才挂收起定时器），
    // 再等 140ms 的收缩延迟走完。
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    // 回到简洁搜索：窗口高度贴合结果行（1 行 = 126），分类栏消失。
    expect(heights.last, 126);
    expect(find.text('网络搜索'), findsNothing);
  });

  testWidgets('the file tab lists files from the backend', (tester) async {
    _useTallSurface(tester);
    final backend = _FakeBackend(
      [_item('a', 'Alpha')],
      files: [
        SearchResult(
          id: r'file:C:\docs\report.pdf',
          title: 'report.pdf',
          subtitle: r'C:\docs\report.pdf',
          kind: 'file',
        ),
      ],
    );
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(TextField));
    await tester.pump(const Duration(milliseconds: 60));
    await tester.tap(find.byType(TextField));
    await tester.pumpAndSettle();

    // 分类栏的「文件」标签；「全部」里可能已有同名分组标题，取第一个（标签在前）。
    await tester.tap(find.text('文件').first);
    await tester.pumpAndSettle();

    expect(_row('report.pdf'), findsOneWidget);
    // 分组标题：分类标签「文件」+ 结果区顶部的同名分组标题（v1 同款）。
    expect(find.text('文件'), findsNWidgets(2));
    // 文件分类下会出现类型胶囊。
    expect(find.text('全部类型'), findsOneWidget);
    // 右侧预览：假路径不存在，界面如实显示「不可用」与三个动作。
    expect(find.text('文件不可用（已移动或删除）'), findsOneWidget);
    expect(find.text('定位'), findsOneWidget);
    expect(find.text('复制'), findsOneWidget);
  });

  testWidgets(
    'catalog tabs list settings and actions, danger needs confirmation',
    (tester) async {
      _useTallSurface(tester);
      final backend = _FakeBackend(
        const [],
        catalog: const [
          SystemCatalogEntry(
            id: 'setting:ms-settings:display',
            title: '显示',
            subtitle: '系统设置',
            danger: false,
            elevatable: false,
          ),
          SystemCatalogEntry(
            id: 'action:act.shutdown',
            title: '关机',
            subtitle: '系统动作',
            danger: true,
            elevatable: false,
          ),
        ],
      );
      await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
      await tester.pumpAndSettle();

      await tester.tap(find.byType(TextField));
      await tester.pump(const Duration(milliseconds: 60));
      await tester.tap(find.byType(TextField));
      await tester.pumpAndSettle();

      await tester.tap(find.text('设置'));
      await tester.pumpAndSettle();
      expect(_row('显示'), findsOneWidget);

      await tester.tap(find.text('系统动作').first);
      await tester.pumpAndSettle();
      expect(_row('关机'), findsOneWidget);

      // 危险动作：第一次 Enter 只上膛，不执行；第二次才真的执行。
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(backend.actions, isEmpty);

      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(backend.actions, ['action:act.shutdown']);
    },
  );

  testWidgets('Shift+Enter runs an elevatable action with admin prefix', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend(
      const [],
      catalog: const [
        SystemCatalogEntry(
          id: 'action:act.devmgr',
          title: '设备管理器',
          subtitle: '系统动作',
          danger: false,
          elevatable: true,
        ),
      ],
    );
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(TextField));
    await tester.pump(const Duration(milliseconds: 60));
    await tester.tap(find.byType(TextField));
    await tester.pumpAndSettle();

    await tester.tap(find.text('系统动作').first);
    await tester.pumpAndSettle();
    expect(_row('设备管理器'), findsOneWidget);

    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.pumpAndSettle();

    // 提权形态把完整 id 交给 Rust：`admin:` + `action:<id>`。
    expect(backend.actions, ['admin:action:act.devmgr']);
  });

  testWidgets(
    'Enter with no rows in the detailed view falls back to web search',
    (tester) async {
      _useTallSurface(tester);
      final backend = _FakeBackend(const []);
      var hideCalls = 0;
      await tester.pumpWidget(
        _wrap(
          SearchPage(
            backend: backend,
            debounce: Duration.zero,
            onHide: () async => hideCalls++,
          ),
        ),
      );
      await tester.pumpAndSettle();

      await tester.tap(find.byType(TextField));
      await tester.pump(const Duration(milliseconds: 60));
      await tester.tap(find.byType(TextField));
      await tester.pumpAndSettle();

      await tester.enterText(find.byType(TextField), 'hello');
      await tester.pumpAndSettle();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();

      expect(backend.actions, ['web:hello']);
      expect(hideCalls, 1);
    },
  );

  testWidgets('right click on an app row offers reveal and copy', (
    tester,
  ) async {
    final backend = _FakeBackend([
      SearchResult(
        id: 'app:C:\\Tools\\Demo.exe',
        title: 'Demo',
        subtitle: '应用程序',
        kind: 'app',
      ),
    ]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Demo'), buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();

    expect(find.text('打开'), findsOneWidget);
    expect(find.text('打开所在位置'), findsOneWidget);
    expect(find.text('复制路径'), findsOneWidget);
    expect(find.text('用 AI 询问'), findsOneWidget);
    expect(find.text('切换为 Bing'), findsNothing);
  });

  testWidgets('right click on a web row offers engine switching', (
    tester,
  ) async {
    final backend = _FakeBackend([
      SearchResult(
        id: 'web:alpha',
        title: '用 Google 搜索 “alpha”',
        subtitle: 'Web 搜索',
        kind: 'web',
      ),
    ]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    await tester.tap(
      find.text('用 Google 搜索 “alpha”'),
      buttons: kSecondaryMouseButton,
    );
    await tester.pumpAndSettle();

    expect(find.text('打开'), findsOneWidget);
    // 当前引擎一项带「（当前）」，另外两项是「切换为 …」。
    expect(find.textContaining('（当前）'), findsOneWidget);
    expect(find.textContaining('切换为 '), findsNWidgets(2));
  });

  testWidgets(
    'hover shows the subtitle without moving the keyboard selection',
    (tester) async {
      final backend = _FakeBackend([_item('a', 'Alpha'), _item('b', 'Beta')]);
      await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
      await tester.pumpAndSettle();

      final gesture = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await gesture.addPointer(location: Offset.zero);
      addTearDown(gesture.removePointer);
      await gesture.moveTo(tester.getCenter(find.text('Beta')));
      await tester.pumpAndSettle();

      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();

      // 选中仍停在第一行（v1：hover 只做视觉高亮，不抢键盘选择）。
      expect(backend.actions, ['a']);
    },
  );

  testWidgets('wake-up re-runs the current query', (tester) async {
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    final before = backend.searchCalls;
    searchRefreshTick.value++;
    await tester.pumpAndSettle();

    expect(backend.searchCalls, before + 1);
  });

  testWidgets(
    'attachments take over the list and Backspace removes the last one',
    (tester) async {
      final backend = _FakeBackend([_item('a', 'Alpha')]);
      final heights = <double>[];
      await tester.pumpWidget(
        _wrap(
          SearchPage(
            backend: backend,
            initialAttachments: const [
              r'C:\docs\report.pdf',
              r'C:\pics\shot.png',
            ],
            onResize: (height) async => heights.add(height),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(_row('report.pdf'), findsOneWidget);
      expect(_row('shot.png'), findsOneWidget);
      expect(_row('AI 分析附件'), findsOneWidget);
      expect(_row('用快捷启动打开'), findsOneWidget);
      expect(_row('Alpha'), findsNothing);

      // 68 基础 + 2 面板描边 + 4 行结果（2 个附件 + AI 占位 + 快捷启动）。
      expect(heights.last, 294);

      await tester.tap(find.byType(TextField));
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
      await tester.pumpAndSettle();

      expect(_row('shot.png'), findsNothing);
      expect(_row('report.pdf'), findsOneWidget);
    },
  );

  testWidgets('tapping an attachment opens it through the backend', (
    tester,
  ) async {
    final backend = _FakeBackend(const []);
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          initialAttachments: const [r'C:\docs\report.pdf'],
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(_row('report.pdf'));
    await tester.pumpAndSettle();

    expect(backend.actions, [r'file:C:\docs\report.pdf']);
  });

  testWidgets('gear button opens the settings panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([_item('a', 'Alpha')]);
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

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();

    // 设置面板与详细搜索同高（640 固定档），且**独占整窗**：搜索栏整个消失。
    expect(heights.last, 640);
    expect(find.byTooltip('设置'), findsNothing);
    expect(find.byType(TextField), findsNothing);
    // 缺省分类「常规」：热键设置 + 开机自启。
    expect(find.text('热键设置'), findsOneWidget);
    // 左侧分类抽屉：常规 / 风格 / AI / 搜索 / 插件（v1 只有这五栏，没有「关于」）。
    for (final label in ['常规', '风格', 'AI', '搜索', '插件']) {
      expect(find.text(label), findsWidgets, reason: '缺少分类：$label');
    }

    // 搜索分类：搜索引擎（下拉 + 保存）+ 文件索引。
    await tester.tap(find.text('搜索').first);
    await tester.pumpAndSettle();
    expect(find.text('搜索引擎'), findsOneWidget);
    expect(find.text('Google'), findsOneWidget);
    expect(find.text('文件索引'), findsOneWidget);
    expect(find.text('热键设置'), findsNothing);

    // AI 模型分区（v1 config/ai.json 的精简四项 + 「命令添加」）。
    await tester.tap(find.text('AI').first);
    await tester.pumpAndSettle();
    expect(find.text('DeepSeek'), findsOneWidget);
    expect(find.text('命令添加'), findsOneWidget);
    // v1 AI 分区末尾的「用量与成本」：打开价表 / 更新价格两个入口 + 候选确认。
    expect(find.text('用量与成本'), findsOneWidget);
    expect(find.text('打开 pricing.json'), findsOneWidget);
    expect(find.text('更新价格（让 agent 抓官方定价页）'), findsOneWidget);

    // 填完三项后保存：单测没有 DLL，界面必须如实报告失败而不是假装成功。
    // 设置面板里没有搜索框，0/1/2 = 模型 / 接口地址 / API 密钥。
    await tester.enterText(find.byType(TextField).at(0), 'deepseek-chat');
    await tester.enterText(
      find.byType(TextField).at(1),
      'https://api.deepseek.com',
    );
    await tester.enterText(find.byType(TextField).at(2), 'sk-test');
    // 分区变多后「保存」可能在滚动区下方：先滚到可见再点。
    await tester.ensureVisible(find.text('保存'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.text('rust bridge not loaded'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    expect(find.text('搜索引擎'), findsNothing);
    expect(find.byTooltip('设置'), findsOneWidget);
    expect(heights.last, 126);
  });

  testWidgets('settings header drags the window like the search bar', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    var drags = 0;
    await tester.pumpWidget(
      _wrap(
        SearchPage(
          backend: backend,
          onResize: (height) async {},
          onDrag: () async => drags++,
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();
    expect(find.text('热键设置'), findsOneWidget);
    // 设置面板独占整窗后，标题栏必须仍是拖拽区（用户 2026-11-20 报障）。
    await tester.dragFrom(
      tester.getCenter(find.text('设置')),
      const Offset(40, 0),
    );
    await tester.pumpAndSettle();
    expect(drags, 1, reason: '设置标题栏没有接过窗口拖动通道');
  });

  testWidgets('settings hotkey recording reports the missing bridge', (
    tester,
  ) async {
    _useTallSurface(tester);
    await tester.pumpWidget(_wrap(SearchPage(backend: _FakeBackend(const []))));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();

    // 桥接缺失时显示默认组合；v1 的热键按钮本身就是录制入口（没有独立「录制」按钮）。
    expect(find.text('热键设置'), findsOneWidget);
    expect(find.text('Ctrl+Alt+Space'), findsOneWidget);

    await tester.tap(find.text('Ctrl+Alt+Space'));
    await tester.pumpAndSettle();
    expect(find.text('按下组合键…'), findsOneWidget);

    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();

    expect(find.text('rust bridge not loaded'), findsOneWidget);
    // 失败不改显示值，也不停在录制态。
    expect(find.text('Ctrl+Alt+Space'), findsOneWidget);
    expect(find.text('按下组合键…'), findsNothing);
  });

  testWidgets('settings general / appearance / plugins panes show their rows', (
    tester,
  ) async {
    _useTallSurface(tester);
    await tester.pumpWidget(_wrap(SearchPage(backend: _FakeBackend(const []))));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();

    // 缺省「常规」：热键设置 + 开机自启；单测没有 DLL，如实显示桥接缺失。
    expect(find.text('开机自启'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);
    expect(find.text('开启'), findsNothing);
    // v1 把「更新」摆在常规分区：当前版本 + 检查更新（检查本身要联网，测试里不点）。
    expect(find.text('更新'), findsOneWidget);
    expect(find.text('当前版本 $kAppVersion'), findsOneWidget);
    expect(find.text('检查更新'), findsOneWidget);
    // v1 2026-10-05 合并后的「自动更新」单开关（默认开，无桥接时也按默认画）。
    expect(find.text('自动更新'), findsOneWidget);
    expect(find.text('开启后启动时自动检查，发现新版本会自动下载安装并重启应用。'), findsOneWidget);

    // 「AI」分类：模型 / 技能 / 用量三组（v1 的四组里 MCP 那组仍缺，见 §待办 3-31）。
    // 技能组在没有桥接时如实报「当前环境没有原生桥」，但仍给出新建入口。
    await tester.tap(find.text('AI').first);
    await tester.pumpAndSettle();
    expect(find.text('技能'), findsOneWidget);
    expect(find.text('新建技能'), findsOneWidget);
    // v1 的 raw URL 导入入口（§待办 3-31 ①）。
    expect(find.text('从 URL 安装'), findsOneWidget);
    expect(find.text('当前环境没有原生桥'), findsOneWidget);

    // 「风格」分类：主题目录 + 主题包（只读展示，不做假的切换按钮）。
    await tester.tap(find.text('风格').first);
    await tester.pumpAndSettle();
    expect(find.text('主题目录'), findsOneWidget);
    expect(find.text('主题包'), findsOneWidget);
    expect(find.text('打开目录'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsWidgets);

    // 「插件」分类：市场面板（单测没有桥接，如实显示不可用）。
    await tester.tap(find.text('插件').first);
    await tester.pumpAndSettle();
    expect(find.text('插件市场'), findsWidgets);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);
  });

  testWidgets('appearance color row opens the v1 picker and commits a preset', (
    tester,
  ) async {
    _useTallSurface(tester);
    await tester.pumpWidget(_wrap(SearchPage(backend: _FakeBackend(const []))));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('风格').first);
    await tester.pumpAndSettle();

    final custom = find.textContaining('底色自定义');
    await tester.ensureVisible(custom);
    await tester.tap(custom);
    await tester.pumpAndSettle();

    final toggle = find.byTooltip('展开色板');
    await tester.ensureVisible(toggle.first);
    await tester.tap(toggle.first);
    await tester.pumpAndSettle();

    // 色板 = 饱和度-明度方块 + 色相条 + 8 个预设（v1 colorPickerHtml 同款结构）。
    expect(
      find.byKey(const ValueKey('appearance-picker-baseColor')),
      findsOneWidget,
    );
    expect(find.byTooltip('收起色板'), findsOneWidget);
    for (final hex in [
      '#cfc8bb',
      '#3a7bd5',
      '#5b9a68',
      '#c9a227',
      '#c05555',
      '#8e6fc0',
      '#3f9a9a',
      '#d07aa0',
    ]) {
      expect(
        find.byKey(ValueKey('appearance-preset-$hex')),
        findsOneWidget,
        reason: '预设色板缺 $hex',
      );
    }

    // 点预设 = 直接把 hex 写进输入框（无桥接时保存会失败，但界面值已更新）。
    await tester.tap(find.byKey(const ValueKey('appearance-preset-#3a7bd5')));
    await tester.pumpAndSettle();
    expect(find.text('#3a7bd5'), findsWidgets);
  });

  testWidgets('settings AI dropdown supports custom entries and delete', (
    tester,
  ) async {
    _useTallSurface(tester);
    await tester.pumpWidget(_wrap(SearchPage(backend: _FakeBackend(const []))));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('AI').first);
    await tester.pumpAndSettle();

    // 供应商下拉：点触发器展开，末尾是 v1 的「— 自定义供应商 —」内联入口。
    await tester.tap(find.text('DeepSeek'));
    await tester.pumpAndSettle();
    expect(find.text('— 自定义供应商 —'), findsOneWidget);
    expect(find.text('OpenAI'), findsOneWidget);

    // × 删除内置项：菜单保持展开，OpenAI 从候选里消失（进隐藏名单）。
    await tester.tap(find.byIcon(Icons.close).first);
    await tester.pumpAndSettle();
    expect(find.text('OpenAI'), findsNothing);

    // 内联新增自定义供应商 → 选中项与触发器标签都变成新名字。
    await tester.tap(find.text('— 自定义供应商 —'));
    await tester.pumpAndSettle();
    expect(find.text('输入名称'), findsOneWidget);
    await tester.enterText(find.byType(TextField).last, 'acme');
    await tester.tap(find.byTooltip('确定'));
    await tester.pumpAndSettle();
    expect(find.text('acme'), findsOneWidget);
  });

  testWidgets('clipboard history entry opens the panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'clipboard-history',
        title: '剪贴板历史',
        subtitle: '最近复制过的文本与文件',
        kind: 'command',
      ),
    ]);
    backend.outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('剪贴板历史'));
    await tester.pumpAndSettle();

    // 面板标题 + 无桥接空态；与设置 / 对话同高（640 固定档）。
    expect(find.text('剪贴板历史'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);
    // 没有条目时不给「清空」按钮（避免点了没反应）。
    expect(find.text('清空'), findsNothing);
    expect(backend.actions, contains('clipboard-history'));
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('不可用（rust 桥接未加载）'), findsNothing);
  });

  testWidgets('quick launch entry opens the panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'quick-launch',
        title: '快捷启动',
        subtitle: '打开自定义文件与程序',
        kind: 'command',
      ),
    ]);
    backend.outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('快捷启动'));
    await tester.pumpAndSettle();

    // 面板标题 + 无桥接空态；与其它大面板同高（640 固定档）。
    expect(find.text('快捷启动'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);
    expect(backend.actions, contains('quick-launch'));
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('不可用（rust 桥接未加载）'), findsNothing);
  });

  testWidgets('memo entry opens the editor panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'memo',
        title: '备忘录',
        subtitle: '本地快速记录（可设检索标识）',
        kind: 'command',
      ),
    ]);
    backend.outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('备忘录'));
    await tester.pumpAndSettle();

    // 面板：标题 + 编辑器 + 动作行（v1 文案：保存为备忘录 / 历史记录）；高度 640。
    expect(find.text('备忘录'), findsOneWidget);
    expect(find.text('保存为备忘录'), findsOneWidget);
    expect(find.text('历史记录'), findsOneWidget);
    expect(backend.actions, contains('memo'));
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('保存为备忘录'), findsNothing);
  });

  testWidgets('translate entry opens the panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'translate',
        title: '翻译',
        subtitle: '翻译与查词',
        kind: 'command',
      ),
    ])..outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('翻译'));
    await tester.pumpAndSettle();

    // 面板：标题栏 + 输入框 + 语言下拉 + 翻译按钮；与其它大面板同高（640）。
    expect(find.text('词典底座 · 免 key'), findsOneWidget);
    expect(find.text('输入要翻译的文字或单词'), findsOneWidget);
    expect(backend.actions, contains('translate'));
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('词典底座 · 免 key'), findsNothing);
  });

  testWidgets('convert entry opens the panel and Esc returns', (tester) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'convert',
        title: '文件转换',
        subtitle: '图片 / 音频 / 视频 / PDF 格式转换',
        kind: 'command',
      ),
    ])..outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('文件转换'));
    await tester.pumpAndSettle();

    // 没有附件：v1 版式的四个页签 + 输入框提示；与其它大面板同高（640）。
    expect(find.text('点击选择文件'), findsOneWidget);
    expect(find.text('PDF 转换'), findsOneWidget);
    expect(find.text('视频格式转换'), findsOneWidget);
    expect(find.text('视频音频提取'), findsOneWidget);
    expect(find.text('音频格式转换'), findsOneWidget);
    expect(find.text('引擎：未选择文件'), findsOneWidget);
    expect(backend.actions, contains('convert'));
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('点击选择文件'), findsNothing);
  });

  testWidgets('AI entry opens the chat panel and Esc returns', (tester) async {
    _useTallSurface(tester);
    addTearDown(chatSession.clear);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'ai',
        title: 'AI 对话',
        subtitle: '与 Lunac 助手对话',
        kind: 'command',
      ),
    ])..outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(find.text('AI 对话'));
    await tester.pumpAndSettle();

    // 对话面板与详细搜索同高（640 固定档）。
    expect(heights.last, 640);
    // 单测环境没有 lunac_core.dll：面板如实显示启动失败原因，而不是空白。
    expect(find.text('rust bridge not loaded'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    expect(find.text('rust bridge not loaded'), findsNothing);
    expect(heights.last, 126);
  });

  testWidgets('chat panel history list opens empty without a native bridge', (
    tester,
  ) async {
    _useTallSurface(tester);
    addTearDown(chatSession.clear);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'ai',
        title: 'AI 对话',
        subtitle: '与 Lunac 助手对话',
        kind: 'command',
      ),
    ])..outcome = SearchActionOutcome.uiHandled;
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    await tester.tap(find.text('AI 对话'));
    await tester.pumpAndSettle();

    // 历史按钮与 v1 一样在输入栏里；没有历史文件时如实显示空态（不崩、不空白）。
    await tester.tap(find.byTooltip('聊天历史'));
    await tester.pumpAndSettle();
    expect(find.text('对话记录'), findsOneWidget);
    expect(find.text('暂无对话记录。开始对话后会自动保存到这里。'), findsOneWidget);

    // 抽屉右上角关闭回到对话（v1 `#chat-drawer-close`）。
    await tester.tap(find.byTooltip('关闭历史'));
    await tester.pumpAndSettle();
    expect(find.text('暂无对话记录。开始对话后会自动保存到这里。'), findsNothing);
  });

  testWidgets('open panels lock the search bar and Esc only closes the panel', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    var hideCalls = 0;
    await tester.pumpWidget(
      _wrap(SearchPage(backend: backend, onHide: () async => hideCalls++)),
    );
    await tester.pumpAndSettle();

    TextField searchField() =>
        tester.widgetList<TextField>(find.byType(TextField)).first;
    expect(searchField().readOnly, isFalse);
    expect(searchField().canRequestFocus, isTrue);

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();
    expect(find.text('热键设置'), findsOneWidget);

    // 设置面板独占整窗：搜索栏（圆钮 / 输入框）都不在 —— 不需要「锁定」也点不到。
    expect(find.byTooltip('设置'), findsNothing);
    expect(find.byType(TextField), findsNothing);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();

    // Esc 只关面板：窗口没有隐藏，搜索栏回来且可编辑。
    expect(find.text('热键设置'), findsNothing);
    expect(hideCalls, 0);
    expect(searchField().readOnly, isFalse);
    expect(searchField().canRequestFocus, isTrue);

    await tester.enterText(find.byType(TextField).first, 'Alpha');
    await tester.pumpAndSettle();
    expect(searchField().controller?.text, 'Alpha');
  });

  testWidgets('wake-up closes an open panel and unlocks the search bar', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([_item('a', 'Alpha')]);
    await tester.pumpWidget(_wrap(SearchPage(backend: backend)));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('设置'));
    await tester.pumpAndSettle();
    expect(find.text('热键设置'), findsOneWidget);
    expect(find.byType(TextField), findsNothing);

    searchRefreshTick.value++;
    await tester.pumpAndSettle();

    // 唤出后回到简洁搜索：面板关闭、搜索栏回来且可编辑。
    expect(find.text('热键设置'), findsNothing);
    final field = tester.widgetList<TextField>(find.byType(TextField)).first;
    expect(field.readOnly, isFalse);
    expect(field.canRequestFocus, isTrue);

    await tester.enterText(find.byType(TextField).first, 'Alpha');
    await tester.pumpAndSettle();
    expect(field.controller?.text, 'Alpha');
  });

  testWidgets('tool editor entry opens the 640 panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'tool-editor',
        title: '工具编辑器',
        subtitle: '管理 AI Agent 自定义工具（MCP 桥）',
        kind: 'command',
      ),
    ])..outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('工具编辑器'));
    await tester.pumpAndSettle();

    // 面板：标题 + 无桥接空态；与其它大面板同高（640 固定档）。
    expect(find.text('工具编辑器'), findsOneWidget);
    expect(find.text('不可用（rust 桥接未加载）'), findsOneWidget);
    expect(backend.actions, contains('tool-editor'));
    expect(heights.last, 640);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('不可用（rust 桥接未加载）'), findsNothing);
  });

  testWidgets('pet entry opens the 640 console panel and Esc returns', (
    tester,
  ) async {
    _useTallSurface(tester);
    final backend = _FakeBackend([
      const SearchResult(
        id: 'pet',
        title: '桌宠',
        subtitle: '独立透明置顶的桌面形象窗（图片或 Live2D 模型）',
        kind: 'command',
      ),
    ])..outcome = SearchActionOutcome.uiHandled;
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

    await tester.tap(_row('桌宠'));
    await tester.pumpAndSettle();

    // 控制台面板：与其它大面板同高（640 固定档）；首行是 v1 的四个入口，
    // 下面两行滑块（形象大小 / 旋转角度）。
    expect(find.text('显示桌宠'), findsOneWidget);
    expect(find.text('鼠标穿透：关'), findsOneWidget);
    expect(find.text('形象大小'), findsOneWidget);
    expect(find.text('旋转角度'), findsOneWidget);
    expect(backend.actions, contains('pet'));
    expect(heights.last, 640);

    // v1 的六个页签都在；「桌宠人格」页有核心人格 + 三份列表（无桥接时如实显示不可用）。
    for (final label in ['模型调试', '热键', '预设动作', '主动反应', '语音朗读', '桌宠人格']) {
      expect(find.text(label), findsOneWidget, reason: '缺少页签：$label');
    }
    // 默认页签「模型调试」已有后端：没选模型时如实提示，而不是「本版尚未接入」。
    expect(
      find.text('先在「更换形象」里导入一个 Live2D 模型（*.model3.json），才能调试参数。'),
      findsOneWidget,
    );
    expect(find.textContaining('本版尚未接入'), findsNothing);
    // 「热键」页也已有后端：没选模型时同样如实提示（不再写「本版尚未接入」）。
    await tester.ensureVisible(find.text('热键'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('热键'));
    await tester.pumpAndSettle();
    expect(
      find.text('先在「更换形象」里导入一个 Live2D 模型（*.model3.json），才能管理热键。'),
      findsOneWidget,
    );
    expect(find.textContaining('本版尚未接入'), findsNothing);

    // 「语音朗读」页也已有配置通道：没有桥接时如实写「不可用」，不再是「本版尚未接入」。
    await tester.ensureVisible(find.text('语音朗读'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('语音朗读'));
    await tester.pumpAndSettle();
    expect(find.textContaining('不可用（rust 桥接未加载）'), findsWidgets);
    expect(find.textContaining('本版尚未接入'), findsNothing);

    await tester.ensureVisible(find.text('桌宠人格'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('桌宠人格'));
    await tester.pumpAndSettle();
    expect(find.text('核心人格'), findsOneWidget);
    expect(find.text('事实记忆'), findsOneWidget);
    expect(find.text('成长笔记'), findsOneWidget);
    expect(find.text('人格建议'), findsOneWidget);
    expect(find.text('只作用于桌宠发起的对话，不影响普通聊天。'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(find.text('显示桌宠'), findsNothing);
  });
}
