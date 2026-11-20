// v2 的主题与色板。
//
// 色值取自 docs/ui-style.md §界面 1 的默认主题（中性石墨 · 印象化）：
// surface 22,24,27 / accent #cfc8bb / text #f2f1ee / dim #c2c3c4 / muted #8b8e92。
//
// 2026-11-14 起色板是**运行时可变**的（v1 `window.__lunac_appearance` 的 Flutter 对应物）：
// 主题包的 tokens 经 [LunacColors.apply] 覆盖到这些静态字段上，[paletteRevision] 通知
// 各 FlutterEngine 重建（三处入口在 runApp 之前应用一次持久化的主题）。
//
// 注意：这些字段**不是 const** —— 任何把 `LunacColors.xxx` 当常量用的 `const`
// 表达式都会编译失败，需要去掉那一层的 `const`（颜色值本身不改）。
import 'dart:io';
import 'dart:ui' show ImageFilter;

import 'package:flutter/material.dart';

abstract final class LunacColors {
  static const Color _defaultBackground = Color(0xFF16181B);
  static const Color _defaultSurface = Color(0xFF1B1E22);
  static const Color _defaultSurfaceHover = Color(0xFF2A2D31);
  static const Color _defaultBorder = Color(0x1FFFFFFF);
  static const Color _defaultDivider = Color(0x3DCFC8BB);
  static const Color _defaultAccent = Color(0xFFCFC8BB);
  static const Color _defaultText = Color(0xFFF2F1EE);
  static const Color _defaultTextDim = Color(0xFFC2C3C4);
  static const Color _defaultTextMuted = Color(0xFF8B8E92);
  static const Color _defaultDanger = Color(0xFFE06C6C);
  static const Color _defaultWarning = Color(0xFFC6A86A);
  static const Color _defaultTerminal = Color(0xFF0B0B0C);

  /// 窗口底色（= ui-style 的 `--surface-rgb` 22,24,27）。
  static Color background = _defaultBackground;

  /// 结果区 / 卡片底（比窗口底色略亮一档）。
  static Color surface = _defaultSurface;

  /// hover / 图标底。
  static Color surfaceHover = _defaultSurfaceHover;

  /// 描边（`--border-glass` 的 Flutter 等价物）。
  static Color border = _defaultBorder;

  /// 面板拼接处的接缝：比 [border] 更亮一档，让「搜索框 / 结果区」的分界一眼可见。
  static Color divider = _defaultDivider;

  /// 主色（`--accent-rgb` 207,200,187）。
  static Color accent = _defaultAccent;

  static Color text = _defaultText;
  static Color textDim = _defaultTextDim;
  static Color textMuted = _defaultTextMuted;
  static Color danger = _defaultDanger;

  /// 警示 / 「已停止」等中间态（v1 `--yellow` 的 Flutter 等价物）。
  static Color warning = _defaultWarning;

  /// 终端底（v1 xterm 卡的 `background: #0b0b0c`）。
  static Color terminal = _defaultTerminal;

  /// 色板版本号：主题切换后自增，`main.dart` 的根组件据此重建。
  static final ValueNotifier<int> paletteRevision = ValueNotifier<int>(0);

  /// 覆盖色板（只覆盖非空项；null = 保持当前值）。改完通知根组件重建。
  ///
  /// [accent] 同时用于派生 `divider`：主题只给了主色时，接缝也要跟着走，否则
  /// 换主题后分隔线还是旧的米色。
  static void apply({
    Color? background,
    Color? surface,
    Color? surfaceHover,
    Color? border,
    Color? divider,
    Color? accent,
    Color? text,
    Color? textDim,
    Color? textMuted,
    Color? danger,
    Color? warning,
    Color? terminal,
  }) {
    if (background != null) {
      LunacColors.background = background;
    }
    if (surface != null) {
      LunacColors.surface = surface;
    }
    if (surfaceHover != null) {
      LunacColors.surfaceHover = surfaceHover;
    }
    if (border != null) {
      LunacColors.border = border;
    }
    if (divider != null) {
      LunacColors.divider = divider;
    }
    if (accent != null) {
      LunacColors.accent = accent;
      LunacColors.divider = accent.withValues(alpha: 0.24);
    }
    if (text != null) {
      LunacColors.text = text;
    }
    if (textDim != null) {
      LunacColors.textDim = textDim;
    }
    if (textMuted != null) {
      LunacColors.textMuted = textMuted;
    }
    if (danger != null) {
      LunacColors.danger = danger;
    }
    if (warning != null) {
      LunacColors.warning = warning;
    }
    if (terminal != null) {
      LunacColors.terminal = terminal;
    }
    paletteRevision.value++;
  }

  /// 恢复默认色板（v1「恢复默认主题」）。
  static void reset() {
    background = _defaultBackground;
    surface = _defaultSurface;
    surfaceHover = _defaultSurfaceHover;
    border = _defaultBorder;
    divider = _defaultDivider;
    accent = _defaultAccent;
    text = _defaultText;
    textDim = _defaultTextDim;
    textMuted = _defaultTextMuted;
    danger = _defaultDanger;
    warning = _defaultWarning;
    terminal = _defaultTerminal;
    paletteRevision.value++;
  }
}

/// 全局主题：Material 3 + 单一深色（跟随 [LunacColors] 的当前值）。
ThemeData buildLunacTheme() {
  final scheme = ColorScheme.dark(
    primary: LunacColors.accent,
    onPrimary: LunacColors.background,
    surface: LunacColors.surface,
    onSurface: LunacColors.text,
    error: LunacColors.danger,
    onError: LunacColors.background,
  );
  final base = ThemeData(useMaterial3: true, colorScheme: scheme);
  return base.copyWith(
    scaffoldBackgroundColor: LunacColors.background,
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: LunacColors.accent,
      // 0.2 在深色玻璃底上几乎看不见（用户 2026-11-21 报障）；0.38 既可见又不遮字。
      selectionColor: LunacColors.accent.withValues(alpha: 0.38),
      selectionHandleColor: LunacColors.accent,
    ),
    snackBarTheme: SnackBarThemeData(
      backgroundColor: LunacColors.surfaceHover,
      contentTextStyle: TextStyle(color: LunacColors.text, fontSize: 13),
      behavior: SnackBarBehavior.floating,
    ),
    tooltipTheme: const TooltipThemeData(
      waitDuration: Duration(milliseconds: 500),
    ),
  );
}

/// 自定义背景图（v1 的 `--bg-image` 三层叠加：图片 + 模糊 + 饱和度 + 反光）。
///
/// 参数刻意用基础类型而不是 `AppearanceConfig`：`platform/appearance.dart` 已经
/// 依赖本文件（色板），这里再反向引用会形成循环。
///
/// 2026-11-21（用户口径）：背景图**只铺在搜索结果区 / AI 对话面板内部**，不再铺整窗
/// （整窗透明区域会把图露在最底层，看起来像单独一层）。面板用 [AppearanceBackdropImage]
/// 在自身裁剪区内铺底，位置与圆角由面板容器决定。
class AppearanceBackdrop extends StatelessWidget {
  const AppearanceBackdrop({
    super.key,
    required this.imagePath,
    required this.blur,
    required this.saturate,
    required this.opacity,
    required this.sheen,
    required this.child,
  });

  final String? imagePath;
  final double blur;
  final double saturate;
  final double opacity;
  final double sheen;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (imagePath == null || imagePath!.isEmpty || opacity <= 0) {
      return child;
    }
    return Stack(
      fit: StackFit.expand,
      children: [
        AppearanceBackdropImage(
          imagePath: imagePath,
          blur: blur,
          saturate: saturate,
          opacity: opacity,
          sheen: sheen,
        ),
        child,
      ],
    );
  }
}

/// 只画背景图本身（无子组件）：面板把它放在内容下方即可。
///
/// 没有图片 / 透明度为 0 ⇒ [SizedBox.shrink]（调用方的 Stack 不需要额外判断）。
class AppearanceBackdropImage extends StatelessWidget {
  const AppearanceBackdropImage({
    super.key,
    required this.imagePath,
    required this.blur,
    required this.saturate,
    required this.opacity,
    required this.sheen,
  });

  final String? imagePath;
  final double blur;
  final double saturate;
  final double opacity;
  final double sheen;

  @override
  Widget build(BuildContext context) {
    final path = imagePath;
    if (path == null || path.isEmpty || opacity <= 0) {
      return const SizedBox.shrink();
    }
    Widget image = Image.file(
      File(path),
      fit: BoxFit.cover,
      gaplessPlayback: true,
      errorBuilder: (context, error, stackTrace) => const SizedBox.shrink(),
    );
    final sat = saturate.clamp(0.0, 200.0) / 100.0;
    if (sat != 1) {
      image = ColorFiltered(
        colorFilter: ColorFilter.matrix(_saturationMatrix(sat)),
        child: image,
      );
    }
    final sigma = blur.clamp(0.0, 40.0);
    if (sigma > 0) {
      image = ImageFiltered(
        imageFilter: ImageFilter.blur(sigmaX: sigma, sigmaY: sigma),
        child: image,
      );
    }
    image = Opacity(opacity: opacity.clamp(0.0, 1.0), child: image);
    return Stack(
      fit: StackFit.expand,
      children: [
        image,
        // 「反光」：顶部一层很淡的白光（v1 `--glass-sheen-alpha` = sheen × 0.22）。
        if (sheen > 0)
          IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topCenter,
                  end: Alignment.bottomCenter,
                  colors: [
                    Colors.white.withValues(
                      alpha: sheen.clamp(0.0, 1.0) * 0.22,
                    ),
                    Colors.transparent,
                  ],
                  stops: const [0, 0.35],
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// 饱和度矩阵（0 = 灰度，1 = 原色，>1 = 加饱和；v1 `--bg-saturate` 0–200%）。
List<double> _saturationMatrix(double value) {
  const lumR = 0.2126;
  const lumG = 0.7152;
  const lumB = 0.0722;
  final invert = 1 - value;
  return [
    lumR * invert + value, lumG * invert, lumB * invert, 0, 0, //
    lumR * invert, lumG * invert + value, lumB * invert, 0, 0,
    lumR * invert, lumG * invert, lumB * invert + value, 0, 0,
    0, 0, 0, 1, 0,
  ];
}
