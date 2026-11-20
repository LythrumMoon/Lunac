// ANSI 解析单测（v1 由 xterm.js 解释；这里锁住 v2 的等价行为）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/ansi.dart';

String _text(List<AnsiSpan> spans) => spans.map((span) => span.text).join();

void main() {
  test('plain text stays one span with the v1 terminal foreground', () {
    final spans = parseAnsi('hello world');
    expect(_text(spans), 'hello world');
    expect(spans.single.foreground, ansiDefaultForeground);
    expect(spans.single.bold, isFalse);
  });

  test('SGR colours, reset, bold and underline follow xterm defaults', () {
    final spans = parseAnsi('\x1b[31mred\x1b[0m plain \x1b[1;4mstyled\x1b[m');
    expect(_text(spans), 'red plain styled');
    expect(spans[0].text, 'red');
    expect(spans[0].foreground, const Color(0xFFCD0000));
    expect(spans[1].text, ' plain ');
    expect(spans[1].foreground, ansiDefaultForeground);
    expect(spans[2].text, 'styled');
    expect(spans[2].bold, isTrue);
    expect(spans[2].underline, isTrue);
  });

  test('bright colours, background, inverse and dim are supported', () {
    final bright = parseAnsi('\x1b[91mx\x1b[0m');
    expect(bright.single.foreground, const Color(0xFFFF0000));
    final background = parseAnsi('\x1b[42mx\x1b[0m');
    expect(background.single.background, const Color(0xFF00CD00));
    final inverse = parseAnsi('\x1b[7mx\x1b[0m');
    expect(inverse.single.foreground, ansiDefaultBackground);
    expect(inverse.single.background, ansiDefaultForeground);
    final dim = parseAnsi('\x1b[2mx\x1b[0m');
    expect(dim.single.dim, isTrue);
    expect(dim.single.toTextStyle().color!.a, lessThan(1.0));
  });

  test('256 colour cube and truecolour match the standard palettes', () {
    expect(ansi256(196), const Color(0xFFFF0000));
    expect(ansi256(16), const Color(0xFF000000));
    expect(ansi256(231), const Color(0xFFFFFFFF));
    expect(ansi256(232), const Color(0xFF080808));
    expect(parseAnsi('\x1b[38;5;196mx').single.foreground, const Color(0xFFFF0000));
    expect(
      parseAnsi('\x1b[38;2;1;2;3mx').single.foreground,
      const Color(0xFF010203),
    );
    expect(
      parseAnsi('\x1b[48;2;10;20;30mx').single.background,
      const Color(0xFF0A141E),
    );
  });

  test('non-SGR escapes are dropped but keep the text readable', () {
    final spans = parseAnsi('\x1b[2K\x1b[1;1Hok\x1b]0;title\x07!');
    expect(_text(spans), 'ok!');
    expect(stripAnsi('\x1b[31mred\x1b[0m'), 'red');
  });

  test('carriage returns overwrite the line and backspace steps back', () {
    expect(_text(parseAnsi('10%\r100%')), '100%');
    expect(_text(parseAnsi('abc\b\bXY')), 'aXY');
    expect(_text(parseAnsi('a\nb\n')), 'a\nb');
  });

  test('CSI erase sequences match xterm (K / J)', () {
    // 整行擦除
    expect(_text(parseAnsi('abc\x1b[2K')), '');
    // 回车后擦到行尾
    expect(_text(parseAnsi('abcdef\r\x1b[K')), '');
    // 光标退 3 格后擦到行尾
    expect(_text(parseAnsi('abcdef\x1b[3D\x1b[K')), 'abc');
    // 上行 + 整行擦除 + 重写（进度输出常见形态）
    expect(_text(parseAnsi('line1\nline2\x1b[1A\x1b[2K\rnew')), 'new\nline2');
    // 清屏
    expect(_text(parseAnsi('a\nb\x1b[2J')), '');
    expect(_text(parseAnsi('one\ntwo\x1b[2K')), 'one');
  });

  test('CSI cursor moves and column set follow xterm', () {
    // 光标回第 1 列后覆盖写
    expect(_text(parseAnsi('%50\x1b[1G100%')), '100%');
    // 光标定位会补空格
    expect(_text(parseAnsi('abc\x1b[1;5HX')), 'abc X');
    // 未知 CSI（隐藏光标的私有模式）被丢掉，不污染文本
    expect(_text(parseAnsi('a\x1b[?25lb')), 'ab');
  });

  test('DECSTBM scrolls only inside the region', () {
    // 五行历史 → 设区域为第 2~4 行（光标被移到区域左上）→ 定位到区域底行换行写 X。
    // 区域外的第 1 行与第 5 行必须原地不动，区域顶行（原来的 2）被滚掉。
    final text = _text(parseAnsi('1\n2\n3\n4\n5\x1b[2;4r\x1b[4;1H\nX'));
    expect(text, '1\n3\n4\nX\n5');
    // `CSI r`（无参）= 复位：定位到末行后换行回到「追加新行」，不再滚动。
    final reset = _text(parseAnsi('a\nb\x1b[1;2r\x1b[r\x1b[2;1H\nc'));
    expect(reset, 'a\nb\nc');
  });

  test('illegal scroll regions are ignored like xterm', () {
    // bottom ≤ top（第 4 行到第 2 行）不是合法区域 ⇒ 整条忽略：区域不变、光标也不动。
    final text = _text(parseAnsi('a\nb\x1b[4;2rZ'));
    expect(text, 'a\nbZ');
  });

  test('alternate screen keeps the main output and drops the alt content', () {
    // 全屏程序（top / vim）的典型形态：进备用屏画画，退出后主屏原样回来。
    final text = _text(
      parseAnsi('main line\x1b[?1049h\x1b[2J\x1b[Hredraw\x1b[?1049l!'),
    );
    expect(text, 'main line!');
    // 只在备用屏里写、且不退出 ⇒ 看到的就是备用屏内容。
    expect(_text(parseAnsi('main\x1b[?47halt')), 'alt');
    // 没进过备用屏就退：无副作用。
    expect(_text(parseAnsi('plain\x1b[?1049l')), 'plain');
  });
}
