// 平台薄层：备忘录（v1 `memo` 插件的原生等价物）。
//
// 边界：落盘 / 校验 / 标识检索都在 `rust/src/memo.rs`；本文件只负责数据形状与四个 C ABI。
// 存储：`<exe 根>/ModuleData/memo/memo.json`，字段与 v1 一致（`id` / `text` / `tag` /
// `images` / `ts`）。v2 面板展示并能逐张移除已有图片；粘贴图片走
// `addMemoImageFromClipboard`（图片落 `ModuleData/memo/images/<id>/`，24 位 BMP）。
// 拖放新增图片仍未接（要先让主窗的 drop 通道分派给面板）。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条备忘录。
class MemoEntry {
  const MemoEntry({
    required this.id,
    required this.text,
    required this.tag,
    required this.images,
    required this.ts,
  });

  final String id;
  final String text;

  /// 用户自定义的检索标识（可空；搜索时精确 / 前缀匹配）。
  final String tag;

  /// 条目附带图片的绝对路径（v2 面板按路径显示缩略图，可逐张移除）。
  final List<String> images;

  /// 最后保存时间（毫秒时间戳）。
  final int ts;

  Map<String, dynamic> toJson() => {
        'id': id,
        'text': text,
        'tag': tag,
        'images': images,
        'ts': ts,
      };

  static MemoEntry? fromJson(Map<String, dynamic> json) {
    final id = json['id'];
    final text = json['text'];
    if (id is! String || id.isEmpty || text is! String) {
      return null;
    }
    return MemoEntry(
      id: id,
      text: text,
      tag: json['tag'] is String ? json['tag'] as String : '',
      images: [
        for (final path in (json['images'] as List? ?? const []))
          if (path is String) path,
      ],
      ts: json['ts'] is int ? json['ts'] as int : 0,
    );
  }
}

typedef _ListC = Pointer<Utf8> Function();
typedef _ListDart = Pointer<Utf8> Function();
typedef _TextI32C = Int32 Function(Pointer<Utf8>);
typedef _TextI32Dart = int Function(Pointer<Utf8>);
typedef _TextStringC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextStringDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _I32C = Int32 Function();
typedef _I32Dart = int Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读全部备忘录；桥接缺失返回 null（面板显示「不可用」）。
List<MemoEntry>? readMemos() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_ListC, _ListDart>('lunac_memo_list')();
    if (raw == nullptr) {
      return null;
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return null;
    }
    return [
      for (final item in value)
        if (item is Map<String, dynamic>) ?MemoEntry.fromJson(item),
    ];
  } catch (_) {
    return null;
  } finally {
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 新增 / 覆盖一条（按 `id`）。返回是否已保存。
bool upsertMemo(MemoEntry entry) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final upsert = library.lookupFunction<_TextI32C, _TextI32Dart>(
      'lunac_memo_upsert',
    );
    pointer = jsonEncode(entry.toJson()).toNativeUtf8();
    return upsert(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 删除一条；不存在也算成功。
bool deleteMemo(String id) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  Pointer<Utf8>? pointer;
  try {
    final delete = library.lookupFunction<_TextI32C, _TextI32Dart>(
      'lunac_memo_delete',
    );
    pointer = id.toNativeUtf8();
    return delete(pointer) == 0;
  } catch (_) {
    return false;
  } finally {
    if (pointer != null) {
      malloc.free(pointer);
    }
  }
}

/// 清空全部（调用方负责二次确认）。
bool clearMemos() {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  try {
    final clear = library.lookupFunction<_I32C, _I32Dart>('lunac_memo_clear');
    return clear() == 0;
  } catch (_) {
    return false;
  }
}

/// 粘贴图片的结果（成功给出落盘路径）。
class MemoImagePaste {
  const MemoImagePaste({this.ok = false, this.path = '', this.error = ''});

  final bool ok;
  final String path;
  final String error;

  static MemoImagePaste? fromJson(Object? value) {
    if (value is! Map) {
      return null;
    }
    return MemoImagePaste(
      ok: value['ok'] == true,
      path: value['path'] is String ? value['path'] as String : '',
      error: value['error'] is String ? value['error'] as String : '',
    );
  }
}

/// 把系统剪贴板里的图片存进 [memoId] 的图片目录，返回落盘路径。
///
/// 只落图片文件；把路径追加进条目的 `images` 并写盘由面板负责（与 v1 同口径：
/// 先保存成备忘录，再粘贴图片）。桥接缺失 / 剪贴板没有图片时返回带原因的结果。
MemoImagePaste pasteMemoImage(String memoId) {
  final library = openLunacNative().library;
  if (library == null) {
    return const MemoImagePaste(error: '当前环境不支持粘贴图片');
  }
  Pointer<Utf8>? argument;
  Pointer<Utf8>? raw;
  try {
    final paste = library.lookupFunction<_TextStringC, _TextStringDart>(
      'lunac_memo_add_clipboard_image',
    );
    argument = memoId.toNativeUtf8();
    raw = paste(argument);
    if (raw == nullptr) {
      return const MemoImagePaste(error: '粘贴图片失败');
    }
    return MemoImagePaste.fromJson(jsonDecode(raw.toDartString())) ??
        const MemoImagePaste(error: '粘贴图片失败');
  } catch (_) {
    return const MemoImagePaste(error: '粘贴图片失败');
  } finally {
    if (argument != null) {
      malloc.free(argument);
    }
    final pointer = raw;
    if (pointer != null && pointer != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}
