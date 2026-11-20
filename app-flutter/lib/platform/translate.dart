// 平台薄层：翻译（v1 `translate` 插件的原生等价物）。
//
// 边界：联网 / 缓存 / 解析都在 `rust/src/translate.rs`；本文件只负责数据形状、
// 三个 C ABI（lookup / ai / free）与容错。返回形状与 v1 `TranslateResult` 逐字段一致：
// `text` / `from` / `to` / `source` / `cached` / `translation` / `phonetic` /
// `explains` / `examples[]`；失败时 Rust 返回 `{"error": "…"}`。
//
// 纪律：坏 JSON / 桥接不可用 → 一句人话错误，**不抛异常、不崩面板**。
// 联网在 FFI 里是阻塞的，所以放到 `Isolate.run` 里跑，别冻住 UI 帧。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一条双语例句。
class TranslateExample {
  const TranslateExample({required this.src, required this.dst});

  final String src;
  final String dst;

  static TranslateExample? fromJson(Map<String, dynamic> json) {
    final src = json['src'];
    final dst = json['dst'];
    if (src is! String || dst is! String) {
      return null;
    }
    return TranslateExample(src: src, dst: dst);
  }
}

/// 一次翻译结果（字段与 v1 一致）。
class TranslateResult {
  const TranslateResult({
    required this.text,
    required this.from,
    required this.to,
    required this.source,
    required this.cached,
    required this.translation,
    required this.phonetic,
    required this.explains,
    required this.examples,
  });

  final String text;
  final String from;
  final String to;

  /// `dict` = 词典底座；`ai` = 模型补漏；`none` = 两层都没有结果。
  final String source;

  /// 结果直接来自本地缓存（这次没碰网络）。
  final bool cached;

  final String translation;
  final String phonetic;
  final List<String> explains;
  final List<TranslateExample> examples;

  static TranslateResult? fromJson(Map<String, dynamic> json) {
    final text = json['text'];
    final from = json['from'];
    final to = json['to'];
    final source = json['source'];
    if (text is! String || from is! String || to is! String || source is! String) {
      return null;
    }
    return TranslateResult(
      text: text,
      from: from,
      to: to,
      source: source,
      cached: json['cached'] == true,
      translation: json['translation'] is String
          ? json['translation'] as String
          : '',
      phonetic: json['phonetic'] is String ? json['phonetic'] as String : '',
      explains: [
        for (final line in (json['explains'] as List? ?? const []))
          if (line is String) line,
      ],
      examples: (json['examples'] as List? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(TranslateExample.fromJson)
          .whereType<TranslateExample>()
          .toList(),
    );
  }
}

/// 平台层返回：要么一份结果，要么一句可显示的错误。
class TranslateReply {
  const TranslateReply.ok(TranslateResult this.result) : error = null;

  const TranslateReply.fail(String this.error) : result = null;

  final TranslateResult? result;
  final String? error;

  bool get isError => error != null;
}

/// 解析 Rust 返回的 JSON（纯函数，单测不加载 DLL）。
///
/// `{"error": "…"}` 优先；坏 JSON / 形状不对都给明确错误。
TranslateReply decodeTranslateReply(String raw) {
  Object? value;
  try {
    value = jsonDecode(raw);
  } catch (_) {
    return const TranslateReply.fail('翻译结果不是合法 JSON');
  }
  if (value is! Map<String, dynamic>) {
    return const TranslateReply.fail('翻译结果格式不对');
  }
  final error = value['error'];
  if (error is String && error.trim().isNotEmpty) {
    return TranslateReply.fail(error.trim());
  }
  final result = TranslateResult.fromJson(value);
  if (result == null) {
    return const TranslateReply.fail('翻译结果缺少必要字段');
  }
  return TranslateReply.ok(result);
}

typedef _TranslateC = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _TranslateDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 词典底座查询（缓存 → MyMemory + 有道）；`source == 'none'` 表示两层都没结果。
Future<TranslateReply> lookupTranslation(
  String text, {
  String from = 'auto',
  String to = 'zh-CN',
}) =>
    _runInIsolate('lunac_translate_lookup', text, from, to);

/// 模型补漏（会花钱，只在用户点「用 AI 翻译 / 重译」时调用）。
Future<TranslateReply> translateWithAi(
  String text, {
  String from = 'auto',
  String to = 'zh-CN',
}) =>
    _runInIsolate('lunac_translate_ai', text, from, to);

Future<TranslateReply> _runInIsolate(
  String symbol,
  String text,
  String from,
  String to,
) async {
  try {
    // 阻塞 HTTP 放到独立 isolate，避免翻译期间 UI 冻住。
    final raw = await Isolate.run(() => _callNative(symbol, text, from, to));
    return decodeTranslateReply(raw);
  } catch (e) {
    return TranslateReply.fail('翻译失败：$e');
  }
}

String _callNative(String symbol, String text, String from, String to) {
  final library = openLunacNative().library;
  if (library == null) {
    return jsonEncode({'error': '翻译不可用（rust 桥接未加载）'});
  }
  Pointer<Utf8>? textPtr;
  Pointer<Utf8>? fromPtr;
  Pointer<Utf8>? toPtr;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TranslateC, _TranslateDart>(symbol);
    textPtr = text.toNativeUtf8();
    fromPtr = from.toNativeUtf8();
    toPtr = to.toNativeUtf8();
    raw = call(textPtr, fromPtr, toPtr);
    if (raw == nullptr) {
      return jsonEncode({'error': '翻译没有返回内容'});
    }
    return raw.toDartString();
  } catch (e) {
    return jsonEncode({'error': '翻译失败：$e'});
  } finally {
    try {
      if (raw != null && raw != nullptr) {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      }
    } catch (_) {
      // 符号缺失时忽略。
    }
    if (textPtr != null) {
      malloc.free(textPtr);
    }
    if (fromPtr != null) {
      malloc.free(fromPtr);
    }
    if (toPtr != null) {
      malloc.free(toPtr);
    }
  }
}
