// 平台薄层：音乐（v1 `music` 插件的本地媒体库 / 本地 LRC 的原生等价物）。
//
// 边界：
//   · 扫描 / 标签 / 封面 / LRC 解析都在 `rust/src/music.rs`；
//   · 本文件只负责数据形状、两个 C ABI（`lunac_music_library` / `lunac_music_lyrics`）
//     与容错解析；
//   · 出声走 Rust（`rust/src/player.rs`，rodio 解码 + 调音链），见 [RustMusicEngine]。
//     2026-11-15 起不再是 just_audio：DSP（调音）必须改样本流，而样本只有 Rust 拿得到。
//
// 纪律：坏 JSON / 桥接不可用 → 明确状态或一句人话，**不抛异常**；
// 扫描是磁盘 + 标签解析，放进 `Isolate.run`，别冻住 UI 帧。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';
import 'player.dart';

/// 一首本地曲目（`lunac_music_library` 的 `tracks[]`）。
class MusicTrack {
  const MusicTrack({
    required this.path,
    required this.title,
    required this.artist,
    required this.album,
    required this.durationMs,
    required this.cover,
  });

  final String path;
  final String title;
  final String artist;
  final String album;

  /// 毫秒；`0` = 标签里读不到时长。
  final int durationMs;

  /// 已缓存封面的绝对路径（空串 = 没有封面）。
  final String cover;

  /// 歌手 · 专辑（与 v1 播放条副标题同一拼法）。
  String get subtitle {
    final parts = [artist, album].where((part) => part.trim().isNotEmpty);
    return parts.join(' · ');
  }

  static MusicTrack? fromJson(Map<dynamic, dynamic> json) {
    final path = json['path'];
    if (path is! String || path.isEmpty) {
      return null;
    }
    final duration = json['duration_ms'] ?? json['durationMs'];
    return MusicTrack(
      path: path,
      title: json['title'] is String ? json['title'] as String : '',
      artist: json['artist'] is String ? json['artist'] as String : '',
      album: json['album'] is String ? json['album'] as String : '',
      durationMs: duration is int ? duration : 0,
      cover: json['cover'] is String ? json['cover'] as String : '',
    );
  }
}

/// 本地曲库（`lunac_music_library`）。
class MusicLibrary {
  const MusicLibrary({
    required this.tracks,
    required this.roots,
    this.error,
  });

  final List<MusicTrack> tracks;

  /// 实际扫描过的目录（空 = 没有可扫的曲库目录）。
  final List<String> roots;

  /// 整轮失败时的一句人话（单个文件解析失败不算）。
  final String? error;
}

/// 一行 LRC 歌词（`[mm:ss.xx]` 已换算成毫秒）。
class MusicLyricLine {
  const MusicLyricLine({required this.timeMs, required this.text});

  final int timeMs;
  final String text;

  static MusicLyricLine? fromJson(Map<dynamic, dynamic> json) {
    final time = json['timeMs'] ?? json['time_ms'];
    final text = json['text'];
    if (time is! int || text is! String) {
      return null;
    }
    return MusicLyricLine(timeMs: time, text: text);
  }
}

/// 一首歌的歌词（`lunac_music_lyrics` / `lunac_music_lyrics_online`）。
class MusicLyrics {
  const MusicLyrics({
    required this.lines,
    this.error,
    this.found = false,
    this.instrumental = false,
    this.source = '',
    this.plain,
  });

  final List<MusicLyricLine> lines;
  final String? error;

  /// 在线检索是否命中（本地 `.lrc` 的解析结果没有这个概念，恒 false）。
  final bool found;

  /// 命中了一条「纯音乐」记录（没有词，但**不是**失败）。
  final bool instrumental;

  /// `lrclib` / `netease`（本地 LRC 时为空串）。
  final String source;

  /// 没有时间轴时的纯文本歌词。
  final String? plain;

  bool get isEmpty => lines.isEmpty;

  /// 面板展示的来源名（本地 LRC 用「本地 .lrc」）。
  String get sourceLabel => switch (source) {
    'lrclib' => 'LRCLIB',
    'netease' => '网易云',
    _ => '',
  };
}

/// 解析曲库 JSON（纯函数，单测不加载 DLL）。
///
/// 坏 JSON / 形状不对都收敛成「空曲库 + 一句原因」，绝不抛异常。
MusicLibrary parseMusicLibrary(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const MusicLibrary(tracks: [], roots: [], error: '曲库结果不是合法 JSON');
  }
  if (decoded is! Map) {
    return const MusicLibrary(tracks: [], roots: [], error: '曲库结果格式不对');
  }
  final tracks = <MusicTrack>[];
  final rawTracks = decoded['tracks'];
  for (final item in (rawTracks is List ? rawTracks : const [])) {
    if (item is Map) {
      final track = MusicTrack.fromJson(item);
      if (track != null) {
        tracks.add(track);
      }
    }
  }
  final roots = <String>[
    for (final root in (decoded['roots'] is List ? decoded['roots'] as List : const []))
      if (root is String && root.isNotEmpty) root,
  ];
  final error = decoded['error'];
  return MusicLibrary(
    tracks: tracks,
    roots: roots,
    error: error is String && error.trim().isNotEmpty ? error.trim() : null,
  );
}

/// 解析歌词 JSON（纯函数，单测不加载 DLL）。
MusicLyrics parseMusicLyrics(String raw) {
  Object? decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    return const MusicLyrics(lines: [], error: '歌词结果不是合法 JSON');
  }
  if (decoded is! Map) {
    return const MusicLyrics(lines: [], error: '歌词结果格式不对');
  }
  final error = decoded['error'];
  final lines = <MusicLyricLine>[];
  for (final item in (decoded['lines'] is List ? decoded['lines'] as List : const [])) {
    if (item is Map) {
      final line = MusicLyricLine.fromJson(item);
      if (line != null) {
        lines.add(line);
      }
    }
  }
  lines.sort((a, b) => a.timeMs.compareTo(b.timeMs));
  final source = decoded['source'];
  final plain = decoded['plain'];
  var text = error is String && error.trim().isNotEmpty ? error.trim() : null;
  // 限流是「等几秒再试」，不是「失败」——把机器码翻成一句人话（v1 同口径）。
  if (text != null && text.startsWith('ERR_RATE_LIMITED:')) {
    final retry = text.substring('ERR_RATE_LIMITED:'.length).trim();
    text = retry.isEmpty || retry == '?'
        ? '歌词服务限流了，等几秒再试'
        : '歌词服务限流了，等 $retry 秒再试';
  }
  return MusicLyrics(
    lines: lines,
    error: text,
    found: decoded['found'] == true,
    instrumental: decoded['instrumental'] == true,
    source: source is String ? source : '',
    plain: plain is String && plain.trim().isNotEmpty ? plain : null,
  );
}

typedef _LibraryC = Pointer<Utf8> Function();
typedef _LibraryDart = Pointer<Utf8> Function();
typedef _StringC = Pointer<Utf8> Function();
typedef _StringDart = Pointer<Utf8> Function();
typedef _LyricsC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _LyricsDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

// ── 曲库目录（v1 本地音乐的「添加目录 / 管理」）─────────────────────

/// 添加 / 移除曲库目录的结果（`lunac_music_*_root` 的 JSON）。
class MusicRootResult {
  const MusicRootResult({required this.ok, required this.path, required this.error});

  final bool ok;

  /// 规范化后的目录（添加成功时有值）。
  final String path;
  final String error;

  static MusicRootResult fromJson(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const MusicRootResult(ok: false, path: '', error: '曲库结果不是合法 JSON');
    }
    if (decoded is! Map) {
      return const MusicRootResult(ok: false, path: '', error: '曲库结果格式不对');
    }
    return MusicRootResult(
      ok: decoded['ok'] == true,
      path: decoded['path'] is String ? decoded['path'] as String : '',
      error: decoded['error'] is String ? decoded['error'] as String : '',
    );
  }
}

/// 添加曲库目录（目录必须存在；重复添加幂等）。
MusicRootResult addMusicRoot(String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return const MusicRootResult(ok: false, path: '', error: 'rust 桥接不可用');
  }
  Pointer<Utf8>? pathPtr;
  Pointer<Utf8>? raw;
  try {
    pathPtr = path.toNativeUtf8();
    raw = library.lookupFunction<_LyricsC, _LyricsDart>(
      'lunac_music_add_root',
    )(pathPtr);
    if (raw == nullptr) {
      return const MusicRootResult(ok: false, path: '', error: '没有返回结果');
    }
    return MusicRootResult.fromJson(raw.toDartString());
  } catch (_) {
    return const MusicRootResult(ok: false, path: '', error: 'rust 桥接调用失败');
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
    if (pathPtr != null) {
      malloc.free(pathPtr);
    }
  }
}

/// 移除曲库目录（幂等）。
MusicRootResult removeMusicRoot(String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return const MusicRootResult(ok: false, path: '', error: 'rust 桥接不可用');
  }
  Pointer<Utf8>? pathPtr;
  Pointer<Utf8>? raw;
  try {
    pathPtr = path.toNativeUtf8();
    raw = library.lookupFunction<_LyricsC, _LyricsDart>(
      'lunac_music_remove_root',
    )(pathPtr);
    if (raw == nullptr) {
      return const MusicRootResult(ok: false, path: '', error: '没有返回结果');
    }
    return MusicRootResult.fromJson(raw.toDartString());
  } catch (_) {
    return const MusicRootResult(ok: false, path: '', error: 'rust 桥接调用失败');
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
    if (pathPtr != null) {
      malloc.free(pathPtr);
    }
  }
}

/// 弹系统的「选择文件夹」选框；取消 / 桥接缺失返回空串。
String pickMusicRoot() {
  final library = openLunacNative().library;
  if (library == null) {
    return '';
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_LibraryC, _LibraryDart>(
      'lunac_music_pick_root',
    )();
    if (raw == nullptr) {
      return '';
    }
    return raw.toDartString().trim();
  } catch (_) {
    return '';
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

// ── 自定义队列（v1 `music.ts` 的 `LocalQueue` 多条版）────────────────

/// 一条自定义队列。字段名与 v1 一致（`id` / `name`）；v2 的条目只存路径
/// （配置落 `config/music-queues.json`，Rust 侧负责收敛与校验）。
class MusicQueue {
  const MusicQueue({required this.id, required this.name, required this.paths});

  final String id;
  final String name;
  final List<String> paths;

  MusicQueue copyWith({String? id, String? name, List<String>? paths}) {
    return MusicQueue(
      id: id ?? this.id,
      name: name ?? this.name,
      paths: paths ?? this.paths,
    );
  }

  Map<String, Object?> toJson() => {'id': id, 'name': name, 'paths': paths};

  static MusicQueue? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    final id = json['id'];
    final name = json['name'];
    final rawPaths = json['paths'];
    if (id is! String || id.isEmpty) {
      return null;
    }
    return MusicQueue(
      id: id,
      name: name is String ? name : '',
      paths: [
        for (final path in (rawPaths is List ? rawPaths : const []))
          if (path is String && path.isNotEmpty) path,
      ],
    );
  }
}

/// 读自定义队列；桥接缺失返回 null（面板显示「不可用」）。
List<MusicQueue>? readMusicQueues() {
  final raw = _callString('lunac_music_queues');
  if (raw == null) {
    return null;
  }
  return _parseQueues(raw);
}

List<MusicQueue>? _parseQueues(String raw) {
  try {
    final decoded = jsonDecode(raw);
    if (decoded is! Map) {
      return null;
    }
    final list = decoded['queues'];
    return [
      for (final item in (list is List ? list : const [])) ?MusicQueue.fromJson(item),
    ];
  } catch (_) {
    return null;
  }
}

/// 覆盖写全部队列。返回 null = 成功；否则是一句原因。收敛后的队列由
/// [readMusicQueues] 重新读取（Rust 会补空 id / 去重）。
String? saveMusicQueues(List<MusicQueue> queues) {
  final library = openLunacNative().library;
  if (library == null) {
    return 'rust 桥接不可用';
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final body = jsonEncode({
      'queues': [for (final queue in queues) queue.toJson()],
    });
    input = body.toNativeUtf8();
    raw = library.lookupFunction<_LyricsC, _LyricsDart>(
      'lunac_music_save_queues',
    )(input);
    if (raw == nullptr) {
      return '没有返回结果';
    }
    final decoded = jsonDecode(raw.toDartString());
    if (decoded is! Map) {
      return '队列保存结果格式不对';
    }
    if (decoded['ok'] == true) {
      return null;
    }
    final error = decoded['error'];
    return error is String && error.isNotEmpty ? error : '保存队列失败';
  } catch (_) {
    return '保存队列失败';
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
    if (input != null) {
      malloc.free(input);
    }
  }
}

/// 导入播放列表的结果（`lunac_music_import_playlist`）。
class MusicPlaylistImport {
  const MusicPlaylistImport({
    required this.ok,
    required this.canceled,
    required this.name,
    required this.paths,
    required this.error,
  });

  final bool ok;

  /// 用户取消了对话框（不算错误）。
  final bool canceled;

  /// 播放列表文件名（不含扩展名，用作队列默认名）。
  final String name;
  final List<String> paths;
  final String error;

  static MusicPlaylistImport? fromJson(String raw) {
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! Map) {
        return null;
      }
      final paths = decoded['paths'];
      return MusicPlaylistImport(
        ok: decoded['ok'] == true,
        canceled: decoded['canceled'] == true,
        name: decoded['name'] is String ? decoded['name'] as String : '',
        paths: [
          for (final path in (paths is List ? paths : const []))
            if (path is String && path.isNotEmpty) path,
        ],
        error: decoded['error'] is String ? decoded['error'] as String : '',
      );
    } catch (_) {
      return null;
    }
  }
}

/// 弹「选择播放列表」对话框并解析 `.m3u/.m3u8/.pls`；桥接缺失 / 解析失败返回 null。
MusicPlaylistImport? importMusicPlaylist() {
  final raw = _callString('lunac_music_import_playlist');
  return raw == null ? null : MusicPlaylistImport.fromJson(raw);
}

/// 弹多选框选音频文件（新建队列用）；取消 / 桥接缺失返回空表。
List<String> pickMusicFiles() {
  final raw = _callString('lunac_music_pick_files');
  if (raw == null) {
    return const [];
  }
  try {
    final decoded = jsonDecode(raw);
    if (decoded is! Map) {
      return const [];
    }
    final paths = decoded['paths'];
    return [
      for (final path in (paths is List ? paths : const []))
        if (path is String && path.isNotEmpty) path,
    ];
  } catch (_) {
    return const [];
  }
}

/// 无参 C ABI 的统一调用（读字符串；桥接缺失 / 抛错返回 null）。
String? _callString(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_LibraryC, _LibraryDart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

/// 读本地曲库。桥接不可用时返回 null（面板显示「不可用」）。
///
/// 扫描要读标签，可能耗秒级 ⇒ 放进独立 isolate。
Future<MusicLibrary?> loadMusicLibrary() async {
  try {
    final raw = await Isolate.run(_callLibrary);
    if (raw == null) {
      return null;
    }
    return parseMusicLibrary(raw);
  } catch (e) {
    return MusicLibrary(tracks: const [], roots: const [], error: '读取曲库失败：$e');
  }
}

String? _callLibrary() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_LibraryC, _LibraryDart>(
      'lunac_music_library',
    )();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

/// 读一首歌的同名 `.lrc`（[audioPath] 是音频路径）。
Future<MusicLyrics> loadMusicLyrics(String audioPath) async {
  try {
    final raw = await Isolate.run(() => _callLyrics(audioPath));
    if (raw == null) {
      return const MusicLyrics(lines: [], error: '不可用（rust 桥接未加载）');
    }
    return parseMusicLyrics(raw);
  } catch (e) {
    return MusicLyrics(lines: const [], error: '读取歌词失败：$e');
  }
}

String? _callLyrics(String audioPath) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? pathPtr;
  Pointer<Utf8>? raw;
  try {
    pathPtr = audioPath.toNativeUtf8();
    raw = library.lookupFunction<_LyricsC, _LyricsDart>(
      'lunac_music_lyrics',
    )(pathPtr);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
    if (pathPtr != null) {
      malloc.free(pathPtr);
    }
  }
}

/// 在线取词（LRCLIB → LRCLIB 搜索 → 网易云兜底，与 v1 三级口径一致）。
///
/// 网络请求在 worker isolate 里跑（Rust 侧同步阻塞，最多 8s 超时），不冻 UI。
/// 三级全落空是「没找到歌词」（`found: false` + `error == null`），**不是错误**。
Future<MusicLyrics> loadOnlineMusicLyrics({
  required String title,
  String artist = '',
  String album = '',
  double duration = 0,
}) async {
  final trimmed = title.trim();
  if (trimmed.isEmpty) {
    return const MusicLyrics(lines: [], error: '这首歌没有歌名，无法搜索在线歌词');
  }
  final payload = jsonEncode({
    'title': trimmed,
    'artist': artist.trim(),
    'album': album.trim(),
    'duration': duration,
  });
  try {
    final raw = await Isolate.run(() => _callOnlineLyrics(payload));
    if (raw == null) {
      return const MusicLyrics(lines: [], error: '不可用（rust 桥接未加载）');
    }
    return parseMusicLyrics(raw);
  } catch (e) {
    return MusicLyrics(lines: const [], error: '在线取词失败：$e');
  }
}

String? _callOnlineLyrics(String payload) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? payloadPtr;
  Pointer<Utf8>? raw;
  try {
    payloadPtr = payload.toNativeUtf8();
    raw = library.lookupFunction<_LyricsC, _LyricsDart>(
      'lunac_music_lyrics_online',
    )(payloadPtr);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
    if (payloadPtr != null) {
      malloc.free(payloadPtr);
    }
  }
}

// ── Spotify 在线库（v1 `music.rs` 的 Web API 那一段）──────────────────

/// Spotify 的连接状态（`lunac_music_spotify_state`）。
class SpotifyState {
  const SpotifyState({
    this.configured = false,
    this.connected = false,
    this.builtin = true,
    this.clientId = '',
    this.port = 0,
    this.redirectUri = '',
    this.displayName = '',
    this.avatar = '',
    this.pending = false,
    this.stopActive = false,
    this.stopReason = '',
    this.retryAfterSecs = 0,
    this.path = '',
    this.error,
  });

  final bool configured;
  final bool connected;

  /// 用的是不是**内置** Client ID（面板据此收起凭据字段）。
  final bool builtin;
  final String clientId;
  final int port;
  final String redirectUri;
  final String displayName;
  final String avatar;

  /// 浏览器里的授权流程是否还在等回调。
  final bool pending;

  /// 429 硬闸：`retryAfterSecs` 秒内所有请求都会被拦下。
  final bool stopActive;
  final String stopReason;
  final int retryAfterSecs;
  final String path;
  final String? error;

  bool get ok => error == null;

  static SpotifyState? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    String text(Object? value) => value is String ? value : '';
    int number(Object? value) => value is int ? value : 0;
    final stop = json['stop'];
    return SpotifyState(
      configured: json['configured'] == true,
      connected: json['connected'] == true,
      builtin: json['builtin'] != false,
      clientId: text(json['clientId']),
      port: number(json['port']),
      redirectUri: text(json['redirectUri']),
      displayName: text(json['displayName']),
      avatar: text(json['avatar']),
      pending: json['pending'] == true,
      stopActive: stop is Map && stop['active'] == true,
      stopReason: stop is Map ? text(stop['reason']) : '',
      retryAfterSecs: stop is Map ? number(stop['retryAfterSecs']) : 0,
      path: text(json['path']),
    );
  }
}

/// 一次 Spotify 动作的应答（`{ok, error, data}`）。
class SpotifyReply {
  const SpotifyReply({required this.ok, this.error = '', this.data});

  final bool ok;
  final String error;
  final Object? data;

  /// 把 Rust 的机器码翻成一句人话（面板直接显示）。
  String get errorText {
    if (error == 'ERR_NOT_CONNECTED') {
      return 'Spotify 未连接（或登录已失效），重新授权一次';
    }
    if (error.startsWith('ERR_RATE_LIMITED:')) {
      final retry = error.substring('ERR_RATE_LIMITED:'.length).trim();
      return 'Spotify 限流了，等 $retry 秒再试';
    }
    return error;
  }

  static SpotifyReply parse(String raw) {
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return const SpotifyReply(ok: false, error: 'Spotify 应答不是合法 JSON');
    }
    if (decoded is! Map) {
      return const SpotifyReply(ok: false, error: 'Spotify 应答格式不对');
    }
    final error = decoded['error'];
    return SpotifyReply(
      ok: decoded['ok'] == true,
      error: error is String ? error : '',
      data: decoded['data'],
    );
  }
}

/// 读 Spotify 状态（只读 `config/music.json`，不发网络请求）。
SpotifyState readMusicSpotifyState() {
  final library = openLunacNative().library;
  if (library == null) {
    return const SpotifyState(error: '不可用（rust 桥接未加载）');
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_StringC, _StringDart>(
      'lunac_music_spotify_state',
    )();
    if (raw == nullptr) {
      return const SpotifyState(error: 'Spotify 状态没有返回内容');
    }
    final state = SpotifyState.fromJson(jsonDecode(raw.toDartString()));
    return state ?? const SpotifyState(error: 'Spotify 状态格式不对');
  } catch (e) {
    return SpotifyState(error: '读取 Spotify 状态失败：$e');
  } finally {
    if (raw != null && raw != nullptr) {
      _free(library, raw);
    }
  }
}

/// 开始授权：Rust 起环回回调并打开浏览器，返回授权链接。
Future<SpotifyReply> connectMusicSpotify() async {
  final raw = await Isolate.run(
    () => _callSpotifyNoArg('lunac_music_spotify_connect'),
  );
  if (raw == null) {
    return const SpotifyReply(ok: false, error: '不可用（rust 桥接未加载）');
  }
  return SpotifyReply.parse(raw);
}

/// 断开 Spotify（清空本地令牌）。
Future<SpotifyReply> disconnectMusicSpotify() async {
  final raw = await Isolate.run(
    () => _callSpotifyNoArg('lunac_music_spotify_disconnect'),
  );
  if (raw == null) {
    return const SpotifyReply(ok: false, error: '不可用（rust 桥接未加载）');
  }
  return SpotifyReply.parse(raw);
}

/// 保存「自己的 Client ID / 回调端口」（两个都可选；换值会清掉旧令牌）。
Future<SpotifyReply> saveMusicSpotifyConfig({
  String? clientId,
  int? port,
}) async {
  final text = jsonEncode({
    'clientId': ?clientId,
    'port': ?port,
  });
  final raw = await Isolate.run(
    () => _callSpotify(text, 'lunac_music_spotify_config'),
  );
  if (raw == null) {
    return const SpotifyReply(ok: false, error: '不可用（rust 桥接未加载）');
  }
  return SpotifyReply.parse(raw);
}

/// 调一个 Spotify Web API 动作（`{action, …}`），返回 `{ok, error, data}`。
Future<SpotifyReply> callMusicSpotify(Map<String, Object?> payload) async {
  final text = jsonEncode(payload);
  final raw = await Isolate.run(() => _callSpotify(text));
  if (raw == null) {
    return const SpotifyReply(ok: false, error: '不可用（rust 桥接未加载）');
  }
  return SpotifyReply.parse(raw);
}

String? _callSpotifyNoArg(String symbol) {
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

String? _callSpotify(
  String payload, [
  String symbol = 'lunac_music_spotify_call',
]) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? payloadPtr;
  Pointer<Utf8>? raw;
  try {
    payloadPtr = payload.toNativeUtf8();
    raw = library.lookupFunction<_LyricsC, _LyricsDart>(symbol)(payloadPtr);
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
    if (payloadPtr != null) {
      malloc.free(payloadPtr);
    }
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

// ── 本地媒体库的 v1 形态（SQLite + 后台扫描线程）──────────────────
//
// 与 `loadMusicLibrary()`（同步全量扫描）的分工：
//   · 面板**打开时**走这里查库（不重新解析几千个文件的标签）；
//   · 「扫描」按钮起后台线程，界面 500ms 轮询 [mediaScanStatus] 拿真进度；
//   · 删除一首歌连索引与封面缓存一起删（`media_delete_file` 的安全边界在 Rust）。
// 桥接不可用时这些函数返回 `ok=false`，调用方回落 `loadMusicLibrary()`。

/// 一个曲库目录（`roots[]`）。
class MediaRoot {
  const MediaRoot({
    required this.path,
    required this.name,
    required this.tracks,
    required this.lastScanAt,
    required this.missing,
  });

  final String path;

  /// 末段目录名（界面上那一行显示它，完整路径放 tooltip）。
  final String name;
  final int tracks;
  final int lastScanAt;

  /// 目录现在还在不在（被删 / 拔盘的根要能看出来）。
  final bool missing;

  static MediaRoot? fromJson(Object? value) {
    if (value is! Map) {
      return null;
    }
    final path = value['path'];
    if (path is! String || path.isEmpty) {
      return null;
    }
    return MediaRoot(
      path: path,
      name: value['name'] is String ? value['name'] as String : path,
      tracks: value['tracks'] is int ? value['tracks'] as int : 0,
      lastScanAt: value['lastScanAt'] is int ? value['lastScanAt'] as int : 0,
      missing: value['missing'] == true,
    );
  }
}

/// 扫描进度（`media_scan_status`；不扫时是上一次的结果）。
class MediaScanStatus {
  const MediaScanStatus({
    required this.running,
    required this.scanned,
    required this.total,
    required this.added,
    required this.updated,
    required this.removed,
    required this.unchanged,
    required this.failed,
    required this.error,
    required this.doneAt,
  });

  static const idle = MediaScanStatus(
    running: false,
    scanned: 0,
    total: 0,
    added: 0,
    updated: 0,
    removed: 0,
    unchanged: 0,
    failed: 0,
    error: '',
    doneAt: 0,
  );

  final bool running;
  final int scanned;

  /// 候选文件总数（`0` = 还没数完）。
  final int total;
  final int added;
  final int updated;
  final int removed;

  /// 本次没重新解析的（size + mtime 都没变）。
  final int unchanged;
  final int failed;
  final String error;
  final int doneAt;

  /// 0..1；分母还没数出来时是 0（界面画不确定态）。
  double get progress => total > 0 ? (scanned / total).clamp(0.0, 1.0) : 0;

  static MediaScanStatus fromJson(Object? value) {
    if (value is! Map) {
      return idle;
    }
    int at(String key) => value[key] is int ? value[key] as int : 0;
    return MediaScanStatus(
      running: value['running'] == true,
      scanned: at('scanned'),
      total: at('total'),
      added: at('added'),
      updated: at('updated'),
      removed: at('removed'),
      unchanged: at('unchanged'),
      failed: at('failed'),
      error: value['error'] is String ? value['error'] as String : '',
      doneAt: at('doneAt'),
    );
  }
}

/// 目录列表的结果。
class MediaRootsReply {
  const MediaRootsReply({required this.ok, required this.error, this.roots});

  final bool ok;
  final String error;
  final List<MediaRoot>? roots;

  static const unavailable = MediaRootsReply(ok: false, error: '桥接不可用');
}

/// 查曲目的结果（`total` 是命中总数，不受 `limit` 影响）。
class MediaTracksReply {
  const MediaTracksReply({
    required this.ok,
    required this.error,
    required this.total,
    this.items,
  });

  final bool ok;
  final String error;
  final int total;
  final List<MusicTrack>? items;

  static const unavailable = MediaTracksReply(
    ok: false,
    error: '桥接不可用',
    total: 0,
  );
}

/// 扫描启动 / 删除文件的结果。
class MediaActionReply {
  const MediaActionReply({required this.ok, required this.error, this.status});

  final bool ok;
  final String error;
  final MediaScanStatus? status;

  static const unavailable = MediaActionReply(ok: false, error: '桥接不可用');
}

/// 读曲库目录（含曲目数 / 目录是否还在）。
MediaRootsReply readMediaRoots() => _parseRoots(_mediaCall0('lunac_media_roots'));

/// 添加一个曲库目录（**不立刻扫描**）。
MediaRootsReply addMediaRoot(String path) =>
    _parseRoots(_mediaCallText('lunac_media_add_root', path));

/// 移除一个曲库目录（连同它的曲目）。
MediaRootsReply removeMediaRoot(String path) =>
    _parseRoots(_mediaCallText('lunac_media_remove_root', path));

/// 开始扫描（后台线程，立刻返回）。`root` 空 = 扫全部。
MediaActionReply startMediaScan([String root = '']) =>
    _parseAction(_mediaCallText('lunac_media_scan_start', root));

/// 扫描进度（界面 500ms 轮询一次）。
MediaScanStatus mediaScanStatus() {
  final raw = _mediaCall0('lunac_media_scan_status');
  if (raw == null) {
    return MediaScanStatus.idle;
  }
  try {
    return MediaScanStatus.fromJson(jsonDecode(raw));
  } catch (_) {
    return MediaScanStatus.idle;
  }
}

/// 查曲目（`root` 空 = 全库；`query` 走标题 / 歌手 / 专辑的模糊匹配）。
MediaTracksReply readMediaTracks({
  String root = '',
  String query = '',
  int limit = 2000,
}) {
  final raw = _mediaCallTracks(root, query, limit);
  if (raw == null) {
    return MediaTracksReply.unavailable;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map) {
      return MediaTracksReply.unavailable;
    }
    final items = value['items'];
    return MediaTracksReply(
      ok: value['ok'] == true,
      error: value['error'] is String ? value['error'] as String : '',
      total: value['total'] is int ? value['total'] as int : 0,
      items: items is List
          ? items
                .map((item) => MusicTrack.fromJson(item as Map))
                .whereType<MusicTrack>()
                .toList(growable: false)
          : const [],
    );
  } catch (_) {
    return MediaTracksReply.unavailable;
  }
}

/// 从库里删除单个文件（磁盘文件 + 索引 + 封面缓存）。
MediaActionReply deleteMediaFile(String path) =>
    _parseAction(_mediaCallText('lunac_media_delete_file', path));

/// 一份播放列表（`media_playlist`：**只读那个文件，不入库**）。
class MediaPlaylistReply {
  const MediaPlaylistReply({
    required this.ok,
    required this.error,
    required this.name,
    required this.total,
    this.items,
  });

  final bool ok;
  final String error;

  /// 文件名（主区标题显示它）。
  final String name;
  final int total;
  final List<MusicTrack>? items;

  static const unavailable = MediaPlaylistReply(
    ok: false,
    error: '桥接不可用',
    name: '',
    total: 0,
  );
}

/// 弹「打开」选一份播放列表（`.m3u/.m3u8/.pls`）；取消 / 不可用 ⇒ 空串。
String pickMediaPlaylist() {
  final raw = _mediaCall0('lunac_media_pick_playlist');
  if (raw == null) {
    return '';
  }
  try {
    final value = jsonDecode(raw);
    if (value is Map && value['ok'] == true && value['path'] is String) {
      return (value['path'] as String).trim();
    }
  } catch (_) {
    // 坏 JSON ⇒ 当作取消。
  }
  return '';
}

/// 读一份播放列表文件的曲目（**不入库**，可以指向没添加过的目录）。
MediaPlaylistReply readMediaPlaylist(String path) {
  final raw = _mediaCallText('lunac_media_playlist', path);
  if (raw == null) {
    return MediaPlaylistReply.unavailable;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map) {
      return MediaPlaylistReply.unavailable;
    }
    final items = value['items'];
    return MediaPlaylistReply(
      ok: value['ok'] == true,
      error: value['error'] is String ? value['error'] as String : '',
      name: value['name'] is String ? value['name'] as String : '',
      total: value['total'] is int ? value['total'] as int : 0,
      items: items is List
          ? items
                .map((item) => MusicTrack.fromJson(item as Map))
                .whereType<MusicTrack>()
                .toList(growable: false)
          : const [],
    );
  } catch (_) {
    return MediaPlaylistReply.unavailable;
  }
}

/// 在资源管理器里定位一个文件（v1 右键菜单的「在资源管理器中显示」）。
bool revealMediaPath(String path) {
  final library = openLunacNative().library;
  if (library == null) {
    return false;
  }
  final arg = path.toNativeUtf8();
  try {
    final code = library
        .lookupFunction<_RevealC, _RevealDart>('lunac_reveal_path')(arg);
    return code == 0;
  } catch (_) {
    return false;
  } finally {
    malloc.free(arg);
  }
}

MediaRootsReply _parseRoots(String? raw) {
  if (raw == null) {
    return MediaRootsReply.unavailable;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map) {
      return MediaRootsReply.unavailable;
    }
    final roots = value['roots'];
    return MediaRootsReply(
      ok: value['ok'] == true,
      error: value['error'] is String ? value['error'] as String : '',
      roots: roots is List
          ? roots
                .map(MediaRoot.fromJson)
                .whereType<MediaRoot>()
                .toList(growable: false)
          : const [],
    );
  } catch (_) {
    return MediaRootsReply.unavailable;
  }
}

MediaActionReply _parseAction(String? raw) {
  if (raw == null) {
    return MediaActionReply.unavailable;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map) {
      return MediaActionReply.unavailable;
    }
    return MediaActionReply(
      ok: value['ok'] == true,
      error: value['error'] is String ? value['error'] as String : '',
      status: value['status'] == null
          ? null
          : MediaScanStatus.fromJson(value['status']),
    );
  } catch (_) {
    return MediaActionReply.unavailable;
  }
}

String? _mediaCall0(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_MediaNoArgC, _MediaNoArgDart>(symbol)();
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null) { _free(library, raw); }
  }
}

String? _mediaCallText(String symbol, String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  final arg = text.toNativeUtf8();
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_MediaTextC, _MediaTextDart>(symbol)(arg);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null) { _free(library, raw); }
    malloc.free(arg);
  }
}

String? _mediaCallTracks(String root, String query, int limit) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  final a = root.toNativeUtf8();
  final b = query.toNativeUtf8();
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_MediaTracksC, _MediaTracksDart>(
      'lunac_media_tracks',
    )(a, b, limit);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (raw != null) { _free(library, raw); }
    malloc.free(a);
    malloc.free(b);
  }
}

typedef _MediaNoArgC = Pointer<Utf8> Function();
typedef _MediaNoArgDart = Pointer<Utf8> Function();
typedef _MediaTextC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _MediaTextDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _MediaTracksC = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Int32,
);
typedef _MediaTracksDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  int,
);
typedef _RevealC = Int32 Function(Pointer<Utf8>);
typedef _RevealDart = int Function(Pointer<Utf8>);

// ── 播放引擎（Rust 宿主）─────────────────────────────────────────

/// 面板依赖的最小播放接口；单测注入假实现，避免在测试进程里建定时器 + 调 FFI。
abstract class MusicAudioEngine {
  Stream<Duration> get positionStream;
  Stream<Duration?> get durationStream;
  Stream<bool> get playingStream;

  /// 一首放完了（用于自动续播）。
  Stream<void> get completedStream;

  /// 打开一个本地文件，返回真实时长（拿不到时 null）。
  Future<Duration?> open(String path);
  Future<void> play();
  Future<void> pause();
  Future<void> stop();
  Future<void> seek(Duration position);
  Future<void> setVolume(double volume);
  Future<void> dispose();
}

/// 默认实现：Rust 宿主（`rust/src/player.rs`：rodio 解码 + cpal 出声 + 调音链）。
///
/// 接口是**流**、Rust 侧是**轮询**（`lunac_player_status`），两者用一条 250ms 的
/// 定时器接起来。不做反向回调（`NativeCallable` + 常驻端口）的理由：位置 / 时长 /
/// 播放态的消费方只有界面，一个进度条不值得多一条跨语言线程模型。
class RustMusicEngine implements MusicAudioEngine {
  RustMusicEngine({this.pollInterval = const Duration(milliseconds: 250)}) {
    _ticker = Timer.periodic(pollInterval, (_) => unawaited(_poll()));
  }

  final Duration pollInterval;

  final _position = StreamController<Duration>.broadcast();
  final _duration = StreamController<Duration?>.broadcast();
  final _playing = StreamController<bool>.broadcast();
  final _completed = StreamController<void>.broadcast();

  Timer? _ticker;
  PlayerState _last = PlayerState.idle;
  /// 记住当前这一首：放完之后再按播放要**重新装一次**（rodio 的队列已空）。
  String _path = '';
  double _volume = 0.8;
  bool _disposed = false;

  @override
  Stream<Duration> get positionStream => _position.stream;

  @override
  Stream<Duration?> get durationStream => _duration.stream;

  @override
  Stream<bool> get playingStream => _playing.stream;

  @override
  Stream<void> get completedStream => _completed.stream;

  @override
  Future<Duration?> open(String path) async {
    // 先把音量交给 Rust：起播那一刻它就按这份值设好，晚一步会有一帧满音量。
    playerVolume(_volume);
    final reply = await playerOpen(path, autoplay: false);
    if (!reply.ok) {
      throw PlayerFailure(reply.error.isEmpty ? 'Rust 侧没有给出原因' : reply.error);
    }
    _path = path;
    final state = reply.state ?? PlayerState.idle;
    _last = state;
    _emitAll(state, previous: PlayerState.idle, completed: false, force: true);
    return state.duration == Duration.zero ? null : state.duration;
  }

  @override
  Future<void> play() async {
    var reply = playerResume();
    // 放完 / 被 stop 过之后队列是空的：`resume` 只会白白失败，这时重新装一次。
    if (!reply.ok && _path.isNotEmpty) {
      reply = await playerOpen(_path, autoplay: true);
    }
    if (!reply.ok) {
      throw PlayerFailure(reply.error.isEmpty ? 'Rust 侧没有给出原因' : reply.error);
    }
    final state = reply.state;
    if (state != null) {
      _emitAll(state, previous: _last, completed: false);
      _last = state;
    }
  }

  @override
  Future<void> pause() {
    // 没有会话 / 已放完时 `pause` 会回 `ok=false`；对界面而言是「已经不在响」，不是错。
    playerPause();
    return Future<void>.value();
  }

  @override
  Future<void> stop() {
    playerStop();
    _last = PlayerState.idle;
    return Future<void>.value();
  }

  @override
  Future<void> seek(Duration position) {
    final reply = playerSeek(position);
    if (!reply.ok) {
      throw PlayerFailure(reply.error.isEmpty ? 'Rust 侧没有给出原因' : reply.error);
    }
    final state = reply.state;
    if (state != null) {
      _emitAll(state, previous: _last, completed: false);
      _last = state;
    }
    return Future<void>.value();
  }

  @override
  Future<void> setVolume(double volume) {
    _volume = volume.clamp(0.0, 1.0);
    playerVolume(_volume);
    return Future<void>.value();
  }

  @override
  Future<void> dispose() async {
    _disposed = true;
    _ticker?.cancel();
    _ticker = null;
    playerStop();
    await _position.close();
    await _duration.close();
    await _playing.close();
    await _completed.close();
  }

  Future<void> _poll() async {
    if (_disposed) {
      return;
    }
    final reply = playerStatus();
    final state = reply.state;
    if (state == null) {
      return;
    }
    final previous = _last;
    _last = state;
    // 「刚放完」= 上一轮还在、这一轮没了、而 path 还在（Rust 用这个组合表达结束）。
    final completed =
        previous.active && !state.active && state.path.isNotEmpty;
    _emitAll(state, previous: previous, completed: completed);
  }

  /// 只在**变化**时推流（250ms 一推会让歌词滚动那层每帧都 setState）。
  void _emitAll(
    PlayerState state, {
    required PlayerState previous,
    required bool completed,
    bool force = false,
  }) {
    if (_disposed) {
      return;
    }
    if (force || state.position != previous.position) {
      _position.add(state.position);
    }
    if (force || state.playing != previous.playing) {
      _playing.add(state.playing);
    }
    if (state.active &&
        state.duration > Duration.zero &&
        (force || state.duration != previous.duration)) {
      _duration.add(state.duration);
    }
    if (completed) {
      _completed.add(null);
    }
  }
}
