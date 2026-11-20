// 平台薄层：OCR（v1 `ocr` 插件的原生等价物）。
//
// 边界：引擎定位 / 子进程 / stdout JSON 解析都在 `rust/src/ocr.rs`；本文件只负责
// 两个 C ABI（`lunac_ocr_status` / `lunac_ocr_image`）与容错解析。
//
// 纪律：
//   · 坏 JSON / 形状不对 / 旧版库缺符号 → 明确状态或一句人话错误，**不抛异常**；
//   · 识别是阻塞子进程调用（首次加载模型要几秒），放进 `Isolate.run`，别冻住 UI 帧；
//   · 返回字符串由 Rust 分配，读完统一用 `lunac_free_string` 释放。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 引擎状态（`lunac_ocr_status` 的 JSON）。
class OcrStatus {
  const OcrStatus({
    required this.installed,
    required this.enginePath,
    required this.searchPaths,
    this.error,
  });

  /// 引擎是否就绪（找到了 PaddleOCR-json.exe 且默认中文模型配置存在）。
  final bool installed;

  /// 定位到的引擎目录；未安装时为空串。
  final String enginePath;

  /// 搜索过的路径（面板未安装时展示给用户，告诉他该往哪放）。
  final List<String> searchPaths;

  /// 解析失败时的一句人话（正常时 null）。
  final String? error;
}

/// 解析 Rust 返回的状态 JSON（纯函数，单测不加载 DLL）。
///
/// 坏 JSON / 缺字段都收敛成 `installed: false` + 一句原因，绝不抛异常。
OcrStatus parseOcrStatus(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const OcrStatus(
      installed: false,
      enginePath: '',
      searchPaths: [],
      error: '引擎状态不是合法 JSON',
    );
  }
  if (decoded is! Map) {
    return const OcrStatus(
      installed: false,
      enginePath: '',
      searchPaths: [],
      error: '引擎状态格式不对',
    );
  }
  final installed = decoded['installed'];
  if (installed is! bool) {
    return const OcrStatus(
      installed: false,
      enginePath: '',
      searchPaths: [],
      error: '引擎状态缺少 installed 字段',
    );
  }
  final enginePath = decoded['enginePath'];
  final searchPaths = decoded['searchPaths'];
  return OcrStatus(
    installed: installed,
    enginePath: enginePath is String ? enginePath : '',
    searchPaths: [
      for (final path in (searchPaths is List ? searchPaths : const []))
        if (path is String && path.isNotEmpty) path,
    ],
  );
}

/// 一张图的识别结果。
class OcrResult {
  const OcrResult({required this.text, required this.lines});

  /// 按行合并的全文（复制用）。
  final String text;

  /// 逐行文本（面板按行展示；空 = 图里没有文字）。
  final List<String> lines;
}

/// 平台层返回：要么一份结果，要么一句可显示的错误。
class OcrReply {
  const OcrReply.ok(OcrResult this.result) : error = null;

  const OcrReply.fail(String this.error) : result = null;

  final OcrResult? result;
  final String? error;

  bool get isError => error != null;
}

/// 解析 Rust 返回的识别 JSON（纯函数，单测不加载 DLL）。
///
/// `error` 非空优先；坏 JSON / 形状不对都给明确错误。
OcrReply decodeOcrReply(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const OcrReply.fail('识别结果不是合法 JSON');
  }
  if (decoded is! Map) {
    return const OcrReply.fail('识别结果格式不对');
  }
  final error = decoded['error'];
  if (error is String && error.trim().isNotEmpty) {
    return OcrReply.fail(error.trim());
  }
  final text = decoded['text'];
  final rawLines = decoded['lines'];
  if (text is! String && rawLines is! List) {
    return const OcrReply.fail('识别结果缺少必要字段');
  }
  final lines = <String>[
    for (final line in (rawLines is List ? rawLines : const []))
      if (line is String) line,
  ];
  return OcrReply.ok(
    OcrResult(text: text is String ? text : lines.join('\n'), lines: lines),
  );
}

typedef _StatusC = Pointer<Utf8> Function();
typedef _StatusDart = Pointer<Utf8> Function();
typedef _ImageC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _ImageDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _CaptureC = Pointer<Utf8> Function(Uint64);
typedef _CaptureDart = Pointer<Utf8> Function(int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 读引擎状态；桥接不可用 / 旧版库缺符号时返回 null（面板显示「不可用」）。
OcrStatus? readOcrStatus() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    final status = library.lookupFunction<_StatusC, _StatusDart>(
      'lunac_ocr_status',
    );
    raw = status();
    if (raw == nullptr) {
      return null;
    }
    return parseOcrStatus(raw.toDartString());
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

/// 「截屏识别」的返回：要么一张落盘的图片路径，要么一句可显示的错误。
class OcrCapture {
  const OcrCapture.ok(String this.path) : error = null;

  const OcrCapture.fail(String this.error) : path = null;

  final String? path;
  final String? error;

  bool get isError => error != null;
}

/// 解析截屏结果 JSON（纯函数，单测不加载 DLL）。
OcrCapture decodeOcrCapture(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const OcrCapture.fail('截屏结果不是合法 JSON');
  }
  if (decoded is! Map) {
    return const OcrCapture.fail('截屏结果格式不对');
  }
  final path = decoded['path'];
  if (path is String && path.trim().isNotEmpty) {
    return OcrCapture.ok(path.trim());
  }
  final error = decoded['error'];
  return OcrCapture.fail(
    error is String && error.trim().isNotEmpty ? error.trim() : '没有截到图片',
  );
}

/// 「选择文件」：系统单选框（图片过滤）。取消 / 桥接缺失返回空串。
String pickOcrImage() {
  final library = openLunacNative().library;
  if (library == null) {
    return '';
  }
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_StatusC, _StatusDart>(
      'lunac_ocr_pick_image',
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

/// 「截屏识别」：拉起系统截图浮层，等剪贴板出现新图（阻塞，放进后台 isolate）。
Future<OcrCapture> captureScreenForOcr({
  Duration timeout = const Duration(seconds: 60),
}) async {
  try {
    final raw = await Isolate.run(
      () => _callCapture(timeout.inMilliseconds),
    );
    return decodeOcrCapture(raw);
  } catch (e) {
    return OcrCapture.fail('截屏失败：$e');
  }
}

String _callCapture(int timeoutMs) {
  final library = openLunacNative().library;
  if (library == null) {
    return jsonEncode({'path': '', 'error': 'OCR 不可用（rust 桥接未加载）'});
  }
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_CaptureC, _CaptureDart>(
      'lunac_ocr_capture_screen',
    );
    raw = call(timeoutMs);
    if (raw == nullptr) {
      return jsonEncode({'path': '', 'error': '截屏没有返回内容'});
    }
    return raw.toDartString();
  } catch (e) {
    return jsonEncode({'path': '', 'error': '截屏失败：$e'});
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

/// 识别一张图片里的文字。识别是阻塞的子进程调用，放到独立 isolate 里跑。
Future<OcrReply> recognizeImage(String path) async {
  try {
    final raw = await Isolate.run(() => _callNative(path));
    return decodeOcrReply(raw);
  } catch (e) {
    return OcrReply.fail('识别失败：$e');
  }
}

String _callNative(String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return jsonEncode({'error': 'OCR 不可用（rust 桥接未加载）'});
  }
  Pointer<Utf8>? pathPtr;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_ImageC, _ImageDart>('lunac_ocr_image');
    pathPtr = path.toNativeUtf8();
    raw = call(pathPtr);
    if (raw == nullptr) {
      return jsonEncode({'error': 'OCR 没有返回内容'});
    }
    return raw.toDartString();
  } catch (e) {
    return jsonEncode({'error': '识别失败：$e'});
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
