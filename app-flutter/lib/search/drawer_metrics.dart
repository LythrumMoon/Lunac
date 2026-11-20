// 附件抽屉的布局常数与测量：主窗口（判断贴合圆角）与抽屉窗口（布局）共用。
//
// 抽屉是独立渲染的第二个 engine，但它和主窗口编译的是同一份代码，所以把
// 尺寸口径收在这里，避免两边各写一套魔法数字后漂移。
import 'dart:math' as math;

import 'package:flutter/painting.dart';

/// 面板四周内边距。
const double kDrawerPanelPadding = 8;

/// 每个文件行的高度（单文件时 8 + 36 + 8 = 52，与搜索栏等高）。
const double kDrawerRowHeight = 36;

/// 面板宽度夹在 [最小, 最大] 之间（按最长文件名算）。
const double kDrawerMinWidth = 150;
const double kDrawerMaxWidth = 240;

/// 面板最大高度：主窗口最大高度 350 - 上下留白 16，即不越过主面板底边。
const double kDrawerMaxHeight = 334;

/// 面板高度 = 上下内边距 + 每行高度；文件多到超出上限后在面板内滚动。
double drawerPanelHeightFor(int fileCount) =>
    (kDrawerPanelPadding * 2 + fileCount * kDrawerRowHeight)
        .clamp(0, kDrawerMaxHeight);

/// 面板宽度 = 最长文件名算出的 chip 宽 + 左右内边距。
double drawerPanelWidthFor(Iterable<String> fileNames) {
  var textWidth = 0.0;
  for (final name in fileNames) {
    final painter = TextPainter(
      text: TextSpan(text: name, style: const TextStyle(fontSize: 12)),
      maxLines: 1,
      textDirection: TextDirection.ltr,
    )..layout(maxWidth: 160);
    textWidth = math.max(textWidth, painter.width.clamp(0.0, 160.0));
  }
  // 与 `AttachmentChip` 的排布一致：左内边距 8 + 图标 12 + 间距 6 +
  // 文本（≤160）+ 关闭按钮 22。
  final chipWidth = 8 + 12 + 6 + textWidth + 22;
  return (chipWidth + kDrawerPanelPadding * 2)
      .clamp(kDrawerMinWidth, kDrawerMaxWidth);
}
