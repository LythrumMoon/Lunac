// 平台薄层：文件转换（v1 `convert` 插件的原生等价物）。
//
// 边界：格式表 / 判族 / 目标列表 / ffmpeg 与 LibreOffice 参数 / 子进程等待都在
// `rust/src/convert.rs`；本文件只做两个 C ABI（plan / run）与容错解析：
//   · `lunac_convert_plan` → `kind` / `targets[]` / `error` / `engine` / `engineFound`；
//   · `lunac_convert_run` → `ok` / `outputPath` / `error`。
//
// 纪律：坏 JSON / 桥接不可用 → 一句人话，不抛异常、不崩面板。
// 转换是分钟级的阻塞调用，`runConvert` 放进 `Isolate.run`，UI 只显示「转换中…」。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一份转换计划（`lunac_convert_plan` 的 JSON）。
class ConvertPlan {
  const ConvertPlan({
    required this.kind,
    required this.targets,
    required this.error,
    required this.engine,
    required this.engineFound,
  });

  /// `image` / `audio` / `video` / `pdf` / `unknown`。
  final String kind;

  /// 该族可选目标格式（Rust 侧已去掉源扩展名本身）。
  final List<String> targets;

  /// 不支持的后缀等不可转换原因；能转时为空串。
  final String error;

  /// 该族要用的引擎：`ffmpeg` / `libreoffice`（unknown 时为空）。
  final String engine;

  /// 引擎是否已找到（false 时面板禁用按钮并显示装法）。
  final bool engineFound;

  /// 现在点「开始转换」有没有意义。
  bool get canConvert => error.isEmpty && targets.isNotEmpty && engineFound;

  static ConvertPlan? fromJson(Map<String, dynamic> json) {
    final kind = json['kind'];
    if (kind is! String) {
      return null;
    }
    return ConvertPlan(
      kind: kind,
      targets: [
        for (final target in (json['targets'] as List? ?? const []))
          if (target is String) target,
      ],
      error: json['error'] is String ? json['error'] as String : '',
      engine: json['engine'] is String ? json['engine'] as String : '',
      engineFound: json['engineFound'] == true,
    );
  }
}

/// 一次转换的结果：要么输出路径，要么一句可显示的错误。
class ConvertRunReply {
  const ConvertRunReply.ok(String this.outputPath) : error = null;

  const ConvertRunReply.fail(String this.error) : outputPath = null;

  final String? outputPath;
  final String? error;

  bool get isError => error != null;
}

/// 解析计划 JSON（纯函数，单测不加载 DLL）。
ConvertPlan? decodeConvertPlan(String raw) {
  Object? value;
  try {
    value = jsonDecode(raw);
  } catch (_) {
    return null;
  }
  if (value is! Map<String, dynamic>) {
    return null;
  }
  return ConvertPlan.fromJson(value);
}

/// 解析转换结果 JSON（纯函数，单测不加载 DLL）。
ConvertRunReply decodeConvertRunReply(String raw) {
  Object? value;
  try {
    value = jsonDecode(raw);
  } catch (_) {
    return const ConvertRunReply.fail('转换结果不是合法 JSON');
  }
  if (value is! Map<String, dynamic>) {
    return const ConvertRunReply.fail('转换结果格式不对');
  }
  final output = value['outputPath'];
  if (value['ok'] == true) {
    if (output is! String || output.isEmpty) {
      return const ConvertRunReply.fail('转换完成但没拿到输出路径');
    }
    return ConvertRunReply.ok(output);
  }
  final error = value['error'];
  return ConvertRunReply.fail(
    error is String && error.trim().isNotEmpty ? error.trim() : '转换失败',
  );
}

/// 当前转换进度（`lunac_convert_progress` 的 JSON）。
///
/// `percent < 0` = 不知道总时长（ffprobe 不在 / 读不到）⇒ 面板显示不确定态。
class ConvertProgress {
  const ConvertProgress({
    required this.running,
    required this.percent,
    required this.totalMs,
    required this.cancelled,
  });

  final bool running;
  final double percent;
  final int totalMs;
  final bool cancelled;

  /// 能不能画确定态进度条。
  bool get determinate => running && percent >= 0;

  /// 解析进度 JSON（纯函数，单测不加载 DLL）。
  static ConvertProgress? decode(String raw) {
    Object? value;
    try {
      value = jsonDecode(raw);
    } catch (_) {
      return null;
    }
    if (value is! Map<String, dynamic>) {
      return null;
    }
    final percent = value['percent'];
    return ConvertProgress(
      running: value['running'] == true,
      percent: percent is num ? percent.toDouble() : -1,
      totalMs: value['totalMs'] is int ? value['totalMs'] as int : 0,
      cancelled: value['cancelled'] == true,
    );
  }
}

/// 读当前转换进度（只读内存状态，不阻塞）。
ConvertProgress? readConvertProgress() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_String0C, _String0Dart>(
      'lunac_convert_progress',
    )();
    if (raw == nullptr) {
      return null;
    }
    return ConvertProgress.decode(raw.toDartString());
  } catch (_) {
    return null;
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
  }
}

/// 取消当前转换（按 pid 杀 ffmpeg 进程树 + 删半成品）。
///
/// PDF（LibreOffice）不在这里杀 —— 那是一次性进程，避免误杀用户自己开的 LibreOffice；
/// 此时后端是 no-op，UI 用 [ConvertProgress.running] 判断按钮该不该出现。
Future<ConvertRunReply> cancelConvert() async {
  try {
    final raw = await Isolate.run(_callCancel);
    if (raw == null) {
      return const ConvertRunReply.fail('不可用（rust 桥接未加载）');
    }
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return const ConvertRunReply.fail('取消结果格式不对');
    }
    final error = value['error'];
    return value['ok'] == true
        ? const ConvertRunReply.ok('')
        : ConvertRunReply.fail(
            error is String && error.trim().isNotEmpty ? error.trim() : '取消失败',
          );
  } catch (e) {
    return ConvertRunReply.fail('取消失败：$e');
  }
}

String? _callCancel() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_String0C, _String0Dart>(
      'lunac_convert_cancel',
    )();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
  }
}

typedef _PlanC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _PlanDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _RunC = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _RunDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读取一个文件的转换计划；桥接缺失 / 坏 JSON 返回 null（面板显示不可用）。
ConvertPlan? loadConvertPlan(String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? pathPtr;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_PlanC, _PlanDart>(
      'lunac_convert_plan',
    );
    pathPtr = path.toNativeUtf8();
    raw = call(pathPtr);
    if (raw == nullptr) {
      return null;
    }
    return decodeConvertPlan(raw.toDartString());
  } catch (_) {
    return null;
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
    if (pathPtr != null) {
      malloc.free(pathPtr);
    }
  }
}

/// 执行一次转换（阻塞到子进程结束；[outDir] 为空 = 源文件同目录）。
Future<ConvertRunReply> runConvert(
  String src,
  String target, {
  String outDir = '',
}) async {
  try {
    // 分钟级阻塞：放进独立 isolate，避免转换期间 UI 冻住。
    final raw = await Isolate.run(() => _callRun(src, target, outDir));
    return decodeConvertRunReply(raw);
  } catch (e) {
    return ConvertRunReply.fail('转换失败：$e');
  }
}

String _callRun(String src, String target, String outDir) {
  final library = openLunacNative().library;
  if (library == null) {
    return jsonEncode({'ok': false, 'error': '转换不可用（rust 桥接未加载）'});
  }
  Pointer<Utf8>? srcPtr;
  Pointer<Utf8>? targetPtr;
  Pointer<Utf8>? outDirPtr;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_RunC, _RunDart>('lunac_convert_run');
    srcPtr = src.toNativeUtf8();
    targetPtr = target.toNativeUtf8();
    outDirPtr = outDir.toNativeUtf8();
    raw = call(srcPtr, targetPtr, outDirPtr);
    if (raw == nullptr) {
      return jsonEncode({'ok': false, 'error': '转换没有返回内容'});
    }
    return raw.toDartString();
  } catch (e) {
    return jsonEncode({'ok': false, 'error': '转换失败：$e'});
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
    if (srcPtr != null) {
      malloc.free(srcPtr);
    }
    if (targetPtr != null) {
      malloc.free(targetPtr);
    }
    if (outDirPtr != null) {
      malloc.free(outDirPtr);
    }
  }
}

typedef _String0C = Pointer<Utf8> Function();
typedef _String0Dart = Pointer<Utf8> Function();

/// 按页签弹多选文件框（v1 转换窗口的输入框点击）。
///
/// [kind] = `pdf` / `video` / `extract` / `audio`。取消 / 桥接缺失返回空列表。
List<String> pickConvertFiles(String kind) {
  final library = openLunacNative().library;
  if (library == null) {
    return const [];
  }
  Pointer<Utf8>? kindPtr;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_PlanC, _PlanDart>(
      'lunac_convert_pick_files',
    );
    kindPtr = kind.toNativeUtf8();
    raw = call(kindPtr);
    if (raw == nullptr) {
      return const [];
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! List) {
      return const [];
    }
    return [
      for (final item in value)
        if (item is String && item.isNotEmpty) item,
    ];
  } catch (_) {
    return const [];
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
    if (kindPtr != null) {
      malloc.free(kindPtr);
    }
  }
}

/// 弹系统的「选择文件夹」对话框；取消 / 桥接缺失返回空串。
String pickConvertOutDir() {
  final library = openLunacNative().library;
  if (library == null) {
    return '';
  }
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_String0C, _String0Dart>(
      'lunac_convert_pick_out_dir',
    );
    raw = call();
    if (raw == nullptr) {
      return '';
    }
    return raw.toDartString().trim();
  } catch (_) {
    return '';
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
  }
}
