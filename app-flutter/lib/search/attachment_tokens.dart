// 附件「胶囊」与正文的关系（§待办 3-28.18）。
//
// 两套表示，各管一段：
//   · **输入框里** = `[[file:<绝对路径>]]` 占位符 —— 用户可以像文字一样插入 / 删除 / 移动，
//     `AttachmentTextEditingController` 把它渲染成可见胶囊；
//   · **发给 agent / 存进 history 的正文** = `[Attached file] "<绝对路径>"` —— 可读的纯文本，
//     位置就是用户在句子里插入的位置，复制到别处也不丢信息。
//
// 这里只放纯函数（可单测）；高亮与气泡渲染在 `chat_panel.dart`。
const String kAttachMarker = '[Attached file]';

final RegExp _tokenPattern = RegExp(r'\[\[file:([^\]]+)\]\]');
final RegExp _expandedPattern = RegExp(r'\[Attached file\]\s*"([^"]*)"');

/// 输入框里的占位符。
String attachToken(String path) => '[[file:$path]]';

/// 正文里实际发出去的片段。
String attachSegment(String path) => '$kAttachMarker "$path"';

/// 从输入框文本里按出现顺序取出附件路径（去重）。
List<String> attachmentPathsIn(String text) {
  final seen = <String>{};
  final out = <String>[];
  for (final match in _tokenPattern.allMatches(text)) {
    final path = match.group(1)!.trim();
    if (path.isEmpty || !seen.add(path)) {
      continue;
    }
    out.add(path);
  }
  return out;
}

/// 占位符 → 可读片段；其余字符原样保留（含换行与标点）。
String expandAttachmentTokens(String text) {
  return text.replaceAllMapped(
    _tokenPattern,
    (match) => attachSegment(match.group(1)!.trim()),
  );
}

/// 正文片段：`path` 非空 = 一个附件胶囊，否则是普通文字。
class AttachmentSegment {
  const AttachmentSegment.text(this.text) : path = null;
  const AttachmentSegment.file(this.path) : text = '';

  final String text;
  final String? path;

  bool get isFile => path != null;
}

/// 把「已展开」的正文拆成文字 / 附件两类片段（给用户气泡渲染胶囊用）。
///
/// 未闭合 / 格式不对的标记不吞掉，原样当文字 —— 用户手打的东西不能凭空消失。
List<AttachmentSegment> splitAttachments(String text) {
  final out = <AttachmentSegment>[];
  var cursor = 0;
  for (final match in _expandedPattern.allMatches(text)) {
    if (match.start > cursor) {
      out.add(AttachmentSegment.text(text.substring(cursor, match.start)));
    }
    final path = match.group(1)!.trim();
    out.add(path.isEmpty ? AttachmentSegment.text(match.group(0)!) : AttachmentSegment.file(path));
    cursor = match.end;
  }
  if (cursor < text.length) {
    out.add(AttachmentSegment.text(text.substring(cursor)));
  }
  return out;
}

/// 文件名（气泡 / 胶囊显示用）。
String attachmentName(String path) {
  final parts = path.split(RegExp(r'[\\/]')).where((item) => item.isNotEmpty);
  return parts.isEmpty ? path : parts.last;
}
