// 对话正文的轻量 markdown 解析 + 渲染（不引第三方依赖）。
//
// 为什么要有这一层：v1 的对话正文是「转义 + pre-wrap」原样铺开，模型输出的
// `**加粗**`、`| 表格 |`、反引号路径都会连星号 / 竖线一起显示，长回答很难读。
// 2026-11-14 用户要求：①不再出现 `**…**` 这种分隔符；②本机绝对路径渲染成紧凑的
// 小泡泡、点一下直接打开；③表格按表格画；④整体易读。
//
// 只覆盖对话里真实出现的子集：标题 / 段落 / 无序与有序列表 / 引用 / 分隔线 /
// 围栏代码块 / 管道表格 + 行内 `**粗体**`、`*斜体*`、`` `代码` ``、本机路径。
// 不认识的语法一律按普通文本保留（不吞内容）。
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../platform/shell_actions.dart';
import '../theme.dart';

/// 行内元素类型。
enum ChatInlineKind { text, bold, italic, code, path, url }

/// 一段行内内容。[text] 是**去掉标记后**的纯文本。
class ChatInlineToken {
  const ChatInlineToken(this.kind, this.text);

  final ChatInlineKind kind;
  final String text;
}

/// 管道表格。
class ChatTableData {
  const ChatTableData({required this.header, required this.rows});

  final List<String> header;
  final List<List<String>> rows;
}

/// 块级元素类型。
enum ChatBlockKind {
  paragraph,
  heading,
  bullet,
  ordered,
  code,
  table,
  quote,
  divider,
}

/// 一个块。[items] 只给列表用，[table] 只给表格用。
class ChatBlock {
  const ChatBlock({
    required this.kind,
    this.text = '',
    this.level = 0,
    this.items = const [],
    this.table,
  });

  final ChatBlockKind kind;
  final String text;
  final int level;
  final List<String> items;
  final ChatTableData? table;
}

/// 本机绝对路径：盘符 + 反斜杠 / 正斜杠，或 UNC（`\\server\share`）。
///
/// 只认这两种开头，并要求盘符前不是 ASCII 字母数字 —— 否则 `https://…` 里的 `s:/`
/// 会被误判成盘符（`s:` + `/`），整条 URL 变成文件泡泡。
final RegExp _pathPattern = RegExp(
  r'(?<![A-Za-z0-9])(?:[A-Za-z]:[\\/]|\\\\)[^\s<>"|?*]+',
);

/// `http(s)://…` 的 URL（模型常把接口地址直接写进正文，也可能是 `…/chat/completions`）。
final RegExp _urlPattern = RegExp(r'https?://[^\s<>"`]+', caseSensitive: false);

/// 路径末尾常粘到的标点（中英文都要剥掉）。
const String _pathTailPunctuation = '.,;:!?，。；：！？、）)】」』';

String _trimPathTail(String raw) {
  var end = raw.length;
  while (end > 0 && _pathTailPunctuation.contains(raw[end - 1])) {
    end--;
  }
  return raw.substring(0, end);
}

/// 解析行内标记。未闭合的 `**` / `` ` `` 原样当文本（避免吞掉正文）。
List<ChatInlineToken> parseChatInline(String source) {
  final tokens = <ChatInlineToken>[];
  final buffer = StringBuffer();

  void flush() {
    if (buffer.isNotEmpty) {
      tokens.add(ChatInlineToken(ChatInlineKind.text, buffer.toString()));
      buffer.clear();
    }
  }

  var i = 0;
  while (i < source.length) {
    final char = source[i];
    // 行内代码：`code`；整段是一个本机路径时仍按路径泡泡渲染
    // （模型习惯把路径包进反引号，用户要求点一下能直接打开）。
    if (char == '`') {
      final end = source.indexOf('`', i + 1);
      if (end > i + 1) {
        final inner = source.substring(i + 1, end);
        final whole = _pathPattern.matchAsPrefix(inner);
        final isWholePath = whole != null && whole.end == inner.length;
        final wholeUrl = _urlPattern.matchAsPrefix(inner);
        final isWholeUrl = wholeUrl != null && wholeUrl.end == inner.length;
        flush();
        tokens.add(
          isWholePath
              ? ChatInlineToken(ChatInlineKind.path, _trimPathTail(inner))
              : isWholeUrl
              ? ChatInlineToken(ChatInlineKind.url, _trimPathTail(inner))
              : ChatInlineToken(ChatInlineKind.code, inner),
        );
        i = end + 1;
        continue;
      }
    }
    // 加粗：**text** / __text__
    if (i + 1 < source.length &&
        (char == '*' || char == '_') &&
        source[i + 1] == char) {
      final end = source.indexOf('$char$char', i + 2);
      if (end > i + 2) {
        flush();
        tokens.add(
          ChatInlineToken(ChatInlineKind.bold, source.substring(i + 2, end)),
        );
        i = end + 2;
        continue;
      }
    }
    // 斜体：*text* / _text_（两侧不能紧贴空白，避免 `2 * 3` 被当成斜体）
    if (char == '*' || char == '_') {
      final end = source.indexOf(char, i + 1);
      final inner = end > i + 1 ? source.substring(i + 1, end) : '';
      if (end > i + 1 &&
          inner.trim() == inner &&
          !inner.startsWith(' ') &&
          !inner.endsWith(' ')) {
        flush();
        tokens.add(ChatInlineToken(ChatInlineKind.italic, inner));
        i = end + 1;
        continue;
      }
    }
    // URL：优先于本机路径匹配；点击在预览区打开。
    final url = _urlPattern.matchAsPrefix(source, i);
    if (url != null) {
      final raw = _trimPathTail(url[0]!);
      if (raw.length >= 8) {
        flush();
        tokens.add(ChatInlineToken(ChatInlineKind.url, raw));
        i += raw.length;
        continue;
      }
    }
    // 本机路径：整段渲染成可点击的小泡泡
    final match = _pathPattern.matchAsPrefix(source, i);
    if (match != null) {
      final raw = _trimPathTail(match[0]!);
      if (raw.length >= 4) {
        flush();
        tokens.add(ChatInlineToken(ChatInlineKind.path, raw));
        i += raw.length;
        continue;
      }
    }
    buffer.write(char);
    i++;
  }
  flush();
  return tokens;
}

final RegExp _headingPattern = RegExp(r'^(#{1,6})\s+(.*)$');
final RegExp _bulletPattern = RegExp(r'^[-*+]\s+(.*)$');
final RegExp _orderedPattern = RegExp(r'^(\d+)[.)]\s+(.*)$');
final RegExp _dividerPattern = RegExp(r'^(?:-{3,}|\*{3,}|_{3,})$');
final RegExp _tableDividerPattern = RegExp(r'^\|?[\s:|-]+\|[\s:|-]*$');

bool _looksLikeTableDivider(String line) {
  final text = line.trim();
  return text.contains('|') &&
      text.contains('-') &&
      _tableDividerPattern.hasMatch(text);
}

bool _looksLikeTableRow(String line) {
  final text = line.trim();
  return text.startsWith('|') && text.endsWith('|') && text.length > 2;
}

List<String> _tableCells(String line) {
  final text = line.trim();
  final body = text.substring(1, text.length - 1);
  return [for (final cell in body.split('|')) cell.trim()];
}

/// 把整段正文拆成块。空行只用来断段，不产出空块。
List<ChatBlock> parseChatBlocks(String source) {
  final lines = source.replaceAll('\r\n', '\n').split('\n');
  final blocks = <ChatBlock>[];
  var i = 0;

  bool startsBlock(String line) {
    final text = line.trim();
    return text.isEmpty ||
        text.startsWith('```') ||
        _headingPattern.hasMatch(text) ||
        _dividerPattern.hasMatch(text) ||
        text.startsWith('>') ||
        _bulletPattern.hasMatch(text) ||
        _orderedPattern.hasMatch(text) ||
        _looksLikeTableRow(line);
  }

  while (i < lines.length) {
    final line = lines[i];
    final text = line.trim();
    if (text.isEmpty) {
      i++;
      continue;
    }

    // 围栏代码块（语言标记丢弃，只保留内容）
    if (text.startsWith('```')) {
      final body = <String>[];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith('```')) {
        body.add(lines[i]);
        i++;
      }
      if (i < lines.length) {
        i++; // 收尾的 ```
      }
      blocks.add(ChatBlock(kind: ChatBlockKind.code, text: body.join('\n')));
      continue;
    }

    // 分隔线
    if (_dividerPattern.hasMatch(text)) {
      blocks.add(const ChatBlock(kind: ChatBlockKind.divider));
      i++;
      continue;
    }

    // 标题
    final heading = _headingPattern.firstMatch(text);
    if (heading != null) {
      blocks.add(
        ChatBlock(
          kind: ChatBlockKind.heading,
          level: heading.group(1)!.length,
          text: heading.group(2)!.trim(),
        ),
      );
      i++;
      continue;
    }

    // 表格：表头 + 分隔行 + 若干数据行
    if (_looksLikeTableRow(line) &&
        i + 1 < lines.length &&
        _looksLikeTableDivider(lines[i + 1])) {
      final header = _tableCells(line);
      i += 2;
      final rows = <List<String>>[];
      while (i < lines.length && _looksLikeTableRow(lines[i])) {
        final cells = _tableCells(lines[i]);
        // 列数与表头不一致时补齐 / 截断，避免 Table 行长度不等直接断言
        rows.add([
          for (var c = 0; c < header.length; c++)
            c < cells.length ? cells[c] : '',
        ]);
        i++;
      }
      blocks.add(
        ChatBlock(
          kind: ChatBlockKind.table,
          table: ChatTableData(header: header, rows: rows),
        ),
      );
      continue;
    }

    // 引用（连续的 > 行合成一段）
    if (text.startsWith('>')) {
      final body = <String>[];
      while (i < lines.length && lines[i].trim().startsWith('>')) {
        body.add(lines[i].trim().replaceFirst(RegExp(r'^>\s?'), ''));
        i++;
      }
      blocks.add(ChatBlock(kind: ChatBlockKind.quote, text: body.join('\n')));
      continue;
    }

    // 无序列表
    if (_bulletPattern.hasMatch(text)) {
      final items = <String>[];
      while (i < lines.length) {
        final item = _bulletPattern.firstMatch(lines[i].trim());
        if (item == null) {
          break;
        }
        items.add(item.group(1)!.trim());
        i++;
      }
      blocks.add(ChatBlock(kind: ChatBlockKind.bullet, items: items));
      continue;
    }

    // 有序列表
    if (_orderedPattern.hasMatch(text)) {
      final items = <String>[];
      while (i < lines.length) {
        final item = _orderedPattern.firstMatch(lines[i].trim());
        if (item == null) {
          break;
        }
        items.add(item.group(2)!.trim());
        i++;
      }
      blocks.add(ChatBlock(kind: ChatBlockKind.ordered, items: items));
      continue;
    }

    // 普通段落：一直吃到空行或下一个块的开头
    final body = <String>[line.trim()];
    i++;
    while (i < lines.length && !startsBlock(lines[i])) {
      body.add(lines[i].trim());
      i++;
    }
    blocks.add(ChatBlock(kind: ChatBlockKind.paragraph, text: body.join('\n')));
  }
  return blocks;
}

/// 对话正文渲染：解析 + 画出块级结构，行内路径是可点击的小泡泡。
class ChatMarkdownView extends StatefulWidget {
  const ChatMarkdownView({
    super.key,
    required this.text,
    this.baseStyle,
    this.onOpenUrl,
  });

  final String text;
  final TextStyle? baseStyle;

  /// URL 点击：由对话面板转到预览区打开；null = 只显示为普通可选中文本。
  final void Function(String url)? onOpenUrl;

  @override
  State<ChatMarkdownView> createState() => _ChatMarkdownViewState();
}

class _ChatMarkdownViewState extends State<ChatMarkdownView> {
  /// 链接用的手势识别器。每次重建前释放上一批，避免流式输出反复建链接泄漏。
  final List<TapGestureRecognizer> _recognizers = <TapGestureRecognizer>[];

  void _disposeRecognizers() {
    for (final recognizer in _recognizers) {
      recognizer.dispose();
    }
    _recognizers.clear();
  }

  @override
  void dispose() {
    _disposeRecognizers();
    super.dispose();
  }

  TapGestureRecognizer _urlRecognizer(String url) {
    final recognizer = TapGestureRecognizer()
      ..onTap = () => widget.onOpenUrl?.call(url);
    _recognizers.add(recognizer);
    return recognizer;
  }

  @override
  Widget build(BuildContext context) {
    _disposeRecognizers();
    final style =
        widget.baseStyle ??
        TextStyle(color: LunacColors.text, fontSize: 13, height: 1.55);
    final blocks = parseChatBlocks(widget.text);
    if (blocks.isEmpty) {
      return const SizedBox.shrink();
    }
    return SelectionArea(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < blocks.length; i++) ...[
            if (i > 0) const SizedBox(height: 8),
            _blockWidget(blocks[i], style),
          ],
        ],
      ),
    );
  }

  Widget _blockWidget(ChatBlock block, TextStyle base) {
    switch (block.kind) {
      case ChatBlockKind.paragraph:
        return Text.rich(TextSpan(children: _inlineSpans(block.text, base)));
      case ChatBlockKind.heading:
        final size = switch (block.level) {
          1 => 15.5,
          2 => 14.5,
          3 => 13.5,
          _ => base.fontSize ?? 13,
        };
        return Padding(
          padding: EdgeInsets.only(top: block.level <= 2 ? 2 : 0),
          child: Text.rich(
            TextSpan(
              children: _inlineSpans(
                block.text,
                base.copyWith(fontSize: size, fontWeight: FontWeight.w600),
              ),
            ),
          ),
        );
      case ChatBlockKind.bullet:
      case ChatBlockKind.ordered:
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < block.items.length; i++)
              Padding(
                padding: EdgeInsets.only(top: i == 0 ? 0 : 3),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(
                      width: block.kind == ChatBlockKind.ordered ? 20 : 14,
                      child: Text(
                        block.kind == ChatBlockKind.ordered ? '${i + 1}.' : '•',
                        style: base.copyWith(
                          color: LunacColors.textDim,
                          height: 1.55,
                        ),
                      ),
                    ),
                    Expanded(
                      child: Text.rich(
                        TextSpan(children: _inlineSpans(block.items[i], base)),
                      ),
                    ),
                  ],
                ),
              ),
          ],
        );
      case ChatBlockKind.quote:
        return Container(
          padding: const EdgeInsets.only(left: 9),
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(
                color: LunacColors.accent.withValues(alpha: 0.45),
                width: 2,
              ),
            ),
          ),
          child: Text.rich(
            TextSpan(
              children: _inlineSpans(
                block.text,
                base.copyWith(color: LunacColors.textDim),
              ),
            ),
          ),
        );
      case ChatBlockKind.code:
        return Container(
          width: double.infinity,
          padding: const EdgeInsets.all(8),
          decoration: BoxDecoration(
            color: LunacColors.terminal,
            border: Border.all(color: LunacColors.border),
            borderRadius: BorderRadius.circular(6),
          ),
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Text(
              block.text,
              style: base.copyWith(
                fontSize: 11.5,
                height: 1.45,
                fontFamily: 'Consolas',
                fontFamilyFallback: const ['Courier New', 'monospace'],
              ),
            ),
          ),
        );
      case ChatBlockKind.table:
        return _tableView(block.table!, base);
      case ChatBlockKind.divider:
        return Divider(height: 12, color: LunacColors.border);
    }
  }

  Widget _tableView(ChatTableData table, TextStyle base) {
    TableRow row(List<String> cells, {bool header = false}) {
      return TableRow(
        decoration: header
            ? BoxDecoration(color: LunacColors.surfaceHover)
            : null,
        children: [
          for (final cell in cells)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 5),
              child: Text.rich(
                TextSpan(
                  children: _inlineSpans(
                    cell,
                    header ? base.copyWith(fontWeight: FontWeight.w600) : base,
                  ),
                ),
              ),
            ),
        ],
      );
    }

    return ClipRRect(
      borderRadius: BorderRadius.circular(6),
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(6),
        ),
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Table(
            defaultColumnWidth: const IntrinsicColumnWidth(),
            border: TableBorder(
              horizontalInside: BorderSide(color: LunacColors.border),
            ),
            children: [
              row(table.header, header: true),
              for (final line in table.rows) row(line),
            ],
          ),
        ),
      ),
    );
  }

  List<InlineSpan> _inlineSpans(String source, TextStyle base) {
    return [
      for (final token in parseChatInline(source))
        switch (token.kind) {
          ChatInlineKind.text => TextSpan(text: token.text, style: base),
          ChatInlineKind.bold => TextSpan(
            text: token.text,
            style: base.copyWith(fontWeight: FontWeight.w600),
          ),
          ChatInlineKind.italic => TextSpan(
            text: token.text,
            style: base.copyWith(fontStyle: FontStyle.italic),
          ),
          ChatInlineKind.code => TextSpan(
            text: token.text,
            style: base.copyWith(
              fontFamily: 'Consolas',
              fontFamilyFallback: const ['Courier New', 'monospace'],
              backgroundColor: LunacColors.surfaceHover,
            ),
          ),
          ChatInlineKind.url => TextSpan(
            text: token.text,
            style: base.copyWith(
              color: LunacColors.accent,
              decoration: TextDecoration.underline,
              decorationColor: LunacColors.accent,
            ),
            recognizer: widget.onOpenUrl == null
                ? null
                : _urlRecognizer(token.text),
          ),
          ChatInlineKind.path => WidgetSpan(
            alignment: PlaceholderAlignment.middle,
            child: _PathChip(path: token.text),
          ),
        },
    ];
  }
}

/// 纯文本里的 URL 链接化（工具输出 / 命令回显用）。
///
/// 与正文的 markdown 渲染分开：这里不解析粗体 / 列表，只把 `http(s)://...`
/// 变成可点击的 `TextSpan`，并保留 `SelectionArea` 的选中高亮。
class LinkifiedText extends StatefulWidget {
  const LinkifiedText(this.text, {super.key, this.style, this.onOpenUrl});

  final String text;
  final TextStyle? style;
  final void Function(String url)? onOpenUrl;

  @override
  State<LinkifiedText> createState() => _LinkifiedTextState();
}

class _LinkifiedTextState extends State<LinkifiedText> {
  final List<TapGestureRecognizer> _recognizers = <TapGestureRecognizer>[];

  void _disposeRecognizers() {
    for (final recognizer in _recognizers) {
      recognizer.dispose();
    }
    _recognizers.clear();
  }

  @override
  void dispose() {
    _disposeRecognizers();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    _disposeRecognizers();
    final base =
        widget.style ??
        TextStyle(color: LunacColors.text, fontSize: 12, height: 1.45);
    final spans = <InlineSpan>[];
    var cursor = 0;
    for (final match in _urlPattern.allMatches(widget.text)) {
      final raw = _trimPathTail(match[0]!);
      if (raw.length < 8) {
        continue;
      }
      if (match.start > cursor) {
        spans.add(
          TextSpan(
            text: widget.text.substring(cursor, match.start),
            style: base,
          ),
        );
      }
      final recognizer = TapGestureRecognizer()
        ..onTap = () => widget.onOpenUrl?.call(raw);
      _recognizers.add(recognizer);
      spans.add(
        TextSpan(
          text: raw,
          style: base.copyWith(
            color: LunacColors.accent,
            decoration: TextDecoration.underline,
            decorationColor: LunacColors.accent,
          ),
          recognizer: widget.onOpenUrl == null ? null : recognizer,
        ),
      );
      cursor = match.start + raw.length;
    }
    if (cursor < widget.text.length) {
      spans.add(TextSpan(text: widget.text.substring(cursor), style: base));
    }
    return SelectionArea(child: Text.rich(TextSpan(children: spans)));
  }
}

/// 本机路径的小泡泡：紧凑、可点击；点开失败时退回「在资源管理器里定位」。
class _PathChip extends StatelessWidget {
  const _PathChip({required this.path});

  final String path;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: '打开 $path',
      child: InkWell(
        borderRadius: BorderRadius.circular(5),
        onTap: () {
          if (!openPath(path)) {
            revealInExplorer(path);
          }
        },
        child: Container(
          constraints: const BoxConstraints(maxWidth: 300),
          padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
          decoration: BoxDecoration(
            color: LunacColors.surfaceHover,
            border: Border.all(color: LunacColors.border),
            borderRadius: BorderRadius.circular(5),
          ),
          child: Text(
            path,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              height: 1.35,
              fontFamily: 'Consolas',
              fontFamilyFallback: const ['Courier New', 'monospace'],
            ),
          ),
        ),
      ),
    );
  }
}
