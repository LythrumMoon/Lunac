// 平台薄层：本地播放（v1 `player.rs` 出声层的 Dart 等价物）。
//
// 边界：解码 / 出声 / 位置 / seek / 调音链全在 `rust/src/player.rs`；这里只做
// JSON 解码与容错，**不自己算位置、不自己管音量**。
//
// 为什么不继续用 `just_audio`：调音链必须挂在解码后、出声前的样本流上，
// 而 just_audio 在 Dart 侧 —— 调音打开也不会改变声音（见 `rust/src/player.rs` 文件头）。
//
// 纪律：坏 JSON / 桥接不可用 ⇒ 明确状态或一句人话，**不抛异常**；
// 只有「播放失败」这一条要让人看见原因，所以它在 `PlayerReply.error` 里原样带回，
// 由面板自己决定怎么显示（与 `platform/music.dart` 的 `error` 同口径）。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 播放器状态（`lunac_player_*` 的 `data`）。
class PlayerState {
  const PlayerState({
    required this.active,
    required this.playing,
    required this.path,
    required this.position,
    required this.duration,
    required this.volume,
    required this.video,
  });

  /// 有内容在播（`false` + `path` 非空 = **这一首放完了**，是自动下一首的判据）。
  final bool active;
  final bool playing;
  final String path;
  final Duration position;

  /// `Duration.zero` = 解码器报不出总时长（没有 Xing 头的 mp3 就是这样）。
  final Duration duration;

  /// 0.0–1.0，跨曲目保留。
  final double volume;

  /// 这是一个要出画面的视频（画面由界面那层出，声音仍归 Rust）。
  final bool video;

  static const idle = PlayerState(
    active: false,
    playing: false,
    path: '',
    position: Duration.zero,
    duration: Duration.zero,
    volume: 0.8,
    video: false,
  );

  static PlayerState fromJson(Map<String, dynamic> json) => PlayerState(
    active: json['active'] == true,
    playing: json['playing'] == true,
    path: json['path'] is String ? json['path'] as String : '',
    position: Duration(milliseconds: _int(json['positionMs'])),
    duration: Duration(milliseconds: _int(json['durationMs'])),
    volume: _double(json['volume'], 0.8),
    video: json['video'] == true,
  );
}

/// 一次播放调用的结果：`ok=false` 时 `error` 是**人话**（`ERR_NO_AUDIO_DEVICE` 也在里面）。
class PlayerReply {
  const PlayerReply({required this.ok, required this.error, this.state});

  final bool ok;
  final String error;
  final PlayerState? state;

  static const unavailable = PlayerReply(ok: false, error: '桥接不可用');
}

/// 播放失败（`message` 是 Rust 给的那句人话）。
///
/// 单独一个类型是为了 `toString()` 只吐那句话 —— 面板的 `_playErrorText`
/// 会拼成「播放失败：<原因>」，带 `Exception:` 前缀很难看。
class PlayerFailure implements Exception {
  const PlayerFailure(this.message);

  final String message;

  @override
  String toString() => message;
}

/// 装好一个文件（`autoplay=false` 时停在首帧，等面板自己 [playerResume]）。
///
/// 开设备 + 建解码器可能要几十毫秒，放进 `Isolate.run`，别冻住 UI 帧
/// （与 `platform/music.dart` 的扫描同一条纪律）。
Future<PlayerReply> playerOpen(String path, {bool autoplay = true}) async {
  String? raw;
  try {
    raw = await Isolate.run(() => _playInIsolate(path, autoplay ? 1 : 0));
  } catch (_) {
    raw = null;
  }
  return _decode(raw);
}

/// 继续播放（没有会话 / 已放完时 `ok=false`）。
PlayerReply playerResume() => _decode(_callNoArg('lunac_player_resume'));

PlayerReply playerPause() => _decode(_callNoArg('lunac_player_pause'));

/// 彻底收掉这一首（下次播放从零开始）。
PlayerReply playerStop() => _decode(_callNoArg('lunac_player_stop'));

/// 拖进度（毫秒；越界由 Rust 饱和到 `[0, duration]`）。
PlayerReply playerSeek(Duration position) =>
    _decode(_callSeek('lunac_player_seek', position.inMilliseconds));

/// 音量（0..1，越界夹紧并记住，下一首沿用）。
PlayerReply playerVolume(double value) =>
    _decode(_callDouble('lunac_player_volume', value));

/// 当前播放状态（面板轮询进度用）。
PlayerReply playerStatus() => _decode(_callNoArg('lunac_player_status'));

// ── 解析 ─────────────────────────────────────────────────────────────

PlayerReply _decode(String? raw) {
  if (raw == null) {
    return PlayerReply.unavailable;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return PlayerReply.unavailable;
    }
    final data = value['data'];
    return PlayerReply(
      ok: value['ok'] == true,
      error: value['error'] is String ? value['error'] as String : '',
      state: data is Map<String, dynamic> ? PlayerState.fromJson(data) : null,
    );
  } catch (_) {
    return PlayerReply.unavailable;
  }
}

int _int(Object? value) => value is int ? value : 0;

double _double(Object? value, double fallback) =>
    value is num ? value.toDouble() : fallback;

// ── FFI 细节 ─────────────────────────────────────────────────────────

typedef _NoArgC = Pointer<Utf8> Function();
typedef _NoArgDart = Pointer<Utf8> Function();
typedef _PlayC = Pointer<Utf8> Function(Pointer<Utf8>, Int32);
typedef _PlayDart = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _SeekC = Pointer<Utf8> Function(Uint64);
typedef _SeekDart = Pointer<Utf8> Function(int);
typedef _DoubleC = Pointer<Utf8> Function(Double);
typedef _DoubleDart = Pointer<Utf8> Function(double);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 在 isolate 里跑的那一段：只返回原始 JSON 字符串。
String? _playInIsolate(String path, int autoplay) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  final native = path.toNativeUtf8();
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_PlayC, _PlayDart>('lunac_player_play')(
      native,
      autoplay,
    );
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
    malloc.free(native);
  }
}

String? _callNoArg(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_NoArgC, _NoArgDart>(symbol)();
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

String? _callSeek(String symbol, int positionMs) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_SeekC, _SeekDart>(symbol)(positionMs);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

String? _callDouble(String symbol, double value) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_DoubleC, _DoubleDart>(symbol)(value);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

void _free(DynamicLibrary library, Pointer<Utf8>? pointer) {
  if (pointer == null || pointer == nullptr) {
    return;
  }
  try {
    library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(pointer);
  } catch (_) {
    // 符号缺失时忽略。
  }
}
