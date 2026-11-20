// 平台薄层：路径的小工具（扩展名 / 图片判定）。
//
// 谁在用：
//   · `platform/agent.dart` — 附件里的图片要追加 `image` 块；
//   · `platform/file_preview.dart` — 详细搜索预览区决定画缩略图还是类型图标。
//
// 扩展名集合与 Rust `clipboard::IMAGE_EXTS` 同一组（v1 的口径），改一处要两边一起改。
const Set<String> imageExtensions = {
  'png',
  'jpg',
  'jpeg',
  'bmp',
  'tiff',
  'webp',
  'gif',
  'ico',
};

/// 小写扩展名（不含点）；没有扩展名时返回空串。
String extensionOf(String path) {
  final dot = path.lastIndexOf('.');
  final slash = path.lastIndexOf(RegExp(r'[\\/]'));
  if (dot <= slash || dot < 0 || dot == path.length - 1) {
    return '';
  }
  return path.substring(dot + 1).toLowerCase();
}

/// 扩展名是否属于「可交给模型看 / 可以画缩略图」的图片。
bool isImagePath(String path) => imageExtensions.contains(extensionOf(path));
