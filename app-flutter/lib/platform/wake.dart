// 平台薄层：窗口「唤出」的界面副作用（对应 v1 的 `lunac-window-shown` 处理）。
//
// v1 在收到 `lunac-window-shown` 时会做三件事：
//   1. 剪贴板是文件 / 图片时加进附件泡泡；
//   2. 剪贴板是文本时填进搜索栏；
//   3. 重跑当前查询（`refresh_if_stale` + 重新搜索），保证结果是最新的。
// v2 由 `main.dart` 的唤出处理写入下面三个通知器，`SearchPage` 消费：
//   · [pendingAttachments]：待加入附件栏的路径（文件 / 剪贴板图片落盘的 BMP）；
//   · [pendingSearchQuery]：只在搜索框为空时填入；
//   · [searchRefreshTick]：每次唤出 +1，界面据此重跑当前查询。
import 'package:flutter/foundation.dart';

/// 待加入附件栏的剪贴板路径（文件 / 图片）。界面消费后重置为空列表。
final ValueNotifier<List<String>> pendingAttachments =
    ValueNotifier<List<String>>(const []);

/// 待填入搜索框的剪贴板文本。界面消费后重置为空串。
final ValueNotifier<String> pendingSearchQuery = ValueNotifier<String>('');

/// 唤出计数：每 +1 表示「窗口被唤出一次」，界面应重跑当前查询。
final ValueNotifier<int> searchRefreshTick = ValueNotifier<int>(0);
