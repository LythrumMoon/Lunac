// 平台薄层：详细搜索预览区要的文件元信息（v1 `detail-preview` 的「类型 / 大小 /
// 修改时间 / 路径」四行）。
//
// 只做只读的 `stat`，不解码、不读内容：图片缩略图交给 Flutter 的 `Image.file`
// （`cacheWidth` 缩到预览尺寸），非图片用 Rust 抽的 32×32 类型图标。
// 路径不存在 / 无权限时返回 null，界面画「文件不可用」。
import 'dart:io';

import 'paths.dart';

/// 一条路径的只读元信息。
class FilePreviewInfo {
  const FilePreviewInfo({
    required this.path,
    required this.name,
    required this.isDirectory,
    required this.sizeBytes,
    required this.modified,
    required this.isImage,
  });

  final String path;
  final String name;
  final bool isDirectory;

  /// 目录恒为 0（不递归统计）。
  final int sizeBytes;
  final DateTime? modified;

  /// 扩展名属于图片集合且不是目录（界面据此画缩略图）。
  final bool isImage;

  /// 预览区「类型」一行。
  String get kindLabel {
    if (isDirectory) {
      return '文件夹';
    }
    final extension = extensionOf(path).toUpperCase();
    if (isImage) {
      return extension.isEmpty ? '图片' : '$extension 图片';
    }
    return extension.isEmpty ? '文件' : '$extension 文件';
  }
}

/// 读一条路径的元信息；不存在 / 出错返回 null。
FilePreviewInfo? readFilePreview(String path) {
  if (path.trim().isEmpty) {
    return null;
  }
  try {
    final stat = FileStat.statSync(path);
    if (stat.type == FileSystemEntityType.notFound) {
      return null;
    }
    final isDirectory = stat.type == FileSystemEntityType.directory;
    return FilePreviewInfo(
      path: path,
      name: fileNameOfPath(path),
      isDirectory: isDirectory,
      sizeBytes: isDirectory ? 0 : stat.size,
      modified: stat.modified,
      isImage: !isDirectory && isImagePath(path),
    );
  } catch (_) {
    return null;
  }
}

/// 路径的最后一段（文件名 / 目录名）。
String fileNameOfPath(String path) {
  final parts = path.split(RegExp(r'[\\/]'));
  return parts.isEmpty || parts.last.isEmpty ? path : parts.last;
}

/// 字节数 → 人话（预览区用；只保留 1 位小数）。
String formatBytes(int bytes) {
  if (bytes < 0) {
    return '—';
  }
  if (bytes < 1024) {
    return '$bytes B';
  }
  const units = ['KB', 'MB', 'GB', 'TB'];
  var value = bytes / 1024;
  var unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  final text = value >= 100 ? value.toStringAsFixed(0) : value.toStringAsFixed(1);
  return '$text ${units[unit]}';
}

/// 修改时间 → `yyyy-MM-dd HH:mm`；缺值给 `—`。
String formatModified(DateTime? time) {
  if (time == null) {
    return '—';
  }
  String two(int value) => value.toString().padLeft(2, '0');
  return '${time.year}-${two(time.month)}-${two(time.day)} '
      '${two(time.hour)}:${two(time.minute)}';
}
