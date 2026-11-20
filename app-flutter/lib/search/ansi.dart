// ANSI 转义解析（v1 由 xterm.js 解释；v2 的对话工具卡用这份纯 Dart 实现把 ANSI 变成
// 可着色的 `TextSpan`）。只做「静态回放」需要的那部分：
//   · SGR（`ESC[…m`）：粗体 / 暗色 / 斜体 / 下划线 / 反显 / 16 色 / 256 色 / 真彩色；
//   · 其余 CSI（光标移动、清屏等）与 OSC 一律**丢弃**（管道输出里出现这些不会破坏排版）；
//   · `\r` 回到行首（进度条覆盖）、`\b` 回退一格、`\n` 换行。
//
// 颜色取 xterm.js 的默认 16 色与 v1 终端主题的前景/背景（`#d6d6d6` / `#0b0b0c`），
// 这样命令输出里的红/绿/黄与 v1 肉眼一致。
import 'package:flutter/material.dart';

/// 一段同一样式的文本。
class AnsiSpan {
  const AnsiSpan({
    required this.text,
    this.foreground,
    this.background,
    this.bold = false,
    this.dim = false,
    this.italic = false,
    this.underline = false,
  });

  final String text;
  final Color? foreground;
  final Color? background;
  final bool bold;
  final bool dim;
  final bool italic;
  final bool underline;

  bool sameStyle(AnsiSpan other) =>
      foreground == other.foreground &&
      background == other.background &&
      bold == other.bold &&
      dim == other.dim &&
      italic == other.italic &&
      underline == other.underline;

  TextStyle toTextStyle() {
    var color = foreground;
    if (dim && color != null) {
      color = color.withValues(alpha: 0.62);
    }
    return TextStyle(
      color: color,
      backgroundColor: background,
      fontWeight: bold ? FontWeight.w700 : FontWeight.w400,
      fontStyle: italic ? FontStyle.italic : FontStyle.normal,
      decoration: underline ? TextDecoration.underline : TextDecoration.none,
      decorationColor: color,
    );
  }
}

/// v1 终端主题的默认前景（xterm `foreground: "#d6d6d6"`）。
const Color ansiDefaultForeground = Color(0xFFD6D6D6);

/// v1 终端背景（`#0b0b0c`）—— 反显时用作前景。
const Color ansiDefaultBackground = Color(0xFF0B0B0C);

/// xterm.js 默认 16 色（0–7 普通，8–15 亮色）。
const List<Color> ansiPalette16 = [
  Color(0xFF000000),
  Color(0xFFCD0000),
  Color(0xFF00CD00),
  Color(0xFFCDCD00),
  Color(0xFF0000EE),
  Color(0xFFCD00CD),
  Color(0xFF00CDCD),
  Color(0xFFE5E5E5),
  Color(0xFF7F7F7F),
  Color(0xFFFF0000),
  Color(0xFF00FF00),
  Color(0xFFFFFF00),
  Color(0xFF5C5CFF),
  Color(0xFFFF00FF),
  Color(0xFF00FFFF),
  Color(0xFFFFFFFF),
];

/// 256 色调色板（16–231 为 6×6×6 色立方，232–255 为灰阶）。
Color ansi256(int index) {
  if (index < 16) {
    return ansiPalette16[index.clamp(0, 15)];
  }
  if (index < 232) {
    final value = index - 16;
    int channel(int n) => n == 0 ? 0 : 55 + n * 40;
    return Color.fromARGB(
      255,
      channel(value ~/ 36),
      channel((value % 36) ~/ 6),
      channel(value % 6),
    );
  }
  final gray = 8 + (index - 232) * 10;
  return Color.fromARGB(255, gray, gray, gray);
}

/// 当前样式状态（解析过程中的可变量）。
class _Style {
  Color? foreground;
  Color? background;
  bool bold = false;
  bool dim = false;
  bool italic = false;
  bool underline = false;
  bool inverse = false;

  _Style copy() => _Style()
    ..foreground = foreground
    ..background = background
    ..bold = bold
    ..dim = dim
    ..italic = italic
    ..underline = underline
    ..inverse = inverse;

  void reset() {
    foreground = null;
    background = null;
    bold = false;
    dim = false;
    italic = false;
    underline = false;
    inverse = false;
  }

  AnsiSpan span(String text) {
    final effectiveForeground = foreground ?? ansiDefaultForeground;
    final fg = inverse
        ? (background ?? ansiDefaultBackground)
        : effectiveForeground;
    final bg = inverse ? effectiveForeground : background;
    return AnsiSpan(
      text: text,
      foreground: fg,
      background: bg,
      bold: bold,
      dim: dim,
      italic: italic,
      underline: underline,
    );
  }
}

/// 解析过程中的「屏幕」状态（v1 由 xterm.js 维护）。
///
/// 已实现：可打印字符 / 换行 / 回车 / 退格 + CSI 的光标移动与擦除（`A B C D G H f K J`）
/// + **滚动区域**（`CSI r`，DECSTBM）+ **备用屏**（`CSI ?47/?1047/?1049 h/l`）。
/// 其余私有模式（`?25l` 等）仍然丢掉 —— 与 xterm 对未知序列的处理一致。
class _Screen {
  /// 行数上限（防极端输出把界面拖垮；与对话条目同量级）。
  static const int maxLines = 4000;

  /// 单行字符上限。
  static const int maxColumns = 4000;

  /// 主屏的行缓冲。进备用屏时整体换掉，退出时换回来（[lines] 因此不是 final）。
  List<List<({String char, AnsiSpan style})>> lines = [[]];
  final _Style style = _Style();

  /// 滚动区域顶行（0 起，含）。
  int regionTop = 0;

  /// 滚动区域底行（0 起，含）；`null` = 一直到缓冲区末尾（未设区域）。
  int? regionBottom;

  /// 进备用屏前的主屏快照（`null` = 当前就在主屏）。
  List<List<({String char, AnsiSpan style})>>? _savedLines;
  int _savedRow = 0;
  int _savedColumn = 0;
  int _savedRegionTop = 0;
  int? _savedRegionBottom;

  /// 当前行号（0 起）。
  int row = 0;

  /// 当前列号（0 起；写入前会按需补空格）。
  int column = 0;

  List<({String char, AnsiSpan style})> get _current => lines[row];

  bool get _regionActive => regionTop > 0 || regionBottom != null;

  void write(String char) {
    final line = _current;
    if (column > line.length) {
      final target = column.clamp(0, maxColumns);
      while (line.length < target && line.length < maxColumns) {
        line.add(_blank());
      }
    }
    if (column < line.length) {
      line[column] = (char: char, style: style.span(char));
    } else if (line.length < maxColumns) {
      line.add((char: char, style: style.span(char)));
    }
    column++;
  }

  void newline() {
    if (_regionActive) {
      final bottom = (regionBottom ?? lines.length - 1).clamp(0, maxLines - 1);
      if (row >= bottom && row >= regionTop) {
        // 在滚动区域底部换行 ⇒ **只滚动区域内**：丢掉区域顶行，底部补空行。
        // 区域外的历史行保持不动（命令回放要留住完整输出）。
        if (bottom < lines.length) {
          lines.removeAt(regionTop.clamp(0, lines.length - 1));
          lines.insert(bottom, []);
        } else if (lines.length < maxLines) {
          lines.add([]);
        }
        row = bottom;
        column = 0;
        return;
      }
    }
    if (row == lines.length - 1) {
      if (lines.length < maxLines) {
        lines.add([]);
        row++;
      }
    } else if (row < lines.length - 1) {
      row++;
    }
    column = 0;
  }

  /// `CSI top ; bottom r`（1 起，含）；`bottom` 为 `null` 表示省略。
  ///
  /// 与 xterm 一致：非法区间（bottom ≤ top）**整条忽略**（区域与光标都不动）。
  void setScrollRegion(int topOneBased, int? bottomOneBased) {
    final top = (topOneBased - 1).clamp(0, maxLines - 1);
    final bottom = bottomOneBased == null
        ? null
        : (bottomOneBased - 1).clamp(0, maxLines - 1);
    if (bottom != null && bottom <= top) {
      return;
    }
    regionTop = top;
    regionBottom = bottom;
    row = regionTop;
    column = 0;
  }

  /// `CSI ? 1049 h` / `?47h` / `?1047h`：切到一张**空白备用屏**，主屏原样保存。
  void enterAlternateScreen() {
    if (_savedLines != null) {
      return;
    }
    _savedLines = lines;
    _savedRow = row;
    _savedColumn = column;
    _savedRegionTop = regionTop;
    _savedRegionBottom = regionBottom;
    lines = [[]];
    row = 0;
    column = 0;
    regionTop = 0;
    regionBottom = null;
  }

  /// `CSI ? 1049 l` / `?47l` / `?1047l`：丢掉备用屏内容，主屏与光标原样恢复。
  void leaveAlternateScreen() {
    final saved = _savedLines;
    if (saved == null) {
      return;
    }
    lines = saved;
    row = _savedRow.clamp(0, lines.length - 1);
    column = _savedColumn;
    regionTop = _savedRegionTop;
    regionBottom = _savedRegionBottom;
    _savedLines = null;
  }

  void carriageReturn() => column = 0;

  void backspace() {
    if (column > 0) {
      column--;
    }
  }

  void moveUp(int n) {
    row = (row - n).clamp(0, lines.length - 1);
  }

  void moveDown(int n) {
    row = (row + n).clamp(0, lines.length - 1);
  }

  void moveForward(int n) {
    column = (column + n).clamp(0, maxColumns);
  }

  void moveBack(int n) {
    column = (column - n).clamp(0, maxColumns);
  }

  /// `CSI n G`（1 起）。
  void setColumn(int oneBased) {
    column = (oneBased - 1).clamp(0, maxColumns);
  }

  /// `CSI r ; c H`（都是 1 起）。
  void setPosition(int oneBasedRow, int oneBasedColumn) {
    row = (oneBasedRow - 1).clamp(0, lines.length - 1);
    column = (oneBasedColumn - 1).clamp(0, maxColumns);
  }

  /// `CSI n K`：0 = 光标到行尾；1 = 行首到光标（含）；2 = 整行。
  void eraseLine(int mode) {
    final line = _current;
    switch (mode) {
      case 1:
        final end = column.clamp(0, line.length);
        for (var i = 0; i < end; i++) {
          line[i] = _blank();
        }
        break;
      case 2:
        line.clear();
        break;
      default:
        final start = column.clamp(0, line.length);
        if (start < line.length) {
          line.removeRange(start, line.length);
        }
    }
  }

  /// `CSI n J`：0 = 光标到屏幕末尾；1 = 屏幕开头到光标；2 / 3 = 整屏。
  void eraseDisplay(int mode) {
    switch (mode) {
      case 1:
        final line = _current;
        final end = column.clamp(0, line.length);
        final kept = [
          for (var i = 0; i < line.length; i++)
            if (i >= end) line[i] else _blank(),
        ];
        lines
          ..clear()
          ..add(kept);
        row = 0;
        break;
      case 2:
      case 3:
        lines
          ..clear()
          ..add([]);
        row = 0;
        column = 0;
        break;
      default:
        final start = column.clamp(0, _current.length);
        if (start < _current.length) {
          _current.removeRange(start, _current.length);
        }
        if (row + 1 < lines.length) {
          lines.removeRange(row + 1, lines.length);
        }
    }
  }

  static ({String char, AnsiSpan style}) _blank() => (
    char: ' ',
    style: const AnsiSpan(text: ' ', foreground: ansiDefaultForeground),
  );
}

/// 把带 ANSI 的终端输出解析成「按行、按样式切段」的文本。
///
/// 返回的顺序即显示顺序；换行由调用方在每行之间补 `\n`（见 [ansiSpansToTextSpan]）。
///
/// 回放口径对齐 xterm：`\r` 覆盖当前行、`\b` 退一格、CSI 的 `A/B/C/D/G/H/f` 移动光标、
/// `K` / `J` 擦除、`r` 设滚动区域（只滚区域内）、`?47/?1047/?1049 h/l` 切备用屏
/// （退出时主屏原样恢复）。
List<AnsiSpan> parseAnsi(String raw) {
  final screen = _Screen();
  var i = 0;
  while (i < raw.length) {
    final code = raw.codeUnitAt(i);
    if (code == 0x1B) {
      i = _consumeEscape(raw, i, screen);
      continue;
    }
    if (code == 0x0A) {
      screen.newline();
      i++;
      continue;
    }
    if (code == 0x0D) {
      screen.carriageReturn();
      i++;
      continue;
    }
    if (code == 0x08) {
      screen.backspace();
      i++;
      continue;
    }
    if (code < 0x20 && code != 0x09) {
      // 其余控制字符（响铃等）不显示。
      i++;
      continue;
    }
    screen.write(String.fromCharCode(code));
    i++;
  }

  // 合并同样式的相邻单元；行尾空白裁掉（xterm 也会裁掉尾随空白）。
  final spans = <AnsiSpan>[];
  final lines = screen.lines;
  for (var index = 0; index < lines.length; index++) {
    if (index > 0) {
      spans.add(AnsiSpan(text: '\n', foreground: ansiDefaultForeground));
    }
    final line = lines[index];
    var end = line.length;
    while (end > 0 && line[end - 1].char == ' ') {
      end--;
    }
    for (var c = 0; c < end; c++) {
      final cell = line[c];
      if (spans.isNotEmpty &&
          spans.last.text != '\n' &&
          spans.last.sameStyle(cell.style)) {
        spans[spans.length - 1] = AnsiSpan(
          text: spans.last.text + cell.char,
          foreground: cell.style.foreground,
          background: cell.style.background,
          bold: cell.style.bold,
          dim: cell.style.dim,
          italic: cell.style.italic,
          underline: cell.style.underline,
        );
      } else {
        spans.add(cell.style);
      }
    }
  }
  // 末尾的空行不占高度（命令输出常以换行收尾）。
  while (spans.isNotEmpty && spans.last.text == '\n') {
    spans.removeLast();
  }
  return spans;
}

/// 解析后拼成可直接交给 `SelectableText.rich` 的 `TextSpan`。
TextSpan ansiSpansToTextSpan(List<AnsiSpan> spans, {TextStyle? base}) {
  return TextSpan(
    style: base,
    children: [
      for (final span in spans)
        TextSpan(text: span.text, style: span.toTextStyle()),
    ],
  );
}

/// 去掉全部 ANSI / 控制序列，只留可读文本（写日志、做单行摘要时用）。
String stripAnsi(String text) {
  final buffer = StringBuffer();
  var i = 0;
  while (i < text.length) {
    final code = text.codeUnitAt(i);
    if (code == 0x1B) {
      i = _consumeEscape(text, i);
      continue;
    }
    if (code == 0x0D || code == 0x08) {
      i++;
      continue;
    }
    if (code < 0x20 && code != 0x09 && code != 0x0A) {
      i++;
      continue;
    }
    buffer.writeCharCode(code);
    i++;
  }
  return buffer.toString();
}

/// 从 [start]（指向 ESC）开始吃掉一个转义序列，返回其后的下标。
///
/// `screen == null` 时只跳过序列（[stripAnsi] 用，不需要解释光标 / 擦除）。
int _consumeEscape(String raw, int start, [_Screen? screen]) {
  final next = start + 1 < raw.length
      ? String.fromCharCode(raw.codeUnitAt(start + 1))
      : '';
  // CSI：ESC [ … 终止字节（0x40–0x7E）
  if (next == '[') {
    var i = start + 2;
    while (i < raw.length) {
      final code = raw.codeUnitAt(i);
      if (code >= 0x40 && code <= 0x7E) {
        final finalByte = String.fromCharCode(code);
        final params = raw.substring(start + 2, i);
        final target = screen;
        if (target != null) {
          _applyCsi(target, finalByte, params);
        }
        return i + 1;
      }
      i++;
    }
    return raw.length;
  }
  // OSC：ESC ] … BEL 或 ST（ESC \）
  if (next == ']') {
    var i = start + 2;
    while (i < raw.length) {
      if (raw.codeUnitAt(i) == 0x07) {
        return i + 1;
      }
      if (raw.codeUnitAt(i) == 0x1B &&
          i + 1 < raw.length &&
          raw.codeUnitAt(i + 1) == 0x5C) {
        return i + 2;
      }
      i++;
    }
    return raw.length;
  }
  // 其它两字节转义（ESC c 等）
  return start + 2 <= raw.length ? start + 2 : raw.length;
}

/// 应用一条 CSI 序列：`m` 走 SGR，`r` 设滚动区域，`?47/?1047/?1049 h/l` 切备用屏，
/// 其余是光标移动 / 擦除。
///
/// 不认识的 CSI（`?25l` 等私有模式、设备状态查询）**直接丢掉** —— 与 xterm 对
/// 未知序列的处理一致，不把控制字符漏到屏幕上。
void _applyCsi(_Screen screen, String finalByte, String params) {
  final values = _csiParams(params);
  final isPrivate = params.startsWith('?');
  int at(int index, int fallback) =>
      index < values.length && values[index] > 0 ? values[index] : fallback;
  int move() => values.isEmpty || values.first <= 0 ? 1 : values.first;
  switch (finalByte) {
    case 'm':
      _applySgr(screen.style, params);
      break;
    case 'A':
      screen.moveUp(move());
      break;
    case 'B':
      screen.moveDown(move());
      break;
    case 'C':
      screen.moveForward(move());
      break;
    case 'D':
      screen.moveBack(move());
      break;
    case 'G':
      screen.setColumn(move());
      break;
    case 'H':
    case 'f':
      screen.setPosition(at(0, 1), at(1, 1));
      break;
    case 'K':
      screen.eraseLine(values.isEmpty ? 0 : values.first);
      break;
    case 'J':
      screen.eraseDisplay(values.isEmpty ? 0 : values.first);
      break;
    case 'r':
      // DECSTBM：`CSI r`（无参）= 复位；`CSI 2;4r` = 第 2~4 行。私有形态不是它。
      if (!isPrivate) {
        screen.setScrollRegion(
          at(0, 1),
          values.length > 1 && values[1] > 0 ? values[1] : null,
        );
      }
      break;
    case 'h':
    case 'l':
      // 备用屏三套写法（`?1049` 会连光标一起存/取，`?47` / `?1047` 只切屏）。
      if (isPrivate &&
          (values.contains(1049) || values.contains(1047) || values.contains(47))) {
        if (finalByte == 'h') {
          screen.enterAlternateScreen();
        } else {
          screen.leaveAlternateScreen();
        }
      }
      break;
    default:
      // 其它 CSI（滚动区域以外的私有模式、设备状态查询）不影响回放。
      break;
  }
}

/// 解析 CSI 参数串（`2` / `1;1` / `?25`）。非法段按 0 处理。
List<int> _csiParams(String params) {
  if (params.isEmpty) {
    return const [];
  }
  final cleaned = params.replaceFirst(RegExp(r'^[?<>=]'), '');
  if (cleaned.isEmpty) {
    return const [];
  }
  return [
    for (final part in cleaned.split(';')) int.tryParse(part.trim()) ?? 0,
  ];
}

/// 应用一条 SGR 参数串（`31;1` 等）到样式上。
void _applySgr(_Style style, String params) {
  if (params.isEmpty) {
    style.reset();
    return;
  }
  final parts = params.split(';');
  for (var i = 0; i < parts.length; i++) {
    final part = parts[i];
    if (part.contains(':')) {
      // 兼容 `38:5:n` / `4:3` 这类冒号写法：只取第一段做常见解释。
      final head = int.tryParse(part.split(':').first);
      if (head != null) {
        _applySimple(style, head);
      }
      continue;
    }
    final value = int.tryParse(part);
    if (value == null) {
      continue;
    }
    if (value == 38 || value == 48) {
      final isForeground = value == 38;
      // 38;5;n 或 38;2;r;g;b
      if (i + 1 < parts.length) {
        final mode = int.tryParse(parts[i + 1]);
        if (mode == 5 && i + 2 < parts.length) {
          final index = int.tryParse(parts[i + 2]);
          if (index != null) {
            _setColor(style, isForeground, ansi256(index));
            i += 2;
            continue;
          }
        }
        if (mode == 2 && i + 4 < parts.length) {
          final r = int.tryParse(parts[i + 2]);
          final g = int.tryParse(parts[i + 3]);
          final b = int.tryParse(parts[i + 4]);
          if (r != null && g != null && b != null) {
            _setColor(
              style,
              isForeground,
              Color.fromARGB(255, r.clamp(0, 255), g.clamp(0, 255), b.clamp(0, 255)),
            );
            i += 4;
            continue;
          }
        }
      }
      continue;
    }
    _applySimple(style, value);
  }
}

void _applySimple(_Style style, int code) {
  switch (code) {
    case 0:
      style.reset();
      break;
    case 1:
      style.bold = true;
      break;
    case 2:
      style.dim = true;
      break;
    case 3:
      style.italic = true;
      break;
    case 4:
      style.underline = true;
      break;
    case 7:
      style.inverse = true;
      break;
    case 22:
      style.bold = false;
      style.dim = false;
      break;
    case 23:
      style.italic = false;
      break;
    case 24:
      style.underline = false;
      break;
    case 27:
      style.inverse = false;
      break;
    case 39:
      style.foreground = null;
      break;
    case 49:
      style.background = null;
      break;
    default:
      if (code >= 30 && code <= 37) {
        style.foreground = ansiPalette16[code - 30];
      } else if (code >= 90 && code <= 97) {
        style.foreground = ansiPalette16[code - 90 + 8];
      } else if (code >= 40 && code <= 47) {
        style.background = ansiPalette16[code - 40];
      } else if (code >= 100 && code <= 107) {
        style.background = ansiPalette16[code - 100 + 8];
      }
  }
}

void _setColor(_Style style, bool foreground, Color color) {
  if (foreground) {
    style.foreground = color;
  } else {
    style.background = color;
  }
}
