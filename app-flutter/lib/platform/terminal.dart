// 平台薄层：预览区终端（Windows）。
//
// 数据在 `rust/src/terminal.rs`：优先 ConPTY 真伪终端，系统不支持时降级为
// `cmd.exe /Q /K` 的管道模式（逐行执行，状态保留；没有全屏交互程序）。
// 这里只做「起 / 写 / 读 / 改尺寸 / 停」四件事，不猜 shell、不解析输出。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 起终端的结果。
class TerminalStart {
  const TerminalStart({
    this.ok = false,
    this.mode = '',
    this.warning = '',
    this.error = '',
  });

  final bool ok;

  /// `conpty`（真伪终端）/ `pipes`（管道降级档）。
  final String mode;

  /// 降级原因（空 = 没降级）。
  final String warning;
  final String error;

  bool get isPipe => mode == 'pipes';

  static const unavailable = TerminalStart(
    ok: false,
    error: '当前环境没有原生桥，终端不可用',
  );

  static TerminalStart fromRaw(String? text) {
    if (text == null || text.isEmpty) {
      return unavailable;
    }
    try {
      final value = jsonDecode(text);
      if (value is! Map<String, dynamic>) {
        return const TerminalStart(ok: false, error: '终端的返回不是合法 JSON');
      }
      String textOf(String key) =>
          value[key] is String ? value[key] as String : '';
      return TerminalStart(
        ok: value['ok'] == true,
        mode: textOf('mode'),
        warning: textOf('warning'),
        error: textOf('error'),
      );
    } catch (_) {
      return const TerminalStart(ok: false, error: '启动终端失败');
    }
  }
}

/// 一次轮询拿到的输出。
class TerminalChunk {
  const TerminalChunk({this.data = '', this.alive = false, this.mode = ''});

  final String data;
  final bool alive;
  final String mode;
}

/// 起终端（已经活着由 Rust 侧复用；cwd 固定是沙箱盘）。
TerminalStart terminalStart({int cols = 120, int rows = 30}) =>
    TerminalStart.fromRaw(
      _call1('lunac_term_start', jsonEncode({'cols': cols, 'rows': rows})),
    );

/// 写入按键 / 整行命令。
bool terminalWrite(String data) =>
    _okOf(_call1('lunac_term_write', jsonEncode({'data': data})));

/// 改终端尺寸（管道模式是空操作）。
void terminalResize({required int cols, required int rows}) {
  _call1('lunac_term_resize', jsonEncode({'cols': cols, 'rows': rows}));
}

/// 取走待显示的输出（排空）。
TerminalChunk terminalRead() {
  final text = _call0('lunac_term_read');
  if (text == null || text.isEmpty) {
    return const TerminalChunk();
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map<String, dynamic>) {
      return const TerminalChunk();
    }
    return TerminalChunk(
      data: value['data'] is String ? value['data'] as String : '',
      alive: value['alive'] == true,
      mode: value['mode'] is String ? value['mode'] as String : '',
    );
  } catch (_) {
    return const TerminalChunk();
  }
}

/// 关掉终端。
void terminalStop() => _call0('lunac_term_stop');

bool _okOf(String? text) {
  if (text == null || text.isEmpty) {
    return false;
  }
  try {
    final value = jsonDecode(text);
    return value is Map && value['ok'] == true;
  } catch (_) {
    return false;
  }
}

typedef _StringC = Pointer<Utf8> Function();
typedef _StringDart = Pointer<Utf8> Function();
typedef _TextC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

String? _call0(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_StringC, _StringDart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

String? _call1(String symbol, String argument) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? arg;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_TextC, _TextDart>(symbol);
    arg = argument.toNativeUtf8();
    raw = call(arg);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (arg != null) {
      malloc.free(arg);
    }
    _free(library, raw);
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? raw) {
  if (raw == null || raw == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
  } catch (_) {
    // 旧桥没有这个符号：忽略。
  }
}
