// 独立文件抽屉窗口的 Dart 入口（runner 的第二个 Flutter engine）。
//
// 窗口本身由原生 `windows/runner/attachment_window.cpp` 创建并吸附在主窗口
// 搜索栏左端；这里负责画一块纯色底板 + 纵向罗列的文件行，并在尺寸变化时把
// 「逻辑宽 × 逻辑高」回传给原生。
//
// 动画：宽度增长 = 底板从搜索栏左端向左拉开。内容按最终宽度右锚定，
// 所以动画期间文件行在屏幕上的位置不动，只是被底板逐渐揭开（用户 2026-11-09
// 裁决：要「拉开面板」，不要「拉开文件选项」）。
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'platform/appearance.dart';
import 'search/attachment_chip.dart';
import 'search/drawer_metrics.dart';
import 'theme.dart';

const MethodChannel _channel = MethodChannel('lunac/attach');

/// 第二个 engine 的实际入口在 `lib/main.dart`（entrypoint 必须落在根库），
/// 那边转调这里。
void runAttachmentDrawerApp() {
  // 每个 engine 有独立的 Dart 静态区：主窗口选的主题要在各窗口自己再套一次。
  applySavedTheme();
  // 主窗设置里换主题 / 改配色后，抽屉不用重开也能跟上（v1 是同一个 WebView）。
  startAppearanceWatch();
  runApp(const AttachmentDrawerApp());
}

class AttachmentDrawerApp extends StatelessWidget {
  const AttachmentDrawerApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<int>(
      valueListenable: LunacColors.paletteRevision,
      builder: (context, revision, child) => MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: buildLunacTheme(),
        // 背景图只属于搜索结果区 / AI 对话面板；附件抽屉不铺。
        home: const _DrawerPage(),
      ),
    );
  }
}

class _DrawerPage extends StatefulWidget {
  const _DrawerPage();

  @override
  State<_DrawerPage> createState() => _DrawerPageState();
}

class _DrawerPageState extends State<_DrawerPage>
    with SingleTickerProviderStateMixin {
  final List<String> _files = [];

  /// 面板「揭开」动画（0 = 一条都不露，1 = 完全展开）。
  ///
  /// 窗口尺寸只在开始 / 结束时各下发一次，动画完全在 Flutter 内完成 ——
  /// 逐帧 `SetWindowPos` 会不断重建 swapchain，debug 下会掉帧，
  /// 看起来像「展开一半突然全部展开」。
  late final AnimationController _reveal = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 180),
    value: 1,
  );

  /// 原生已施加的逻辑尺寸（避免重复下发）。
  double _windowWidth = 0;
  double _windowHeight = 0;

  /// 当前显示的面板宽度（动画起点）与目标宽度。
  double _panelWidth = 0;
  double _fromWidth = 0;
  double _toWidth = 0;

  /// 面板高度；文件增减时立即生效（只有宽度做「拉开」动画）。
  double _height = 0;

  @override
  void initState() {
    super.initState();
    _channel.setMethodCallHandler(_onCall);
    _channel.invokeMethod<void>('ready');
  }

  @override
  void dispose() {
    _channel.setMethodCallHandler(null);
    _reveal.dispose();
    super.dispose();
  }

  Future<void> _onCall(MethodCall call) async {
    if (call.method != 'update') {
      return;
    }
    final files = (call.arguments as List).cast<String>();
    setState(() {
      _files
        ..clear()
        ..addAll(files);
      if (files.isEmpty) {
        // 窗口收起后鼠标不会再触发 onExit，主动复位。
        _hovering = false;
      }
    });

    final targetWidth = files.isEmpty
        ? 0.0
        : drawerPanelWidthFor(files.map(fileNameOf));
    // 空列表时保留高度：关闭动画期间面板还要有尺寸，宽度归零后原生隐藏窗口。
    final height = files.isEmpty ? _height : drawerPanelHeightFor(files.length);

    _height = height;
    // 窗口先撑到动画期间需要的最大宽度；动画只在 Flutter 内部揭开底板。
    _applyWindowSize(math.max(_panelWidth, targetWidth), height);

    _fromWidth = _visibleWidth;
    _toWidth = targetWidth;
    if ((_toWidth - _fromWidth).abs() < 0.5) {
      _panelWidth = _toWidth;
      if (_windowWidth > _toWidth + 0.5) {
        _applyWindowSize(_toWidth, height);
      }
      return;
    }
    if (_toWidth == 0) {
      // 关闭：先播完收缩动画，再让原生把窗口收掉。
      await _reveal.forward(from: 0);
      _panelWidth = 0;
      _applyWindowSize(0, height);
      return;
    }
    await _reveal.forward(from: 0);
    _panelWidth = _toWidth;
    // 收缩到新的目标宽度后，把窗口一起收回来（展开时窗口已经是目标宽度）。
    if (_windowWidth > _toWidth + 0.5) {
      _applyWindowSize(_toWidth, height);
    }
  }

  /// 当前真正画出来的面板宽度（动画中间值）。
  double get _visibleWidth {
    final t = Curves.easeOutCubic.transform(_reveal.value);
    return _fromWidth + (_toWidth - _fromWidth) * t;
  }

  /// 把「宽 × 高」下发给原生（只在尺寸真的变了时调用）。
  void _applyWindowSize(double width, double height) {
    if ((width - _windowWidth).abs() < 0.5 &&
        (height - _windowHeight).abs() < 0.5) {
      return;
    }
    _windowWidth = width;
    _windowHeight = height;
    _channel.invokeMethod<void>('resize', <double>[width, height]);
  }

  /// 鼠标是否停在文件粘贴区：只有悬停时才显示顶部「一键删除」浮层。
  bool _hovering = false;

  Future<void> _remove(int index) =>
      _channel.invokeMethod<void>('remove', index);

  Future<void> _clear() async {
    await _channel.invokeMethod<void>('clear');
    if (mounted) {
      setState(() => _hovering = false);
    }
  }

  Widget _buildClearButton() {
    return Container(
      decoration: BoxDecoration(
        color: LunacColors.surfaceHover.withValues(alpha: 0.94),
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: IconButton(
        tooltip: '清空全部附件',
        icon: const Icon(Icons.delete_sweep_outlined, size: 16),
        color: LunacColors.textDim,
        visualDensity: VisualDensity.compact,
        padding: const EdgeInsets.symmetric(horizontal: 6),
        constraints: const BoxConstraints(minWidth: 30, minHeight: 24),
        onPressed: _clear,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final targetWidth = _files.isEmpty
        ? 0.0
        : drawerPanelWidthFor(_files.map(fileNameOf));
    return Scaffold(
      backgroundColor: Colors.transparent,
      body: AnimatedBuilder(
        animation: _reveal,
        builder: (context, _) {
          final t = Curves.easeOutCubic.transform(_reveal.value);
          final width = _fromWidth + (_toWidth - _fromWidth) * t;
          return Align(
            alignment: Alignment.centerRight,
            child: SizedBox(
              width: width,
              height: _height,
              child: MouseRegion(
                onEnter: (_) {
                  if (!_hovering) {
                    setState(() => _hovering = true);
                  }
                },
                onExit: (_) {
                  if (_hovering) {
                    setState(() => _hovering = false);
                  }
                },
                child: ClipRRect(
                  // 左端圆角、右端贴着搜索栏（直角）。
                  borderRadius: const BorderRadius.horizontal(
                    left: Radius.circular(12),
                  ),
                  // 最底层的纯色面板：抽屉是一整块面板，文件行画在它上面。
                  child: ColoredBox(
                    color: LunacColors.surface,
                    child: ClipRect(
                      child: Stack(
                        fit: StackFit.expand,
                        children: [
                          // 内容按最终宽度右锚定：动画只揭开底板，文件行不滑动。
                          Positioned.fill(
                            child: OverflowBox(
                              alignment: Alignment.centerRight,
                              minWidth: targetWidth,
                              maxWidth: targetWidth,
                              child: SizedBox(
                                width: targetWidth,
                                height: _height,
                                child: Padding(
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: kDrawerPanelPadding,
                                    vertical: kDrawerPanelPadding,
                                  ),
                                  child: SingleChildScrollView(
                                    child: Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        for (
                                          var index = 0;
                                          index < _files.length;
                                          index++
                                        )
                                          SizedBox(
                                            height: kDrawerRowHeight,
                                            child: Align(
                                              alignment: Alignment.centerLeft,
                                              child: AttachmentChip(
                                                name: fileNameOf(_files[index]),
                                                onRemove: () => _remove(index),
                                              ),
                                            ),
                                          ),
                                      ],
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                          // 一键删除：鼠标不进入粘贴区时完全隐藏；进入后浮在顶部中间。
                          if (_files.isNotEmpty)
                            Positioned(
                              top: 2,
                              left: 0,
                              right: 0,
                              child: IgnorePointer(
                                ignoring: !_hovering,
                                child: AnimatedOpacity(
                                  opacity: _hovering ? 1 : 0,
                                  duration: const Duration(milliseconds: 120),
                                  child: Center(child: _buildClearButton()),
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}
