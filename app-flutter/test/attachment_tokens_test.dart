import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/attachment_tokens.dart';

void main() {
  test('tokens expand to readable segments at their original position', () {
    final text = '看这个 ${attachToken(r'D:\a\报告.pdf')} 和这个 '
        '${attachToken(r'D:\b\图.png')}，谢谢';
    final body = expandAttachmentTokens(text);
    expect(body.contains('[[file:'), isFalse);
    expect(
      body,
      '看这个 [Attached file] "D:\\a\\报告.pdf" 和这个 '
      '[Attached file] "D:\\b\\图.png"，谢谢',
    );
    final first = body.indexOf('报告.pdf');
    final second = body.indexOf('图.png');
    expect(first < second, isTrue, reason: '顺序必须按用户插入的位置');
  });

  test('attachment paths are read once and in order', () {
    final text = '${attachToken('a.mp3')} ${attachToken('b.mp3')} '
        '${attachToken('a.mp3')}';
    expect(attachmentPathsIn(text), ['a.mp3', 'b.mp3']);
  });

  test('splitAttachments keeps plain text and unclosed markers untouched', () {
    final segments = splitAttachments(
      '前 [Attached file] "D:\\x\\a.pdf" 后 [Attached file] 手打残句',
    );
    expect(segments.map((s) => s.isFile).toList(), [false, true, false]);
    expect(segments[1].path, r'D:\x\a.pdf');
    expect(segments[2].text, contains('[Attached file] 手打残句'));
  });

  test('attachment name falls back to the whole path', () {
    expect(attachmentName(r'D:\Music\a.flac'), 'a.flac');
    expect(attachmentName('a.flac'), 'a.flac');
  });
}
