// 附件泡泡与文件名工具：主搜索窗口与独立抽屉窗口共用。
//
// 独立抽屉窗口见 `lib/attachment_drawer_main.dart`（第二个 Flutter engine）。
import 'package:flutter/material.dart';

import '../theme.dart';

/// 路径里的文件名（同时兼容 `\` 与 `/`）。
String fileNameOf(String path) {
  final parts = path.split(RegExp(r'[\\/]'));
  return parts.isEmpty ? path : parts.last;
}

/// 附件泡泡：文件名 + 删除按钮；宽度受限，长名省略。
class AttachmentChip extends StatelessWidget {
  const AttachmentChip({
    super.key,
    required this.name,
    required this.onRemove,
  });

  final String name;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    return Container(
      constraints: const BoxConstraints(maxWidth: 220),
      padding: const EdgeInsets.only(left: 8),
      decoration: BoxDecoration(
        color: LunacColors.surfaceHover,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.insert_drive_file_outlined,
            size: 12,
            color: LunacColors.textDim,
          ),
          const SizedBox(width: 6),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 160),
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.text, fontSize: 12),
            ),
          ),
          InkWell(
            onTap: onRemove,
            borderRadius: BorderRadius.circular(4),
            child: Padding(
              padding: EdgeInsets.all(5),
              child: Icon(Icons.close, size: 12, color: LunacColors.textDim),
            ),
          ),
        ],
      ),
    );
  }
}
