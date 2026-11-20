// 详细搜索预览区要的文件元信息（纯 Dart + dart:io，不碰 FFI）。
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/file_preview.dart';

void main() {
  test('formatBytes keeps one decimal and switches units', () {
    expect(formatBytes(0), '0 B');
    expect(formatBytes(1023), '1023 B');
    expect(formatBytes(1024), '1.0 KB');
    expect(formatBytes(1536), '1.5 KB');
    expect(formatBytes(1024 * 1024 * 3 ~/ 2), '1.5 MB');
    expect(formatBytes(1024 * 1024 * 1024 * 200), '200 GB');
  });

  test('formatModified pads to yyyy-MM-dd HH:mm', () {
    expect(formatModified(DateTime(2026, 1, 2, 3, 4)), '2026-01-02 03:04');
    expect(formatModified(null), '—');
  });

  test('readFilePreview reads a real temp file', () {
    final dir = Directory.systemTemp.createTempSync('lunac-preview-');
    addTearDown(() {
      if (dir.existsSync()) {
        dir.deleteSync(recursive: true);
      }
    });
    final file = File('${dir.path}${Platform.pathSeparator}shot.PNG')
      ..writeAsBytesSync(List.filled(2048, 0));

    final info = readFilePreview(file.path);
    expect(info, isNotNull);
    expect(info!.name, 'shot.PNG');
    expect(info.isDirectory, isFalse);
    expect(info.sizeBytes, 2048);
    expect(info.isImage, isTrue);
    expect(info.kindLabel, 'PNG 图片');
    expect(info.modified, isNotNull);
  });

  test('directories report folder label and zero size', () {
    final dir = Directory.systemTemp.createTempSync('lunac-preview-dir-');
    addTearDown(() {
      if (dir.existsSync()) {
        dir.deleteSync(recursive: true);
      }
    });
    final info = readFilePreview(dir.path);
    expect(info, isNotNull);
    expect(info!.isDirectory, isTrue);
    expect(info.sizeBytes, 0);
    expect(info.kindLabel, '文件夹');
    expect(info.isImage, isFalse);
  });

  test('missing path returns null instead of throwing', () {
    expect(readFilePreview(''), isNull);
    expect(readFilePreview('Z:\\definitely\\missing\\file.txt'), isNull);
  });
}
